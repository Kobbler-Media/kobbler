//! File-based configuration persistence.
//!
//! Configuration is stored in a TOML file (default: /config/kobbler.toml).
//! The Settings UI reads from and writes to this file.
//! Uses toml_edit to preserve comments and formatting when saving.

use crate::config::KoboMode;
use crate::runtime_config::{DiscoverStreamingService, RuntimeConfig};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::Path;
use std::str::FromStr;
use toml_edit::DocumentMut;
use tracing::info;

/// Configuration stored in the TOML file.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileConfig {
    #[serde(default)]
    pub public_url: Option<String>,
    #[serde(default)]
    pub public_proxy_url: String,
    #[serde(default = "default_kobo_storeapi_base_url")]
    pub kobo_storeapi_base_url: String,
    #[serde(default = "default_kobo_mode")]
    pub kobo_mode: String,
    #[serde(default = "default_true")]
    pub proxy_logging_enabled: bool,
    #[serde(default = "default_proxy_log_retention_days")]
    pub proxy_log_retention_days: i32,
    #[serde(default)]
    pub blob_capture_enabled: bool,
    #[serde(default = "default_kobo_sync_batch_size")]
    pub kobo_sync_batch_size: i64,
    #[serde(default = "default_activity_limit")]
    pub activity_limit: i64,
    #[serde(default = "default_page_size")]
    pub default_page_size: i64,
    #[serde(default = "default_max_page_size")]
    pub max_page_size: i64,
    #[serde(default = "default_entitlement_mode")]
    pub default_entitlement_mode: String,
    #[serde(default)]
    pub enrichment_enabled: bool,
    #[serde(default)]
    pub watcher_enabled: bool,
    #[serde(default = "default_watcher_scan_interval")]
    pub watcher_scan_interval_seconds: i32,
    #[serde(default = "default_media_root_scan_interval")]
    pub media_root_scan_interval_seconds: i32,
    #[serde(default = "default_true")]
    pub metadata_providers_enabled: bool,
    #[serde(default = "default_metadata_cache_days")]
    pub metadata_cache_days: i32,
    #[serde(default)]
    pub rewrite_kobo_urls: bool,
    #[serde(default)]
    pub wanted_default_provider: Option<String>,
    #[serde(default)]
    pub wanted_min_score: Option<f64>,
    #[serde(default)]
    pub kobload_base_url: Option<String>,
    #[serde(default)]
    pub kobload_api_key: Option<String>,
    #[serde(default)]
    pub kobload_ingest_url: Option<String>,
    #[serde(default)]
    pub kobload_communication_mode: Option<String>,

    // MeiliSearch configuration
    #[serde(default)]
    pub meili_url: Option<String>,
    #[serde(default)]
    pub meili_master_key: Option<String>,

    // Koblem (local LLM service)
    #[serde(default)]
    pub koblem_service_url: Option<String>,
    #[serde(default)]
    pub koblem_api_key: Option<String>,

    // Chat (AI Assistant)
    #[serde(default)]
    pub chat_enabled: bool,
    #[serde(default)]
    pub chat_llm_provider: Option<String>,
    #[serde(default)]
    pub chat_llm_base_url: Option<String>,
    #[serde(default)]
    pub chat_llm_api_key: Option<String>,
    #[serde(default)]
    pub chat_llm_model: Option<String>,
    #[serde(default)]
    pub chat_llm_thinking: Option<String>,

    // Conversion tool paths (optional - defaults to searching PATH).
    // EPUB→KEPUB and EPUB/MOBI/AZW3→EPUB are handled in-process via kob-ebook;
    // ebook-convert (Calibre) is only the long-tail fallback for PDF/FB2/etc.
    #[serde(default)]
    pub ebook_convert_path: Option<String>,
    #[serde(default)]
    pub ebook_meta_path: Option<String>,

    // Migration (Sonarr/Radarr)
    #[serde(default)]
    pub sonarr_url: Option<String>,
    #[serde(default)]
    pub sonarr_api_key: Option<String>,
    #[serde(default)]
    pub radarr_url: Option<String>,
    #[serde(default)]
    pub radarr_api_key: Option<String>,

    #[serde(default)]
    pub video_storage_root: Option<String>,

    // Streaming / transcoding configuration
    #[serde(default)]
    pub transcode_cache_dir: Option<String>,
    #[serde(default)]
    pub debrid_cache_dir: Option<String>,
    #[serde(default)]
    pub cache_dir: Option<String>,
    #[serde(default = "default_preferred_language")]
    pub preferred_audio_language: String,
    #[serde(default = "default_preferred_language")]
    pub preferred_subtitle_language: String,
    /// Trim bloated Matroska cue indexes out of direct-play byte ranges as they are
    /// served. Costs one ~1.7MB read per file; saves seconds of demuxer startup on
    /// releases that cue every line of 30-odd subtitle languages.
    #[serde(default = "default_true")]
    pub mkv_cue_trim_enabled: bool,

    // Transcription (STT service)
    #[serde(default = "default_transcript_mode")]
    pub transcript_mode: String,
    /// Legacy: migrated into job_concurrency["audiobook_transcribe"]
    #[serde(default)]
    pub transcribe_parallelism: Option<u32>,
    #[serde(default)]
    pub stt_service_url: Option<String>,

    /// Per-job-type concurrency overrides (key = job type, value = 1–32)
    #[serde(default)]
    pub job_concurrency: Option<std::collections::HashMap<String, u32>>,

    // TTS (text-to-speech) sidecar service
    #[serde(default)]
    pub tts_enabled: bool,
    #[serde(default)]
    pub tts_service_url: Option<String>,
    #[serde(default = "default_auto")]
    pub tts_service_type: String,
    #[serde(default)]
    pub tts_default_voice: Option<String>,
    #[serde(default)]
    pub tts_default_temperature: Option<f64>,

    // Hardware encoder preference
    #[serde(default)]
    pub hw_encoder: Option<String>,

    // GPU sharing
    #[serde(default = "default_gpu_preempt_tts")]
    pub gpu_preempt_tts: bool,

    /// Route vision/TTS/STT calls through koblem (so koblem's priority GPU lock
    /// serialises across all three services). When false, kobbler uses the
    /// per-service URLs and its local GpuLock (standalone mode — no koblem
    /// required). Defaults to false so kobbler works standalone; flip on once
    /// koblem's GPU upstreams are wired up.
    #[serde(default)]
    pub route_gpu_via_koblem: bool,

    // Skip detection (intro/credits/recap)
    #[serde(default = "default_skip_detect_mode")]
    pub skip_detect_mode: String,
    #[serde(default = "default_vision_service_url")]
    pub vision_service_url: Option<String>,

    // IPTV configuration
    #[serde(default = "default_iptv_language_filter")]
    pub iptv_language_filter: Vec<String>,
    #[serde(default = "default_true")]
    pub iptv_auto_create_channels: bool,
    #[serde(default = "default_true")]
    pub iptv_health_check_enabled: bool,

    // Live TV (HDHomeRun tuner) configuration
    #[serde(default = "default_livetv_guide_source")]
    pub livetv_guide_source: String,
    #[serde(default = "default_livetv_buffer_policy")]
    pub livetv_buffer_policy: String,
    #[serde(default = "default_livetv_buffer_cap_hours")]
    pub livetv_buffer_cap_hours: i64,
    #[serde(default = "default_true")]
    pub livetv_buffer_thumbnails: bool,
    #[serde(default = "default_livetv_clash_policy")]
    pub livetv_clash_policy: String,
    #[serde(default = "default_livetv_ad_posture")]
    pub livetv_ad_posture: String,
    #[serde(default = "default_livetv_ad_disable_genres")]
    pub livetv_ad_disable_genres: Vec<String>,
    #[serde(default = "default_true")]
    pub livetv_ad_train_from_corrections: bool,
    #[serde(default)]
    pub livetv_recording_path: Option<String>,
    #[serde(default = "default_true")]
    pub livetv_deinterlace_web: bool,
    #[serde(default = "default_livetv_transcode_height")]
    pub livetv_transcode_height: i64,

    // Discover preferences
    #[serde(default = "default_discover_region")]
    pub discover_region: String,
    #[serde(default)]
    pub discover_streaming_services: Vec<DiscoverStreamingService>,

    // Google Cast
    #[serde(default)]
    pub cast_receiver_app_id: Option<String>,

    // Legacy fields (ignored, kept for backwards-compatible config loading)
    #[serde(default)]
    pub whisper_model_path: Option<String>,
    #[serde(default)]
    pub whisper_binary_path: Option<String>,
    #[serde(default)]
    pub whisper_model: Option<String>,
    #[serde(default)]
    pub transcribe_script_path: Option<String>,
    #[serde(default)]
    pub whisper_threads: Option<u32>,
    #[serde(default)]
    pub stt_engine: Option<String>,
    #[serde(default)]
    pub stt_device: Option<String>,
    #[serde(default)]
    pub stt_parakeet_url: Option<String>,
    #[serde(default)]
    pub tts_engine: Option<String>,
    #[serde(default)]
    pub tts_chatterbox_url: Option<String>,
    #[serde(default)]
    pub tts_chatterbox_voice: Option<String>,
}

