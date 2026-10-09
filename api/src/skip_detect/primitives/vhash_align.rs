//! Pure inverted-index alignment + scoring for vHash descriptors.
//!
//! No I/O. Operates over `&[FrameDescriptor]` slices and returns matched
//! regions. `vhash.rs` extracts the descriptors via libav, then calls into
//! these helpers.

use std::collections::HashMap;

/// Frames per second the matching pipeline expects descriptors to be sampled at.
pub const VHASH_FPS: f64 = 3.0;

/// Tight similarity threshold for shift voting (higher bar for alignment discovery).
const VOTE_SIMILARITY: f64 = 0.80;

/// A compact visual descriptor for a single frame.
/// 16x16 grayscale. Spatial structure is captured without being
/// sensitive to small spatial shifts (unlike 9x8 dHash).
#[derive(Debug, Clone)]
pub struct FrameDescriptor {
    pub timestamp: f64,
    pub values: Vec<u8>,
    /// Coarse quantized hash for alignment discovery via inverted index.
    pub quantized: u32,
}

/// A matched visual region.
#[derive(Debug, Clone)]
pub struct VHashMatch {
    pub start_seconds: f64,
    pub end_seconds: f64,
    pub density: f64,
    /// Mean per-frame similarity across surviving frames in the run.
    /// Separate from `density` (proportion of frames clearing the per-frame
    /// bar): a 42s run that barely-passes each frame (many in the 0.70–0.75
    /// band) can have density=0.96 but mean_similarity=0.78. Genuine repeated
    /// content runs 0.9+.
    pub mean_similarity: f64,
}

/// A pairwise vHash match containing positions from both sequences.
#[derive(Debug, Clone)]
pub struct VHashPairMatch {
    pub lhs: VHashMatch,
    pub rhs: VHashMatch,
}

/// Mean-centered cosine similarity (Pearson correlation) between two descriptors.
///
/// Subtracting the mean before computing cosine similarity makes it invariant
/// to overall brightness and discriminates spatial patterns. Without centering,
/// all-positive grayscale vectors cluster together (cosine > 0.7 for any two
/// non-black frames), making the metric useless for discrimination.
///
/// Zero-variance vectors (constant — e.g. solid-black fade frames) fall back
/// to a mean-distance check: two constant frames at similar brightness are
/// visually identical (both black or both white) and must be treated as a
/// match, otherwise intros that fade to/from black silently break into
/// short sub-min-duration runs.
pub fn frame_similarity(a: &[u8], b: &[u8]) -> f64 {
    let n = a.len().min(b.len());
    if n == 0 {
        return 0.0;
    }
    let mean_a = a[..n].iter().map(|&v| v as f64).sum::<f64>() / n as f64;
    let mean_b = b[..n].iter().map(|&v| v as f64).sum::<f64>() / n as f64;

    let mut dot = 0.0f64;
    let mut norm_a = 0.0f64;
    let mut norm_b = 0.0f64;
    for (&va_raw, &vb_raw) in a[..n].iter().zip(b[..n].iter()) {
        let va = va_raw as f64 - mean_a;
        let vb = vb_raw as f64 - mean_b;
        dot += va * vb;
        norm_a += va * va;
        norm_b += vb * vb;
    }
    let denom = norm_a.sqrt() * norm_b.sqrt();
    if denom == 0.0 {
        // Pearson is undefined when either side has zero variance. Only claim
        // a match when BOTH sides are constant at approximately the same
        // value (both black, both white, etc.) — those frames really are
        // visually identical. A constant-vs-varied pair has no meaningful
        // similarity; return 0.
        if norm_a == 0.0 && norm_b == 0.0 && (mean_a - mean_b).abs() < 4.0 {
            return 1.0;
        }
        return 0.0;
    }
    (dot / denom).max(0.0)
}

