//! Runtime configuration that can be changed without restart.
//!
//! Configuration values are stored in a TOML file and can be updated via the admin API.
//! Changes take effect immediately for new requests and are persisted to the file.

use crate::config::KoboMode;
use crate::file_config::{FileConfig, FileConfigError};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::str::FromStr;
use std::sync::Arc;
use tokio::sync::RwLock;

/// A streaming service for discover recommendations.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct DiscoverStreamingService {
    pub id: i32,
    pub name: String,
}

/// Configuration values that can be changed at runtime without restart.
#[derive(Debug, Clone, Serialize)]
pub struct RuntimeConfig {
    /// Externally reachable base URL for this kobbler, e.g. `https://kobbler.example.com`.
    /// Only needed where kobbler has to emit a link that is opened outside the app —
    /// a chat citation in a phone notification, for instance. Unset is valid and is
    /// what every install had before this key existed.
    pub public_url: Option<String>,
    pub public_proxy_url: String,
    pub kobo_storeapi_base_url: String,
    pub kobo_mode: KoboMode,
    pub proxy_logging_enabled: bool,
    pub proxy_log_retention_days: i32,
    pub blob_capture_enabled: bool,
    pub kobo_sync_batch_size: i64,
    pub activity_limit: i64,
    pub default_page_size: i64,
    pub max_page_size: i64,
    /// Default entitlement mode for new books: "all_devices" or "none"
    pub default_entitlement_mode: String,
    /// Whether media enrichment (thumbnails, LUFS) is enabled.
    /// When false, enrichment jobs are skipped. Default: false.
    pub enrichment_enabled: bool,
    /// Whether directory watching is enabled
    pub watcher_enabled: bool,
    /// Interval for directory scanning in seconds (fallback for filesystems without inotify)
    pub watcher_scan_interval_seconds: i32,
    /// Default interval (seconds) between periodic media-root filesystem scans.
    /// Per-root overrides live on `media_roots.scan_interval_seconds`.
    pub media_root_scan_interval_seconds: i32,
    /// Whether metadata providers are enabled for automatic lookups
    pub metadata_providers_enabled: bool,
    /// Off by default: the weekly sync spends ~100 Hardcover requests of the key's daily budget.
    pub book_title_suggestions_enabled: bool,
    /// Number of days to cache metadata lookups
    pub metadata_cache_days: i32,
    /// Whether to rewrite external Kobo URLs to go through our proxy
    /// Useful for debugging but may not be needed in production
    pub rewrite_kobo_urls: bool,
    /// Default metadata provider for Wanted searches (optional)
    pub wanted_default_provider: Option<String>,
    /// Minimum score for fuzzy matching in watch patterns (0.0-1.0, default 0.65)
    pub wanted_min_score: Option<f64>,
    /// Kobload base URL (optional; enables download orchestration)
    pub kobload_base_url: Option<String>,
    /// Kobload API key (optional; enables download orchestration)
    pub kobload_api_key: Option<String>,
    /// Full ingest URL with token in path (e.g., http://kobbler:6766/api/v1/ingest/xxx)
    pub kobload_ingest_url: Option<String>,
    /// Communication mode for Kobbler→Kobload actions: "nats" (default) or "http"
    pub kobload_communication_mode: Option<String>,

    // MeiliSearch configuration
    /// MeiliSearch URL (optional)
    pub meili_url: Option<String>,
    /// MeiliSearch master key (optional)
    pub meili_master_key: Option<String>,
    pub semantic_search_enabled: bool,
    pub semantic_search_ratio: f64,

    // Koblem (local LLM service)
    /// Koblem service URL (optional; enables local LLM features)
    pub koblem_service_url: Option<String>,
    /// Koblem API key. Required for tier-2 endpoints (e.g. TTS streaming
    /// TTS with forced alignment). Tier-1 mirrors don't enforce auth.
    pub koblem_api_key: Option<String>,

    // Chat (AI Assistant)
    /// Whether the chat/conversational search feature is enabled
    pub chat_enabled: bool,
    /// LLM provider: "gemini", "openai", "custom"
    pub chat_llm_provider: Option<String>,
    /// Base URL for the LLM API (auto-populated for known providers)
    pub chat_llm_base_url: Option<String>,
    /// API key for the LLM provider
    pub chat_llm_api_key: Option<String>,
    /// Model name (e.g., "gemini-2.5-flash-lite", "gpt-4o-mini")
    pub chat_llm_model: Option<String>,
    /// Chat LLM thinking level for koblem routes: "off", "low", "medium",
    /// "high". Unset means auto: "medium" for Qwen-family models, "off" otherwise.
    pub chat_llm_thinking: Option<String>,

    // Conversion tool paths (optional - defaults to searching PATH).
    // ebook-convert is the long-tail fallback; native pipeline handles the rest.
    pub ebook_convert_path: Option<String>,
    pub ebook_meta_path: Option<String>,

    // Migration (Sonarr/Radarr)
    pub sonarr_url: Option<String>,
    pub sonarr_api_key: Option<String>,
    pub radarr_url: Option<String>,
    pub radarr_api_key: Option<String>,

    /// Root directory for video files. Default: None (falls back to {storage_root}/video)
    pub video_storage_root: Option<String>,

    // Streaming / transcoding
    /// Directory for temporary transcode segments (optional — defaults to {storage_root}/cache/transcode)
    pub transcode_cache_dir: Option<String>,

    /// General-purpose cache directory (skip detection, etc). Default: {storage_root}/cache
    pub cache_dir: Option<String>,

