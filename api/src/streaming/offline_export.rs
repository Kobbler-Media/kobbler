use std::path::{Path, PathBuf};

use kob_ffmpeg::format::input::InputContext;
use kob_ffmpeg::format::output::OutputContext;
use kob_ffmpeg::packet::Packet;
use kob_ffmpeg::transcode::{HlsConfig, HlsWindow};
use kob_ffmpeg::{FfmpegError, RunContext};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::streaming::variant::scale_to_height;

pub const EXPORT_FILE: &str = "export.mp4";
const PARTIAL_FILE: &str = "export.mp4.part";
const WORK_DIR: &str = "hls";
const SEGMENT_SECONDS: f64 = 6.0;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Quality {
    #[serde(rename = "original")]
    Original,
    #[serde(rename = "1080p")]
    P1080,
    #[serde(rename = "720p")]
    P720,
    #[serde(rename = "480p")]
    P480,
}

impl Quality {
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "original" => Some(Self::Original),
            "1080p" => Some(Self::P1080),
            "720p" => Some(Self::P720),
            "480p" => Some(Self::P480),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Original => "original",
            Self::P1080 => "1080p",
            Self::P720 => "720p",
            Self::P480 => "480p",
        }
    }

    /// Kept in step with the client's streaming-quality presets, so a download looks
    /// like the stream it replaces.
    fn height_and_kbps(self) -> Option<(u32, u32)> {
        match self {
            Self::Original => None,
            Self::P1080 => Some((1080, 8000)),
            Self::P720 => Some((720, 4000)),
            Self::P480 => Some((480, 2000)),
        }
    }

    /// Output size (when it differs from the source) and video bitrate cap. Sources
    /// already at or under the target height are never upscaled.
    pub fn scale_and_bitrate(self, src_w: u32, src_h: u32) -> (Option<(u32, u32)>, Option<u32>) {
        let Some((height, kbps)) = self.height_and_kbps() else {
            return (None, None);
        };
        let scale = (src_w > 0 && src_h > height).then(|| scale_to_height(src_w, src_h, height));
        (scale, Some(kbps))
    }
}

pub fn exports_root(storage_root: &str) -> PathBuf {
    PathBuf::from(storage_root).join("offline_exports")
}

pub fn export_dir(storage_root: &str, export_id: Uuid) -> PathBuf {
    exports_root(storage_root).join(export_id.to_string())
}

