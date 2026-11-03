//! Zero-shot "is this frame an advertisement", by cosine against ad and
//! programme prompts.
//!
//! The one modality with no cold-start problem: it needs neither a prior airing
//! (chromaprint) nor a teletext stream, so it is what supplies the second
//! independent signal on a recording whose transport stream carried no teletext.

use crate::embedding::EmbeddingClient;
use crate::embedding::profile::{self, ModelProfile};

#[derive(Debug, Clone)]
pub struct VisionConfig {
    /// Per-frame ad score (see [`ad_score`]) at or above which a frame is an ad.
    pub ad_threshold: f64,
    pub smoothing_window: usize,
    pub smoothing_needed: usize,
    pub min_run_secs: f64,
}

impl Default for VisionConfig {
    fn default() -> Self {
        Self {
            ad_threshold: profile::active().ad_margin,
            smoothing_window: 7,
            smoothing_needed: 4,
            min_run_secs: 15.0,
        }
    }
}

/// The caller holds `gpu_lock.acquire_for_vision()` across this — the image
/// encode is gated, and an ungated call can OOM avatar/flux.
pub async fn classify_frames(
    client: &EmbeddingClient,
    frames: &[(f64, Vec<u8>)],
) -> Result<Vec<(f64, f64)>, String> {
    if frames.is_empty() {
        return Ok(Vec::new());
    }
    let profile = profile::active();
    let images: Vec<Vec<u8>> = frames.iter().map(|(_, jpeg)| jpeg.clone()).collect();
    let prompts: Vec<&str> = profile
        .ad_prompts
        .iter()
        .chain(profile.programme_prompts)
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
        .map(|((time, _), row)| (*time, ad_score(&row, profile)))
        .collect())
}

/// Best ad-prompt cosine minus best programme-prompt cosine. Every cosine sits
/// in a narrow positive band, so the margin between the two sides is what
/// separates them; a ratio would hover near 0.5 for every frame.
fn ad_score(row: &[f32], profile: &ModelProfile) -> f64 {
    let best = |slice: &[f32]| -> f64 {
        slice
            .iter()
            .fold(f64::NEG_INFINITY, |acc, v| acc.max(f64::from(*v)))
    };
    let split = profile.ad_prompts.len().min(row.len());
    let (ad, programme) = row.split_at(split);
    if ad.is_empty() || programme.is_empty() {
        return 0.0;
    }
    best(ad) - best(programme)
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

    /// Smoothing and run-building are scale-free; written on a 0.5 threshold.
    fn cfg() -> VisionConfig {
        VisionConfig {
            ad_threshold: 0.5,
            ..VisionConfig::default()
        }
    }

    #[test]
    fn a_sustained_run_of_ad_frames_becomes_one_span() {
        let mut pattern = vec![0.1; 30];
        pattern.extend(std::iter::repeat_n(0.9, 30));
        pattern.extend(std::iter::repeat_n(0.1, 30));
        let runs = runs_from_scores(&scores(&pattern, 1.0), &cfg());
        assert_eq!(runs.len(), 1, "{runs:?}");
        assert!((runs[0].0 - 30.0).abs() <= 3.0, "{runs:?}");
        assert!((runs[0].1 - 60.0).abs() <= 3.0, "{runs:?}");
    }

    #[test]
    fn an_isolated_spike_is_smoothed_away() {
        let mut pattern = vec![0.1; 40];
        pattern[20] = 0.95;
        pattern[21] = 0.95;
        assert!(runs_from_scores(&scores(&pattern, 1.0), &cfg()).is_empty());
    }

    #[test]
    fn the_ad_score_is_the_margin_between_the_best_of_each_side() {
        let p = profile::active();
        let mut row = vec![0.20_f32; p.ad_prompts.len() + p.programme_prompts.len()];
        row[p.ad_prompts.len()] = 0.30;
        assert!(ad_score(&row, p) < 0.0, "programme wins");
        row[0] = 0.36;
        assert!((ad_score(&row, p) - 0.06).abs() < 1e-6);
        assert!(ad_score(&row, p) >= VisionConfig::default().ad_threshold);
        assert_eq!(
            ad_score(
                &[0.3, 0.2],
                &ModelProfile {
                    ad_prompts: &[],
                    ..*p
                }
            ),
            0.0
        );
    }

    #[test]
    fn too_few_frames_to_smooth_marks_nothing() {
        assert!(runs_from_scores(&scores(&[0.9; 4], 1.0), &cfg()).is_empty());
    }
}