    // Transcription (STT service)
    /// Transcription mode: "off" (default), "auto", "manual"
    pub transcript_mode: String,
    /// STT service URL (OpenAI-compatible, e.g. http://stt:8880)
    pub stt_service_url: Option<String>,

    /// Per-job-type concurrency overrides.
    /// Keys are job type strings (e.g. "media_analyze"), values are 1–32.
    /// The settable set is `routes::jobs_concurrency::job_registry`, which the
    /// PUT endpoint validates against — a key naming a job type that has no
    /// handler is simply never read.
    pub job_concurrency: Option<HashMap<String, u32>>,

    // TTS (text-to-speech) sidecar service
    /// Whether TTS read-along is enabled
    pub tts_enabled: bool,
    /// OpenAI-compatible TTS service URL (e.g., http://tts:8880)
    pub tts_service_url: Option<String>,
    /// TTS service type: "auto", "kokoro", "chatterbox", "openai"
    pub tts_service_type: String,
    /// Default TTS voice ID (e.g., "af_heart")
    pub tts_default_voice: Option<String>,
    /// Default temperature (variation) for TTS (0.0–1.5)
    pub tts_default_temperature: Option<f64>,

    // Hardware encoder preference
    /// Hardware encoder: "auto" (default), "vaapi", "nvenc", or "none"
    pub hw_encoder: Option<String>,

    // GPU sharing
    /// Whether TTS preempts STT when both share a GPU (default: true).
    /// Disable if you have enough VRAM to run TTS + STT concurrently.
    pub gpu_preempt_tts: bool,

    /// Route vision/TTS/STT calls through koblem's priority GPU lock instead
    /// of kobbler's local one. Requires `koblem_service_url` to be set.
    /// Standalone kobbler (no koblem) → set to false.
    pub route_gpu_via_koblem: bool,

    // Skip detection (intro/credits/recap)
    /// Skip detection mode: "off" (default), "audio", "full"
    pub skip_detect_mode: String,
    /// Image embedding server (EmbeddingGemma-2 llama-server, e.g.
    /// http://embed-vision:8080, or koblem). Also serves text when
    /// `text_embedding_service_url` is unset.
    pub vision_service_url: Option<String>,
    /// Always-on text embedding server (e.g. http://embed-text:8080). Optional;
    /// unset → text goes to the vision URL.
    pub text_embedding_service_url: Option<String>,

    // IPTV configuration
    /// Language codes to show by default in IPTV channel listings (display filter, not storage filter)
    pub iptv_language_filter: Vec<String>,
    /// Whether to auto-create channels for unmatched streams during sync
    pub iptv_auto_create_channels: bool,
    /// Whether periodic IPTV source health checks are enabled
    pub iptv_health_check_enabled: bool,

    // Live TV (HDHomeRun tuner) configuration
    /// Guide source: "tuner" (guide.php), "silicondust" (DVR sub) or "xmltv"
    pub livetv_guide_source: String,
    /// Buffer retention policy: "tune_in", "rolling" or "programme"
    pub livetv_buffer_policy: String,
    /// Hard ceiling on retained live buffer, in hours
    pub livetv_buffer_cap_hours: i64,
    pub livetv_buffer_thumbnails: bool,
    /// What a viewer sees when a recording takes their tuner: "keep_playing", "stop" or "ask"
    pub livetv_clash_policy: String,
    /// How detected ad breaks are surfaced: "shade", "button" or "auto"
    pub livetv_ad_posture: String,
    /// Genres where ad detection is suppressed outright
    pub livetv_ad_disable_genres: Vec<String>,
    /// Whether a user's break correction seeds the ad fingerprint bank
    pub livetv_ad_train_from_corrections: bool,
    /// Recording output directory (overrides default {storage_root}/livetv)
    pub livetv_recording_path: Option<String>,
    /// Deinterlace DVB-T sources on the web transcode path
    pub livetv_deinterlace_web: bool,
    /// Target height for the web transcode path
    pub livetv_transcode_height: i64,
    /// Debrid cache directory (overrides default {storage_root}/debrid_cache)
    pub debrid_cache_dir: Option<String>,
    /// Debrid cache TTL in seconds (default 7 days)
    pub debrid_cache_ttl_secs: i64,
    /// Max debrid cache size in bytes (default 50GB)
    pub debrid_cache_max_bytes: i64,
    /// Preferred audio language for playback (e.g. "english", "french").
    /// Auto-selects matching audio track when creating sessions.
    pub preferred_audio_language: String,
    /// Preferred subtitle language for playback (e.g. "english").
    /// Auto-enables subtitles when no matching audio track is found.
    pub preferred_subtitle_language: String,
    /// Trim bloated Matroska cue indexes out of direct-play byte ranges as served.
    pub mkv_cue_trim_enabled: bool,
    /// ISO region code for streaming service discover (e.g. US, GB, AU)
    pub discover_region: String,
    /// Streaming services to include in discover recommendations
    pub discover_streaming_services: Vec<DiscoverStreamingService>,
    /// Google Cast receiver application ID (from Cast Developer Console)
    pub cast_receiver_app_id: Option<String>,
}

