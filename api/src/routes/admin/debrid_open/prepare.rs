use std::path::PathBuf;
use std::sync::Arc;

use tracing::{error, info, warn};
use uuid::Uuid;

use super::WarmGate;
use crate::AppState;
use crate::debrid_outcomes::{self, StreamRelease};
use crate::routes::admin::debrid_stream::probe_sidecar_path;
use crate::streaming::decision::{ClientCapabilities, PlaybackMethod, ViewerPrefs};
use crate::streaming::download_meter::DownloadMeter;
use crate::streaming::session::PlaybackSession;

pub(super) struct Prepare {
    pub state: AppState,
    pub session_id: Uuid,
    pub release: StreamRelease,
    pub capabilities: ClientCapabilities,
    pub stream_url: String,
    pub container: String,
    pub segment_dir: PathBuf,
    pub local_cache_path: PathBuf,
    pub download_meter: Arc<DownloadMeter>,
    pub start_time: Option<f64>,
    pub audio_language: String,
    /// A warm session probes and decides, then holds off its transcode until claimed.
    pub warm: Option<WarmGate>,
}

/// What the probe found and decided, as the session row and its SSE events carry it.
pub(crate) fn probe_json(session: &PlaybackSession) -> serde_json::Value {
    let method = match session.decision.method {
        PlaybackMethod::DirectPlay => "direct_play",
        PlaybackMethod::Remux => "remux",
        PlaybackMethod::AudioTranscode => "audio_transcode",
        PlaybackMethod::VideoTranscode => "video_transcode",
    };
    let (width, height) = session.source_resolution;
    serde_json::json!({
        "duration": session.duration_seconds,
        "video_codec": session.video_codec,
        "video_width": width,
        "video_height": height,
        "method": method,
        "selected_audio_stream_index": session.decision.audio_stream_index,
        "preferred_audio_stream_index": session.preferred_audio_index,
        "audio_streams": session.audio_metas.iter().map(|a| serde_json::json!({
            "stream_index": a.stream_index,
            "codec": a.codec,
            "channels": a.channels,
            "language": a.language,
        })).collect::<Vec<_>>(),
    })
}

