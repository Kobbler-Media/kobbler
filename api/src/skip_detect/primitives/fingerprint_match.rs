//! Pure inverted-index alignment for Chromaprint sub-fingerprints.
//!
//! No I/O. Operates over `&[u32]` sample sequences (with duration metadata via
//! [`Fingerprint`]) and returns matched regions. Used by `chromaprint.rs` after
//! it extracts fingerprints from media via libchromaprint.

use std::collections::{HashMap, HashSet};
use tracing::debug;

use crate::skip_detect::{MIN_MATCH_DENSITY, MIN_MATCH_DURATION};

/// Raw Chromaprint fingerprint as a sequence of 32-bit sub-fingerprints.
#[derive(Debug, Clone)]
pub struct Fingerprint {
    pub data: Vec<u32>,
    pub duration_seconds: f64,
}

#[derive(Debug, Clone)]
pub struct MatchedRegion {
    pub start_seconds: f64,
    pub end_seconds: f64,
    /// Average match quality (0.0-1.0, higher = better).
    pub score: f64,
}

/// A pairwise match containing positions from both fingerprints.
#[derive(Debug, Clone)]
pub struct PairwiseMatch {
    pub lhs: MatchedRegion,
    pub rhs: MatchedRegion,
}

/// Build an inverted index: fingerprint point -> last position it appeared at.
pub fn build_inverted_index(fingerprint: &[u32]) -> HashMap<u32, usize> {
    let mut index = HashMap::new();
    for (i, &point) in fingerprint.iter().enumerate() {
        index.insert(point, i);
    }
    index
}

/// Find alignment shifts between two fingerprints using inverted indexes.
/// An alignment shift is the offset needed to align matching points.
pub fn find_alignment_shifts(lhs: &[u32], rhs: &[u32], index_shift_tolerance: i32) -> Vec<i32> {
    let lhs_index = build_inverted_index(lhs);
    let rhs_index = build_inverted_index(rhs);
    let mut shifts = HashSet::new();

    for (&point, &lhs_pos) in &lhs_index {
        for delta in -index_shift_tolerance..=index_shift_tolerance {
            let modified = point.wrapping_add(delta as u32);
            if let Some(&rhs_pos) = rhs_index.get(&modified) {
                shifts.insert(rhs_pos as i32 - lhs_pos as i32);
            }
        }
    }

    shifts.into_iter().collect()
}

/// Find contiguous matching region at a given alignment shift.
/// Uses popcount-based similarity but only at the aligned positions.
/// Returns both lhs and rhs matched regions for per-episode position tracking.
pub fn find_contiguous_at_shift(
    lhs: &[u32],
    rhs: &[u32],
    shift: i32,
    max_bit_diff: u32,
    samples_to_seconds: f64,
    max_time_skip: f64,
) -> Option<(MatchedRegion, MatchedRegion)> {
    let (left_offset, right_offset) = if shift < 0 {
        ((-shift) as usize, 0usize)
    } else {
        (0usize, shift as usize)
    };

    let upper_limit = lhs
        .len()
        .saturating_sub(left_offset)
        .min(rhs.len().saturating_sub(right_offset));
    let mut matched_pairs: Vec<(f64, f64)> = Vec::new();

    for i in 0..upper_limit {
        let lhs_pos = i + left_offset;
        let rhs_pos = i + right_offset;
        let diff = lhs[lhs_pos] ^ rhs[rhs_pos];

        if diff.count_ones() <= max_bit_diff {
            matched_pairs.push((
                lhs_pos as f64 * samples_to_seconds,
                rhs_pos as f64 * samples_to_seconds,
            ));
        }
    }

    if matched_pairs.is_empty() {
        return None;
    }

    find_longest_contiguous_run_pair(&matched_pairs, max_time_skip, samples_to_seconds)
}

