//! Post-credits scene detection within a credits region — cache-only.
//!
//! Primary signal: audio hard cuts (sharp RMS drops) within the credits
//! region, read from the cached `audio_peaks.bin`. Secondary signal: SigLIP
//! embedding cluster break for visual confirmation, read from the cached
//! `embeddings.bin` window.

use tracing::debug;

use super::primitives::cluster as cluster_primitive;
use super::{
    BoundaryMatch, Candidate, CandidateSignals, MAX_POST_CREDITS_DURATION,
    MIN_POST_CREDITS_DURATION, SegmentType,
};
use crate::media_cache::readers::EmbeddingsView;

/// Detect post-credits scenes entirely from cached sidecars. RMS drops come
/// from the cached `audio_peaks.bin`, visual confirmation from cached
/// embeddings — no ffmpeg re-probe, no vision HTTP call.
///
/// `audio_peaks_bytes` is the raw sidecar. Header parsing handles both
/// wire-format versions (v1: pps=u8, v2: pps=u16_le).
pub fn detect_post_credits_from_cache(
    audio_peaks_bytes: &[u8],
    credits_start: f64,
    file_duration: f64,
    embeddings: Option<&EmbeddingsView>,
) -> Vec<Candidate> {
    let credits_duration = file_duration - credits_start;
    if credits_duration < MIN_POST_CREDITS_DURATION * 2.0 {
        debug!("Credits region too short for post-credits detection");
        return vec![];
    }

    let Some((audio_peaks_pps, pairs)) = parse_peaks_header(audio_peaks_bytes) else {
        return vec![];
    };
    let pair_count = pairs.len() / 2;
    if pair_count == 0 || audio_peaks_pps == 0 {
        return vec![];
    }

    // Slice to the credits region + compute RMS in 0.5s windows (0.5 s × pps pairs).
    let window_pairs = ((audio_peaks_pps as f64) * 0.5).round().max(1.0) as usize;
    let start_pair = ((credits_start * audio_peaks_pps as f64).floor() as usize).min(pair_count);
    let end_pair = ((file_duration * audio_peaks_pps as f64).ceil() as usize).min(pair_count);
    if end_pair.saturating_sub(start_pair) < window_pairs * 2 {
        debug!("Too few peak pairs in credits region for RMS windows");
        return vec![];
    }

    let mut rms_values: Vec<f64> = Vec::new();
    let mut t = start_pair;
    while t < end_pair {
        let hi = (t + window_pairs).min(end_pair);
        let mut sum_sq = 0.0_f64;
        let mut n = 0usize;
        for p in t..hi {
            // Each pair is (min_i8, max_i8). `AudioPeaksSink` writes both as the
            // same downsampled sample; `audio::peaks` writes true min/max. Use
            // the larger magnitude as the sample amplitude so both formats work.
            let min = pairs[p * 2] as i8 as f64;
            let max = pairs[p * 2 + 1] as i8 as f64;
            let amp = min.abs().max(max.abs());
            sum_sq += amp * amp;
            n += 1;
        }
        if n > 0 {
            rms_values.push((sum_sq / n as f64).sqrt());
        }
        t += window_pairs;
    }

    let hard_cuts = find_hard_cuts(&rms_values, audio_peaks_pps, start_pair, window_pairs, 20.0);
    let hard_cuts = if hard_cuts.is_empty() && credits_duration > 120.0 {
        debug!("Long credits with no 20dB cuts — retrying at 15dB");
        find_hard_cuts(&rms_values, audio_peaks_pps, start_pair, window_pairs, 15.0)
    } else {
        hard_cuts
    };

    if hard_cuts.is_empty() {
        return vec![];
    }

    // Take the LAST cut with enough remaining content, same rules as V1.
    let viable_cut = hard_cuts.iter().rev().find(|&&cut_time| {
        let remaining = file_duration - cut_time;
        let into_credits = cut_time - credits_start;
        remaining >= MIN_POST_CREDITS_DURATION && into_credits < credits_duration * 0.8
    });
    let Some(&cut_time) = viable_cut else {
        return vec![];
    };
    if file_duration - cut_time > MAX_POST_CREDITS_DURATION {
        debug!(
            duration = file_duration - cut_time,
            max = MAX_POST_CREDITS_DURATION,
            "Post-credits scene too long, skipping"
        );
        return vec![];
    }

    let visual_score = embeddings.and_then(|view| {
        // Align a cluster break within the credits region against the cut.
        let window = view.window(credits_start, file_duration);
        if window.len() < 4 {
            return None;
        }
        let break_idx = cluster_primitive::find_cluster_break(&window, 0.5)?;
        let cut_frame_idx = ((cut_time - credits_start) * view.fps).round() as isize;
        let distance = (break_idx as isize - cut_frame_idx).abs();
        if distance <= 2 {
            // Audio cut coincides with the visual scene change → a real,
            // visually distinct post-credits scene. Score in fusion's passing
            // band (has_visual > 0.5, below the >0.9 "same cluster" veto).
            Some(0.6)
        } else {
            // Audio dropped but the visuals didn't change there — most likely
            // the credits music just ending under continuing credits, not a
            // scene. Below has_visual so fusion leans on boundary alone.
            Some(0.3)
        }
    });

    let mut signals = CandidateSignals {
        visual_score,
        ..Default::default()
    };
    signals.boundary_match = Some(BoundaryMatch {
        has_black_frame: false,
        has_silence: true, // RMS drop is effectively silence
        refined_start: cut_time,
        refined_end: file_duration,
    });

    vec![Candidate {
        segment_type: SegmentType::PostCreditsScene,
        start_seconds: cut_time,
        end_seconds: file_duration,
        file_duration: Some(file_duration),
        signals,
    }]
}

