//! Near-black + long-silence cut points, scored on the standard ad-length grid.
//!
//! Pure: takes `(start, end)` pairs so it never depends on the ffmpeg types.
//!
//! Two measured departures from comskip, both Australian:
//! - The caller must run `blackdetect` at `pic_th=0.85, pix_th=0.15`. AU networks
//!   fade to *near* black, and comskip's own 0.98/0.10 found 5 events in 11
//!   minutes — indistinguishable from "the detector does not work here".
//! - The silence discriminator is >= 0.9 s, not the p50. Break boundaries
//!   measured 0.92 s and 2.00 s against a p50 of 0.143 s, which is just speech
//!   pausing and carries no information.
//!
//! There is deliberately no logo detector: Seven alternates two different bugs,
//! so a persistence mask learns one and misses the other (SPIKES.md retraction).

pub const AD_LENGTH_GRID: [f64; 11] = [
    10.0, 15.0, 20.0, 25.0, 30.0, 45.0, 60.0, 90.0, 120.0, 150.0, 180.0,
];

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BlockCandidate {
    pub start_seconds: f64,
    pub end_seconds: f64,
    pub score: f64,
}

#[derive(Debug, Clone)]
pub struct BlockConfig {
    pub min_silence_secs: f64,
    pub colocation_secs: f64,
    pub grid_tolerance_secs: f64,
    pub max_combination: usize,
    pub min_pod_secs: f64,
    /// A span between two boundary cuts that is plausibly a whole break, used
    /// when the ads inside it could not be resolved.
    pub pod_fallback_secs: (f64, f64),
}

impl Default for BlockConfig {
    fn default() -> Self {
        Self {
            min_silence_secs: 0.9,
            colocation_secs: 1.0,
            grid_tolerance_secs: 1.0,
            max_combination: 4,
            min_pod_secs: 30.0,
            pod_fallback_secs: (45.0, 300.0),
        }
    }
}

fn cut_points(black: &[(f64, f64)], silence: &[(f64, f64)], cfg: &BlockConfig) -> Vec<f64> {
    let long_silence: Vec<(f64, f64)> = silence
        .iter()
        .copied()
        .filter(|(s, e)| e - s >= cfg.min_silence_secs)
        .collect();

    let mut cuts: Vec<f64> = black
        .iter()
        .filter(|(bs, be)| {
            long_silence
                .iter()
                .any(|(ss, se)| bs - se <= cfg.colocation_secs && ss - be <= cfg.colocation_secs)
        })
        .map(|(bs, be)| (bs + be) / 2.0)
        .collect();

    cuts.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    cuts.dedup_by(|a, b| (*a - *b).abs() < 0.05);
    cuts
}

fn on_grid(length: f64, tolerance: f64) -> bool {
    AD_LENGTH_GRID
        .iter()
        .any(|g| (length - g).abs() <= tolerance)
}

