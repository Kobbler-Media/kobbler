use std::path::PathBuf;

use axum::Json;
use axum::http::StatusCode;
use axum::response::IntoResponse;
use serde_json::json;
use tokio_util::sync::CancellationToken;
use tracing::{info, warn};
use uuid::Uuid;

use super::prepare::{self, Prepare};
use super::upgrade;
use super::{OpenMode, Opened, SessionRow, VIDEO_CONTAINERS, audio_language, register};
use crate::AppState;
use crate::debrid_outcomes::{self, StreamRelease};
use crate::repositories::debrid_cache_files::CacheRelease;
use crate::routes::admin::debrid_errors::StreamFailure;
use crate::routes::admin::debrid_stream::StreamResolveBody;
use crate::routes::admin::streaming::{SessionResponse, progressive_url};
use crate::runtime_config::RuntimeConfig;
use crate::streaming::cache_download;
use crate::streaming::decision::PlaybackMethod;
use crate::streaming::download_lane::DownloadLane;
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
    let priority = match mode {
        OpenMode::Play => nats_jobs::StreamPriority::User,
        OpenMode::Warm(_) => nats_jobs::StreamPriority::Warm,
    };
    let resolved = match body.resolved.clone() {
        Some(resolved) => resolved,
        None => super::pin::resolve(state, runtime, body, priority).await?,
    };

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

    // An NZB is only named, and a pack's file only picked, once kobload has imported it.
    let keyed;
    let learned_key = resolved
        .release_key
        .clone()
        .filter(|_| body.release_key.is_none());
    let learned_file = resolved.file_id.filter(|_| body.file_id.is_none());
    let body = match (learned_key, learned_file) {
        (None, None) => body,
        (key, file_id) => {
            keyed = StreamResolveBody {
                release_key: body.release_key.clone().or(key),
                file_id: body.file_id.or(file_id),
                resolved: None,
                ..body.clone()
            };
            if let Some(cached) = super::complete_cache_entry(
                state,
                keyed.debrid_provider_id,
                keyed.release_key.as_deref(),
                keyed.file_id,
            )
            .await
            {
                return super::local::open(state, runtime, user_id, &keyed, cached, mode).await;
            }
            &keyed
        }
    };

    let release = StreamRelease {
        debrid_provider_id: Some(body.debrid_provider_id),
        release_key: body.release_key.clone(),
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
    let release_key = super::release_key(body.release_key.as_deref(), body.file_id);
    // Keyed by release file so another session for the same file finds the bytes.
    let Some(local_cache_path) = cache_download::cache_file(
        &cache_download::cache_dir(state, runtime),
        release_key
            .clone()
            .unwrap_or_else(|| session_id.to_string())
            .as_str(),
        &resolved.filename,
    ) else {
        warn!(release_key = ?release_key, filename = ?resolved.filename, "Refused an unsafe cache path");
        let failure = StreamFailure::NotVideo;
        debrid_outcomes::report_failure(state, &release, failure.code()).await;
        return Err(failure.response(
            StatusCode::UNPROCESSABLE_ENTITY,
            "The release's key or filename cannot name a cache file",
        ));
    };
    let row = SessionRow {
        session_id,
        user_id,
        release_title: body.release_title.clone(),
        debrid_provider: body.debrid_provider.clone(),
        debrid_provider_id: body.debrid_provider_id,
        release_key: body.release_key.clone(),
        nzb_ref: body.nzb_ref.clone(),
        file_id: body.file_id,
        quality: body.quality.clone(),
        filename: resolved.filename.clone(),
        filesize: resolved.filesize,
        cache_path: Some(local_cache_path.to_string_lossy().to_string()),
        debrid_url: Some(resolved.stream_url.clone()).filter(|u| !u.is_empty()),
        debrid_cache_url: resolved.cache_url.clone(),
        debrid_url_expires_at: resolved.expires_at.as_deref().and_then(|s| {
            time::OffsetDateTime::parse(s, &time::format_description::well_known::Rfc3339).ok()
        }),
        chapters: Vec::new(),
        downloaded: false,
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
    // A Warm session stops its download at the head, which would stop the shared one.
    let joined = match mode {
        OpenMode::Play => release_key
            .as_deref()
            .and_then(|k| state.downloads_ahead.join(k)),
        OpenMode::Warm(_) => None,
    };
    // Seeded with what is already on disk so HybridIo reads it from there.
    let download_meter = joined
        .as_ref()
        .map(|j| j.meter.clone())
        .unwrap_or_else(|| DownloadMeter::new(resolved.filesize.max(0) as u64, on_disk));
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

    // A warm session is speculative: everything it reads stays out of the stream-first reserve.
    let probe_url = match (&mode, &resolved.cache_url) {
        (OpenMode::Warm(_), Some(url)) if !url.is_empty() => url.clone(),
        _ => resolved.stream_url.clone(),
    };

    let download_cancel = CancellationToken::new();
    // Ranges analysis or an earlier play found broken in this download, transcoded
    // ahead of the playhead as a library file's are.
    let corrupt_ranges = crate::repositories::debrid_files::corrupt_ranges_at(
        &state.pool,
        &local_cache_path.to_string_lossy(),
    )
    .await
    .unwrap_or_default();
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
                    target_codec: crate::streaming::decision::aac_encoder().to_string(),
                    bitrate_kbps: 192,
                },
                audio_stream_index: 0,
                subtitle_handling: crate::streaming::decision::SubtitleHandling::None,
                deinterlace: false,
                tone_map_hdr: false,
                h264_level: None,
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
            max_height: None,
            keyframes: None,
            error: None,
            pending_segment: None,
            download_cancel: Some(download_cancel.clone()),
            thumbnail_dir: None,
            debrid_cache_key: release_key.clone(),
            audio_metas: vec![],
            file_meta: None,
            audio_loudness: media
                .as_ref()
                .map(|c| c.audio_loudness.clone())
                .unwrap_or_default(),
            preferred_audio_index: None,
            force_video_transcode: false,
            corrupt_ranges,
            chapters: Vec::new(),
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
            release_key: release_key.clone(),
            filename: &resolved.filename,
            filesize: resolved.filesize,
            local: Some(&local_cache_path),
            url: Some(probe_url.clone()),
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
        progressive_url: progressive_url(session_id, initial_method, body.delivery),
        duration_seconds: estimated_duration,
        subtitles: vec![],
        audio_tracks: vec![],
        thumbnails_url: None,
        chapters: vec![],
        // Direct play has no server-side seek; the client resumes via its own position.
        start_time: if direct_play { None } else { body.start_time },
        media_id: media.as_ref().map(|c| c.media_id.to_string()),
        skip_segments: media
            .as_ref()
            .map(|c| c.skip_segments.clone())
            .unwrap_or_default(),
        picture: media.as_ref().and_then(|c| c.picture),
        media_type: media.map(|c| c.media_type),
        file_id: None,
        selected_audio_index: None,
        preferred_audio_stream_index: None,
        selected_subtitle_index: None,
        video_width: None,
        video_height: None,
    };

    let target = DownloadTarget {
        session_id,
        url: resolved
            .cache_url
            .clone()
            .unwrap_or_else(|| resolved.stream_url.clone()),
        cache_path: local_cache_path.clone(),
        filesize: resolved.filesize,
        release: CacheRelease {
            debrid_provider_id: Some(body.debrid_provider_id),
            release_key: body.release_key.clone(),
            file_id: body.file_id,
            filename: resolved.filename.clone(),
        },
        lane: match mode {
            OpenMode::Play => DownloadLane::viewer(),
            OpenMode::Warm(_) => DownloadLane::warm(resolved.cache_url.is_some()),
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
    if let Some(joined) = joined {
        info!(%session_id, "joined the download ahead of this release");
        upgrade::for_joined(state, body, session_id, joined, resolved.clone());
    } else {
        if let Some(key) = &release_key {
            state.downloads_ahead.hand_over(key).await;
        }
        cache_download::spawn(state, target, download_cancel, download_meter.clone());
    }

    tokio::spawn(prepare::run(Prepare {
        state: state.clone(),
        session_id,
        release,
        capabilities: body.capabilities.clone().unwrap_or_default(),
        probe_url,
        container,
        segment_dir,
        local_cache_path,
        download_meter,
        start_time: body.start_time,
        audio_language: audio_language(body, runtime).to_string(),
        audio_title: body.preferred_audio_title.clone(),
        title: super::title_of(body),
        warm,
    }));

    Ok(Opened {
        session_id,
        response,
        row,
        remainder,
    })
}