/// Quantize a descriptor into a u32 hash for inverted-index alignment.
///
/// Divides the grid into 4x4 blocks, averages each → 16 values.
/// Thresholds at the global median → 16 bits.
/// Upper 16 bits encode variance (texture information).
pub fn quantize_descriptor(values: &[u8]) -> u32 {
    let size = (values.len() as f64).sqrt() as usize;
    let mut block_avgs = [0u16; 16];
    let block_size = size / 4;

    for by in 0..4 {
        for bx in 0..4 {
            let mut sum = 0u32;
            let mut count = 0u32;
            for py in 0..block_size {
                for px in 0..block_size {
                    let y = by * block_size + py;
                    let x = bx * block_size + px;
                    if y * size + x < values.len() {
                        sum += values[y * size + x] as u32;
                        count += 1;
                    }
                }
            }
            if let Some(avg) = sum.checked_div(count) {
                block_avgs[by * 4 + bx] = avg as u16;
            }
        }
    }

    // Median of block averages
    let mut sorted = block_avgs;
    sorted.sort_unstable();
    let median = (sorted[7] as u32 + sorted[8] as u32) / 2;

    // Lower 16 bits: threshold at median
    let mut hash: u32 = 0;
    for (i, &avg) in block_avgs.iter().enumerate() {
        if avg as u32 > median {
            hash |= 1 << i;
        }
    }

    // Upper 16 bits: variance encoding (captures texture)
    let mean = block_avgs.iter().map(|&v| v as u32).sum::<u32>() / 16;
    let variance = block_avgs
        .iter()
        .map(|&v| {
            let diff = v as i32 - mean as i32;
            (diff * diff) as u32
        })
        .sum::<u32>()
        / 16;
    // Quantize variance into 16 levels (4 bits), spread across upper 16 bits
    let var_level = ((variance.min(4096) as f64 / 4096.0) * 15.0) as u32;
    hash |= var_level << 16;

    hash
}

/// Find alignment shifts between two descriptor sequences using inverted index.
///
/// Returns the top alignment shifts sorted by vote count (descending).
fn find_alignment_shifts(seq_a: &[FrameDescriptor], seq_b: &[FrameDescriptor]) -> Vec<i32> {
    // Build inverted index for seq_b: quantized_hash → list of frame indices
    let mut index_b: HashMap<u32, Vec<usize>> = HashMap::new();
    for (i, desc) in seq_b.iter().enumerate() {
        index_b.entry(desc.quantized).or_default().push(i);
    }

    // For each frame in seq_a, find matching quantized hashes in seq_b
    // and record the shift (in frame indices)
    let mut shift_votes: HashMap<i32, usize> = HashMap::new();
    for (i, desc_a) in seq_a.iter().enumerate() {
        if let Some(matches) = index_b.get(&desc_a.quantized) {
            for &j in matches {
                // Verify with actual similarity before voting
                let sim = frame_similarity(&desc_a.values, &seq_b[j].values);
                if sim >= VOTE_SIMILARITY {
                    let shift = j as i32 - i as i32;
                    *shift_votes.entry(shift).or_default() += 1;
                }
            }
        }
    }

    // Return shifts with at least 2 votes, sorted by vote count desc.
    // Tie-break on the shift value (asc) so the output is stable across
    // runs — `shift_votes` is a HashMap whose iteration order is seeded
    // randomly, and downstream `find_visual_matches` extends boundaries
    // starting from the first shift; without a tiebreaker the picked
    // match drifts run-to-run on inputs with multiple equally-supported
    // shifts (Paradise S02E06, three near-identical recurring scenes).
    let mut shifts: Vec<(i32, usize)> = shift_votes
        .into_iter()
        .filter(|&(_, count)| count >= 2)
        .collect();
    shifts.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));

    // Keep top shifts (up to 10) — more than enough for any real alignment
    shifts.truncate(10);
    shifts.into_iter().map(|(shift, _)| shift).collect()
}

/// Validate match quality at a specific alignment shift using continuous similarity.
///
/// Returns the best contiguous run of similar frames at this shift.
fn validate_at_shift(
    seq_a: &[FrameDescriptor],
    seq_b: &[FrameDescriptor],
    shift: i32,
    min_similarity: f64,
) -> Vec<(f64, f64, f64)> {
    // Collect all aligned pairs with their similarity
    let mut pairs: Vec<(f64, f64, f64)> = Vec::new(); // (ts_a, ts_b, similarity)

    for (i, desc_a) in seq_a.iter().enumerate() {
        let j = i as i32 + shift;
        if j < 0 || j as usize >= seq_b.len() {
            continue;
        }
        let desc_b = &seq_b[j as usize];
        let sim = frame_similarity(&desc_a.values, &desc_b.values);
        if sim >= min_similarity {
            pairs.push((desc_a.timestamp, desc_b.timestamp, sim));
        }
    }

    pairs
}