pub fn candidates(
    black: &[(f64, f64)],
    silence: &[(f64, f64)],
    cfg: &BlockConfig,
) -> Vec<BlockCandidate> {
    let cuts = cut_points(black, silence, cfg);
    if cuts.len() < 2 {
        return Vec::new();
    }
    let blocks: Vec<(f64, f64)> = cuts.windows(2).map(|w| (w[0], w[1])).collect();
    let lengths: Vec<f64> = blocks.iter().map(|(s, e)| e - s).collect();

    let direct: Vec<bool> = lengths
        .iter()
        .map(|len| on_grid(*len, cfg.grid_tolerance_secs))
        .collect();

    // Blocks are also tested in combination: a 30+15+15 run summing to 60 is a
    // recognisable pod even though the parts straddle grid values.
    let mut matched = direct.clone();
    for i in 0..blocks.len() {
        let mut sum = lengths[i];
        for span in 2..=cfg.max_combination {
            let j = i + span - 1;
            if j >= blocks.len() {
                break;
            }
            sum += lengths[j];
            if on_grid(sum, cfg.grid_tolerance_secs) {
                for m in matched.iter_mut().take(j + 1).skip(i) {
                    *m = true;
                }
            }
        }
    }

    // The grid describes individual ads, and resolving them needs a near-black
    // threshold this decode may not have. A span bounded at both ends by a
    // near-black co-located with a long silence is still evidence of a break, so
    // it stands at a lower score — `blocks` never marks anything on its own, so
    // a permissive corroborator costs the two-signal rule no precision.
    let mut out: Vec<BlockCandidate> = Vec::new();
    for (i, (start, end)) in blocks.iter().enumerate() {
        if matched[i] {
            continue;
        }
        let length = lengths[i];
        if length >= cfg.pod_fallback_secs.0 && length <= cfg.pod_fallback_secs.1 {
            out.push(BlockCandidate {
                start_seconds: *start,
                end_seconds: *end,
                score: 0.35,
            });
        }
    }

    let mut i = 0usize;
    while i < blocks.len() {
        if !matched[i] {
            i += 1;
            continue;
        }
        let start = i;
        while i < blocks.len() && matched[i] {
            i += 1;
        }
        let span_start = blocks[start].0;
        let span_end = blocks[i - 1].1;
        if span_end - span_start < cfg.min_pod_secs {
            continue;
        }
        let count = i - start;
        let hits = direct[start..i].iter().filter(|d| **d).count();
        out.push(BlockCandidate {
            start_seconds: span_start,
            end_seconds: span_end,
            score: (0.5 + 0.5 * (hits as f64 / count as f64)).clamp(0.0, 1.0),
        });
    }
    out.sort_by(|a, b| {
        a.start_seconds
            .partial_cmp(&b.start_seconds)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn black_at(times: &[f64]) -> Vec<(f64, f64)> {
        times.iter().map(|t| (*t, t + 0.08)).collect()
    }

    fn silence_at(times: &[f64], dur: f64) -> Vec<(f64, f64)> {
        times.iter().map(|t| (*t, t + dur)).collect()
    }

    #[test]
    fn short_speech_pauses_are_not_cut_points() {
        let black = black_at(&[100.0, 130.0]);
        let short = silence_at(&[100.0, 130.0], 0.143);
        assert!(cut_points(&black, &short, &BlockConfig::default()).is_empty());

        let long = silence_at(&[100.0, 130.0], 0.92);
        assert_eq!(cut_points(&black, &long, &BlockConfig::default()).len(), 2);
    }

    #[test]
    fn a_grid_shaped_pod_is_a_candidate() {
        let cuts = [164.0, 194.0, 224.0, 254.0, 284.0, 374.0];
        let black = black_at(&cuts);
        let silence = silence_at(&cuts, 1.2);
        let found = candidates(&black, &silence, &BlockConfig::default());
        assert_eq!(found.len(), 1, "{found:?}");
        assert!((found[0].start_seconds - 164.04).abs() < 0.2, "{found:?}");
        assert!((found[0].end_seconds - 374.04).abs() < 0.2, "{found:?}");
        assert!(found[0].score > 0.5);
    }

    #[test]
    fn blocks_that_only_sum_to_a_grid_value_still_form_a_pod() {
        // 7 + 15 + 15 + 23: only the two 15s sit on the grid individually, but
        // the four together sum to 60.
        let cuts = [0.0, 7.0, 22.0, 37.0, 60.0];
        let black = black_at(&cuts);
        let silence = silence_at(&cuts, 1.0);
        let found = candidates(&black, &silence, &BlockConfig::default());
        assert_eq!(found.len(), 1, "{found:?}");
        assert!(found[0].end_seconds - found[0].start_seconds >= 59.0);
        assert!((found[0].score - 0.75).abs() < 0.01, "{found:?}");
    }

    #[test]
    fn off_grid_blocks_produce_nothing() {
        let cuts = [0.0, 7.0, 13.5];
        let black = black_at(&cuts);
        let silence = silence_at(&cuts, 1.0);
        assert!(candidates(&black, &silence, &BlockConfig::default()).is_empty());
    }

    #[test]
    fn a_pod_shorter_than_the_minimum_is_dropped() {
        let cuts = [0.0, 10.0, 20.0];
        let black = black_at(&cuts);
        let silence = silence_at(&cuts, 1.0);
        assert!(candidates(&black, &silence, &BlockConfig::default()).is_empty());
    }
}

#[cfg(test)]
mod pod_fallback_tests {
    use super::*;

    /// Measured on `scratchpad/gt.ts`: at the `pix_th` this decode can set, the
    /// only near-black events co-located with a >= 0.9 s silence are the two
    /// break boundaries, so the ads inside never resolve onto the grid.
    #[test]
    fn a_pod_the_grid_cannot_resolve_still_produces_a_candidate() {
        let black = vec![(163.0, 163.16), (373.04, 373.24)];
        let silence = vec![(162.6, 164.6), (372.9, 373.82)];
        let out = candidates(&black, &silence, &BlockConfig::default());
        assert_eq!(out.len(), 1);
        assert!((out[0].start_seconds - 163.08).abs() < 0.2);
        assert!((out[0].end_seconds - 373.14).abs() < 0.2);
        assert!(out[0].score < 0.5, "a pod with no grid evidence scores low");
    }

    #[test]
    fn a_span_far_too_long_to_be_a_break_is_not_a_pod() {
        let black = vec![(100.0, 100.1), (900.0, 900.1)];
        let silence = vec![(99.0, 100.5), (899.5, 901.0)];
        assert!(candidates(&black, &silence, &BlockConfig::default()).is_empty());
    }

    #[test]
    fn a_grid_resolved_pod_still_beats_the_fallback_on_score() {
        let cfg = BlockConfig::default();
        let black: Vec<(f64, f64)> = [0.0, 30.0, 60.0, 90.0]
            .iter()
            .map(|t| (*t, t + 0.08))
            .collect();
        let silence: Vec<(f64, f64)> = [0.0, 30.0, 60.0, 90.0]
            .iter()
            .map(|t| (t - 0.5, t + 0.5))
            .collect();
        let out = candidates(&black, &silence, &cfg);
        assert_eq!(out.len(), 1);
        assert!(out[0].score > 0.5);
    }
}
