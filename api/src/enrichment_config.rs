//! Config tag computation for enrichment jobs.
//!
//! Each enrichment job stamps its output with a "config tag" that captures
//! which configuration was active when it ran. The backfill handlers compare
//! stored tags against the current config to decide whether re-runs are needed.

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
/// whose stored tag differs from the current one. The intent is to invalidate
/// on a genuine STT *model* change — but the model isn't visible to kobbler,
/// so historically this hashed the STT service URL. That conflates routing
/// with model identity: renaming a host or pointing through a gateway (e.g.
/// requests now go via koblem, not the literal `stt_service_url`) flips the
/// hash and needlessly re-transcribes the entire library.
///
/// `STT_CONFIG_TAG`, when set, pins the tag to an explicit, stable value so
/// routing/URL changes are inert. Bump it only when you actually want to
/// force a full re-transcription (a real model swap). When unset, falls back
/// to the legacy URL hash for backward compatibility. Returns `"none"` if
/// neither is available.
pub fn stt_config_tag(stt_url: &Option<String>) -> String {
    stt_config_tag_inner(stt_url, std::env::var("STT_CONFIG_TAG").ok().as_deref())
}

/// Pure core of [`stt_config_tag`], split out so the env-independent logic is
/// unit-testable without mutating process env (unsafe under edition 2024).
fn stt_config_tag_inner(stt_url: &Option<String>, override_tag: Option<&str>) -> String {
    if let Some(tag) = override_tag {
        let tag = tag.trim();
        if !tag.is_empty() {
            return tag.to_string();
        }
    }
    match stt_url.as_deref() {
        Some(url) => {
            let mut h = DefaultHasher::new();
            url.hash(&mut h);
            format!("stt:{:08x}", h.finish() as u32)
        }
        None => "none".to_string(),
    }
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
        // URL-hash fallback (no override). Use the inner fn so the test is
        // deterministic regardless of STT_CONFIG_TAG in the build env.
        assert_eq!(stt_config_tag_inner(&None, None), "none");
        let tag1 = stt_config_tag_inner(&Some("http://stt:8880".into()), None);
        let tag2 = stt_config_tag_inner(&Some("http://other-stt:8880".into()), None);
        assert!(tag1.starts_with("stt:"));
        assert!(tag2.starts_with("stt:"));
        assert_ne!(tag1, tag2, "Different URLs should produce different tags");
        // Same URL should be stable
        assert_eq!(
            tag1,
            stt_config_tag_inner(&Some("http://stt:8880".into()), None)
        );
    }

    #[test]
    fn stt_tag_override_pins_value() {
        // An explicit override wins over the URL — routing changes are inert.
        assert_eq!(
            stt_config_tag_inner(&Some("http://stt:8880".into()), Some("stt:45fe0e56")),
            "stt:45fe0e56"
        );
        assert_eq!(
            stt_config_tag_inner(&Some("http://other-stt:9999".into()), Some("stt:45fe0e56")),
            "stt:45fe0e56"
        );
        // Empty / whitespace override falls back to the URL hash.
        assert_eq!(
            stt_config_tag_inner(&Some("http://stt:8880".into()), Some("  ")),
            stt_config_tag_inner(&Some("http://stt:8880".into()), None)
        );
    }
}
