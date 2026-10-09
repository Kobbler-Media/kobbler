//! FFmpeg transcode process manager.
//!
//! Manages FFmpeg child processes for on-the-fly remuxing and transcoding,
//! outputting HLS segments in fragmented MP4 format.

use super::detect::HwAccel;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use tracing::info;

/// What to do with the video stream.
#[derive(Debug, Clone, Copy)]
pub enum VideoMode {
    /// Copy video stream unchanged.
    Copy,
    /// Transcode to H.264 using the best available encoder.
    Transcode,
}

/// What to do with the audio stream.
#[derive(Debug, Clone)]
pub enum AudioMode {
    /// Copy audio stream unchanged.
    Copy,
    /// Transcode to the specified codec.
    Transcode {
        target_codec: String,
        bitrate_kbps: i32,
    },
}

/// Arguments for starting a transcode session.
#[derive(Debug, Clone)]
pub struct TranscodeArgs {
    /// Path to the input media file.
    pub input_path: PathBuf,
    /// Directory for HLS segment output.
    pub output_dir: PathBuf,
    /// What to do with the video stream.
    pub video_mode: VideoMode,
    /// What to do with the audio stream.
    pub audio_mode: AudioMode,
    /// Which audio stream index to select (ffprobe index).
    pub audio_stream_index: i32,
    /// Seek position in seconds (None = start from beginning).
    pub start_time: Option<f64>,
    /// HLS segment duration in seconds.
    pub segment_duration: u32,
    /// Hardware acceleration to use.
    pub hw_accel: HwAccel,
    /// Source video codec (e.g. "hevc", "h264") for codec-specific output flags.
    pub video_codec: String,
    /// FFmpeg encoder to use when transcoding (e.g. "h264_vaapi", "hevc_nvenc", "libx264").
    /// Ignored when video_mode is Copy.
    pub target_encoder: String,
    /// Burn bitmap subtitle into video via overlay filter (stream index).
    /// Uses hwdownload/hwupload pattern to keep GPU encoding when available.
    pub burn_in_subtitle: Option<i32>,
    /// Target output resolution for downscaling (width, height).
    /// None = native resolution (no scaling).
    pub scale: Option<(u32, u32)>,
    /// Target video bitrate in kbps. Replaces CRF/QP with bitrate mode
    /// for ABR variants where predictable bandwidth matters.
    pub target_bitrate_kbps: Option<u32>,
    /// Total media duration in seconds (from probe). Needed for segment indexing.
    pub duration_seconds: f64,
    /// Starting generation number (default 0). Set when restarting a session
    /// mid-seek to continue from the parent session's generation counter.
    pub initial_generation: u32,
    /// Shared GPU device context for hardware-accelerated decoding.
    pub hw_device: Option<kob_ffmpeg::hw::device::SharedHwDevice>,
}

/// Health state for a live IPTV stream, shared between the feeder task and playlist handler.
pub struct StreamHealth {
    /// Number of fMP4 segments produced so far.
    pub segments_produced: u32,
    /// When the last segment was written to disk.
    pub last_segment_at: Option<std::time::Instant>,
    /// Total bytes received from the source HTTP stream.
    pub source_bytes_received: u64,
    /// Whether the source HTTP connection is currently alive.
    pub is_source_connected: bool,
    /// When the last chunk of data was received from the source.
    pub last_data_at: Option<std::time::Instant>,
    /// Last error message from the feeder or transcode, if any.
    pub error_message: Option<String>,
    /// Number of reconnect attempts so far.
    pub reconnect_count: u32,
}

impl Default for StreamHealth {
    fn default() -> Self {
        Self::new()
    }
}

impl StreamHealth {
    pub fn new() -> Self {
        Self {
            segments_produced: 0,
            last_segment_at: None,
            source_bytes_received: 0,
            is_source_connected: true,
            last_data_at: Some(std::time::Instant::now()),
            error_message: None,
            reconnect_count: 0,
        }
    }

    /// Whether the source appears stalled (connected but no data for > threshold).
    pub fn is_stalled(&self, threshold: std::time::Duration) -> bool {
        self.is_source_connected && self.last_data_at.is_some_and(|t| t.elapsed() > threshold)
    }
}

/// Stream metadata extracted during HybridIo open (avoids a separate probe connection).
#[derive(Serialize, Deserialize)]
pub struct HybridProbeInfo {
    pub duration_seconds: f64,
    pub video_codec: String,
    pub audio_codec: String,
    pub audio_stream_index: i32,
    pub video_width: u32,
    pub video_height: u32,
    pub audio_tracks: Vec<HybridAudioTrack>,
}

