use std::path::PathBuf;

use axum::Json;
use axum::http::StatusCode;
use axum::response::IntoResponse;
use serde_json::json;
use tracing::{error, info, warn};
use uuid::Uuid;

use super::{OpenMode, Opened, SessionRow, audio_language, register};
use crate::AppState;
use crate::debrid_outcomes::StreamRelease;
use crate::repositories::debrid_cache_files::{self as cache_files, CachedFile};
use crate::routes::admin::debrid_stream::StreamResolveBody;
use crate::routes::admin::streaming::{
    ChapterResponse, SessionResponse, build_audio_responses, progressive_url,
};
use crate::runtime_config::RuntimeConfig;
use crate::streaming::decision::{
    AudioMode, AudioTrackMeta, FileMeta, PlaybackDecision, PlaybackMethod, VideoMode, ViewerPrefs,
};
use crate::streaming::download_meter::DownloadMeter;
use crate::streaming::feed::StreamFeed;
use crate::streaming::session::{MediaType, PlaybackSession};

/// A release already complete on disk plays without asking kobload or the debrid host
/// for anything.
pub(super) async fn open(
    state: &AppState,
    runtime: &RuntimeConfig,
    user_id: Uuid,
    body: &StreamResolveBody,
    cached: CachedFile,
    mode: OpenMode,
) -> Result<Opened, axum::response::Response> {
    let session_id = Uuid::new_v4();
    let file_path = PathBuf::from(&cached.file_path);
    let filename = file_path
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_default();
    let filesize = cached.file_size;

    let probe_path = file_path.clone();
    let probe =
        match tokio::task::spawn_blocking(move || kob_ffmpeg::probe::probe(&probe_path)).await {
            Ok(Ok(p)) => p,
            Ok(Err(e)) => {
                error!(error = %e, "Failed to probe cached file");
                return Err(server_error(format!("probe failed: {e}")));
            }
            Err(e) => return Err(server_error(format!("probe task: {e}"))),
        };

    let row = SessionRow {
        session_id,
        user_id,
        release_title: body.release_title.clone(),
        debrid_provider: body.debrid_provider.clone(),
        debrid_provider_id: body.debrid_provider_id,
        info_hash: body.info_hash.clone(),
        file_id: body.file_id,
        quality: body.quality.clone(),
        filename: filename.clone(),
        filesize,
        cache_path: Some(cached.file_path.clone()),
        debrid_url: None,
        debrid_url_expires_at: None,
        chapters: probe
            .chapters
            .iter()
            .map(|ch| ChapterResponse {
                index: ch.index,
                title: ch.title.clone(),
                start_seconds: ch.start_seconds,
                end_seconds: ch.end_seconds,
            })
            .collect(),
        downloaded: true,
    };
    let _ = cache_files::touch(&state.pool, &cached.file_path).await;

    let duration = probe.format.duration_seconds.unwrap_or(0.0);
    let video = probe.video_streams.first();
    let video_codec = video.map(|v| v.codec.clone()).unwrap_or_default();
    let (width, height) = video
        .map(|v| (v.width as u32, v.height as u32))
        .unwrap_or((0, 0));
    let container = filename.rsplit('.').next().unwrap_or("mkv").to_lowercase();
    let mut audio_metas: Vec<AudioTrackMeta> = probe
        .audio_streams
        .iter()
        .map(|a| AudioTrackMeta {
            stream_index: a.index,
            codec: a.codec.clone(),
            channels: a.channels,
            language: a.language.clone(),
            title: a.title.clone(),
            is_default: a.is_default,
        })
        .collect();
    crate::streaming::presumed_audio::presume(
        &state.pool,
        super::title_of(body).as_ref(),
        &super::release_name(&body.release_title, &filename),
        &mut audio_metas,
    )
    .await;
    let preferred = crate::streaming::decision::preferred_audio(
        &audio_metas,
        body.preferred_audio_title.as_deref(),
        audio_language(body, runtime),
    );
    let caps = body.capabilities.clone().unwrap_or_default();
    let video_level = if crate::streaming::decision::h264_level_matters(&caps, &video_codec) {
        crate::ffmpeg::probe::video_level(&file_path).await
    } else {
        None
    };
    let file_meta = FileMeta {
        container: container.clone(),
        video_codec: video_codec.clone(),
        video_width: width,
        video_height: height,
        interlaced: video.is_some_and(|v| v.is_interlaced),
        dynamic_range: video
            .map(crate::streaming::decision::probed_dynamic_range)
            .unwrap_or_default(),
        video_level,
    };
    let decision = crate::streaming::decision::decide(
        &file_meta,
        &audio_metas,
        &[],
        &caps,
        ViewerPrefs {
            preferred_audio_index: preferred,
            preferred_subtitle_index: None,
            understood_language: audio_language(body, runtime),
            subtitle_language: "",
        },
        &state.encoder_caps.preferred_codecs(),
    );
    let audio_index = decision.audio_stream_index;
    let method = decision.method;
    let direct_play = method == PlaybackMethod::DirectPlay;
    info!(session_id = %session_id, file = %filename, method = ?decision.method, "Playing from local cache (no debrid)");
    let segment_dir = super::segment_dir(state, runtime, session_id);
    let transcode = if direct_play {
        None
    } else {
        Some(
            start_transcode(
                state,
                session_id,
                &file_path,
                &segment_dir,
                &decision,
                &video_codec,
                duration,
                body.start_time,
            )
            .await?,
        )
    };
    let media = match mode {
        OpenMode::Play => Some(row.commit(state, body).await?),
        OpenMode::Warm(_) => None,
    };

    let mut segment_cache =
        crate::streaming::segment_cache::SegmentCache::new(segment_dir, 6, duration);
    if let Some(st) = body.start_time {
        segment_cache.set_playback_start(segment_cache.time_to_segment(st), st);
    }
    let thumb_dir = crate::media_cache::debrid_file_cache_dir(&file_path).join("trickplay");
    let has_thumbnails = tokio::fs::try_exists(thumb_dir.join("thumbnails.vtt"))
        .await
        .unwrap_or(false);
    let audio_loudness = media
        .as_ref()
        .map(|c| c.audio_loudness.clone())
        .unwrap_or_default();
    let audio_tracks = build_audio_responses(&audio_metas, audio_index, &audio_loudness);

    let meter = DownloadMeter::new(filesize.max(0) as u64, filesize.max(0) as u64);
    tokio::spawn(super::subtitles::name_once_head_arrives(
        state.clone(),
        session_id,
        file_path.clone(),
        meter.clone(),
        duration,
    ));
    // Ranges analysis or an earlier play found broken in this download, transcoded
    // ahead of the playhead as a library file's are.
    let corrupt_ranges =
        crate::repositories::debrid_files::corrupt_ranges_at(&state.pool, &cached.file_path)
            .await
            .unwrap_or_default();
    state.playback_sessions.insert(
        session_id,
        PlaybackSession {
            id: session_id,
            user_id,
            media_type: MediaType::Movie,
            file_id: session_id,
            file_path,
            decision,
            transcode,
            segment_cache,
            created_at: std::time::Instant::now(),
            last_activity: std::time::Instant::now(),
            seek_generation: 0,
            duration_seconds: duration,
            container,
            video_codec,
            subtitle_files: vec![],
            hw_accel: (*state.hw_accel).clone(),
            source_resolution: (
                if width > 0 { width } else { 1920 },
                if height > 0 { height } else { 1080 },
            ),
            variants: None,
            is_live: false,
            provider_id: None,
            max_bitrate_kbps: None,
            max_height: None,
            keyframes: None,
            error: None,
            pending_segment: None,
            download_cancel: None,
            thumbnail_dir: Some(thumb_dir),
            debrid_cache_key: super::release_key(body.info_hash.as_deref(), body.file_id),
            audio_metas,
            file_meta: Some(file_meta.clone()),
            audio_loudness,
            preferred_audio_index: preferred,
            force_video_transcode: false,
            corrupt_ranges,
            chapters: row.chapters.clone(),
            overlay_spawned_starts: std::collections::HashSet::new(),
            highest_served_segment: None,
            feed: Some(StreamFeed::new()),
            download: Some(meter),
            cue_gate: None,
            persisted: false,
        },
    );
    if matches!(mode, OpenMode::Play) {
        register(state, body, session_id);
    }
    // A file cached before anything measured its bars, or whose measuring session is gone.
    tokio::spawn(super::picture::measure_once_probed(
        state.clone(),
        session_id,
        StreamRelease {
            debrid_provider_id: Some(row.debrid_provider_id),
            info_hash: row.info_hash.clone(),
            file_id: row.file_id,
            release_title: row.release_title.clone(),
            filename: row.filename.clone(),
        },
        cached.file_path.clone(),
        duration,
    ));

    let base_url = format!("/api/v1/admin/playback/sessions/{session_id}");
    let response = SessionResponse {
        session_id: session_id.to_string(),
        method,
        playlist_url: (!direct_play).then(|| format!("{base_url}/master.m3u8")),
        direct_url: direct_play.then(|| format!("/api/v1/admin/stream/{session_id}/play")),
        progressive_url: progressive_url(session_id, method, body.delivery),
        duration_seconds: duration,
        subtitles: vec![],
        audio_tracks,
        thumbnails_url: has_thumbnails.then(|| format!("{base_url}/thumbnails.vtt")),
        chapters: row.chapters.clone(),
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
        selected_audio_index: Some(audio_index),
        preferred_audio_stream_index: preferred,
        selected_subtitle_index: None,
        video_width: (width > 0).then_some(width),
        video_height: (height > 0).then_some(height),
    };
    Ok(Opened {
        session_id,
        response,
        row,
        remainder: None,
    })
}

