//! Audio waveform peaks generation for seek bar visualization.
//!
//! Extracts raw PCM audio via kob-ffmpeg, then downsamples to min/max amplitude
//! pairs at a configurable rate. The result is a compact binary blob suitable
//! for efficient transfer and canvas rendering.

use std::path::Path;
use tracing::info;

/// Binary peaks format:
/// - Byte 0: version (1)
/// - Byte 1: peaks per second
/// - Remaining bytes: alternating (min_i8, max_i8) pairs
pub async fn generate_peaks(input: &Path, peaks_per_second: u8) -> Result<Vec<u8>, String> {
    let sample_rate: usize = 8000;
    let bucket_size = sample_rate / peaks_per_second as usize;
    if bucket_size == 0 {
        return Err("Invalid peaks_per_second".to_string());
    }

    let input = input.to_owned();
    let pps = peaks_per_second;

    tokio::task::spawn_blocking(move || generate_peaks_sync(&input, pps, sample_rate, bucket_size))
        .await
        .map_err(|e| format!("task join error: {e}"))?
}

fn generate_peaks_sync(
    input: &Path,
    peaks_per_second: u8,
    _sample_rate: usize,
    bucket_size: usize,
) -> Result<Vec<u8>, String> {
    // Header: [version, peaks_per_second]
    let mut result = Vec::with_capacity(2 + 1024);
    result.push(1); // version
    result.push(peaks_per_second);

    let mut min_val = i16::MAX;
    let mut max_val = i16::MIN;
    let mut samples_in_bucket: usize = 0;
    let mut total_samples: usize = 0;

    kob_ffmpeg::audio::extract_pcm_range(input, 8000, 0.0, f64::INFINITY, None, &mut |data| {
        // Process PCM s16le data — 2 bytes per sample
        let mut offset = 0;
        while offset + 1 < data.len() {
            let sample = i16::from_le_bytes([data[offset], data[offset + 1]]);
            offset += 2;
            if sample < min_val {
                min_val = sample;
            }
            if sample > max_val {
                max_val = sample;
            }
            samples_in_bucket += 1;
            total_samples += 1;
            if samples_in_bucket >= bucket_size {
                result.push((min_val as f32 / 256.0).clamp(-128.0, 127.0) as i8 as u8);
                result.push((max_val as f32 / 256.0).clamp(-128.0, 127.0) as i8 as u8);
                min_val = i16::MAX;
                max_val = i16::MIN;
                samples_in_bucket = 0;
            }
        }
        true // continue
    })
    .map_err(|e| format!("FFmpeg PCM extraction failed: {e}"))?;

    // Flush final partial bucket
    if samples_in_bucket > 0 {
        result.push((min_val as f32 / 256.0).clamp(-128.0, 127.0) as i8 as u8);
        result.push((max_val as f32 / 256.0).clamp(-128.0, 127.0) as i8 as u8);
    }

    if total_samples == 0 {
        return Err("No audio data extracted".to_string());
    }

    let num_peaks = (result.len() - 2) / 2;
    info!(
        path = %input.display(),
        peaks = num_peaks,
        bytes = result.len(),
        "Generated audio peaks"
    );

    Ok(result)
}
