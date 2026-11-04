//! Pure scoring for zero-shot credit text detection.
//!
//! The embedding client turns cached frame vectors into a per-frame /
//! per-prompt cosine matrix; this module collapses prompt scores, thresholds
//! them, and groups gap-tolerant regions into [`CreditText`]. The prompts and
//! every score-scale number come from the model's
//! [`ModelProfile`](crate::embedding::profile::ModelProfile).

use crate::embedding::profile::ModelProfile;

/// Maximum gap (seconds) between hits to merge them into the same region.
pub const GAP_TOLERANCE: f64 = 10.0;

/// Minimum number of hits required for a region to count as credits.
pub const MIN_HITS: usize = 5;

/// Name cards on black often precede the crawl, split from it by logo cards or
/// a season preview vision doesn't score as text. The crawl is usually denser,
/// but the episode ended where the cards began.
const MAX_CREDITS_INTERLUDE_SECS: f64 = 150.0;
/// Sustained enough to be a credit block rather than on-screen text in a scene.
const MIN_CHAINED_REGION_HITS: usize = 30;
/// Longer than logo cards ever run between two credit blocks, so what fills the
/// gap is footage — a mid-credits scene or a preview — and must stay watchable.
const MAX_SAME_BLOCK_GAP_SECS: f64 = 30.0;

#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct CreditText {
    /// Earliest first. The last is the roll itself; any before it are credit
    /// blocks with footage between them and the roll.
    pub blocks: Vec<(f64, f64)>,
    pub confidence: f64,
    /// First text vision sees again after the roll — a vanity card or studio
    /// logo. What lies between the roll and it is footage.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub after_roll: Option<f64>,
}

impl CreditText {
    pub fn start(&self) -> f64 {
        self.blocks[0].0
    }

    pub fn roll(&self) -> (f64, f64) {
        self.blocks[self.blocks.len() - 1]
    }

    pub fn lead_blocks(&self) -> &[(f64, f64)] {
        &self.blocks[..self.blocks.len() - 1]
    }
}

/// The prompts to classify against, credit prompts first — the order
/// [`score_credit_text`] expects its score rows in.
pub fn credit_prompts(profile: &ModelProfile) -> Vec<&'static str> {
    profile
        .credit_prompts
        .iter()
        .chain(profile.non_credit_prompts)
        .copied()
        .collect()
}

/// Score a batch of classification results for credit text.
///
/// `scores[i]` is the per-prompt score row for frame `i`, ordered as
/// [`credit_prompts`] returns them. `frame_times[i]` is the
/// timestamp (seconds into the file) of frame `i`.
///
/// Returns `(credits_start, credits_end, confidence)` for the densest detected
/// region — extended back over preceding credit blocks, never before
/// `earliest_start` — or `None` if too few frames cleared the threshold.
pub fn score_credit_text(
    scores: &[Vec<f32>],
    frame_times: &[f64],
    earliest_start: f64,
    profile: &ModelProfile,
) -> Option<CreditText> {
    if scores.len() != frame_times.len() {
        return None;
    }

    let credit_prompt_count = profile.credit_prompts.len();
    let non_credit_count = profile.non_credit_prompts.len();
    let total_prompts = credit_prompt_count + non_credit_count;

    let mut frame_scores: Vec<(f64, f64)> = Vec::new();
    for (i, score_row) in scores.iter().enumerate() {
        if score_row.len() != total_prompts {
            continue;
        }
        let credit_avg: f64 = score_row[..credit_prompt_count]
            .iter()
            .map(|&s| s as f64)
            .sum::<f64>()
            / credit_prompt_count as f64;
        let non_credit_avg: f64 = score_row[credit_prompt_count..]
            .iter()
            .map(|&s| s as f64)
            .sum::<f64>()
            / non_credit_count as f64;
        let credit_score = (credit_avg - non_credit_avg).max(0.0);
        frame_scores.push((frame_times[i], credit_score));
    }

    detect_credit_regions(&frame_scores, earliest_start, profile)
}

