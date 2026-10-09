//! Cache-driven skip-detect: orchestrates intro/credits/post-credits detection
//! entirely from cached sidecars produced by `media_analyze`. Every ffmpeg
//! probe is replaced by a read from `{storage_root}/media_cache/<file_id>/`.
//! Called by the `skip_detect` job handler.
//!
//! Shared math with the (now-retired) V1 path via
//! `find_per_episode_positions`, `find_per_episode_vhash`,
//! `primitives::boundary::refine_boundary`, `credit_scoring::score_credit_text`,
//! `fusion::score_candidates`, `detect_post_credits_from_cache`.

use std::collections::HashMap;

use serde::Serialize;
use sqlx::PgPool;
use tracing::{debug, warn};
use uuid::Uuid;

use super::chromaprint::Fingerprint;
use super::fusion::{RejectedCandidate, pre_fusion_hard_reject, score_candidates_with_rejections};
use super::post_credits::detect_post_credits_from_cache;
use super::primitives::boundary as boundary_primitive;
use super::primitives::boundary::BoundarySearchDirection;
use super::primitives::cosine::cluster_consistency;
use super::primitives::credit_scoring::{CREDIT_PROMPTS, NON_CREDIT_PROMPTS, score_credit_text};
use super::tv::{
    EpisodeFile, chapter_override_position, find_per_episode_positions, find_per_episode_vhash,
};
use super::vhash::FrameDescriptor;
use super::vision::VisionClient;
use super::{
    BoundaryMatch, Candidate, CandidateSignals, DetectedSegment, INTRO_MAX_BER,
    MIN_CREDITS_DURATION, MIN_MATCH_DURATION, MIN_SKIP_DURATION, SegmentType,
    extract_duration_secs, extraction_windows,
};
use crate::media_cache::readers::{self, BlackGap, EmbeddingsView, SilenceGap};
use crate::repositories::video_chapters;
use kob_ffmpeg::filter::detect::{BlackInterval, SilenceInterval};

/// Cap for intro-boundary refinement against whole-file cache intervals.
/// Mirrors V1's `refine_boundary(..., 3.0)` call site for intro start/end.
const BOUNDARY_REFINE_WINDOW_SECS: f64 = 3.0;
/// Cap for credits-start boundary refinement. Slightly wider than intro
/// because credits transitions sometimes have a longer fade + silence span.
const BOUNDARY_CREDITS_WINDOW_SECS: f64 = 5.0;
/// Tolerance for "chroma and vhash agree" — if both pure cross-episode
/// signals land within this many seconds of each other on a given axis,
/// skip boundary refinement for that axis. Two independent signals
/// converging on a point is stronger evidence than a nearby black+silence
/// co-occurrence, and snapping to a black pair risks crossing an actual
/// content transition (logo-card fade, pre-show gap).
const SIGNALS_AGREE_TOLERANCE_SECS: f64 = 1.0;

/// Minimum fraction of a vhash-only candidate window that can overlap a
/// single black-frame interval before we reject the candidate as a pure
/// black-screen transition (title card, fade-to-black act break). Black
/// frames are visually identical across every episode, so vhash scores
/// them at near-1.0 density — without this filter the detector invents
/// intros/credits wherever any two episodes share a ≥5s black stretch.
const BLACK_FRAME_COVERAGE_REJECT: f64 = 0.8;

/// Minimum black-run duration that marks the transition into a credit roll on
/// its own. Below this it is an ordinary cut or an in-scene fade.
const MIN_CREDITS_BLACK_SECS: f64 = 1.5;
/// A black run this long *is* the roll — white-on-black credits read as black
/// frames end to end. The one boundary shape strong enough to correct a vision
/// start (see [`pick_credits_start`]).
const STRONG_CREDITS_BLACK_SECS: f64 = 10.0;
/// Minimum silence that can mark the credits boundary, and only when the
/// picture goes dark with it.
const MIN_CREDITS_SILENCE_SECS: f64 = 1.5;
/// How far apart a silence and a black interval may sit and still count as one
/// fade-to-credits transition.
const CREDITS_SILENCE_BLACK_TOLERANCE: f64 = 2.0;
/// A qualifying boundary this far ahead of vision's first credit-text frame is
/// the fade into the roll. Vision samples at 1 fps and its first hit lands a
/// beat late, so snapping back to the boundary is a strict improvement.
const CREDITS_BOUNDARY_SNAP_SECS: f64 = 15.0;
/// Vision's credit region starting this close to the start of its embed window
/// means the roll began before the window did — its start is a floor, not a
/// finding.
const VISION_WINDOW_EDGE_SECS: f64 = 3.0;

/// Minimum duration for a vhash-only intro to be kept. Below this, the
/// match is almost certainly a shared-ident (HBO logo, network card) or
/// similar cross-episode visual that isn't worth a "skip intro" button
/// anyway — under 10s the button latency ≈ just watching. Chroma-confirmed
/// intros keep the looser `MIN_INTRO_DURATION` (5s) floor: Scrubs has a
/// ~12–16s theme that audio+vhash both match, so the chroma path handles
/// legitimate short intros. Vhash-only is the path prone to false positives
/// (logos, transition black cards, recurring scene framing) — a dropped
/// intro is preferred over a wrong one.
const MIN_VHASH_ONLY_INTRO_DURATION: f64 = 10.0;

/// Per-episode intermediate state captured during detection.
///
/// Production callers discard this via the DB-wrapping `detect_season_from_cache`
/// (and `detect_movie_from_cache`) entry points. Debug / bench tools use the
/// `_with_chapters` variants and print every pass's output.
#[derive(Debug, Clone, Default, Serialize)]
pub struct EpisodeDiagnostics {
    pub file_id: Uuid,
    pub duration: f64,
    pub chromaprint_samples: usize,
    pub vhash_frames: usize,
    pub silence_count: usize,
    pub black_frames_count: usize,
    pub chapter_count: usize,
    /// Chromaprint cross-episode match — intro region (absolute time).
    pub intro_chromaprint: Option<MatchDiag>,
    /// vHash cross-episode match — intro region.
    pub intro_vhash: Option<MatchDiag>,
    /// Chromaprint cross-episode match — credits region (absolute time,
    /// already flipped back from the reversed end-window).
    pub credits_chromaprint: Option<MatchDiag>,
    /// vHash cross-episode match — credits region.
    pub credits_vhash: Option<MatchDiag>,
    /// Raw candidate list pre-fusion (post boundary refinement, post chapter
    /// override, post post-credits).
    pub candidates_pre_fusion: Vec<Candidate>,
    /// Final fused segments (what the handler would persist).
    pub final_segments: Vec<DetectedSegment>,
    /// Candidates that were dropped by fusion, with the reason. Callers
    /// persist these to `skip_detect_candidates` so missed-segment debugging
    /// is queryable from the DB.
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub rejected_candidates: Vec<RejectedCandidate>,
    /// Human-readable narrative of passes that produced nothing or contradicted.
    pub notes: Vec<String>,
    /// Raw `(text_start, text_end, confidence)` returned by
    /// `classify_credit_text` BEFORE `apply_vision_credit_text` merged it
    /// into an existing Credits candidate. Useful for spotting cases where
    /// the merge conflates a vision-detected region with a chromaprint/vhash
    /// candidate at a wildly different timestamp (observed on Leftovers 2x01).
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub vision_credit_text_raw: Option<(f64, f64, f64)>,
}

#[derive(Debug, Clone, Serialize)]
pub struct MatchDiag {
    pub start_seconds: f64,
    pub end_seconds: f64,
    pub score: f64,
    /// For vhash, mean per-frame Pearson similarity across the matched run
    /// (averaged across pairs). Distinguishes a dense-real run (0.9+) from
    /// a dense-but-marginal sandwich run (0.75-0.82). None for chroma.
    #[serde(default)]
    pub mean_similarity: Option<f64>,
    pub pair_count: usize,
    /// Per-pair endpoints from vhash/chromaprint cross-episode matching.
    /// For vhash: this episode's match boundaries against each other episode
    /// (lhs_start..lhs_end). Useful for spotting outlier pairs that drag the
    /// aggregate min-start/max-end envelope past the real intro/credits edges.
    #[serde(default)]
    pub pairs: Vec<PairDiag>,
}

#[derive(Debug, Clone, Serialize)]
pub struct PairDiag {
    pub vs_ep: usize,
    pub density: f64,
    /// Mean per-frame similarity for this pair's matched run (vhash only).
    #[serde(default)]
    pub mean_similarity: Option<f64>,
    pub lhs_start: f64,
    pub lhs_end: f64,
}

/// Movie-file handle for cache-driven detection. `movie_id` is the canonical
/// `movies.id` UUID when known; the cache_detect resolver prefers
/// `episode_cache/movie/{movie_id}/` over `media_cache/{file_id}/` when set.
/// Native ingest callers without a canonical id pass `None`.
#[derive(Debug, Clone, Copy)]
pub struct MovieHandle {
    pub file_id: Uuid,
    pub duration_seconds: f64,
    pub movie_id: Option<Uuid>,
}

/// Per-episode cache view built up by [`detect_season_from_cache`]. Re-used
/// across all the per-episode stages so we only read each sidecar once.
struct EpisodeCache {
    file_id: Uuid,
    duration: f64,
    chromaprint: Vec<u32>,
    vhash: Vec<FrameDescriptor>,
    silence: Vec<SilenceInterval>,
    black_frames: Vec<BlackInterval>,
    audio_peaks: Vec<u8>,
    embeddings: Option<EmbeddingsView>,
}

/// Detect skip segments for an entire TV season using only cached sidecars.
///
/// Returns one entry per episode whose cache loaded successfully. Episodes
/// missing any required sidecar (manifest, chromaprint, vhash) are silently
/// skipped — the caller's sibling-readiness gate should have filtered these
/// out already. `silence`, `black_frames`, and `audio_peaks` are optional:
/// `audio_peaks` is audiobook-only and is never produced for video files, so
/// requiring it here would silently drop every episode and produce zero
/// segments; downstream consumers (`detect_post_credits_from_cache`,
/// `classify_audio_profile_from_cache`) return `None`/empty when the peaks
/// buffer is empty, so the absence cleanly degrades to "no post-credits
/// detection" for video.
pub async fn detect_season_from_cache(
    pool: &PgPool,
    config_vision_url: Option<&str>,
    episodes: &[EpisodeFile],
    storage_root: &str,
) -> Vec<(Uuid, Vec<DetectedSegment>, EpisodeDiagnostics)> {
    let mut chapters_by_file: HashMap<Uuid, Vec<(f64, Option<String>)>> = HashMap::new();
    for ep in episodes {
        chapters_by_file.insert(ep.file_id, load_chapters_for_file(pool, ep.file_id).await);
    }
    detect_season_from_cache_with_chapters(
        config_vision_url,
        episodes,
        storage_root,
        &chapters_by_file,
    )
    .await
}

