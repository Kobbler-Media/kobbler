//! Multi-signal intro/credits/post-credits detection pipeline.
//!
//! Three detection signals feed into a fusion layer:
//! 1. Chromaprint audio fingerprinting (cross-episode similarity)
//! 2. Black frame + silence boundary refinement
//! 3. SigLIP2 visual embeddings (optional, requires vision service)

pub mod boundary;
pub mod cache_detect;
pub mod chromaprint;
pub mod fusion;
pub mod post_credits;
pub mod primitives;
pub mod tv;
pub mod vhash;
pub mod vision;

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::repositories::skip_detect_candidates::StoredCandidate;

/// A detected skip segment with confidence and veto scoring.
#[derive(Debug, Clone, Serialize)]
pub struct DetectedSegment {
    pub segment_type: SegmentType,
    pub start_seconds: f64,
    pub end_seconds: f64,
    pub confidence: f64,
    pub veto_score: f64,
    pub source: String,
}

impl DetectedSegment {
    /// Effective confidence after applying veto.
    pub fn effective_confidence(&self) -> f64 {
        self.confidence * (1.0 - self.veto_score)
    }

    /// Whether this segment should be persisted.
    pub fn should_persist(&self) -> bool {
        self.effective_confidence() >= MIN_EFFECTIVE_CONFIDENCE
    }
}

/// Outcome of an audio detection pass.
///
/// `Completed` — detection ran and produced a (possibly empty) candidate
/// list. Caller should store candidates and record an `enrichment_attempts`
/// row so backfill stops re-enqueuing.
///
/// `NoProbeSucceeded` — every ffmpeg probe failed (unreadable file,
/// unsupported codec, systemic ffmpeg issue). Caller should NOT record an
/// attempt and should return a retryable error so backfill comes back later.
#[derive(Debug)]
pub enum DetectionOutcome<T> {
    Completed(T),
    NoProbeSucceeded,
}

/// Tally of ffmpeg probe calls made during detection. Used to distinguish
/// "legitimately nothing found" (attempted > 0, succeeded > 0, candidates
/// empty) from "couldn't probe at all" (attempted > 0, succeeded == 0) so
/// the caller can decide whether to record an enrichment attempt or retry.
#[derive(Debug, Default, Clone, Copy)]
pub struct ProbeHealth {
    pub attempted: usize,
    pub succeeded: usize,
}

impl ProbeHealth {
    pub fn record_ok(&mut self) {
        self.attempted += 1;
        self.succeeded += 1;
    }

    pub fn record_err(&mut self) {
        self.attempted += 1;
    }

    pub fn merge(&mut self, other: ProbeHealth) {
        self.attempted += other.attempted;
        self.succeeded += other.succeeded;
    }

    /// True if the detection had a fair shot — either nothing needed probing
    /// (empty input) or at least one probe completed successfully.
    pub fn had_fair_shot(&self) -> bool {
        self.attempted == 0 || self.succeeded > 0
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SegmentType {
    Intro,
    Credits,
    PostCreditsScene,
    Recap,
}

impl SegmentType {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Intro => "intro",
            Self::Credits => "credits",
            Self::PostCreditsScene => "post_credits_scene",
            Self::Recap => "recap",
        }
    }
}