// Default value functions for serde
fn default_kobo_storeapi_base_url() -> String {
    "https://storeapi.kobo.com".to_string()
}
fn default_kobo_mode() -> String {
    "merge".to_string()
}
fn default_true() -> bool {
    true
}
fn default_proxy_log_retention_days() -> i32 {
    7
}
fn default_kobo_sync_batch_size() -> i64 {
    100
}
fn default_activity_limit() -> i64 {
    200
}
fn default_page_size() -> i64 {
    50
}
fn default_max_page_size() -> i64 {
    200
}
fn default_entitlement_mode() -> String {
    "all_devices".to_string()
}
fn default_watcher_scan_interval() -> i32 {
    60
}
fn default_media_root_scan_interval() -> i32 {
    1800
}
fn default_metadata_cache_days() -> i32 {
    30
}
fn default_transcript_mode() -> String {
    "off".to_string()
}
fn default_preferred_language() -> String {
    "english".to_string()
}
fn default_gpu_preempt_tts() -> bool {
    true
}
fn default_auto() -> String {
    "auto".to_string()
}
fn default_skip_detect_mode() -> String {
    "off".to_string()
}
fn default_iptv_language_filter() -> Vec<String> {
    vec!["en".to_string()]
}
fn default_vision_service_url() -> Option<String> {
    Some("http://siglip2:8890".to_string())
}
fn default_discover_region() -> String {
    "US".to_string()
}
fn default_livetv_guide_source() -> String {
    "tuner".to_string()
}
fn default_livetv_buffer_policy() -> String {
    "tune_in".to_string()
}
fn default_livetv_buffer_cap_hours() -> i64 {
    3
}
fn default_livetv_clash_policy() -> String {
    "keep_playing".to_string()
}
fn default_livetv_ad_posture() -> String {
    "shade".to_string()
}
fn default_livetv_ad_disable_genres() -> Vec<String> {
    vec!["News".to_string(), "Sports".to_string()]
}
fn default_livetv_transcode_height() -> i64 {
    720
}
impl Default for FileConfig {
    fn default() -> Self {
        Self {
            public_url: None,
            public_proxy_url: String::new(),
            kobo_storeapi_base_url: default_kobo_storeapi_base_url(),
            kobo_mode: default_kobo_mode(),
            proxy_logging_enabled: true,
            proxy_log_retention_days: default_proxy_log_retention_days(),
            blob_capture_enabled: false,
            kobo_sync_batch_size: default_kobo_sync_batch_size(),
            activity_limit: default_activity_limit(),
            default_page_size: default_page_size(),
            max_page_size: default_max_page_size(),
            default_entitlement_mode: default_entitlement_mode(),
            enrichment_enabled: false,
            watcher_enabled: false,
            watcher_scan_interval_seconds: default_watcher_scan_interval(),
            media_root_scan_interval_seconds: default_media_root_scan_interval(),
            metadata_providers_enabled: true,
            metadata_cache_days: default_metadata_cache_days(),
            rewrite_kobo_urls: false,
            wanted_default_provider: None,
            wanted_min_score: None,
            kobload_base_url: None,
            kobload_api_key: None,
            kobload_ingest_url: None,
            kobload_communication_mode: None,
            meili_url: None,
            meili_master_key: None,
            koblem_service_url: None,
            koblem_api_key: None,
            chat_enabled: false,
            chat_llm_provider: None,
            chat_llm_base_url: None,
            chat_llm_api_key: None,
            chat_llm_model: None,
            chat_llm_thinking: None,
            ebook_convert_path: None,
            ebook_meta_path: None,
            sonarr_url: None,
            sonarr_api_key: None,
            radarr_url: None,
            radarr_api_key: None,
            video_storage_root: None,
            transcode_cache_dir: None,
            debrid_cache_dir: None,
            cache_dir: None,
            preferred_audio_language: default_preferred_language(),
            preferred_subtitle_language: default_preferred_language(),
            mkv_cue_trim_enabled: true,
            transcript_mode: default_transcript_mode(),
            transcribe_parallelism: None,
            stt_service_url: None,
            job_concurrency: None,
            tts_enabled: false,
            tts_service_url: None,
            tts_service_type: "auto".to_string(),
            tts_default_voice: None,
            tts_default_temperature: None,
            hw_encoder: None,
            gpu_preempt_tts: true,
            route_gpu_via_koblem: false,
            skip_detect_mode: default_skip_detect_mode(),
            vision_service_url: default_vision_service_url(),
            iptv_language_filter: default_iptv_language_filter(),
            iptv_auto_create_channels: true,
            iptv_health_check_enabled: true,
            livetv_guide_source: default_livetv_guide_source(),
            livetv_buffer_policy: default_livetv_buffer_policy(),
            livetv_buffer_cap_hours: default_livetv_buffer_cap_hours(),
            livetv_buffer_thumbnails: true,
            livetv_clash_policy: default_livetv_clash_policy(),
            livetv_ad_posture: default_livetv_ad_posture(),
            livetv_ad_disable_genres: default_livetv_ad_disable_genres(),
            livetv_ad_train_from_corrections: true,
            livetv_recording_path: None,
            livetv_deinterlace_web: true,
            livetv_transcode_height: default_livetv_transcode_height(),
            discover_region: default_discover_region(),
            discover_streaming_services: vec![],
            cast_receiver_app_id: None,
            // Legacy
            whisper_model_path: None,
            whisper_binary_path: None,
            whisper_model: None,
            transcribe_script_path: None,
            whisper_threads: None,
            stt_engine: None,
            stt_device: None,
            stt_parakeet_url: None,
            tts_engine: None,
            tts_chatterbox_url: None,
            tts_chatterbox_voice: None,
        }
    }
}

impl FileConfig {
    /// Load config from TOML file.
    pub fn load(path: &str) -> Result<Self, FileConfigError> {
        let path = Path::new(path);
        if !path.exists() {
            return Err(FileConfigError::NotFound(path.display().to_string()));
        }

        let content = fs::read_to_string(path)
            .map_err(|e| FileConfigError::ReadError(path.display().to_string(), e.to_string()))?;

        let config: FileConfig = toml::from_str(&content)
            .map_err(|e| FileConfigError::ParseError(path.display().to_string(), e.to_string()))?;

        info!("Loaded configuration from {}", path.display());
        Ok(config)
    }

    pub fn load_or_create(path: &str) -> Result<Self, FileConfigError> {
        match Self::load(path) {
            Err(FileConfigError::NotFound(_)) => {
                if let Some(parent) = Path::new(path).parent()
                    && !parent.as_os_str().is_empty()
                {
                    fs::create_dir_all(parent).map_err(|e| {
                        FileConfigError::WriteError(path.to_string(), e.to_string())
                    })?;
                }
                let config = Self::default();
                config.save(path)?;
                info!("No config at {path}; wrote defaults. Edit it or use Settings.");
                Ok(config)
            }
            other => other,
        }
    }