#[derive(Serialize, Deserialize)]
pub struct HybridAudioTrack {
    pub stream_index: i32,
    pub codec: String,
    pub channels: i32,
    pub language: Option<String>,
}

impl HybridProbeInfo {
    fn from_input_context(ctx: &kob_ffmpeg::format::input::InputContext) -> Self {
        use kob_ffmpeg::format::input::MediaType;

        let duration_seconds = ctx.duration_secs().unwrap_or(0.0);

        let mut video_codec = String::new();
        let mut video_width = 1920u32;
        let mut video_height = 1080u32;
        let mut audio_codec = String::new();
        let mut audio_stream_index = 0i32;
        let mut audio_tracks = Vec::new();
        let mut first_audio = true;

        for i in 0..ctx.nb_streams() {
            let Some(stream) = ctx.stream(i) else {
                continue;
            };
            match stream.codec_type() {
                MediaType::Video if video_codec.is_empty() => {
                    let params = stream.codec_parameters();
                    unsafe {
                        let name = kob_ffmpeg::ffi::avcodec_get_name((*params).codec_id);
                        video_codec = std::ffi::CStr::from_ptr(name)
                            .to_string_lossy()
                            .into_owned();
                        video_width = (*params).width as u32;
                        video_height = (*params).height as u32;
                    }
                }
                MediaType::Audio => {
                    let params = stream.codec_parameters();
                    let (codec_name, channels) = unsafe {
                        let name = kob_ffmpeg::ffi::avcodec_get_name((*params).codec_id);
                        let n = std::ffi::CStr::from_ptr(name)
                            .to_string_lossy()
                            .into_owned();
                        (n, (*params).ch_layout.nb_channels)
                    };
                    let language = stream.metadata().get("language").map(|s| s.to_owned());
                    if first_audio {
                        audio_codec = codec_name.clone();
                        audio_stream_index = i as i32;
                        first_audio = false;
                    }
                    audio_tracks.push(HybridAudioTrack {
                        stream_index: i as i32,
                        codec: codec_name,
                        channels,
                        language,
                    });
                }
                _ => {}
            }
        }

        Self {
            duration_seconds,
            video_codec,
            audio_codec,
            audio_stream_index,
            video_width,
            video_height,
            audio_tracks,
        }
    }
}

/// Result of probing via HybridIo — includes the open contexts so the
/// caller can either start a transcode or discard them for direct play.
pub struct HybridProbeResult {
    pub probe: HybridProbeInfo,
    pub custom_io: kob_ffmpeg::format::io_context::CustomIoContext,
    pub in_ctx: kob_ffmpeg::format::input::InputContext,
}

/// A running transcode session.
///
/// Supports two backends:
/// - **Session**: kob-ffmpeg `TranscodeSession` with in-process seeking (file-based input)
/// - **Library**: kob-ffmpeg in `spawn_blocking` with `CancellationToken` (IPTV live)
pub struct TranscodeProcess {
    pub(crate) backend: TranscodeBackend,
    /// Directory containing HLS segments for the current generation.
    pub segment_dir: PathBuf,
    /// Path to the HLS playlist file for the current generation.
    pub playlist_path: PathBuf,
    /// Live stream health, shared with the feeder task. `None` for file-based transcodes.
    pub stream_health: Option<std::sync::Arc<std::sync::Mutex<StreamHealth>>>,
    /// Wall-clock instant the transcode process was started. Used by the
    /// playlist handler to apply a startup grace window before reporting
    /// "ffmpeg produced nothing" as a fatal error.
    pub started_at: std::time::Instant,
}

pub(crate) enum TranscodeBackend {
    Session {
        session: kob_ffmpeg::session::TranscodeSession,
    },
    /// kob-ffmpeg in `spawn_blocking` with `CancellationToken`.
    /// Used for IPTV live transcoding where input comes via custom I/O (pipe),
    /// which `TranscodeSession` doesn't support.
    Library {
        handle: tokio::task::JoinHandle<Result<(), kob_ffmpeg::FfmpegError>>,
        cancel: tokio_util::sync::CancellationToken,
    },
}