/// Find matching visual regions between two episodes.
/// Returns both lhs AND rhs positions (per-episode matching).
///
/// Uses adaptive shift matching: the intro's internal timing varies slightly
/// between episodes (different cut points in the montage), so a single alignment
/// shift works for the middle but is off by 1-3 frames at the edges.
///
/// Strategy: find the best single-shift match, then extend its boundaries by
/// checking nearby shifts for frames that the primary shift missed.
pub fn find_visual_matches(
    seq_a: &[FrameDescriptor],
    seq_b: &[FrameDescriptor],
    min_duration: f64,
    min_similarity: f64,
) -> Option<VHashPairMatch> {
    if seq_a.is_empty() || seq_b.is_empty() {
        return None;
    }

    let shifts = find_alignment_shifts(seq_a, seq_b);
    if shifts.is_empty() {
        return None;
    }

    // Phase 1: Find the best single-shift match (anchors the region).
    //
    // Runs are first gated by density: a long-but-sparse run (many gaps)
    // must not win over a short-but-dense run just because it spans more
    // wall-clock. find_best_ordered_run permits gaps up to max_gap, so
    // without the density gate here a mostly-unmatched shift can end up
    // scoring higher than the real aligned shift when we tighten the
    // per-frame similarity filter.
    let mut best_run: Option<(VHashMatch, VHashMatch)> = None;
    let mut best_score = 0.0f64;

    for &shift in &shifts {
        let pairs = validate_at_shift(seq_a, seq_b, shift, min_similarity);
        if pairs.is_empty() {
            continue;
        }
        if let Some((lhs, rhs)) = find_best_ordered_run(&pairs, min_duration, VHASH_FPS) {
            if lhs.density < min_similarity {
                continue;
            }
            let score = lhs.density * (lhs.end_seconds - lhs.start_seconds);
            if score > best_score {
                best_score = score;
                best_run = Some((lhs, rhs));
            }
        }
    }

    let (anchor_lhs, _) = best_run.as_ref()?;

    // Phase 2: Extend boundaries using ALL shifts.
    // For each frame in seq_a, check if ANY shift produces a match above threshold.
    // This catches frames where the primary shift is off by 1-2 frames due to
    // variable cut timing in the montage.
    // Collect (ts_a, ts_b, sim) triples — track both sides so we can enforce
    // temporal monotonicity on the b-side (prevents one "sticky" frame
    // in seq_b from matching many seq_a frames and extending the region).
    // Similarity is kept so we can compute the run's mean_similarity at the
    // end — a sandwich match (anchor + noise + anchor) can pass density but
    // have a low mean, which the caller can use to reject it.
    let mut matched_pairs: Vec<(f64, f64, f64)> = Vec::new();

    for (i, desc_a) in seq_a.iter().enumerate() {
        if desc_a.timestamp < anchor_lhs.start_seconds - 30.0
            || desc_a.timestamp > anchor_lhs.end_seconds + 30.0
        {
            continue;
        }

        let mut best_sim = 0.0f64;
        let mut best_b_ts = 0.0f64;
        for &shift in &shifts {
            let j = i as i32 + shift;
            if j < 0 || j as usize >= seq_b.len() {
                continue;
            }
            let sim = frame_similarity(&desc_a.values, &seq_b[j as usize].values);
            if sim > best_sim {
                best_sim = sim;
                best_b_ts = seq_b[j as usize].timestamp;
            }
        }
        if best_sim >= min_similarity {
            matched_pairs.push((desc_a.timestamp, best_b_ts, best_sim));
        }
    }

    if matched_pairs.is_empty() {
        return best_run.map(|(lhs, rhs)| VHashPairMatch { lhs, rhs });
    }

    matched_pairs.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));

    // Find longest contiguous run where BOTH a and b timestamps advance.
    // This prevents a single sticky b-frame from extending the match.
    let frame_interval = 1.0 / VHASH_FPS;
    let max_gap = 15.0 * frame_interval + 0.5;

    let mut best_start = matched_pairs[0].0;
    let mut best_end = matched_pairs[0].0;
    let mut best_count = 1usize;
    let mut run_start = matched_pairs[0].0;
    let mut run_start_idx = 0usize;
    let mut run_count = 1usize;
    let mut last_b: f64 = matched_pairs[0].1;

    for i in 1..matched_pairs.len() {
        let gap_a = matched_pairs[i].0 - matched_pairs[i - 1].0;
        // Check that b is progressing — not stagnating on one sticky frame.
        // Allow b to go backwards by up to 3s (timing shifts between episodes)
        // but also require b to have advanced from the run start over longer spans.
        // A sticky frame shows as b staying constant while a advances.
        let b_not_backwards = matched_pairs[i].1 >= last_b - 3.0;
        let a_span = matched_pairs[i].0 - run_start;
        let b_span = matched_pairs[i].1 - matched_pairs[run_start_idx].1;
        // If a has advanced >5s, b should have advanced at least 25% as much
        let b_progressing = a_span < 5.0 || b_span > a_span * 0.25;
        let b_advances = b_not_backwards && b_progressing;

        if gap_a <= max_gap && b_advances {
            run_count += 1;
            if matched_pairs[i].1 > last_b {
                last_b = matched_pairs[i].1;
            }
        } else {
            if run_count > best_count {
                best_start = run_start;
                best_end = matched_pairs[i - 1].0;
                best_count = run_count;
            }
            run_start = matched_pairs[i].0;
            run_start_idx = i;
            run_count = 1;
            last_b = matched_pairs[i].1;
        }
    }
    if run_count > best_count {
        best_start = run_start;
        best_end = matched_pairs.last().unwrap().0;
        best_count = run_count;
    }

    let duration = best_end - best_start;
    if duration < min_duration {
        return best_run.map(|(lhs, rhs)| VHashPairMatch { lhs, rhs });
    }

    let expected = (duration * VHASH_FPS) as usize + 1;
    let density = (best_count as f64 / expected as f64).min(1.0);

    // Reject low-density matches — real intro sequences have density ~1.0.
    // False matches from similar-looking but unrelated content have 0.50-0.65.
    if density < min_similarity {
        return best_run.map(|(lhs, rhs)| VHashPairMatch { lhs, rhs });
    }

    // Use actual b-timestamps from the matched pairs in the best run,
    // not the shift estimate (which can produce negative/wrong values
    // when the phase 2 extension spans different alignment shifts).
    let run_pairs: Vec<&(f64, f64, f64)> = matched_pairs
        .iter()
        .filter(|(a, _, _)| *a >= best_start && *a <= best_end + frame_interval)
        .collect();
    let rhs_start = run_pairs
        .iter()
        .map(|(_, b, _)| *b)
        .fold(f64::MAX, f64::min);
    let rhs_end = run_pairs
        .iter()
        .map(|(_, b, _)| *b)
        .fold(f64::MIN, f64::max);
    let mean_similarity = if run_pairs.is_empty() {
        0.0
    } else {
        run_pairs.iter().map(|(_, _, s)| *s).sum::<f64>() / run_pairs.len() as f64
    };

    Some(VHashPairMatch {
        lhs: VHashMatch {
            start_seconds: best_start,
            end_seconds: best_end + frame_interval,
            density,
            mean_similarity,
        },
        rhs: VHashMatch {
            start_seconds: rhs_start.max(0.0),
            end_seconds: rhs_end + frame_interval,
            density,
            mean_similarity,
        },
    })
}