/// Find the longest contiguous run of matched timestamp pairs, allowing gaps up to max_skip.
/// Scores by density (ratio of matched samples to total in region). Returns both lhs and rhs regions.
pub fn find_longest_contiguous_run_pair(
    pairs: &[(f64, f64)],
    max_skip: f64,
    samples_to_seconds: f64,
) -> Option<(MatchedRegion, MatchedRegion)> {
    if pairs.is_empty() {
        return None;
    }

    // Collect all contiguous runs (keyed off lhs times for gap detection)
    // Track start/end indices to derive both lhs and rhs regions from the same run
    let mut runs: Vec<(usize, usize, usize)> = Vec::new(); // (start_idx, end_idx, count)
    let mut run_start_idx = 0;
    let mut prev_lhs = pairs[0].0;
    let mut run_count = 1usize;

    for (i, &(lhs_t, _)) in pairs.iter().enumerate().skip(1) {
        if lhs_t - prev_lhs <= max_skip + samples_to_seconds {
            run_count += 1;
        } else {
            runs.push((run_start_idx, i - 1, run_count));
            run_start_idx = i;
            run_count = 1;
        }
        prev_lhs = lhs_t;
    }
    runs.push((run_start_idx, pairs.len() - 1, run_count));

    // Score each run by density, pick the best
    let mut best: Option<(MatchedRegion, MatchedRegion)> = None;
    let mut best_score: f64 = 0.0;

    for (start_idx, end_idx, count) in runs {
        let lhs_start = pairs[start_idx].0;
        let lhs_end = pairs[end_idx].0;
        let rhs_start = pairs[start_idx].1;
        let rhs_end = pairs[end_idx].1;

        let duration = lhs_end - lhs_start;
        if duration < MIN_MATCH_DURATION {
            continue;
        }
        let expected_samples = (duration / samples_to_seconds) as usize + 1;
        let density = if expected_samples > 0 {
            (count as f64 / expected_samples as f64).min(1.0)
        } else {
            0.0
        };
        if density < MIN_MATCH_DENSITY {
            continue;
        }
        // Score by density * sqrt(duration) so that longer runs are preferred
        // when they're reasonably dense. A 90s run at 0.85 density scores higher
        // than a 6s run at 1.0 density (0.85 * 9.5 = 8.1 vs 1.0 * 2.4 = 2.4).
        let score = density * duration.sqrt();
        if score > best_score {
            best_score = score;
            best = Some((
                MatchedRegion {
                    start_seconds: lhs_start,
                    end_seconds: lhs_end + samples_to_seconds,
                    score: density,
                },
                MatchedRegion {
                    start_seconds: rhs_start,
                    end_seconds: rhs_end + samples_to_seconds,
                    score: density,
                },
            ));
        }
    }

    best
}