fn server_error(message: String) -> axum::response::Response {
    (
        StatusCode::INTERNAL_SERVER_ERROR,
        Json(json!({ "error": message })),
    )
        .into_response()
}

/// Walk the encoder chain (primary, then the h264 fallbacks) until one survives startup.
#[allow(clippy::too_many_arguments)]
async fn start_transcode(
    state: &AppState,
    session_id: Uuid,
    file_path: &std::path::Path,
    segment_dir: &std::path::Path,
    decision: &PlaybackDecision,
    video_codec: &str,
    duration: f64,
    start_time: Option<f64>,
) -> Result<crate::ffmpeg::transcode::TranscodeProcess, axum::response::Response> {
    let needs_transcode = matches!(decision.video_mode, VideoMode::Transcode { .. });
    let primary = if let VideoMode::Transcode { target_codec } = &decision.video_mode {
        state
            .encoder_caps
            .encoder_for(target_codec)
            .unwrap_or(&state.encoder_caps.h264)
            .to_string()
    } else {
        state
            .encoder_caps
            .encoder_for(video_codec)
            .unwrap_or(&state.encoder_caps.h264)
            .to_string()
    };
    let mut chain = vec![primary.clone()];
    chain.extend(
        state
            .encoder_caps
            .h264_fallbacks()
            .into_iter()
            .map(String::from),
    );
    let ffmpeg_audio = match &decision.audio_mode {
        AudioMode::Copy => crate::ffmpeg::transcode::AudioMode::Copy,
        AudioMode::Transcode {
            target_codec,
            bitrate_kbps,
        } => crate::ffmpeg::transcode::AudioMode::Transcode {
            target_codec: target_codec.clone(),
            bitrate_kbps: *bitrate_kbps,
        },
    };

    let mut last_error = String::new();
    for encoder in &chain {
        if !last_error.is_empty() {
            let _ = tokio::fs::remove_dir_all(segment_dir).await;
        }
        let hw_device = if encoder == "libx264" {
            None
        } else if encoder.contains("vaapi")
            && matches!(*state.hw_accel, crate::ffmpeg::detect::HwAccel::Nvenc)
        {
            match state.vaapi_fallback_device {
                Some(ref dev) => Some(dev.clone()),
                None => {
                    warn!(session_id = %session_id, "No VAAPI fallback device available, skipping VAAPI encoder");
                    last_error = "No VAAPI fallback device available".to_string();
                    continue;
                }
            }
        } else {
            state.hw_device.get()
        };
        let args = crate::ffmpeg::transcode::TranscodeArgs {
            input_path: file_path.to_path_buf(),
            output_dir: segment_dir.to_path_buf(),
            video_mode: if needs_transcode {
                crate::ffmpeg::transcode::VideoMode::Transcode
            } else {
                crate::ffmpeg::transcode::VideoMode::Copy
            },
            audio_mode: ffmpeg_audio.clone(),
            audio_stream_index: decision.audio_stream_index,
            start_time,
            segment_duration: 6,
            hw_accel: (*state.hw_accel).clone(),
            video_codec: video_codec.to_string(),
            target_encoder: encoder.clone(),
            burn_in_subtitle: None,
            scale: None,
            target_bitrate_kbps: None,
            duration_seconds: duration,
            initial_generation: 0,
            hw_device,
            prior_runs: Vec::new(),
            deinterlace: decision.deinterlace,
            tone_map_hdr: decision.tone_map_hdr,
            h264_level: decision.h264_level,
        };
        match crate::ffmpeg::transcode::TranscodeProcess::start(args).await {
            Ok(mut process) => match process.survived_launch().await {
                Ok(()) => {
                    if encoder != &primary {
                        info!(session_id = %session_id, encoder = %encoder, "Encoder fallback succeeded");
                    }
                    return Ok(process);
                }
                Err(e) => {
                    warn!(session_id = %session_id, encoder = %encoder, error = %e, "Encoder crashed immediately, trying fallback");
                    process.kill().await;
                    last_error = e;
                }
            },
            Err(e) => {
                warn!(session_id = %session_id, encoder = %encoder, error = %e, "Failed to start encoder, trying fallback");
                last_error = e.to_string();
            }
        }
    }
    error!(session_id = %session_id, error = %last_error, "All encoders failed for cached file");
    Err(server_error(format!("transcode failed: {last_error}")))
}