/// Pure variant: takes pre-loaded chapters, returns segments + diagnostics.
///
/// Does not touch the database — the only I/O is cache reads under
/// `storage_root` and (optionally) an HTTP call to `config_vision_url`.
/// Mirrors `detect_season_from_cache` exactly; that function is a thin DB
/// wrapper around this one.
pub async fn detect_season_from_cache_with_chapters(
    config_vision_url: Option<&str>,
    episodes: &[EpisodeFile],
    storage_root: &str,
    chapters_by_file: &HashMap<Uuid, Vec<(f64, Option<String>)>>,
) -> Vec<(Uuid, Vec<DetectedSegment>, EpisodeDiagnostics)> {
    if episodes.is_empty() {
        return Vec::new();
    }

    // Load all episode caches up front. Track cache-incomplete episodes too
    // so the caller sees why they produced no segments.
    //
    // On the blocking pool for the same reason as the pairwise block below:
    // these are synchronous `std::fs` reads, one sidecar set per episode, and a
    // whole season's worth of them on a runtime worker stalls the I/O driver.
    let (caches, skipped) = {
        let episodes = episodes.to_vec();
        let storage_root = storage_root.to_string();
        tokio::task::spawn_blocking(move || {
            let mut caches: Vec<EpisodeCache> = Vec::new();
            let mut skipped: Vec<(Uuid, f64)> = Vec::new();
            for ep in &episodes {
                match load_cache_for_episode(&storage_root, ep) {
                    Some(c) => caches.push(c),
                    None => {
                        debug!(file_id = %ep.file_id, "cache incomplete — skipping");
                        skipped.push((ep.file_id, ep.duration_seconds));
                    }
                }
            }
            (caches, skipped)
        })
        .await
        .expect("skip-detect cache load task panicked")
    };

    let mut out: Vec<(Uuid, Vec<DetectedSegment>, EpisodeDiagnostics)> = Vec::new();

    if caches.len() < 2 {
        // Cross-episode matching needs ≥ 2 episodes. Fall back to per-file
        // (boundary + credits + post-credits only, no intro).
        for cache in &caches {
            let chapters = chapters_by_file
                .get(&cache.file_id)
                .cloned()
                .unwrap_or_default();
            let (segments, mut diag) =
                detect_single_file_with_diag(cache, config_vision_url, &chapters).await;
            diag.notes.push(
                "fallback: single-file path (needed ≥2 episodes for cross-correlation)".into(),
            );
            out.push((cache.file_id, segments, diag));
        }
        for (fid, dur) in skipped {
            let mut diag = EpisodeDiagnostics {
                file_id: fid,
                duration: dur,
                ..Default::default()
            };
            diag.notes.push("cache incomplete — not analyzed".into());
            out.push((fid, Vec::new(), diag));
        }
        return out;
    }

    // Build start-window and end-window fingerprints for cross-correlation.
    // Matches the slicing in `extract_season_fingerprints`.
    let (start_fps, end_fps) = build_fingerprint_slices(&caches);
    // Visual corroboration slices, built here so the pairwise block below can
    // take ownership of everything it needs in one move.
    let (intro_vhash_seqs, credits_vhash_seqs) = build_vhash_slices(&caches);

    // Per-episode intro/credits positions from pairwise matching, plus the
    // vhash corroboration maps.
    //
    // ⚠️ This MUST stay on the blocking pool. Pairwise matching is O(n²) in
    // episode count with no await point anywhere inside it — a 52-episode
    // season is 1326 pairs per call and holds its thread for tens of seconds
    // (`primitives::vhash_align::find_visual_matches` measured ~96% of this
    // job's CPU). Left on a runtime worker it doesn't just slow this job down:
    // the worker it occupies is also the one driving tokio's I/O driver, so
    // epoll stops being polled and *every* endpoint stops answering — trivial
    // state-less handlers included — until the season finishes. That reads as
    // "kobbler is down", and the watchdog in `main.rs` then kills the process.
    let (per_ep_intro, per_ep_credits, vhash_intro_map, vhash_credits_map) =
        tokio::task::spawn_blocking(move || {
            (
                find_per_episode_positions(&start_fps, MIN_MATCH_DURATION),
                find_per_episode_positions(&end_fps, MIN_CREDITS_DURATION),
                find_per_episode_vhash(&intro_vhash_seqs, MIN_MATCH_DURATION),
                find_per_episode_vhash(&credits_vhash_seqs, MIN_MATCH_DURATION),
            )
        })
        .await
        .expect("skip-detect pairwise matching task panicked");

    let vision_client = config_vision_url
        .filter(|u| !u.is_empty())
        .map(VisionClient::new);
    let vision_healthy = match vision_client.as_ref() {
        Some(c) => c.health().await,
        None => false,
    };

    // Minimum sibling agreement for vhash-only fallback matches. 4+ ep seasons
    // require ≥3 agreeing episodes; 2-3 ep seasons accept whatever's possible
    // (cap at `ep_count - 1`). Same reasoning as chromaprint's min_matches_per_ep.
    let min_vhash_pairs: usize = 3.min(caches.len().saturating_sub(1));

    for (ep_idx, cache) in caches.iter().enumerate() {
        let empty_chapters = Vec::new();
        let chapters = chapters_by_file
            .get(&cache.file_id)
            .unwrap_or(&empty_chapters);

        let mut diag = EpisodeDiagnostics {
            file_id: cache.file_id,
            duration: cache.duration,
            chromaprint_samples: cache.chromaprint.len(),
            vhash_frames: cache.vhash.len(),
            silence_count: cache.silence.len(),
            black_frames_count: cache.black_frames.len(),
            chapter_count: chapters.len(),
            ..Default::default()
        };

        let mut candidates: Vec<Candidate> = Vec::new();

        // Intro candidates — every detection method that fires produces its
        // own candidate. Chroma no longer pre-empts vhash. Overlapping
        // same-region candidates are unioned below by merge_overlapping_same_type;
        // non-overlapping ones compete in fusion with hard-reject culling
        // dead-on-arrival regions.
        let mut pushed_intro = 0;
        if let Some(intro) = per_ep_intro.get(&ep_idx) {
            diag.intro_chromaprint = Some(MatchDiag {
                start_seconds: intro.start_seconds,
                end_seconds: intro.end_seconds,
                score: intro.avg_score,
                mean_similarity: None,
                pair_count: intro.pairwise.len(),
                pairs: intro
                    .pairwise
                    .iter()
                    .map(|p| PairDiag {
                        vs_ep: p.vs_ep,
                        density: p.density,
                        mean_similarity: None,
                        lhs_start: p.lhs_start,
                        lhs_end: p.lhs_end,
                    })
                    .collect(),
            });
            let (start, end) = apply_chapter_override(chapters, "intro", intro);
            let vhash_extended = extend_with_vhash(start, end, vhash_intro_map.get(&ep_idx));
            // Skip boundary refinement for an axis when chroma and vhash
            // already agree there within SIGNALS_AGREE_TOLERANCE_SECS. Two
            // independent cross-episode signals converging on a point is
            // stronger evidence than a nearby black+silence.
            let (skip_start, skip_end) = match vhash_intro_map.get(&ep_idx) {
                Some(vm) => (
                    (intro.start_seconds - vm.start_seconds).abs() < SIGNALS_AGREE_TOLERANCE_SECS,
                    (intro.end_seconds - vm.end_seconds).abs() < SIGNALS_AGREE_TOLERANCE_SECS,
                ),
                None => (false, false),
            };
            let refined = refine_candidate(
                SegmentType::Intro,
                vhash_extended.0,
                vhash_extended.1,
                intro.avg_score,
                cache,
                vhash_intro_map.get(&ep_idx).map(|d| d.density),
                skip_start,
                skip_end,
            );
            candidates.push(refined);
            pushed_intro += 1;
        }
        if let Some(vhash_match) = vhash_intro_map.get(&ep_idx) {
            let vhash_dur = vhash_match.end_seconds - vhash_match.start_seconds;
            let black_coverage = fraction_inside_black_interval(
                vhash_match.start_seconds,
                vhash_match.end_seconds,
                &cache.black_frames,
            );
            if vhash_match.pair_count < min_vhash_pairs {
                diag.notes.push(format!(
                    "intro vhash has only {} pair(s) (<{min_vhash_pairs}) — dropped",
                    vhash_match.pair_count
                ));
            } else if vhash_dur < MIN_VHASH_ONLY_INTRO_DURATION {
                // Under 10s, a vhash match alone is almost always a network
                // ident (HBO logo), transition card, or recurring scene
                // framing — not a skip-worthy intro. If chroma already
                // agreed, that candidate is already pushed and merge below
                // will carry the vhash density.
                diag.notes.push(format!(
                    "intro vhash at {:.1}..{:.1}s is only {:.1}s (< {:.0}s min) — \
                     dropped as standalone candidate",
                    vhash_match.start_seconds,
                    vhash_match.end_seconds,
                    vhash_dur,
                    MIN_VHASH_ONLY_INTRO_DURATION
                ));
            } else if black_coverage.is_some_and(|c| c >= BLACK_FRAME_COVERAGE_REJECT) {
                // Pure black screens (title-card transitions, fade-to-black act
                // breaks) are visually identical across every episode — vhash
                // "matches" at density 1.0 but it's not an intro. Reject the
                // standalone vhash candidate; any chroma-confirmed intro
                // already pushed above is unaffected.
                diag.notes.push(format!(
                    "intro vhash at {:.1}..{:.1}s is {:.0}% inside a black-frame \
                     interval — rejected as title-card transition",
                    vhash_match.start_seconds,
                    vhash_match.end_seconds,
                    black_coverage.unwrap_or(0.0) * 100.0
                ));
            } else {
                candidates.push(build_vhash_only_candidate(
                    SegmentType::Intro,
                    vhash_match.start_seconds,
                    vhash_match.end_seconds,
                    vhash_match.density,
                    vhash_match.pair_count,
                    cache,
                ));
                pushed_intro += 1;
            }
        }
        if pushed_intro == 0 {
            diag.notes
                .push("intro: neither chromaprint nor vhash produced a match".into());
        }

        if let Some(vhash_intro) = vhash_intro_map.get(&ep_idx) {
            diag.intro_vhash = Some(MatchDiag {
                start_seconds: vhash_intro.start_seconds,
                end_seconds: vhash_intro.end_seconds,
                score: vhash_intro.density,
                mean_similarity: Some(vhash_intro.mean_similarity),
                pair_count: vhash_intro.pair_count,
                pairs: vhash_intro
                    .pairwise
                    .iter()
                    .map(|p| PairDiag {
                        vs_ep: p.vs_ep,
                        density: p.density,
                        mean_similarity: Some(p.mean_similarity),
                        lhs_start: p.lhs_start,
                        lhs_end: p.lhs_end,
                    })
                    .collect(),
            });
        }

        // Credits candidates — same independent-build pattern as intro.
        // Chroma and vhash each push their own candidate if they fire.
        // merge_overlapping_same_type below unions them if they point at the
        // same region; hard-reject kills candidates that can't survive
        // position/duration gates (e.g. a 2-pair chroma match mid-episode
        // that would otherwise absorb vision's real end-credits score).
        let mut pushed_credits = 0;
        if let Some(credits) = per_ep_credits.get(&ep_idx) {
            let abs_end = cache.duration - credits.start_seconds;
            let abs_start = cache.duration - credits.end_seconds;
            diag.credits_chromaprint = Some(MatchDiag {
                start_seconds: abs_start,
                end_seconds: abs_end,
                score: credits.avg_score,
                mean_similarity: None,
                pair_count: credits.pairwise.len(),
                // Credits pairwise endpoints are in reversed-fingerprint
                // coordinates; flip to absolute so the report reads the same
                // as start/end above.
                pairs: credits
                    .pairwise
                    .iter()
                    .map(|p| PairDiag {
                        vs_ep: p.vs_ep,
                        density: p.density,
                        mean_similarity: None,
                        lhs_start: cache.duration - p.lhs_end,
                        lhs_end: cache.duration - p.lhs_start,
                    })
                    .collect(),
            });
            let (start, end) = apply_chapter_override_flipped(
                chapters,
                "credits",
                credits,
                cache.duration,
                abs_start,
                abs_end,
            );
            let (skip_start, skip_end) = match vhash_credits_map.get(&ep_idx) {
                Some(vm) => (
                    (abs_start - vm.start_seconds).abs() < SIGNALS_AGREE_TOLERANCE_SECS,
                    (abs_end - vm.end_seconds).abs() < SIGNALS_AGREE_TOLERANCE_SECS,
                ),
                None => (false, false),
            };
            let mut refined = refine_candidate(
                SegmentType::Credits,
                start.max(0.0),
                end.min(cache.duration),
                credits.avg_score,
                cache,
                vhash_credits_map.get(&ep_idx).map(|d| d.density),
                skip_start,
                skip_end,
            );
            refined.signals.visual_score = visual_consistency_for_window(
                cache.embeddings.as_ref(),
                refined.start_seconds,
                refined.end_seconds,
            );
            candidates.push(refined);
            pushed_credits += 1;
        }
        if let Some(vhash_match) = vhash_credits_map.get(&ep_idx) {
            let dur = vhash_match.end_seconds - vhash_match.start_seconds;
            let black_coverage = fraction_inside_black_interval(
                vhash_match.start_seconds,
                vhash_match.end_seconds,
                &cache.black_frames,
            );
            if vhash_match.pair_count < min_vhash_pairs || dur < MIN_CREDITS_DURATION {
                diag.notes.push(format!(
                    "credits vhash pair_count={} dur={:.1}s — below thresholds (need ≥{min_vhash_pairs} pairs)",
                    vhash_match.pair_count, dur
                ));
            } else if black_coverage.is_some_and(|c| c >= BLACK_FRAME_COVERAGE_REJECT) {
                diag.notes.push(format!(
                    "credits vhash at {:.1}..{:.1}s is {:.0}% inside a black-frame \
                     interval — rejected as title-card transition",
                    vhash_match.start_seconds,
                    vhash_match.end_seconds,
                    black_coverage.unwrap_or(0.0) * 100.0
                ));
            } else {
                let mut cand = build_vhash_only_candidate(
                    SegmentType::Credits,
                    vhash_match.start_seconds,
                    vhash_match.end_seconds,
                    vhash_match.density,
                    vhash_match.pair_count,
                    cache,
                );
                cand.signals.visual_score = visual_consistency_for_window(
                    cache.embeddings.as_ref(),
                    cand.start_seconds,
                    cand.end_seconds,
                );
                candidates.push(cand);
                pushed_credits += 1;
            }
        }
        if pushed_credits == 0 {
            diag.notes
                .push("credits: neither chromaprint nor vhash produced a match".into());
        }

        if let Some(vhash_credits) = vhash_credits_map.get(&ep_idx) {
            diag.credits_vhash = Some(MatchDiag {
                start_seconds: vhash_credits.start_seconds,
                end_seconds: vhash_credits.end_seconds,
                score: vhash_credits.density,
                mean_similarity: Some(vhash_credits.mean_similarity),
                pair_count: vhash_credits.pair_count,
                pairs: vhash_credits
                    .pairwise
                    .iter()
                    .map(|p| PairDiag {
                        vs_ep: p.vs_ep,
                        density: p.density,
                        mean_similarity: Some(p.mean_similarity),
                        lhs_start: p.lhs_start,
                        lhs_end: p.lhs_end,
                    })
                    .collect(),
            });
        }

        // Hard-reject candidates that can't possibly pass fusion's position /
        // duration gates — they're out of the running across all downstream
        // logic (vision merge, post-credits, chapter agreement, final fusion).
        // Prevents the 2x01-class bug: a weak 2-pair chromaprint "credits"
        // match mid-episode (start_pct 0.45 < MIN_CREDITS_START_PCT 0.85) was
        // absorbing vision's real end-credits score before fusion could reject
        // it — losing the signal entirely.
        let mut hard_rejected = filter_hard_rejects(&mut candidates, &mut diag);

        // Fold overlapping same-type candidates into one. Chromaprint and
        // vhash both building a Credits candidate at the same region should
        // produce a single candidate carrying BOTH signals — so fusion sees
        // "chroma+dhash agree" as the triple-signal confluence it actually is,
        // not two competing half-signalled copies.
        merge_overlapping_same_type(&mut candidates, cache.embeddings.as_ref());

        // Vision credit-text scan over the entire baked-in embed window.
        // See `credits_window_secs` for sizing — read back via `start_time_secs`.
        if vision_healthy
            && let Some(client) = vision_client.as_ref()
            && let Some(view) = cache.embeddings.as_ref()
        {
            let scan_start = view.start_time_secs.max(0.0);
            let window = view.window(scan_start, cache.duration);
            if window.len() >= 5
                && let Some((text_start, text_end, confidence)) =
                    classify_credit_text(client, &window, view.fps, scan_start).await
            {
                diag.vision_credit_text_raw = Some((text_start, text_end, confidence));
                apply_vision_credit_text(
                    &mut candidates,
                    text_start,
                    text_end,
                    confidence,
                    cache.duration,
                    cache.embeddings.as_ref(),
                );
            }
        }

        // Post-credits via cached peaks.
        if let Some(credits) = candidates
            .iter()
            .find(|c| matches!(c.segment_type, SegmentType::Credits))
        {
            let credits_start = credits.start_seconds;
            let embeddings = cache.embeddings.as_ref();
            let post = detect_post_credits_from_cache(
                &cache.audio_peaks,
                credits_start,
                cache.duration,
                embeddings,
            );
            candidates.extend(post);
        }

        // Chapter agreement score (same rules as V1).
        score_chapter_agreement(chapters, &mut candidates);

        // Second hard-reject pass — vision's `apply_vision_credit_text` may
        // have widened a candidate's window past MAX_CREDITS_DURATION_ABS or
        // similar. Catch anything that's now out-of-bounds before fusion so
        // dead candidates don't clutter scoring (fusion's internal gate
        // catches these too; this records the reject in diag with the richer
        // signal context already attached).
        hard_rejected.extend(filter_hard_rejects(&mut candidates, &mut diag));

        diag.candidates_pre_fusion = candidates.clone();
        let mut fusion = score_candidates_with_rejections(candidates);
        fusion.rejected.extend(hard_rejected);
        diag.final_segments = fusion.segments.clone();
        diag.rejected_candidates = fusion.rejected;
        if diag.final_segments.is_empty() {
            diag.notes
                .push("fusion rejected all candidates (effective_confidence < threshold)".into());
        }
        out.push((cache.file_id, fusion.segments, diag));
    }

    for (fid, dur) in skipped {
        let mut diag = EpisodeDiagnostics {
            file_id: fid,
            duration: dur,
            ..Default::default()
        };
        diag.notes.push("cache incomplete — not analyzed".into());
        out.push((fid, Vec::new(), diag));
    }

    out
}