/// Signals collected for a candidate region.
#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct CandidateSignals {
    /// Chromaprint audio match density (0.0-1.0). Higher = more matched samples.
    pub chromaprint_score: Option<f64>,
    /// SigLIP2 cross-episode visual similarity.
    pub visual_score: Option<f64>,
    /// dHash visual sequence match density (0.0-1.0).
    pub dhash_score: Option<f64>,
    /// SigLIP2 zero-shot credit text detection confidence (0.0-1.0).
    pub credit_text_score: Option<f64>,
    pub boundary_match: Option<BoundaryMatch>,
    pub audio_profile: Option<AudioProfile>,
    /// Pairwise match detail for debugging (chromaprint pairs, dHash pairs, etc.)
    #[serde(default)]
    pub match_detail: Option<serde_json::Value>,
    /// Chapter marker agreement: positive = confirms detection, negative = contradicts.
    pub chapter_agreement: Option<f64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BoundaryMatch {
    pub has_black_frame: bool,
    pub has_silence: bool,
    pub refined_start: f64,
    pub refined_end: f64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum AudioProfile {
    MusicOnly,
    Dialogue,
    NarrationLoop,
    Mixed,
    Unknown,
}

/// A candidate region before fusion scoring.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Candidate {
    pub segment_type: SegmentType,
    pub start_seconds: f64,
    pub end_seconds: f64,
    pub signals: CandidateSignals,
    /// Episode/file duration for proportion-based constraints (credits start %, max duration %).
    #[serde(default)]
    pub file_duration: Option<f64>,
}

/// Pipeline configuration derived from RuntimeConfig.
#[derive(Debug, Clone)]
pub struct PipelineConfig {
    pub mode: PipelineMode,
    pub vision_service_url: Option<String>,
    pub reuse_sprites: bool,
    pub storage_root: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PipelineMode {
    Off,
    Audio,
    Full,
}

impl std::str::FromStr for PipelineMode {
    type Err = std::convert::Infallible;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(match s {
            "audio" => Self::Audio,
            "full" => Self::Full,
            _ => Self::Off,
        })
    }
}

// Thresholds — matching layer
/// Minimum match duration in the audio matching layer (prevents sound effects).
/// Short legitimate intros (e.g. Scrubs 5s theme) are preserved.
pub const MIN_MATCH_DURATION: f64 = 5.0;
/// Minimum match density (ratio of matched samples to expected samples in region).
/// Below this, the match is likely ambient audio coincidence, not a real theme.
pub const MIN_MATCH_DENSITY: f64 = 0.7;
/// Default BER threshold for intro/credits matching (max 6 bits out of 32 different).
pub const INTRO_MAX_BER: f64 = 0.20;

// Thresholds — UI/skip layer
/// Minimum duration to show a "skip" button in the player UI.
pub const MIN_SKIP_DURATION: f64 = 5.0;

// Thresholds — intro
/// Under 10s, "skip intro" button latency is close to just watching it;
/// short real intros below this threshold aren't worth the UX cost of an
/// interactive skip. Bumped from 5s (2026-04-22) to filter network idents
/// and short logo cards that match across episodes. Scrubs sits at ~12s so
/// short-theme shows still pass.
pub const MIN_INTRO_DURATION: f64 = 10.0;
pub const MAX_INTRO_DURATION: f64 = 150.0; // 2.5 minutes — no legitimate intro is longer

// Thresholds — credits
pub const MIN_CREDITS_DURATION: f64 = 20.0;
pub const MIN_CREDITS_START_PCT: f64 = 0.85; // Credits can't start before 85% of episode
pub const MAX_CREDITS_DURATION_PCT: f64 = 0.15; // Max 15% of episode length
pub const MAX_CREDITS_DURATION_ABS: f64 = 720.0; // Hard cap: 12 minutes (matches CREDITS_SCAN_WINDOW; accommodates long Pixar/Marvel + streaming multi-language credit rolls)

// Thresholds — post-credits
pub const MIN_POST_CREDITS_DURATION: f64 = 5.0;
pub const MAX_POST_CREDITS_DURATION: f64 = 120.0; // 2 minutes

// Thresholds — fusion
pub const MIN_EFFECTIVE_CONFIDENCE: f64 = 0.55;
/// Tail window (seconds) used by the embed_src branch — feeds vision SigLIP2
/// for credits classification. The detector vision scan reads back from the
/// same window, so producer (`media_cache::analyze`) and consumer
/// (`cache_detect`) must agree.
///
/// Movies need wider coverage than TV: long Pixar/Marvel/Wes Anderson credit
/// rolls run ~12 min, plus we want ~2 min of pre-credits movie content as a
/// non-credits baseline for the classifier to contrast against.
pub fn credits_window_secs(media_type: &str) -> u32 {
    match media_type {
        "movie" => 960, // 16 min
        _ => 480,       // 8 min — episode + everything else
    }
}