impl Default for RuntimeConfig {
    fn default() -> Self {
        Self {
            public_url: None,
            public_proxy_url: String::new(),
            kobo_storeapi_base_url: "https://storeapi.kobo.com".to_string(),
            kobo_mode: KoboMode::Merge,
            proxy_logging_enabled: true,
            proxy_log_retention_days: 7,
            blob_capture_enabled: false,
            kobo_sync_batch_size: 100,
            activity_limit: 200,
            default_page_size: 50,
            max_page_size: 200,
            default_entitlement_mode: "all_devices".to_string(),
            enrichment_enabled: false,
            watcher_enabled: false,
            watcher_scan_interval_seconds: 60,
            media_root_scan_interval_seconds: 1800,
            metadata_providers_enabled: true,
            book_title_suggestions_enabled: false,
            metadata_cache_days: 30,
            rewrite_kobo_urls: false,
            wanted_default_provider: None,
            wanted_min_score: None,
            kobload_base_url: None,
            kobload_api_key: None,
            kobload_ingest_url: None,
            kobload_communication_mode: None,
            meili_url: None,
            meili_master_key: None,
            semantic_search_enabled: false,
            semantic_search_ratio: 0.5,
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
            cache_dir: None,
            transcript_mode: "off".to_string(),
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
            skip_detect_mode: "off".to_string(),
            vision_service_url: None,
            text_embedding_service_url: None,
            iptv_language_filter: vec!["en".to_string()],
            iptv_auto_create_channels: true,
            iptv_health_check_enabled: true,
            livetv_guide_source: "tuner".to_string(),
            livetv_buffer_policy: "tune_in".to_string(),
            livetv_buffer_cap_hours: 3,
            livetv_buffer_thumbnails: true,
            livetv_clash_policy: "keep_playing".to_string(),
            livetv_ad_posture: "shade".to_string(),
            livetv_ad_disable_genres: vec!["News".to_string(), "Sports".to_string()],
            livetv_ad_train_from_corrections: true,
            livetv_recording_path: None,
            livetv_deinterlace_web: true,
            livetv_transcode_height: 720,
            debrid_cache_dir: None,
            debrid_cache_ttl_secs: 7 * 24 * 3600, // 7 days
            debrid_cache_max_bytes: 50 * 1024 * 1024 * 1024, // 50GB
            preferred_audio_language: "english".to_string(),
            preferred_subtitle_language: "english".to_string(),
            mkv_cue_trim_enabled: true,
            discover_region: "US".to_string(),
            discover_streaming_services: vec![],
            cast_receiver_app_id: None,
        }
    }
}

/// Update payload for runtime configuration.
#[derive(Debug, Deserialize, Default)]
pub struct RuntimeConfigUpdate {
    pub public_url: Option<String>,
    pub public_proxy_url: Option<String>,
    pub kobo_storeapi_base_url: Option<String>,
    pub kobo_mode: Option<String>,
    pub proxy_logging_enabled: Option<bool>,
    pub proxy_log_retention_days: Option<i32>,
    pub blob_capture_enabled: Option<bool>,
    pub kobo_sync_batch_size: Option<i64>,
    pub activity_limit: Option<i64>,
    pub default_page_size: Option<i64>,
    pub max_page_size: Option<i64>,
    pub default_entitlement_mode: Option<String>,
    pub enrichment_enabled: Option<bool>,
    pub watcher_enabled: Option<bool>,
    pub watcher_scan_interval_seconds: Option<i32>,
    pub media_root_scan_interval_seconds: Option<i32>,
    pub metadata_providers_enabled: Option<bool>,
    pub book_title_suggestions_enabled: Option<bool>,
    pub metadata_cache_days: Option<i32>,
    pub rewrite_kobo_urls: Option<bool>,
    pub wanted_default_provider: Option<String>,
    pub wanted_min_score: Option<f64>,
    pub kobload_base_url: Option<String>,
    pub kobload_api_key: Option<String>,
    pub kobload_ingest_url: Option<String>,
    pub kobload_communication_mode: Option<String>,
    pub meili_url: Option<String>,
    pub meili_master_key: Option<String>,
    pub semantic_search_enabled: Option<bool>,
    pub semantic_search_ratio: Option<f64>,
    pub koblem_service_url: Option<String>,
    pub koblem_api_key: Option<String>,
    pub chat_enabled: Option<bool>,
    pub chat_llm_provider: Option<String>,
    pub chat_llm_base_url: Option<String>,
    pub chat_llm_api_key: Option<String>,
    pub chat_llm_model: Option<String>,
    pub chat_llm_thinking: Option<String>,
    pub ebook_convert_path: Option<String>,
    pub ebook_meta_path: Option<String>,
    pub sonarr_url: Option<String>,
    pub sonarr_api_key: Option<String>,
    pub radarr_url: Option<String>,
    pub radarr_api_key: Option<String>,
    pub video_storage_root: Option<String>,
    pub transcode_cache_dir: Option<String>,
    pub cache_dir: Option<String>,
    pub transcript_mode: Option<String>,
    pub stt_service_url: Option<String>,
    pub job_concurrency: Option<HashMap<String, u32>>,
    pub tts_enabled: Option<bool>,
    pub tts_service_url: Option<String>,
    pub tts_service_type: Option<String>,
    pub tts_default_voice: Option<String>,
    pub tts_default_temperature: Option<f64>,
    pub hw_encoder: Option<String>,
    pub gpu_preempt_tts: Option<bool>,
    pub route_gpu_via_koblem: Option<bool>,
    pub skip_detect_mode: Option<String>,
    pub vision_service_url: Option<String>,
    pub text_embedding_service_url: Option<String>,
    pub iptv_language_filter: Option<Vec<String>>,
    pub iptv_auto_create_channels: Option<bool>,
    pub iptv_health_check_enabled: Option<bool>,
    pub livetv_guide_source: Option<String>,
    pub livetv_buffer_policy: Option<String>,
    pub livetv_buffer_cap_hours: Option<i64>,
    pub livetv_buffer_thumbnails: Option<bool>,
    pub livetv_clash_policy: Option<String>,
    pub livetv_ad_posture: Option<String>,
    pub livetv_ad_disable_genres: Option<Vec<String>>,
    pub livetv_ad_train_from_corrections: Option<bool>,
    pub livetv_recording_path: Option<String>,
    pub livetv_deinterlace_web: Option<bool>,
    pub livetv_transcode_height: Option<i64>,
    pub debrid_cache_dir: Option<String>,
    pub debrid_cache_ttl_secs: Option<i64>,
    pub debrid_cache_max_bytes: Option<i64>,
    pub preferred_audio_language: Option<String>,
    pub preferred_subtitle_language: Option<String>,
    pub mkv_cue_trim_enabled: Option<bool>,
    pub discover_region: Option<String>,
    pub discover_streaming_services: Option<Vec<DiscoverStreamingService>>,
    pub cast_receiver_app_id: Option<String>,
}