/// Per-file cache-only detection for movies (or single-episode fallback).
/// No cross-correlation — audio/visual for credits only + post-credits.
pub async fn detect_movie_from_cache(
    pool: &PgPool,
    config_vision_url: Option<&str>,
    movie: &MovieHandle,
    storage_root: &str,
) -> (Vec<DetectedSegment>, EpisodeDiagnostics) {
    let chapters = load_chapters_for_file(pool, movie.file_id).await;
    detect_movie_from_cache_with_chapters(config_vision_url, movie, storage_root, &chapters).await
}

/// Pure variant: takes pre-loaded chapters, returns segments + diagnostics.
/// No DB access.
pub async fn detect_movie_from_cache_with_chapters(
    config_vision_url: Option<&str>,
    movie: &MovieHandle,
    storage_root: &str,
    chapters: &[(f64, Option<String>)],
) -> (Vec<DetectedSegment>, EpisodeDiagnostics) {
    // Same rule as the season path: the sidecar reads are synchronous, so they
    // belong on the blocking pool and not on a runtime worker.
    let cache = {
        let movie = *movie;
        let storage_root = storage_root.to_string();
        tokio::task::spawn_blocking(move || load_cache_for_movie(&storage_root, &movie))
            .await
            .expect("skip-detect movie cache load task panicked")
    };
    let Some(cache) = cache else {
        let mut diag = EpisodeDiagnostics {
            file_id: movie.file_id,
            duration: movie.duration_seconds,
            ..Default::default()
        };
        diag.notes.push("cache incomplete — not analyzed".into());
        return (Vec::new(), diag);
    };
    detect_single_file_with_diag(&cache, config_vision_url, chapters).await
}

async fn detect_single_file_with_diag(
    cache: &EpisodeCache,
    vision_url: Option<&str>,
    chapters: &[(f64, Option<String>)],
) -> (Vec<DetectedSegment>, EpisodeDiagnostics) {
    let mut diag = EpisodeDiagnostics {
        file_id: cache.file_id,
        duration: cache.duration,
        chromaprint_samples: cache.chromaprint.len(),
        vhash_frames: cache.vhash.len(),
        silence_count: cache.silence.len(),
        black_frames_count: cache.black_frames.len(),
        chapter_count: chapters.len(),
        ..Default::default()
    };
    let mut candidates: Vec<Candidate> = Vec::new();

    let vision_client = vision_url.filter(|u| !u.is_empty()).map(VisionClient::new);
    let vision_healthy = match vision_client.as_ref() {
        Some(c) => c.health().await,
        None => false,
    };

    // Step 1: vision-based credits-text scan (if vision is configured) over the
    // entire baked-in embed window. Producer (`media_cache::analyze`) sized
    // the window via `credits_window_secs(media_type)`; we read it back from
    // `view.start_time_secs` so the two stay in sync.
    let mut visual_credits: Option<(f64, f64, f64)> = None;
    // Where vision was allowed to look. Needed downstream to tell "the roll
    // starts here" from "the roll was already running when my window opened".
    let mut vision_scan_start: Option<f64> = None;
    if vision_healthy
        && let Some(client) = vision_client.as_ref()
        && let Some(view) = cache.embeddings.as_ref()
    {
        let scan_start = view.start_time_secs.max(0.0);
        let window = view.window(scan_start, cache.duration);
        if window.len() >= 5 {
            vision_scan_start = Some(scan_start);
            visual_credits = classify_credit_text(client, &window, view.fps, scan_start).await;
            diag.vision_credit_text_raw = visual_credits;
        }
    }

    // Step 2: black/silence boundary scan over the last 12 minutes. This finds
    // the transition *into* a credit roll and carries no credits evidence of
    // its own — see `find_credits_via_boundary_from_cache` for the shapes it
    // accepts and `pick_credits_start` for how little authority it has.
    let boundary_credits =
        find_credits_via_boundary_from_cache(&cache.silence, &cache.black_frames, cache.duration);

    let credits_start = pick_credits_start(
        boundary_credits,
        visual_credits.map(|(v, _, _)| v),
        vision_scan_start,
        &mut diag,
    );

    if let Some(start) = credits_start {
        let credit_text_conf = visual_credits.map(|(_, _, c)| c);
        let credit_text_end = visual_credits
            .map(|(_, e, _)| e)
            .filter(|&e| e < cache.duration - 30.0);
        let end = credit_text_end.unwrap_or(cache.duration);
        let refined = boundary_primitive::refine_boundary(
            start,
            &cache.black_frames,
            &cache.silence,
            BOUNDARY_CREDITS_WINDOW_SECS,
            BoundarySearchDirection::Either,
        );
        let audio_profile = classify_audio_profile_from_cache(
            &cache.audio_peaks,
            refined.refined_time,
            end - refined.refined_time,
        );
        let visual_score = visual_consistency_for_window(
            cache.embeddings.as_ref(),
            refined.refined_time,
            end.min(cache.duration),
        );
        candidates.push(Candidate {
            segment_type: SegmentType::Credits,
            start_seconds: refined.refined_time,
            end_seconds: end.min(cache.duration),
            signals: CandidateSignals {
                credit_text_score: credit_text_conf,
                visual_score,
                boundary_match: if refined.has_black_frame || refined.has_silence {
                    Some(BoundaryMatch {
                        has_black_frame: refined.has_black_frame,
                        has_silence: refined.has_silence,
                        refined_start: refined.refined_time,
                        refined_end: end,
                    })
                } else {
                    None
                },
                audio_profile,
                ..Default::default()
            },
            file_duration: Some(cache.duration),
        });
    } else {
        diag.notes.push(
            "credits: boundary scan + vision both empty (no usable signal in last 12 min)".into(),
        );
    }

    // Hard-reject before post-credits/chapter/fusion — see the season path
    // for the full rationale.
    let mut hard_rejected = filter_hard_rejects(&mut candidates, &mut diag);
    merge_overlapping_same_type(&mut candidates, cache.embeddings.as_ref());

    // Post-credits if we found a credits start.
    if let Some(credits) = candidates
        .iter()
        .find(|c| matches!(c.segment_type, SegmentType::Credits))
    {
        let credits_start = credits.start_seconds;
        let post = detect_post_credits_from_cache(
            &cache.audio_peaks,
            credits_start,
            cache.duration,
            cache.embeddings.as_ref(),
        );
        candidates.extend(post);
    }

    score_chapter_agreement(chapters, &mut candidates);
    hard_rejected.extend(filter_hard_rejects(&mut candidates, &mut diag));
    diag.candidates_pre_fusion = candidates.clone();
    let mut fusion = score_candidates_with_rejections(candidates);
    fusion.rejected.extend(hard_rejected);
    diag.final_segments = fusion.segments.clone();
    diag.rejected_candidates = fusion.rejected;
    if diag.final_segments.is_empty() {
        diag.notes
            .push("fusion rejected all candidates (effective_confidence < threshold)".into());
    }
    (fusion.segments, diag)
}