    /// Save config to TOML file, preserving comments if the file already exists.
    pub fn save(&self, path: &str) -> Result<(), FileConfigError> {
        let content = if Path::new(path).exists() {
            // Read existing file and update values in place to preserve comments
            let existing = fs::read_to_string(path)
                .map_err(|e| FileConfigError::ReadError(path.to_string(), e.to_string()))?;

            match existing.parse::<DocumentMut>() {
                Ok(mut doc) => {
                    // Update each field in the document
                    if let Some(ref url) = self.public_url {
                        doc["public_url"] = toml_edit::value(url);
                    } else {
                        doc.remove("public_url");
                    }
                    doc["public_proxy_url"] = toml_edit::value(&self.public_proxy_url);
                    doc["kobo_storeapi_base_url"] = toml_edit::value(&self.kobo_storeapi_base_url);
                    doc["kobo_mode"] = toml_edit::value(&self.kobo_mode);
                    doc["proxy_logging_enabled"] = toml_edit::value(self.proxy_logging_enabled);
                    doc["proxy_log_retention_days"] =
                        toml_edit::value(self.proxy_log_retention_days as i64);
                    doc["blob_capture_enabled"] = toml_edit::value(self.blob_capture_enabled);
                    doc["kobo_sync_batch_size"] = toml_edit::value(self.kobo_sync_batch_size);
                    doc["activity_limit"] = toml_edit::value(self.activity_limit);
                    doc["default_page_size"] = toml_edit::value(self.default_page_size);
                    doc["max_page_size"] = toml_edit::value(self.max_page_size);
                    doc["default_entitlement_mode"] =
                        toml_edit::value(&self.default_entitlement_mode);
                    doc["enrichment_enabled"] = toml_edit::value(self.enrichment_enabled);
                    doc["watcher_enabled"] = toml_edit::value(self.watcher_enabled);
                    doc["watcher_scan_interval_seconds"] =
                        toml_edit::value(self.watcher_scan_interval_seconds as i64);
                    doc["media_root_scan_interval_seconds"] =
                        toml_edit::value(self.media_root_scan_interval_seconds as i64);
                    doc["metadata_providers_enabled"] =
                        toml_edit::value(self.metadata_providers_enabled);
                    doc["metadata_cache_days"] = toml_edit::value(self.metadata_cache_days as i64);
                    doc["rewrite_kobo_urls"] = toml_edit::value(self.rewrite_kobo_urls);
                    doc["wanted_default_provider"] =
                        toml_edit::value(self.wanted_default_provider.as_deref().unwrap_or(""));
                    doc.remove("wanted_kobload_profile_type_ids");
                    doc.remove("wanted_kobload_profile_types");
                    doc.remove("wanted_kobload_profile_type");
                    if let Some(score) = self.wanted_min_score {
                        doc["wanted_min_score"] = toml_edit::value(score);
                    }
                    doc["kobload_base_url"] =
                        toml_edit::value(self.kobload_base_url.as_deref().unwrap_or(""));
                    doc["kobload_api_key"] =
                        toml_edit::value(self.kobload_api_key.as_deref().unwrap_or(""));
                    doc["kobload_ingest_url"] =
                        toml_edit::value(self.kobload_ingest_url.as_deref().unwrap_or(""));
                    doc["kobload_communication_mode"] =
                        toml_edit::value(self.kobload_communication_mode.as_deref().unwrap_or(""));
                    doc["meili_url"] = toml_edit::value(self.meili_url.as_deref().unwrap_or(""));
                    doc["meili_master_key"] =
                        toml_edit::value(self.meili_master_key.as_deref().unwrap_or(""));
                    // Koblem
                    doc["koblem_service_url"] =
                        toml_edit::value(self.koblem_service_url.as_deref().unwrap_or(""));
                    doc["koblem_api_key"] =
                        toml_edit::value(self.koblem_api_key.as_deref().unwrap_or(""));
                    // Chat
                    doc["chat_enabled"] = toml_edit::value(self.chat_enabled);
                    doc["chat_llm_provider"] =
                        toml_edit::value(self.chat_llm_provider.as_deref().unwrap_or(""));
                    doc["chat_llm_base_url"] =
                        toml_edit::value(self.chat_llm_base_url.as_deref().unwrap_or(""));
                    doc["chat_llm_api_key"] =
                        toml_edit::value(self.chat_llm_api_key.as_deref().unwrap_or(""));
                    doc["chat_llm_model"] =
                        toml_edit::value(self.chat_llm_model.as_deref().unwrap_or(""));
                    doc["chat_llm_thinking"] =
                        toml_edit::value(self.chat_llm_thinking.as_deref().unwrap_or(""));
                    doc["ebook_convert_path"] =
                        toml_edit::value(self.ebook_convert_path.as_deref().unwrap_or(""));
                    doc["ebook_meta_path"] =
                        toml_edit::value(self.ebook_meta_path.as_deref().unwrap_or(""));
                    doc["sonarr_url"] = toml_edit::value(self.sonarr_url.as_deref().unwrap_or(""));
                    doc["sonarr_api_key"] =
                        toml_edit::value(self.sonarr_api_key.as_deref().unwrap_or(""));
                    doc["radarr_url"] = toml_edit::value(self.radarr_url.as_deref().unwrap_or(""));
                    doc["radarr_api_key"] =
                        toml_edit::value(self.radarr_api_key.as_deref().unwrap_or(""));
                    doc["video_storage_root"] =
                        toml_edit::value(self.video_storage_root.as_deref().unwrap_or(""));
                    doc.remove("ffmpeg_path");
                    doc.remove("ffprobe_path");
                    doc["transcode_cache_dir"] =
                        toml_edit::value(self.transcode_cache_dir.as_deref().unwrap_or(""));
                    if let Some(ref dir) = self.debrid_cache_dir {
                        doc["debrid_cache_dir"] = toml_edit::value(dir);
                    }
                    doc["cache_dir"] = toml_edit::value(self.cache_dir.as_deref().unwrap_or(""));
                    doc["preferred_audio_language"] =
                        toml_edit::value(&self.preferred_audio_language);
                    doc["preferred_subtitle_language"] =
                        toml_edit::value(&self.preferred_subtitle_language);
                    doc["mkv_cue_trim_enabled"] = toml_edit::value(self.mkv_cue_trim_enabled);
                    doc["transcript_mode"] = toml_edit::value(&self.transcript_mode);
                    doc.remove("transcribe_parallelism");
                    doc["stt_service_url"] =
                        toml_edit::value(self.stt_service_url.as_deref().unwrap_or(""));

                    // Job concurrency overrides
                    if let Some(ref map) = self.job_concurrency {
                        let mut tbl = toml_edit::InlineTable::new();
                        for (k, v) in map {
                            tbl.insert(
                                k,
                                toml_edit::Value::Integer(toml_edit::Formatted::new(*v as i64)),
                            );
                        }
                        doc["job_concurrency"] =
                            toml_edit::Item::Value(toml_edit::Value::InlineTable(tbl));
                    } else {
                        doc.remove("job_concurrency");
                    }

                    // TTS
                    doc["tts_enabled"] = toml_edit::value(self.tts_enabled);
                    doc["tts_service_url"] =
                        toml_edit::value(self.tts_service_url.as_deref().unwrap_or(""));
                    doc["tts_service_type"] = toml_edit::value(&self.tts_service_type);
                    doc["tts_default_voice"] =
                        toml_edit::value(self.tts_default_voice.as_deref().unwrap_or(""));

                    // Hardware encoder
                    doc["hw_encoder"] = toml_edit::value(self.hw_encoder.as_deref().unwrap_or(""));

                    // GPU sharing
                    doc["gpu_preempt_tts"] = toml_edit::value(self.gpu_preempt_tts);
                    doc["route_gpu_via_koblem"] = toml_edit::value(self.route_gpu_via_koblem);

                    // Skip detection
                    doc["skip_detect_mode"] = toml_edit::value(&self.skip_detect_mode);
                    doc["vision_service_url"] =
                        toml_edit::value(self.vision_service_url.as_deref().unwrap_or(""));

                    // IPTV configuration
                    let mut lang_arr = toml_edit::Array::new();
                    for lang in &self.iptv_language_filter {
                        lang_arr.push(lang.as_str());
                    }
                    doc["iptv_language_filter"] = toml_edit::value(lang_arr);
                    doc["iptv_auto_create_channels"] =
                        toml_edit::value(self.iptv_auto_create_channels);
                    doc["iptv_health_check_enabled"] =
                        toml_edit::value(self.iptv_health_check_enabled);

                    // Live TV configuration
                    doc["livetv_guide_source"] = toml_edit::value(&self.livetv_guide_source);
                    doc["livetv_buffer_policy"] = toml_edit::value(&self.livetv_buffer_policy);
                    doc["livetv_buffer_cap_hours"] = toml_edit::value(self.livetv_buffer_cap_hours);
                    doc["livetv_buffer_thumbnails"] =
                        toml_edit::value(self.livetv_buffer_thumbnails);
                    doc["livetv_clash_policy"] = toml_edit::value(&self.livetv_clash_policy);
                    doc["livetv_ad_posture"] = toml_edit::value(&self.livetv_ad_posture);
                    let mut genre_arr = toml_edit::Array::new();
                    for g in &self.livetv_ad_disable_genres {
                        genre_arr.push(g.as_str());
                    }
                    doc["livetv_ad_disable_genres"] = toml_edit::value(genre_arr);
                    doc["livetv_ad_train_from_corrections"] =
                        toml_edit::value(self.livetv_ad_train_from_corrections);
                    doc["livetv_recording_path"] =
                        toml_edit::value(self.livetv_recording_path.as_deref().unwrap_or(""));
                    doc["livetv_deinterlace_web"] = toml_edit::value(self.livetv_deinterlace_web);
                    doc["livetv_transcode_height"] = toml_edit::value(self.livetv_transcode_height);

                    // Discover
                    doc["discover_region"] = toml_edit::value(&self.discover_region);
                    let mut services_arr = toml_edit::Array::new();
                    for svc in &self.discover_streaming_services {
                        let mut tbl = toml_edit::InlineTable::new();
                        tbl.insert("id", toml_edit::Value::from(i64::from(svc.id)));
                        tbl.insert("name", toml_edit::Value::from(&*svc.name));
                        services_arr.push(toml_edit::Value::InlineTable(tbl));
                    }
                    doc["discover_streaming_services"] =
                        toml_edit::Item::Value(toml_edit::Value::Array(services_arr));

                    // Google Cast
                    if let Some(ref app_id) = self.cast_receiver_app_id {
                        doc["cast_receiver_app_id"] = toml_edit::value(app_id);
                    } else {
                        doc.remove("cast_receiver_app_id");
                    }

                    // Remove legacy fields
                    doc.remove("whisper_model_path");
                    doc.remove("whisper_binary_path");
                    doc.remove("whisper_model");
                    doc.remove("transcribe_script_path");
                    doc.remove("whisper_threads");
                    doc.remove("stt_engine");
                    doc.remove("stt_device");
                    doc.remove("stt_parakeet_url");
                    doc.remove("tts_engine");
                    doc.remove("tts_chatterbox_url");
                    doc.remove("tts_chatterbox_voice");

                    doc.to_string()
                }
                Err(_) => {
                    // Couldn't parse existing file, fall back to generating new content
                    self.to_toml_with_comments()
                }
            }
        } else {
            // File doesn't exist, generate new content with comments
            self.to_toml_with_comments()
        };

        fs::write(path, content)
            .map_err(|e| FileConfigError::WriteError(path.to_string(), e.to_string()))?;

        info!("Saved configuration to {}", path);
        Ok(())
    }

