//! Playback session management.
//!
//! Manages in-memory playback sessions, each wrapping an optional FFmpeg
//! transcode process and tracking client activity for cleanup.

use crate::ffmpeg::cue_gate::CueGate;
use crate::ffmpeg::detect::HwAccel;
use crate::ffmpeg::transcode::TranscodeProcess;
use crate::streaming::decision::{AudioTrackMeta, PlaybackDecision};
use crate::streaming::download_meter::DownloadMeter;
use crate::streaming::feed::StreamFeed;
use crate::streaming::segment_cache::SegmentCache;
use crate::streaming::variant::QualityVariant;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Instant;
use uuid::Uuid;

/// Type of media being played.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MediaType {
    Movie,
    Episode,
    Recording,
}

impl MediaType {
    pub fn as_str(self) -> &'static str {
        match self {
            MediaType::Movie => "movie",
            MediaType::Episode => "episode",
            MediaType::Recording => "recording",
        }
    }
}

/// A terminal playback error surfaced to the client. The `code` is a stable
/// snake_case identifier the frontend switches on; `message` is human-readable
/// detail for the error UI / debugging.
#[derive(Debug, Clone)]
pub struct SessionError {
    pub code: &'static str,
    pub message: String,
}

impl SessionError {
    pub fn new(code: &'static str, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
        }
    }
}

/// A pending segment request used by the demand-driven stall detector. The
/// segment handler records the index of a segment that doesn't exist on disk
/// yet; if the same index is still missing after `STALL_THRESHOLD`, the
/// session is marked stalled.
#[derive(Debug, Clone, Copy)]
pub struct PendingSegmentRequest {
    pub segment_index: u32,
    pub first_requested_at: Instant,
}

/// A playback session tracked in memory.
pub struct PlaybackSession {
    pub id: Uuid,
    pub user_id: Uuid,
    pub media_type: MediaType,
    pub file_id: Uuid,
    /// Absolute path to the media file on disk.
    pub file_path: PathBuf,
    /// The decision engine result.
    pub decision: PlaybackDecision,
    /// FFmpeg process (None for DirectPlay).
    pub transcode: Option<TranscodeProcess>,
    /// Segment cache tracking segments across FFmpeg generations.
    pub segment_cache: SegmentCache,
    /// When the session was created.
    pub created_at: Instant,
    /// Last time the client accessed any endpoint for this session.
    pub last_activity: Instant,
    /// Incremented on each seek to track discontinuities.
    pub seek_generation: u32,
    /// Total duration of the media file in seconds.
    pub duration_seconds: f64,
    /// Container format of the source file.
    pub container: String,
    /// Source video codec (e.g. "hevc", "h264") for codec-specific FFmpeg flags.
    pub video_codec: String,
    /// Available subtitle tracks with their extracted paths (populated at session creation).
    pub subtitle_files: Vec<SubtitleFile>,
    /// Hardware acceleration in use for this session's transcode.
    pub hw_accel: HwAccel,
    /// Source video resolution (width, height) for ABR variant computation.
    pub source_resolution: (u32, u32),
    /// Additional ABR quality variants (None for non-ABR sessions).
    pub variants: Option<HashMap<String, QualityVariant>>,
    /// Whether this is a live stream (IPTV) rather than a finite media file.
    pub is_live: bool,
    /// For IPTV sessions: the provider that owns the upstream connection.
    /// Used to release the provider connection slot when the session ends.
    pub provider_id: Option<Uuid>,
    /// Client-requested maximum video bitrate in kbps.
    /// Used to filter ABR variants in the master playlist.
    pub max_bitrate_kbps: Option<u32>,
    /// Pre-computed keyframe timestamps for playlist prediction.
    /// Shared with transcode sessions for immediate playlist generation.
    pub keyframes: Option<Vec<f64>>,
    /// Set when the session is in a terminal error state (probe failure,
    /// transcode crash, stall, etc.). The playlist and segment handlers
    /// return an HTTP 500 with a machine-readable error code when this is set.
    pub error: Option<SessionError>,
    /// Demand-driven stall detector: tracks the latest segment the player
    /// requested that didn't yet exist on disk. If the same index is still
    /// missing after `STALL_THRESHOLD`, the session is marked stalled.
    pub pending_segment: Option<PendingSegmentRequest>,
    /// Cancellation token for debrid progressive download (None for local files).
    /// Cancelled when the session is destroyed to stop the background download.
    pub download_cancel: Option<tokio_util::sync::CancellationToken>,
    /// Override directory for thumbnails. When set, the serve handler reads
    /// thumbnails from this path instead of `{storage_root}/thumbnails/{file_id}/`.
    /// Used by debrid sessions to serve thumbnails from the cache directory.
    pub thumbnail_dir: Option<PathBuf>,
    /// Dedup key for debrid sessions (e.g. "infohash" or "infohash_f3").
    /// Used to remove the entry from `debrid_session_index` when the session is reaped.
    pub debrid_cache_key: Option<String>,
    /// Audio track metadata from probe/DB — stored so the decision engine can be
    /// re-run when switching audio tracks without re-probing the file.
    pub audio_metas: Vec<AudioTrackMeta>,
    /// Debrid sessions only: the audio track the viewer's language preference picks, once
    /// the probe has listed the tracks. `None` when no track matches.
    pub preferred_audio_index: Option<i32>,
    /// Force the video stream to transcode even if the decision engine chose
    /// `VideoMode::Copy`. Set by the Tier-3 decode-error endpoint when the
    /// player reports a `MEDIA_ERR_DECODE` from corrupt source bitstream so
    /// subsequent FFmpeg starts emit a clean re-encoded stream that the
    /// browser HW decoder will accept.
    pub force_video_transcode: bool,
    /// Time ranges where media_analyze observed decoder errors, fetched from
    /// the file row's `corrupt_ranges` column at session creation. The lazy
    /// overlay scheduler reads this to pre-emptively spawn overlay transcode
    /// generations as the playhead approaches each range.
    pub corrupt_ranges: Vec<crate::corruption::CorruptRange>,
    /// Set of corrupt-range start timestamps for which an overlay generation
    /// has already been spawned this session (avoids re-spawning on every
    /// playlist request). Identifying ranges by their start timestamp is
    /// sufficient because `corrupt_ranges` is canonicalised (no overlaps).
    pub overlay_spawned_starts: std::collections::HashSet<u64>,
    /// Highest absolute segment index the player has *requested* this session.
    /// Updated by `serve_segment`, used by `maybe_spawn_overlays` as the real
    /// playhead signal — `playback_start_segment` only updates on seek and
    /// would never advance during continuous playback.
    pub highest_served_segment: Option<u32>,
    /// Debrid sessions only: the player's byte feed, reported in the session status.
    pub feed: Option<Arc<StreamFeed>>,
    /// Debrid sessions only: the progressive download into the cache file.
    pub download: Option<Arc<DownloadMeter>>,
    /// Debrid sessions only: the cue trim decision every response must agree on.
    pub cue_gate: Option<Arc<CueGate>>,
    /// Has a `playback_sessions` row to be rebuilt from after a restart.
    pub persisted: bool,
}