/// Errors from transcode operations.
#[derive(Debug, thiserror::Error)]
pub enum TranscodeError {
    #[error("ffmpeg not found at '{0}'")]
    NotFound(String),
    #[error("failed to start ffmpeg: {0}")]
    StartFailed(String),
    #[error("I/O error: {0}")]
    IoError(#[from] std::io::Error),
}

/// Build a `SessionConfig` from `TranscodeArgs` without starting the session.
fn build_config_for_args(args: &TranscodeArgs) -> kob_ffmpeg::session::SessionConfig {
    let mut config =
        kob_ffmpeg::session::SessionConfig::new(&args.output_dir, args.duration_seconds)
            .segment_duration(args.segment_duration as f64);

    if let VideoMode::Transcode = &args.video_mode {
        config = config.video_encoder(&args.target_encoder);
    }
    if let AudioMode::Transcode {
        target_codec,
        bitrate_kbps,
    } = &args.audio_mode
    {
        config = config
            .audio_encoder(target_codec)
            .audio_bitrate(*bitrate_kbps as i64 * 1000);
    }
    if let Some(br) = args.target_bitrate_kbps {
        config = config.video_bitrate(br as i64 * 1000);
    }
    if let Some((w, h)) = args.scale {
        config.scale_width = Some(w as i32);
        config.scale_height = Some(h as i32);
    }
    config.start_time = args.start_time;
    if let Some(sub_idx) = args.burn_in_subtitle {
        config = config.subtitle_burn_in(sub_idx as u32);
    }
    config.audio_stream_index = Some(args.audio_stream_index as u32);
    if args.initial_generation > 0 {
        config = config.initial_generation(args.initial_generation);
    }
    if let Some(ref hw) = args.hw_device {
        config = config.hw_device(hw.clone());
    }
    config
}

/// Build a `SessionConfig` for a hybrid (debrid) transcode from probe + decision.
fn build_config_for_hybrid(
    decision: &crate::streaming::decision::PlaybackDecision,
    output_dir: &Path,
    duration: f64,
    segment_duration: u32,
    target_encoder: &str,
    start_time: Option<f64>,
    hw_device: Option<&kob_ffmpeg::hw::device::SharedHwDevice>,
) -> kob_ffmpeg::session::SessionConfig {
    let mut config = kob_ffmpeg::session::SessionConfig::new(output_dir, duration)
        .segment_duration(segment_duration as f64);

    let video_encoder = match &decision.video_mode {
        crate::streaming::decision::VideoMode::Transcode { .. } => Some(target_encoder.to_string()),
        crate::streaming::decision::VideoMode::Copy => None,
    };
    let audio_config = match &decision.audio_mode {
        crate::streaming::decision::AudioMode::Transcode {
            target_codec,
            bitrate_kbps,
        } => Some((target_codec.clone(), *bitrate_kbps)),
        crate::streaming::decision::AudioMode::Copy => None,
    };

    if let Some(ref enc) = video_encoder {
        config = config.video_encoder(enc);
    }
    if let Some((ref codec, bitrate)) = audio_config {
        config = config
            .audio_encoder(codec)
            .audio_bitrate(bitrate as i64 * 1000);
    }
    config.audio_stream_index = Some(decision.audio_stream_index as u32);
    config.start_time = start_time;
    if let Some(hw) = hw_device {
        config = config.hw_device(hw.clone());
    }
    config
}

impl TranscodeProcess {
    /// Start a new transcode session using kob-ffmpeg's `TranscodeSession`.
    ///
    /// The session runs in a background thread and supports in-process seeking
    /// without killing/restarting. HLS segments are written to generation
    /// subdirectories (`gen_0/`, `gen_1/`, ...) under `output_dir`.
    pub async fn start(args: TranscodeArgs) -> Result<Self, TranscodeError> {
        tokio::fs::create_dir_all(&args.output_dir).await?;

        let config = build_config_for_args(&args);

        let initial_gen = args.initial_generation;
        let input_path = args.input_path.clone();

        info!(
            input = %args.input_path.display(),
            output_dir = %args.output_dir.display(),
            video_mode = ?args.video_mode,
            audio_mode = ?args.audio_mode,
            "Starting kob-ffmpeg transcode session"
        );

        // TranscodeSession::start is blocking (opens input, creates decoders)
        // so run it on a blocking thread.
        let session = tokio::task::spawn_blocking(move || {
            kob_ffmpeg::session::TranscodeSession::start(&input_path, config)
        })
        .await
        .map_err(|e| TranscodeError::StartFailed(format!("task join error: {e}")))?
        .map_err(|e| TranscodeError::StartFailed(format!("session start failed: {e}")))?;

        let segment_dir = session.gen_dir(initial_gen);
        let playlist_path = session.gen_dir(initial_gen).join("stream.m3u8");

        Ok(Self {
            backend: TranscodeBackend::Session { session },
            segment_dir,
            playlist_path,
            stream_health: None,
            started_at: std::time::Instant::now(),
        })
    }