#[cfg(test)]
mod route_gpu_tests {
    use super::*;

    fn sample_config() -> RuntimeConfig {
        RuntimeConfig {
            koblem_service_url: Some("http://koblem:8090".to_string()),
            stt_service_url: Some("http://stt-parakeet:8022".to_string()),
            tts_service_url: Some("http://tts-chatterbox:8000".to_string()),
            vision_service_url: Some("http://embed-vision:8080".to_string()),
            ..RuntimeConfig::default()
        }
    }

    #[test]
    fn standalone_mode_uses_direct_urls() {
        let mut c = sample_config();
        c.route_gpu_via_koblem = false;
        assert_eq!(
            c.effective_stt_url().as_deref(),
            Some("http://stt-parakeet:8022")
        );
        assert_eq!(
            c.effective_tts_url().as_deref(),
            Some("http://tts-chatterbox:8000")
        );
        assert_eq!(
            c.effective_vision_url().as_deref(),
            Some("http://embed-vision:8080")
        );
        assert!(
            c.use_local_gpu_lock(),
            "standalone should keep local lock active"
        );
    }

    #[test]
    fn routing_mode_uses_koblem_url() {
        let mut c = sample_config();
        c.route_gpu_via_koblem = true;
        assert_eq!(c.effective_stt_url().as_deref(), Some("http://koblem:8090"));
        assert_eq!(c.effective_tts_url().as_deref(), Some("http://koblem:8090"));
        assert_eq!(
            c.effective_vision_url().as_deref(),
            Some("http://koblem:8090")
        );
        assert!(!c.use_local_gpu_lock(), "koblem owns lock in routing mode");
    }

    #[test]
    fn routing_falls_back_when_koblem_url_missing() {
        let mut c = sample_config();
        c.route_gpu_via_koblem = true;
        c.koblem_service_url = None;
        assert_eq!(
            c.effective_stt_url().as_deref(),
            Some("http://stt-parakeet:8022")
        );
        assert_eq!(
            c.effective_tts_url().as_deref(),
            Some("http://tts-chatterbox:8000")
        );
        assert_eq!(
            c.effective_vision_url().as_deref(),
            Some("http://embed-vision:8080")
        );
        assert!(c.use_local_gpu_lock(), "should fall back to local lock");
    }

    #[test]
    fn empty_koblem_url_treated_as_unset() {
        let mut c = sample_config();
        c.route_gpu_via_koblem = true;
        c.koblem_service_url = Some(String::new());
        assert_eq!(
            c.effective_stt_url().as_deref(),
            Some("http://stt-parakeet:8022")
        );
        assert!(c.use_local_gpu_lock());
    }

    /// Must track `effective_tts_url` exactly: the flag is what decides whether
    /// a buffered synth is speaking to koblem's narrow mirror or to the engine,
    /// and a disagreement between the two 400s every preview.
    #[test]
    fn tts_routes_via_koblem_tracks_the_effective_url() {
        let mut c = sample_config();
        for (route, url, want) in [
            (true, Some("http://koblem:8090"), true),
            (false, Some("http://koblem:8090"), false),
            (true, None, false),
            (true, Some(""), false),
        ] {
            c.route_gpu_via_koblem = route;
            c.koblem_service_url = url.map(str::to_string);
            assert_eq!(c.tts_routes_via_koblem(), want, "{route} {url:?}");
            assert_eq!(
                c.effective_tts_url().as_deref() == Some("http://koblem:8090"),
                want,
                "disagrees with effective_tts_url: {route} {url:?}"
            );
        }
    }

    #[test]
    fn trailing_slash_stripped_from_koblem_url() {
        let mut c = sample_config();
        c.route_gpu_via_koblem = true;
        c.koblem_service_url = Some("http://koblem:8090/".to_string());
        assert_eq!(c.effective_stt_url().as_deref(), Some("http://koblem:8090"));
    }
}

impl RuntimeConfig {
    /// Effective URL for the STT service. When `route_gpu_via_koblem` is on
    /// and `koblem_service_url` is set, returns the koblem URL (so the shared
    /// priority GPU lock gates the call). Otherwise returns the per-service
    /// URL for standalone kobbler operation.
    pub fn effective_stt_url(&self) -> Option<String> {
        if self.route_gpu_via_koblem
            && let Some(ref koblem) = self.koblem_service_url
            && !koblem.is_empty()
        {
            return Some(koblem.trim_end_matches('/').to_string());
        }
        self.stt_service_url.clone()
    }

