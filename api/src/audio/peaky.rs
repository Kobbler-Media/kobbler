//! Auto-pick a "peaky" window inside an audio file for voice-clone reference clips.
//!
//! Two paths share the same scoring rule (loudness × non-silence):
//! - Fast path reads the precomputed `audio_peaks/{file_id}.peaks` blob from
//!   `enrich_audiobook` — i8 (min, max) pairs at a known rate, no ffmpeg.
//! - Fallback decodes mono PCM at 8 kHz via ffmpeg when the peaks file is
//!   missing (e.g. enrichment hasn't run yet).

use std::path::Path;
use tracing::info;

const SAMPLE_RATE: i32 = 8_000;
const STEP_SECONDS: f64 = 5.0;
/// Roughly −40 dBFS — anything quieter than this counts as "silent" sample.
const SILENCE_THRESHOLD: i32 = 327;
/// Same threshold scaled to the i8 amplitude domain stored in peaks blobs.
const SILENCE_THRESHOLD_I8: i32 = 2;
/// Reject windows where more than this fraction of samples are silent.
const MAX_SILENCE_RATIO: f64 = 0.3;
/// Peaks blob format constants — see `audio::peaks::generate_peaks`.
const PEAKS_HEADER_LEN: usize = 2;
const PEAKS_VERSION: u8 = 1;

#[derive(Debug, Clone, Copy)]
pub struct PeakyResult {
    pub offset_seconds: f64,
    pub duration_seconds: f64,
    pub total_seconds: f64,
}

/// Pick the best window using the precomputed peaks file if present, falling
/// back to a PCM scan of `audio_path` when not.
pub async fn pick_peaky_window_with_peaks(
    audio_path: &Path,
    peaks_path: Option<&Path>,
    duration_seconds: f64,
) -> Result<PeakyResult, String> {
    if duration_seconds <= 0.0 {
        return Err("duration_seconds must be > 0".to_string());
    }
    if let Some(p) = peaks_path
        && let Ok(bytes) = tokio::fs::read(p).await
        && let Some(res) = pick_peaky_window_from_peaks(&bytes, duration_seconds)
    {
        info!(
            path = %p.display(),
            offset = res.offset_seconds,
            duration = res.duration_seconds,
            total = res.total_seconds,
            "Picked peaky window from cached peaks"
        );
        return Ok(res);
    }
    pick_peaky_window(audio_path, duration_seconds).await
}

pub async fn pick_peaky_window(input: &Path, duration_seconds: f64) -> Result<PeakyResult, String> {
    if duration_seconds <= 0.0 {
        return Err("duration_seconds must be > 0".to_string());
    }
    let input = input.to_owned();
    tokio::task::spawn_blocking(move || pick_peaky_window_sync(&input, duration_seconds))
        .await
        .map_err(|e| format!("task join error: {e}"))?
}

/// Slide a window over the (i8 min, i8 max) peaks blob and score by mean
/// peak-to-peak amplitude × (1 − silence_ratio). Returns `None` for malformed
/// blobs or windows shorter than the requested duration so the caller can
/// fall back to the PCM path.
fn pick_peaky_window_from_peaks(bytes: &[u8], duration_seconds: f64) -> Option<PeakyResult> {
    if bytes.len() <= PEAKS_HEADER_LEN || bytes[0] != PEAKS_VERSION {
        return None;
    }
    let pps = bytes[1] as f64;
    if pps <= 0.0 {
        return None;
    }
    let payload = &bytes[PEAKS_HEADER_LEN..];
    let n_buckets = payload.len() / 2;
    if n_buckets == 0 {
        return None;
    }
    let total_seconds = n_buckets as f64 / pps;
    if total_seconds <= duration_seconds {
        return Some(PeakyResult {
            offset_seconds: 0.0,
            duration_seconds: total_seconds,
            total_seconds,
        });
    }

    let window = (duration_seconds * pps) as usize;
    let step = (STEP_SECONDS * pps).max(1.0) as usize;
    if window == 0 {
        return None;
    }

    let mut best_offset = 0.0;
    let mut best_score = -1.0;
    let mut fallback_offset = 0.0;
    let mut fallback_amp = -1.0;

    let mut start = 0;
    while start + window <= n_buckets {
        let mut sum_amp: i64 = 0;
        let mut silent: usize = 0;
        for i in start..start + window {
            let min = payload[i * 2] as i8 as i32;
            let max = payload[i * 2 + 1] as i8 as i32;
            sum_amp += (max - min) as i64;
            if min.abs() <= SILENCE_THRESHOLD_I8 && max.abs() <= SILENCE_THRESHOLD_I8 {
                silent += 1;
            }
        }
        let mean_amp = sum_amp as f64 / window as f64;
        let silence_ratio = silent as f64 / window as f64;
        let offset_secs = start as f64 / pps;

        if mean_amp > fallback_amp {
            fallback_amp = mean_amp;
            fallback_offset = offset_secs;
        }
        if silence_ratio <= MAX_SILENCE_RATIO {
            let score = mean_amp * (1.0 - silence_ratio);
            if score > best_score {
                best_score = score;
                best_offset = offset_secs;
            }
        }

        start += step;
    }

    let chosen = if best_score < 0.0 {
        fallback_offset
    } else {
        best_offset
    };
    Some(PeakyResult {
        offset_seconds: chosen,
        duration_seconds,
        total_seconds,
    })
}