/// A credits-shaped boundary found in the last 12 minutes of a file.
#[derive(Debug, Clone, Copy, PartialEq)]
struct CreditsBoundary {
    /// Earliest qualifying boundary in the scan window.
    earliest: f64,
    /// Earliest *sustained* black run, when the window holds one.
    strong_black: Option<f64>,
}

/// Find the transition into the credit roll from cached silence + black
/// intervals within the last 12 minutes.
///
/// This used to take the earliest silence or black gap ≥0.3s anywhere in the
/// window, which is not a credits test at all: every quiet beat in a film's
/// last act qualifies, and the earliest one wins by construction, so the
/// answer trended toward "12 minutes before the end" on any film with a
/// sparsely-scored finale. On Looper it picked an 8.8s lull in the cornfield
/// showdown at 1:48:53 and swallowed the last two minutes of the film.
///
/// A boundary now has to look like a fade into credits:
///
/// * a black run of at least [`MIN_CREDITS_BLACK_SECS`], or
/// * a silence of at least [`MIN_CREDITS_SILENCE_SECS`] that coincides with a
///   black interval of any length — picture and audio cutting out together,
///   rather than a pause in the dialogue.
///
/// `None` when nothing in the window qualifies, and that is a real answer: with
/// no vision signal either, no credits segment beats one anchored to an
/// arbitrary pause.
fn find_credits_via_boundary_from_cache(
    silence: &[SilenceInterval],
    black: &[BlackInterval],
    duration: f64,
) -> Option<CreditsBoundary> {
    const SCAN_WINDOW: f64 = 12.0 * 60.0; // matches movie::CREDITS_SCAN_WINDOW
    const MIN_GAP_TO_END: f64 = 30.0;

    let scan_start = (duration - SCAN_WINDOW).max(0.0);
    let scan_end = duration - MIN_GAP_TO_END;
    if scan_end <= scan_start {
        return None;
    }

    fn keep_earliest(slot: &mut Option<f64>, t: f64) {
        if slot.is_none_or(|prev| t < prev) {
            *slot = Some(t);
        }
    }

    let in_window = |t: f64| t >= scan_start && t <= scan_end;

    let mut earliest: Option<f64> = None;
    let mut strong_black: Option<f64> = None;

    for b in black {
        if !in_window(b.start_secs) || b.duration_secs < MIN_CREDITS_BLACK_SECS {
            continue;
        }
        keep_earliest(&mut earliest, b.start_secs);
        if b.duration_secs >= STRONG_CREDITS_BLACK_SECS {
            keep_earliest(&mut strong_black, b.start_secs);
        }
    }

    for s in silence {
        if !in_window(s.start_secs) || s.duration_secs < MIN_CREDITS_SILENCE_SECS {
            continue;
        }
        let goes_dark = black.iter().any(|b| {
            s.start_secs <= b.end_secs + CREDITS_SILENCE_BLACK_TOLERANCE
                && b.start_secs <= s.end_secs + CREDITS_SILENCE_BLACK_TOLERANCE
        });
        if goes_dark {
            keep_earliest(&mut earliest, s.start_secs);
        }
    }

    earliest.map(|earliest| CreditsBoundary {
        earliest,
        strong_black,
    })
}

/// Reconcile the two credits-start signals into one.
///
/// Vision is the only signal that actually *sees* credits, so it owns the start
/// whenever it fired. The previous rule took `boundary.min(vision)`, which let
/// a black/silence gap with no credits evidence behind it drag a confident
/// vision start (Looper scored 0.95) minutes earlier.
///
/// The boundary now gets exactly two corrections:
///
/// * **Snap back.** A qualifying boundary within [`CREDITS_BOUNDARY_SNAP_SECS`]
///   before vision's first credit-text frame is the fade into the roll.
/// * **Rescue a clipped window.** When vision's region starts flush against the
///   start of its embed window, the roll began before vision could see it, so
///   its start is a floor rather than a finding. There the earliest sustained
///   black run wins — caches written before `credits_window_secs` was widened
///   to 16 minutes cover only the last 5, and on those vision reports the
///   window edge on films whose credits are twice that long.
///
/// With no vision at all the boundary originates the start unaided, which is
/// why `find_credits_via_boundary_from_cache` has to be credits-shaped.
fn pick_credits_start(
    boundary: Option<CreditsBoundary>,
    vision_start: Option<f64>,
    vision_scan_start: Option<f64>,
    diag: &mut EpisodeDiagnostics,
) -> Option<f64> {
    match (boundary, vision_start) {
        (boundary, Some(vision)) => {
            if let Some(b) = boundary {
                if b.earliest < vision && vision - b.earliest <= CREDITS_BOUNDARY_SNAP_SECS {
                    return Some(b.earliest);
                }
                let clipped = vision_scan_start
                    .is_some_and(|scan_start| vision - scan_start <= VISION_WINDOW_EDGE_SECS);
                if let Some(black) = b.strong_black
                    && clipped
                    && black < vision
                {
                    diag.notes.push(format!(
                        "credits: vision start {vision:.1}s sits at the edge of its embed \
                         window — using the sustained black run at {black:.1}s instead"
                    ));
                    return Some(black);
                }
            }
            Some(vision)
        }
        (Some(b), None) => Some(b.earliest),
        (None, None) => None,
    }
}

// ── Helpers ──────────────────────────────────────────────────────────────────

/// Build an [`EpisodeFile`] list for every sibling tv_episode in the same
/// season as `episode_id`. Each sibling's cache is sourced via the
/// junction-aware path: prefer `episode_cache/tv_file/{file_id}/` (any local
/// `tv_episode_files` row linked via `tv_episode_file_episodes`), fall back
/// to `episode_cache/debrid_episode/{episode_id}/` (debrid-only history).
/// Episodes whose cache is missing, malformed, or below
/// [`MIN_SUPPORTED_PIPELINE_VERSION`] are skipped silently — the
/// cross-episode detector treats fewer than 2 loaded caches as the
/// single-file fallback case.
///
/// `file_id` on each returned [`EpisodeFile`] is the resolved cache key:
/// the linked `tv_episode_files.id` for native, or the sibling episode_id
/// when only debrid_episode/ is present. Used only as a unique in-memory
/// key for per-episode result attribution.
pub async fn season_episode_files(
    pool: &PgPool,
    storage_root: &str,
    episode_id: Uuid,
) -> Result<Vec<crate::skip_detect::tv::EpisodeFile>, sqlx::Error> {
    use crate::media_cache::MIN_SUPPORTED_PIPELINE_VERSION;
    use crate::media_cache::episode_cache::{MediaKind, episode_cache_dir};

    // Sibling episodes + an aggregated list of file_ids linked via the
    // junction. Multiple files per episode (re-encodes, mixed releases) all
    // get tried in turn; the first hit with a valid manifest wins.
    //
    // The agg order is newest-first to match `tv_episode_files::
    // get_preferred_available`, so detection reads the cache of the file that
    // actually plays. Unordered, two cached re-encodes give two different
    // durations, and the winner is persisted to `cached_segments` for every
    // later viewer of the episode.
    let rows = sqlx::query!(
        r#"
        SELECT
            e.id AS "episode_id!",
            COALESCE(
                array_agg(j.episode_file_id ORDER BY f.created_at DESC, f.id DESC)
                    FILTER (WHERE j.episode_file_id IS NOT NULL),
                '{}'::uuid[]
            ) AS "file_ids!: Vec<Uuid>"
        FROM tv_episodes e
        LEFT JOIN tv_episode_file_episodes j ON j.episode_id = e.id
        LEFT JOIN tv_episode_files f ON f.id = j.episode_file_id
        WHERE e.season_id = (SELECT season_id FROM tv_episodes WHERE id = $1)
        GROUP BY e.id
        "#,
        episode_id,
    )
    .fetch_all(pool)
    .await?;

    let mut out = Vec::with_capacity(rows.len());
    for row in rows {
        let sib_id = row.episode_id;
        // Try every linked file's tv_file/ cache first.
        let mut found: Option<(Uuid, f64)> = None;
        for fid in &row.file_ids {
            let dir = episode_cache_dir(storage_root, &MediaKind::TvFile { file_id: *fid });
            if let Some(m) = readers::read_manifest_at(&dir)
                && m.pipeline_version >= MIN_SUPPORTED_PIPELINE_VERSION
                && m.input_duration_seconds > 0.0
            {
                found = Some((*fid, m.input_duration_seconds));
                break;
            }
        }
        // No local file cache hit — try the debrid-only fallback.
        if found.is_none() {
            let dir = episode_cache_dir(
                storage_root,
                &MediaKind::DebridEpisode { episode_id: sib_id },
            );
            if let Some(m) = readers::read_manifest_at(&dir)
                && m.pipeline_version >= MIN_SUPPORTED_PIPELINE_VERSION
                && m.input_duration_seconds > 0.0
            {
                // Use sib_id as the file_id sentinel — unique within the
                // season; the loader will fall through to debrid_episode/.
                found = Some((sib_id, m.input_duration_seconds));
            }
        }
        if let Some((file_id, duration)) = found {
            out.push(crate::skip_detect::tv::EpisodeFile {
                file_id,
                file_path: String::new(),
                duration_seconds: duration,
                episode_id: Some(sib_id),
            });
        }
    }
    Ok(out)
}

/// Resolve where to read an episode's persisted artefacts from. Tries
/// `tv_file/{file_id}/` first (native ingest, junction-linked file), falls
/// back to `debrid_episode/{episode_id}/` if `episode_id` is set. Returns
/// `None` if neither has a manifest at or above
/// [`MIN_SUPPORTED_PIPELINE_VERSION`].
fn resolve_episode_cache_dir(
    storage_root: &str,
    ep: &EpisodeFile,
) -> Option<(uuid::Uuid, std::path::PathBuf)> {
    use crate::media_cache::MIN_SUPPORTED_PIPELINE_VERSION;
    use crate::media_cache::episode_cache::{MediaKind, episode_cache_dir};

    let tv_file_dir = episode_cache_dir(
        storage_root,
        &MediaKind::TvFile {
            file_id: ep.file_id,
        },
    );
    if let Some(m) = readers::read_manifest_at(&tv_file_dir)
        && m.pipeline_version >= MIN_SUPPORTED_PIPELINE_VERSION
    {
        return Some((ep.file_id, tv_file_dir));
    }

    if let Some(episode_id) = ep.episode_id {
        let debrid_dir = episode_cache_dir(storage_root, &MediaKind::DebridEpisode { episode_id });
        if let Some(m) = readers::read_manifest_at(&debrid_dir)
            && m.pipeline_version >= MIN_SUPPORTED_PIPELINE_VERSION
        {
            let provenance = m.analyzed_file_id.unwrap_or(episode_id);
            return Some((provenance, debrid_dir));
        }
    }
    None
}

