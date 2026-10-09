//! FFprobe wrapper for media file analysis.
//!
//! Delegates to `kob_ffmpeg::probe` for actual probing. This module re-exports
//! the probe types and wraps synchronous kob-ffmpeg calls in `spawn_blocking`.

use std::path::Path;

// Re-export probe types so callers don't need to change their imports.
pub use kob_ffmpeg::probe::{
    AudioStream, ChapterInfo, FormatInfo, ProbeOptions, ProbeResult, SubtitleStream, VideoStream,
};

/// Errors from ffprobe operations.
#[derive(Debug, thiserror::Error)]
pub enum ProbeError {
    #[error("ffprobe not found at '{0}' — is ffmpeg installed?")]
    NotFound(String),
    #[error("ffprobe failed with exit code {0}: {1}")]
    ExitError(i32, String),
    #[error("failed to parse ffprobe output: {0}")]
    ParseError(String),
    #[error("I/O error running ffprobe: {0}")]
    IoError(#[from] std::io::Error),
    #[error("ffmpeg error: {0}")]
    Ffmpeg(#[from] kob_ffmpeg::FfmpegError),
}

/// Probe a media file and return structured metadata.
///
pub async fn probe(path: &Path) -> Result<ProbeResult, ProbeError> {
    let path = path.to_owned();
    tokio::task::spawn_blocking(move || kob_ffmpeg::probe::probe(&path).map_err(ProbeError::Ffmpeg))
        .await
        .map_err(|e| ProbeError::IoError(std::io::Error::other(e)))?
}

/// Probe keyframe timestamps from the first video stream.
///
pub async fn probe_keyframes(path: &Path) -> Result<Vec<f64>, ProbeError> {
    let path = path.to_owned();
    tokio::task::spawn_blocking(move || {
        kob_ffmpeg::probe::probe_keyframes(&path).map_err(ProbeError::Ffmpeg)
    })
    .await
    .map_err(|e| ProbeError::IoError(std::io::Error::other(e)))?
}

/// Check if a probed file would be universally direct-playable in browsers.
///
/// Returns true for MP4 container + H.264 video + browser-compatible audio.
pub fn is_direct_playable(probe: &ProbeResult) -> bool {
    let mp4_container = matches!(
        probe.format.format_name.as_str(),
        "mov,mp4,m4a,3gp,3g2,mj2" | "mp4"
    );

    let h264_video = probe
        .video_streams
        .first()
        .is_some_and(|v| v.codec == "h264");

    let has_compatible_audio = probe
        .audio_streams
        .iter()
        .any(|a| matches!(a.codec.as_str(), "aac" | "opus" | "flac"));

    mp4_container && h264_video && has_compatible_audio
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_probe(format: &str, video_codec: &str, audio_codecs: &[&str]) -> ProbeResult {
        ProbeResult {
            format: FormatInfo {
                format_name: format.to_string(),
                duration_seconds: Some(120.0),
                bit_rate_kbps: None,
                size_bytes: None,
            },
            video_streams: vec![VideoStream {
                index: 0,
                codec: video_codec.to_string(),
                profile: None,
                width: 1920,
                height: 1080,
                bitrate_kbps: None,
                framerate: Some(24.0),
                bit_depth: Some(8),
                is_hdr: false,
                hdr_format: None,
                is_interlaced: false,
            }],
            audio_streams: audio_codecs
                .iter()
                .enumerate()
                .map(|(i, codec)| AudioStream {
                    index: (i + 1) as i32,
                    codec: codec.to_string(),
                    channels: 2,
                    sample_rate: Some(48000),
                    bitrate_kbps: Some(128),
                    language: None,
                    title: None,
                    is_default: i == 0,
                })
                .collect(),
            subtitle_streams: vec![],
            chapters: vec![],
        }
    }

    #[test]
    fn test_direct_playable_mp4_h264_aac() {
        assert!(is_direct_playable(&make_probe(
            "mov,mp4,m4a,3gp,3g2,mj2",
            "h264",
            &["aac"]
        )));
        assert!(is_direct_playable(&make_probe("mp4", "h264", &["aac"])));
    }

    #[test]
    fn test_not_direct_playable_mkv() {
        assert!(!is_direct_playable(&make_probe(
            "matroska,webm",
            "h264",
            &["aac"]
        )));
    }

    #[test]
    fn test_not_direct_playable_hevc() {
        assert!(!is_direct_playable(&make_probe(
            "mov,mp4,m4a,3gp,3g2,mj2",
            "hevc",
            &["aac"]
        )));
    }

    #[test]
    fn test_not_direct_playable_dts_only() {
        assert!(!is_direct_playable(&make_probe(
            "mov,mp4,m4a,3gp,3g2,mj2",
            "h264",
            &["dts"]
        )));
    }

    #[test]
    fn test_direct_playable_mixed_audio_with_compatible_track() {
        assert!(is_direct_playable(&make_probe(
            "mov,mp4,m4a,3gp,3g2,mj2",
            "h264",
            &["dts", "aac"]
        )));
    }

    #[test]
    fn test_direct_playable_opus() {
        assert!(is_direct_playable(&make_probe("mp4", "h264", &["opus"])));
    }

    #[test]
    fn test_direct_playable_flac() {
        assert!(is_direct_playable(&make_probe("mp4", "h264", &["flac"])));
    }
}
