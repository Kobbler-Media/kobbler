//! Pure boundary refinement: pick the best black-frame / silence pairing near
//! a target timestamp.
//!
//! The async [`super::super::boundary::refine_boundary`] runs the FFmpeg
//! detection and then delegates to [`refine_boundary`] here. Re-exposed as a
//! pure function so it can be tested without invoking FFmpeg.

use kob_ffmpeg::filter::detect::{BlackInterval, SilenceInterval};

use crate::skip_detect::boundary::BoundaryResult;

/// Which direction from `target_time` a boundary feature is allowed to sit.
///
/// `Either` — any feature within `max_distance_secs` (used by V1's async
/// wrapper which already pre-filters to a small window around the target).
///
/// `Backward` — only features at or before `target_time`. Use for intro/credits
/// END: snapping forward typically lands inside a post-segment transition
/// (post-intro logo fade, post-credits short) which is content from the user's
/// POV.
///
/// `Forward` — only features at or after `target_time`. Use for intro/credits
/// START: snapping backward typically pre-emptively skips content leading into
/// the segment (e.g. final seconds of the cold open).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BoundarySearchDirection {
    Either,
    Backward,
    Forward,
}

/// Pick the best boundary near `target_time` from pre-computed black-frame and
/// silence intervals.
///
/// `max_distance_secs` caps how far from `target_time` a feature can be and
/// still be used. Features outside the cap are ignored; the function falls
/// back to `target_time` unchanged if nothing qualifies. This is the critical
/// bound when the caller feeds whole-file intervals (cache path): without
/// it, every refinement collapses to whichever co-occurrence happens to sit
/// closest in file order, regardless of whether it's anywhere near the target.
/// V1's async wrapper pre-filters inputs to ±window via ffmpeg seek, so passing
/// the same window here yields identical behaviour.
///
/// `direction` constrains which side of `target_time` a feature can sit
/// ([`BoundarySearchDirection::Either`] matches historical behaviour).
///
/// Preference order:
/// 1. The co-occurring black frame + silence pair (within 0.5s of each other)
///    whose midpoint is nearest to `target_time`, within `max_distance_secs`
///    and on the allowed side.
/// 2. The nearest silence gap within the cap + direction constraint.
/// 3. The nearest black frame within the cap + direction constraint.
/// 4. Fallback: `target_time` unchanged, with no boundary features.
pub fn refine_boundary(
    target_time: f64,
    black_intervals: &[BlackInterval],
    silence_intervals: &[SilenceInterval],
    max_distance_secs: f64,
    direction: BoundarySearchDirection,
) -> BoundaryResult {
    let allowed = |midpoint: f64| -> bool {
        match direction {
            BoundarySearchDirection::Either => true,
            BoundarySearchDirection::Backward => midpoint <= target_time + 0.01,
            BoundarySearchDirection::Forward => midpoint >= target_time - 0.01,
        }
    };
    // Find the co-occurring black-frame + silence pair whose midpoint is
    // nearest to target_time (within the cap and direction constraint).
    let mut best: Option<(f64, f64)> = None; // (midpoint, distance_to_target)
    for bf in black_intervals {
        for sg in silence_intervals {
            if bf.start_secs <= sg.end_secs + 0.5 && sg.start_secs <= bf.end_secs + 0.5 {
                let midpoint = ((bf.start_secs + bf.end_secs) / 2.0
                    + (sg.start_secs + sg.end_secs) / 2.0)
                    / 2.0;
                if !allowed(midpoint) {
                    continue;
                }
                let dist = (midpoint - target_time).abs();
                if dist > max_distance_secs {
                    continue;
                }
                if best.is_none_or(|(_, d)| dist < d) {
                    best = Some((midpoint, dist));
                }
            }
        }
    }
    if let Some((midpoint, _)) = best {
        return BoundaryResult {
            refined_time: midpoint,
            has_black_frame: true,
            has_silence: true,
        };
    }

    // Fall back to nearest silence gap within the cap + direction.
    if let Some(nearest) = silence_intervals
        .iter()
        .filter(|s| {
            let mid = (s.start_secs + s.end_secs) / 2.0;
            allowed(mid) && (mid - target_time).abs() <= max_distance_secs
        })
        .min_by(|a, b| {
            let da = ((a.start_secs + a.end_secs) / 2.0 - target_time).abs();
            let db = ((b.start_secs + b.end_secs) / 2.0 - target_time).abs();
            da.partial_cmp(&db).unwrap_or(std::cmp::Ordering::Equal)
        })
    {
        return BoundaryResult {
            refined_time: (nearest.start_secs + nearest.end_secs) / 2.0,
            has_black_frame: false,
            has_silence: true,
        };
    }

    // Fall back to nearest black frame within the cap + direction.
    if let Some(nearest) = black_intervals
        .iter()
        .filter(|b| {
            let mid = (b.start_secs + b.end_secs) / 2.0;
            allowed(mid) && (mid - target_time).abs() <= max_distance_secs
        })
        .min_by(|a, b| {
            let da = ((a.start_secs + a.end_secs) / 2.0 - target_time).abs();
            let db = ((b.start_secs + b.end_secs) / 2.0 - target_time).abs();
            da.partial_cmp(&db).unwrap_or(std::cmp::Ordering::Equal)
        })
    {
        return BoundaryResult {
            refined_time: (nearest.start_secs + nearest.end_secs) / 2.0,
            has_black_frame: true,
            has_silence: false,
        };
    }

    BoundaryResult {
        refined_time: target_time,
        has_black_frame: false,
        has_silence: false,
    }
}

/// Default cap for boundary refinement when callers don't know better — matches
/// V1's async-wrapper intro window.
pub const DEFAULT_BOUNDARY_WINDOW_SECS: f64 = 3.0;