    /// See `effective_stt_url`.
    pub fn effective_tts_url(&self) -> Option<String> {
        if self.route_gpu_via_koblem
            && let Some(ref koblem) = self.koblem_service_url
            && !koblem.is_empty()
        {
            return Some(koblem.trim_end_matches('/').to_string());
        }
        self.tts_service_url.clone()
    }

    /// True when `effective_tts_url` resolves to koblem rather than a TTS
    /// engine. koblem's `/v1/audio/speech` mirror is narrower than the engine
    /// behind it, so a caller that speaks the engine's wire shape has to know
    /// which of the three it is actually talking to — see `tts::SpeechEndpoint`.
    pub fn tts_routes_via_koblem(&self) -> bool {
        self.route_gpu_via_koblem
            && self
                .koblem_service_url
                .as_deref()
                .is_some_and(|s| !s.is_empty())
    }

    /// See `effective_stt_url`.
    pub fn effective_vision_url(&self) -> Option<String> {
        if self.route_gpu_via_koblem
            && let Some(ref koblem) = self.koblem_service_url
            && !koblem.is_empty()
        {
            return Some(koblem.trim_end_matches('/').to_string());
        }
        self.vision_service_url.clone()
    }

    /// Where text is embedded: `text_embedding_service_url` when set (an
    /// always-on instance needs no GPU gate), otherwise the vision URL.
    pub fn effective_text_embedding_url(&self) -> Option<String> {
        if let Some(url) = self.text_embedding_service_url.as_deref()
            && !url.trim().is_empty()
        {
            return Some(url.trim().trim_end_matches('/').to_string());
        }
        self.effective_vision_url()
    }

    /// True when kobbler's local GpuLock should stay active. When routing
    /// through koblem, koblem owns the authoritative lock and kobbler's local
    /// one is a no-op (avoiding double-gating).
    pub fn use_local_gpu_lock(&self) -> bool {
        !self.route_gpu_via_koblem
            || self
                .koblem_service_url
                .as_deref()
                .is_none_or(|s| s.is_empty())
    }
}

/// Thread-safe manager for runtime configuration.
///
/// Provides read access via `get()` which returns a cloned snapshot,
/// and write access via `update()` which persists to the config file.
#[derive(Clone)]
pub struct RuntimeConfigManager {
    inner: Arc<RwLock<RuntimeConfig>>,
    config_path: String,
}

impl RuntimeConfigManager {
    /// Create a new manager, loading initial config from the TOML file.
    pub fn new(config_path: String) -> Result<Self, FileConfigError> {
        let file_config = FileConfig::load_or_create(&config_path)?;
        let config = file_config.to_runtime_config();
        Ok(Self {
            inner: Arc::new(RwLock::new(config)),
            config_path,
        })
    }

    /// Get current config snapshot (fast read lock).
    pub async fn get(&self) -> RuntimeConfig {
        self.inner.read().await.clone()
    }

    pub async fn discover_region(&self) -> String {
        self.inner.read().await.discover_region.clone()
    }

    /// Try to get current config snapshot without blocking.
    /// Returns None if the lock is currently held by a writer.
    pub fn try_get(&self) -> Option<RuntimeConfig> {
        self.inner.try_read().ok().map(|guard| guard.clone())
    }