/// Probe, decide, and start whatever transcode the decision needs.
pub(super) async fn run(ctx: Prepare) {
    let Prepare {
        state: bg_state,
        session_id,
        release: bg_release,
        capabilities,
        stream_url: bg_stream_url,
        container: bg_container,
        segment_dir: bg_segment_dir,
        local_cache_path,
        download_meter,
        start_time,
        audio_language,
        warm,
    } = ctx;

    // Check for cached probe sidecar (only valid for fully-downloaded files)
    let sidecar_path = probe_sidecar_path(&local_cache_path);
    let cached_probe = if download_meter.is_complete() {
        match tokio::fs::read_to_string(&sidecar_path).await {
            Ok(json) => {
                match serde_json::from_str::<crate::ffmpeg::transcode::HybridProbeInfo>(&json) {
                    Ok(probe) => {
                        info!(session_id = %session_id, "Using cached probe sidecar");
                        Some(probe)
                    }
                    Err(e) => {
                        warn!(session_id = %session_id, error = %e, "Invalid probe sidecar, re-probing");
                        None
                    }
                }
            }
            Err(_) => None,
        }
    } else {
        None
    };

    // Either use the cached probe or probe via HybridIo
    let (probe_info, probe_result) = if let Some(cached) = cached_probe {
        (cached, None)
    } else {
        let result = match crate::ffmpeg::transcode::TranscodeProcess::probe_hybrid(
            local_cache_path.clone(),
            bg_stream_url.clone(),
            download_meter.progress.clone(),
        )
        .await
        {
            Ok(r) => r,
            Err(e) => {
                error!(session_id = %session_id, error = %e, "Background probe failed");
                fail_debrid_session(
                    &bg_state,
                    session_id,
                    &bg_release,
                    crate::streaming::session::SessionError::new(
                        "probe_failed",
                        format!("Probe failed: {e}"),
                    ),
                )
                .await;
                return;
            }
        };

        // Write probe sidecar for future plays of this cached file
        if download_meter.is_complete() {
            match serde_json::to_string(&result.probe) {
                Ok(json) => {
                    if let Err(e) = tokio::fs::write(&sidecar_path, json).await {
                        warn!(session_id = %session_id, error = %e, "Failed to write probe sidecar");
                    }
                }
                Err(e) => {
                    warn!(session_id = %session_id, error = %e, "Failed to serialize probe sidecar");
                }
            }
        }

        let info = crate::ffmpeg::transcode::HybridProbeInfo {
            duration_seconds: result.probe.duration_seconds,
            video_codec: result.probe.video_codec.clone(),
            audio_codec: result.probe.audio_codec.clone(),
            audio_stream_index: result.probe.audio_stream_index,
            video_width: result.probe.video_width,
            video_height: result.probe.video_height,
            interlaced: result.probe.interlaced,
            audio_tracks: result
                .probe
                .audio_tracks
                .iter()
                .map(|a| crate::ffmpeg::transcode::HybridAudioTrack {
                    stream_index: a.stream_index,
                    codec: a.codec.clone(),
                    channels: a.channels,
                    language: a.language.clone(),
                })
                .collect(),
        };
        (info, Some(result))
    };

    let duration = probe_info.duration_seconds;
    let video_codec = probe_info.video_codec.clone();
    let video_width = probe_info.video_width;
    let video_height = probe_info.video_height;

    info!(
        session_id = %session_id,
        duration,
        video_codec = %video_codec,
        "Debrid stream probed (background)"
    );

    // Decision engine
    let file_meta = crate::streaming::decision::FileMeta {
        container: bg_container,
        video_codec: video_codec.clone(),
        video_width,
        video_height,
        interlaced: probe_info.interlaced,
    };
    let audio_metas: Vec<crate::streaming::decision::AudioTrackMeta> = probe_info
        .audio_tracks
        .iter()
        .map(|a| crate::streaming::decision::AudioTrackMeta {
            stream_index: a.stream_index,
            codec: a.codec.clone(),
            channels: a.channels,
            language: a.language.clone(),
            title: None,
            is_default: false,
        })
        .collect();
    // Resolve preferred audio from language settings
    let preferred_audio =
        crate::streaming::decision::find_preferred_audio(&audio_metas, &audio_language);

    let decision = crate::streaming::decision::decide(
        &file_meta,
        &audio_metas,
        &[],
        &capabilities,
        ViewerPrefs {
            preferred_audio_index: preferred_audio,
            preferred_subtitle_index: None,
            understood_language: &audio_language,
        },
        &bg_state.encoder_caps.preferred_codecs(),
    );

    info!(session_id = %session_id, method = ?decision.method, "Debrid playback decision (background)");

    // Stamp probe-derived state onto the in-memory session IMMEDIATELY so
    // that master.m3u8 / stream.m3u8 handlers stop returning 503 as soon as
    // the probe finishes. Previously these fields were only set after
    // ffmpeg passed a health check (see below), opening a long window where
    // `probe_complete` SSE had already fired but the playlist handler
    // still rejected the first hls.js request.
    //
    // Remux/Transcode path sets `session.transcode` further down once the
    // encoder is healthy; the playlist handler's estimated path works fine
    // with `transcode = None` (3 placeholder segments, serve_segment
    // block-waits for the files to appear).
    {
        let total_segments = (duration / 6.0).ceil() as u32;
        if let Some(mut session) = bg_state.playback_sessions.get_mut(&session_id) {
            session.decision = decision.clone();
            session.duration_seconds = duration;
            session.segment_cache.duration_seconds = duration;
            session.segment_cache.total_segments = total_segments;
            session.video_codec = video_codec.clone();
            session.source_resolution = (video_width, video_height);
            session.thumbnail_dir = Some(
                local_cache_path
                    .parent()
                    .unwrap_or(&local_cache_path)
                    .join("media_cache")
                    .join("trickplay"),
            );
            session.audio_metas = audio_metas.clone();
            session.preferred_audio_index = preferred_audio;
            session.file_path = local_cache_path.clone();
        }
    }

    // Stored so the SSE status endpoint can emit probe_complete. `method` is the refined
    // decision: a client that started on the optimistic one compares against it.
    let probe_json = bg_state
        .playback_sessions
        .get(&session_id)
        .map(|s| probe_json(&s))
        .unwrap_or_default();
    let _ = sqlx::query(
        "UPDATE debrid_stream_sessions SET probe_data = $1, updated_at = NOW() WHERE id = $2",
    )
    .bind(&probe_json)
    .bind(session_id)
    .execute(&bg_state.pool)
    .await;

    // DirectPlay: client handles the container natively — no FFmpeg needed.
    // All session fields were set above; nothing more to do.
    if decision.method == PlaybackMethod::DirectPlay {
        info!(session_id = %session_id, "DirectPlay — skipping FFmpeg, serving raw file");
        return;
    }

    if let Some(gate) = warm {
        tokio::select! {
            _ = gate.claimed.cancelled() => {}
            _ = gate.released.cancelled() => return,
        }
    }

    // Remux/Transcode: start FFmpeg to produce HLS segments.
    // Uses the same encoder fallback chain as the cached-file path:
    // primary (e.g. h264_nvenc) → vaapi → libx264.
    let primary_encoder = match &decision.video_mode {
        crate::streaming::decision::VideoMode::Transcode { target_codec } => bg_state
            .encoder_caps
            .encoder_for(target_codec)
            .unwrap_or(&bg_state.encoder_caps.h264)
            .to_string(),
        crate::streaming::decision::VideoMode::Copy => bg_state.encoder_caps.h264.clone(),
    };

    let needs_transcode = matches!(
        &decision.video_mode,
        crate::streaming::decision::VideoMode::Transcode { .. }
    );

    // Determine hw_device for the primary encoder
    let primary_hw_device: Option<kob_ffmpeg::hw::device::SharedHwDevice> =
        if primary_encoder == "libx264" {
            None
        } else {
            bg_state.hw_device.get()
        };

    // Try primary encoder via hybrid IO (only when we have the full probe result,
    // i.e. not using cached sidecar probe)
    let mut transcode = None;
    let mut fail_reason: Option<&'static str> = None;
    if let Some(probe_result) = probe_result {
        match crate::ffmpeg::transcode::TranscodeProcess::start_hybrid_from_probe(
            probe_result,
            &decision,
            bg_segment_dir.clone(),
            6,
            &primary_encoder,
            start_time,
            primary_hw_device,
        )
        .await
        {
            Ok(mut process) => match process.survived_launch().await {
                Ok(()) => transcode = Some(process),
                Err(e) => {
                    warn!(
                        session_id = %session_id,
                        encoder = %primary_encoder,
                        error = %e,
                        "Hybrid encoder crashed immediately, will retry from cache"
                    );
                    fail_reason = Some("hybrid_health_fail");
                    process.kill().await;
                }
            },
            Err(e) => {
                warn!(
                    session_id = %session_id,
                    encoder = %primary_encoder,
                    error = %e,
                    "Failed to start hybrid encoder, will retry from cache"
                );
                fail_reason = Some("hybrid_start_failed");
            }
        }
    } else {
        fail_reason = Some("no_probe_result");
    }

    // If the primary encoder failed, fall back using the cached-file path.
    // The hybrid IO is consumed, but the local cache file has enough data
    // for fallback encoders to start producing segments.
    //
    // For VideoMode::Transcode we walk the h264 fallback encoder chain.
    // For VideoMode::Copy (Remux / AudioTranscode) the encoder choice is
    // irrelevant — video is copied — so we make a single attempt with the
    // same encoder value (it's unused by ffmpeg in Copy mode).
    if transcode.is_none() {
        let (video_mode, encoder_chain): (crate::ffmpeg::transcode::VideoMode, Vec<String>) =
            if needs_transcode {
                let mut chain: Vec<String> = bg_state
                    .encoder_caps
                    .h264_fallbacks()
                    .into_iter()
                    .map(String::from)
                    .collect();
                // Don't retry the primary — it already failed
                chain.retain(|e| e != &primary_encoder);
                (crate::ffmpeg::transcode::VideoMode::Transcode, chain)
            } else {
                // Video copy: one shot from cached file. target_encoder is
                // unused by ffmpeg in Copy mode but we pass primary_encoder
                // for logging consistency.
                (
                    crate::ffmpeg::transcode::VideoMode::Copy,
                    vec![primary_encoder.clone()],
                )
            };

        let audio_mode = match &decision.audio_mode {
            crate::streaming::decision::AudioMode::Transcode {
                target_codec,
                bitrate_kbps,
            } => crate::ffmpeg::transcode::AudioMode::Transcode {
                target_codec: target_codec.clone(),
                bitrate_kbps: *bitrate_kbps,
            },
            crate::streaming::decision::AudioMode::Copy => {
                crate::ffmpeg::transcode::AudioMode::Copy
            }
        };

        for encoder in &encoder_chain {
            let _ = tokio::fs::remove_dir_all(&bg_segment_dir).await;

            let hw_device = if encoder == "libx264" {
                None
            } else if encoder.contains("vaapi")
                && matches!(*bg_state.hw_accel, crate::ffmpeg::detect::HwAccel::Nvenc)
            {
                if let Some(ref dev) = bg_state.vaapi_fallback_device {
                    Some(dev.clone())
                } else {
                    warn!(
                        session_id = %session_id,
                        "No VAAPI fallback device available, skipping VAAPI encoder"
                    );
                    continue;
                }
            } else {
                bg_state.hw_device.get()
            };

            let args = crate::ffmpeg::transcode::TranscodeArgs {
                input_path: local_cache_path.clone(),
                output_dir: bg_segment_dir.clone(),
                video_mode,
                audio_mode: audio_mode.clone(),
                audio_stream_index: decision.audio_stream_index,
                start_time,
                segment_duration: 6,
                hw_accel: (*bg_state.hw_accel).clone(),
                video_codec: video_codec.clone(),
                target_encoder: encoder.clone(),
                burn_in_subtitle: None,
                scale: None,
                target_bitrate_kbps: None,
                duration_seconds: duration,
                initial_generation: 0,
                hw_device,
                prior_runs: Vec::new(),
                deinterlace: decision.deinterlace,
            };

            match crate::ffmpeg::transcode::TranscodeProcess::start(args).await {
                Ok(mut process) => match process.survived_launch().await {
                    Ok(()) => {
                        info!(
                            session_id = %session_id,
                            encoder = %encoder,
                            "Encoder fallback succeeded (hybrid→cached)"
                        );
                        transcode = Some(process);
                        break;
                    }
                    Err(e) => {
                        warn!(
                            session_id = %session_id,
                            encoder = %encoder,
                            error = %e,
                            "Fallback encoder crashed immediately, trying next"
                        );
                        process.kill().await;
                    }
                },
                Err(e) => {
                    warn!(
                        session_id = %session_id,
                        encoder = %encoder,
                        error = %e,
                        "Failed to start fallback encoder, trying next"
                    );
                }
            }
        }
    }

    let transcode = match transcode {
        Some(t) => t,
        None => {
            let reason = fail_reason.unwrap_or("unknown");
            error!(
                session_id = %session_id,
                primary_encoder = %primary_encoder,
                fail_reason = %reason,
                video_mode = ?decision.video_mode,
                audio_mode = ?decision.audio_mode,
                "All encoders failed"
            );
            fail_debrid_session(
                &bg_state,
                session_id,
                &bg_release,
                crate::streaming::session::SessionError::new(
                    "no_fallback_encoder",
                    format!("All encoders failed ({reason})"),
                ),
            )
            .await;
            return;
        }
    };

    // Attach the running transcode process to the session. All other
    // probe-derived fields were stamped immediately after the decision
    // above, so the playlist handler has been serving the estimated
    // playlist while ffmpeg was starting.
    if let Some(mut session) = bg_state.playback_sessions.get_mut(&session_id) {
        session.transcode = Some(transcode);
    }
}

pub(crate) async fn fail_debrid_session(
    state: &AppState,
    session_id: Uuid,
    release: &StreamRelease,
    error: crate::streaming::session::SessionError,
) {
    let code = error.code;
    if let Some(mut session) = state.playback_sessions.get_mut(&session_id) {
        session.error = Some(error);
    }
    debrid_outcomes::report_failure(state, release, code).await;
}
