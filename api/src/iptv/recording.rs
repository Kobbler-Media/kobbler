//! FFmpeg recording management.
//!
//! Records IPTV streams to disk using kob-ffmpeg, encoding directly to H.264 MP4
//! in a single pass with optional hardware acceleration (NVENC/VAAPI).

use std::path::PathBuf;

use anyhow::{Context, Result, bail};
use tracing::info;

pub use kob_ffmpeg::iptv::RecordResult;

pub struct RecordingConfig {
    pub source_url: String,
    pub output_path: PathBuf,
    pub duration_secs: i64,
    /// Hardware acceleration: "vaapi", "nvenc", or None for software.
    pub hw_accel: Option<String>,
}

/// Start a recording: encode directly to MP4 in a single pass.
pub async fn start_recording(config: RecordingConfig) -> Result<RecordResult> {
    info!(
        url = %config.source_url,
        output = %config.output_path.display(),
        duration_secs = config.duration_secs,
        "starting recording"
    );

    let url = config.source_url.clone();
    let output = config.output_path.clone();
    let max_dur = config.duration_secs as f64;
    let hw = config.hw_accel.clone();

    let result = tokio::task::spawn_blocking(move || {
        let audio_enc = crate::streaming::decision::AAC_ENCODER;
        let video_enc = match hw.as_deref() {
            Some("vaapi") => "h264_vaapi",
            Some("nvenc") => "h264_nvenc",
            _ => "libx264",
        };

        kob_ffmpeg::iptv::record_and_encode(
            &url,
            &output,
            video_enc,
            audio_enc,
            128_000,
            Some(max_dur),
            None,
        )
    })
    .await
    .context("record task join error")?;

    match result {
        Ok(record_result) => {
            info!(
                packets = record_result.packets_written,
                duration_secs = record_result.duration_recorded_secs,
                output = %config.output_path.display(),
                "recording complete"
            );
            Ok(record_result)
        }
        Err(e) => {
            // Err means no usable data was captured (record_and_encode returns Ok
            // for partial recordings where flush+trailer succeeded). Safe to delete.
            let _ = tokio::fs::remove_file(&config.output_path).await;
            bail!("recording failed: {e}");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recording_config_fields() {
        let config = RecordingConfig {
            source_url: "http://example.com/stream".to_string(),
            output_path: PathBuf::from("/tmp/output.mp4"),
            duration_secs: 3600,
            hw_accel: Some("nvenc".to_string()),
        };
        assert_eq!(config.duration_secs, 3600);
        assert_eq!(config.hw_accel.as_deref(), Some("nvenc"));
    }
}