    /// Update config and persist to file.
    pub async fn update(&self, updates: RuntimeConfigUpdate) -> Result<(), RuntimeConfigError> {
        // Validate numeric values before applying
        if let Some(v) = updates.proxy_log_retention_days
            && v < 0
        {
            return Err(RuntimeConfigError::Validation(
                "proxy_log_retention_days must be >= 0".to_string(),
            ));
        }
        if let Some(v) = updates.kobo_sync_batch_size
            && v <= 0
        {
            return Err(RuntimeConfigError::Validation(
                "kobo_sync_batch_size must be > 0".to_string(),
            ));
        }
        if let Some(v) = updates.activity_limit
            && v <= 0
        {
            return Err(RuntimeConfigError::Validation(
                "activity_limit must be > 0".to_string(),
            ));
        }
        if let Some(v) = updates.default_page_size
            && v <= 0
        {
            return Err(RuntimeConfigError::Validation(
                "default_page_size must be > 0".to_string(),
            ));
        }
        if let Some(v) = updates.max_page_size
            && v <= 0
        {
            return Err(RuntimeConfigError::Validation(
                "max_page_size must be > 0".to_string(),
            ));
        }
        if let Some(v) = updates.watcher_scan_interval_seconds
            && v <= 0
        {
            return Err(RuntimeConfigError::Validation(
                "watcher_scan_interval_seconds must be > 0".to_string(),
            ));
        }
        if let Some(v) = updates.media_root_scan_interval_seconds
            && !(60..=86400).contains(&v)
        {
            return Err(RuntimeConfigError::Validation(
                "media_root_scan_interval_seconds must be between 60 and 86400".to_string(),
            ));
        }
        if let Some(v) = updates.metadata_cache_days
            && v < 0
        {
            return Err(RuntimeConfigError::Validation(
                "metadata_cache_days must be >= 0".to_string(),
            ));
        }
        if let Some(ref v) = updates.transcript_mode
            && v != "off"
            && v != "auto"
            && v != "manual"
        {
            return Err(RuntimeConfigError::Validation(
                "transcript_mode must be \"off\", \"auto\", or \"manual\"".to_string(),
            ));
        }
        if let Some(ref v) = updates.hw_encoder
            && !v.is_empty()
            && v != "auto"
            && v != "vaapi"
            && v != "nvenc"
            && v != "none"
        {
            return Err(RuntimeConfigError::Validation(
                "hw_encoder must be \"auto\", \"vaapi\", \"nvenc\", or \"none\"".to_string(),
            ));
        }
        if let Some(ref v) = updates.skip_detect_mode
            && v != "off"
            && v != "audio"
            && v != "full"
        {
            return Err(RuntimeConfigError::Validation(
                "skip_detect_mode must be \"off\", \"audio\", or \"full\"".to_string(),
            ));
        }

        if let Some(v) = updates.semantic_search_ratio
            && !(0.0..=1.0).contains(&v)
        {
            return Err(RuntimeConfigError::Validation(
                "semantic_search_ratio must be between 0.0 and 1.0".to_string(),
            ));
        }

        let public_url = match updates.public_url {
            Some(raw) => Some(normalize_public_url(&raw).map_err(RuntimeConfigError::Validation)?),
            None => None,
        };

        // Get current config
        let mut config = self.inner.write().await;

        let semantic_after = updates
            .semantic_search_enabled
            .unwrap_or(config.semantic_search_enabled);
        let set_after = |update: &Option<String>, current: &Option<String>| match update {
            Some(v) => !v.trim().is_empty(),
            None => current.as_deref().is_some_and(|u| !u.trim().is_empty()),
        };
        let routed_after = updates
            .route_gpu_via_koblem
            .unwrap_or(config.route_gpu_via_koblem)
            && set_after(&updates.koblem_service_url, &config.koblem_service_url);
        let embedder_after =
            routed_after || set_after(&updates.vision_service_url, &config.vision_service_url);
        if semantic_after && !embedder_after {
            return Err(RuntimeConfigError::Validation(
                "semantic search needs an embedding service: set vision_service_url, or route GPU calls via koblem"
                    .to_string(),
            ));
        }

        // Apply updates
        if let Some(v) = public_url {
            config.public_url = v;
        }
        if let Some(v) = updates.public_proxy_url {
            config.public_proxy_url = v;
        }
        if let Some(v) = updates.kobo_storeapi_base_url {
            config.kobo_storeapi_base_url = v;
        }
        if let Some(v) = updates.kobo_mode {
            config.kobo_mode = KoboMode::from_str(&v).unwrap_or_default();
        }
        if let Some(v) = updates.proxy_logging_enabled {
            config.proxy_logging_enabled = v;
        }
        if let Some(v) = updates.proxy_log_retention_days {
            config.proxy_log_retention_days = v;
        }
        if let Some(v) = updates.blob_capture_enabled {
            config.blob_capture_enabled = v;
        }
        if let Some(v) = updates.kobo_sync_batch_size {
            config.kobo_sync_batch_size = v;
        }
        if let Some(v) = updates.activity_limit {
            config.activity_limit = v;
        }
        if let Some(v) = updates.default_page_size {
            config.default_page_size = v;
        }
        if let Some(v) = updates.max_page_size {
            config.max_page_size = v;
        }
        if let Some(v) = updates.default_entitlement_mode {
            config.default_entitlement_mode = v;
        }
        if let Some(v) = updates.enrichment_enabled {
            config.enrichment_enabled = v;
        }
        if let Some(v) = updates.watcher_enabled {
            config.watcher_enabled = v;
        }
        if let Some(v) = updates.watcher_scan_interval_seconds {
            config.watcher_scan_interval_seconds = v;
        }
        if let Some(v) = updates.media_root_scan_interval_seconds {
            config.media_root_scan_interval_seconds = v;
        }
        if let Some(v) = updates.metadata_providers_enabled {
            config.metadata_providers_enabled = v;
        }
        if let Some(v) = updates.book_title_suggestions_enabled {
            config.book_title_suggestions_enabled = v;
        }
        if let Some(v) = updates.metadata_cache_days {
            config.metadata_cache_days = v;
        }
        if let Some(v) = updates.rewrite_kobo_urls {
            config.rewrite_kobo_urls = v;
        }
        if let Some(v) = updates.wanted_default_provider {
            config.wanted_default_provider = if v.is_empty() { None } else { Some(v) };
        }
        if let Some(v) = updates.kobload_base_url {
            config.kobload_base_url = if v.is_empty() { None } else { Some(v) };
        }
        if let Some(v) = updates.kobload_api_key {
            config.kobload_api_key = if v.is_empty() { None } else { Some(v) };
        }
        if let Some(v) = updates.kobload_ingest_url {
            config.kobload_ingest_url = if v.is_empty() { None } else { Some(v) };
        }
        if let Some(v) = updates.kobload_communication_mode {
            config.kobload_communication_mode = if v.is_empty() { None } else { Some(v) };
        }
        if let Some(v) = updates.meili_url {
            config.meili_url = if v.is_empty() { None } else { Some(v) };
        }
        if let Some(v) = updates.meili_master_key {
            config.meili_master_key = if v.is_empty() { None } else { Some(v) };
        }
        if let Some(v) = updates.semantic_search_enabled {
            config.semantic_search_enabled = v;
        }
        if let Some(v) = updates.semantic_search_ratio {
            config.semantic_search_ratio = v;
        }
        if let Some(v) = updates.koblem_service_url {
            config.koblem_service_url = if v.is_empty() { None } else { Some(v) };
        }
        if let Some(v) = updates.koblem_api_key {
            config.koblem_api_key = if v.is_empty() { None } else { Some(v) };
        }
        if let Some(v) = updates.chat_enabled {
            config.chat_enabled = v;
        }
        if let Some(v) = updates.chat_llm_provider {
            config.chat_llm_provider = if v.is_empty() { None } else { Some(v) };
        }
        if let Some(v) = updates.chat_llm_base_url {
            config.chat_llm_base_url = if v.is_empty() { None } else { Some(v) };
        }
        if let Some(v) = updates.chat_llm_api_key {
            config.chat_llm_api_key = if v.is_empty() { None } else { Some(v) };
        }
        if let Some(v) = updates.chat_llm_model {
            config.chat_llm_model = if v.is_empty() { None } else { Some(v) };
        }
        if let Some(v) = updates.chat_llm_thinking {
            config.chat_llm_thinking = if v.is_empty() { None } else { Some(v) };
        }
        if let Some(v) = updates.wanted_min_score {
            config.wanted_min_score = if v <= 0.0 { None } else { Some(v) };
        }
        if let Some(v) = updates.ebook_convert_path {
            config.ebook_convert_path = if v.is_empty() { None } else { Some(v) };
        }
        if let Some(v) = updates.ebook_meta_path {
            config.ebook_meta_path = if v.is_empty() { None } else { Some(v) };
        }
        if let Some(v) = updates.sonarr_url {
            config.sonarr_url = if v.is_empty() { None } else { Some(v) };
        }
        if let Some(v) = updates.sonarr_api_key {
            config.sonarr_api_key = if v.is_empty() { None } else { Some(v) };
        }
        if let Some(v) = updates.radarr_url {
            config.radarr_url = if v.is_empty() { None } else { Some(v) };
        }
        if let Some(v) = updates.radarr_api_key {
            config.radarr_api_key = if v.is_empty() { None } else { Some(v) };
        }
        if let Some(v) = updates.video_storage_root {
            config.video_storage_root = if v.is_empty() { None } else { Some(v) };
        }
        if let Some(v) = updates.transcode_cache_dir {
            config.transcode_cache_dir = if v.is_empty() { None } else { Some(v) };
        }
        if let Some(v) = updates.cache_dir {
            config.cache_dir = if v.is_empty() { None } else { Some(v) };
        }
        if let Some(v) = updates.transcript_mode {
            config.transcript_mode = v;
        }
        if let Some(v) = updates.stt_service_url {
            config.stt_service_url = if v.is_empty() { None } else { Some(v) };
        }
        if let Some(v) = updates.tts_enabled {
            config.tts_enabled = v;
        }
        if let Some(v) = updates.tts_service_url {
            config.tts_service_url = if v.is_empty() { None } else { Some(v) };
        }
        if let Some(v) = updates.tts_service_type {
            config.tts_service_type = if v.is_empty() { "auto".to_string() } else { v };
        }
        if let Some(v) = updates.tts_default_voice {
            config.tts_default_voice = if v.is_empty() { None } else { Some(v) };
        }
        if let Some(v) = updates.tts_default_temperature {
            config.tts_default_temperature = if v < 0.0 { None } else { Some(v) };
        }
        if let Some(v) = updates.hw_encoder {
            config.hw_encoder = if v.is_empty() { None } else { Some(v) };
        }
        if let Some(v) = updates.gpu_preempt_tts {
            config.gpu_preempt_tts = v;
        }
        if let Some(v) = updates.route_gpu_via_koblem {
            config.route_gpu_via_koblem = v;
        }
        if let Some(v) = updates.skip_detect_mode {
            config.skip_detect_mode = v;
        }
        if let Some(v) = updates.vision_service_url {
            config.vision_service_url = if v.is_empty() { None } else { Some(v) };
        }
        if let Some(v) = updates.text_embedding_service_url {
            config.text_embedding_service_url = if v.trim().is_empty() { None } else { Some(v) };
        }
        if let Some(v) = updates.iptv_language_filter {
            config.iptv_language_filter = v;
        }
        if let Some(v) = updates.iptv_auto_create_channels {
            config.iptv_auto_create_channels = v;
        }
        if let Some(v) = updates.iptv_health_check_enabled {
            config.iptv_health_check_enabled = v;
        }
        if let Some(v) = updates.livetv_guide_source {
            config.livetv_guide_source = v;
        }
        if let Some(v) = updates.livetv_buffer_policy {
            config.livetv_buffer_policy = v;
        }
        if let Some(v) = updates.livetv_buffer_thumbnails {
            config.livetv_buffer_thumbnails = v;
        }
        if let Some(v) = updates.livetv_buffer_cap_hours {
            config.livetv_buffer_cap_hours = v;
        }
        if let Some(v) = updates.livetv_clash_policy {
            config.livetv_clash_policy = v;
        }
        if let Some(v) = updates.livetv_ad_posture {
            config.livetv_ad_posture = v;
        }
        if let Some(v) = updates.livetv_ad_disable_genres {
            config.livetv_ad_disable_genres = v;
        }
        if let Some(v) = updates.livetv_ad_train_from_corrections {
            config.livetv_ad_train_from_corrections = v;
        }
        if let Some(v) = updates.livetv_recording_path {
            config.livetv_recording_path = if v.is_empty() { None } else { Some(v) };
        }
        if let Some(v) = updates.livetv_deinterlace_web {
            config.livetv_deinterlace_web = v;
        }
        if let Some(v) = updates.livetv_transcode_height {
            config.livetv_transcode_height = v;
        }
        if let Some(v) = updates.debrid_cache_dir {
            config.debrid_cache_dir = Some(v);
        }
        if let Some(v) = updates.debrid_cache_ttl_secs {
            config.debrid_cache_ttl_secs = v;
        }
        if let Some(v) = updates.debrid_cache_max_bytes {
            config.debrid_cache_max_bytes = v;
        }
        if let Some(v) = updates.preferred_audio_language {
            config.preferred_audio_language = v;
        }
        if let Some(v) = updates.mkv_cue_trim_enabled {
            config.mkv_cue_trim_enabled = v;
        }
        if let Some(v) = updates.preferred_subtitle_language {
            config.preferred_subtitle_language = v;
        }
        if let Some(v) = updates.discover_region {
            config.discover_region = if v.is_empty() { "US".to_string() } else { v };
        }
        if let Some(v) = updates.discover_streaming_services {
            config.discover_streaming_services = v;
        }
        if let Some(v) = updates.cast_receiver_app_id {
            config.cast_receiver_app_id = if v.is_empty() { None } else { Some(v) };
        }
        if let Some(map) = updates.job_concurrency {
            if map.is_empty() {
                config.job_concurrency = None;
            } else {
                // Validate all values are 1–32
                for (key, val) in &map {
                    if *val < 1 || *val > 32 {
                        return Err(RuntimeConfigError::Validation(format!(
                            "job_concurrency[{key}] must be between 1 and 32, got {val}"
                        )));
                    }
                }
                let merged = config.job_concurrency.get_or_insert_with(HashMap::new);
                for (key, val) in map {
                    merged.insert(key, val);
                }
            }
        }

        // Save to file
        let file_config = FileConfig::from_runtime_config(&config);
        file_config
            .save(&self.config_path)
            .map_err(RuntimeConfigError::FileError)?;

        Ok(())
    }