fn load_cache_for_episode(storage_root: &str, ep: &EpisodeFile) -> Option<EpisodeCache> {
    let (file_id, dir) = resolve_episode_cache_dir(storage_root, ep)?;
    load_cache_from_dir(&dir, file_id, ep.duration_seconds)
}

fn load_cache_for_movie(storage_root: &str, movie: &MovieHandle) -> Option<EpisodeCache> {
    use crate::media_cache::MIN_SUPPORTED_PIPELINE_VERSION;
    use crate::media_cache::episode_cache::{MediaKind, episode_cache_dir};

    if let Some(movie_id) = movie.movie_id {
        let dir = episode_cache_dir(storage_root, &MediaKind::Movie { movie_id });
        if let Some(manifest) = readers::read_manifest_at(&dir)
            && manifest.pipeline_version >= MIN_SUPPORTED_PIPELINE_VERSION
        {
            let provenance = manifest.analyzed_file_id.unwrap_or(movie.file_id);
            return load_cache_from_dir(&dir, provenance, movie.duration_seconds);
        }
    }
    None
}

fn load_cache_from_dir(
    dir: &std::path::Path,
    file_id: Uuid,
    duration: f64,
) -> Option<EpisodeCache> {
    use crate::media_cache::MIN_SUPPORTED_PIPELINE_VERSION;

    let manifest = readers::read_manifest_at(dir)?;
    if manifest.pipeline_version < MIN_SUPPORTED_PIPELINE_VERSION {
        return None;
    }
    let chromaprint = readers::read_chromaprint_at(dir)?;
    let vhash = readers::read_vhash_at(dir)?;
    let silence = readers::read_silence_at(dir)
        .map(|gaps| gaps.into_iter().map(silence_gap_to_interval).collect())
        .unwrap_or_default();
    let black_frames = readers::read_black_frames_at(dir)
        .map(|gaps| gaps.into_iter().map(black_gap_to_interval).collect())
        .unwrap_or_default();
    // audio_peaks is audiobook-only and doesn't live in episode_cache. Read it
    // from the per-file dir if present; for episode_cache hits this returns
    // empty, which the post-credits / audio-profile stages cleanly no-op on.
    let audio_peaks = std::fs::read(dir.join("audio_peaks.bin")).unwrap_or_default();
    let embeddings = readers::read_embeddings_at(dir);
    Some(EpisodeCache {
        file_id,
        duration: if duration > 0.0 {
            duration
        } else {
            manifest.input_duration_seconds
        },
        chromaprint,
        vhash,
        silence,
        black_frames,
        audio_peaks,
        embeddings,
    })
}

fn silence_gap_to_interval(g: SilenceGap) -> SilenceInterval {
    SilenceInterval {
        start_secs: g.start_secs,
        end_secs: g.end_secs,
        duration_secs: g.duration_secs,
    }
}

fn black_gap_to_interval(g: BlackGap) -> BlackInterval {
    BlackInterval {
        start_secs: g.start_secs,
        end_secs: g.end_secs,
        duration_secs: g.duration_secs,
    }
}

type EpFingerprints = Vec<(usize, Fingerprint)>;
type EpVhashSeqs = Vec<(usize, Vec<FrameDescriptor>)>;

/// Slice cached full-file chromaprints into start- and end-region fingerprints
/// with the same windows V1's `extract_season_fingerprints` uses.
fn build_fingerprint_slices(caches: &[EpisodeCache]) -> (EpFingerprints, EpFingerprints) {
    let mut start = Vec::with_capacity(caches.len());
    let mut end = Vec::with_capacity(caches.len());
    for (idx, cache) in caches.iter().enumerate() {
        let total_samples = cache.chromaprint.len() as f64;
        if total_samples == 0.0 || cache.duration <= 0.0 {
            continue;
        }
        let samples_per_sec = total_samples / cache.duration;
        let (intro_window_secs, credits_window_secs) = extraction_windows(cache.duration);

        let intro_len =
            ((intro_window_secs * samples_per_sec) as usize).min(cache.chromaprint.len());
        let credits_len =
            ((credits_window_secs * samples_per_sec) as usize).min(cache.chromaprint.len());

        if intro_len > 0 {
            let slice = cache.chromaprint[..intro_len].to_vec();
            start.push((
                idx,
                Fingerprint {
                    data: slice,
                    duration_seconds: intro_window_secs,
                },
            ));
        }
        if credits_len > 0 {
            // End window: take the last `credits_len` samples and reverse them,
            // so pairwise matching aligns from the end of the file — same as V1.
            let start_idx = cache.chromaprint.len() - credits_len;
            let mut slice: Vec<u32> = cache.chromaprint[start_idx..].to_vec();
            slice.reverse();
            end.push((
                idx,
                Fingerprint {
                    data: slice,
                    duration_seconds: credits_window_secs,
                },
            ));
        }
    }
    (start, end)
}

fn build_vhash_slices(caches: &[EpisodeCache]) -> (EpVhashSeqs, EpVhashSeqs) {
    let mut start = Vec::with_capacity(caches.len());
    let mut end = Vec::with_capacity(caches.len());
    for (idx, cache) in caches.iter().enumerate() {
        let duration_window = extract_duration_secs(cache.duration) as f64;
        let start_cutoff = duration_window;
        let end_cutoff_start = (cache.duration - duration_window).max(0.0);
        let start_slice: Vec<FrameDescriptor> = cache
            .vhash
            .iter()
            .filter(|f| f.timestamp <= start_cutoff)
            .cloned()
            .collect();
        let end_slice: Vec<FrameDescriptor> = cache
            .vhash
            .iter()
            .filter(|f| f.timestamp >= end_cutoff_start)
            .cloned()
            .collect();
        if !start_slice.is_empty() {
            start.push((idx, start_slice));
        }
        if !end_slice.is_empty() {
            end.push((idx, end_slice));
        }
    }
    (start, end)
}

async fn load_chapters_for_file(pool: &PgPool, file_id: Uuid) -> Vec<(f64, Option<String>)> {
    video_chapters::list_by_episode_file(pool, file_id)
        .await
        .map(|chs| {
            chs.into_iter()
                .map(|c| (c.start_seconds, c.title))
                .collect()
        })
        .unwrap_or_default()
}

fn apply_chapter_override(
    chapters: &[(f64, Option<String>)],
    label: &str,
    pos: &super::tv::PerEpisodePosition,
) -> (f64, f64) {
    let intro_chapter = chapters.iter().find(|(_, title)| match title {
        Some(t) => matches_label(t, label),
        None => false,
    });
    if let Some((chapter_start, _)) = intro_chapter
        && let Some((s, e)) = chapter_override_position(pos, *chapter_start)
    {
        return (s, e);
    }
    (pos.start_seconds, pos.end_seconds)
}

fn apply_chapter_override_flipped(
    chapters: &[(f64, Option<String>)],
    label: &str,
    pos: &super::tv::PerEpisodePosition,
    duration: f64,
    abs_start: f64,
    abs_end: f64,
) -> (f64, f64) {
    let ch_start = chapters
        .iter()
        .find(|(_, title)| match title {
            Some(t) => matches_label(t, label),
            None => false,
        })
        .map(|(s, _)| *s);
    let Some(ch_start) = ch_start else {
        return (abs_start, abs_end);
    };
    let delta = (abs_start - ch_start).abs();
    if delta <= 30.0 {
        return (abs_start, abs_end);
    }
    // Search pairwise matches (in reversed coords) for alignment with chapter.
    let aligned = pos
        .pairwise
        .iter()
        .filter(|p| {
            let p_abs_start = duration - p.lhs_end;
            (p_abs_start - ch_start).abs() <= 30.0
        })
        .max_by(|a, b| {
            let dur_a = a.lhs_end - a.lhs_start;
            let dur_b = b.lhs_end - b.lhs_start;
            dur_a.partial_cmp(&dur_b).unwrap()
        });
    match aligned {
        Some(a) => (duration - a.lhs_end, duration - a.lhs_start),
        None => (abs_start, abs_end),
    }
}

/// Fraction of `[start_seconds, end_seconds]` that overlaps the single
/// black-frame interval with the largest overlap. Returns `None` when the
/// window has zero or negative duration.
///
/// Used to reject vhash-only candidates that are wholly explained by a
/// black-screen transition (title card, fade-to-black act break). Black
/// stretches are visually identical across any pair of episodes so vhash
/// scores them at 1.0 density — without a structural check we invent
/// intros/credits wherever two episodes share a sustained black screen.
fn fraction_inside_black_interval(
    start_seconds: f64,
    end_seconds: f64,
    black_intervals: &[BlackInterval],
) -> Option<f64> {
    let duration = end_seconds - start_seconds;
    if duration <= 0.0 {
        return None;
    }
    let max_overlap = black_intervals
        .iter()
        .map(|bf| {
            let lo = start_seconds.max(bf.start_secs);
            let hi = end_seconds.min(bf.end_secs);
            (hi - lo).max(0.0)
        })
        .fold(0.0_f64, f64::max);
    Some(max_overlap / duration)
}

fn extend_with_vhash(
    best_start: f64,
    best_end: f64,
    vhash: Option<&super::tv::VHashMatchWithDetail>,
) -> (f64, f64) {
    let Some(vm) = vhash else {
        return (best_start, best_end);
    };
    let vhash_wider = (vm.end_seconds - vm.start_seconds) > (best_end - best_start);
    if !vhash_wider {
        return (best_start, best_end);
    }
    let overlaps = best_start <= vm.end_seconds && vm.start_seconds <= best_end;
    if overlaps {
        (
            best_start.min(vm.start_seconds),
            best_end.max(vm.end_seconds),
        )
    } else {
        (vm.start_seconds, vm.end_seconds)
    }
}

fn matches_label(title: &str, label: &str) -> bool {
    let t = title.to_lowercase();
    let t = t.trim();
    match label {
        "intro" => ["intro", "opening", "cold open", "op"]
            .iter()
            .any(|p| t.contains(p)),
        "credits" => ["credits", "ending", "ed"].iter().any(|p| t.contains(p)),
        _ => false,
    }
}

