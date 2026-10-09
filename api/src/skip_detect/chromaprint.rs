//! Chromaprint audio fingerprinting via libchromaprint (in-process, no CLI).

use std::path::Path;
use tracing::debug;

use super::primitives::fingerprint_match::{find_alignment_shifts, find_contiguous_at_shift};

pub use super::primitives::fingerprint_match::{
    Fingerprint, MatchedRegion, PairwiseMatch, build_inverted_index, find_matching_regions,
};

/// Extract a Chromaprint fingerprint from a media file.
///
/// Uses kob-ffmpeg's chromaprint module (libchromaprint FFI, in-process).
/// `offset_secs` allows extracting from the end of the file by piping through ffmpeg.
pub async fn extract_fingerprint(
    file_path: &Path,
    offset_secs: f64,
    length_secs: u32,
) -> Result<Fingerprint, String> {
    let path = file_path.to_owned();
    let length = length_secs as f64;

    tokio::task::spawn_blocking(move || {
        let offset = if offset_secs > 0.0 {
            Some(offset_secs)
        } else {
            None
        };

        let kob_fp = kob_ffmpeg::chromaprint::extract_fingerprint(&path, offset, length)
            .map_err(|e| format!("fingerprint extraction failed: {e}"))?;

        Ok(Fingerprint {
            data: kob_fp.raw,
            duration_seconds: kob_fp.duration_secs,
        })
    })
    .await
    .map_err(|e| format!("task join error: {e}"))?
}