    /// Get configured concurrency for a job type, falling back to the provided default.
    pub fn job_concurrency(&self, job_type: &str, default: usize) -> usize {
        self.try_get()
            .and_then(|c| c.job_concurrency.as_ref()?.get(job_type).copied())
            .map(|v| v as usize)
            .unwrap_or(default)
    }

    /// Reload config from file.
    pub async fn reload(&self) -> Result<(), FileConfigError> {
        let file_config = FileConfig::load(&self.config_path)?;
        let new_config = file_config.to_runtime_config();
        *self.inner.write().await = new_config;
        Ok(())
    }

    /// Get the config file path.
    pub fn config_path(&self) -> &str {
        &self.config_path
    }
}

/// Errors from RuntimeConfigManager operations.
#[derive(Debug, thiserror::Error)]
pub enum RuntimeConfigError {
    #[error("File config error: {0}")]
    FileError(#[from] FileConfigError),
    #[error("Validation error: {0}")]
    Validation(String),
}

/// Turn what an admin typed into either a usable base URL or a refusal.
///
/// A relative path or a bare hostname stored here would silently produce dead links
/// wherever it is used, so those are rejected rather than normalised into a guess.
/// Empty clears the setting.
pub fn normalize_public_url(raw: &str) -> Result<Option<String>, String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Ok(None);
    }
    let rest = trimmed
        .strip_prefix("https://")
        .or_else(|| trimmed.strip_prefix("http://"))
        .ok_or_else(|| {
            "public_url must start with http:// or https:// (e.g. https://kobbler.example.com)"
                .to_string()
        })?;
    let host = rest.split(['/', '?', '#']).next().unwrap_or("");
    if host.is_empty() {
        return Err(
            "public_url must include a host (e.g. https://kobbler.example.com)".to_string(),
        );
    }
    Ok(Some(trimmed.trim_end_matches('/').to_string()))
}

