//! Video fingerprinting (vHash) for skip detection.
//!
//! Replaces dHash with a chromaprint-inspired approach: extract 16x16 grayscale
//! frame descriptors, discover alignment shifts via quantized hash inverted index,
//! then validate with continuous cosine similarity. Handles dynamic montage intros
//! (slowly panning content) that dHash's binary pixel-matching misses.

use std::path::Path;

pub use super::primitives::vhash_align::{
    FrameDescriptor, VHashMatch, VHashPairMatch, find_visual_matches, frame_similarity,
    quantize_descriptor,
};

use super::primitives::vhash_align::VHASH_FPS;

/// Size of the grayscale descriptor grid.
const DESCRIPTOR_SIZE: usize = 16;

/// Serialize descriptors to bytes for DB storage.
/// Format: [descriptor_size: u16][frame_count: u32][per frame: timestamp f64 LE + values]
pub fn serialize_descriptors(descs: &[FrameDescriptor]) -> Vec<u8> {
    let ds = DESCRIPTOR_SIZE as u16;
    let vals_len = DESCRIPTOR_SIZE * DESCRIPTOR_SIZE;
    let mut buf = Vec::with_capacity(6 + descs.len() * (8 + vals_len));
    buf.extend_from_slice(&ds.to_le_bytes());
    buf.extend_from_slice(&(descs.len() as u32).to_le_bytes());
    for d in descs {
        buf.extend_from_slice(&d.timestamp.to_le_bytes());
        let padded: Vec<u8> = d
            .values
            .iter()
            .copied()
            .chain(std::iter::repeat(0))
            .take(vals_len)
            .collect();
        buf.extend_from_slice(&padded);
    }
    buf
}

/// Deserialize descriptors from bytes.
pub fn deserialize_descriptors(data: &[u8]) -> Option<Vec<FrameDescriptor>> {
    if data.len() < 6 {
        return None;
    }
    let ds = u16::from_le_bytes([data[0], data[1]]) as usize;
    let count = u32::from_le_bytes([data[2], data[3], data[4], data[5]]) as usize;
    let vals_len = ds * ds;
    let frame_size = 8 + vals_len;
    if data.len() < 6 + count * frame_size {
        return None;
    }

    let mut descs = Vec::with_capacity(count);
    for i in 0..count {
        let offset = 6 + i * frame_size;
        let timestamp = f64::from_le_bytes(data[offset..offset + 8].try_into().ok()?);
        let values = data[offset + 8..offset + 8 + vals_len].to_vec();
        let quantized = quantize_descriptor(&values);
        descs.push(FrameDescriptor {
            timestamp,
            values,
            quantized,
        });
    }
    Some(descs)
}