/// Dynamic extraction window based on episode length, matching Emby's approach.
/// Longer episodes get wider windows to catch intros after cold opens.
pub fn extract_duration_secs(episode_duration: f64) -> u32 {
    if episode_duration < 900.0 {
        // < 15 min: 8 min window
        480
    } else if episode_duration < 2400.0 {
        // < 40 min: 15 min window
        900
    } else {
        // 40+ min: 20 min window
        1200
    }
}

/// Compute extraction windows for all detection signals.
/// Returns `(intro_end_secs, credits_start_secs)`.
/// Covers chromaprint, dHash/vHash, and vision extraction ranges.
pub fn extraction_windows(duration: f64) -> (f64, f64) {
    let intro_window = extract_duration_secs(duration) as f64;
    // Credits: chromaprint uses `duration - extract_duration_secs`,
    // dHash/vHash uses `duration * 0.80`. Take the earlier start (more coverage).
    let chroma_credits_start = (duration - extract_duration_secs(duration) as f64).max(0.0);
    let dhash_credits_start = duration * 0.80;
    let credits_start = chroma_credits_start.min(dhash_credits_start);
    (intro_window, credits_start)
}

/// Find the best matching candidate for a segment by (file_id, segment_type)
/// with closest start_seconds within 0.5s epsilon.
pub fn find_matching_candidate<'a>(
    candidates: &'a [StoredCandidate],
    file_id: Uuid,
    segment_type: &str,
    start_seconds: f64,
) -> Option<&'a StoredCandidate> {
    const EPSILON: f64 = 0.5;
    candidates
        .iter()
        .filter(|c| c.file_id == file_id && c.segment_type == segment_type)
        .filter(|c| (c.start_seconds - start_seconds).abs() < EPSILON)
        .min_by(|a, b| {
            let da = (a.start_seconds - start_seconds).abs();
            let db = (b.start_seconds - start_seconds).abs();
            da.partial_cmp(&db).unwrap_or(std::cmp::Ordering::Equal)
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extract_duration_short_episodes() {
        // 5-minute episode → 8 min window
        assert_eq!(extract_duration_secs(300.0), 480);
        // 14 min → still short
        assert_eq!(extract_duration_secs(840.0), 480);
    }

    #[test]
    fn extract_duration_medium_episodes() {
        // 22-minute episode → 15 min window
        assert_eq!(extract_duration_secs(1320.0), 900);
        // 39 min → still medium
        assert_eq!(extract_duration_secs(2340.0), 900);
    }

    #[test]
    fn extract_duration_long_episodes() {
        // 45-minute episode → 20 min window
        assert_eq!(extract_duration_secs(2700.0), 1200);
        // 60 min → 20 min
        assert_eq!(extract_duration_secs(3600.0), 1200);
    }

    #[test]
    fn extract_duration_boundary_values() {
        // Exactly at thresholds
        assert_eq!(extract_duration_secs(900.0), 900); // 15 min → medium
        assert_eq!(extract_duration_secs(2400.0), 1200); // 40 min → long
    }

    #[test]
    fn extraction_windows_55min_episode() {
        // 55 min = 3300s → 20 min window
        let (intro, credits_start) = extraction_windows(3300.0);
        assert_eq!(intro, 1200.0); // 20 min intro window
        // credits: min(3300-1200=2100, 3300*0.80=2640) = 2100
        assert_eq!(credits_start, 2100.0);
    }

    #[test]
    fn extraction_windows_22min_episode() {
        // 22 min = 1320s → 15 min window
        let (intro, credits_start) = extraction_windows(1320.0);
        assert_eq!(intro, 900.0); // 15 min intro window
        // credits: min(1320-900=420, 1320*0.80=1056) = 420
        assert_eq!(credits_start, 420.0);
    }
}