    /// Start a transcode session using HybridIo for debrid streaming.
    ///
    /// Reads from local cache for downloaded portions, HTTP for the rest.
    /// The `progress` is shared with the progressive download task.
    /// Returns both the process and stream metadata (extracted from the same
    /// open, avoiding a separate probe connection to the debrid URL).
    /// Probe a remote file via HybridIo without starting a transcode.
    /// Returns probe info and the open contexts, which can be passed to
    /// `start_hybrid_from_probe` if transcoding is needed.
    pub async fn probe_hybrid(
        local_cache_path: PathBuf,
        debrid_url: String,
        progress: std::sync::Arc<kob_ffmpeg::hybrid_io::DownloadProgress>,
    ) -> Result<HybridProbeResult, TranscodeError> {
        info!(
            cache = %local_cache_path.display(),
            url = %debrid_url,
            "Probing via HybridIo"
        );

        tokio::task::spawn_blocking(move || {
            let io = kob_ffmpeg::hybrid_io::HybridIo::new(local_cache_path, debrid_url, progress);
            let mut custom_io = kob_ffmpeg::format::io_context::CustomIoContext::new(io)
                .map_err(|e| kob_ffmpeg::FfmpegError::new(-1, format!("custom IO error: {e}")))?;
            let mut in_ctx = kob_ffmpeg::format::input::InputContext::open_custom(&mut custom_io)
                .map_err(|e| {
                kob_ffmpeg::FfmpegError::new(-1, format!("open custom IO: {e}"))
            })?;
            in_ctx.find_stream_info()?;

            let probe = HybridProbeInfo::from_input_context(&in_ctx);
            Ok::<_, kob_ffmpeg::FfmpegError>(HybridProbeResult {
                probe,
                custom_io,
                in_ctx,
            })
        })
        .await
        .map_err(|e| TranscodeError::StartFailed(format!("task join error: {e}")))?
        .map_err(|e| TranscodeError::StartFailed(format!("probe failed: {e}")))
    }

    /// Start an HLS transcode from an already-probed HybridIo context.
    /// Call this after `probe_hybrid` when the decision engine says transcode is needed.
    pub async fn start_hybrid_from_probe(
        probe_result: HybridProbeResult,
        decision: &crate::streaming::decision::PlaybackDecision,
        output_dir: PathBuf,
        segment_duration: u32,
        target_encoder: &str,
        start_time: Option<f64>,
        hw_device: Option<kob_ffmpeg::hw::device::SharedHwDevice>,
    ) -> Result<Self, TranscodeError> {
        tokio::fs::create_dir_all(&output_dir).await?;

        let duration = probe_result.probe.duration_seconds;

        info!(
            output_dir = %output_dir.display(),
            video_mode = ?decision.video_mode,
            audio_mode = ?decision.audio_mode,
            target_encoder = %target_encoder,
            duration_secs = duration,
            start_time = ?start_time,
            "Starting kob-ffmpeg hybrid transcode session (from probe)"
        );

        let config = build_config_for_hybrid(
            decision,
            &output_dir,
            duration,
            segment_duration,
            target_encoder,
            start_time,
            hw_device.as_ref(),
        );

        let session = tokio::task::spawn_blocking(move || {
            kob_ffmpeg::session::TranscodeSession::start_from_custom_io(
                probe_result.custom_io,
                probe_result.in_ctx,
                None,
                config,
            )
        })
        .await
        .map_err(|e| TranscodeError::StartFailed(format!("task join error: {e}")))?
        .map_err(|e| TranscodeError::StartFailed(format!("session start failed: {e}")))?;

        let segment_dir = session.gen_dir(0);
        let playlist_path = session.gen_dir(0).join("stream.m3u8");

        info!(
            segment_dir = %segment_dir.display(),
            playlist = %playlist_path.display(),
            "Hybrid transcode session started"
        );

        Ok(Self {
            backend: TranscodeBackend::Session { session },
            segment_dir,
            playlist_path,
            stream_health: None,
            started_at: std::time::Instant::now(),
        })
    }

    /// Seek the transcode session to a new position in-process.
    ///
    /// If the position is already cached by a previous generation, returns
    /// instantly. Otherwise flushes the pipeline and seeks the input.
    /// Returns `None` for IPTV child-process transcodes (no in-process seek).
    pub fn seek(&mut self, position_secs: f64) -> Option<kob_ffmpeg::session::SeekResult> {
        match &self.backend {
            TranscodeBackend::Session { session } => {
                let result = session.seek(position_secs)?;
                // Update paths to the active generation
                let active_gen = match &result {
                    kob_ffmpeg::session::SeekResult::Cached {
                        active_generation, ..
                    } => *active_generation,
                    kob_ffmpeg::session::SeekResult::NewGeneration { generation } => *generation,
                };
                self.segment_dir = session.gen_dir(active_gen);
                self.playlist_path = session.gen_dir(active_gen).join("stream.m3u8");
                Some(result)
            }
            TranscodeBackend::Library { .. } => None,
        }
    }