    /// Generate TOML content with section comments.
    fn to_toml_with_comments(&self) -> String {
        let mut output = String::new();

        output.push_str("# Kobbler Configuration\n");
        output.push_str("# Edit this file before first run to configure your instance.\n");
        output.push_str("# Changes made in Settings UI are saved here automatically.\n\n");

        // Public URLs section
        output.push_str(
            "# =============================================================================\n",
        );
        output.push_str("# Public URLs\n");
        output.push_str(
            "# =============================================================================\n\n",
        );
        output.push_str(
            "# Externally reachable base URL for this Kobbler, used wherever a link has to\n",
        );
        output.push_str("# work outside the app (chat citations, for example). Optional.\n");
        output.push_str("# Example: \"https://kobbler.example.com\" (no trailing slash)\n");
        match self.public_url {
            Some(ref url) => output.push_str(&format!("public_url = {:?}\n\n", url)),
            None => output.push_str("# public_url = \"https://kobbler.example.com\"\n\n"),
        }

        output.push_str("# Public URL for Kobo device sync, including mount point\n");
        output.push_str("# Set this to your server's externally-accessible URL\n");
        output.push_str(
            "# Example: \"https://kobbler.example.com/kobo\" or \"http://192.168.1.100:6767\"\n",
        );
        output.push_str(&format!(
            "public_proxy_url = {:?}\n\n",
            self.public_proxy_url
        ));

        // Wanted section
        output.push_str(
            "# =============================================================================\n",
        );
        output.push_str("# Wanted\n");
        output.push_str(
            "# =============================================================================\n\n",
        );
        output.push_str("# Default metadata provider for Wanted searches (optional)\n");
        output.push_str(&format!(
            "wanted_default_provider = {:?}\n",
            self.wanted_default_provider.as_deref().unwrap_or("")
        ));
        output.push_str(
            "# Minimum score for fuzzy matching in watch patterns (0.0-1.0, default 0.65)\n",
        );
        if let Some(score) = self.wanted_min_score {
            output.push_str(&format!("wanted_min_score = {}\n", score));
        } else {
            output.push_str("# wanted_min_score = 0.65\n");
        }
        output.push_str("# Kobload base URL (optional)\n");
        output.push_str(&format!(
            "kobload_base_url = {:?}\n",
            self.kobload_base_url.as_deref().unwrap_or("")
        ));
        output.push_str("# Kobload API key (optional)\n");
        output.push_str(&format!(
            "kobload_api_key = {:?}\n",
            self.kobload_api_key.as_deref().unwrap_or("")
        ));
        output.push_str("# Full ingest URL for Kobload callbacks (e.g., http://kobbler:6766/api/v1/ingest/token)\n");
        output.push_str(&format!(
            "kobload_ingest_url = {:?}\n\n",
            self.kobload_ingest_url.as_deref().unwrap_or("")
        ));

        output.push_str("# Kobo store API URL (for merge/passthrough modes)\n");
        output.push_str(&format!(
            "kobo_storeapi_base_url = {:?}\n\n",
            self.kobo_storeapi_base_url
        ));

        // Kobo Sync Mode section
        output.push_str(
            "# =============================================================================\n",
        );
        output.push_str("# Kobo Sync Mode\n");
        output.push_str(
            "# =============================================================================\n\n",
        );
        output.push_str("# Kobo proxy mode: merge, passthrough, or local\n");
        output.push_str(
            "#   merge       - Combine your books with Kobo store purchases (recommended)\n",
        );
        output.push_str("#   passthrough - Pure proxy, just forward to Kobo store\n");
        output.push_str("#   local       - Local only, never call Kobo servers\n");
        output.push_str(&format!("kobo_mode = {:?}\n\n", self.kobo_mode));

        // Proxy Logging section
        output.push_str(
            "# =============================================================================\n",
        );
        output.push_str("# Proxy Logging\n");
        output.push_str(
            "# =============================================================================\n\n",
        );
        output.push_str("# Enable logging of all Kobo proxy requests (useful for debugging)\n");
        output.push_str(&format!(
            "proxy_logging_enabled = {}\n\n",
            self.proxy_logging_enabled
        ));
        output.push_str("# Number of days to keep proxy logs before cleanup\n");
        output.push_str(&format!(
            "proxy_log_retention_days = {}\n\n",
            self.proxy_log_retention_days
        ));
        output.push_str("# Enable capturing binary blobs (request/response bodies)\n");
        output.push_str("# Warning: This can use significant disk space\n");
        output.push_str(&format!(
            "blob_capture_enabled = {}\n\n",
            self.blob_capture_enabled
        ));

        // Pagination & Limits section
        output.push_str(
            "# =============================================================================\n",
        );
        output.push_str("# Pagination & Limits\n");
        output.push_str(
            "# =============================================================================\n\n",
        );
        output.push_str("# Maximum books to sync per batch (Kobo protocol batching)\n");
        output.push_str(&format!(
            "kobo_sync_batch_size = {}\n\n",
            self.kobo_sync_batch_size
        ));
        output.push_str("# Maximum activity entries to keep\n");
        output.push_str(&format!("activity_limit = {}\n\n", self.activity_limit));
        output.push_str("# Default and maximum page sizes for API responses\n");
        output.push_str(&format!("default_page_size = {}\n", self.default_page_size));
        output.push_str(&format!("max_page_size = {}\n\n", self.max_page_size));

        // Book Entitlements section
        output.push_str(
            "# =============================================================================\n",
        );
        output.push_str("# Book Entitlements\n");
        output.push_str(
            "# =============================================================================\n\n",
        );
        output.push_str("# Default entitlement mode for new books\n");
        output.push_str("#   all_devices - New books are available to all devices (recommended)\n");
        output.push_str("#   none        - New books must be manually assigned to devices\n");
        output.push_str(&format!(
            "default_entitlement_mode = {:?}\n\n",
            self.default_entitlement_mode
        ));

        // Media Enrichment section
        output.push_str(
            "# =============================================================================\n",
        );
        output.push_str("# Media Enrichment\n");
        output.push_str(
            "# =============================================================================\n\n",
        );
        output.push_str("# Enable background media enrichment (thumbnails, LUFS)\n");
        output.push_str(&format!(
            "enrichment_enabled = {}\n",
            self.enrichment_enabled
        ));

        // Directory Watcher section
        output.push_str(
            "# =============================================================================\n",
        );
        output.push_str("# Directory Watcher\n");
        output.push_str(
            "# =============================================================================\n\n",
        );
        output.push_str("# Enable automatic directory watching for new books\n");
        output.push_str(&format!("watcher_enabled = {}\n\n", self.watcher_enabled));
        output.push_str("# Interval for directory scanning in seconds\n");
        output.push_str("# (used as fallback when filesystem events aren't available)\n");
        output.push_str(&format!(
            "watcher_scan_interval_seconds = {}\n\n",
            self.watcher_scan_interval_seconds
        ));
        output.push_str(
            "# Default interval (seconds) between periodic media-root filesystem scans.\n",
        );
        output.push_str("# Safety net — real-time ingest uses kobload + the filesystem watcher.\n");
        output.push_str("# Per-root overrides live on the media root edit form.\n");
        output.push_str(&format!(
            "media_root_scan_interval_seconds = {}\n\n",
            self.media_root_scan_interval_seconds
        ));

        // Metadata Providers section
        output.push_str(
            "# =============================================================================\n",
        );
        output.push_str("# Metadata Providers\n");
        output.push_str(
            "# =============================================================================\n\n",
        );
        output.push_str("# Enable automatic metadata lookups from external providers\n");
        output.push_str("# (OpenLibrary, Google Books, Hardcover)\n");
        output.push_str(&format!(
            "metadata_providers_enabled = {}\n\n",
            self.metadata_providers_enabled
        ));
        output.push_str("# Number of days to cache metadata lookup results\n");
        output.push_str(&format!(
            "metadata_cache_days = {}\n\n",
            self.metadata_cache_days
        ));

        // URL Rewriting section
        output.push_str(
            "# =============================================================================\n",
        );
        output.push_str("# URL Rewriting\n");
        output.push_str(
            "# =============================================================================\n\n",
        );
        output.push_str("# Rewrite external Kobo URLs to go through our proxy\n");
        output.push_str(
            "# Useful for debugging to see all traffic, but may not be needed in production\n",
        );
        output.push_str(&format!(
            "rewrite_kobo_urls = {}\n\n",
            self.rewrite_kobo_urls
        ));

        // MeiliSearch section
        output.push_str(
            "# =============================================================================\n",
        );
        output.push_str("# Search (MeiliSearch)\n");
        output.push_str(
            "# =============================================================================\n\n",
        );
        output.push_str("# Meilisearch URL. Leave empty to search with SQL queries instead,\n");
        output.push_str("# which needs no Meilisearch at all.\n");
        output.push_str(&format!(
            "meili_url = {:?}\n",
            self.meili_url.as_deref().unwrap_or("")
        ));
        output.push_str("# MeiliSearch master key (optional)\n");
        output.push_str(&format!(
            "meili_master_key = {:?}\n\n",
            self.meili_master_key.as_deref().unwrap_or("")
        ));

        // Chat (AI Assistant) section
        output.push_str(
            "# =============================================================================\n",
        );
        output.push_str("# Koblem (Local LLM Service)\n");
        output.push_str(
            "# =============================================================================\n\n",
        );
        output.push_str("# Koblem service URL (e.g., http://koblem:8090)\n");
        output.push_str(&format!(
            "koblem_service_url = {:?}\n\n",
            self.koblem_service_url.as_deref().unwrap_or("")
        ));
        output.push_str("# Koblem API key (required for tier-2 endpoints)\n");
        output.push_str(&format!(
            "koblem_api_key = {:?}\n\n",
            self.koblem_api_key.as_deref().unwrap_or("")
        ));

        output.push_str("# Chat (AI Assistant)\n");
        output.push_str(
            "# =============================================================================\n\n",
        );
        output.push_str("# Enable conversational AI search (requires LLM provider or koblem)\n");
        output.push_str(&format!("chat_enabled = {}\n", self.chat_enabled));
        output.push_str("# LLM provider: \"gemini\", \"openai\", \"custom\"\n");
        output.push_str(&format!(
            "chat_llm_provider = {:?}\n",
            self.chat_llm_provider.as_deref().unwrap_or("")
        ));
        output.push_str("# Base URL for the LLM API\n");
        output.push_str(&format!(
            "chat_llm_base_url = {:?}\n",
            self.chat_llm_base_url.as_deref().unwrap_or("")
        ));
        output.push_str("# API key for the LLM provider\n");
        output.push_str(&format!(
            "chat_llm_api_key = {:?}\n",
            self.chat_llm_api_key.as_deref().unwrap_or("")
        ));
        output.push_str("# Model name (e.g., \"gemini-2.0-flash\", \"gpt-4o-mini\")\n");
        output.push_str(&format!(
            "chat_llm_model = {:?}\n\n",
            self.chat_llm_model.as_deref().unwrap_or("")
        ));
        output.push_str("# Thinking level for the Koblem chat route (\"\" = auto)\n");
        output.push_str(&format!(
            "chat_llm_thinking = {:?}\n\n",
            self.chat_llm_thinking.as_deref().unwrap_or("")
        ));

        // Conversion Tools section
        output.push_str(
            "# =============================================================================\n",
        );
        output.push_str("# Conversion Tools\n");
        output.push_str(
            "# =============================================================================\n\n",
        );
        output.push_str("# Paths to conversion tools (leave empty to search PATH).\n");
        output.push_str("# EPUB/MOBI/AZW3 -> EPUB and EPUB -> KEPUB are now in-process via\n");
        output.push_str("# kob-ebook (boko + native kepubifier); ebook-convert is only the\n");
        output.push_str("# fallback for PDF/FB2/RTF/TXT/HTML/DOCX/LIT/PDB.\n");
        output.push_str("# ebook-convert: Format conversion (Calibre)\n");
        output.push_str(&format!(
            "ebook_convert_path = {:?}\n",
            self.ebook_convert_path.as_deref().unwrap_or("")
        ));
        output.push_str("# ebook-meta: Metadata extraction (Calibre)\n");
        output.push_str(&format!(
            "ebook_meta_path = {:?}\n",
            self.ebook_meta_path.as_deref().unwrap_or("")
        ));

        // Migration section
        output.push_str(
            "\n# =============================================================================\n",
        );
        output.push_str("# Migration (Sonarr/Radarr)\n");
        output.push_str(
            "# =============================================================================\n\n",
        );
        output.push_str("# Sonarr URL and API key (for one-time migration)\n");
        output.push_str(&format!(
            "sonarr_url = {:?}\n",
            self.sonarr_url.as_deref().unwrap_or("")
        ));
        output.push_str(&format!(
            "sonarr_api_key = {:?}\n",
            self.sonarr_api_key.as_deref().unwrap_or("")
        ));
        output.push_str("# Radarr URL and API key (for one-time migration)\n");
        output.push_str(&format!(
            "radarr_url = {:?}\n",
            self.radarr_url.as_deref().unwrap_or("")
        ));
        output.push_str(&format!(
            "radarr_api_key = {:?}\n",
            self.radarr_api_key.as_deref().unwrap_or("")
        ));

        // Video Storage section
        output.push_str(
            "\n# =============================================================================\n",
        );
        output.push_str("# Video Storage\n");
        output.push_str(
            "# =============================================================================\n\n",
        );
        output.push_str(
            "# Root directory for video files (leave empty to use {storage_root}/video)\n",
        );
        output.push_str(&format!(
            "video_storage_root = {:?}\n",
            self.video_storage_root.as_deref().unwrap_or("")
        ));

        // Streaming / Transcoding section
        output.push_str(
            "\n# =============================================================================\n",
        );
        output.push_str("# Streaming / Transcoding\n");
        output.push_str(
            "# =============================================================================\n\n",
        );
        output.push_str(
            "# Directory for temporary transcode segments (leave empty for {storage_root}/cache/transcode)\n",
        );
        output.push_str(&format!(
            "transcode_cache_dir = {:?}\n",
            self.transcode_cache_dir.as_deref().unwrap_or("")
        ));
        output.push_str(
            "# General-purpose cache directory (skip detection, etc). Leave empty for {storage_root}/cache\n",
        );
        output.push_str(&format!(
            "cache_dir = {:?}\n",
            self.cache_dir.as_deref().unwrap_or("")
        ));

        // Transcription section
        output.push_str(
            "\n# =============================================================================\n",
        );
        output.push_str("# Speech-to-Text\n");
        output.push_str(
            "# =============================================================================\n\n",
        );
        output.push_str("# Transcription mode: \"off\" (disabled), \"auto\" (on ingest), \"manual\" (on demand)\n");
        output.push_str(&format!("transcript_mode = {:?}\n", self.transcript_mode));
        output.push_str("# OpenAI-compatible STT service URL (e.g., http://stt:8880)\n");
        output.push_str(&format!(
            "stt_service_url = {:?}\n",
            self.stt_service_url.as_deref().unwrap_or("")
        ));

        // TTS section
        output.push_str(
            "\n# =============================================================================\n",
        );
        output.push_str("# Text-to-Speech\n");
        output.push_str(
            "# =============================================================================\n\n",
        );
        output.push_str("# Enable TTS read-along feature\n");
        output.push_str(&format!("tts_enabled = {}\n", self.tts_enabled));
        output.push_str("# OpenAI-compatible TTS service URL (e.g., http://tts:8880)\n");
        output.push_str(&format!(
            "tts_service_url = {:?}\n",
            self.tts_service_url.as_deref().unwrap_or("")
        ));
        output.push_str("# TTS service type: auto, kokoro, chatterbox, openai\n");
        output.push_str(&format!("tts_service_type = {:?}\n", self.tts_service_type));
        output.push_str("# Default TTS voice ID (e.g., \"af_heart\")\n");
        output.push_str(&format!(
            "tts_default_voice = {:?}\n",
            self.tts_default_voice.as_deref().unwrap_or("")
        ));

        // Hardware encoder section
        output.push_str(
            "\n# =============================================================================\n",
        );
        output.push_str("# Hardware Encoder\n");
        output.push_str(
            "# =============================================================================\n\n",
        );
        output.push_str("# Hardware encoder preference: \"auto\" (default), \"vaapi\", \"nvenc\", or \"none\"\n");
        output.push_str("# Requires restart to take effect\n");
        output.push_str(&format!(
            "hw_encoder = {:?}\n",
            self.hw_encoder.as_deref().unwrap_or("")
        ));
        output.push_str("\n# Whether TTS preempts STT when both share a GPU (default: true)\n");
        output.push_str("# Set to false if you have enough VRAM to run TTS + STT concurrently\n");
        output.push_str(&format!("gpu_preempt_tts = {}\n", self.gpu_preempt_tts));
        output.push_str("\n# Route GPU-service calls (vision/TTS/STT) through koblem's shared\n");
        output.push_str("# priority lock instead of kobbler's local GpuLock. Set to false for\n");
        output.push_str("# standalone kobbler deployments (no koblem).\n");
        output.push_str(&format!(
            "route_gpu_via_koblem = {}\n",
            self.route_gpu_via_koblem
        ));

        // Job concurrency section
        output.push_str(
            "\n# =============================================================================\n",
        );
        output.push_str("# Job Concurrency\n");
        output.push_str(
            "# =============================================================================\n\n",
        );
        output.push_str("# Per-job-type concurrency overrides. Values must be 1-32.\n");
        output.push_str("# Example: { media_analyze = 1, media_probe = 8 }\n");
        if let Some(ref map) = self.job_concurrency {
            let entries: Vec<String> = map.iter().map(|(k, v)| format!("{k} = {v}")).collect();
            output.push_str(&format!("job_concurrency = {{ {} }}\n", entries.join(", ")));
        } else {
            output.push_str("# job_concurrency = {}\n");
        }

        // Discover section
        output.push_str(
            "\n# =============================================================================\n",
        );
        output.push_str("# Discover\n");
        output.push_str(
            "# =============================================================================\n\n",
        );
        output.push_str("# Region code for streaming service discover (e.g. US, GB, AU)\n");
        output.push_str(&format!("discover_region = {:?}\n", self.discover_region));
        output.push_str("# Streaming services are managed through the UI\n");

        output
    }