/// Build a candidate with boundary refinement from cached silence+black intervals.
///
/// `skip_start_refine` / `skip_end_refine`: when chroma and vhash already
/// agree tightly on an axis, bypass boundary snapping for that axis — two
/// independent signals converging on a point is stronger than a nearby
/// black+silence co-occurrence, which risks crossing a genuine content edge
/// (logo-card fade, pre-intro gap).
///
/// Refinement directions are asymmetric: segment START snaps forward only
/// (don't skip content leading into the segment), segment END snaps backward
/// only (don't overshoot into content after the segment).
#[allow(clippy::too_many_arguments)]
fn refine_candidate(
    segment_type: SegmentType,
    start: f64,
    end: f64,
    chromaprint_score: f64,
    cache: &EpisodeCache,
    vhash_density: Option<f64>,
    skip_start_refine: bool,
    skip_end_refine: bool,
) -> Candidate {
    let _ = segment_type;
    let start_boundary = if skip_start_refine {
        crate::skip_detect::boundary::BoundaryResult {
            refined_time: start,
            has_black_frame: false,
            has_silence: false,
        }
    } else {
        // Allow the segment start to snap in either direction — a black/silence
        // pair just BEFORE the detected start is a real pre-intro lead-in
        // (typical at file open, or after a cold-open-to-intro cut) and should
        // be absorbed into the skip window. Segment END stays backward-only
        // because forward-snap there lands inside post-segment content (the
        // post-theme logo fade that drags "+3s" without this bound).
        boundary_primitive::refine_boundary(
            start,
            &cache.black_frames,
            &cache.silence,
            BOUNDARY_REFINE_WINDOW_SECS,
            BoundarySearchDirection::Either,
        )
    };
    let end_boundary = if skip_end_refine {
        crate::skip_detect::boundary::BoundaryResult {
            refined_time: end,
            has_black_frame: false,
            has_silence: false,
        }
    } else {
        boundary_primitive::refine_boundary(
            end,
            &cache.black_frames,
            &cache.silence,
            BOUNDARY_REFINE_WINDOW_SECS,
            BoundarySearchDirection::Backward,
        )
    };

    let refined_start = start_boundary.refined_time;
    let mut refined_end = end_boundary.refined_time;
    if refined_end <= refined_start {
        refined_end = end.max(refined_start + MIN_SKIP_DURATION);
    }

    let has_black = start_boundary.has_black_frame || end_boundary.has_black_frame;
    let has_silence = start_boundary.has_silence || end_boundary.has_silence;

    let audio_profile = classify_audio_profile_from_cache(
        &cache.audio_peaks,
        refined_start,
        refined_end - refined_start,
    );

    Candidate {
        segment_type,
        start_seconds: refined_start,
        end_seconds: refined_end,
        signals: CandidateSignals {
            chromaprint_score: Some(chromaprint_score),
            dhash_score: vhash_density,
            boundary_match: if has_black || has_silence {
                Some(BoundaryMatch {
                    has_black_frame: has_black,
                    has_silence,
                    refined_start,
                    refined_end,
                })
            } else {
                None
            },
            audio_profile,
            ..Default::default()
        },
        file_duration: Some(cache.duration),
    }
}

/// Classify audio over `[start_secs, start_secs + duration_secs]` using the
/// cached audio_peaks buffer. Mirrors V1's `classify_audio_profile` ZCR + RMS-CV
/// math but reads from the cached peaks (1000pps) instead of re-extracting PCM.
fn classify_audio_profile_from_cache(
    peaks: &[u8],
    start_secs: f64,
    duration_secs: f64,
) -> Option<super::AudioProfile> {
    if duration_secs <= 0.0 {
        return None;
    }
    let (pps, pairs) = super::post_credits::parse_peaks_header(peaks)?;
    if pps == 0 {
        return None;
    }
    let pair_count = pairs.len() / 2;
    if pair_count == 0 {
        return None;
    }

    let pps_f = pps as f64;
    let start_pair = ((start_secs * pps_f).floor() as usize).min(pair_count);
    let end_pair = (((start_secs + duration_secs) * pps_f).ceil() as usize).min(pair_count);
    if end_pair.saturating_sub(start_pair) < (pps_f as usize).max(1000) {
        // V1 requires ≥ 1s of audio (8000 samples at 8kHz). Mirror that minimum.
        return Some(super::AudioProfile::Unknown);
    }

    let window_pairs = ((pps_f * 0.5).round() as usize).max(1);
    let mut zcr_values: Vec<f64> = Vec::new();
    let mut rms_values: Vec<f64> = Vec::new();
    let mut t = start_pair;
    while t + window_pairs / 2 <= end_pair {
        let hi = (t + window_pairs).min(end_pair);
        let mut crossings = 0u32;
        let mut sum_sq = 0.0_f64;
        let mut prev_sign: Option<bool> = None;
        for p in t..hi {
            // Each pair is (min_i8, max_i8); single_pass writes both equal, kobbler
            // peaks bucket-mins/maxes can differ. Use max amplitude for energy and
            // its sign for crossings — works for both formats.
            let max = pairs[p * 2 + 1] as i8 as f64;
            let amp = max.abs().max(pairs[p * 2] as i8 as f64);
            sum_sq += amp * amp;
            let sign = max >= 0.0;
            if let Some(prev) = prev_sign
                && prev != sign
            {
                crossings += 1;
            }
            prev_sign = Some(sign);
        }
        let n = (hi - t) as f64;
        zcr_values.push(crossings as f64 / n);
        rms_values.push((sum_sq / n).sqrt());
        t += window_pairs;
    }
    if zcr_values.is_empty() {
        return Some(super::AudioProfile::Unknown);
    }

    let mean_zcr: f64 = zcr_values.iter().sum::<f64>() / zcr_values.len() as f64;
    let zcr_variance: f64 = zcr_values
        .iter()
        .map(|z| (z - mean_zcr).powi(2))
        .sum::<f64>()
        / zcr_values.len() as f64;
    let mean_rms: f64 = rms_values.iter().sum::<f64>() / rms_values.len() as f64;
    let rms_variance: f64 = rms_values
        .iter()
        .map(|r| (r - mean_rms).powi(2))
        .sum::<f64>()
        / rms_values.len() as f64;
    let rms_cv = if mean_rms > 0.0 {
        rms_variance.sqrt() / mean_rms
    } else {
        0.0
    };

    // Same regimes as V1's classify_audio_profile. ZCR is a per-sample-rate
    // crossing rate so absolute values are comparable across PCM (8kHz) and
    // peaks (1kHz); RMS-CV is scale-invariant.
    let profile = if zcr_variance < 0.001 && rms_cv < 0.5 {
        super::AudioProfile::MusicOnly
    } else if zcr_variance > 0.002 && rms_cv > 0.8 {
        super::AudioProfile::Dialogue
    } else if zcr_variance > 0.001 && rms_cv < 0.5 {
        super::AudioProfile::NarrationLoop
    } else {
        super::AudioProfile::Mixed
    };
    Some(profile)
}

fn build_vhash_only_candidate(
    segment_type: SegmentType,
    start: f64,
    end: f64,
    density: f64,
    pair_count: usize,
    cache: &EpisodeCache,
) -> Candidate {
    // Cap density by pair-count confidence curve (same as V1's `vhash_confidence_cap`).
    let capped = density.min(match pair_count {
        0 => 0.0,
        1 => 0.40,
        2 => 0.60,
        3 => 0.65,
        4 => 0.70,
        5 => 0.73,
        _ => 0.75,
    });
    Candidate {
        segment_type,
        start_seconds: start,
        end_seconds: end,
        signals: CandidateSignals {
            dhash_score: Some(capped),
            ..Default::default()
        },
        file_duration: Some(cache.duration),
    }
}

/// Call the vision service's `classify_from_embeddings` + score with `score_credit_text`.
async fn classify_credit_text(
    client: &VisionClient,
    window: &[Vec<f32>],
    fps: f64,
    window_start_time: f64,
) -> Option<(f64, f64, f64)> {
    let prompts: Vec<&str> = CREDIT_PROMPTS
        .iter()
        .chain(NON_CREDIT_PROMPTS.iter())
        .copied()
        .collect();
    let classify = match client
        .classify_from_embeddings(window, &prompts, false)
        .await
    {
        Ok(c) => c,
        Err(e) => {
            warn!(error = %e, "classify_from_embeddings failed");
            return None;
        }
    };
    let frame_times: Vec<f64> = (0..window.len())
        .map(|i| window_start_time + i as f64 / fps)
        .collect();
    score_credit_text(&classify.scores, &frame_times)
}

fn apply_vision_credit_text(
    candidates: &mut Vec<Candidate>,
    text_start: f64,
    text_end: f64,
    confidence: f64,
    duration: f64,
    embeddings: Option<&EmbeddingsView>,
) {
    // Only merge into an existing Credits candidate if its time window
    // OVERLAPS vision's detection. A weak chromaprint "credits" false-positive
    // mid-episode (e.g. Leftovers 2x01 at 1656s vs vision's 3649s) must not
    // absorb vision's score — that conflates two unrelated regions and
    // silently hides the real credits. When vision disagrees spatially, push
    // a separate Credits candidate so fusion can score both on their own
    // merits.
    let overlap = candidates.iter_mut().find(|c| {
        matches!(c.segment_type, SegmentType::Credits)
            && windows_touch(c.start_seconds, c.end_seconds, text_start, text_end)
    });
    match overlap {
        Some(existing) => {
            if text_start < existing.start_seconds {
                existing.start_seconds = text_start;
            }
            let prev = existing.signals.credit_text_score.unwrap_or(0.0);
            existing.signals.credit_text_score = Some(prev.max(confidence));
            if text_end < duration - 30.0 {
                existing.end_seconds = text_end;
            }
            // Recompute visual consistency — the window bounds may have moved.
            existing.signals.visual_score = visual_consistency_for_window(
                embeddings,
                existing.start_seconds,
                existing.end_seconds,
            );
        }
        None => {
            let credits_end = if text_end < duration - 30.0 {
                text_end
            } else {
                duration
            };
            let visual_score = visual_consistency_for_window(embeddings, text_start, credits_end);
            candidates.push(Candidate {
                segment_type: SegmentType::Credits,
                start_seconds: text_start,
                end_seconds: credits_end,
                signals: CandidateSignals {
                    credit_text_score: Some(confidence),
                    visual_score,
                    ..Default::default()
                },
                file_duration: Some(duration),
            });
            if credits_end < duration - 30.0 {
                candidates.push(Candidate {
                    segment_type: SegmentType::PostCreditsScene,
                    start_seconds: text_end,
                    end_seconds: duration,
                    signals: CandidateSignals::default(),
                    file_duration: Some(duration),
                });
            }
        }
    }
}

/// Two time windows "touch" when they share at least one second of overlap.
/// Used to decide whether two independent credit-detection signals describe
/// the same region (and can be merged) or different regions (and must compete
/// as separate fusion candidates).
fn windows_touch(a_start: f64, a_end: f64, b_start: f64, b_end: f64) -> bool {
    a_start.max(b_start) < a_end.min(b_end)
}

/// Average pairwise cosine similarity of the embeddings that fall within
/// `[start, end)`. High = visually uniform (title cards, credit-roll text
/// against a plain background). Returns `None` when fewer than 4 frames
/// cover the window — below that the mean is too noisy to rank with.
///
/// Re-wires the V1 signal that commit `a7214e14` (V1 retirement) dropped:
/// embeddings are generated and cached by `media_analyze`, so this is
/// zero additional network cost — just a local dot-product sweep.
fn visual_consistency_for_window(
    embeddings: Option<&EmbeddingsView>,
    start: f64,
    end: f64,
) -> Option<f64> {
    let view = embeddings?;
    let window = view.window(start, end);
    if window.len() < 4 {
        return None;
    }
    Some(cluster_consistency(&window))
}

/// Filter out candidates that cannot possibly survive fusion's position /
/// duration gates (e.g. credits whose start_pct is too low, or intros outside
/// allowed duration). Running this BEFORE vision / post-credits / chapter
/// passes prevents a dead-on-arrival candidate from absorbing signals that
/// belong to the real region elsewhere in the file.
///
/// Dropped candidates are recorded on `diag.notes` (human-readable) and
/// returned as `RejectedCandidate` entries for the caller to append to
/// fusion's own rejected list — preserving the full audit trail that lands
/// in `skip_detect_candidates`.
fn filter_hard_rejects(
    candidates: &mut Vec<Candidate>,
    diag: &mut EpisodeDiagnostics,
) -> Vec<RejectedCandidate> {
    let mut rejected: Vec<RejectedCandidate> = Vec::new();
    candidates.retain(|c| match pre_fusion_hard_reject(c) {
        Some(reason) => {
            diag.notes.push(format!(
                "pre-fusion reject: {} {:.1}..{:.1}s — {reason}",
                c.segment_type.as_str(),
                c.start_seconds,
                c.end_seconds,
            ));
            rejected.push(RejectedCandidate {
                candidate: c.clone(),
                reason: format!("pre-fusion hard reject: {reason}"),
                scored: None,
            });
            false
        }
        None => true,
    });
    rejected
}