fn pick_peaky_window_sync(input: &Path, duration_seconds: f64) -> Result<PeakyResult, String> {
    let mut samples: Vec<i16> = Vec::new();
    kob_ffmpeg::audio::extract_pcm_range(
        input,
        SAMPLE_RATE,
        0.0,
        f64::INFINITY,
        None,
        &mut |data| {
            let mut offset = 0;
            while offset + 1 < data.len() {
                samples.push(i16::from_le_bytes([data[offset], data[offset + 1]]));
                offset += 2;
            }
            true
        },
    )
    .map_err(|e| format!("ffmpeg PCM extraction failed: {e}"))?;

    if samples.is_empty() {
        return Err("no audio data extracted".to_string());
    }

    let sr = SAMPLE_RATE as f64;
    let total_seconds = samples.len() as f64 / sr;

    if total_seconds <= duration_seconds {
        return Ok(PeakyResult {
            offset_seconds: 0.0,
            duration_seconds: total_seconds,
            total_seconds,
        });
    }

    let window_len = (duration_seconds * sr) as usize;
    let step_len = (STEP_SECONDS * sr) as usize;

    let mut best_offset = 0.0;
    let mut best_score = -1.0;
    let mut fallback_offset = 0.0;
    let mut fallback_rms = -1.0;

    let mut start = 0;
    while start + window_len <= samples.len() {
        let window = &samples[start..start + window_len];

        let mut sum_sq: f64 = 0.0;
        let mut silent: usize = 0;
        for &s in window {
            let s_f = s as f64;
            sum_sq += s_f * s_f;
            if (s as i32).abs() <= SILENCE_THRESHOLD {
                silent += 1;
            }
        }
        let rms = (sum_sq / window.len() as f64).sqrt();
        let silence_ratio = silent as f64 / window.len() as f64;
        let offset_secs = start as f64 / sr;

        if rms > fallback_rms {
            fallback_rms = rms;
            fallback_offset = offset_secs;
        }

        if silence_ratio <= MAX_SILENCE_RATIO {
            let score = rms * (1.0 - silence_ratio);
            if score > best_score {
                best_score = score;
                best_offset = offset_secs;
            }
        }

        start += step_len;
    }

    let chosen = if best_score < 0.0 {
        fallback_offset
    } else {
        best_offset
    };

    info!(
        path = %input.display(),
        offset = chosen,
        duration = duration_seconds,
        total = total_seconds,
        "Picked peaky window"
    );

    Ok(PeakyResult {
        offset_seconds: chosen,
        duration_seconds,
        total_seconds,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    /// Write a 16-bit PCM mono WAV file. Each sample is i16 little-endian.
    fn write_wav(path: &Path, sample_rate: u32, samples: &[i16]) {
        let bytes_per_sample = 2u32;
        let data_bytes = samples.len() as u32 * bytes_per_sample;
        let mut f = std::fs::File::create(path).unwrap();
        f.write_all(b"RIFF").unwrap();
        f.write_all(&(36u32 + data_bytes).to_le_bytes()).unwrap();
        f.write_all(b"WAVE").unwrap();
        f.write_all(b"fmt ").unwrap();
        f.write_all(&16u32.to_le_bytes()).unwrap(); // PCM fmt chunk size
        f.write_all(&1u16.to_le_bytes()).unwrap(); // PCM format
        f.write_all(&1u16.to_le_bytes()).unwrap(); // mono
        f.write_all(&sample_rate.to_le_bytes()).unwrap();
        f.write_all(&(sample_rate * bytes_per_sample).to_le_bytes())
            .unwrap(); // byte rate
        f.write_all(&(bytes_per_sample as u16).to_le_bytes())
            .unwrap(); // block align
        f.write_all(&16u16.to_le_bytes()).unwrap(); // bits per sample
        f.write_all(b"data").unwrap();
        f.write_all(&data_bytes.to_le_bytes()).unwrap();
        for &s in samples {
            f.write_all(&s.to_le_bytes()).unwrap();
        }
    }

    /// 60s of silence with a 5s loud sine tone centred at t=30s.
    fn synth_silence_with_tone(sample_rate: u32) -> Vec<i16> {
        let total = sample_rate as usize * 60;
        let mut samples = vec![0i16; total];
        let tone_start = sample_rate as usize * 28; // 28s..33s
        let tone_end = sample_rate as usize * 33;
        let freq = 440.0;
        let amp = 20_000.0; // loud
        for (i, sample) in samples
            .iter_mut()
            .enumerate()
            .take(tone_end)
            .skip(tone_start)
        {
            let t = i as f64 / sample_rate as f64;
            let v = (2.0 * std::f64::consts::PI * freq * t).sin() * amp;
            *sample = v as i16;
        }
        samples
    }

    #[tokio::test]
    async fn picker_lands_in_tone_region() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("sample.wav");
        // Use 16 kHz so the WAV decode is meaningful; the picker downsamples to 8 kHz internally.
        write_wav(&path, 16_000, &synth_silence_with_tone(16_000));

        let res = pick_peaky_window(&path, 5.0).await.unwrap();
        // Tone is at 28-33s. The 5s window covering it must start within [23, 33).
        // With 5s steps the picker considers offsets {0, 5, 10, ..., 55}, so the
        // best one will be 25 or 30.
        assert!(
            (25.0..=30.0).contains(&res.offset_seconds),
            "expected offset near tone region, got {}",
            res.offset_seconds
        );
        assert!((res.duration_seconds - 5.0).abs() < 0.001);
        assert!(res.total_seconds > 59.0 && res.total_seconds < 61.0);
    }

    #[tokio::test]
    async fn shorter_than_window_returns_zero() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("short.wav");
        write_wav(&path, 16_000, &vec![0i16; 16_000 * 3]); // 3s

        let res = pick_peaky_window(&path, 30.0).await.unwrap();
        assert_eq!(res.offset_seconds, 0.0);
        assert!(res.total_seconds < 4.0);
    }

    /// Synthetic peaks blob: 60s at 4 peaks/sec, near-silent except for a loud
    /// region between t=28s and t=33s.
    fn synth_peaks_blob() -> Vec<u8> {
        let pps: u8 = 4;
        let total_buckets = 60 * pps as usize;
        let mut bytes = Vec::with_capacity(2 + total_buckets * 2);
        bytes.push(PEAKS_VERSION);
        bytes.push(pps);
        for i in 0..total_buckets {
            let secs = i as f64 / pps as f64;
            let (min, max) = if (28.0..33.0).contains(&secs) {
                (-100i8, 100i8)
            } else {
                (-1i8, 1i8)
            };
            bytes.push(min as u8);
            bytes.push(max as u8);
        }
        bytes
    }

    #[test]
    fn peaks_picker_lands_in_loud_region() {
        let bytes = synth_peaks_blob();
        let res = pick_peaky_window_from_peaks(&bytes, 5.0).unwrap();
        assert!(
            (25.0..=33.0).contains(&res.offset_seconds),
            "expected offset near loud region, got {}",
            res.offset_seconds
        );
        assert_eq!(res.duration_seconds, 5.0);
    }

    #[test]
    fn peaks_picker_rejects_garbage_blob() {
        // Wrong version byte -> caller falls back to PCM path.
        assert!(pick_peaky_window_from_peaks(&[9, 4, 0, 0, 0, 0], 5.0).is_none());
        assert!(pick_peaky_window_from_peaks(&[], 5.0).is_none());
    }

    #[tokio::test]
    async fn fast_path_used_when_peaks_present() {
        let dir = tempfile::tempdir().unwrap();
        let peaks_path = dir.path().join("file.peaks");
        std::fs::write(&peaks_path, synth_peaks_blob()).unwrap();
        // The audio path is intentionally bogus — the fast path must succeed
        // without ever touching it.
        let bogus_audio = dir.path().join("does-not-exist.wav");
        let res = pick_peaky_window_with_peaks(&bogus_audio, Some(&peaks_path), 5.0)
            .await
            .unwrap();
        assert!((25.0..=33.0).contains(&res.offset_seconds));
    }
}