    /// Convert to RuntimeConfig.
    pub fn to_runtime_config(&self) -> RuntimeConfig {
        let wanted_default_provider = self.wanted_default_provider.as_ref().and_then(|val| {
            if val.is_empty() {
                None
            } else {
                Some(val.clone())
            }
        });
        let kobload_base_url = self.kobload_base_url.as_ref().and_then(|val| {
            if val.is_empty() {
                None
            } else {
                Some(val.clone())
            }
        });
        let kobload_api_key = self.kobload_api_key.as_ref().and_then(|val| {
            if val.is_empty() {
                None
            } else {
                Some(val.clone())
            }
        });
        let kobload_ingest_url = self.kobload_ingest_url.as_ref().and_then(|val| {
            if val.is_empty() {
                None
            } else {
                Some(val.clone())
            }
        });
        let meili_url = self
            .meili_url
            .as_ref()
            .and_then(|val| {
                if val.is_empty() {
                    None
                } else {
                    Some(val.clone())
                }
            })
            .or_else(|| std::env::var("MEILI_URL").ok());
        let meili_master_key = self
            .meili_master_key
            .as_ref()
            .and_then(|val| {
                if val.is_empty() {
                    None
                } else {
                    Some(val.clone())
                }
            })
            .or_else(|| std::env::var("MEILI_MASTER_KEY").ok());
        RuntimeConfig {
            public_proxy_url: self.public_proxy_url.clone(),
            kobo_storeapi_base_url: self.kobo_storeapi_base_url.clone(),
            kobo_mode: KoboMode::from_str(&self.kobo_mode).unwrap_or_default(),
            proxy_logging_enabled: self.proxy_logging_enabled,
            proxy_log_retention_days: self.proxy_log_retention_days,
            blob_capture_enabled: self.blob_capture_enabled,
            kobo_sync_batch_size: self.kobo_sync_batch_size,
            activity_limit: self.activity_limit,
            default_page_size: self.default_page_size,
            max_page_size: self.max_page_size,
            default_entitlement_mode: self.default_entitlement_mode.clone(),
            enrichment_enabled: self.enrichment_enabled,
            watcher_enabled: self.watcher_enabled,
            watcher_scan_interval_seconds: self.watcher_scan_interval_seconds,
            media_root_scan_interval_seconds: self.media_root_scan_interval_seconds,
            metadata_providers_enabled: self.metadata_providers_enabled,
            metadata_cache_days: self.metadata_cache_days,
            rewrite_kobo_urls: self.rewrite_kobo_urls,
            wanted_default_provider,
            wanted_min_score: self.wanted_min_score,
            kobload_base_url,
            kobload_api_key,
            kobload_ingest_url,
            kobload_communication_mode: self.kobload_communication_mode.as_ref().and_then(|val| {
                if val.is_empty() {
                    None
                } else {
                    Some(val.clone())
                }
            }),
            meili_url,
            meili_master_key,
            koblem_service_url: self.koblem_service_url.as_ref().and_then(|val| {
                if val.is_empty() {
                    None
                } else {
                    Some(val.clone())
                }
            }),
            koblem_api_key: self.koblem_api_key.as_ref().and_then(|val| {
                if val.is_empty() {
                    None
                } else {
                    Some(val.clone())
                }
            }),
            chat_enabled: self.chat_enabled,
            chat_llm_provider: self.chat_llm_provider.as_ref().and_then(|val| {
                if val.is_empty() {
                    None
                } else {
                    Some(val.clone())
                }
            }),
            chat_llm_base_url: self.chat_llm_base_url.as_ref().and_then(|val| {
                if val.is_empty() {
                    None
                } else {
                    Some(val.clone())
                }
            }),
            chat_llm_api_key: self.chat_llm_api_key.as_ref().and_then(|val| {
                if val.is_empty() {
                    None
                } else {
                    Some(val.clone())
                }
            }),
            chat_llm_model: self.chat_llm_model.as_ref().and_then(|val| {
                if val.is_empty() {
                    None
                } else {
                    Some(val.clone())
                }
            }),
            chat_llm_thinking: self.chat_llm_thinking.as_ref().and_then(|val| {
                if val.is_empty() {
                    None
                } else {
                    Some(val.clone())
                }
            }),
            ebook_convert_path: self.ebook_convert_path.clone(),
            ebook_meta_path: self.ebook_meta_path.clone(),
            sonarr_url: self.sonarr_url.as_ref().and_then(|val| {
                if val.is_empty() {
                    None
                } else {
                    Some(val.clone())
                }
            }),
            sonarr_api_key: self.sonarr_api_key.as_ref().and_then(|val| {
                if val.is_empty() {
                    None
                } else {
                    Some(val.clone())
                }
            }),
            radarr_url: self.radarr_url.as_ref().and_then(|val| {
                if val.is_empty() {
                    None
                } else {
                    Some(val.clone())
                }
            }),
            radarr_api_key: self.radarr_api_key.as_ref().and_then(|val| {
                if val.is_empty() {
                    None
                } else {
                    Some(val.clone())
                }
            }),
            video_storage_root: self.video_storage_root.as_ref().and_then(|val| {
                if val.is_empty() {
                    None
                } else {
                    Some(val.clone())
                }
            }),
            transcode_cache_dir: self.transcode_cache_dir.as_ref().and_then(|val| {
                if val.is_empty() {
                    None
                } else {
                    Some(val.clone())
                }
            }),
            cache_dir: self.cache_dir.as_ref().and_then(|val| {
                if val.is_empty() {
                    None
                } else {
                    Some(val.clone())
                }
            }),
            transcript_mode: self.transcript_mode.clone(),
            stt_service_url: self.stt_service_url.as_ref().and_then(|val| {
                if val.is_empty() {
                    None
                } else {
                    Some(val.clone())
                }
            }),
            tts_enabled: self.tts_enabled,
            tts_service_url: self.tts_service_url.as_ref().and_then(|val| {
                if val.is_empty() {
                    None
                } else {
                    Some(val.clone())
                }
            }),
            tts_service_type: if self.tts_service_type.is_empty() {
                "auto".to_string()
            } else {
                self.tts_service_type.clone()
            },
            tts_default_voice: self.tts_default_voice.as_ref().and_then(|val| {
                if val.is_empty() {
                    None
                } else {
                    Some(val.clone())
                }
            }),
            tts_default_temperature: self.tts_default_temperature,
            hw_encoder: self.hw_encoder.as_ref().and_then(|val| {
                if val.is_empty() {
                    None
                } else {
                    Some(val.clone())
                }
            }),
            gpu_preempt_tts: self.gpu_preempt_tts,
            route_gpu_via_koblem: self.route_gpu_via_koblem,
            skip_detect_mode: self.skip_detect_mode.clone(),
            vision_service_url: self.vision_service_url.as_ref().and_then(|val| {
                if val.is_empty() {
                    None
                } else {
                    Some(val.clone())
                }
            }),
            iptv_language_filter: self.iptv_language_filter.clone(),
            iptv_auto_create_channels: self.iptv_auto_create_channels,
            iptv_health_check_enabled: self.iptv_health_check_enabled,
            livetv_guide_source: self.livetv_guide_source.clone(),
            livetv_buffer_policy: self.livetv_buffer_policy.clone(),
            livetv_buffer_cap_hours: self.livetv_buffer_cap_hours,
            livetv_buffer_thumbnails: self.livetv_buffer_thumbnails,
            livetv_clash_policy: self.livetv_clash_policy.clone(),
            livetv_ad_posture: self.livetv_ad_posture.clone(),
            livetv_ad_disable_genres: self.livetv_ad_disable_genres.clone(),
            livetv_ad_train_from_corrections: self.livetv_ad_train_from_corrections,
            livetv_recording_path: self
                .livetv_recording_path
                .as_ref()
                .filter(|v| !v.is_empty())
                .cloned(),
            livetv_deinterlace_web: self.livetv_deinterlace_web,
            livetv_transcode_height: self.livetv_transcode_height,
            job_concurrency: {
                // Merge legacy transcribe_parallelism into job_concurrency map
                let mut map = self.job_concurrency.clone().unwrap_or_default();
                if let Some(p) = self.transcribe_parallelism {
                    map.entry("audiobook_transcribe".to_string()).or_insert(p);
                }
                if map.is_empty() { None } else { Some(map) }
            },
            debrid_cache_dir: self.debrid_cache_dir.clone(),
            debrid_cache_ttl_secs: 7 * 24 * 3600,
            debrid_cache_max_bytes: 50 * 1024 * 1024 * 1024,
            preferred_audio_language: self.preferred_audio_language.clone(),
            preferred_subtitle_language: self.preferred_subtitle_language.clone(),
            mkv_cue_trim_enabled: self.mkv_cue_trim_enabled,
            discover_region: self.discover_region.clone(),
            discover_streaming_services: self.discover_streaming_services.clone(),
            cast_receiver_app_id: self.cast_receiver_app_id.clone(),
            public_url: self.public_url.clone(),
        }
    }