/// Compare two fingerprints and find matching regions using inverted-index alignment.
///
/// Instead of linear BER comparison at every position, builds inverted indexes
/// to find alignment shifts, then verifies contiguity at each shift.
pub fn find_matching_regions(
    fp_a: &Fingerprint,
    fp_b: &Fingerprint,
    _window_secs: f64,
    max_ber: f64,
) -> Vec<MatchedRegion> {
    let max_bit_diff = (max_ber * 32.0) as u32;
    let samples_to_seconds = if fp_a.data.is_empty() || fp_a.duration_seconds <= 0.0 {
        0.1238 // Chromaprint default
    } else {
        fp_a.duration_seconds / fp_a.data.len() as f64
    };
    let max_time_skip = 3.0; // Allow up to 3 second gaps in contiguous runs

    if fp_a.data.is_empty() || fp_b.data.is_empty() {
        return vec![];
    }

    // Find all possible alignment shifts
    let shifts = find_alignment_shifts(&fp_a.data, &fp_b.data, 2);

    let mut matches: Vec<MatchedRegion> = Vec::new();
    for shift in shifts {
        if let Some((lhs, _rhs)) = find_contiguous_at_shift(
            &fp_a.data,
            &fp_b.data,
            shift,
            max_bit_diff,
            samples_to_seconds,
            max_time_skip,
        ) {
            matches.push(lhs);
        }
    }

    // Sort by score descending, deduplicate overlapping regions
    matches.sort_by(|a, b| {
        b.score
            .partial_cmp(&a.score)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    deduplicate_regions(&mut matches);

    // Filter out low-density matches
    matches.retain(|m| m.score >= MIN_MATCH_DENSITY);

    // If we have a high-density match, prefer it over hunting for longer garbage
    if let Some(best) = matches.first()
        && best.score >= 0.85
    {
        matches.truncate(1);
    }

    debug!(count = matches.len(), "Found matching regions");
    matches
}

pub fn deduplicate_regions(regions: &mut Vec<MatchedRegion>) {
    let mut i = 0;
    while i < regions.len() {
        let mut j = i + 1;
        while j < regions.len() {
            let overlap_start = regions[i].start_seconds.max(regions[j].start_seconds);
            let overlap_end = regions[i].end_seconds.min(regions[j].end_seconds);
            if overlap_end > overlap_start {
                regions.remove(j);
            } else {
                j += 1;
            }
        }
        i += 1;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::skip_detect::chromaprint::find_pairwise_matches;

    const SPS: f64 = 0.1;

    fn synth(seed: u32, len: usize) -> Vec<u32> {
        let mut x = seed | 1;
        (0..len)
            .map(|_| {
                x ^= x << 13;
                x ^= x >> 17;
                x ^= x << 5;
                x
            })
            .collect()
    }

    /// Overlap window must be bounded per-side, not by `min(len) - |shift|`.
    /// When the two fingerprints differ in length (episodes with different
    /// runtimes), the latter truncates the comparison window and clips the run.
    #[test]
    fn contiguous_run_covers_full_overlap_for_unequal_lengths() {
        let shared = synth(0x5EED_1234, 150);
        let filler = synth(0xABCD_0001, 80);

        // shift > 0: short lhs, shared block sits late in the long rhs.
        let lhs = shared.clone();
        let rhs: Vec<u32> = filler.iter().chain(shared.iter()).copied().collect();
        let (l, r) = find_contiguous_at_shift(&lhs, &rhs, 80, 6, SPS, 3.0)
            .expect("shift=+80 must yield a run");
        assert!(
            (l.end_seconds - 15.0).abs() < 1e-6,
            "lhs run clipped: {:?}",
            l
        );
        assert!((l.start_seconds - 0.0).abs() < 1e-6, "lhs start: {:?}", l);
        assert!((r.start_seconds - 8.0).abs() < 1e-6, "rhs start: {:?}", r);
        assert!(
            (r.end_seconds - 23.0).abs() < 1e-6,
            "rhs run clipped: {:?}",
            r
        );

        // shift < 0: the same overlap, sides swapped.
        let (l, r) = find_contiguous_at_shift(&rhs, &lhs, -80, 6, SPS, 3.0)
            .expect("shift=-80 must yield a run");
        assert!((l.start_seconds - 8.0).abs() < 1e-6, "lhs start: {:?}", l);
        assert!(
            (l.end_seconds - 23.0).abs() < 1e-6,
            "lhs run clipped: {:?}",
            l
        );
        assert!((r.start_seconds - 0.0).abs() < 1e-6, "rhs start: {:?}", r);
        assert!(
            (r.end_seconds - 15.0).abs() < 1e-6,
            "rhs run clipped: {:?}",
            r
        );
    }

    /// A shift larger than the shorter fingerprint is still a valid alignment.
    #[test]
    fn contiguous_run_survives_shift_beyond_shorter_length() {
        let shared = synth(0x5EED_1234, 100);
        let filler = synth(0xABCD_0001, 120);

        let lhs = shared.clone();
        let rhs: Vec<u32> = filler.iter().chain(shared.iter()).copied().collect();
        let (l, _) = find_contiguous_at_shift(&lhs, &rhs, 120, 6, SPS, 3.0)
            .expect("shift beyond lhs.len() must still be searched");
        assert!((l.start_seconds - 0.0).abs() < 1e-6, "lhs start: {:?}", l);
        assert!((l.end_seconds - 10.0).abs() < 1e-6, "lhs end: {:?}", l);
    }

    /// End-to-end through the pairwise pipeline (shift discovery included).
    #[test]
    fn pairwise_pipeline_reports_full_shared_intro() {
        let shared = synth(0x5EED_1234, 150);
        let filler = synth(0xABCD_0001, 80);

        let fp_a = Fingerprint {
            data: shared.clone(),
            duration_seconds: 15.0,
        };
        let fp_b = Fingerprint {
            data: filler.iter().chain(shared.iter()).copied().collect(),
            duration_seconds: 23.0,
        };

        let matches = find_pairwise_matches(&fp_a, &fp_b, 1.0, 0.20);
        let best = matches
            .first()
            .expect("pipeline must find the shared block");
        assert!(
            (best.lhs.end_seconds - 15.0).abs() < 1e-6,
            "lhs {:?}",
            best.lhs
        );
        assert!(
            (best.rhs.start_seconds - 8.0).abs() < 1e-6,
            "rhs {:?}",
            best.rhs
        );
        assert!(
            (best.rhs.end_seconds - 23.0).abs() < 1e-6,
            "rhs {:?}",
            best.rhs
        );
    }
}