/// Compare two fingerprints and return both lhs and rhs positions for each match.
///
/// Same algorithm as `find_matching_regions` but preserves both sides of each match
/// for per-episode position tracking.
pub fn find_pairwise_matches(
    fp_a: &Fingerprint,
    fp_b: &Fingerprint,
    _window_secs: f64,
    max_ber: f64,
) -> Vec<PairwiseMatch> {
    let max_bit_diff = (max_ber * 32.0) as u32;
    let samples_to_seconds = if fp_a.data.is_empty() || fp_a.duration_seconds <= 0.0 {
        0.1238 // Chromaprint default
    } else {
        fp_a.duration_seconds / fp_a.data.len() as f64
    };
    let max_time_skip = 3.0;

    if fp_a.data.is_empty() || fp_b.data.is_empty() {
        return vec![];
    }

    let shifts = find_alignment_shifts(&fp_a.data, &fp_b.data, 2);

    let mut matches: Vec<PairwiseMatch> = Vec::new();
    for shift in shifts {
        if let Some((lhs, rhs)) = find_contiguous_at_shift(
            &fp_a.data,
            &fp_b.data,
            shift,
            max_bit_diff,
            samples_to_seconds,
            max_time_skip,
        ) {
            matches.push(PairwiseMatch { lhs, rhs });
        }
    }

    // Sort by lhs score descending, deduplicate overlapping lhs regions
    matches.sort_by(|a, b| {
        b.lhs
            .score
            .partial_cmp(&a.lhs.score)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    deduplicate_pairwise(&mut matches);

    matches.retain(|m| m.lhs.score >= super::MIN_MATCH_DENSITY);

    if let Some(best) = matches.first()
        && best.lhs.score >= 0.85
    {
        matches.truncate(1);
    }

    debug!(count = matches.len(), "Found pairwise matches");
    matches
}

fn deduplicate_pairwise(matches: &mut Vec<PairwiseMatch>) {
    let mut i = 0;
    while i < matches.len() {
        let mut j = i + 1;
        while j < matches.len() {
            let overlap_start = matches[i]
                .lhs
                .start_seconds
                .max(matches[j].lhs.start_seconds);
            let overlap_end = matches[i].lhs.end_seconds.min(matches[j].lhs.end_seconds);
            if overlap_end > overlap_start {
                matches.remove(j);
            } else {
                j += 1;
            }
        }
        i += 1;
    }
}

#[cfg(test)]
mod tests {
    /// Compute the bit error rate between two 32-bit sub-fingerprints.
    /// Lower = more similar. 0.0 = identical, 1.0 = completely different.
    fn bit_error_rate(a: u32, b: u32) -> f64 {
        let diff = a ^ b;
        let errors = diff.count_ones();
        errors as f64 / 32.0
    }

    use super::*;
    use crate::skip_detect::primitives::fingerprint_match::deduplicate_regions;

    #[test]
    fn test_bit_error_rate_identical() {
        assert!((bit_error_rate(0x12345678, 0x12345678) - 0.0).abs() < f64::EPSILON);
    }

    #[test]
    fn test_bit_error_rate_opposite() {
        assert!((bit_error_rate(0x00000000, 0xFFFFFFFF) - 1.0).abs() < f64::EPSILON);
    }

    #[test]
    fn test_bit_error_rate_partial() {
        // One bit different = 1/32
        assert!((bit_error_rate(0x00000000, 0x00000001) - 1.0 / 32.0).abs() < f64::EPSILON);
    }

    #[test]
    fn test_find_matching_regions() {
        // Use pseudo-random values with high hamming distance between neighbors
        // so only exact-position matches (shift=0) produce contiguous runs
        let shared: Vec<u32> = vec![
            0xA5A5A5A5, 0x5A5A5A5A, 0x12345678, 0x87654321, 0xDEADBEEF, 0xCAFEBABE, 0xF00DBABE,
            0xBAADF00D, 0x1337C0DE, 0xDECAFBAD, 0xFACEFEED, 0x0BADC0DE, 0xC0FFEE00, 0xBEEFCAFE,
            0xFEEDFACE, 0xBABECAFE,
        ];
        let mut data_a = shared.clone();
        data_a.extend([0x11111111, 0x22222222, 0x33333333]);
        let mut data_b = shared;
        data_b.extend([0xEEEEEEEE, 0xDDDDDDDD, 0xCCCCCCCC]);

        let fp_a = Fingerprint {
            data: data_a,
            duration_seconds: 19.0,
        };
        let fp_b = Fingerprint {
            data: data_b,
            duration_seconds: 19.0,
        };
        let matches = find_matching_regions(&fp_a, &fp_b, 1.0, 0.20);
        assert!(
            !matches.is_empty(),
            "Should find at least one matching region"
        );
        // The shift=0 alignment should produce the best match from the start
        assert!(
            matches[0].start_seconds < 2.0,
            "start={}",
            matches[0].start_seconds
        );
        assert!(
            matches[0].end_seconds > 14.0,
            "end={}",
            matches[0].end_seconds
        );
    }

    #[test]
    fn test_inverted_index_basic() {
        let data = vec![10u32, 20, 30, 10]; // last insert wins for key 10
        let index = build_inverted_index(&data);
        assert_eq!(index.get(&10), Some(&3)); // position 3, not 0
        assert_eq!(index.get(&20), Some(&1));
        assert_eq!(index.get(&30), Some(&2));
    }

    #[test]
    fn test_pairwise_matches_both_sides() {
        // fp_a has shared data at start (index 0), fp_b has it at offset (index 8)
        let shared: Vec<u32> = vec![
            0xA5A5A5A5, 0x5A5A5A5A, 0x12345678, 0x87654321, 0xDEADBEEF, 0xCAFEBABE, 0xF00DBABE,
            0xBAADF00D, 0x1337C0DE, 0xDECAFBAD, 0xFACEFEED, 0x0BADC0DE, 0xC0FFEE00, 0xBEEFCAFE,
            0xFEEDFACE, 0xBABECAFE,
        ];
        let mut data_a = shared.clone();
        data_a.extend([0x11111111, 0x22222222, 0x33333333, 0x44444444, 0x55555555]);

        let mut data_b = vec![
            0xAAAAAAAA, 0xBBBBBBBB, 0xCCCCCCCC, 0xDDDDDDDD, 0xEEEEEEEE, 0x99999999, 0x88888888,
            0x77777777,
        ];
        data_b.extend(shared);

        let fp_a = Fingerprint {
            data: data_a,
            duration_seconds: 21.0,
        };
        let fp_b = Fingerprint {
            data: data_b,
            duration_seconds: 24.0,
        };

        let matches = find_pairwise_matches(&fp_a, &fp_b, 1.0, 0.20);
        assert!(!matches.is_empty(), "Should find pairwise match");

        let m = &matches[0];
        // lhs match near start of fp_a (index 0)
        assert!(
            m.lhs.start_seconds < 2.0,
            "lhs start={}",
            m.lhs.start_seconds
        );
        // rhs match at offset ~8 samples in fp_b
        assert!(
            m.rhs.start_seconds > 3.0,
            "rhs start={} should be offset",
            m.rhs.start_seconds
        );
        // Both sides should have same score
        assert!(
            (m.lhs.score - m.rhs.score).abs() < 0.01,
            "scores should match: lhs={} rhs={}",
            m.lhs.score,
            m.rhs.score
        );
    }

    #[test]
    fn test_deduplicate_overlapping_regions() {
        let mut regions = vec![
            MatchedRegion {
                start_seconds: 0.0,
                end_seconds: 10.0,
                score: 0.9,
            },
            MatchedRegion {
                start_seconds: 5.0,
                end_seconds: 15.0,
                score: 0.8,
            },
            MatchedRegion {
                start_seconds: 20.0,
                end_seconds: 30.0,
                score: 0.7,
            },
        ];
        deduplicate_regions(&mut regions);
        assert_eq!(regions.len(), 2); // first two overlap, third is kept
    }
}
