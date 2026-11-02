use std::path::PathBuf;

use axum::Json;
use axum::http::StatusCode;
use axum::response::IntoResponse;
use serde_json::json;
use tokio_util::sync::CancellationToken;
use tracing::{error, info, warn};
use uuid::Uuid;

use super::prepare::{self, Prepare};
use super::{OpenMode, Opened, SessionRow, VIDEO_CONTAINERS, audio_language, register};
use crate::AppState;
use crate::debrid_outcomes::{self, StreamRelease};
use crate::repositories::debrid_cache_files::CacheRelease;
use crate::routes::admin::debrid_errors::StreamFailure;
use crate::routes::admin::debrid_stream::StreamResolveBody;
use crate::routes::admin::streaming::SessionResponse;
use crate::routes::helpers::build_kobload_client;
use crate::runtime_config::RuntimeConfig;
use crate::streaming::cache_download;
use crate::streaming::decision::{AAC_ENCODER, PlaybackMethod};
use crate::streaming::download_meter::DownloadMeter;
use crate::streaming::feed::StreamFeed;
use crate::streaming::progressive::DownloadTarget;

/// How much of the file a warm session downloads before it waits to be claimed: enough
/// for the container header and the first seconds of the picture.
pub(crate) const WARM_HEAD_BYTES: u64 = 8 * 1024 * 1024;

