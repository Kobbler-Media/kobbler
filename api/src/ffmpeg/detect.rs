//! Hardware acceleration detection for FFmpeg.
//!
//! Detects available GPU encoding capabilities at startup using kob-ffmpeg.

use std::path::Path;
use tracing::info;

/// Available hardware acceleration method.
#[derive(Debug, Clone)]
pub enum HwAccel {
    /// Intel/AMD VAAPI (Linux).
    Vaapi { device: String },
    /// NVIDIA NVENC.
    Nvenc,
    /// No hardware acceleration available.
    None,
}

/// Whether NVDEC (CUDA) hardware-accelerated video decoding is available.
/// Useful for thumbnail generation — GPU decode is 3-5x faster than CPU.
#[derive(Debug, Clone, Copy)]
pub struct DecoderCaps {
    pub nvdec: bool,
    pub fpcalc: bool,
}

impl HwAccel {
    /// Whether any hardware acceleration is available.
    pub fn is_available(&self) -> bool {
        !matches!(self, HwAccel::None)
    }

    /// FFmpeg args to prepend for hardware acceleration input setup.
    pub fn input_args(&self) -> Vec<String> {
        match self {
            HwAccel::Vaapi { device } => vec![
                "-hwaccel".to_string(),
                "vaapi".to_string(),
                "-hwaccel_device".to_string(),
                device.clone(),
                "-hwaccel_output_format".to_string(),
                "vaapi".to_string(),
            ],
            HwAccel::Nvenc => vec![
                "-hwaccel".to_string(),
                "cuda".to_string(),
                "-hwaccel_output_format".to_string(),
                "cuda".to_string(),
                "-extra_hw_frames".to_string(),
                "3".to_string(),
            ],
            HwAccel::None => vec![],
        }
    }

    /// FFmpeg args for HW-accelerated subtitle burn-in input setup.
    pub fn burn_in_input_args(&self) -> Vec<String> {
        match self {
            HwAccel::Vaapi { device } => vec![
                "-init_hw_device".to_string(),
                format!("vaapi=va:{device}"),
                "-hwaccel".to_string(),
                "vaapi".to_string(),
                "-hwaccel_device".to_string(),
                "va".to_string(),
                "-hwaccel_output_format".to_string(),
                "vaapi".to_string(),
            ],
            HwAccel::Nvenc => self.input_args(),
            HwAccel::None => vec![],
        }
    }
}

/// Detected encoder capabilities — which video codecs the server can encode.
#[derive(Debug, Clone)]
pub struct EncoderCaps {
    /// H.264 encoder (always present).
    pub h264: String,
    /// HEVC encoder, if available.
    pub hevc: Option<String>,
    /// AV1 encoder, if available.
    pub av1: Option<String>,
}

impl EncoderCaps {
    /// Get the encoder name for a given codec, if available.
    pub fn encoder_for(&self, codec: &str) -> Option<&str> {
        match codec {
            "h264" => Some(&self.h264),
            "hevc" => self.hevc.as_deref(),
            "av1" => self.av1.as_deref(),
            _ => Option::None,
        }
    }

    /// List of codecs the server can encode, ordered by efficiency.
    pub fn preferred_codecs(&self) -> Vec<&str> {
        let mut codecs = Vec::with_capacity(3);
        if self.hevc.is_some() {
            codecs.push("hevc");
        }
        codecs.push("h264");
        codecs
    }

    /// All codecs the server can encode.
    pub fn available_codecs(&self) -> Vec<&str> {
        let mut codecs = vec!["h264"];
        if self.hevc.is_some() {
            codecs.push("hevc");
        }
        if self.av1.is_some() {
            codecs.push("av1");
        }
        codecs
    }

    /// Fallback H.264 encoders to try when the primary one fails.
    /// Returns encoders in priority order (vaapi before software).
    pub fn h264_fallbacks(&self) -> Vec<&str> {
        let mut fallbacks = Vec::new();
        // If primary is nvenc, try vaapi first (if available), then software
        if self.h264.contains("nvenc") {
            fallbacks.push("h264_vaapi");
        }
        // Always include software as last resort
        if self.h264 != "libx264" {
            fallbacks.push("libx264");
        }
        fallbacks
    }
}