/// Fold overlapping same-SegmentType candidates into a single candidate that
/// carries the union of their signals and the widest covering window. Lets
/// fusion see the "three signals agree on this region" confluence directly
/// instead of scoring two half-signalled copies of the same region.
///
/// Overlap uses [`windows_touch`] (strict intersection). Adjacent-but-not-
/// intersecting candidates stay as separate inputs to fusion — they describe
/// different regions and should compete on their own merits.
fn merge_overlapping_same_type(
    candidates: &mut Vec<Candidate>,
    embeddings: Option<&EmbeddingsView>,
) {
    let mut i = 0;
    while i < candidates.len() {
        let mut j = i + 1;
        while j < candidates.len() {
            let same = candidates[i].segment_type == candidates[j].segment_type;
            let touches = windows_touch(
                candidates[i].start_seconds,
                candidates[i].end_seconds,
                candidates[j].start_seconds,
                candidates[j].end_seconds,
            );
            if same && touches {
                let other = candidates.remove(j);
                merge_into(&mut candidates[i], other, embeddings);
            } else {
                j += 1;
            }
        }
        i += 1;
    }
}

fn merge_into(dst: &mut Candidate, src: Candidate, embeddings: Option<&EmbeddingsView>) {
    dst.start_seconds = dst.start_seconds.min(src.start_seconds);
    dst.end_seconds = dst.end_seconds.max(src.end_seconds);
    dst.file_duration = dst.file_duration.or(src.file_duration);
    dst.signals.chromaprint_score =
        max_option(dst.signals.chromaprint_score, src.signals.chromaprint_score);
    dst.signals.dhash_score = max_option(dst.signals.dhash_score, src.signals.dhash_score);
    dst.signals.credit_text_score =
        max_option(dst.signals.credit_text_score, src.signals.credit_text_score);
    dst.signals.chapter_agreement =
        max_option(dst.signals.chapter_agreement, src.signals.chapter_agreement);
    if dst.signals.boundary_match.is_none() {
        dst.signals.boundary_match = src.signals.boundary_match;
    }
    if dst.signals.audio_profile.is_none() {
        dst.signals.audio_profile = src.signals.audio_profile;
    }
    if dst.signals.match_detail.is_none() {
        dst.signals.match_detail = src.signals.match_detail;
    }
    // Recompute visual consistency over the unioned window — the per-source
    // scores were calculated on narrower slices and don't represent the merge.
    dst.signals.visual_score =
        visual_consistency_for_window(embeddings, dst.start_seconds, dst.end_seconds);
}

fn max_option(a: Option<f64>, b: Option<f64>) -> Option<f64> {
    match (a, b) {
        (Some(x), Some(y)) => Some(x.max(y)),
        (Some(x), None) | (None, Some(x)) => Some(x),
        (None, None) => None,
    }
}

fn score_chapter_agreement(chapters: &[(f64, Option<String>)], candidates: &mut [Candidate]) {
    if chapters.is_empty() {
        return;
    }
    // Build chapter segments (simple: pair consecutive chapters).
    let mut chapter_segs: HashMap<String, (f64, f64)> = HashMap::new();
    let mut sorted = chapters.to_vec();
    sorted.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap());
    for (i, (start, title)) in sorted.iter().enumerate() {
        let Some(title) = title else { continue };
        let label = if matches_label(title, "intro") {
            Some("intro".to_string())
        } else if matches_label(title, "credits") {
            Some("credits".to_string())
        } else {
            None
        };
        let Some(label) = label else { continue };
        let end = sorted.get(i + 1).map(|next| next.0).unwrap_or(start + 90.0);
        chapter_segs.insert(label, (*start, end));
    }

    for candidate in candidates.iter_mut() {
        let seg_type = candidate.segment_type.as_str();
        let Some((ch_start, ch_end)) = chapter_segs.get(seg_type) else {
            continue;
        };
        let start_delta = (candidate.start_seconds - ch_start).abs();
        let ch_duration = ch_end - ch_start;
        let det_duration = candidate.end_seconds - candidate.start_seconds;
        let agreement = if start_delta <= 10.0 {
            if ch_duration >= det_duration * 0.5 {
                0.10
            } else {
                0.05
            }
        } else if start_delta <= 30.0 {
            0.0
        } else {
            -0.10
        };
        candidate.signals.chapter_agreement = Some(agreement);
    }
}

#[allow(dead_code)]
const _INTRO_MAX_BER_CHECK: f64 = INTRO_MAX_BER;

#[cfg(test)]
mod credits_boundary_tests {
    //! Regression cover for the Looper credits leak. The numbers here are the
    //! real cached artefacts for `Looper (2012)` — 7133.054s runtime, credits
    //! rolling from 1:51:01 — which the old rules put at 1:48:58.

    use super::*;

    fn black(start: f64, duration: f64) -> BlackInterval {
        BlackInterval {
            start_secs: start,
            end_secs: start + duration,
            duration_secs: duration,
        }
    }

    fn silence(start: f64, duration: f64) -> SilenceInterval {
        SilenceInterval {
            start_secs: start,
            end_secs: start + duration,
            duration_secs: duration,
        }
    }

    const LOOPER_DURATION: f64 = 7133.054;

    /// Looper's silence intervals inside the 12-minute scan window, plus the
    /// 119.9s black run that is the credit roll itself.
    fn looper_cache() -> (Vec<SilenceInterval>, Vec<BlackInterval>) {
        let silence = vec![
            silence(6533.294, 8.800),
            silence(6727.398, 0.702),
            silence(6842.003, 1.293),
            silence(7004.309, 6.695),
            silence(7046.395, 9.498),
            silence(7062.709, 9.193),
        ];
        let black = vec![black(6660.665, 119.870)];
        (silence, black)
    }

    /// The 8.8s lull at 1:48:53 is the cornfield showdown, not the credits.
    /// It is the earliest gap in the window, which is exactly why the old
    /// "first gap ≥0.3s wins" rule picked it.
    #[test]
    fn showdown_lull_is_not_a_credits_boundary() {
        let (silence, black) = looper_cache();
        let found = find_credits_via_boundary_from_cache(&silence, &black, LOOPER_DURATION)
            .expect("the black run qualifies");
        assert_eq!(found.earliest, 6660.665);
        assert_eq!(found.strong_black, Some(6660.665));
    }

    /// A quiet stretch with the picture still up never qualifies on its own,
    /// however long it runs.
    #[test]
    fn dialogue_lull_without_black_never_qualifies() {
        let (silence, _) = looper_cache();
        assert_eq!(
            find_credits_via_boundary_from_cache(&silence, &[], LOOPER_DURATION),
            None
        );
    }

    /// Audio and picture cutting out together is a boundary even when the
    /// black itself is a short fade.
    #[test]
    fn silence_coinciding_with_black_qualifies() {
        let found = find_credits_via_boundary_from_cache(
            &[silence(6600.0, 4.0)],
            &[black(6601.0, 0.4)],
            LOOPER_DURATION,
        )
        .expect("silence + black is a boundary");
        assert_eq!(found.earliest, 6600.0);
        // A 0.4s fade is not the roll, so it cannot override vision.
        assert_eq!(found.strong_black, None);
    }

    /// The bug as shipped: vision scored the roll at 0.95 and the boundary
    /// dragged the start ~2 minutes earlier via `boundary.min(vision)`.
    #[test]
    fn unrelated_boundary_does_not_drag_vision_earlier() {
        let mut diag = EpisodeDiagnostics::default();
        let boundary = CreditsBoundary {
            earliest: 6537.694,
            strong_black: None,
        };
        let start = pick_credits_start(Some(boundary), Some(6660.7), Some(6173.0), &mut diag);
        assert_eq!(start, Some(6660.7));
    }

    /// Vision samples at 1 fps, so a boundary a few seconds earlier is the
    /// fade it missed.
    #[test]
    fn boundary_snaps_vision_back_to_the_fade() {
        let mut diag = EpisodeDiagnostics::default();
        let boundary = CreditsBoundary {
            earliest: 6660.665,
            strong_black: Some(6660.665),
        };
        let start = pick_credits_start(Some(boundary), Some(6664.0), Some(6173.0), &mut diag);
        assert_eq!(start, Some(6660.665));
    }

    /// Looper's actual cache: the embed window opens at 6833.921 and vision's
    /// first frame already scores 0.50, so its region starts at the window
    /// edge. The 119.9s black run before it is the real start.
    #[test]
    fn clipped_vision_window_defers_to_a_sustained_black_run() {
        let mut diag = EpisodeDiagnostics::default();
        let boundary = CreditsBoundary {
            earliest: 6660.665,
            strong_black: Some(6660.665),
        };
        let start = pick_credits_start(Some(boundary), Some(6833.921), Some(6833.921), &mut diag);
        assert_eq!(start, Some(6660.665));
        assert!(diag.notes.iter().any(|n| n.contains("embed")));
    }

    /// A vision start well inside its window is a finding, not a floor — an
    /// earlier black run does not get to override it.
    #[test]
    fn unclipped_vision_keeps_its_own_start() {
        let mut diag = EpisodeDiagnostics::default();
        let boundary = CreditsBoundary {
            earliest: 6300.0,
            strong_black: Some(6300.0),
        };
        let start = pick_credits_start(Some(boundary), Some(6900.0), Some(6173.0), &mut diag);
        assert_eq!(start, Some(6900.0));
    }

    /// No vision: the boundary originates the start on its own.
    #[test]
    fn boundary_alone_still_originates_a_start() {
        let mut diag = EpisodeDiagnostics::default();
        let boundary = CreditsBoundary {
            earliest: 6660.665,
            strong_black: Some(6660.665),
        };
        assert_eq!(
            pick_credits_start(Some(boundary), None, None, &mut diag),
            Some(6660.665)
        );
    }

    #[test]
    fn no_signal_at_all_yields_no_start() {
        let mut diag = EpisodeDiagnostics::default();
        assert_eq!(pick_credits_start(None, None, None, &mut diag), None);
    }
}

#[cfg(test)]
mod black_frame_filter_tests {
    use super::*;

    fn black(start: f64, end: f64) -> BlackInterval {
        BlackInterval {
            start_secs: start,
            end_secs: end,
            duration_secs: end - start,
        }
    }

    /// Regression: a vhash-only intro that lands entirely inside a detected
    /// black-frame stretch is a title-card transition, not an intro.
    /// Reproduces Leftovers 1x01 Pilot "2 years later" card (21s black at
    /// 148.69..169.88) and 3x01 (5.8s black at 303.64..309.48).
    #[test]
    fn pilot_2_years_later_card_rejected() {
        let blacks = vec![black(148.69, 169.88)];
        // Vhash-only match 2:34..2:42 sits inside the 21s black card.
        let coverage = fraction_inside_black_interval(154.0, 162.0, &blacks);
        assert!(coverage.unwrap() >= BLACK_FRAME_COVERAGE_REJECT);
    }

    #[test]
    fn leftovers_3x01_black_transition_rejected() {
        let blacks = vec![black(303.64, 309.48)];
        let coverage = fraction_inside_black_interval(303.9, 309.6, &blacks);
        assert!(coverage.unwrap() >= BLACK_FRAME_COVERAGE_REJECT);
    }

    /// A real long intro (80s+) isn't rejected even if it brushes a
    /// black-frame stretch at its edges.
    #[test]
    fn real_long_intro_not_rejected() {
        // 80s intro at 4..84s with a 2s black at both ends.
        let blacks = vec![black(3.0, 5.0), black(83.0, 85.0)];
        let coverage = fraction_inside_black_interval(4.0, 84.0, &blacks).unwrap();
        assert!(coverage < BLACK_FRAME_COVERAGE_REJECT);
    }

    #[test]
    fn zero_duration_returns_none() {
        let blacks = vec![black(0.0, 5.0)];
        assert!(fraction_inside_black_interval(3.0, 3.0, &blacks).is_none());
        assert!(fraction_inside_black_interval(5.0, 4.0, &blacks).is_none());
    }
}

