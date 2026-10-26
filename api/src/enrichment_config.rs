//! Config tag computation for enrichment jobs.
//!
//! Each enrichment job stamps its output with a "config tag" that captures
//! which configuration was active when it ran. The backfill handlers compare
//! stored tags against the current config to decide whether re-runs are needed.

use crate::runtime_config::RuntimeConfig;
use crate::skip_detect::PipelineMode;
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};

/// Config tag for skip_detect: captures detection mode and vision availability.
///
/// Returns human-readable values like `"audio"`, `"full+vision"`, `"full"`.
pub fn skip_detect_config_tag(mode: &PipelineMode, vision_url: &Option<String>) -> String {
    match (mode, vision_url.is_some()) {
        (PipelineMode::Full, true) => "full+vision".to_string(),
        (PipelineMode::Full, false) => "full".to_string(),
        (PipelineMode::Audio, _) => "audio".to_string(),
        (PipelineMode::Off, _) => "off".to_string(),
    }
}

/// Config tag for audiobook transcription.
///
/// Identifies the transcript "generation": backfill re-transcribes any file
/// whose stored tag differs from the current one. It has two halves:
///
/// - the STT *model*, which kobbler can't see. `STT_CONFIG_TAG` pins it to an
///   explicit value so routing changes are inert; bump it on a real model
///   swap. Unset, it falls back to a hash of the effective STT URL.
/// - [`STT_GENERATION`], bumped to re-transcribe everything kobbler already
///   holds, e.g. after an engine-side fix that doesn't change the model.
///
/// Both the transcribe job and the backfill must derive the tag from the same
/// config, or the backfill sees every book as stale forever.
pub fn stt_config_tag(config: &RuntimeConfig) -> String {
    stt_config_tag_inner(
        &config.effective_stt_url(),
        std::env::var("STT_CONFIG_TAG").ok().as_deref(),
    )
}

pub const STT_GENERATION: &str = "g2";

fn stt_config_tag_inner(stt_url: &Option<String>, override_tag: Option<&str>) -> String {
    let model = match (override_tag.map(str::trim), stt_url.as_deref()) {
        (Some(tag), _) if !tag.is_empty() => tag.to_string(),
        (_, Some(url)) => {
            let mut h = DefaultHasher::new();
            url.hash(&mut h);
            format!("stt:{:08x}", h.finish() as u32)
        }
        _ => "none".to_string(),
    };
    format!("{model}/{STT_GENERATION}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn skip_detect_tags() {
        assert_eq!(skip_detect_config_tag(&PipelineMode::Audio, &None), "audio");
        assert_eq!(
            skip_detect_config_tag(&PipelineMode::Full, &Some("http://vision:8890".into())),
            "full+vision"
        );
        assert_eq!(skip_detect_config_tag(&PipelineMode::Full, &None), "full");
        assert_eq!(skip_detect_config_tag(&PipelineMode::Off, &None), "off");
    }

    #[test]
    fn stt_tags() {
        assert_eq!(stt_config_tag_inner(&None, None), "none/g2");
        let tag1 = stt_config_tag_inner(&Some("http://stt:8880".into()), None);
        let tag2 = stt_config_tag_inner(&Some("http://other-stt:8880".into()), None);
        assert!(tag1.starts_with("stt:") && tag1.ends_with("/g2"));
        assert_ne!(tag1, tag2);
        assert_eq!(
            tag1,
            stt_config_tag_inner(&Some("http://stt:8880".into()), None)
        );
    }

    #[test]
    fn stt_tag_override_pins_the_model_half() {
        assert_eq!(
            stt_config_tag_inner(&Some("http://stt:8880".into()), Some("stt:45fe0e56")),
            "stt:45fe0e56/g2"
        );
        assert_eq!(
            stt_config_tag_inner(&Some("http://other-stt:9999".into()), Some("stt:45fe0e56")),
            "stt:45fe0e56/g2"
        );
        assert_eq!(
            stt_config_tag_inner(&Some("http://stt:8880".into()), Some("  ")),
            stt_config_tag_inner(&Some("http://stt:8880".into()), None)
        );
    }
}