/// Resolve hardware acceleration from a user preference string.
///
/// Returns HW accel method, encoder capabilities, and decoder capabilities.
pub async fn resolve_hw_accel(preference: &str) -> (HwAccel, EncoderCaps, DecoderCaps) {
    // kob-ffmpeg detection is synchronous — run in blocking thread
    let pref = preference.to_string();
    let (hw, caps, decoder_caps) = tokio::task::spawn_blocking(move || {
        let hw_caps = kob_ffmpeg::detect::detect_hw_capabilities();

        let hw = match pref.as_str() {
            "vaapi" => {
                if hw_caps.vaapi_available {
                    HwAccel::Vaapi {
                        device: find_vaapi_device(),
                    }
                } else {
                    HwAccel::None
                }
            }
            "nvenc" => {
                if hw_caps.cuda_available {
                    HwAccel::Nvenc
                } else {
                    HwAccel::None
                }
            }
            "none" => HwAccel::None,
            _ => {
                // Auto-detect: prefer VAAPI, then NVENC
                if hw_caps.vaapi_available && !hw_caps.vaapi_encoders.is_empty() {
                    HwAccel::Vaapi {
                        device: find_vaapi_device(),
                    }
                } else if hw_caps.cuda_available && !hw_caps.nvenc_encoders.is_empty() {
                    HwAccel::Nvenc
                } else {
                    info!("No hardware acceleration detected — will use software encoding");
                    HwAccel::None
                }
            }
        };

        let caps = build_encoder_caps(&hw, &hw_caps);
        let fpcalc = kob_ffmpeg::detect::chromaprint_available();
        let decoder_caps = DecoderCaps {
            nvdec: hw_caps.nvdec_available,
            fpcalc,
        };

        (hw, caps, decoder_caps)
    })
    .await
    .expect("hw detection task panicked");

    info!(
        h264 = %caps.h264,
        hevc = ?caps.hevc,
        av1 = ?caps.av1,
        nvdec = decoder_caps.nvdec,
        fpcalc = decoder_caps.fpcalc,
        "Encoder/decoder capabilities detected"
    );

    (hw, caps, decoder_caps)
}

/// Find the first available VAAPI render device.
fn find_vaapi_device() -> String {
    for device in ["/dev/dri/renderD128", "/dev/dri/renderD129"] {
        if Path::new(device).exists() {
            return device.to_string();
        }
    }
    "/dev/dri/renderD128".to_string()
}

/// Build encoder capabilities from kob-ffmpeg's HwCapabilities.
fn build_encoder_caps(hw: &HwAccel, hw_caps: &kob_ffmpeg::detect::HwCapabilities) -> EncoderCaps {
    let h264 = match hw {
        HwAccel::Vaapi { .. } => "h264_vaapi".to_string(),
        HwAccel::Nvenc => "h264_nvenc".to_string(),
        HwAccel::None => "libx264".to_string(),
    };

    let hevc = match hw {
        HwAccel::Vaapi { .. } => {
            if hw_caps.vaapi_encoders.iter().any(|e| e == "hevc_vaapi") {
                Some("hevc_vaapi".to_string())
            } else {
                None
            }
        }
        HwAccel::Nvenc => {
            if hw_caps.nvenc_encoders.iter().any(|e| e == "hevc_nvenc") {
                Some("hevc_nvenc".to_string())
            } else {
                None
            }
        }
        HwAccel::None => None,
    };

    let av1 = match hw {
        HwAccel::Vaapi { .. } => {
            if hw_caps.vaapi_encoders.iter().any(|e| e == "av1_vaapi") {
                Some("av1_vaapi".to_string())
            } else {
                None
            }
        }
        HwAccel::Nvenc => {
            if hw_caps.nvenc_encoders.iter().any(|e| e == "av1_nvenc") {
                Some("av1_nvenc".to_string())
            } else {
                None
            }
        }
        HwAccel::None => None,
    };

    EncoderCaps { h264, hevc, av1 }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn resolve_none_always_returns_none() {
        let (hw, caps, _decoder) = resolve_hw_accel("none").await;
        assert!(matches!(hw, HwAccel::None));
        assert_eq!(caps.h264, "libx264");
        assert!(caps.hevc.is_none());
        assert!(caps.av1.is_none());
    }

    #[tokio::test]
    async fn resolve_unknown_defaults_to_auto() {
        let (hw, _caps, _decoder) = resolve_hw_accel("auto").await;
        let _ = hw.is_available();
    }

    #[test]
    fn encoder_caps_h264_always_available() {
        let caps = EncoderCaps {
            h264: "libx264".to_string(),
            hevc: None,
            av1: None,
        };
        assert_eq!(caps.encoder_for("h264"), Some("libx264"));
        assert_eq!(caps.encoder_for("hevc"), None);
        assert_eq!(caps.encoder_for("av1"), None);
        assert_eq!(caps.preferred_codecs(), vec!["h264"]);
    }

    #[test]
    fn encoder_caps_with_hevc() {
        let caps = EncoderCaps {
            h264: "h264_vaapi".to_string(),
            hevc: Some("hevc_vaapi".to_string()),
            av1: None,
        };
        assert_eq!(caps.encoder_for("hevc"), Some("hevc_vaapi"));
        assert_eq!(caps.preferred_codecs(), vec!["hevc", "h264"]);
    }

    #[test]
    fn encoder_caps_with_all() {
        let caps = EncoderCaps {
            h264: "h264_nvenc".to_string(),
            hevc: Some("hevc_nvenc".to_string()),
            av1: Some("av1_nvenc".to_string()),
        };
        assert_eq!(caps.encoder_for("av1"), Some("av1_nvenc"));
        assert_eq!(caps.available_codecs(), vec!["h264", "hevc", "av1"]);
        assert_eq!(caps.preferred_codecs(), vec!["hevc", "h264"]);
    }

    #[test]
    fn hw_accel_none_has_no_input_args() {
        assert!(HwAccel::None.input_args().is_empty());
    }

    #[test]
    fn hw_accel_nvenc_input_args_include_cuda() {
        let args = HwAccel::Nvenc.input_args();
        assert!(args.contains(&"cuda".to_string()));
    }
}