    /// Redirect the transcode thread unconditionally (new generation + seek).
    /// Returns the new generation number, or `None` if the thread is dead.
    pub fn redirect(&mut self, position_secs: f64) -> Option<u32> {
        match &self.backend {
            TranscodeBackend::Session { session } => {
                let new_gen = session.redirect(position_secs)?;
                self.segment_dir = session.gen_dir(new_gen);
                self.playlist_path = session.gen_dir(new_gen).join("stream.m3u8");
                Some(new_gen)
            }
            TranscodeBackend::Library { .. } => None,
        }
    }

    // =========================================================================
    // Delegated segment state methods (TranscodeSession is source of truth)
    // =========================================================================

    /// Update segment info from disk for all generations.
    /// Only meaningful for Session backends; no-op for others.
    pub fn update_segments(&self) {
        if let TranscodeBackend::Session { session } = &self.backend {
            session.update_segments();
        }
    }

    /// Resolve an absolute segment index to its file path.
    pub fn resolve_segment(&self, abs_index: u32) -> Option<kob_ffmpeg::session::ResolvedSegment> {
        match &self.backend {
            TranscodeBackend::Session { session } => session.resolve_segment(abs_index),
            _ => None,
        }
    }

    /// Get playlist segments starting from a given segment index.
    pub fn playlist_segments_from(
        &self,
        start_segment: u32,
    ) -> Vec<kob_ffmpeg::session::PlaylistSegment> {
        match &self.backend {
            TranscodeBackend::Session { session } => session.playlist_segments_from(start_segment),
            _ => Vec::new(),
        }
    }

    /// Spawn an overlay generation re-encoding a bounded `[start, end)` time
    /// range with the given encoder. Returns the new generation number.
    /// Library-backed (custom-IO) sessions don't support overlays — use
    /// Tier-3 full-restart there. See [`kob_ffmpeg::session::TranscodeSession::start_overlay`].
    pub fn start_overlay(
        &self,
        time_range: std::ops::Range<f64>,
        overlay: kob_ffmpeg::session::OverlayConfig,
    ) -> std::result::Result<u32, kob_ffmpeg::FfmpegError> {
        match &self.backend {
            TranscodeBackend::Session { session } => session.start_overlay(time_range, overlay),
            _ => Err(kob_ffmpeg::FfmpegError::new(
                kob_ffmpeg::ffi::AVERROR_EXTERNAL,
                "start_overlay: backend does not support overlays (custom I/O / library backend)",
            )),
        }
    }

    /// Evict old generations to stay within cache budget.
    pub fn evict_if_needed(&self, playback_segment: u32) {
        if let TranscodeBackend::Session { session } = &self.backend {
            session.evict_if_needed(playback_segment);
        }
    }

    /// Current generation number. Returns 0 for non-session backends.
    pub fn current_generation(&self) -> u32 {
        match &self.backend {
            TranscodeBackend::Session { session } => session.current_generation(),
            _ => 0,
        }
    }

    /// Generation directory path.
    pub fn gen_dir(&self, generation: u32) -> std::path::PathBuf {
        match &self.backend {
            TranscodeBackend::Session { session } => session.gen_dir(generation),
            _ => self.segment_dir.clone(),
        }
    }

    /// Convert time to segment index.
    pub fn time_to_segment(&self, secs: f64) -> u32 {
        match &self.backend {
            TranscodeBackend::Session { session } => session.time_to_segment(secs),
            _ => 0,
        }
    }

    /// Convert segment index to time.
    pub fn segment_to_time(&self, idx: u32) -> f64 {
        match &self.backend {
            TranscodeBackend::Session { session } => session.segment_to_time(idx),
            _ => 0.0,
        }
    }

    /// Find the segment containing the given time using actual durations.
    pub fn time_to_segment_accurate(&self, secs: f64) -> u32 {
        match &self.backend {
            TranscodeBackend::Session { session } => session.time_to_segment_accurate(secs),
            _ => 0,
        }
    }

    /// Cumulative real time at a given segment index.
    pub fn segment_to_time_accurate(&self, idx: u32) -> f64 {
        match &self.backend {
            TranscodeBackend::Session { session } => session.segment_to_time_accurate(idx),
            _ => 0.0,
        }
    }

