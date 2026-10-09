//! Pure scoring for SigLIP2 zero-shot credit text detection.
//!
//! The vision client handles the network call to the classifier; this module
//! turns the raw per-frame / per-prompt score matrix into a `(start, end,
//! confidence)` region by collapsing prompt scores, thresholding, and grouping
//! gap-tolerant regions.

/// Per-frame credit-score threshold below which a frame is ignored.
pub const CREDIT_TEXT_THRESHOLD: f64 = 0.10;

/// Maximum gap (seconds) between hits to merge them into the same region.
pub const GAP_TOLERANCE: f64 = 10.0;

/// Minimum number of hits required for a region to count as credits.
pub const MIN_HITS: usize = 5;

/// Text prompts for credit detection via zero-shot classification.
/// Tuned against Invincible (white-on-black credits) and Scrubs 2026
/// (small text over active scene). Real credits score 0.15-0.80,
/// scenes score 0.000, credits-over-footage max 0.059.
pub const CREDIT_PROMPTS: &[&str] = &[
    "white text on a black background",
    "end credits of a TV show",
    "names and titles on a dark screen",
    "production company logo",
    "credits text overlaid on a scene",
];

/// Non-credit prompts for comparison (helps calibrate scores).
pub const NON_CREDIT_PROMPTS: &[&str] = &[
    "a scene from a TV show",
    "animated characters",
    "people in a room",
];

/// Score a batch of classification results for credit text.
///
/// `scores[i]` is the per-prompt score row for frame `i`, ordered as
/// `CREDIT_PROMPTS` followed by `NON_CREDIT_PROMPTS`. `frame_times[i]` is the
/// timestamp (seconds into the file) of frame `i`.
///
/// Returns `(credits_start, credits_end, confidence)` for the densest detected
/// region, or `None` if too few frames cleared the threshold.
pub fn score_credit_text(scores: &[Vec<f32>], frame_times: &[f64]) -> Option<(f64, f64, f64)> {
    if scores.len() != frame_times.len() {
        return None;
    }

    let credit_prompt_count = CREDIT_PROMPTS.len();
    let total_prompts = credit_prompt_count + NON_CREDIT_PROMPTS.len();

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
            / NON_CREDIT_PROMPTS.len() as f64;
        let credit_score = (credit_avg - non_credit_avg).max(0.0);
        frame_scores.push((frame_times[i], credit_score));
    }

    detect_credit_regions(&frame_scores)
}

/// Pure region grouping — public so tests can exercise it directly.
/// Returns `(credits_start, credits_end, confidence)` from per-frame scores.
pub fn detect_credit_regions(frame_scores: &[(f64, f64)]) -> Option<(f64, f64, f64)> {
    let hits: Vec<(f64, f64)> = frame_scores
        .iter()
        .filter(|(_, score)| *score >= CREDIT_TEXT_THRESHOLD)
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

    // Pick the region with the most hits — most evidence of sustained credits
    let best = regions.iter().max_by_key(|r| r.2.len())?;
    let avg_score = best.2.iter().sum::<f64>() / best.2.len() as f64;
    let confidence = (avg_score / 0.5).clamp(0.3, 0.95);

    Some((best.0, best.1, confidence))
}

#[cfg(test)]
mod tests {
    use super::*;

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

        let result = detect_credit_regions(&scores);
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

        let result = detect_credit_regions(&scores);
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
        assert!(detect_credit_regions(&scores).is_none());
    }

    #[test]
    fn test_detect_credit_regions_no_hits() {
        // All scores below threshold — no credits
        let scores: Vec<(f64, f64)> = (0..100).map(|t| (t as f64, 0.05)).collect();
        assert!(detect_credit_regions(&scores).is_none());
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
        let result = detect_credit_regions(&scores);
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