/// Pure region grouping — public so tests can exercise it directly.
pub fn detect_credit_regions(
    frame_scores: &[(f64, f64)],
    earliest_start: f64,
    profile: &ModelProfile,
) -> Option<CreditText> {
    let hits: Vec<(f64, f64)> = frame_scores
        .iter()
        .filter(|(_, score)| *score >= profile.credit_threshold)
        .copied()
        .collect();

    if hits.len() < MIN_HITS {
        return None;
    }

    // Group hits into regions: merge hits within GAP_TOLERANCE of each other
    let mut regions: Vec<(f64, f64, Vec<f64>)> = vec![]; // (start, end, scores)
    let mut region_start = hits[0].0;
    let mut region_end = hits[0].0;
    let mut region_scores = vec![hits[0].1];

    for &(ts, score) in &hits[1..] {
        if ts - region_end <= GAP_TOLERANCE {
            region_end = ts;
            region_scores.push(score);
        } else {
            regions.push((region_start, region_end, region_scores.clone()));
            region_start = ts;
            region_end = ts;
            region_scores = vec![score];
        }
    }
    regions.push((region_start, region_end, region_scores));

    let best_idx = regions
        .iter()
        .enumerate()
        .max_by_key(|(_, r)| r.2.len())
        .map(|(i, _)| i)?;
    let scores = &regions[best_idx].2;
    let avg_score = scores.iter().sum::<f64>() / scores.len() as f64;
    let confidence = (avg_score / profile.credit_confidence_full).clamp(0.3, 0.95);
    let substantial = |i: usize| i == best_idx || regions[i].2.len() >= MIN_CHAINED_REGION_HITS;

    let mut first = best_idx;
    // A short run of logo cards does not start the credits, but it does not
    // break the chain to name cards before it either.
    let mut edge = regions[best_idx].0;
    for i in (0..best_idx).rev() {
        if edge - regions[i].1 > MAX_CREDITS_INTERLUDE_SECS || regions[i].0 < earliest_start {
            break;
        }
        edge = regions[i].0;
        if substantial(i) {
            first = i;
        }
    }

    let mut blocks: Vec<(f64, f64, bool)> = Vec::new();
    for (i, region) in regions.iter().enumerate().take(best_idx + 1).skip(first) {
        let (start, end) = (region.0, region.1);
        match blocks.last_mut() {
            Some(block) if start - block.1 <= MAX_SAME_BLOCK_GAP_SECS => {
                block.1 = end;
                block.2 |= substantial(i);
            }
            _ => blocks.push((start, end, substantial(i))),
        }
    }

    Some(CreditText {
        blocks: blocks
            .into_iter()
            .filter(|b| b.2)
            .map(|(start, end, _)| (start, end))
            .collect(),
        confidence,
        after_roll: regions.get(best_idx + 1).map(|r| r.0),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::embedding::profile::EG2;

    const NO_FLOOR: f64 = f64::NEG_INFINITY;

    /// The grouping logic is scale-free; these cases are written on a 0.10
    /// threshold / 0.5 full-confidence scale whatever the live model's is.
    const SCALE: ModelProfile = ModelProfile {
        credit_threshold: 0.10,
        credit_confidence_full: 0.5,
        ..EG2
    };

    fn regions(scores: &[(f64, f64)], floor: f64) -> Option<CreditText> {
        detect_credit_regions(scores, floor, &SCALE)
    }

    fn block(scores: &mut Vec<(f64, f64)>, from: u32, to: u32, score: f64) {
        scores.extend((from..to).map(|t| (t as f64, score)));
    }

    fn flat(text: Option<CreditText>) -> Option<(f64, f64, f64)> {
        text.map(|t| (t.start(), t.roll().1, t.confidence))
    }

    /// Carrie 1x01: name cards on black, a two-minute season preview of real
    /// footage, a few seconds of title and studio logos, then the crawl. The
    /// episode ended where the name cards began, but the preview between them is
    /// footage — the slot a mid-credits scene takes — so it is left out of both.
    /// Rick and Morty 9x01, last 92s as EmbeddingGemma-2 scores them: the
    /// crawl to 1292, the post-credits scene, then the vanity card and the
    /// Williams Street logo from 1328.
    #[test]
    fn rick_and_morty_s9e01_marks_where_text_returns_after_the_roll() {
        let scores: [f64; 93] = [
            -0.046, -0.044, -0.046, -0.053, -0.049, -0.048, -0.043, -0.039, -0.044, -0.044, -0.041,
            -0.041, -0.045, -0.035, -0.038, -0.036, -0.034, -0.043, -0.041, -0.042, 0.071, 0.072,
            0.068, 0.068, 0.069, 0.076, 0.082, 0.076, 0.099, 0.085, 0.076, 0.063, 0.075, 0.082,
            0.068, 0.068, 0.074, 0.083, 0.074, 0.074, 0.067, 0.069, 0.066, 0.082, 0.091, 0.067,
            0.069, 0.059, 0.084, 0.086, 0.089, 0.081, -0.029, -0.020, -0.039, -0.035, -0.034,
            -0.040, -0.041, -0.034, -0.038, -0.032, -0.039, -0.030, -0.037, -0.040, -0.038, -0.049,
            -0.044, -0.038, -0.041, -0.031, -0.047, -0.033, -0.032, -0.034, -0.035, -0.032, -0.033,
            -0.033, -0.041, -0.037, -0.033, -0.028, -0.026, -0.028, -0.029, 0.035, -0.004, -0.015,
            0.053, 0.055, 0.052,
        ];
        let frames: Vec<(f64, f64)> = scores
            .iter()
            .enumerate()
            .map(|(i, &s)| (1240.9 + i as f64, s))
            .collect();
        let text = detect_credit_regions(&frames, 1133.0, &EG2).unwrap();
        let (start, end) = text.roll();
        assert!((start - 1260.9).abs() < 1e-6 && (end - 1291.9).abs() < 1e-6);
        let after = text.after_roll.expect("text returns after the roll");
        assert!((after - 1327.9).abs() < 1e-6, "after_roll {after}");
    }

    #[test]
    fn name_cards_and_the_crawl_are_separate_blocks_around_footage() {
        let mut scores = Vec::new();
        block(&mut scores, 2800, 2922, 0.0);
        block(&mut scores, 2922, 3007, 0.3);
        block(&mut scores, 3007, 3135, 0.0);
        block(&mut scores, 3135, 3153, 0.3);
        block(&mut scores, 3153, 3172, 0.0);
        block(&mut scores, 3172, 3311, 0.3);
        let text = regions(&scores, NO_FLOOR).unwrap();
        assert_eq!(text.blocks, vec![(2922.0, 3006.0), (3135.0, 3310.0)]);
    }

    /// Carrie 1x06: name cards, a 20 s run of logo cards vision scores low, then
    /// the crawl — one unbroken roll.
    #[test]
    fn name_cards_split_from_the_crawl_by_logo_cards_are_one_block() {
        let mut scores = Vec::new();
        block(&mut scores, 3200, 3283, 0.0);
        block(&mut scores, 3283, 3404, 0.3);
        block(&mut scores, 3404, 3424, 0.0);
        block(&mut scores, 3424, 3558, 0.3);
        let text = regions(&scores, NO_FLOOR).unwrap();
        assert_eq!(text.blocks, vec![(3283.0, 3557.0)]);
    }

    /// Logo cards stranded between a mid-credits scene and the crawl bridge the
    /// chain back to the name cards but are not credits worth a skip of their own.
    #[test]
    fn a_short_block_bridges_but_is_never_a_block_of_its_own() {
        let mut scores = Vec::new();
        block(&mut scores, 2900, 2990, 0.3);
        block(&mut scores, 2990, 3100, 0.0);
        block(&mut scores, 3100, 3115, 0.3);
        block(&mut scores, 3115, 3160, 0.0);
        block(&mut scores, 3160, 3300, 0.3);
        let text = regions(&scores, NO_FLOOR).unwrap();
        assert_eq!(text.blocks, vec![(2900.0, 2989.0), (3160.0, 3299.0)]);
    }

    #[test]
    fn an_earlier_region_is_not_chained_when_it_is_brief_far_or_overlong() {
        let cases: [(&str, u32, u32, f64); 3] = [
            ("brief on-screen text", 2850, 2865, NO_FLOOR),
            ("a block long before the roll", 2500, 2600, NO_FLOOR),
            ("chaining would start before the floor", 2780, 2860, 2800.0),
        ];
        for (name, from, to, floor) in cases {
            let mut scores = Vec::new();
            block(&mut scores, from, to, 0.3);
            block(&mut scores, 2900, 3100, 0.3);
            let (start, _, _) = flat(regions(&scores, floor)).unwrap();
            assert_eq!(start, 2900.0, "{name}");
        }
    }

    #[test]
    fn test_detect_credit_regions_basic() {
        // Simulate: scene frames (low score) then credits (high score) with a brief gap
        let mut scores: Vec<(f64, f64)> = Vec::new();
        // Scene: 0-2880s at 1fps, all below threshold
        for t in (2700..2880).map(|t| t as f64) {
            scores.push((t, 0.02));
        }
        // Credits start at 2880 (48:00), with a 3s dip at 2895
        for t in (2880..2895).map(|t| t as f64) {
            scores.push((t, 0.25));
        }
        // Brief dip (names transitioning)
        for t in (2895..2898).map(|t| t as f64) {
            scores.push((t, 0.05));
        }
        // Credits resume
        for t in (2898..2940).map(|t| t as f64) {
            scores.push((t, 0.30));
        }

        let result = flat(regions(&scores, NO_FLOOR));
        assert!(result.is_some());
        let (start, end, confidence) = result.unwrap();
        assert!(
            (start - 2880.0).abs() < 0.01,
            "Credits should start at 2880, got {start}"
        );
        assert!(
            (end - 2939.0).abs() < 0.01,
            "Credits should end at 2939, got {end}"
        );
        assert!(
            confidence > 0.3,
            "Confidence should be reasonable, got {confidence}"
        );
    }

    #[test]
    fn test_detect_credit_regions_picks_largest_region() {
        // Early blip (4 hits) and main credits block (20 hits)
        let mut scores: Vec<(f64, f64)> = Vec::new();
        // Early blip at 2800
        for t in (2800..2804).map(|t| t as f64) {
            scores.push((t, 0.15));
        }
        // Gap (scene)
        for t in (2804..2880).map(|t| t as f64) {
            scores.push((t, 0.01));
        }
        // Main credits at 2880
        for t in (2880..2900).map(|t| t as f64) {
            scores.push((t, 0.30));
        }

        let result = flat(regions(&scores, NO_FLOOR));
        assert!(result.is_some());
        let (start, end, _) = result.unwrap();
        assert!(
            (start - 2880.0).abs() < 0.01,
            "Should pick main block at 2880, got {start}"
        );
        assert!((end - 2899.0).abs() < 0.01, "Should end at 2899, got {end}");
    }

    #[test]
    fn test_detect_credit_regions_too_few_hits() {
        // Only 3 hits — below MIN_HITS of 5
        let scores: Vec<(f64, f64)> = vec![
            (2880.0, 0.15),
            (2881.0, 0.20),
            (2882.0, 0.15),
            (2883.0, 0.01),
            (2884.0, 0.01),
        ];
        assert!(flat(regions(&scores, NO_FLOOR)).is_none());
    }

    #[test]
    fn test_detect_credit_regions_no_hits() {
        // All scores below threshold — no credits
        let scores: Vec<(f64, f64)> = (0..100).map(|t| (t as f64, 0.05)).collect();
        assert!(flat(regions(&scores, NO_FLOOR)).is_none());
    }

    #[test]
    fn test_detect_credit_regions_gap_tolerance() {
        // Two groups 8s apart (within 10s gap tolerance) should merge
        let mut scores: Vec<(f64, f64)> = Vec::new();
        for t in (100..106).map(|t| t as f64) {
            scores.push((t, 0.20));
        }
        // 8s gap (within tolerance)
        for t in (114..120).map(|t| t as f64) {
            scores.push((t, 0.20));
        }
        let result = flat(regions(&scores, NO_FLOOR));
        assert!(result.is_some());
        let (start, end, _) = result.unwrap();
        assert!(
            (start - 100.0).abs() < 0.01,
            "Merged region should start at 100, got {start}"
        );
        assert!(
            (end - 119.0).abs() < 0.01,
            "Merged region should end at 119, got {end}"
        );
    }
}