    /// Store pre-computed keyframe timestamps for playlist prediction.
    pub fn set_keyframes(&self, keyframes: Vec<f64>) {
        if let TranscodeBackend::Session { session } = &self.backend {
            session.set_keyframes(keyframes);
        }
    }

    /// Generate predicted segments from cached keyframes.
    pub fn predicted_segments(
        &self,
        current_generation: u32,
        run_start_time: f64,
    ) -> Vec<kob_ffmpeg::session::PlaylistSegment> {
        match &self.backend {
            TranscodeBackend::Session { session } => {
                session.predicted_segments(current_generation, run_start_time)
            }
            _ => Vec::new(),
        }
    }

    /// Check if the transcode is still running.
    pub fn is_running(&mut self) -> bool {
        match &mut self.backend {
            TranscodeBackend::Session { session } => session.is_running(),
            TranscodeBackend::Library { handle, .. } => !handle.is_finished(),
        }
    }

    /// Check transcode health (non-blocking).
    ///
    /// Returns `Ok(true)` if the process is still running, `Ok(false)` if it
    /// finished successfully, or `Err(message)` if it crashed. Stall detection
    /// is *not* performed here — that's the playlist/segment handler's job
    /// (demand-driven). A paused ffmpeg with the player reading cached/ahead
    /// segments is legitimate backpressure, not a stall.
    pub fn check_health(&mut self) -> Result<bool, String> {
        match &mut self.backend {
            TranscodeBackend::Session { session } => {
                if !session.is_running() {
                    return match session.error() {
                        Some(err) => Err(err),
                        None => Ok(false),
                    };
                }
                Ok(true)
            }
            TranscodeBackend::Library { handle, .. } => Ok(!handle.is_finished()),
        }
    }

