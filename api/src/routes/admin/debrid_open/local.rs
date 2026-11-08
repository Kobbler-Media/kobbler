use std::path::PathBuf;

use axum::Json;
use axum::http::StatusCode;
use axum::response::IntoResponse;
use serde_json::json;
use tracing::{error, info, warn};
use uuid::Uuid;

use super::{OpenMode, Opened, SessionRow, audio_language, register};
use crate::AppState;
use crate::repositories::debrid_cache_files::{self as cache_files, CachedFile};
use crate::routes::admin::debrid_stream::StreamResolveBody;
use crate::routes::admin::streaming::{AudioTrackResponse, SessionResponse};
use crate::runtime_config::RuntimeConfig;
use crate::streaming::decision::{
    AAC_ENCODER, AudioMode, AudioTrackMeta, PlaybackDecision, PlaybackMethod, SubtitleHandling,
    VideoMode,
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
    };
    let _ = cache_files::touch(&state.pool, &cached.file_path).await;

    let duration = probe.format.duration_seconds.unwrap_or(0.0);
    let video = probe.video_streams.first();
    let video_codec = video.map(|v| v.codec.clone()).unwrap_or_default();
    let (width, height) = video
        .map(|v| (v.width as u32, v.height as u32))
        .unwrap_or((0, 0));
    let container = filename.rsplit('.').next().unwrap_or("mkv").to_lowercase();
    let audio_metas: Vec<AudioTrackMeta> = probe
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
    let preferred = crate::streaming::decision::preferred_audio(
        &audio_metas,
        body.preferred_audio_title.as_deref(),
        audio_language(body, runtime),
    );
    let audio_index =
        preferred.unwrap_or_else(|| probe.audio_streams.first().map(|a| a.index).unwrap_or(0));
    let audio_track = audio_metas.iter().find(|a| a.stream_index == audio_index);

    let deinterlace = video.is_some_and(|v| v.is_interlaced)
        && body
            .capabilities
            .as_ref()
            .is_some_and(|caps| caps.supports_interlaced == Some(false));
    let dynamic_range = video
        .map(crate::streaming::decision::probed_dynamic_range)
        .unwrap_or_default();
    let caps = body.capabilities.clone().unwrap_or_default();
    let displays = caps.displays(dynamic_range);
    let direct_play = !deinterlace
        && displays
        && body.capabilities.as_ref().is_some_and(|caps| {
            crate::streaming::decision::is_container_compatible(&container, &caps.containers)
        });
    let segment_dir = super::segment_dir(state, runtime, session_id);
    let (decision, transcode) = if direct_play {
        info!(session_id = %session_id, file = %filename, "Direct play from local cache (no debrid, no transcode)");
        (
            PlaybackDecision {
                method: PlaybackMethod::DirectPlay,
                video_mode: VideoMode::Copy,
                audio_mode: AudioMode::Copy,
                audio_stream_index: audio_index,
                subtitle_handling: SubtitleHandling::None,
                deinterlace: false,
                tone_map_hdr: false,
            },
            None,
        )
    } else {
        info!(session_id = %session_id, file = %filename, "Playing from local cache (no debrid)");
        let needs_transcode =
            deinterlace || !displays || video_codec == "hevc" || video_codec == "av1";
        let audio_mode = if audio_track.is_some_and(|a| a.codec == "aac") {
            AudioMode::Copy
        } else {
            AudioMode::Transcode {
                target_codec: AAC_ENCODER.to_string(),
                bitrate_kbps: crate::streaming::decision::suggest_audio_bitrate(audio_track),
            }
        };
        let method = if needs_transcode {
            PlaybackMethod::VideoTranscode
        } else if matches!(audio_mode, AudioMode::Transcode { .. }) {
            PlaybackMethod::AudioTranscode
        } else {
            PlaybackMethod::Remux
        };
        let decision = PlaybackDecision {
            method,
            video_mode: if needs_transcode {
                VideoMode::Transcode {
                    target_codec: "h264".to_string(),
                }
            } else {
                VideoMode::Copy
            },
            audio_mode,
            audio_stream_index: audio_index,
            subtitle_handling: SubtitleHandling::None,
            deinterlace,
            tone_map_hdr: needs_transcode && !caps.displays_transcode_of(dynamic_range),
        };
        let process = start_transcode(
            state,
            session_id,
            &file_path,
            &segment_dir,
            &decision,
            &video_codec,
            duration,
            body.start_time,
        )
        .await?;
        (decision, Some(process))
    };
    let method = decision.method;
    let media = match mode {
        OpenMode::Play => Some(row.commit(state, body).await?),
        OpenMode::Warm(_) => None,
    };

    let mut segment_cache =
        crate::streaming::segment_cache::SegmentCache::new(segment_dir, 6, duration);
    if let Some(st) = body.start_time {
        segment_cache.set_playback_start(segment_cache.time_to_segment(st), st);
    }
    let thumb_dir = file_path
        .parent()
        .unwrap_or(&file_path)
        .join("media_cache")
        .join("trickplay");
    let has_thumbnails = thumb_dir.join("thumbnails.vtt").exists();
    let audio_tracks: Vec<AudioTrackResponse> = probe
        .audio_streams
        .iter()
        .map(|a| AudioTrackResponse {
            stream_index: a.index,
            codec: a.codec.clone(),
            channels: a.channels,
            language: a.language.clone(),
            title: a.title.clone(),
            is_default: a.index == audio_index,
        })
        .collect();

    let meter = DownloadMeter::new(filesize.max(0) as u64, filesize.max(0) as u64);
    tokio::spawn(super::subtitles::name_once_head_arrives(
        state.clone(),
        session_id,
        file_path.clone(),
        meter.clone(),
        duration,
    ));
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
            preferred_audio_index: preferred,
            force_video_transcode: false,
            corrupt_ranges: Vec::new(),
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

    let base_url = format!("/api/v1/admin/playback/sessions/{session_id}");
    let response = SessionResponse {
        session_id: session_id.to_string(),
        method,
        playlist_url: (!direct_play).then(|| format!("{base_url}/master.m3u8")),
        direct_url: direct_play.then(|| format!("/api/v1/admin/stream/{session_id}/play")),
        duration_seconds: duration,
        subtitles: vec![],
        audio_tracks,
        thumbnails_url: has_thumbnails.then(|| format!("{base_url}/thumbnails.vtt")),
        lufs: None,
        chapters: vec![],
        skip_segments: vec![],
        // Direct play has no server-side seek; the client resumes via its own position.
        start_time: if direct_play { None } else { body.start_time },
        media_id: media.as_ref().map(|(id, _)| id.to_string()),
        media_type: media.map(|(_, t)| t),
        file_id: None,
        selected_audio_index: Some(audio_index),
        preferred_audio_stream_index: preferred,
        selected_subtitle_index: None,
        video_width: (width > 0).then_some(width),
        video_height: (height > 0).then_some(height),
        picture: None,
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
    let primary = if needs_transcode {
        state.encoder_caps.h264.clone()
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
