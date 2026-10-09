//! Cut a WAV slice from an audiobook chapter for voice-cloning workflows.
//!
//! Reads mono s16 PCM for the requested range via `kob_ffmpeg::audio::extract_pcm_range`,
//! then wraps it in a minimal WAV header. Used by both:
//! - the admin TTS routes (preview-stream + audiobook → preset clip), and
//! - the public-library route (`/api/v1/library/audiobooks/{id}/clip`) that
//!   koblem hits for TTS voice-clone-from-library.
//!
//! Avoids `split_audio_segment`'s encoder graph (which hardcodes stereo/FLTP
//! and only round-trips through codecs like libmp3lame) — the goal here is a
//! short canonical reference clip suitable for any TTS voice-cloning service.

use std::io::Write;
use std::path::{Path, PathBuf};

/// Sample rate of the WAV clips this module writes. Matches what
/// `convert_to_wav` produces for non-WAV uploads, so a TTS service sees the
/// same format whether the user uploaded a file or pulled from a chapter.
pub const CLIP_SAMPLE_RATE: i32 = 22_050;

/// Maximum clip duration accepted from any caller (in seconds).
pub const MAX_CLIP_SECS: f64 = 60.0;

/// Errors from clip extraction.
#[derive(Debug, thiserror::Error)]
pub enum ClipError {
    #[error("ffmpeg extract failed: {0}")]
    Ffmpeg(String),
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
    #[error("blocking task join failed: {0}")]
    Join(#[from] tokio::task::JoinError),
}

/// Cut a window from `abs_path` and write it as a 16-bit mono WAV to
/// `output_path`. The blocking ffmpeg work runs on a `spawn_blocking` worker.
pub async fn cut_audiobook_segment(
    abs_path: PathBuf,
    output_path: PathBuf,
    offset_seconds: f64,
    duration_seconds: f64,
) -> Result<(), ClipError> {
    tokio::task::spawn_blocking(move || -> Result<(), ClipError> {
        let mut pcm: Vec<u8> = Vec::new();
        kob_ffmpeg::audio::extract_pcm_range(
            &abs_path,
            CLIP_SAMPLE_RATE,
            offset_seconds,
            duration_seconds,
            None,
            &mut |data| {
                pcm.extend_from_slice(data);
                true
            },
        )
        .map_err(|e| ClipError::Ffmpeg(e.to_string()))?;
        write_pcm_as_wav(&output_path, CLIP_SAMPLE_RATE as u32, 1, &pcm)?;
        Ok(())
    })
    .await?
}

/// Write s16le PCM bytes as a canonical 16-bit WAV file.
pub fn write_pcm_as_wav(
    path: &Path,
    sample_rate: u32,
    channels: u16,
    pcm: &[u8],
) -> std::io::Result<()> {
    let bits_per_sample: u16 = 16;
    let block_align: u16 = channels * (bits_per_sample / 8);
    let byte_rate: u32 = sample_rate * block_align as u32;
    let data_len: u32 = pcm.len() as u32;
    let mut f = std::fs::File::create(path)?;
    f.write_all(b"RIFF")?;
    f.write_all(&(36u32 + data_len).to_le_bytes())?;
    f.write_all(b"WAVE")?;
    f.write_all(b"fmt ")?;
    f.write_all(&16u32.to_le_bytes())?;
    f.write_all(&1u16.to_le_bytes())?;
    f.write_all(&channels.to_le_bytes())?;
    f.write_all(&sample_rate.to_le_bytes())?;
    f.write_all(&byte_rate.to_le_bytes())?;
    f.write_all(&block_align.to_le_bytes())?;
    f.write_all(&bits_per_sample.to_le_bytes())?;
    f.write_all(b"data")?;
    f.write_all(&data_len.to_le_bytes())?;
    f.write_all(pcm)?;
    Ok(())
}