/// Find the best ordered run of matched frame pairs.
/// Both sequences of timestamps must be monotonically increasing.
/// Pairs are `(ts_a, ts_b, similarity)`; the returned match carries the
/// run's mean similarity so the caller can distinguish "dense high-quality
/// run" from "dense but marginal run" (sandwich match anchored by a black
/// frame + low-quality mid-region + logo block).
fn find_best_ordered_run(
    pairs: &[(f64, f64, f64)],
    min_duration: f64,
    fps: f64,
) -> Option<(VHashMatch, VHashMatch)> {
    if pairs.is_empty() {
        return None;
    }

    let frame_interval = 1.0 / fps;
    // Allow gaps of up to ~5s to bridge sections where the montage timing
    // shifts between episodes (e.g., a 3-4s gap between segments with
    // different time offsets). Scene transitions can cause 1-3 frame dips,
    // and timing discontinuities can cause 3-5s gaps.
    let max_gap = 15.0 * frame_interval + 0.5;

    let mut best: Option<(VHashMatch, VHashMatch)> = None;
    let mut best_density: f64 = 0.0;
    let mut run_start = 0usize;

    for i in 1..=pairs.len() {
        let broke = if i < pairs.len() {
            let gap_a = pairs[i].0 - pairs[i - 1].0;
            let gap_b = pairs[i].1 - pairs[i - 1].1;
            gap_a > max_gap || gap_b < 0.0 || gap_b > max_gap
        } else {
            true
        };

        if broke {
            let run_end = i - 1;
            let duration = pairs[run_end].0 - pairs[run_start].0;
            if duration >= min_duration {
                let run_len = run_end - run_start + 1;
                let expected_frames = (duration * fps) as usize + 1;
                let density = if expected_frames > 0 {
                    (run_len as f64 / expected_frames as f64).min(1.0)
                } else {
                    0.0
                };
                let mean_similarity =
                    pairs[run_start..=run_end].iter().map(|p| p.2).sum::<f64>() / run_len as f64;
                if density > best_density
                    || (density == best_density
                        && best
                            .as_ref()
                            .is_some_and(|b| duration > b.0.end_seconds - b.0.start_seconds))
                {
                    best_density = density;
                    best = Some((
                        VHashMatch {
                            start_seconds: pairs[run_start].0,
                            end_seconds: pairs[run_end].0 + frame_interval,
                            density,
                            mean_similarity,
                        },
                        VHashMatch {
                            start_seconds: pairs[run_start].1,
                            end_seconds: pairs[run_end].1 + frame_interval,
                            density,
                            mean_similarity,
                        },
                    ));
                }
            }
            if i < pairs.len() {
                run_start = i;
            }
        }
    }

    best
}