/// An extracted/prepared subtitle file for a session.
#[derive(Debug, Clone)]
pub struct SubtitleFile {
    pub track_id: Uuid,
    pub stream_index: i32,
    pub codec: String,
    pub language: Option<String>,
    pub title: Option<String>,
    /// Path to the extracted subtitle file on disk (in session's base dir).
    pub file_path: PathBuf,
    /// Whether the subtitle file has been extracted from the container.
    /// When false, extraction happens on-demand when the subtitle is requested.
    pub extracted: bool,
    pub is_forced: bool,
    pub is_hearing_impaired: bool,
}

impl PlaybackSession {
    /// Create a minimal DirectPlay session for testing.
    #[doc(hidden)]
    pub fn test_direct_play(id: Uuid, user_id: Uuid, file_path: PathBuf) -> Self {
        Self {
            id,
            user_id,
            media_type: MediaType::Movie,
            file_id: id,
            file_path,
            decision: PlaybackDecision {
                method: crate::streaming::decision::PlaybackMethod::DirectPlay,
                video_mode: crate::streaming::decision::VideoMode::Copy,
                audio_mode: crate::streaming::decision::AudioMode::Copy,
                audio_stream_index: 0,
                subtitle_handling: crate::streaming::decision::SubtitleHandling::None,
                deinterlace: false,
            },
            transcode: None,
            segment_cache: SegmentCache::new(PathBuf::new(), 6, 0.0),
            created_at: Instant::now(),
            last_activity: Instant::now(),
            seek_generation: 0,
            duration_seconds: 120.0,
            container: "matroska,webm".to_string(),
            video_codec: "h264".to_string(),
            subtitle_files: vec![],
            hw_accel: HwAccel::None,
            source_resolution: (1920, 1080),
            variants: None,
            is_live: false,
            provider_id: None,
            max_bitrate_kbps: None,
            keyframes: None,
            error: None,
            pending_segment: None,
            download_cancel: None,
            thumbnail_dir: None,
            debrid_cache_key: None,
            audio_metas: vec![],
            preferred_audio_index: None,
            force_video_transcode: false,
            corrupt_ranges: Vec::new(),
            overlay_spawned_starts: std::collections::HashSet::new(),
            highest_served_segment: None,
            feed: None,
            download: None,
            cue_gate: None,
            persisted: false,
        }
    }

    /// The session's base directory (containing gen_N/ subdirs and subtitles).
    pub fn segment_dir(&self) -> &Path {
        &self.segment_cache.base_dir
    }

    /// Start the playlist somewhere else. The player's progress through the old one says
    /// nothing about where it is in this one.
    pub fn move_playlist_start(&mut self, segment: u32, secs: f64) {
        self.segment_cache.set_playback_start(segment, secs);
        self.highest_served_segment = None;
    }

    /// Update the last activity timestamp (call on every client request).
    pub fn touch(&mut self) {
        self.last_activity = Instant::now();
    }

    /// Whether this session has been inactive longer than the given duration.
    pub fn is_expired(&self, max_idle: std::time::Duration) -> bool {
        self.last_activity.elapsed() > max_idle
    }

    /// Whether this session has ABR variants.
    pub fn is_abr(&self) -> bool {
        self.variants.as_ref().is_some_and(|v| !v.is_empty())
    }

    /// Kill all FFmpeg processes (primary + all ABR variants).
    pub async fn kill_all_transcodes(&mut self) {
        if let Some(ref mut transcode) = self.transcode {
            transcode.kill().await;
        }
        if let Some(ref mut variants) = self.variants {
            for variant in variants.values_mut() {
                if let Some(ref mut transcode) = variant.transcode {
                    transcode.kill().await;
                }
            }
        }
    }

    /// Kill FFmpeg process only (no file cleanup — that's a NATS job).
    pub async fn kill_transcode(&mut self) {
        if let Some(ref mut transcode) = self.transcode {
            transcode.kill().await;
        }
    }

    /// Kill FFmpeg and clean up segment directory.
    /// Used only as a fallback when NATS cleanup can't be enqueued.
    pub async fn cleanup(&mut self) {
        self.kill_all_transcodes().await;
        let dir = self.segment_dir();
        if dir.exists() {
            let _ = tokio::fs::remove_dir_all(dir).await;
        }
    }
}