    /// Stop the transcode, wait for GPU resources to be released.
    pub async fn kill(&mut self) {
        match &mut self.backend {
            TranscodeBackend::Session { session } => {
                session.cancel();
                // Wait for the thread to exit on a blocking task so we don't
                // block the tokio runtime. This ensures NVDEC/NVENC sessions
                // and CUDA contexts are released before we return.
                while session.is_running() {
                    tokio::time::sleep(std::time::Duration::from_millis(10)).await;
                }
            }
            TranscodeBackend::Library { handle, cancel } => {
                cancel.cancel();
                let _ = handle.await;
            }
        }
    }
}

/// Extract an embedded subtitle track to a file.
///
/// Uses kob-ffmpeg to copy the subtitle stream without transcoding.
pub async fn extract_subtitle(
    input_path: &Path,
    stream_index: i32,
    output_path: &Path,
) -> Result<(), TranscodeError> {
    let input = input_path.to_owned();
    let output = output_path.to_owned();

    tokio::task::spawn_blocking(move || {
        kob_ffmpeg::subtitles::extract_subtitle(&input, stream_index, &output, None)
            .map_err(|e| TranscodeError::StartFailed(format!("subtitle extraction failed: {e}")))
    })
    .await
    .map_err(|e| TranscodeError::StartFailed(format!("task join error: {e}")))?
}

/// A subtitle track to extract from a container.
pub struct SubtitleExtractSpec {
    pub stream_index: i32,
    pub output_path: PathBuf,
}

/// Extract multiple subtitle tracks in a single pass using kob-ffmpeg.
pub async fn extract_all_subtitles(
    input_path: &Path,
    tracks: &[SubtitleExtractSpec],
) -> Result<(), TranscodeError> {
    if tracks.is_empty() {
        return Ok(());
    }

    // Create output directories
    for track in tracks {
        if let Some(parent) = track.output_path.parent() {
            let _ = tokio::fs::create_dir_all(parent).await;
        }
    }

    let input = input_path.to_owned();
    let extractions: Vec<(i32, PathBuf)> = tracks
        .iter()
        .map(|t| (t.stream_index, t.output_path.clone()))
        .collect();

    tokio::task::spawn_blocking(move || {
        let refs: Vec<(i32, &Path)> = extractions
            .iter()
            .map(|(idx, p)| (*idx, p.as_path()))
            .collect();
        kob_ffmpeg::subtitles::extract_subtitles_batch(&input, &refs, None).map_err(|e| {
            TranscodeError::StartFailed(format!("batch subtitle extraction failed: {e}"))
        })?;
        Ok(())
    })
    .await
    .map_err(|e| TranscodeError::StartFailed(format!("task join error: {e}")))?
}

// =============================================================================
// LUFS loudness measurement
// =============================================================================

/// Measure integrated loudness (LUFS) of a media file's audio.
///
/// Uses kob-ffmpeg's ebur128 filter to measure integrated loudness.
/// Returns `None` if the file has no audio or measurement fails.
pub async fn measure_lufs(input: &Path) -> Result<Option<f64>, TranscodeError> {
    let path = input.to_owned();
    tokio::task::spawn_blocking(move || {
        kob_ffmpeg::audio::measure_lufs(&path, None)
            .map_err(|e| TranscodeError::StartFailed(format!("LUFS measurement failed: {e}")))
    })
    .await
    .map_err(|e| TranscodeError::StartFailed(format!("task join error: {e}")))?
}

// =============================================================================
// Sprite thumbnail generation
// =============================================================================

/// Result of sprite thumbnail generation.
#[derive(Debug)]
pub struct ThumbnailResult {
    /// Number of sprite sheets generated.
    pub sprite_count: u32,
    /// Width of each individual thumbnail in pixels.
    pub thumb_width: u32,
    /// Height of each individual thumbnail in pixels.
    pub thumb_height: u32,
    /// Columns per sprite sheet.
    pub tile_cols: u32,
    /// Rows per sprite sheet.
    pub tile_rows: u32,
    /// Total number of thumbnails generated.
    pub total_thumbs: u32,
    /// Actual capture timestamps for each thumbnail (seconds).
    /// Used to build accurate VTT time ranges.
    pub timestamps: Vec<f64>,
}

/// Encode one chunk of in-memory JPEG frames into a single sprite sheet at
/// `output_path`. Used by both the buffered batch tiler ([`tile_frames_to_sprites`])
/// and the progressive-emission [`crate::media_cache::trickplay_sink::IncrementalSpriteSink`].
/// Returns `(thumb_width, thumb_height)` of the individual tiles.
///
/// `chunk.len()` must be `<= tile_cols * tile_rows`. A short trailing chunk is
/// padded with empty cells (the unfilled cells stay black).
pub fn tile_chunk_to_sprite(
    chunk: &[kob_ffmpeg::thumbnails::JpegFrame],
    output_path: &Path,
    tile_cols: u32,
    tile_rows: u32,
) -> Result<(u32, u32), TranscodeError> {
    use image::GenericImageView;

    if chunk.is_empty() {
        return Err(TranscodeError::StartFailed(
            "tile_chunk_to_sprite called with empty chunk".into(),
        ));
    }

    let first = image::load_from_memory(&chunk[0].jpeg_data)
        .map_err(|e| TranscodeError::StartFailed(format!("Failed to decode first frame: {e}")))?;
    let (thumb_w, thumb_h) = first.dimensions();

    let sprite_w = thumb_w * tile_cols;
    let sprite_h = thumb_h * tile_rows;
    let mut sprite = image::RgbImage::new(sprite_w, sprite_h);

    for (j, frame) in chunk.iter().enumerate() {
        let decoded = image::load_from_memory(&frame.jpeg_data)
            .map_err(|e| TranscodeError::StartFailed(format!("Failed to decode frame: {e}")))?
            .to_rgb8();

        let col = (j as u32) % tile_cols;
        let row = (j as u32) / tile_cols;
        let x_off = col * thumb_w;
        let y_off = row * thumb_h;

        image::imageops::overlay(&mut sprite, &decoded, x_off as i64, y_off as i64);
    }

    sprite
        .save(output_path)
        .map_err(|e| TranscodeError::StartFailed(format!("Failed to save sprite sheet: {e}")))?;

    Ok((thumb_w, thumb_h))
}

/// Tile in-memory JPEG frames into sprite sheets using the `image` crate.
///
/// Takes JPEG bytes directly from the koblibs extractor and decodes them in
/// memory — no disk round-trip. Returns `(thumb_width, thumb_height)` of the
/// individual tiles. Public so the single-pass `media_analyze` handler
/// can produce sprite sheets in the exact same format as the legacy
/// `generate_thumbnails` job.
pub fn tile_frames_to_sprites(
    frames: &[kob_ffmpeg::thumbnails::JpegFrame],
    output_dir: &Path,
    tile_cols: u32,
    tile_rows: u32,
) -> Result<(u32, u32), TranscodeError> {
    let thumbs_per_sprite = (tile_cols * tile_rows) as usize;
    let mut dims: Option<(u32, u32)> = None;
    for (sprite_idx, chunk) in frames.chunks(thumbs_per_sprite).enumerate() {
        let sprite_path = output_dir.join(format!("sprite_{:03}.jpg", sprite_idx + 1));
        let (w, h) = tile_chunk_to_sprite(chunk, &sprite_path, tile_cols, tile_rows)?;
        dims.get_or_insert((w, h));
    }
    dims.ok_or_else(|| TranscodeError::StartFailed("no frames to tile".into()))
}

/// Generate a WebVTT file mapping time ranges to sprite sheet coordinates.
///
/// Uses actual capture timestamps from frame extraction so each entry
/// spans from its capture time to the next, eliminating drift.
pub fn generate_thumbnails_vtt(result: &ThumbnailResult, duration_secs: f64) -> String {
    let mut vtt = String::from("WEBVTT\n\n");
    let thumbs_per_sprite = result.tile_cols * result.tile_rows;
    let count = result.total_thumbs as usize;

    for i in 0..count {
        let start = if i < result.timestamps.len() {
            result.timestamps[i]
        } else {
            // Fallback if timestamps are missing (shouldn't happen)
            0.0
        };
        let end = if i + 1 < result.timestamps.len() {
            result.timestamps[i + 1]
        } else {
            duration_secs
        };

        let sprite_index = i as u32 / thumbs_per_sprite;
        let pos_in_sprite = i as u32 % thumbs_per_sprite;
        let col = pos_in_sprite % result.tile_cols;
        let row = pos_in_sprite / result.tile_cols;

        let x = col * result.thumb_width;
        let y = row * result.thumb_height;

        vtt.push_str(&format!(
            "{} --> {}\nsprite_{:03}.jpg#xywh={},{},{},{}\n\n",
            format_vtt_time(start),
            format_vtt_time(end),
            sprite_index + 1,
            x,
            y,
            result.thumb_width,
            result.thumb_height,
        ));
    }

    vtt
}

fn format_vtt_time(seconds: f64) -> String {
    let total_ms = (seconds * 1000.0) as u64;
    let h = total_ms / 3_600_000;
    let m = (total_ms % 3_600_000) / 60_000;
    let s = (total_ms % 60_000) / 1_000;
    let ms = total_ms % 1_000;
    format!("{h:02}:{m:02}:{s:02}.{ms:03}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::streaming::decision::{
        AudioMode as DecisionAudioMode, PlaybackDecision, PlaybackMethod, SubtitleHandling,
        VideoMode as DecisionVideoMode,
    };

    fn dummy_args(audio_stream_index: i32) -> TranscodeArgs {
        TranscodeArgs {
            input_path: PathBuf::from("/tmp/test.mkv"),
            output_dir: PathBuf::from("/tmp/out"),
            video_mode: VideoMode::Copy,
            audio_mode: AudioMode::Copy,
            audio_stream_index,
            start_time: None,
            segment_duration: 6,
            hw_accel: super::super::detect::HwAccel::None,
            video_codec: "h264".into(),
            target_encoder: "libx264".into(),
            burn_in_subtitle: None,
            scale: None,
            target_bitrate_kbps: None,
            duration_seconds: 120.0,
            initial_generation: 0,
            hw_device: None,
        }
    }

    fn dummy_decision(audio_stream_index: i32) -> PlaybackDecision {
        PlaybackDecision {
            method: PlaybackMethod::VideoTranscode,
            video_mode: DecisionVideoMode::Copy,
            audio_mode: DecisionAudioMode::Copy,
            audio_stream_index,
            subtitle_handling: SubtitleHandling::None,
        }
    }

    #[test]
    fn build_config_for_args_preserves_audio_stream_index_zero() {
        let args = dummy_args(0);
        let config = build_config_for_args(&args);
        assert_eq!(
            config.audio_stream_index,
            Some(0),
            "stream index 0 must not be dropped"
        );
    }

    #[test]
    fn build_config_for_args_preserves_nonzero_audio_stream_index() {
        let args = dummy_args(2);
        let config = build_config_for_args(&args);
        assert_eq!(config.audio_stream_index, Some(2));
    }

    #[test]
    fn build_config_for_hybrid_uses_decision_audio_stream_index() {
        let decision = dummy_decision(3);
        let config = build_config_for_hybrid(
            &decision,
            Path::new("/tmp/out"),
            120.0,
            6,
            "libx264",
            None,
            None,
        );
        assert_eq!(
            config.audio_stream_index,
            Some(3),
            "hybrid config must use decision audio_stream_index, not probe"
        );
    }

    #[test]
    fn build_config_for_hybrid_preserves_audio_stream_index_zero() {
        let decision = dummy_decision(0);
        let config = build_config_for_hybrid(
            &decision,
            Path::new("/tmp/out"),
            120.0,
            6,
            "libx264",
            None,
            None,
        );
        assert_eq!(
            config.audio_stream_index,
            Some(0),
            "stream index 0 must not be dropped in hybrid path"
        );
    }
}