    /// Create from RuntimeConfig.
    pub fn from_runtime_config(config: &RuntimeConfig) -> Self {
        Self {
            public_url: config.public_url.clone(),
            public_proxy_url: config.public_proxy_url.clone(),
            kobo_storeapi_base_url: config.kobo_storeapi_base_url.clone(),
            kobo_mode: config.kobo_mode.to_string(),
            proxy_logging_enabled: config.proxy_logging_enabled,
            proxy_log_retention_days: config.proxy_log_retention_days,
            blob_capture_enabled: config.blob_capture_enabled,
            kobo_sync_batch_size: config.kobo_sync_batch_size,
            activity_limit: config.activity_limit,
            default_page_size: config.default_page_size,
            max_page_size: config.max_page_size,
            default_entitlement_mode: config.default_entitlement_mode.clone(),
            enrichment_enabled: config.enrichment_enabled,
            watcher_enabled: config.watcher_enabled,
            watcher_scan_interval_seconds: config.watcher_scan_interval_seconds,
            media_root_scan_interval_seconds: config.media_root_scan_interval_seconds,
            metadata_providers_enabled: config.metadata_providers_enabled,
            metadata_cache_days: config.metadata_cache_days,
            rewrite_kobo_urls: config.rewrite_kobo_urls,
            wanted_default_provider: config.wanted_default_provider.clone(),
            wanted_min_score: config.wanted_min_score,
            kobload_base_url: config.kobload_base_url.clone(),
            kobload_api_key: config.kobload_api_key.clone(),
            kobload_ingest_url: config.kobload_ingest_url.clone(),
            kobload_communication_mode: config.kobload_communication_mode.clone(),
            meili_url: config.meili_url.clone(),
            meili_master_key: config.meili_master_key.clone(),
            koblem_service_url: config.koblem_service_url.clone(),
            koblem_api_key: config.koblem_api_key.clone(),
            chat_enabled: config.chat_enabled,
            chat_llm_provider: config.chat_llm_provider.clone(),
            chat_llm_base_url: config.chat_llm_base_url.clone(),
            chat_llm_api_key: config.chat_llm_api_key.clone(),
            chat_llm_model: config.chat_llm_model.clone(),
            chat_llm_thinking: config.chat_llm_thinking.clone(),
            ebook_convert_path: config.ebook_convert_path.clone(),
            ebook_meta_path: config.ebook_meta_path.clone(),
            sonarr_url: config.sonarr_url.clone(),
            sonarr_api_key: config.sonarr_api_key.clone(),
            radarr_url: config.radarr_url.clone(),
            radarr_api_key: config.radarr_api_key.clone(),
            video_storage_root: config.video_storage_root.clone(),
            transcode_cache_dir: config.transcode_cache_dir.clone(),
            debrid_cache_dir: config.debrid_cache_dir.clone(),
            cache_dir: config.cache_dir.clone(),
            preferred_audio_language: config.preferred_audio_language.clone(),
            preferred_subtitle_language: config.preferred_subtitle_language.clone(),
            mkv_cue_trim_enabled: config.mkv_cue_trim_enabled,
            discover_region: config.discover_region.clone(),
            discover_streaming_services: config.discover_streaming_services.clone(),
            cast_receiver_app_id: config.cast_receiver_app_id.clone(),
            transcript_mode: config.transcript_mode.clone(),
            transcribe_parallelism: None,
            stt_service_url: config.stt_service_url.clone(),
            job_concurrency: config.job_concurrency.clone(),
            tts_enabled: config.tts_enabled,
            tts_service_url: config.tts_service_url.clone(),
            tts_service_type: config.tts_service_type.clone(),
            tts_default_voice: config.tts_default_voice.clone(),
            tts_default_temperature: config.tts_default_temperature,
            hw_encoder: config.hw_encoder.clone(),
            gpu_preempt_tts: config.gpu_preempt_tts,
            route_gpu_via_koblem: config.route_gpu_via_koblem,
            skip_detect_mode: config.skip_detect_mode.clone(),
            vision_service_url: config.vision_service_url.clone(),
            iptv_language_filter: config.iptv_language_filter.clone(),
            iptv_auto_create_channels: config.iptv_auto_create_channels,
            iptv_health_check_enabled: config.iptv_health_check_enabled,
            livetv_guide_source: config.livetv_guide_source.clone(),
            livetv_buffer_policy: config.livetv_buffer_policy.clone(),
            livetv_buffer_cap_hours: config.livetv_buffer_cap_hours,
            livetv_buffer_thumbnails: config.livetv_buffer_thumbnails,
            livetv_clash_policy: config.livetv_clash_policy.clone(),
            livetv_ad_posture: config.livetv_ad_posture.clone(),
            livetv_ad_disable_genres: config.livetv_ad_disable_genres.clone(),
            livetv_ad_train_from_corrections: config.livetv_ad_train_from_corrections,
            livetv_recording_path: config.livetv_recording_path.clone(),
            livetv_deinterlace_web: config.livetv_deinterlace_web,
            livetv_transcode_height: config.livetv_transcode_height,
            // Legacy
            whisper_model_path: None,
            whisper_binary_path: None,
            whisper_model: None,
            transcribe_script_path: None,
            whisper_threads: None,
            stt_engine: None,
            stt_device: None,
            stt_parakeet_url: None,
            tts_engine: None,
            tts_chatterbox_url: None,
            tts_chatterbox_voice: None,
        }
    }
}