pub fn export_file(storage_root: &str, export_id: Uuid) -> PathBuf {
    export_dir(storage_root, export_id).join(EXPORT_FILE)
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExportPlan {
    pub export_id: Uuid,
    pub input_path: PathBuf,
    pub duration_seconds: f64,
    pub quality: Quality,
    pub source_width: u32,
    pub source_height: u32,
    pub audio_stream_index: i32,
    pub audio_bitrate_kbps: i32,
    pub deinterlace: bool,
}

pub struct Encoders {
    pub video: String,
    pub audio: String,
    pub hw_device: Option<kob_ffmpeg::hw::device::SharedHwDevice>,
}

pub struct ExportPaths {
    pub dir: PathBuf,
    pub work_dir: PathBuf,
    pub playlist: PathBuf,
    pub partial: PathBuf,
    pub file: PathBuf,
}

impl ExportPaths {
    pub fn new(storage_root: &str, export_id: Uuid) -> Self {
        let dir = export_dir(storage_root, export_id);
        let work_dir = dir.join(WORK_DIR);
        Self {
            playlist: work_dir.join("stream.m3u8"),
            partial: dir.join(PARTIAL_FILE),
            file: dir.join(EXPORT_FILE),
            work_dir,
            dir,
        }
    }
}

pub fn hls_config(
    plan: &ExportPlan,
    work_dir: &Path,
    encoders: &Encoders,
    run: RunContext,
) -> HlsConfig {
    let mut config = HlsConfig::new(work_dir)
        .segment_duration(SEGMENT_SECONDS)
        .hls_window(HlsWindow::Vod)
        .video_encoder(&encoders.video)
        .audio_encoder(&encoders.audio)
        .audio_bitrate(i64::from(plan.audio_bitrate_kbps) * 1000)
        .run_context(run);
    config.audio_stream_index = Some(plan.audio_stream_index.max(0) as u32);
    let (scale, kbps) = plan
        .quality
        .scale_and_bitrate(plan.source_width, plan.source_height);
    if let Some((w, h)) = scale {
        config.scale_width = Some(w as i32);
        config.scale_height = Some(h as i32);
    }
    if let Some(kbps) = kbps {
        config = config.video_bitrate(i64::from(kbps) * 1000);
    }
    if plan.deinterlace {
        config = config.deinterlace(kob_ffmpeg::filter::graph::Deinterlace::Auto);
    }
    if let Some(hw) = &encoders.hw_device {
        config = config.hw_device(hw.clone());
    }
    config
}

/// How much of the title the VOD playlist lists so far, as a fraction of its duration.
pub fn transcoded_fraction(playlist: &str, duration_seconds: f64) -> Option<f64> {
    if duration_seconds <= 0.0 {
        return None;
    }
    let done: f64 = playlist
        .lines()
        .filter_map(|l| l.strip_prefix("#EXTINF:"))
        .filter_map(|l| l.split(',').next()?.trim().parse::<f64>().ok())
        .sum();
    Some((done / duration_seconds).clamp(0.0, 1.0))
}

/// Copy every stream of `input` into a progressive MP4 with its index ahead of the media,
/// so a player can start on a partial download.
pub fn remux_faststart(input: &Path, output: &Path, run: &RunContext) -> kob_ffmpeg::Result<()> {
    let mut in_ctx = InputContext::open_path(input)?;
    in_ctx.find_stream_info()?;
    let output_str = output.to_str().ok_or_else(|| {
        FfmpegError::new(
            kob_ffmpeg::ffi::AVERROR_EINVAL,
            "output path not valid UTF-8",
        )
    })?;
    let mut out_ctx = OutputContext::create_with_format(output_str, "mp4")?;
    out_ctx.set_option("movflags", "+faststart")?;
    let stream_map = (0..in_ctx.nb_streams())
        .map(|i| out_ctx.add_stream_copy(&in_ctx.stream(i).expect("stream index in range")))
        .collect::<kob_ffmpeg::Result<Vec<i32>>>()?;
    out_ctx.write_header()?;

    let mut packet = Packet::new()?;
    while in_ctx.read_packet(&mut packet)? {
        if run.is_cancelled() {
            return Err(FfmpegError::cancelled("offline export remux cancelled"));
        }
        let idx = packet.stream_index();
        if let Some(&out_idx) = usize::try_from(idx).ok().and_then(|i| stream_map.get(i)) {
            let in_tb = in_ctx
                .stream(idx as u32)
                .expect("stream index in range")
                .time_base();
            out_ctx.write_packet_rescaled(&mut packet, out_idx, in_tb)?;
        }
    }
    out_ctx.write_trailer()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn qualities_scale_down_to_their_height_and_cap_bitrate() {
        let cases = [
            (Quality::Original, 1920, 1080, None, None),
            (Quality::P1080, 3840, 2160, Some((1920, 1080)), Some(8000)),
            (Quality::P1080, 1920, 1080, None, Some(8000)),
            (Quality::P720, 1920, 1080, Some((1280, 720)), Some(4000)),
            (Quality::P720, 1920, 800, Some((1728, 720)), Some(4000)),
            (Quality::P480, 960, 540, Some((852, 480)), Some(2000)),
            (Quality::P480, 640, 360, None, Some(2000)),
            (Quality::P480, 0, 0, None, Some(2000)),
        ];
        for (quality, w, h, scale, kbps) in cases {
            assert_eq!(
                quality.scale_and_bitrate(w, h),
                (scale, kbps),
                "{quality:?} {w}x{h}"
            );
        }
    }

    #[test]
    fn quality_names_round_trip() {
        for q in [
            Quality::Original,
            Quality::P1080,
            Quality::P720,
            Quality::P480,
        ] {
            assert_eq!(Quality::parse(q.as_str()), Some(q));
            assert_eq!(serde_json::to_value(q).unwrap(), q.as_str());
        }
        assert_eq!(Quality::parse("4k"), None);
    }

    #[test]
    fn progress_is_the_listed_share_of_the_duration() {
        let playlist = "#EXTM3U\n#EXTINF:6.000000,\nseg_00000.m4s\n#EXTINF:4.5,\nseg_00001.m4s\n";
        assert_eq!(transcoded_fraction(playlist, 21.0), Some(0.5));
        assert_eq!(transcoded_fraction(playlist, 5.0), Some(1.0));
        assert_eq!(transcoded_fraction(playlist, 0.0), None);
        assert_eq!(transcoded_fraction("#EXTM3U\n", 10.0), Some(0.0));
    }
}