#[cfg(test)]
#[allow(clippy::needless_range_loop)]
mod tests {
    use super::*;

    fn make_descriptor(timestamp: f64, values: &[u8]) -> FrameDescriptor {
        let values = values.to_vec();
        let quantized = quantize_descriptor(&values);
        FrameDescriptor {
            timestamp,
            values,
            quantized,
        }
    }

    #[test]
    fn alignment_identical_sequences() {
        let descs: Vec<FrameDescriptor> = (0..30)
            .map(|i| {
                let mut values = [0u8; 256];
                // Each frame has a unique pattern
                for j in 0..256 {
                    values[j] = ((i * 37 + j) % 256) as u8;
                }
                make_descriptor(i as f64 / 3.0, &values)
            })
            .collect();

        let shifts = find_alignment_shifts(&descs, &descs);
        assert!(!shifts.is_empty(), "identical sequences should find shifts");
        assert_eq!(
            shifts[0], 0,
            "dominant shift should be 0 for identical seqs"
        );
    }

    #[test]
    fn ordered_run_basic() {
        let pairs: Vec<(f64, f64, f64)> = (0..10)
            .map(|i| (i as f64 * 0.333, i as f64 * 0.333 + 1.0, 0.95))
            .collect();

        let result = find_best_ordered_run(&pairs, 2.0, 3.0);
        assert!(result.is_some());
        let (lhs, rhs) = result.unwrap();
        assert!(lhs.end_seconds - lhs.start_seconds >= 2.0);
        assert!(
            (rhs.start_seconds - lhs.start_seconds - 1.0).abs() < 0.5,
            "rhs offset should be ~1.0"
        );
        assert!(
            (lhs.mean_similarity - 0.95).abs() < 1e-6,
            "mean_similarity should be 0.95, got {}",
            lhs.mean_similarity
        );
    }
}