/// Errors that can occur when working with the config file.
#[derive(Debug, thiserror::Error)]
pub enum FileConfigError {
    #[error("Config file not found: {0}")]
    NotFound(String),
    #[error("Failed to read config file {0}: {1}")]
    ReadError(String, String),
    #[error("Failed to parse config file {0}: {1}")]
    ParseError(String, String),
    #[error("Failed to write config file {0}: {1}")]
    WriteError(String, String),
}

impl std::fmt::Display for KoboMode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            KoboMode::Merge => write!(f, "merge"),
            KoboMode::Passthrough => write!(f, "passthrough"),
            KoboMode::Local => write!(f, "local"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn test_file_config_round_trip() {
        let temp_dir = TempDir::new().unwrap();
        let path = temp_dir.path().join("kobbler.toml");
        let path_str = path.to_str().unwrap();

        let config = FileConfig {
            public_proxy_url: "https://example.com/kobo".to_string(),
            kobo_mode: "local".to_string(),
            proxy_logging_enabled: false,
            ..Default::default()
        };

        config.save(path_str).unwrap();

        let loaded = FileConfig::load(path_str).unwrap();
        assert_eq!(loaded.public_proxy_url, "https://example.com/kobo");
        assert_eq!(loaded.kobo_mode, "local");
        assert!(!loaded.proxy_logging_enabled);
    }

    #[test]
    fn public_url_survives_a_save_and_reload_and_can_be_cleared() {
        let temp_dir = TempDir::new().unwrap();
        let path = temp_dir.path().join("kobbler.toml");
        let path_str = path.to_str().unwrap();

        FileConfig {
            public_url: Some("https://kobbler.example.com".to_string()),
            ..Default::default()
        }
        .save(path_str)
        .unwrap();
        assert_eq!(
            FileConfig::load(path_str).unwrap().public_url.as_deref(),
            Some("https://kobbler.example.com")
        );

        // Clearing has to remove the key, not persist an empty string that would
        // read back as "configured".
        FileConfig {
            public_url: None,
            ..Default::default()
        }
        .save(path_str)
        .unwrap();
        assert!(FileConfig::load(path_str).unwrap().public_url.is_none());
        assert!(
            !std::fs::read_to_string(path_str)
                .unwrap()
                .contains("public_url =")
        );
    }

    #[test]
    fn a_missing_config_file_is_created_from_defaults_instead_of_failing() {
        let temp_dir = TempDir::new().unwrap();
        let path = temp_dir.path().join("fresh").join("kobbler.toml");
        let path_str = path.to_str().unwrap();

        let config = FileConfig::load_or_create(path_str).unwrap();

        assert!(
            path.exists(),
            "first run must write the file it just defaulted"
        );
        assert!(!config.tts_enabled);
        assert!(!config.chat_enabled);
        assert!(!config.route_gpu_via_koblem);

        let reloaded = FileConfig::load(path_str).unwrap();
        assert_eq!(reloaded.kobo_mode, config.kobo_mode);
    }

    #[test]
    fn load_or_create_leaves_an_existing_config_untouched() {
        let temp_dir = TempDir::new().unwrap();
        let path = temp_dir.path().join("kobbler.toml");
        let path_str = path.to_str().unwrap();

        FileConfig {
            kobo_mode: "local".to_string(),
            ..Default::default()
        }
        .save(path_str)
        .unwrap();

        assert_eq!(
            FileConfig::load_or_create(path_str).unwrap().kobo_mode,
            "local"
        );
    }

    #[test]
    fn test_file_config_not_found() {
        let result = FileConfig::load("/nonexistent/path/kobbler.toml");
        assert!(matches!(result, Err(FileConfigError::NotFound(_))));
    }

    #[test]
    fn test_file_config_to_runtime_config() {
        let file_config = FileConfig {
            public_proxy_url: "http://test:8080/kobo".to_string(),
            kobo_mode: "passthrough".to_string(),
            ..Default::default()
        };

        let runtime_config = file_config.to_runtime_config();
        assert_eq!(runtime_config.public_proxy_url, "http://test:8080/kobo");
        assert!(matches!(runtime_config.kobo_mode, KoboMode::Passthrough));
    }

    #[test]
    fn test_legacy_config_loads_without_new_fields() {
        let temp_dir = TempDir::new().unwrap();
        let path = temp_dir.path().join("kobbler.toml");

        // Write a minimal TOML without the new stt_service_url or hw_encoder fields
        let legacy_toml = r#"
public_proxy_url = "https://old.example.com"
kobo_mode = "local"
proxy_logging_enabled = true
whisper_model = "base.en"
whisper_threads = 4
transcribe_script_path = "/old/path.py"
"#;
        std::fs::write(&path, legacy_toml).unwrap();

        let loaded = FileConfig::load(path.to_str().unwrap()).unwrap();
        assert_eq!(loaded.public_proxy_url, "https://old.example.com");
        // New fields should have serde defaults
        assert!(loaded.stt_service_url.is_none());
        assert!(loaded.hw_encoder.is_none());
        // Legacy fields should still load for backward compat
        assert_eq!(loaded.whisper_model.as_deref(), Some("base.en"));
    }

    #[test]
    fn test_new_config_fields_round_trip() {
        let temp_dir = TempDir::new().unwrap();
        let path = temp_dir.path().join("kobbler.toml");
        let path_str = path.to_str().unwrap();

        let config = FileConfig {
            stt_service_url: Some("http://stt:8022".to_string()),
            hw_encoder: Some("nvenc".to_string()),
            tts_enabled: true,
            tts_service_url: Some("http://tts:8880".to_string()),
            ..Default::default()
        };

        config.save(path_str).unwrap();
        let loaded = FileConfig::load(path_str).unwrap();

        assert_eq!(loaded.stt_service_url.as_deref(), Some("http://stt:8022"));
        assert_eq!(loaded.hw_encoder.as_deref(), Some("nvenc"));
        assert!(loaded.tts_enabled);
        assert_eq!(loaded.tts_service_url.as_deref(), Some("http://tts:8880"));
    }

    #[test]
    fn test_new_fields_in_runtime_config() {
        let file_config = FileConfig {
            stt_service_url: Some("http://stt:8022".to_string()),
            hw_encoder: Some("nvenc".to_string()),
            ..Default::default()
        };

        let runtime = file_config.to_runtime_config();
        assert_eq!(runtime.stt_service_url.as_deref(), Some("http://stt:8022"));
        assert_eq!(runtime.hw_encoder.as_deref(), Some("nvenc"));
    }
}