#[cfg(test)]
mod tests {
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

    /// Regression: when the cache feeds whole-file intervals, the co-occurring
    /// pair branch MUST pick the pair nearest target_time, not the first one
    /// in file order. Reproduces a Leftovers S01 intro refinement bug where
    /// the intro at ~240s was collapsed to a ~3s boundary pair early in the
    /// episode because that was the first co-occurrence in the arrays.
    #[test]
    fn co_occurring_pair_picks_nearest_to_target() {
        // Two co-occurring pairs: one at ~3s (e.g. cold-open fade-up), one at
        // ~240s (intro boundary). Target is 240s — we want the 240s pair.
        let blacks = vec![black(2.5, 1.0), black(239.5, 1.0), black(1800.0, 0.5)];
        let silences = vec![silence(2.6, 0.9), silence(239.4, 1.2), silence(1800.1, 0.4)];
        let r = refine_boundary(
            240.0,
            &blacks,
            &silences,
            5.0,
            BoundarySearchDirection::Either,
        );
        assert!(r.has_black_frame);
        assert!(r.has_silence);
        assert!(
            (r.refined_time - 240.0).abs() < 1.0,
            "refined_time={} — expected ~240s, got early-file boundary",
            r.refined_time
        );
    }

    /// Regression: even after picking the nearest pair, far-away pairs should
    /// NOT be used — we'd rather return `target_time` unchanged than snap the
    /// refinement to a boundary 500s away.
    #[test]
    fn far_away_pair_is_rejected() {
        let blacks = vec![black(3.0, 0.5), black(800.0, 0.5)];
        let silences = vec![silence(3.0, 0.5), silence(800.0, 0.5)];
        let r = refine_boundary(
            400.0,
            &blacks,
            &silences,
            5.0,
            BoundarySearchDirection::Either,
        );
        assert_eq!(r.refined_time, 400.0);
        assert!(!r.has_black_frame);
        assert!(!r.has_silence);
    }

    #[test]
    fn co_occurring_pair_picks_nearest_for_late_target() {
        let blacks = vec![black(5.0, 0.5), black(3100.0, 0.5)];
        let silences = vec![silence(5.1, 0.4), silence(3099.9, 0.6)];
        let r = refine_boundary(
            3100.5,
            &blacks,
            &silences,
            5.0,
            BoundarySearchDirection::Either,
        );
        assert!((r.refined_time - 3100.0).abs() < 2.0);
    }

    #[test]
    fn no_pair_falls_back_to_nearest_silence() {
        let blacks = vec![black(10.0, 0.5)];
        let silences = vec![silence(100.0, 1.0), silence(500.0, 1.0)];
        let r = refine_boundary(
            500.0,
            &blacks,
            &silences,
            5.0,
            BoundarySearchDirection::Either,
        );
        assert!(!r.has_black_frame);
        assert!(r.has_silence);
        assert!((r.refined_time - 500.5).abs() < 0.1);
    }

    #[test]
    fn no_silence_falls_back_to_nearest_black() {
        let blacks = vec![black(100.0, 0.5), black(500.0, 0.5)];
        let silences: Vec<SilenceInterval> = vec![];
        let r = refine_boundary(
            500.5,
            &blacks,
            &silences,
            5.0,
            BoundarySearchDirection::Either,
        );
        assert!(r.has_black_frame);
        assert!(!r.has_silence);
        assert!((r.refined_time - 500.25).abs() < 0.1);
    }

    #[test]
    fn empty_intervals_return_target_unchanged() {
        let r = refine_boundary(42.0, &[], &[], 5.0, BoundarySearchDirection::Either);
        assert_eq!(r.refined_time, 42.0);
        assert!(!r.has_black_frame);
        assert!(!r.has_silence);
    }

    /// Regression: intro END refinement with `Backward` must not snap forward.
    /// Reproduces Leftovers S01 1x02: chroma theme ends at 330.4; a real black+
    /// silence pair exists at 333.1 (post-theme logo-card fade into dialogue).
    /// Under `Either` the primitive snaps forward to 333.1, pushing "skip intro"
    /// into the first line of dialogue. `Backward` rejects forward features.
    #[test]
    fn backward_direction_rejects_forward_pair() {
        let blacks = vec![black(330.4, 0.3), black(333.1, 0.5)];
        let silences = vec![silence(333.0, 0.6)];
        let target = 330.6; // chroma+vhash agreement point

        // Either: drags forward to 333.1 (the co-occurring pair).
        let r_either = refine_boundary(
            target,
            &blacks,
            &silences,
            3.0,
            BoundarySearchDirection::Either,
        );
        assert!(r_either.refined_time > target);

        // Backward: doesn't snap to 333.1; no pre-target pair exists (silence
        // starts at 333.0 which is after target), so falls back to the lone
        // pre-target black at 330.4.
        let r_back = refine_boundary(
            target,
            &blacks,
            &silences,
            3.0,
            BoundarySearchDirection::Backward,
        );
        assert!(
            r_back.refined_time <= target + 0.01,
            "backward search should not move forward; got {}",
            r_back.refined_time
        );
    }

    /// Regression: `Forward` must not snap backward for segment START.
    #[test]
    fn forward_direction_rejects_backward_pair() {
        let blacks = vec![black(238.0, 0.3), black(241.0, 0.5)];
        let silences = vec![silence(237.9, 0.6), silence(241.1, 0.6)];
        let target = 240.0;
        let r = refine_boundary(
            target,
            &blacks,
            &silences,
            3.0,
            BoundarySearchDirection::Forward,
        );
        assert!(
            r.refined_time >= target - 0.01,
            "forward search should not move backward; got {}",
            r.refined_time
        );
    }
}