/// Extract frame descriptors from a video region.
///
/// Uses sequential decode (single file open + seek) for ~10-20x faster
/// extraction compared to per-frame random access.
pub async fn extract_descriptors(
    file_path: &Path,
    start_secs: f64,
    length_secs: f64,
) -> Result<Vec<FrameDescriptor>, String> {
    let path = file_path.to_owned();

    tokio::task::spawn_blocking(move || {
        // Sequential decode at 64px with skip_loop_filter, then Lanczos 16x16 via image crate.
        // The two-stage downscale (FFmpeg bilinear→64px, image Lanczos3→16x16) produces
        // the most discriminative descriptors.
        //
        // Sequential (not seek) because vhash extracts at 3fps from short regions —
        // dense enough that sequential decode is faster than per-frame seeking.
        // CPU-only because CUDA starves on NAS I/O.
        let frames = kob_ffmpeg::thumbnails::extract_frames_sequential(
            &path,
            start_secs,
            length_secs,
            VHASH_FPS,
            Some(64),
            3,
            false,
        )
        .map_err(|e| format!("frame extraction failed: {e}"))?;

        let mut descriptors = Vec::with_capacity(frames.len());
        for jpeg_frame in frames {
            let img = image::load_from_memory(&jpeg_frame.jpeg_data).map_err(|e| {
                format!(
                    "image decode failed at {:.1}s: {e}",
                    jpeg_frame.timestamp_secs
                )
            })?;
            let ds = DESCRIPTOR_SIZE as u32;
            let gray = image::imageops::grayscale(&img.resize_exact(
                ds,
                ds,
                image::imageops::FilterType::Lanczos3,
            ));

            let values: Vec<u8> = gray.pixels().map(|p| p.0[0]).collect();

            let quantized = quantize_descriptor(&values);
            descriptors.push(FrameDescriptor {
                timestamp: jpeg_frame.timestamp_secs,
                values,
                quantized,
            });
        }

        Ok(descriptors)
    })
    .await
    .map_err(|e| format!("task join error: {e}"))?
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
    fn similarity_identical_frames() {
        // Needs non-constant values — Pearson of constant vector is 0/0
        let mut values = [0u8; 256];
        for (i, v) in values.iter_mut().enumerate() {
            *v = (i * 37 % 256) as u8;
        }
        let sim = frame_similarity(&values, &values);
        assert!((sim - 1.0).abs() < 0.001, "identical = {sim}");
    }

    #[test]
    fn similarity_opposite_frames() {
        // Use gradient patterns — uniform vectors would be parallel (cosine = 1.0)
        let mut a = [0u8; 256];
        let mut b = [0u8; 256];
        for i in 0..256 {
            a[i] = i as u8;
            b[i] = 255 - i as u8;
        }
        let sim = frame_similarity(&a, &b);
        assert!(sim < 0.5, "anti-correlated = {sim}");
    }

    #[test]
    fn similarity_slightly_shifted() {
        // Same gradient pattern, shifted by 1 pixel
        let mut a = [0u8; 256];
        let mut b = [0u8; 256];
        for i in 0..256 {
            a[i] = (i % 128) as u8 * 2;
        }
        for i in 0..256 {
            // Shift by 1 position (wrapping)
            b[i] = a[(i + 1) % 256];
        }
        let sim = frame_similarity(&a, &b);
        assert!(sim > 0.85, "shifted 1px should be very similar: {sim}");
    }

    #[test]
    fn quantize_produces_nonzero() {
        let mut values = [0u8; 256];
        for i in 0..256 {
            values[i] = (i * 37 % 256) as u8;
        }
        let q = quantize_descriptor(&values);
        assert!(q != 0, "quantized hash should be nonzero for varied input");
    }

    #[test]
    fn alignment_offset_sequences() {
        let make_frame = |i: usize| -> [u8; 256] {
            let mut values = [0u8; 256];
            for j in 0..256 {
                values[j] = ((i * 37 + j) % 256) as u8;
            }
            values
        };

        let seq_a: Vec<FrameDescriptor> = (0..30)
            .map(|i| make_descriptor(i as f64 / 3.0, &make_frame(i)))
            .collect();

        // seq_b has the same frames but offset by 5 frames (shift = 5)
        let seq_b: Vec<FrameDescriptor> = (0..30)
            .map(|i| make_descriptor(i as f64 / 3.0 + 5.0, &make_frame(i)))
            .collect();

        // The shift should be detected
        let result = find_visual_matches(&seq_a, &seq_b, 3.0, 0.70);
        assert!(result.is_some(), "offset sequences should match");
        let m = result.unwrap();
        assert!(m.lhs.end_seconds - m.lhs.start_seconds >= 3.0);
    }

    #[test]
    fn no_match_for_different_content() {
        let seq_a: Vec<FrameDescriptor> = (0..30)
            .map(|i| {
                let mut values = [0u8; 256];
                for j in 0..256 {
                    values[j] = ((i * 37 + j) % 256) as u8;
                }
                make_descriptor(i as f64 / 3.0, &values)
            })
            .collect();

        let seq_b: Vec<FrameDescriptor> = (0..30)
            .map(|i| {
                let mut values = [0u8; 256];
                for j in 0..256 {
                    values[j] = ((i * 71 + j * 3 + 128) % 256) as u8;
                }
                make_descriptor(i as f64 / 3.0, &values)
            })
            .collect();

        let result = find_visual_matches(&seq_a, &seq_b, 3.0, 0.70);
        assert!(result.is_none(), "different content shouldn't match");
    }

    /// Regression test for the 2026-04-21 Leftovers S3 silent-zero-intros bug.
    ///
    /// Fixture: 180s intro-region vHash descriptors captured from
    /// The Leftovers S3E07 and S3E08 (show with same-visual different-audio
    /// intros — the audio chromaprint pipeline can't see them, so intro
    /// detection depends entirely on the vhash visual path).
    ///
    /// Bugs exercised:
    ///   1. Frame rate halving in the koblibs sequential extractor — caught
    ///      by asserting extracted sequences are dense enough (~3fps).
    ///   2. Pearson similarity returning 0 for constant-vector pairs —
    ///      caught by the density floor being near 1.0.
    ///   3. Phase-1 selection scoring density*duration without a density
    ///      gate — caught because a long-sparse-shift competes with the
    ///      real dense shift and wins under the old code.
    #[test]
    fn leftovers_s3_same_visual_intros_match() {
        let e07 = include_bytes!("../../tests/fixtures/vhash/leftovers_s3e07_intro.bin");
        let e08 = include_bytes!("../../tests/fixtures/vhash/leftovers_s3e08_intro.bin");
        let d07 = deserialize_descriptors(e07).expect("decode e07 fixture");
        let d08 = deserialize_descriptors(e08).expect("decode e08 fixture");

        // Descriptor density: 180s window at 3fps should be ~540 frames.
        // Half of that would mean the koblibs fps-halving regression returned.
        assert!(
            d07.len() >= 500,
            "e07 descriptor count too low: {} — koblibs fps regression?",
            d07.len()
        );
        assert!(
            d08.len() >= 500,
            "e08 descriptor count too low: {}",
            d08.len()
        );

        // Match at production MIN_MATCH_DENSITY threshold.
        let pair = find_visual_matches(&d07, &d08, 15.0, 0.70)
            .expect("Leftovers intros must match at 0.70 threshold");
        assert!(
            pair.lhs.density >= 0.90,
            "density too low: {}",
            pair.lhs.density
        );
        let duration = pair.lhs.end_seconds - pair.lhs.start_seconds;
        assert!(duration >= 60.0, "matched duration too short: {duration}");
    }
}
