//! SigLIP2 zero-shot "is this frame an advertisement".
//!
//! The one modality with no cold-start problem: it needs neither a prior airing
//! (chromaprint) nor a teletext stream, so it is what supplies the second
//! independent signal on a recording whose transport stream carried no teletext.

use crate::skip_detect::vision::VisionClient;

/// The caller holds `gpu_lock.acquire_for_vision()` across this — vision is
/// one-at-a-time in the current topology and an ungated call OOMs avatar/flux.
const AD_PROMPTS: [&str; 4] = [
    "a television advertisement",
    "a commercial break",
    "a product advertisement with on-screen pricing",
    "a station identification bumper",
];

const PROGRAMME_PROMPTS: [&str; 4] = [
    "a television programme",
    "a live sports broadcast",
    "a news broadcast",
    "a scene from a television show",
];

#[derive(Debug, Clone)]
pub struct VisionConfig {
    pub ad_threshold: f64,
    pub smoothing_window: usize,
    pub smoothing_needed: usize,
    pub min_run_secs: f64,
}

impl Default for VisionConfig {
    fn default() -> Self {
        Self {
            ad_threshold: 0.6,
            smoothing_window: 7,
            smoothing_needed: 4,
            min_run_secs: 15.0,
        }
    }
}

pub async fn classify_frames(
    client: &VisionClient,
    frames: &[(f64, Vec<u8>)],
) -> Result<Vec<(f64, f64)>, String> {
    if frames.is_empty() {
        return Ok(Vec::new());
    }
    let images: Vec<Vec<u8>> = frames.iter().map(|(_, jpeg)| jpeg.clone()).collect();
    let prompts: Vec<&str> = AD_PROMPTS
        .iter()
        .chain(PROGRAMME_PROMPTS.iter())
        .copied()
        .collect();
    let scores = client.classify_images(&images, &prompts).await?;
    if scores.len() != frames.len() {
        return Err(format!(
            "vision returned {} rows for {} frames",
            scores.len(),
            frames.len()
        ));
    }
    Ok(frames
        .iter()
        .zip(scores)
        .map(|((time, _), row)| (*time, p_ad(&row)))
        .collect())
}

fn p_ad(row: &[f32]) -> f64 {
    let best = |slice: &[f32]| -> f64 {
        slice
            .iter()
            .fold(0.0f64, |acc, v| acc.max(f64::from(*v).max(0.0)))
    };
    let split = AD_PROMPTS.len().min(row.len());
    let ad = best(&row[..split]);
    let programme = best(&row[split..]);
    if ad + programme <= 0.0 {
        return 0.0;
    }
    ad / (ad + programme)
}

pub fn runs_from_scores(scores: &[(f64, f64)], cfg: &VisionConfig) -> Vec<(f64, f64)> {
    if scores.len() < cfg.smoothing_window {
        return Vec::new();
    }
    let raw: Vec<bool> = scores.iter().map(|(_, p)| *p >= cfg.ad_threshold).collect();
    let half = (cfg.smoothing_window / 2) as i64;
    let smoothed: Vec<bool> = (0..raw.len())
        .map(|i| {
            let lo = (i as i64 - half).max(0) as usize;
            let hi = ((i as i64 + half) as usize).min(raw.len() - 1);
            raw[lo..=hi].iter().filter(|v| **v).count() >= cfg.smoothing_needed
        })
        .collect();

    let step = median_step(scores);
    let mut out: Vec<(f64, f64)> = Vec::new();
    let mut start: Option<usize> = None;
    for i in 0..=smoothed.len() {
        let inside = i < smoothed.len() && smoothed[i];
        match (inside, start) {
            (true, None) => start = Some(i),
            (false, Some(s)) => {
                let from = scores[s].0;
                let to = scores[i - 1].0 + step;
                if to - from >= cfg.min_run_secs {
                    out.push((from, to));
                }
                start = None;
            }
            _ => {}
        }
    }
    out
}

fn median_step(scores: &[(f64, f64)]) -> f64 {
    if scores.len() < 2 {
        return 1.0;
    }
    let mut gaps: Vec<f64> = scores.windows(2).map(|w| w[1].0 - w[0].0).collect();
    gaps.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let mid = gaps[gaps.len() / 2];
    if mid > 0.0 { mid } else { 1.0 }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scores(pattern: &[f64], step: f64) -> Vec<(f64, f64)> {
        pattern
            .iter()
            .enumerate()
            .map(|(i, p)| (i as f64 * step, *p))
            .collect()
    }

    #[test]
    fn a_sustained_run_of_ad_frames_becomes_one_span() {
        let mut pattern = vec![0.1; 30];
        pattern.extend(std::iter::repeat_n(0.9, 30));
        pattern.extend(std::iter::repeat_n(0.1, 30));
        let runs = runs_from_scores(&scores(&pattern, 1.0), &VisionConfig::default());
        assert_eq!(runs.len(), 1, "{runs:?}");
        assert!((runs[0].0 - 30.0).abs() <= 3.0, "{runs:?}");
        assert!((runs[0].1 - 60.0).abs() <= 3.0, "{runs:?}");
    }

    #[test]
    fn an_isolated_spike_is_smoothed_away() {
        let mut pattern = vec![0.1; 40];
        pattern[20] = 0.95;
        pattern[21] = 0.95;
        assert!(runs_from_scores(&scores(&pattern, 1.0), &VisionConfig::default()).is_empty());
    }

    #[test]
    fn programme_confidence_beats_ad_confidence() {
        assert!(p_ad(&[0.1, 0.1, 0.1, 0.1, 0.8, 0.2, 0.1, 0.1]) < 0.3);
        assert!(p_ad(&[0.9, 0.2, 0.1, 0.1, 0.1, 0.1, 0.1, 0.1]) > 0.7);
        assert_eq!(p_ad(&[0.0; 8]), 0.0);
    }

    #[test]
    fn too_few_frames_to_smooth_marks_nothing() {
        assert!(runs_from_scores(&scores(&[0.9; 4], 1.0), &VisionConfig::default()).is_empty());
    }
}
