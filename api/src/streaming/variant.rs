//! Quality variant management for ABR streaming.
//!
//! Computes available quality variants based on source resolution.
//! Only used when the playback method is `VideoTranscode`.

use crate::ffmpeg::transcode::TranscodeProcess;
use crate::streaming::segment_cache::SegmentCache;

/// A live quality variant in an ABR session.
///
/// Each variant has its own FFmpeg process and segment cache.
/// Variants are lazy-started: FFmpeg only runs when hls.js
/// first requests that variant's stream.m3u8.
pub struct QualityVariant {
    pub spec: VariantSpec,
    pub transcode: Option<TranscodeProcess>,
    pub segment_cache: SegmentCache,
    /// Runs a restored session found on disk, handed to the variant's first transcode.
    pub prior_runs: Vec<kob_ffmpeg::session::PriorRun>,
}

/// Specification for a quality variant (computed before session creation).
#[derive(Debug, Clone)]
pub struct VariantSpec {
    /// Variant identifier (e.g., "720p", "480p").
    pub id: String,
    /// Output width.
    pub width: u32,
    /// Output height.
    pub height: u32,
    /// Estimated bandwidth in bits/sec for `#EXT-X-STREAM-INF`.
    pub bandwidth: u32,
    /// Target video bitrate in kbps. `None` = CRF mode (highest quality variant).
    pub target_bitrate_kbps: Option<u32>,
}

/// Compute which quality variants to offer based on source resolution.
///
/// Returns specs for the lower-quality variants only. The primary/high
/// variant is the session's main transcode (not in this list).
pub fn compute_variants(src_w: u32, src_h: u32) -> Vec<VariantSpec> {
    let mut variants = vec![];

    if src_h >= 1080 {
        let (w, h) = scale_to_height(src_w, src_h, 720);
        variants.push(VariantSpec {
            id: "720p".to_string(),
            width: w,
            height: h,
            bandwidth: estimate_bandwidth(Some(2500)),
            target_bitrate_kbps: Some(2500),
        });
    }

    if src_h >= 720 {
        let (w, h) = scale_to_height(src_w, src_h, 480);
        variants.push(VariantSpec {
            id: "480p".to_string(),
            width: w,
            height: h,
            bandwidth: estimate_bandwidth(Some(1000)),
            target_bitrate_kbps: Some(1000),
        });
    }

    variants
}

/// Compute the primary variant spec (native resolution, CRF mode).
pub fn primary_variant_spec(src_w: u32, src_h: u32) -> VariantSpec {
    VariantSpec {
        id: "high".to_string(),
        width: src_w,
        height: src_h,
        bandwidth: estimate_bandwidth_for_resolution(src_w, src_h),
        target_bitrate_kbps: None,
    }
}

/// Scale to a target height while maintaining aspect ratio.
/// Both dimensions are rounded to even (required by x264/VAAPI encoders).
pub fn scale_to_height(src_w: u32, src_h: u32, target_h: u32) -> (u32, u32) {
    let ratio = src_w as f64 / src_h as f64;
    let w = (ratio * target_h as f64).round() as u32;
    // Round both to even
    (w & !1, target_h & !1)
}

/// Estimate HLS bandwidth (bits/sec) for a given target bitrate.
pub fn estimate_bandwidth(bitrate_kbps: Option<u32>) -> u32 {
    let video_kbps = bitrate_kbps.unwrap_or(5000);
    (video_kbps + 192) * 1000 // video + audio overhead
}

/// Filter variants to only include those at or below the given bitrate cap.
pub fn filter_by_max_bitrate(variants: Vec<VariantSpec>, max_kbps: u32) -> Vec<VariantSpec> {
    variants
        .into_iter()
        .filter(|v| v.target_bitrate_kbps.is_some_and(|br| br <= max_kbps))
        .collect()
}

/// Estimate bandwidth for CRF-encoded content at a given resolution.
fn estimate_bandwidth_for_resolution(width: u32, height: u32) -> u32 {
    let pixels = width * height;
    let video_kbps = if pixels >= 3840 * 2160 {
        15000
    } else if pixels >= 1920 * 1080 {
        5000
    } else if pixels >= 1280 * 720 {
        3000
    } else {
        1500
    };
    (video_kbps + 192) * 1000
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_compute_variants_4k() {
        let variants = compute_variants(3840, 2160);
        assert_eq!(variants.len(), 2);
        assert_eq!(variants[0].id, "720p");
        assert_eq!(variants[1].id, "480p");
    }

    #[test]
    fn test_compute_variants_1080p() {
        let variants = compute_variants(1920, 1080);
        assert_eq!(variants.len(), 2);
        assert_eq!(variants[0].id, "720p");
        assert_eq!(variants[1].id, "480p");
    }

    #[test]
    fn test_compute_variants_720p() {
        let variants = compute_variants(1280, 720);
        assert_eq!(variants.len(), 1);
        assert_eq!(variants[0].id, "480p");
    }

    #[test]
    fn test_compute_variants_480p_no_abr() {
        let variants = compute_variants(854, 480);
        assert!(variants.is_empty());
    }

    #[test]
    fn test_scale_preserves_aspect_ratio_16_9() {
        let (w, h) = scale_to_height(1920, 1080, 720);
        assert_eq!(w, 1280);
        assert_eq!(h, 720);
    }

    #[test]
    fn test_scale_preserves_aspect_ratio_21_9() {
        let (w, h) = scale_to_height(2560, 1080, 720);
        // 2560/1080 * 720 ≈ 1706.67 → 1706, rounded to even
        assert_eq!(w, 1706);
        assert_eq!(h, 720);
    }

    #[test]
    fn test_scale_rounds_to_even() {
        // Odd source dimensions
        let (w, h) = scale_to_height(1921, 1081, 720);
        assert_eq!(w % 2, 0);
        assert_eq!(h % 2, 0);
    }

    #[test]
    fn test_scale_4k_to_480() {
        let (w, h) = scale_to_height(3840, 2160, 480);
        // 3840/2160 * 480 = 853.33 → rounds to 853 → even-rounded to 852
        assert_eq!(w, 852);
        assert_eq!(h, 480);
    }

    #[test]
    fn test_primary_variant_spec() {
        let spec = primary_variant_spec(1920, 1080);
        assert_eq!(spec.id, "high");
        assert_eq!(spec.width, 1920);
        assert_eq!(spec.height, 1080);
        assert!(spec.target_bitrate_kbps.is_none());
    }

    #[test]
    fn test_estimate_bandwidth() {
        // 2500 kbps video + 192 kbps audio = 2692 kbps = 2692000 bps
        assert_eq!(estimate_bandwidth(Some(2500)), 2_692_000);
    }

    #[test]
    fn test_filter_by_max_bitrate_keeps_lower() {
        let variants = compute_variants(1920, 1080);
        // Cap at 2000 kbps: 720p@2500 excluded, 480p@1000 kept
        let filtered = filter_by_max_bitrate(variants, 2000);
        assert_eq!(filtered.len(), 1);
        assert_eq!(filtered[0].id, "480p");
    }

    #[test]
    fn test_filter_by_max_bitrate_keeps_all_when_high() {
        let variants = compute_variants(1920, 1080);
        let filtered = filter_by_max_bitrate(variants, 5000);
        assert_eq!(filtered.len(), 2);
    }

    #[test]
    fn test_filter_by_max_bitrate_excludes_all_when_low() {
        let variants = compute_variants(1920, 1080);
        let filtered = filter_by_max_bitrate(variants, 500);
        assert!(filtered.is_empty());
    }
}