/// Parse the cached `audio_peaks.bin` wire-format header.
///
/// Returns `(peaks_per_second, pairs_slice)`. Handles both versions:
/// - v1: `[1, pps:u8, pairs...]` (legacy `kobbler/api/src/audio/peaks.rs`)
/// - v2: `[2, pps:u16_le, pairs...]` (`kob_ffmpeg::single_pass::AudioPeaksSink`)
pub(crate) fn parse_peaks_header(bytes: &[u8]) -> Option<(u32, &[u8])> {
    match bytes.first()? {
        1 => {
            if bytes.len() < 2 {
                return None;
            }
            Some((bytes[1] as u32, &bytes[2..]))
        }
        2 => {
            if bytes.len() < 3 {
                return None;
            }
            let pps = u16::from_le_bytes([bytes[1], bytes[2]]) as u32;
            Some((pps, &bytes[3..]))
        }
        _ => None,
    }
}

fn find_hard_cuts(
    rms_values: &[f64],
    pps: u32,
    start_pair: usize,
    window_pairs: usize,
    drop_threshold_db: f64,
) -> Vec<f64> {
    let mut cuts = Vec::new();
    let window_secs = window_pairs as f64 / pps as f64;
    for i in 1..rms_values.len() {
        if rms_values[i - 1] > 0.0 {
            let ratio_db = 20.0 * (rms_values[i] / rms_values[i - 1]).max(1e-10).log10();
            if ratio_db < -drop_threshold_db {
                // Cut-time = start_of_window(i) = start_pair + i*window_pairs pairs.
                let time = (start_pair + i * window_pairs) as f64 / pps as f64;
                cuts.push(time);
            }
        }
    }
    // Sanity: window_secs unused in the return value but derived for clarity/assertion.
    debug_assert!(window_secs > 0.0);
    cuts
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::media_cache::readers::EmbeddingsView;

    /// Build a synthetic peaks buffer at `pps=1000` (v2 wire format).
    fn build_peaks(segments: &[(usize, i8)]) -> Vec<u8> {
        let mut bytes = vec![2u8];
        bytes.extend_from_slice(&1000u16.to_le_bytes());
        for &(len, amp) in segments {
            for _ in 0..len {
                bytes.push(amp as u8);
                bytes.push(amp as u8);
            }
        }
        bytes
    }

    #[test]
    fn from_cache_finds_sharp_rms_drop() {
        // 300s file, credits region [200, 300] = 100s. Drop at 220s.
        // post_credits = [220, 300] = 80s (< MAX=120), into_credits=20s (< 80% = 80), remaining=80s (> MIN=5).
        let peaks = build_peaks(&[(220_000, 100), (80_000, 5)]);
        let out = detect_post_credits_from_cache(&peaks, 200.0, 300.0, None);
        assert_eq!(out.len(), 1, "got: {:?}", out);
        let c = &out[0];
        assert!(
            (c.start_seconds - 220.0).abs() <= 0.5,
            "expected ~220s, got {}",
            c.start_seconds
        );
        assert_eq!(c.end_seconds, 300.0);
        assert!(c.signals.boundary_match.as_ref().unwrap().has_silence);
        assert!(c.signals.visual_score.is_none());
    }

    #[test]
    fn aligned_visual_break_scores_in_passing_band() {
        // A "perfect" post-credits scene: a sharp audio cut at 270s that lands
        // on the same frame as a visual scene change inside the credits region.
        // That aligned audio+visual evidence is the STRONGEST signal we have, so
        // it must score in fusion's passing band — has_visual (> 0.5) and not the
        // "too similar to credits" veto (> 0.9) — or the scene gets dropped.
        let peaks = build_peaks(&[(270_000, 100), (30_000, 5)]);

        // 1 fps embeddings, dim 2. Frames [0,270) match the credits cluster
        // ([1,0]); from 270s the scene is visually distinct ([0,1]), so the
        // cluster break inside the [200,300) window lands on the audio cut.
        let dim = 2;
        let count = 300;
        let mut rows = Vec::with_capacity(count * dim);
        for i in 0..count {
            if i < 270 {
                rows.extend_from_slice(&[1.0, 0.0]);
            } else {
                rows.extend_from_slice(&[0.0, 1.0]);
            }
        }
        let view = EmbeddingsView {
            rows,
            count,
            dim,
            fps: 1.0,
            start_time_secs: 0.0,
        };

        let out = detect_post_credits_from_cache(&peaks, 200.0, 300.0, Some(&view));
        assert_eq!(out.len(), 1, "got: {:?}", out);
        let vs = out[0]
            .signals
            .visual_score
            .expect("visual_score is set when embeddings are present");
        assert!(
            vs > 0.5 && vs <= 0.9,
            "aligned audio+visual cut must land in fusion's passing band (0.5, 0.9]; got {vs}"
        );
    }

    #[test]
    fn from_cache_no_drop_returns_empty() {
        // Flat amplitude all the way through.
        let peaks = build_peaks(&[(300_000, 80)]);
        let out = detect_post_credits_from_cache(&peaks, 0.0, 300.0, None);
        assert!(out.is_empty());
    }

    #[test]
    fn from_cache_short_region_returns_empty() {
        // credits_duration < 2 × MIN_POST_CREDITS_DURATION.
        let peaks = build_peaks(&[(5_000, 100)]);
        let out = detect_post_credits_from_cache(&peaks, 0.0, 5.0, None);
        assert!(out.is_empty());
    }

    #[test]
    fn from_cache_rejects_cut_too_late_in_credits() {
        // Drop right at the very end (98% into credits) — filter says into_credits
        // must be < 0.8 × credits_duration, so this should be rejected.
        let peaks = build_peaks(&[(294_000, 100), (6_000, 5)]);
        let out = detect_post_credits_from_cache(&peaks, 0.0, 300.0, None);
        assert!(out.is_empty(), "got cuts: {:?}", out);
    }

    #[test]
    fn parse_peaks_header_handles_v1_and_v2() {
        let v1: Vec<u8> = vec![1, 200, 0, 0, 0, 0];
        let v2: Vec<u8> = {
            let mut b = vec![2u8];
            b.extend_from_slice(&1000u16.to_le_bytes());
            b.extend_from_slice(&[0, 0, 0, 0]);
            b
        };
        let (pps_v1, pairs_v1) = parse_peaks_header(&v1).unwrap();
        let (pps_v2, pairs_v2) = parse_peaks_header(&v2).unwrap();
        assert_eq!(pps_v1, 200);
        assert_eq!(pairs_v1.len(), 4);
        assert_eq!(pps_v2, 1000);
        assert_eq!(pairs_v2.len(), 4);
        assert!(parse_peaks_header(&[]).is_none());
        assert!(parse_peaks_header(&[3, 0, 0]).is_none()); // unknown version
    }
}