#[cfg(test)]
mod leftovers_tests {
    //! End-to-end detection test against two real TV episodes. Validates that
    //! the V2 cross-episode math (via the same primitives V1 uses) finds the
    //! correct signals on a known-good input pair.
    //!
    //! The Leftovers S3E07 and S3E08 share a visually-identical animated intro
    //! but each episode's audio track is scored differently — so this is a
    //! good stress test for the audio-vs-visual split:
    //!
    //! - **intro audio (chromaprint)**: should NOT find a cross-episode match
    //!   — the soundtracks under the intro are different.
    //! - **intro visual (vhash)**: should match strongly — the animation is
    //!   byte-identical between the two episodes.
    //! - **credits**: less clear — music cue may repeat, but the visual
    //!   design of credits can vary.
    //!
    //! Run with:
    //! ```sh
    //! LEFTOVERS_MKV_A="/path/to/3x07.mkv" \
    //! LEFTOVERS_MKV_B="/path/to/3x08.mkv" \
    //!   cargo test -p api --lib leftovers -- --ignored --nocapture
    //! ```
    //! Test skips gracefully if either env var is unset.
    use super::*;
    use std::path::Path;

    fn load_fixtures() -> Option<(std::path::PathBuf, std::path::PathBuf)> {
        let a = std::env::var("LEFTOVERS_MKV_A").ok()?;
        let b = std::env::var("LEFTOVERS_MKV_B").ok()?;
        let pa = std::path::PathBuf::from(&a);
        let pb = std::path::PathBuf::from(&b);
        if !pa.exists() || !pb.exists() {
            eprintln!("LEFTOVERS_MKV_A/B set but one or both files missing — skipping");
            return None;
        }
        Some((pa, pb))
    }

    fn run_single_pass_for_cross_ep(
        path: &Path,
    ) -> (Vec<u32>, Vec<kob_ffmpeg::single_pass::VhashDescriptor>, f64) {
        use kob_ffmpeg::single_pass::{
            ChromaprintParams, SinglePassRequest, VhashParams, run_single_pass,
        };
        let mut req = SinglePassRequest::new(path);
        req.chromaprint = Some(ChromaprintParams::default());
        req.vhash = Some(VhashParams {
            intermediate_width: 64,
            fps: 3.0,
            windows: Vec::new(),
        });
        let result = run_single_pass(req, None).expect("single_pass");
        let cp = result.chromaprint.expect("chromaprint branch").raw;
        let vh = result.vhash.expect("vhash branch");
        (cp, vh, result.input_duration_secs)
    }

    #[test]
    #[ignore = "requires LEFTOVERS_MKV_A and LEFTOVERS_MKV_B env vars"]
    fn leftovers_cross_episode_intro_is_vhash_match_only() {
        let Some((path_a, path_b)) = load_fixtures() else {
            return;
        };

        eprintln!("extracting single_pass for A…");
        let (cp_a, vh_a, dur_a) = run_single_pass_for_cross_ep(&path_a);
        eprintln!(
            "  duration={dur_a:.1}s  chromaprint={}u32  vhash={}",
            cp_a.len(),
            vh_a.len()
        );
        eprintln!("extracting single_pass for B…");
        let (cp_b, vh_b, dur_b) = run_single_pass_for_cross_ep(&path_b);
        eprintln!(
            "  duration={dur_b:.1}s  chromaprint={}u32  vhash={}",
            cp_b.len(),
            vh_b.len()
        );

        // Build the same start/end fingerprint slices V2 would, using the
        // extraction_windows helper.
        let (intro_window_a, _) = extraction_windows(dur_a);
        let (intro_window_b, _) = extraction_windows(dur_b);
        let samples_per_sec_a = cp_a.len() as f64 / dur_a;
        let samples_per_sec_b = cp_b.len() as f64 / dur_b;
        let intro_len_a = ((intro_window_a * samples_per_sec_a) as usize).min(cp_a.len());
        let intro_len_b = ((intro_window_b * samples_per_sec_b) as usize).min(cp_b.len());

        let start_fps: Vec<(usize, Fingerprint)> = vec![
            (
                0,
                Fingerprint {
                    data: cp_a[..intro_len_a].to_vec(),
                    duration_seconds: intro_window_a,
                },
            ),
            (
                1,
                Fingerprint {
                    data: cp_b[..intro_len_b].to_vec(),
                    duration_seconds: intro_window_b,
                },
            ),
        ];

        let per_ep_intro = find_per_episode_positions(&start_fps, MIN_MATCH_DURATION);
        eprintln!(
            "intro audio matches: {} (expected 0 — soundtracks differ)",
            per_ep_intro.len()
        );
        for (k, v) in &per_ep_intro {
            eprintln!(
                "  ep{k} audio intro: {:.1}..{:.1}s  avg_score={:.3}",
                v.start_seconds, v.end_seconds, v.avg_score
            );
        }

        // Build vhash sequences restricted to the intro window.
        let intro_vhash: Vec<(usize, Vec<FrameDescriptor>)> = vec![
            (
                0,
                vh_a.iter()
                    .filter(|f| f.timestamp_secs <= intro_window_a)
                    .map(|f| FrameDescriptor {
                        timestamp: f.timestamp_secs,
                        values: f.data.clone(),
                        quantized: super::super::primitives::vhash_align::quantize_descriptor(
                            &f.data,
                        ),
                    })
                    .collect(),
            ),
            (
                1,
                vh_b.iter()
                    .filter(|f| f.timestamp_secs <= intro_window_b)
                    .map(|f| FrameDescriptor {
                        timestamp: f.timestamp_secs,
                        values: f.data.clone(),
                        quantized: super::super::primitives::vhash_align::quantize_descriptor(
                            &f.data,
                        ),
                    })
                    .collect(),
            ),
        ];
        let vhash_intro_map = find_per_episode_vhash(&intro_vhash, MIN_MATCH_DURATION);
        eprintln!(
            "intro vhash matches: {} (expected 2, both episodes)",
            vhash_intro_map.len()
        );
        for (k, v) in &vhash_intro_map {
            eprintln!(
                "  ep{k} visual intro: {:.1}..{:.1}s  density={:.3}  pair_count={}",
                v.start_seconds, v.end_seconds, v.density, v.pair_count
            );
        }

        // Primary assertion: vHash finds the intro for BOTH episodes. This is
        // what unlocks the vhash-only candidate path in `detect_season_from_cache`.
        assert_eq!(
            vhash_intro_map.len(),
            2,
            "vhash should match the animated intro on BOTH episodes (got {})",
            vhash_intro_map.len()
        );
        for (ep, detail) in &vhash_intro_map {
            assert!(
                detail.density >= 0.70,
                "ep{ep} intro vhash density={} — animated intros should match hard",
                detail.density
            );
            let duration = detail.end_seconds - detail.start_seconds;
            assert!(
                duration >= MIN_MATCH_DURATION,
                "ep{ep} intro vhash duration={duration}s is below MIN_MATCH_DURATION"
            );
        }

        // Secondary observation (not a hard failure): audio intro shouldn't
        // match. If it does, that's surprising but not a correctness bug —
        // log it so the human can inspect.
        if !per_ep_intro.is_empty() {
            eprintln!(
                "WARNING: intro audio matched despite different soundtracks — \
                 inspect chromaprint slice for this pair"
            );
        }
    }
}

#[cfg(test)]
mod resolver_tests {
    use super::*;
    use crate::media_cache::PIPELINE_VERSION;
    use crate::media_cache::episode_cache::{EpisodeArtefacts, MediaKind, write_episode_cache};
    use crate::media_cache::writers::{ArtefactMap, Manifest};
    use crate::skip_detect::tv::EpisodeFile;
    use crate::skip_detect::vhash::serialize_descriptors;
    use chrono::Utc;
    use tempfile::TempDir;

    fn manifest(version: i32, file_id: Uuid) -> Manifest {
        Manifest {
            pipeline_version: version,
            generated_at: Utc::now(),
            input_duration_seconds: 100.0,
            trickplay_dir: "/dev/null".into(),
            artefacts: ArtefactMap::default(),
            analyzed_file_id: Some(file_id),
            analyzed_at: Some(Utc::now()),
            input_size_bytes: None,
            input_mtime: None,
        }
    }

    /// Write a minimal artefact set into the supplied namespace and return
    /// the chromaprint bytes for assertion. `marker` distinguishes one
    /// namespace's bytes from another in the same test.
    fn populate(storage: &str, kind: &MediaKind, marker: u8) -> Vec<u8> {
        let cp: Vec<u8> = (0..16)
            .flat_map(|_| (marker as u32).to_le_bytes().to_vec())
            .collect();
        let descs = vec![
            crate::skip_detect::primitives::vhash_align::FrameDescriptor {
                timestamp: 0.0,
                values: vec![marker; 32],
                quantized: marker as u32,
            },
        ];
        let vhash = serialize_descriptors(&descs);
        write_episode_cache(
            storage,
            kind,
            &manifest(PIPELINE_VERSION, kind.id()),
            &EpisodeArtefacts {
                vhash: Some(&vhash),
                chromaprint: Some(&cp),
                silence: None,
                loudness: None,
                black_frames: None,
                embeddings: None,
            },
        )
        .unwrap();
        cp
    }

    #[test]
    fn tv_file_wins_over_debrid_episode_when_both_exist() {
        let tmp = TempDir::new().unwrap();
        let storage = tmp.path().to_string_lossy().into_owned();
        let file_id = Uuid::now_v7();
        let episode_id = Uuid::now_v7();

        // Both namespaces populated. tv_file/ should win.
        populate(&storage, &MediaKind::TvFile { file_id }, 0xAA);
        populate(&storage, &MediaKind::DebridEpisode { episode_id }, 0xBB);

        let ep = EpisodeFile {
            file_id,
            file_path: String::new(),
            duration_seconds: 100.0,
            episode_id: Some(episode_id),
        };
        let cache = load_cache_for_episode(&storage, &ep).expect("cache loads");
        assert_eq!(
            cache.chromaprint.first(),
            Some(&0xAAu32),
            "tv_file/ takes priority over debrid_episode/"
        );
    }

    #[test]
    fn debrid_episode_used_when_tv_file_missing() {
        let tmp = TempDir::new().unwrap();
        let storage = tmp.path().to_string_lossy().into_owned();
        let file_id = Uuid::now_v7();
        let episode_id = Uuid::now_v7();

        // Only debrid_episode/ populated.
        populate(&storage, &MediaKind::DebridEpisode { episode_id }, 0xBB);

        let ep = EpisodeFile {
            file_id,
            file_path: String::new(),
            duration_seconds: 100.0,
            episode_id: Some(episode_id),
        };
        let cache = load_cache_for_episode(&storage, &ep).expect("cache loads");
        assert_eq!(cache.chromaprint.first(), Some(&0xBBu32));
    }

    #[test]
    fn returns_none_when_neither_namespace_populated() {
        let tmp = TempDir::new().unwrap();
        let storage = tmp.path().to_string_lossy().into_owned();
        let ep = EpisodeFile {
            file_id: Uuid::now_v7(),
            file_path: String::new(),
            duration_seconds: 100.0,
            episode_id: Some(Uuid::now_v7()),
        };
        assert!(load_cache_for_episode(&storage, &ep).is_none());
    }

    #[test]
    fn movie_loads_from_movie_namespace() {
        let tmp = TempDir::new().unwrap();
        let storage = tmp.path().to_string_lossy().into_owned();
        let movie_id = Uuid::now_v7();
        populate(&storage, &MediaKind::Movie { movie_id }, 0xCC);

        let movie = MovieHandle {
            file_id: Uuid::now_v7(),
            duration_seconds: 100.0,
            movie_id: Some(movie_id),
        };
        let cache = load_cache_for_movie(&storage, &movie).expect("cache loads");
        assert_eq!(cache.chromaprint.first(), Some(&0xCCu32));
    }
}