pub(super) async fn open(
    state: &AppState,
    runtime: &RuntimeConfig,
    user_id: Uuid,
    body: &StreamResolveBody,
    mode: OpenMode,
) -> Result<Opened, axum::response::Response> {
    let client = build_kobload_client(runtime).map_err(IntoResponse::into_response)?;
    let priority = match mode {
        OpenMode::Play => nats_jobs::StreamPriority::User,
        OpenMode::Warm(_) => nats_jobs::StreamPriority::Warm,
    };
    let resolve_req = nats_jobs::StreamResolveRequest {
        debrid_provider_id: body.debrid_provider_id,
        info_hash: body.info_hash.clone(),
        magnet_url: body.magnet_url.clone(),
        nzb_url: body.nzb_url.clone(),
        file_id: body.file_id,
        priority: Some(priority),
        release_title: Some(body.release_title.clone()).filter(|t| !t.is_empty()),
        file_name: None,
    };
    let resolved = client.stream_resolve(&resolve_req).await.map_err(|e| {
        error!(error = %e, "stream resolve failed");
        StreamFailure::from_kobload(&e).response(StatusCode::BAD_GATEWAY, &e.to_string())
    })?;

    if !resolved.cached {
        info!(torrent_id = ?resolved.torrent_id, "Stream not cached, returning caching status for client polling");
        return Err(Json(json!({
            "session_id": serde_json::Value::Null,
            "status": "caching",
            "debrid_provider_id": body.debrid_provider_id,
            "torrent_id": resolved.torrent_id,
        }))
        .into_response());
    }

    let release = StreamRelease {
        debrid_provider_id: Some(body.debrid_provider_id),
        info_hash: body.info_hash.clone(),
        file_id: body.file_id,
        release_title: body.release_title.clone(),
        filename: resolved.filename.clone(),
    };
    let container = resolved
        .filename
        .rsplit('.')
        .next()
        .unwrap_or("mkv")
        .to_lowercase();
    // A fake release (.exe and the like) fails here so the client walks on to the next.
    if !VIDEO_CONTAINERS.contains(&container.as_str()) {
        warn!(filename = %resolved.filename, container = %container, "Rejected non-video file from debrid resolve");
        let failure = StreamFailure::NotVideo;
        debrid_outcomes::report_failure(state, &release, failure.code()).await;
        return Err(failure.response(
            StatusCode::UNPROCESSABLE_ENTITY,
            &format!("Resolved file is not a video (.{container})"),
        ));
    }

    let session_id = Uuid::new_v4();
    let session_key = session_id.to_string();
    // Keyed by info_hash so another session for the same release finds the bytes.
    let local_cache_path = cache_download::cache_dir(state, runtime)
        .join(body.info_hash.as_deref().unwrap_or(&session_key))
        .join(&resolved.filename);
    let row = SessionRow {
        session_id,
        user_id,
        release_title: body.release_title.clone(),
        debrid_provider: body.debrid_provider.clone(),
        debrid_provider_id: body.debrid_provider_id,
        info_hash: body.info_hash.clone(),
        file_id: body.file_id,
        quality: body.quality.clone(),
        filename: resolved.filename.clone(),
        filesize: resolved.filesize,
        cache_path: Some(local_cache_path.to_string_lossy().to_string()),
        debrid_url: Some(resolved.stream_url.clone()).filter(|u| !u.is_empty()),
        debrid_url_expires_at: resolved.expires_at.as_deref().and_then(|s| {
            time::OffsetDateTime::parse(s, &time::format_description::well_known::Rfc3339).ok()
        }),
    };
    let media = match mode {
        OpenMode::Play => Some(row.commit(state, body).await?),
        OpenMode::Warm(_) => None,
    };
    info!(session_id = %session_id, filename = %resolved.filename, "created debrid stream session");

    let estimated_duration = body.estimated_duration.unwrap_or(0.0);
    let segment_dir = super::segment_dir(state, runtime, session_id);
    let mut segment_cache = crate::streaming::segment_cache::SegmentCache::new(
        segment_dir.clone(),
        6,
        estimated_duration,
    );
    if let Some(st) = body.start_time {
        segment_cache.set_playback_start(segment_cache.time_to_segment(st), st);
    }

    let on_disk = tokio::fs::metadata(&local_cache_path)
        .await
        .map_or(0, |m| m.len());
    // Seeded with what is already on disk so HybridIo reads it from there.
    let download_meter = DownloadMeter::new(resolved.filesize.max(0) as u64, on_disk);
    if download_meter.is_complete() {
        info!(session_id = %session_id, "Using completed cached file");
    } else if on_disk > 0 {
        info!(session_id = %session_id, cached_mb = on_disk / (1024 * 1024), "Resuming from partial cache");
    }

    // Optimistic: a client whose capabilities include the container is assumed to direct
    // play; the background probe refines this once the codecs are known.
    let initial_method = match &body.capabilities {
        Some(caps)
            if crate::streaming::decision::is_container_compatible(
                &container,
                &caps.containers,
            ) =>
        {
            PlaybackMethod::DirectPlay
        }
        _ => PlaybackMethod::Remux,
    };

    let download_cancel = CancellationToken::new();
    let release_key = super::release_key(body.info_hash.as_deref(), body.file_id);
    state.playback_sessions.insert(
        session_id,
        crate::streaming::session::PlaybackSession {
            id: session_id,
            user_id,
            media_type: crate::streaming::session::MediaType::Movie,
            file_id: session_id,
            file_path: PathBuf::new(),
            decision: crate::streaming::decision::PlaybackDecision {
                method: initial_method,
                video_mode: crate::streaming::decision::VideoMode::Copy,
                audio_mode: crate::streaming::decision::AudioMode::Transcode {
                    target_codec: AAC_ENCODER.to_string(),
                    bitrate_kbps: 192,
                },
                audio_stream_index: 0,
                subtitle_handling: crate::streaming::decision::SubtitleHandling::None,
                deinterlace: false,
            },
            transcode: None,
            segment_cache,
            created_at: std::time::Instant::now(),
            last_activity: std::time::Instant::now(),
            seek_generation: 0,
            duration_seconds: estimated_duration,
            container: container.clone(),
            video_codec: String::new(),
            subtitle_files: vec![],
            hw_accel: (*state.hw_accel).clone(),
            source_resolution: (1920, 1080),
            variants: None,
            is_live: false,
            provider_id: None,
            max_bitrate_kbps: None,
            keyframes: None,
            error: None,
            pending_segment: None,
            download_cancel: Some(download_cancel.clone()),
            thumbnail_dir: None,
            debrid_cache_key: release_key,
            audio_metas: vec![],
            preferred_audio_index: None,
            force_video_transcode: false,
            corrupt_ranges: Vec::new(),
            overlay_spawned_starts: std::collections::HashSet::new(),
            highest_served_segment: None,
            feed: Some(StreamFeed::new()),
            download: Some(download_meter.clone()),
            cue_gate: None,
            persisted: false,
        },
    );
    if matches!(mode, OpenMode::Play) {
        register(state, body, session_id);
    }
    crate::routes::admin::debrid_play::session_cue_gate(
        state,
        session_id,
        crate::routes::admin::debrid_play::GateSpec {
            info_hash: body.info_hash.as_deref(),
            filename: &resolved.filename,
            filesize: resolved.filesize,
            local: Some(&local_cache_path),
            url: Some(resolved.stream_url.clone()),
        },
    )
    .await;

    let direct_play = initial_method == PlaybackMethod::DirectPlay;
    let response = SessionResponse {
        session_id: session_id.to_string(),
        method: initial_method,
        playlist_url: (!direct_play)
            .then(|| format!("/api/v1/admin/playback/sessions/{session_id}/master.m3u8")),
        direct_url: direct_play.then(|| format!("/api/v1/admin/stream/{session_id}/play")),
        duration_seconds: estimated_duration,
        subtitles: vec![],
        audio_tracks: vec![],
        thumbnails_url: None,
        lufs: None,
        chapters: vec![],
        skip_segments: vec![],
        // Direct play has no server-side seek; the client resumes via its own position.
        start_time: if direct_play { None } else { body.start_time },
        media_id: media.as_ref().map(|(id, _)| id.to_string()),
        media_type: media.map(|(_, t)| t),
        file_id: None,
        selected_audio_index: None,
        preferred_audio_stream_index: None,
        selected_subtitle_index: None,
        video_width: None,
        video_height: None,
        picture: None,
    };

    let target = DownloadTarget {
        session_id,
        url: resolved.stream_url.clone(),
        cache_path: local_cache_path.clone(),
        filesize: resolved.filesize,
        release: CacheRelease {
            debrid_provider_id: Some(body.debrid_provider_id),
            info_hash: body.info_hash.clone(),
            file_id: body.file_id,
            filename: resolved.filename.clone(),
        },
    };
    let (warm, remainder) = match mode {
        OpenMode::Play => (None, None),
        OpenMode::Warm(gate) => {
            cache_download::stop_after_head(
                download_meter.clone(),
                WARM_HEAD_BYTES,
                download_cancel.clone(),
            );
            (Some(gate), Some(target.clone()))
        }
    };
    cache_download::spawn(state, target, download_cancel, download_meter.clone());

    tokio::spawn(prepare::run(Prepare {
        state: state.clone(),
        session_id,
        release,
        capabilities: body.capabilities.clone().unwrap_or_default(),
        stream_url: resolved.stream_url,
        container,
        segment_dir,
        local_cache_path,
        download_meter,
        start_time: body.start_time,
        audio_language: audio_language(body, runtime).to_string(),
        audio_title: body.preferred_audio_title.clone(),
        warm,
    }));

    Ok(Opened {
        session_id,
        response,
        row,
        remainder,
    })
}