#[cfg(test)]
mod public_url_tests {
    use super::normalize_public_url;

    #[test]
    fn an_absolute_url_is_kept_without_its_trailing_slash() {
        assert_eq!(
            normalize_public_url("https://kobbler.example.com"),
            Ok(Some("https://kobbler.example.com".to_string()))
        );
        assert_eq!(
            normalize_public_url("https://kobbler.example.com/"),
            Ok(Some("https://kobbler.example.com".to_string()))
        );
        assert_eq!(
            normalize_public_url("  http://192.168.1.10:6767///  "),
            Ok(Some("http://192.168.1.10:6767".to_string()))
        );
        assert_eq!(
            normalize_public_url("https://example.com/kobbler/"),
            Ok(Some("https://example.com/kobbler".to_string()))
        );
    }

    #[test]
    fn unset_stays_unset() {
        assert_eq!(normalize_public_url(""), Ok(None));
        assert_eq!(normalize_public_url("   "), Ok(None));
    }

    #[test]
    fn anything_that_would_produce_a_dead_link_is_refused() {
        for bad in [
            "kobbler.example.com",
            "//kobbler.example.com",
            "/movies",
            "ftp://kobbler.example.com",
            "https://",
            "http:///movies",
        ] {
            assert!(
                normalize_public_url(bad).is_err(),
                "{bad} should not be storable"
            );
        }
    }
}

impl Serialize for KoboMode {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        let s = match self {
            KoboMode::Merge => "merge",
            KoboMode::Passthrough => "passthrough",
            KoboMode::Local => "local",
        };
        serializer.serialize_str(s)
    }
}
