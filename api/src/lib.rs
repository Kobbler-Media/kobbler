#![recursion_limit = "512"]

//! Kobbler API library - exposes modules for integration testing.

pub mod audio;
pub mod auth;
pub mod auto_sync;
pub mod backup;
pub mod blob_storage;
pub mod chatbot;
pub mod config;
pub mod convert;
pub mod corruption;
pub mod db;
pub mod debrid_outcomes;
pub mod discover_art;
pub mod ebook;
pub mod enrichment_config;
pub mod events;
pub mod ffmpeg;
pub mod file_config;
pub mod file_link_mode;
pub mod for_you;
pub mod google;
pub mod gpu;
pub mod gpu_defaults;
pub mod hardcover_titles;
pub mod http;
pub mod images;
pub mod iptv;
pub mod jobs;
pub mod kids;
pub mod kobload;
pub mod kobload_bridge;
pub mod kobo;
pub mod kobo_handler;
pub mod library_edits;
pub mod livetv;
pub mod media_cache;
pub mod media_interpreter;
pub mod media_root_naming;
pub mod media_root_resolve;
pub mod metadata;
pub mod nas_health;
pub mod nats;
pub mod notifications;
pub mod rate_limit;
pub mod repositories;
pub mod routes;
pub mod runtime_config;
pub mod search;
pub mod serde_date;
pub mod serde_patch;
pub mod serde_pg;
pub mod sidecar;
pub mod skip_detect;
pub mod source_cache;
pub mod sport_want;
pub mod sse_client;
pub mod streaming;
pub mod stremio_library;
pub mod subtitle_providers;
pub mod sync;
pub mod sync_providers;
pub mod tmdb_titles;
pub mod tracking;
pub mod tts;
pub mod types;
pub mod video_metadata;
pub mod video_sync_providers;
pub mod wanted;
pub mod watch_handoff;
pub mod watcher;

pub use config::Config;
pub use db::DbPool;
pub use events::LibraryEvent;
pub use runtime_config::RuntimeConfigManager;
pub use types::EntitlementScope;

use axum::http::header::{ACCEPT, AUTHORIZATION, CONTENT_TYPE};
use axum::http::{HeaderValue, Method};
use axum::{
    Router,
    extract::DefaultBodyLimit,
    routing::{delete, get, patch, post, put},
};
use routes::admin::audiobook as audiobook_routes;
use routes::admin::calendar as calendar_routes;
use routes::admin::dashboard as dashboard_routes;
use routes::admin::debrid_stream as debrid_stream_routes;
use routes::admin::diagnostics as diagnostics_routes;
use routes::admin::discover as discover_routes;
use routes::admin::episode_cast as episode_cast_routes;
use routes::admin::for_you as for_you_routes;
use routes::admin::inbox as inbox_routes;
use routes::admin::iptv as iptv_routes;
use routes::admin::livetv as livetv_routes;
use routes::admin::livetv_play as livetv_play_routes;
use routes::admin::livetv_recordings as livetv_recording_routes;
use routes::admin::livetv_series_rules as livetv_rule_routes;
use routes::admin::livetv_watch as livetv_watch_routes;
use routes::admin::media_roots as media_root_routes;
use routes::admin::movies as movie_routes;
use routes::admin::notifications as notification_routes;
use routes::admin::people as people_routes;
use routes::admin::sports as sport_routes;
use routes::admin::streaming as streaming_routes;
use routes::admin::subtitle_providers as subtitle_provider_routes;
use routes::admin::subtitles as subtitle_routes;
use routes::admin::tts as tts_routes;
use routes::admin::tv as tv_routes;
use routes::admin::video_metadata as video_metadata_routes;
use routes::admin::video_sync as video_sync_routes;
use routes::admin::watch_history as watch_history_routes;
use routes::admin::watchlist as watchlist_routes;
use routes::providers::library as provider_library_routes;
use routes::stremio as stremio_routes;
use routes::{
    admin, health, ingest, jobs as jobs_routes, jobs_concurrency, library,
    metadata as metadata_routes, sync as sync_routes, wanted as wanted_routes,
    watcher as watcher_routes,
};
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use tokio::sync::broadcast;
use tower_http::cors::{Any, CorsLayer};

#[derive(Clone)]
pub struct AppState {
    pub pool: DbPool,
    pub config: Arc<Config>,
    pub runtime_config: RuntimeConfigManager,
    pub library_events: broadcast::Sender<events::LibraryEvent>,
    pub job_events: broadcast::Sender<jobs::JobEvent>,
    pub ingest_queue_events: broadcast::Sender<IngestQueueEvent>,
    pub nats: nats::NatsState,
    pub search: Arc<dyn search::SearchBackend>,
    pub playback_sessions: Arc<dashmap::DashMap<uuid::Uuid, streaming::session::PlaybackSession>>,
    pub cue_trim_cache: Arc<ffmpeg::mkv_cue_cache::CueTrimCache>,
    pub hw_accel: Arc<ffmpeg::detect::HwAccel>,
    pub encoder_caps: Arc<ffmpeg::detect::EncoderCaps>,
    pub decoder_caps: ffmpeg::detect::DecoderCaps,
    /// Lazily-created GPU device context for hardware-accelerated decoding
    /// (NVDEC/VAAPI). The underlying context (~100MB VRAM for CUDA) is only built
    /// on first transcode and released after an idle grace period.
    pub hw_device: Arc<kob_ffmpeg::hw::device::LazyHwDevice>,
    /// VAAPI device for encoder fallback when primary accel is NVENC.
    /// Created once at startup to avoid concurrent `cuCtxCreate` races.
    pub vaapi_fallback_device: Option<kob_ffmpeg::hw::device::SharedHwDevice>,
    /// Sessions temporarily removed from playback_sessions during seek operations.
    /// Endpoints check this to return 503 (retry) instead of 404 (dead).
    pub busy_sessions: Arc<dashmap::DashSet<uuid::Uuid>>,
    /// Last position each user's devices saved, per show/movie. See `watch_handoff`.
    pub watch_handoffs: Arc<watch_handoff::HandoffStore>,
    pub gpu_lock: gpu::GpuLock,
    pub login_limiter: Arc<rate_limit::RateLimiter>,
    pub cancel_registry: nats_jobs::CancelRegistry,
    pub live_streams: Arc<dashmap::DashMap<uuid::Uuid, iptv::live_session::LiveStream>>,
    /// Live TV tuner leases, tunes and viewing sessions. Separate from the IPTV
    /// maps because the tuner is a LAN device with a hard 4-slot cap, not a pool
    /// of interchangeable remote sources.
    pub livetv: Arc<livetv::live_session::LivetvState>,
    pub stremio_play_cache: Arc<dashmap::DashMap<String, routes::stremio::CachedStremioRedirect>>,
    pub stremio_resolve_locks:
        Arc<dashmap::DashMap<String, std::sync::Arc<tokio::sync::Mutex<()>>>>,
    /// Maps debrid cache key (info_hash or info_hash_fN) → active session_id.
    /// Prevents duplicate sessions for the same torrent file.
    pub debrid_session_index: Arc<dashmap::DashMap<String, uuid::Uuid>>,
    /// Per-key locks for serializing debrid session creation.
    pub debrid_resolve_locks: Arc<dashmap::DashMap<String, std::sync::Arc<tokio::sync::Mutex<()>>>>,
    /// Instant Stream: debrid sessions warmed ahead of a Play press, until claimed.
    pub stream_prewarms: Arc<routes::admin::stream_prewarm::PrewarmStore>,
    /// Set to true after the first media_root_scan completes a full scan.
    /// Backfill handlers skip their work until this is set, preventing them
    /// from queuing jobs for files that haven't been checked for deletion yet.
    pub initial_scan_complete: Arc<AtomicBool>,
    /// Per-media-root concurrency gate for heavy NAS IO (thumbnails, skip detect,
    /// extract). Configured via `media_roots.max_concurrent_io`.
    pub media_root_io_gate: jobs::MediaRootIoGate,
    /// Optional CIFS contention monitor. Always-on struct, but only polls when
    /// `config.nas_health_monitoring` is true.
    pub nas_health: nas_health::NasHealthMonitor,
}

/// Event emitted when ingest queue items change status.
#[derive(Debug, Clone, serde::Serialize)]
pub struct IngestQueueEvent {
    pub item_id: uuid::Uuid,
    pub status: String,
}

/// Create the API router with the given state.
/// This is extracted so tests can create a router without running the full server.
pub fn create_router(state: AppState) -> Router {
    let cors = {
        let origins = state
            .config
            .web_origins
            .iter()
            .filter_map(|origin| HeaderValue::from_str(origin).ok())
            .collect::<Vec<_>>();

        let base_cors = CorsLayer::new()
            .allow_methods([
                Method::GET,
                Method::POST,
                Method::PUT,
                Method::DELETE,
                Method::OPTIONS,
            ])
            .allow_headers([CONTENT_TYPE, AUTHORIZATION, ACCEPT]);

        // Can't combine credentials with wildcard origin, so only enable credentials
        // when specific origins are configured
        if origins.is_empty() {
            base_cors.allow_origin(Any)
        } else {
            base_cors.allow_credentials(true).allow_origin(origins)
        }
    };

    Router::new()
        .route("/health", get(health::health))
        .route("/api/v1/admin/login", post(admin::login))
        .route("/api/v1/admin/me", get(admin::me))
        .route("/api/v1/admin/logout", post(admin::logout))
        .route("/api/v1/admin/pin-login", post(admin::pin_login))
        .route("/api/v1/admin/me/password", put(admin::change_password))
        .route(
            "/api/v1/admin/me/avatar",
            put(admin::upload_avatar).delete(admin::delete_avatar),
        )
        .route(
            "/api/v1/admin/me/display-name",
            put(admin::update_display_name),
        )
        .route("/api/v1/admin/me/timezone", put(admin::update_timezone))
        .route(
            "/api/v1/admin/me/notification-prefs",
            get(admin::get_notification_prefs).put(admin::update_notification_prefs),
        )
        .route(
            "/api/v1/admin/me/profile-pin",
            put(admin::set_profile_pin).delete(admin::clear_profile_pin),
        )
        .route(
            "/api/v1/admin/me/verify-pin",
            post(admin::verify_profile_pin),
        )
        .route(
            "/api/v1/admin/users/{user_id}/avatar",
            get(admin::get_avatar),
        )
        .route("/api/v1/admin/calendar", get(calendar_routes::get_calendar))
        .route(
            "/api/v1/admin/dashboard",
            get(dashboard_routes::get_dashboard),
        )
        .route(
            "/api/v1/admin/watchlist",
            get(watchlist_routes::list_watchlist).post(watchlist_routes::add_to_watchlist),
        )
        .route(
            "/api/v1/admin/watchlist/{media_type}/{media_id}",
            delete(watchlist_routes::remove_from_watchlist),
        )
        // Discover (trending/popular)
        .route(
            "/api/v1/admin/discover",
            get(discover_routes::list_discover),
        )
        .route(discover_art::ART_ROUTE, get(discover_art::serve))
        .route(
            "/api/v1/admin/discover/for-you/dismiss",
            post(for_you_routes::dismiss).delete(for_you_routes::undismiss),
        )
        .route(
            "/api/v1/admin/watch-history",
            get(watch_history_routes::list_history),
        )
        .route(
            "/api/v1/admin/watch-history/exclude",
            post(watch_history_routes::exclude),
        )
        .route(
            "/api/v1/admin/watch-history/remove",
            post(watch_history_routes::remove),
        )
        .route(
            "/api/v1/admin/discover/refresh",
            post(discover_routes::refresh_discover),
        )
        .route(
            "/api/v1/admin/discover/{media_type}/{provider}/{provider_id}",
            get(discover_routes::get_discover_detail),
        )
        .route(
            "/api/v1/admin/discover/tv/{provider}/{provider_id}/season/{season_number}",
            get(discover_routes::get_discover_season),
        )
        .route(
            "/api/v1/admin/discover/want-episode",
            post(discover_routes::want_discover_episode),
        )
        .route(
            "/api/v1/admin/discover/want-season",
            post(discover_routes::want_discover_season),
        )
        .route(
            "/api/v1/admin/discover/watch-providers",
            get(discover_routes::list_watch_providers),
        )
        .route(
            "/api/v1/admin/users",
            get(admin::list_users).post(admin::create_user),
        )
        .route(
            "/api/v1/admin/users/{user_id}",
            patch(admin::update_user).delete(admin::delete_user),
        )
        .route(
            "/api/v1/admin/users/{user_id}/device-token",
            post(admin::mint_device_token),
        )
        .route("/api/v1/admin/sessions", get(admin::list_sessions))
        .route(
            "/api/v1/admin/sessions/refresh",
            post(admin::refresh_session),
        )
        .route(
            "/api/v1/admin/sessions/{session_id}",
            delete(admin::revoke_session),
        )
        .route(
            "/api/v1/admin/me/sync-providers",
            get(admin::get_sync_providers),
        )
        .route(
            "/api/v1/admin/me/sync-providers/{provider_type}",
            put(admin::update_sync_provider),
        )
        .route(
            "/api/v1/admin/devices",
            get(admin::list_devices).post(admin::create_device),
        )
        .route(
            "/api/v1/admin/device-types",
            get(admin::get_active_device_types),
        )
        .route(
            "/api/v1/admin/devices/{device_id}/token",
            post(admin::rotate_device_token),
        )
        .route(
            "/api/v1/admin/devices/{device_id}/resync",
            post(admin::resync_device),
        )
        .route(
            "/api/v1/admin/devices/{device_id}",
            patch(admin::update_device).delete(admin::delete_device),
        )
        .route(
            "/api/v1/admin/devices/{device_id}/sync-providers/{provider_type}",
            put(admin::update_device_sync_provider),
        )
        .route(
            "/api/v1/admin/devices/{device_id}/books",
            get(admin::list_device_books).post(admin::add_device_books),
        )
        .route(
            "/api/v1/admin/devices/{device_id}/books/{book_id}",
            delete(admin::remove_device_book),
        )
        .route(
            "/api/v1/admin/devices/{device_id}/activity",
            get(admin::get_device_activity),
        )
        .route(
            "/api/v1/admin/ingest-tokens",
            get(admin::list_ingest_tokens).post(admin::create_ingest_token),
        )
        .route(
            "/api/v1/admin/ingest-tokens/{token_id}",
            patch(admin::update_ingest_token),
        )
        .route(
            "/api/v1/admin/ingest-tokens/{token_id}/revoke",
            post(admin::revoke_ingest_token),
        )
        .route(
            "/api/v1/admin/playback/tokens",
            post(admin::create_playback_token),
        )
        .route(
            "/api/v1/admin/playback/tokens/{token}",
            delete(admin::revoke_playback_token),
        )
        .route(
            "/api/v1/admin/books/{book_id}/remove",
            post(admin::remove_book_entitlements),
        )
        .route(
            "/api/v1/admin/books/{book_id}/restore",
            post(admin::restore_book_entitlements),
        )
        // Book device sync (per-book entitlements)
        .route(
            "/api/v1/admin/books/{book_id}/devices",
            get(admin::list_book_devices),
        )
        .route(
            "/api/v1/admin/books/{book_id}/devices/all",
            post(admin::add_book_all_devices),
        )
        .route(
            "/api/v1/admin/books/{book_id}/devices/{device_id}",
            post(admin::add_book_device).delete(admin::remove_book_device),
        )
        // Web reader
        .route(
            "/api/v1/admin/books/{book_id}/file",
            get(admin::download_book_file),
        )
        .route(
            "/api/v1/admin/books/{book_id}/reader-progress",
            get(admin::get_reader_progress).post(admin::save_reader_progress),
        )
        // Audiobook files
        .route(
            "/api/v1/admin/books/{book_id}/audio-files",
            get(audiobook_routes::list_audio_files),
        )
        // Audiobook streaming
        .route(
            "/api/v1/admin/books/{book_id}/audio/{file_id}",
            get(audiobook_routes::stream_audio),
        )
        .route(
            "/api/v1/admin/books/{book_id}/audio/{file_id}/peaks",
            get(audiobook_routes::serve_audio_peaks),
        )
        .route(
            "/api/v1/admin/books/{book_id}/audio/{file_id}/chapters",
            get(audiobook_routes::get_chapters).put(audiobook_routes::replace_file_chapters),
        )
        .route(
            "/api/v1/admin/books/{book_id}/chapters",
            get(audiobook_routes::get_unified_chapters),
        )
        .route(
            "/api/v1/admin/books/{book_id}/chapters/{chapter_id}",
            patch(audiobook_routes::update_single_chapter)
                .delete(audiobook_routes::delete_single_chapter),
        )
        .route(
            "/api/v1/admin/books/{book_id}/generate-chapters",
            post(audiobook_routes::generate_chapters),
        )
        .route(
            "/api/v1/admin/books/{book_id}/probe-audio",
            post(audiobook_routes::probe_audio_files),
        )
        .route(
            "/api/v1/admin/books/{book_id}/fetch-chapters",
            post(audiobook_routes::fetch_audnexus_chapters),
        )
        .route(
            "/api/v1/admin/books/{book_id}/detect-chapters",
            post(audiobook_routes::detect_silence_chapters_endpoint),
        )
        .route(
            "/api/v1/admin/books/{book_id}/detect-text-chapters",
            post(audiobook_routes::detect_text_chapters_endpoint),
        )
        .route(
            "/api/v1/admin/books/{book_id}/audio-position",
            get(audiobook_routes::get_audio_position).put(audiobook_routes::save_audio_position),
        )
        // Audiobook bookmarks
        .route(
            "/api/v1/admin/books/{book_id}/bookmarks",
            get(audiobook_routes::list_bookmarks).post(audiobook_routes::create_bookmark),
        )
        .route(
            "/api/v1/admin/books/{book_id}/bookmarks/{bookmark_id}",
            delete(audiobook_routes::delete_bookmark),
        )
        // Transcript & cross-format sync
        .route(
            "/api/v1/admin/books/{book_id}/transcript/status",
            get(audiobook_routes::transcript_status),
        )
        .route(
            "/api/v1/admin/books/{book_id}/transcribe",
            post(audiobook_routes::transcribe_audiobook),
        )
        .route(
            "/api/v1/admin/books/bulk-transcribe",
            post(audiobook_routes::bulk_transcribe),
        )
        .route(
            "/api/v1/admin/books/bulk-chapter-detect",
            post(audiobook_routes::bulk_chapter_detect),
        )
        .route(
            "/api/v1/admin/books/bulk-redetect-chapters",
            post(audiobook_routes::bulk_redetect_chapters),
        )
        .route(
            "/api/v1/admin/books/bulk-ebook-extract",
            post(audiobook_routes::bulk_ebook_extract),
        )
        .route(
            "/api/v1/admin/books/repair-ebooks",
            post(audiobook_routes::repair_ebooks),
        )
        .route(
            "/api/v1/admin/books/repair-audiobook-chapters",
            post(audiobook_routes::repair_audiobook_chapters),
        )
        .route(
            "/api/v1/admin/books/kepubify-synced",
            post(audiobook_routes::kepubify_synced),
        )
        .route(
            "/api/v1/admin/books/{book_id}/extract-text",
            post(audiobook_routes::extract_ebook_text),
        )
        .route(
            "/api/v1/admin/books/{book_id}/transcript/search",
            get(audiobook_routes::search_transcript),
        )
        .route(
            "/api/v1/admin/books/{book_id}/sync-position",
            get(audiobook_routes::get_sync_position),
        )
        .route(
            "/api/v1/admin/books/{book_id}/transcript/segments",
            get(audiobook_routes::get_transcript_segments),
        )
        .route(
            "/api/v1/admin/books/{book_id}/read-along-data",
            get(audiobook_routes::get_read_along_data),
        )
        // TTS (text-to-speech)
        .route("/api/v1/admin/tts/status", get(tts_routes::get_tts_status))
        .route("/api/v1/admin/tts/synthesize", post(tts_routes::synthesize))
        .route(
            "/api/v1/admin/tts/presets",
            get(tts_routes::list_presets).post(tts_routes::create_preset),
        )
        .route(
            "/api/v1/admin/tts/presets/active",
            get(tts_routes::get_active_preset).put(tts_routes::set_active_preset),
        )
        .route(
            "/api/v1/admin/tts/presets/{preset_id}",
            put(tts_routes::update_preset).delete(tts_routes::delete_preset),
        )
        .route(
            "/api/v1/admin/tts/presets/{preset_id}/clip",
            get(tts_routes::get_preset_clip)
                .post(tts_routes::upload_preset_clip)
                .delete(tts_routes::delete_preset_clip),
        )
        .route(
            "/api/v1/admin/tts/presets/{preset_id}/clip-from-audiobook",
            post(tts_routes::create_preset_clip_from_audiobook),
        )
        .route(
            "/api/v1/admin/tts/presets/{preset_id}/transcribe-clip",
            post(tts_routes::transcribe_preset_clip),
        )
        .route(
            "/api/v1/admin/tts/preview-sample",
            post(tts_routes::preview_sample),
        )
        .route(
            "/api/v1/admin/tts/audiobook-clip",
            get(tts_routes::get_audiobook_clip),
        )
        .route(
            "/api/v1/admin/tts/audiobook-clip/suggest",
            get(tts_routes::suggest_audiobook_clip),
        )
        // Reading status & rating
        .route(
            "/api/v1/admin/books/{book_id}/reading",
            get(admin::get_reading_data).put(admin::update_reading_data),
        )
        .route(
            "/api/v1/admin/books/{book_id}/reading/log-time",
            post(admin::log_reading_time),
        )
        .route(
            "/api/v1/admin/books/{book_id}/move-to-media-root",
            post(admin::book_moves::move_book_to_media_root),
        )
        .route(
            "/api/v1/admin/proxy-logs",
            get(admin::list_proxy_logs).delete(admin::clear_proxy_logs),
        )
        .route(
            "/api/v1/admin/proxy-logs/export",
            get(admin::export_proxy_logs),
        )
        .route(
            "/api/v1/admin/proxy-logs/{log_id}/blob",
            get(admin::download_proxy_log_blob),
        )
        .route("/api/v1/admin/uuid-resolve", get(admin::resolve_uuids))
        .route(
            "/api/v1/admin/metadata-cache",
            delete(admin::clear_metadata_cache),
        )
        .route(
            "/api/v1/admin/metadata-write-all",
            post(admin::metadata_write_all),
        )
        .route(
            "/api/v1/admin/library-cleanup",
            post(admin::trigger_library_cleanup),
        )
        .route(
            "/api/v1/admin/config",
            get(admin::get_config).patch(admin::update_config),
        )
        // Backup, restore, and maintenance
        .route("/api/v1/admin/backup", get(admin::download_backup))
        .route("/api/v1/admin/restore", post(admin::restore_backup))
        .route(
            "/api/v1/admin/orphan-cleanup",
            post(admin::trigger_orphan_cleanup),
        )
        // Recents (command palette frecency)
        .route(
            "/api/v1/admin/recents",
            get(admin::list_recents)
                .post(admin::record_recent)
                .delete(admin::clear_recents),
        )
        // Search (command palette)
        .route("/api/v1/admin/search", get(admin::search))
        .route("/api/v1/admin/search/suggestions", get(admin::suggestions))
        .route("/api/v1/admin/search/status", get(admin::search_status))
        .route("/api/v1/admin/search/test", post(admin::test_connection))
        .route("/api/v1/admin/search/reindex", post(admin::trigger_reindex))
        .route("/api/v1/admin/search/sync-static", post(admin::sync_static))
        // Chat (Meilisearch RAG proxy)
        .route(
            "/api/v1/admin/chat/completions",
            post(admin::chat_completions),
        )
        .route("/api/v1/admin/chat/status", get(admin::chat_status))
        // Chat identities (bot ↔ kobbler user bindings)
        .route(
            "/api/v1/admin/chat/identities",
            get(admin::list_chat_identities).post(admin::create_chat_identity),
        )
        .route(
            "/api/v1/admin/chat/identities/{identity_id}",
            patch(admin::update_chat_identity).delete(admin::delete_chat_identity),
        )
        .route(
            "/api/v1/admin/chat/pairings/{pairing_id}",
            delete(admin::reject_chat_pairing),
        )
        .route(
            "/api/v1/admin/chat/pairings/{pairing_id}/approve",
            post(admin::approve_chat_pairing),
        )
        // Chat bot webhook — unauthenticated, verified by the transport itself
        .route(
            "/api/v1/chat/{transport}/webhook",
            post(routes::chat_webhook::chat_webhook),
        )
        .route("/api/v1/admin/chat/logs", get(admin::list_chat_logs))
        .route("/api/v1/admin/chat/logs/{log_id}", get(admin::get_chat_log))
        // Chat memory — Tier 3 profile facts and session summaries, owner-scoped
        .route(
            "/api/v1/admin/chat/memory",
            get(admin::get_chat_memory).delete(admin::forget_chat_memory),
        )
        .route(
            "/api/v1/admin/chat/memory/users",
            get(admin::list_chat_memory_users),
        )
        .route(
            "/api/v1/admin/chat/memory/facts",
            post(admin::create_chat_fact),
        )
        .route(
            "/api/v1/admin/chat/memory/facts/{fact_id}",
            patch(admin::update_chat_fact).delete(admin::delete_chat_fact),
        )
        .route(
            "/api/v1/admin/search/transcripts",
            get(audiobook_routes::search_all_transcripts),
        )
        // Migration (Sonarr/Radarr)
        .route(
            "/api/v1/admin/migration/test",
            post(routes::admin::migration::test_connection),
        )
        .route(
            "/api/v1/admin/migration/preview",
            post(routes::admin::migration::preview),
        )
        .route(
            "/api/v1/admin/migration/import",
            post(routes::admin::migration::import_items),
        )
        // Dev snapshot (dev mode only)
        .route(
            "/api/v1/admin/dev/snapshot",
            get(admin::get_dev_snapshot_status).delete(admin::delete_dev_snapshot),
        )
        .route(
            "/api/v1/admin/dev/snapshot/save",
            post(admin::save_dev_snapshot),
        )
        // Filesystem browsing
        .route(
            "/api/v1/admin/filesystem/browse",
            get(routes::admin::browse_directories),
        )
        .route(
            "/api/v1/admin/filesystem/detect-media-type",
            get(routes::admin::detect_media_type),
        )
        // Media roots
        .route(
            "/api/v1/admin/media-roots",
            get(media_root_routes::list_media_roots).post(media_root_routes::create_media_root),
        )
        .route(
            "/api/v1/admin/media-roots/{id}",
            get(media_root_routes::get_media_root)
                .put(media_root_routes::update_media_root)
                .delete(media_root_routes::delete_media_root),
        )
        .route(
            "/api/v1/admin/media-roots/{id}/set-default",
            post(media_root_routes::set_default_media_root),
        )
        .route(
            "/api/v1/admin/media-roots/{id}/scan",
            post(media_root_routes::trigger_scan),
        )
        .route(
            "/api/v1/admin/media-roots/{id}/files",
            get(media_root_routes::list_files),
        )
        .route(
            "/api/v1/admin/media-roots/{id}/directories",
            get(media_root_routes::list_directories),
        )
        .route(
            "/api/v1/admin/media-roots/{id}/directory-action",
            post(media_root_routes::directory_action),
        )
        .route(
            "/api/v1/admin/media-roots/{id}/entity-groups",
            get(media_root_routes::list_entity_groups),
        )
        .route(
            "/api/v1/admin/media-roots/{id}/status-counts",
            get(media_root_routes::list_status_counts),
        )
        .route(
            "/api/v1/admin/media-roots/{id}/files/search",
            get(media_root_routes::search_files),
        )
        .route(
            "/api/v1/admin/media-roots/{id}/reorganize",
            post(media_root_routes::trigger_reorganize),
        )
        .route(
            "/api/v1/admin/media-roots/{id}/backfill",
            post(media_root_routes::trigger_backfill),
        )
        .route(
            "/api/v1/admin/media-roots/sidecar-rewrite",
            post(media_root_routes::trigger_sidecar_rewrite),
        )
        .route(
            "/api/v1/admin/media-roots/{id}/write-sidecars",
            post(media_root_routes::trigger_write_sidecars),
        )
        .route(
            "/api/v1/admin/media-roots/{id}/write-images",
            post(media_root_routes::trigger_write_images),
        )
        .route(
            "/api/v1/admin/media-roots/{id}/reinterpret",
            post(media_root_routes::reinterpret_media_root),
        )
        .route(
            "/api/v1/admin/media-roots/{id}/match-to-library",
            post(media_root_routes::match_to_library),
        )
        .route(
            "/api/v1/admin/media-roots/files/{id}",
            get(media_root_routes::get_file),
        )
        .route(
            "/api/v1/admin/media-roots/files/{id}/match",
            post(media_root_routes::resolve_match),
        )
        .route(
            "/api/v1/admin/media-roots/files/{id}/add",
            post(media_root_routes::add_to_library),
        )
        .route(
            "/api/v1/admin/media-roots/files/{id}/ignore",
            post(media_root_routes::ignore_file),
        )
        .route(
            "/api/v1/admin/media-roots/files/{id}/rematch",
            post(media_root_routes::rematch_file),
        )
        .route(
            "/api/v1/admin/media-roots/files/bulk-resolve",
            post(media_root_routes::bulk_resolve),
        )
        .route(
            "/api/v1/admin/media-roots/files/bulk-manual-resolve",
            post(media_root_routes::bulk_manual_resolve),
        )
        .route(
            "/api/v1/admin/media-roots/files/bulk-rematch",
            post(media_root_routes::bulk_rematch),
        )
        .route(
            "/api/v1/admin/media-roots/files/bulk-ignore",
            post(media_root_routes::bulk_ignore),
        )
        .route(
            "/api/v1/admin/media-roots/files/{id}/link",
            post(media_root_routes::link_to_entity),
        )
        .route(
            "/api/v1/admin/media-roots/files/bulk-link",
            post(media_root_routes::bulk_link_to_entity),
        )
        // Unified event stream (SSE)
        .route("/api/v1/admin/events", get(routes::events::events_stream))
        // Ingest queue
        .route(
            "/api/v1/admin/ingest-queue",
            get(watcher_routes::list_ingest_queue).post(watcher_routes::upload_book),
        )
        .route(
            "/api/v1/admin/ingest-queue/{id}",
            get(watcher_routes::get_ingest_queue_item)
                .delete(watcher_routes::delete_ingest_queue_item),
        )
        .route(
            "/api/v1/admin/ingest-queue/{id}/approve",
            post(watcher_routes::approve_ingest_item),
        )
        .route(
            "/api/v1/admin/ingest-queue/{id}/reject",
            post(watcher_routes::reject_ingest_item),
        )
        .route(
            "/api/v1/admin/ingest-queue/batch-reject",
            post(watcher_routes::batch_reject_ingest_items),
        )
        .route(
            "/api/v1/admin/ingest-queue/{id}/restore",
            post(watcher_routes::restore_ingest_item),
        )
        .route(
            "/api/v1/admin/ingest-queue/{id}/cover",
            get(watcher_routes::get_ingest_queue_cover).put(watcher_routes::upload_ingest_cover),
        )
        .route(
            "/api/v1/admin/ingest-queue/{id}/extract-metadata",
            post(watcher_routes::extract_metadata),
        )
        .route(
            "/api/v1/admin/wanted",
            get(wanted_routes::list_wanted).post(wanted_routes::create_wanted),
        )
        .route(
            "/api/v1/admin/wanted/{id}",
            get(wanted_routes::get_wanted).delete(wanted_routes::delete_wanted),
        )
        .route(
            "/api/v1/admin/books/{book_id}/releases",
            get(wanted_routes::search_releases),
        )
        .route(
            "/api/v1/admin/books/{book_id}/queue",
            post(wanted_routes::queue_book_release),
        )
        .route(
            "/api/v1/admin/wanted/{id}/files",
            get(wanted_routes::get_wanted_files),
        )
        .route(
            "/api/v1/admin/wanted/{id}/ingest-files",
            post(wanted_routes::ingest_selected_files),
        )
        .route(
            "/api/v1/admin/wanted/{id}/skip",
            post(wanted_routes::skip_wanted_review),
        )
        .route(
            "/api/v1/admin/wanted/{id}/status",
            get(wanted_routes::get_wanted_status),
        )
        .route(
            "/api/v1/admin/wanted/{id}/search-now",
            post(wanted_routes::search_wanted_now),
        )
        .route(
            "/api/v1/admin/wanted/{id}/grab-held",
            post(wanted_routes::grab_wanted_held),
        )
        .route(
            "/api/v1/admin/wanted/{id}/reject-download",
            post(wanted_routes::reject_wanted_download),
        )
        .route(
            "/api/v1/wanted/watch-callback/{token}",
            post(wanted_routes::watch_callback),
        )
        .route("/api/v1/ingest/{token}", post(ingest::kobload_ingest))
        .route(
            "/api/v1/admin/test-kobload",
            post(wanted_routes::test_kobload_connection),
        )
        .route(
            "/api/v1/admin/kobload/quality-profiles",
            get(wanted_routes::list_quality_profiles),
        )
        // Metadata search
        .route(
            "/api/v1/admin/metadata/search",
            get(metadata_routes::search_metadata),
        )
        .route(
            "/api/v1/admin/metadata/search-series",
            get(metadata_routes::search_series),
        )
        .route(
            "/api/v1/admin/metadata/merge",
            post(metadata_routes::merge_metadata),
        )
        .route(
            "/api/v1/admin/metadata/search-authors",
            get(metadata_routes::search_authors),
        )
        .route(
            "/api/v1/admin/metadata/providers",
            get(metadata_routes::list_providers),
        )
        .route(
            "/api/v1/admin/metadata/providers/{id}",
            patch(metadata_routes::update_provider),
        )
        .route(
            "/api/v1/admin/ingest-queue/{id}/apply-metadata",
            post(metadata_routes::apply_metadata),
        )
        // Video metadata search
        .route(
            "/api/v1/admin/video-metadata/providers",
            get(video_metadata_routes::list_providers),
        )
        .route(
            "/api/v1/admin/video-metadata/providers/{id}",
            patch(video_metadata_routes::update_provider),
        )
        .route(
            "/api/v1/admin/video-metadata/search/movies",
            get(video_metadata_routes::search_movies),
        )
        .route(
            "/api/v1/admin/video-metadata/search/tv",
            get(video_metadata_routes::search_tv),
        )
        .route(
            "/api/v1/admin/video-metadata/movies/{provider_id}",
            get(video_metadata_routes::get_movie_details),
        )
        .route(
            "/api/v1/admin/video-metadata/tv/{provider_id}",
            get(video_metadata_routes::get_tv_details),
        )
        .route(
            "/api/v1/admin/video-metadata/tv/{provider_id}/season/{season_number}",
            get(video_metadata_routes::get_season_details),
        )
        .route(
            "/api/v1/admin/video-metadata/merge",
            post(video_metadata_routes::merge_results),
        )
        // Notification agents
        .route(
            "/api/v1/admin/notifications/types",
            get(notification_routes::list_types),
        )
        .route(
            "/api/v1/admin/notifications/agents",
            get(notification_routes::list_agents).post(notification_routes::create_agent),
        )
        .route(
            "/api/v1/admin/notifications/agents/{id}",
            patch(notification_routes::update_agent).delete(notification_routes::delete_agent),
        )
        .route(
            "/api/v1/admin/notifications/agents/{id}/test",
            post(notification_routes::test_agent),
        )
        .route(
            "/api/v1/admin/notifications/webpush/vapid-key",
            get(notification_routes::get_vapid_key),
        )
        .route(
            "/api/v1/admin/notifications/webpush/subscribe",
            post(notification_routes::webpush_subscribe)
                .delete(notification_routes::webpush_unsubscribe),
        )
        // Notification inbox (in-app)
        .route(
            "/api/v1/admin/notifications/inbox",
            get(inbox_routes::list_inbox),
        )
        .route(
            "/api/v1/admin/notifications/inbox/unread-count",
            get(inbox_routes::unread_count),
        )
        .route(
            "/api/v1/admin/notifications/inbox/{id}/read",
            patch(inbox_routes::mark_read),
        )
        .route(
            "/api/v1/admin/notifications/inbox/mark-all-read",
            post(inbox_routes::mark_all_read),
        )
        // Subtitle providers
        .route(
            "/api/v1/admin/subtitle-providers",
            get(subtitle_provider_routes::list_providers),
        )
        .route(
            "/api/v1/admin/subtitle-providers/{id}",
            patch(subtitle_provider_routes::update_provider),
        )
        // Video sync providers (Trakt, etc.)
        .route(
            "/api/v1/admin/video-sync/providers",
            get(video_sync_routes::list_providers),
        )
        .route(
            "/api/v1/admin/video-sync/providers/{provider_type}",
            patch(video_sync_routes::update_provider),
        )
        .route(
            "/api/v1/admin/video-sync/trakt/authorize",
            post(video_sync_routes::trakt_authorize),
        )
        .route(
            "/api/v1/admin/video-sync/trakt/poll/{device_code}",
            post(video_sync_routes::trakt_poll),
        )
        .route(
            "/api/v1/admin/video-sync/trakt/disconnect",
            post(video_sync_routes::trakt_disconnect),
        )
        .route(
            "/api/v1/admin/video-sync/trakt/import-history",
            post(video_sync_routes::trakt_import_history),
        )
        // Movies
        .route(
            "/api/v1/admin/movies",
            get(movie_routes::list_movies).post(movie_routes::create_movie),
        )
        .route(
            "/api/v1/admin/movies/{id}",
            get(movie_routes::get_movie)
                .patch(movie_routes::update_movie)
                .delete(movie_routes::delete_movie),
        )
        .route(
            "/api/v1/admin/movies/{id}/want",
            post(movie_routes::want_movie).delete(movie_routes::unwant_movie),
        )
        .route(
            "/api/v1/admin/movies/{id}/files",
            get(movie_routes::list_movie_files).post(movie_routes::add_movie_file),
        )
        .route(
            "/api/v1/admin/movies/{id}/files/{file_id}",
            delete(movie_routes::delete_movie_file),
        )
        .route(
            "/api/v1/admin/movies/{id}/files/{file_id}/move",
            post(movie_routes::move_movie_file),
        )
        .route(
            "/api/v1/admin/movies/{id}/refresh-metadata",
            post(movie_routes::refresh_metadata),
        )
        .route(
            "/api/v1/admin/movies/{id}/watch-state",
            get(movie_routes::get_watch_state).put(movie_routes::update_watch_state),
        )
        .route(
            "/api/v1/admin/movies/{id}/watch-state/position",
            patch(movie_routes::update_position),
        )
        .route(
            "/api/v1/admin/movies/{id}/poster",
            get(movie_routes::get_poster).post(movie_routes::upload_poster),
        )
        .route(
            "/api/v1/admin/movies/{id}/backdrop",
            get(movie_routes::get_backdrop).post(movie_routes::upload_backdrop),
        )
        .route(
            "/api/v1/admin/movies/{id}/logo",
            get(movie_routes::get_logo).post(movie_routes::upload_logo),
        )
        .route(
            "/api/v1/admin/movies/{id}/cast",
            put(movie_routes::replace_movie_cast),
        )
        .route(
            "/api/v1/admin/movies/{id}/crew",
            put(movie_routes::replace_movie_crew),
        )
        .route(
            "/api/v1/admin/movies/first-by-letter",
            get(movie_routes::first_by_letter),
        )
        .route(
            "/api/v1/admin/movies/available-letters",
            get(movie_routes::available_letters),
        )
        .route("/api/v1/admin/movies/facets", get(movie_routes::facets))
        .route("/api/v1/admin/movies/genres", get(movie_routes::genres))
        .route(
            "/api/v1/admin/movies/{id}/tracks",
            get(movie_routes::get_movie_tracks),
        )
        .route(
            "/api/v1/admin/movies/{id}/subtitles/search",
            post(subtitle_routes::search_movie_subtitles),
        )
        .route(
            "/api/v1/admin/movies/{id}/releases",
            get(movie_routes::search_movie_releases),
        )
        .route(
            "/api/v1/admin/movies/{id}/queue",
            post(movie_routes::queue_movie_release),
        )
        .route(
            "/api/v1/admin/movies/files/{file_id}/skip-segments",
            put(movie_routes::set_movie_skip_segments),
        )
        .route(
            "/api/v1/admin/movies/{id}/media-analyze",
            post(movie_routes::run_media_analyze_movie),
        )
        // TV Shows
        .route(
            "/api/v1/admin/tv/shows",
            get(tv_routes::list_shows).post(tv_routes::create_show),
        )
        .route(
            "/api/v1/admin/tv/shows/bulk",
            post(tv_routes::bulk_update_shows),
        )
        .route(
            "/api/v1/admin/tv/shows/{id}",
            get(tv_routes::get_show)
                .patch(tv_routes::update_show)
                .delete(tv_routes::delete_show),
        )
        .route(
            "/api/v1/admin/tv/shows/{id}/monitor",
            post(tv_routes::set_monitor),
        )
        .route(
            "/api/v1/admin/tv/shows/{id}/refresh-metadata",
            post(tv_routes::refresh_metadata),
        )
        .route(
            "/api/v1/admin/tv/shows/{id}/want-all-missing",
            post(tv_routes::want_all_missing).delete(tv_routes::unwant_all),
        )
        .route(
            "/api/v1/admin/tv/shows/{id}/media-analyze",
            post(tv_routes::run_media_analyze_show),
        )
        .route(
            "/api/v1/admin/tv/seasons/{id}/media-analyze",
            post(tv_routes::run_media_analyze_season),
        )
        .route(
            "/api/v1/admin/tv/shows/{id}/poster",
            get(tv_routes::get_show_poster).post(tv_routes::upload_show_poster),
        )
        .route(
            "/api/v1/admin/tv/shows/{id}/backdrop",
            get(tv_routes::get_show_backdrop).post(tv_routes::upload_show_backdrop),
        )
        .route(
            "/api/v1/admin/tv/shows/{id}/logo",
            get(tv_routes::get_show_logo).post(tv_routes::upload_show_logo),
        )
        .route(
            "/api/v1/admin/tv/shows/{id}/cast",
            put(tv_routes::replace_show_cast),
        )
        .route(
            "/api/v1/admin/tv/shows/{id}/crew",
            put(tv_routes::replace_show_crew),
        )
        .route(
            "/api/v1/admin/tv/shows/{id}/rating",
            get(tv_routes::get_show_rating)
                .put(tv_routes::update_show_rating)
                .delete(tv_routes::delete_show_rating),
        )
        .route(
            "/api/v1/admin/tv/shows/{id}/watch-state",
            put(tv_routes::bulk_update_show_watch_state),
        )
        .route(
            "/api/v1/admin/tv/shows/first-by-letter",
            get(tv_routes::first_by_letter),
        )
        .route(
            "/api/v1/admin/tv/shows/available-letters",
            get(tv_routes::available_letters),
        )
        .route("/api/v1/admin/tv/shows/facets", get(tv_routes::facets))
        .route("/api/v1/admin/tv/shows/genres", get(tv_routes::genres))
        // TV Seasons
        .route(
            "/api/v1/admin/tv/shows/{id}/seasons",
            post(tv_routes::create_season),
        )
        .route(
            "/api/v1/admin/tv/seasons/{id}",
            get(tv_routes::get_season)
                .patch(tv_routes::update_season)
                .delete(tv_routes::delete_season),
        )
        .route(
            "/api/v1/admin/tv/seasons/{id}/want",
            post(tv_routes::want_season).delete(tv_routes::unwant_season),
        )
        .route(
            "/api/v1/admin/tv/seasons/{id}/auto-want",
            post(tv_routes::set_season_auto_want),
        )
        .route(
            "/api/v1/admin/tv/seasons/{id}/watch-state",
            put(tv_routes::bulk_update_season_watch_state),
        )
        .route(
            "/api/v1/admin/tv/seasons/{id}/poster",
            get(tv_routes::get_season_poster).post(tv_routes::upload_season_poster),
        )
        // TV Episodes
        .route(
            "/api/v1/admin/tv/seasons/{id}/episodes",
            post(tv_routes::create_episode),
        )
        .route(
            "/api/v1/admin/tv/episodes/bulk",
            post(tv_routes::bulk_episodes),
        )
        .route(
            "/api/v1/admin/tv/episodes/{id}",
            get(tv_routes::get_episode)
                .patch(tv_routes::update_episode)
                .delete(tv_routes::delete_episode),
        )
        .route(
            "/api/v1/admin/tv/episodes/{id}/next",
            get(tv_routes::next_episode),
        )
        .route(
            "/api/v1/admin/tv/episodes/{id}/cast",
            get(episode_cast_routes::get_episode_cast),
        )
        .route(
            "/api/v1/admin/tv/episodes/{id}/want",
            post(tv_routes::want_episode).delete(tv_routes::unwant_episode),
        )
        .route(
            "/api/v1/admin/tv/episodes/{id}/files",
            get(tv_routes::list_episode_files).post(tv_routes::add_episode_file),
        )
        .route(
            "/api/v1/admin/tv/episodes/{id}/files/{file_id}",
            delete(tv_routes::delete_episode_file),
        )
        .route(
            "/api/v1/admin/tv/episodes/{id}/files/{file_id}/move",
            post(tv_routes::move_episode_file),
        )
        .route(
            "/api/v1/admin/tv/episodes/{id}/watch-state",
            get(tv_routes::get_watch_state).put(tv_routes::update_watch_state),
        )
        .route(
            "/api/v1/admin/tv/episodes/{id}/watch-state/position",
            patch(tv_routes::update_episode_position),
        )
        .route(
            "/api/v1/admin/tv/episodes/{id}/still",
            get(tv_routes::get_episode_still).post(tv_routes::upload_episode_still),
        )
        .route(
            "/api/v1/admin/tv/episodes/{id}/tracks",
            get(tv_routes::get_episode_tracks),
        )
        .route(
            "/api/v1/admin/tv/episodes/{id}/subtitles/search",
            post(subtitle_routes::search_episode_subtitles),
        )
        .route(
            "/api/v1/admin/tv/episodes/files/{file_id}/skip-segments",
            put(tv_routes::set_skip_segments),
        )
        .route(
            "/api/v1/admin/tv/episodes/{id}/releases",
            get(tv_routes::search_episode_releases),
        )
        .route(
            "/api/v1/admin/tv/episodes/{id}/queue",
            post(tv_routes::queue_episode_release),
        )
        // Sports (Leagues/Seasons/Events)
        .route(
            "/api/v1/admin/sports/leagues",
            get(sport_routes::list_leagues).post(sport_routes::create_league),
        )
        .route(
            "/api/v1/admin/sports/leagues/{id}",
            get(sport_routes::get_league)
                .patch(sport_routes::update_league)
                .delete(sport_routes::delete_league),
        )
        .route(
            "/api/v1/admin/sports/leagues/{id}/monitor",
            post(sport_routes::set_monitor),
        )
        .route(
            "/api/v1/admin/sports/leagues/{id}/refresh",
            post(sport_routes::refresh_metadata),
        )
        .route(
            "/api/v1/admin/sports/leagues/{id}/want-all-missing",
            post(sport_routes::want_all_missing).delete(sport_routes::unwant_all_missing),
        )
        .route(
            "/api/v1/admin/sports/leagues/{id}/poster",
            get(sport_routes::get_league_poster),
        )
        .route(
            "/api/v1/admin/sports/leagues/{id}/backdrop",
            get(sport_routes::get_league_backdrop),
        )
        .route(
            "/api/v1/admin/sports/leagues/{id}/logo",
            get(sport_routes::get_league_logo),
        )
        .route(
            "/api/v1/admin/sports/leagues/first-by-letter",
            get(sport_routes::first_by_letter),
        )
        .route(
            "/api/v1/admin/sports/leagues/available-letters",
            get(sport_routes::available_letters),
        )
        .route(
            "/api/v1/admin/sports/leagues/{id}/seasons",
            get(sport_routes::list_league_seasons).post(sport_routes::create_season),
        )
        .route(
            "/api/v1/admin/sports/seasons/{id}",
            get(sport_routes::get_season).patch(sport_routes::update_season),
        )
        .route(
            "/api/v1/admin/sports/seasons/{id}/want",
            post(sport_routes::want_season).delete(sport_routes::unwant_season),
        )
        .route(
            "/api/v1/admin/sports/seasons/{id}/auto-want",
            post(sport_routes::set_season_auto_want),
        )
        .route(
            "/api/v1/admin/sports/seasons/{id}/events",
            post(sport_routes::create_event),
        )
        .route(
            "/api/v1/admin/sports/events/{id}",
            get(sport_routes::get_event)
                .patch(sport_routes::update_event)
                .delete(sport_routes::delete_event),
        )
        .route(
            "/api/v1/admin/sports/events/{id}/want",
            post(sport_routes::want_event).delete(sport_routes::unwant_event),
        )
        .route(
            "/api/v1/admin/sports/events/{id}/files",
            get(sport_routes::list_event_files),
        )
        .route(
            "/api/v1/admin/sports/events/{event_id}/files/{file_id}/move",
            post(sport_routes::move_event_file),
        )
        .route(
            "/api/v1/admin/sports/events/{id}/releases",
            get(sport_routes::search_event_releases),
        )
        .route(
            "/api/v1/admin/sports/events/{id}/queue",
            post(sport_routes::queue_event_release),
        )
        .route(
            "/api/v1/admin/sports/search",
            get(sport_routes::search_leagues),
        )
        // IPTV
        .route(
            "/api/v1/admin/iptv/providers",
            get(iptv_routes::list_providers).post(iptv_routes::create_provider),
        )
        .route(
            "/api/v1/admin/iptv/providers/{id}",
            get(iptv_routes::get_provider)
                .patch(iptv_routes::update_provider)
                .delete(iptv_routes::delete_provider),
        )
        .route(
            "/api/v1/admin/iptv/providers/{id}/sync",
            post(iptv_routes::sync_provider),
        )
        .route(
            "/api/v1/admin/iptv/channels",
            get(iptv_routes::list_channels),
        )
        .route(
            "/api/v1/admin/iptv/channels/{id}",
            get(iptv_routes::get_channel).patch(iptv_routes::update_channel),
        )
        .route(
            "/api/v1/admin/iptv/channels/{id}/icon",
            get(iptv_routes::get_channel_icon),
        )
        .route(
            "/api/v1/admin/iptv/channels/{id}/epg",
            get(iptv_routes::get_channel_epg),
        )
        .route(
            "/api/v1/admin/iptv/channels/{id}/sources",
            get(iptv_routes::get_channel_sources),
        )
        .route(
            "/api/v1/admin/iptv/channels/merge",
            post(iptv_routes::merge_channels),
        )
        .route(
            "/api/v1/admin/iptv/channels/bulk-enable",
            post(iptv_routes::bulk_enable_channels),
        )
        .route(
            "/api/v1/admin/iptv/channels/cleanup",
            post(iptv_routes::cleanup_channels),
        )
        .route(
            "/api/v1/admin/iptv/channels/rematch",
            post(iptv_routes::rematch_channels),
        )
        .route(
            "/api/v1/admin/iptv/channels/{id}/play",
            get(iptv_routes::play_channel),
        )
        .route(
            "/api/v1/admin/iptv/channels/{id}/play/{segment}",
            get(iptv_routes::play_segment),
        )
        .route(
            "/api/v1/admin/iptv/channels/{id}/play.ts",
            get(iptv_routes::play_ts),
        )
        .route(
            "/api/v1/admin/iptv/groups",
            get(iptv_routes::list_groups).post(iptv_routes::create_group),
        )
        .route(
            "/api/v1/admin/iptv/groups/{id}",
            patch(iptv_routes::update_group).delete(iptv_routes::delete_group),
        )
        .route(
            "/api/v1/admin/iptv/unmatched",
            get(iptv_routes::list_unmatched),
        )
        .route(
            "/api/v1/admin/iptv/unmatched/{id}/assign",
            post(iptv_routes::assign_unmatched),
        )
        .route(
            "/api/v1/admin/iptv/unmatched/{id}/create-channel",
            post(iptv_routes::create_channel_from_unmatched),
        )
        .route(
            "/api/v1/admin/iptv/unmatched/{id}/dismiss",
            post(iptv_routes::dismiss_unmatched),
        )
        .route(
            "/api/v1/admin/iptv/unmatched/{id}",
            delete(iptv_routes::delete_unmatched),
        )
        .route("/api/v1/admin/iptv/epg", get(iptv_routes::get_epg_grid))
        .route(
            "/api/v1/admin/iptv/epg/search",
            get(iptv_routes::search_epg),
        )
        .route("/api/v1/admin/iptv/epg/now", get(iptv_routes::get_epg_now))
        .route(
            "/api/v1/admin/iptv/epg/favourites",
            get(iptv_routes::get_epg_favourites),
        )
        .route("/api/v1/admin/iptv/catchup", post(iptv_routes::catchup))
        .route(
            "/api/v1/admin/iptv/recordings",
            get(iptv_routes::list_recordings).post(iptv_routes::create_recording),
        )
        .route(
            "/api/v1/admin/iptv/recordings/from-epg",
            post(iptv_routes::record_from_epg),
        )
        .route(
            "/api/v1/admin/iptv/recordings/{id}",
            delete(iptv_routes::delete_recording),
        )
        .route(
            "/api/v1/admin/iptv/recordings/{id}/stream",
            get(iptv_routes::stream_recording),
        )
        .route(
            "/api/v1/admin/iptv/series-rules",
            get(iptv_routes::list_series_rules).post(iptv_routes::create_series_rule),
        )
        .route(
            "/api/v1/admin/iptv/series-rules/{id}",
            patch(iptv_routes::update_series_rule).delete(iptv_routes::delete_series_rule),
        )
        .route(
            "/api/v1/admin/iptv/watches",
            get(iptv_routes::list_watches).post(iptv_routes::create_watch),
        )
        .route(
            "/api/v1/admin/iptv/watches/matches",
            get(iptv_routes::get_watch_matches),
        )
        .route(
            "/api/v1/admin/iptv/watches/{id}",
            delete(iptv_routes::delete_watch),
        )
        .route(
            "/api/v1/admin/iptv/favourites",
            get(iptv_routes::list_favourites).post(iptv_routes::add_favourite),
        )
        .route(
            "/api/v1/admin/iptv/favourites/{id}",
            delete(iptv_routes::remove_favourite),
        )
        .route(
            "/api/v1/admin/iptv/favourites/reorder",
            post(iptv_routes::reorder_favourites),
        )
        .route(
            "/api/v1/admin/iptv/favourites/groups",
            get(iptv_routes::list_favourite_groups),
        )
        .route(
            "/api/v1/admin/iptv/favourites/groups/{id}",
            post(iptv_routes::add_favourite_group).delete(iptv_routes::remove_favourite_group),
        )
        .route(
            "/api/v1/admin/iptv/history",
            get(iptv_routes::list_history).post(iptv_routes::record_watch),
        )
        .route(
            "/api/v1/admin/iptv/settings",
            get(iptv_routes::get_settings).patch(iptv_routes::update_settings),
        )
        .route(
            "/api/v1/admin/iptv/sessions",
            post(iptv_routes::create_iptv_session),
        )
        // Live TV (HDHomeRun)
        .route(
            "/api/v1/admin/diagnostics/playback",
            get(diagnostics_routes::list_playback_failures)
                .post(diagnostics_routes::report_playback_failure),
        )
        .route(
            "/api/v1/admin/livetv/tuners",
            get(livetv_routes::list_tuners).post(livetv_routes::add_tuner),
        )
        .route(
            "/api/v1/admin/livetv/tuners/{id}",
            delete(livetv_routes::delete_tuner),
        )
        .route(
            "/api/v1/admin/livetv/tuners/{id}/scan",
            post(livetv_routes::scan_tuner),
        )
        .route(
            "/api/v1/admin/livetv/tuners/{id}/guide-sync",
            post(livetv_routes::guide_sync_tuner),
        )
        .route(
            "/api/v1/admin/livetv/channels",
            get(livetv_routes::list_channels),
        )
        .route(
            "/api/v1/admin/livetv/channels/bulk",
            post(livetv_routes::bulk_channels),
        )
        .route(
            "/api/v1/admin/livetv/channels/{id}",
            patch(livetv_routes::update_channel),
        )
        .route("/api/v1/admin/livetv/guide", get(livetv_routes::get_guide))
        .route(
            "/api/v1/admin/livetv/guide/now",
            get(livetv_routes::get_guide_now),
        )
        .route(
            "/api/v1/admin/livetv/guide/{id}",
            get(livetv_routes::get_guide_entry),
        )
        .route(
            "/api/v1/admin/livetv/leases",
            get(livetv_play_routes::list_leases),
        )
        .route(
            "/api/v1/admin/livetv/sessions",
            post(livetv_play_routes::create_session),
        )
        .route(
            "/api/v1/admin/livetv/sessions/{sid}",
            delete(livetv_play_routes::destroy_session),
        )
        .route(
            "/api/v1/admin/livetv/sessions/{sid}/channel",
            post(livetv_play_routes::switch_channel),
        )
        .route(
            "/api/v1/admin/livetv/sessions/{sid}/heartbeat",
            post(livetv_play_routes::heartbeat),
        )
        .route(
            "/api/v1/admin/livetv/sessions/{sid}/buffer",
            get(livetv_play_routes::buffer_status),
        )
        .route(
            "/api/v1/admin/livetv/sessions/{sid}/record",
            post(livetv_play_routes::record_session),
        )
        .route(
            "/api/v1/admin/livetv/sessions/{sid}/stream.m3u8",
            get(livetv_play_routes::stream_playlist),
        )
        .route(
            "/api/v1/admin/livetv/sessions/{sid}/segments/{file}",
            get(livetv_play_routes::serve_segment),
        )
        .route(
            "/api/v1/admin/livetv/sessions/{sid}/thumbnails.vtt",
            get(livetv_play_routes::thumbnails_vtt),
        )
        .route(
            "/api/v1/admin/livetv/sessions/{sid}/thumbnails/{file}",
            get(livetv_play_routes::serve_sprite),
        )
        .route(
            "/api/v1/admin/livetv/channels/{id}/stream.ts",
            get(livetv_play_routes::stream_ts),
        )
        .route(
            "/api/v1/admin/livetv/recordings",
            get(livetv_recording_routes::list_recordings)
                .post(livetv_recording_routes::create_recording),
        )
        .route(
            "/api/v1/admin/livetv/recordings/{id}",
            get(livetv_recording_routes::get_recording)
                .delete(livetv_recording_routes::delete_recording),
        )
        .route(
            "/api/v1/admin/livetv/recordings/{id}/stop",
            post(livetv_recording_routes::stop_recording),
        )
        .route(
            "/api/v1/admin/livetv/recordings/{id}/playback",
            get(livetv_recording_routes::recording_playback),
        )
        .route(
            "/api/v1/admin/livetv/recordings/{id}/stream.m3u8",
            get(livetv_recording_routes::recording_playlist),
        )
        .route(
            "/api/v1/admin/livetv/recordings/{id}/segments/{*file}",
            get(livetv_recording_routes::recording_segment),
        )
        .route(
            "/api/v1/admin/livetv/recordings/{id}/ad-breaks",
            get(livetv_watch_routes::list_ad_breaks),
        )
        .route(
            "/api/v1/admin/livetv/recordings/{id}/ad-breaks.edl",
            get(livetv_watch_routes::ad_breaks_edl),
        )
        .route(
            "/api/v1/admin/livetv/recordings/{id}/ad-breaks/{break_id}",
            patch(livetv_watch_routes::correct_ad_break)
                .delete(livetv_watch_routes::delete_ad_break),
        )
        .route(
            "/api/v1/admin/livetv/watch-events",
            post(livetv_watch_routes::create_watch_event),
        )
        .route(
            "/api/v1/admin/livetv/on-now",
            get(livetv_watch_routes::on_now),
        )
        .route(
            "/api/v1/admin/livetv/series-rules",
            get(livetv_rule_routes::list_series_rules).post(livetv_rule_routes::create_series_rule),
        )
        .route(
            "/api/v1/admin/livetv/series-rules/preview",
            post(livetv_rule_routes::preview_series_rule),
        )
        .route(
            "/api/v1/admin/livetv/series-rules/{id}",
            patch(livetv_rule_routes::update_series_rule)
                .delete(livetv_rule_routes::delete_series_rule),
        )
        // Subtitle download
        .route(
            "/api/v1/admin/subtitles/download",
            post(subtitle_routes::download_subtitle),
        )
        // People
        .route(
            "/api/v1/admin/people",
            get(people_routes::list_people).post(people_routes::create_person),
        )
        .route(
            "/api/v1/admin/people/{id}",
            get(people_routes::get_person)
                .patch(people_routes::update_person)
                .delete(people_routes::delete_person),
        )
        .route(
            "/api/v1/admin/people/{id}/image",
            get(people_routes::get_person_image).post(people_routes::upload_person_image),
        )
        .route(
            "/api/v1/admin/people/{id}/filmography",
            get(people_routes::get_filmography),
        )
        .route(
            "/api/v1/admin/people/{id}/filmography/want",
            post(people_routes::want_credit),
        )
        .route(
            "/api/v1/admin/people/{id}/filmography/dismiss",
            post(people_routes::dismiss_credit).delete(people_routes::undismiss_credit),
        )
        .route(
            "/api/v1/admin/people/{id}/monitoring",
            put(people_routes::update_person_monitoring),
        )
        .route(
            "/api/v1/admin/people/{id}/refresh",
            post(people_routes::refresh_person),
        )
        // Jobs queue
        .route("/api/v1/admin/jobs", get(jobs_routes::list_jobs))
        .route(
            "/api/v1/admin/jobs/running",
            get(jobs_routes::running_count),
        )
        .route("/api/v1/admin/jobs/counts", get(jobs_routes::job_counts))
        .route(
            "/api/v1/admin/jobs/completed",
            delete(jobs_routes::delete_completed_jobs),
        )
        .route(
            "/api/v1/admin/jobs/{job_id}",
            get(jobs_routes::get_job).delete(jobs_routes::delete_job),
        )
        .route(
            "/api/v1/admin/jobs/{job_id}/retry",
            post(jobs_routes::retry_job),
        )
        .route(
            "/api/v1/admin/jobs/{job_id}/cancel",
            post(jobs_routes::cancel_job),
        )
        .route(
            "/api/v1/admin/jobs/concurrency",
            get(jobs_concurrency::get_job_concurrency)
                .put(jobs_concurrency::update_job_concurrency),
        )
        .route(
            "/api/v1/admin/system/nas-health",
            get(jobs_concurrency::get_nas_health),
        )
        .route(
            "/api/v1/admin/system/vision-health",
            get(jobs_concurrency::get_vision_health),
        )
        // Sync providers
        .nest("/api/v1/admin/sync", sync_routes::routes())
        // Library provider (disambiguation)
        .nest(
            "/api/v1/providers/library",
            provider_library_routes::routes(),
        )
        // Kindle
        .route("/api/v1/admin/kindle/oauth-url", get(admin::get_oauth_url))
        .route(
            "/api/v1/admin/kindle/oauth-complete",
            post(admin::complete_oauth),
        )
        .route(
            "/api/v1/admin/kindle/account",
            get(admin::get_account).delete(admin::disconnect_account),
        )
        .route(
            "/api/v1/admin/kindle/refresh-devices",
            post(admin::refresh_devices),
        )
        .route(
            "/api/v1/admin/kindle/targets/{target_id}",
            patch(admin::update_target),
        )
        .route(
            "/api/v1/admin/books/{book_id}/kindle",
            post(admin::send_to_kindle),
        )
        .route(
            "/api/v1/admin/books/{book_id}/kindle/deliveries",
            get(admin::get_book_deliveries),
        )
        // Downloads (Kobload proxy)
        .route("/api/v1/admin/downloads", get(admin::list_downloads))
        .route(
            "/api/v1/admin/downloads/{download_id}/cancel",
            post(admin::cancel_download),
        )
        .route(
            "/api/v1/admin/downloads/{download_id}/retry",
            post(admin::retry_download),
        )
        .route(
            "/api/v1/admin/downloads/{download_id}/retry-callback",
            post(admin::retry_callback),
        )
        .route(
            "/api/v1/admin/library/{media_type}/{entity_id}/reject-download",
            post(admin::reject_download),
        )
        .route(
            "/api/v1/admin/library/{media_type}/{entity_id}/replace",
            post(admin::replace_file),
        )
        // Playback / Streaming
        .route(
            "/api/v1/admin/playback/sessions",
            post(streaming_routes::create_session),
        )
        .route(
            "/api/v1/admin/playback/handoff",
            get(streaming_routes::latest_handoff),
        )
        .route(
            "/api/v1/admin/playback/sessions/{id}",
            delete(streaming_routes::destroy_session),
        )
        .merge(playback_session_routes(&state))
        .route(
            "/api/v1/admin/offline/exports",
            post(routes::admin::offline_exports::create_export),
        )
        .route(
            "/api/v1/admin/offline/exports/{id}",
            get(routes::admin::offline_exports::get_export),
        )
        .route(
            "/api/v1/admin/offline/exports/{id}/file",
            get(routes::admin::offline_exports::download_export),
        )
        .route(
            "/api/v1/admin/playback/enrich-backfill",
            post(streaming_routes::trigger_enrichment_backfill),
        )
        .route(
            "/api/v1/admin/playback/skip-detect-rerun",
            post(streaming_routes::rerun_skip_detect),
        )
        .route(
            "/api/v1/admin/playback/integrity-check",
            post(streaming_routes::run_integrity_check),
        )
        // Debrid streaming
        .route(
            "/api/v1/admin/stream/search",
            post(debrid_stream_routes::stream_search),
        )
        .route(
            "/api/v1/admin/stream/resolve",
            post(debrid_stream_routes::stream_resolve),
        )
        .route(
            "/api/v1/admin/stream/resolve/status",
            get(debrid_stream_routes::stream_resolve_status),
        )
        .route(
            "/api/v1/admin/stream/prewarm",
            post(routes::admin::stream_prewarm::prewarm),
        )
        .route(
            "/api/v1/admin/stream/prewarm/{id}",
            delete(routes::admin::stream_prewarm::release),
        )
        .route(
            "/api/v1/admin/stream/prewarm/{id}/claim",
            post(routes::admin::stream_prewarm::claim),
        )
        .route(
            "/api/v1/admin/stream/prewarm/{id}/touch",
            post(routes::admin::stream_prewarm::touch),
        )
        .route(
            "/api/v1/admin/stream/{id}/keep",
            post(routes::admin::stream_keep::keep).get(routes::admin::stream_keep::keep_status),
        )
        .route(
            "/api/v1/admin/stream/{id}/play",
            get(routes::admin::debrid_play::stream_play),
        )
        .route(
            "/api/v1/admin/stream/{id}/status",
            get(debrid_stream_routes::stream_status),
        )
        .route(
            "/api/v1/admin/stream/{id}/progress",
            post(debrid_stream_routes::stream_progress),
        )
        .route(
            "/api/v1/admin/stream/{id}/outcome",
            post(routes::admin::debrid_outcomes::stream_outcome),
        )
        .route(
            "/api/v1/admin/stream/{id}",
            delete(debrid_stream_routes::stream_delete),
        )
        .route(
            "/api/v1/admin/stream/{id}/pin",
            post(debrid_stream_routes::stream_pin).delete(debrid_stream_routes::stream_unpin),
        )
        .route(
            "/api/v1/admin/stream/recent",
            get(debrid_stream_routes::stream_recent),
        )
        // Stremio API key management
        .route(
            "/api/v1/admin/settings/stremio-keys",
            get(debrid_stream_routes::list_stremio_keys)
                .post(debrid_stream_routes::create_stremio_key),
        )
        .route(
            "/api/v1/admin/settings/stremio-keys/{key_id}",
            delete(debrid_stream_routes::delete_stremio_key),
        )
        .route(
            "/api/v1/admin/stremio/account",
            get(routes::admin::stremio_account::get_account)
                .put(routes::admin::stremio_account::link_account)
                .delete(routes::admin::stremio_account::unlink_account),
        )
        .route(
            "/api/v1/admin/stremio/account/sync",
            post(routes::admin::stremio_account::sync_account),
        )
        .route(
            "/api/v1/admin/stremio/history",
            get(routes::admin::stremio_history::list_history),
        )
        .route(
            "/api/v1/admin/stremio/history/imports",
            put(routes::admin::stremio_history::select_imports),
        )
        .route(
            "/api/v1/admin/stremio/history/imports/{stremio_id}",
            delete(routes::admin::stremio_history::deselect_import),
        )
        .route(
            "/api/v1/library/books",
            get(library::list_books).post(library::create_book),
        )
        .route(
            "/api/v1/library/books/first-by-letter",
            get(library::first_by_letter),
        )
        .route(
            "/api/v1/library/books/available-letters",
            get(library::available_letters),
        )
        .route("/api/v1/library/books/facets", get(library::facets))
        .route(
            "/api/v1/library/books/unavailable",
            get(library::list_unavailable_books).delete(library::delete_unavailable_books),
        )
        .route(
            "/api/v1/library/books/{book_id}",
            get(library::get_book)
                .patch(library::update_book)
                .delete(library::delete_book),
        )
        .route(
            "/api/v1/library/books/{book_id}/series-primary",
            put(library::set_book_primary_series),
        )
        .route(
            "/api/v1/library/books/{book_id}/check-availability",
            post(library::check_book_availability),
        )
        .route(
            "/api/v1/library/books/{book_id}/recommendations",
            get(library::get_book_recommendations),
        )
        .route("/api/v1/library/books/touch", post(library::touch_books))
        .route(
            "/api/v1/library/books/{book_id}/cover",
            get(library::cover).post(library::upload_cover),
        )
        .route(
            "/api/v1/library/books/{book_id}/cover-from-url",
            post(library::cover_from_url),
        )
        .route(
            "/api/v1/library/books/{book_id}/extracted-cover",
            get(library::extracted_cover),
        )
        .route(
            "/api/v1/library/books/{book_id}/cover/use-extracted",
            post(library::use_extracted_cover),
        )
        .route(
            "/api/v1/library/series",
            get(library::list_series).post(library::create_series),
        )
        .route(
            "/api/v1/library/series/{series_id}",
            get(library::get_series)
                .put(library::update_series)
                .patch(library::rename_series)
                .delete(library::delete_series),
        )
        .route("/api/v1/library/series/merge", post(library::merge_series))
        .route(
            "/api/v1/library/provider-link-issues",
            get(library::list_provider_link_issues),
        )
        .route(
            "/api/v1/library/series/{series_id}/reorder",
            post(library::reorder_series),
        )
        .route(
            "/api/v1/library/series/{series_id}/books",
            get(library::list_series_books),
        )
        .route(
            "/api/v1/library/series/{series_id}/monitoring",
            put(library::update_series_monitoring),
        )
        .route(
            "/api/v1/library/series/{series_id}/refresh",
            post(library::refresh_series_metadata),
        )
        .route("/api/v1/library/shelves", get(library::list_shelves))
        .route("/api/v1/library/shelves", post(library::create_shelf))
        .route(
            "/api/v1/library/shelves/{shelf_id}",
            patch(library::rename_shelf),
        )
        .route(
            "/api/v1/library/shelves/{shelf_id}",
            delete(library::delete_shelf),
        )
        .route(
            "/api/v1/library/shelves/{shelf_id}/items",
            post(library::add_shelf_items),
        )
        .route(
            "/api/v1/library/shelves/{shelf_id}/items",
            delete(library::remove_shelf_items),
        )
        .route(
            "/api/v1/library/shelves/{shelf_id}/items",
            get(library::list_shelf_items),
        )
        .route(
            "/api/v1/library/authors",
            get(library::list_authors).post(library::create_author),
        )
        .route(
            "/api/v1/library/authors/{author_id}",
            get(library::get_author)
                .put(library::update_author)
                .patch(library::rename_author)
                .delete(library::delete_author),
        )
        .route(
            "/api/v1/library/authors/{author_id}/books",
            get(library::list_author_books),
        )
        .route(
            "/api/v1/library/authors/merge",
            post(library::merge_authors),
        )
        .route(
            "/api/v1/library/authors/{author_id}/monitoring",
            put(library::update_author_monitoring),
        )
        .route(
            "/api/v1/library/authors/{author_id}/refresh",
            post(library::refresh_author_metadata),
        )
        .route(
            "/api/v1/library/authors/{author_id}/apply-metadata",
            post(library::apply_author_metadata),
        )
        .route(
            "/api/v1/library/authors/{author_id}/image",
            get(library::get_author_image),
        )
        .route(
            "/api/v1/library/series/{series_id}/apply-metadata",
            post(library::apply_series_metadata),
        )
        .route(
            "/api/v1/library/series/{series_id}/image",
            get(library::get_series_image),
        )
        // Genres
        .route("/api/v1/library/genres", get(library::list_genres))
        .route(
            "/api/v1/library/genres/{genre_id}/books",
            get(library::list_genre_books),
        )
        // Library search and download (independent of wanted system)
        .route(
            "/api/v1/library/search/releases",
            get(library::search_library_releases),
        )
        .route(
            "/api/v1/library/search/download",
            post(library::download_to_library),
        )
        // Book files with provenance
        .route(
            "/api/v1/library/books/{book_id}/files",
            get(library::get_book_files),
        )
        // Audiobooks (koblem LibraryProvider)
        .route(
            "/api/v1/library/audiobooks",
            get(library::search_audiobooks),
        )
        .route(
            "/api/v1/library/audiobooks/{book_id}",
            get(library::get_audiobook),
        )
        .route(
            "/api/v1/library/audiobooks/{book_id}/clip",
            get(library::get_audiobook_clip),
        )
        .route(
            "/api/v1/library/books/{book_id}/files/{file_id}",
            delete(library::delete_book_file),
        )
        .route(
            "/api/v1/library/books/{book_id}/files/{file_id}/move",
            post(library::move_book_file),
        )
        // Book want/skip for non-library books
        .route(
            "/api/v1/library/books/{book_id}/want",
            post(library::want_book),
        )
        .route(
            "/api/v1/library/books/{book_id}/skip",
            post(library::skip_book),
        )
        // Bulk want operations for authors and series
        .route(
            "/api/v1/library/authors/{author_id}/want-pending",
            post(library::want_all_pending_for_author).delete(library::unwant_all_for_author),
        )
        .route(
            "/api/v1/library/series/{series_id}/want-pending",
            post(library::want_all_pending_for_series).delete(library::unwant_all_for_series),
        )
        .route("/api/v1/library/activity", get(library::list_activity))
        .route("/api/v1/library/devices", get(library::list_devices))
        .route("/api/v1/library/sync", post(library::sync_books))
        .route("/api/v1/library/stats", get(library::get_stats))
        .route("/api/v1/books/{book_id}", put(ingest::upsert_book))
        .route("/api/v1/books/{book_id}/cover", put(ingest::upload_cover))
        .route(
            "/api/v1/books/{book_id}/files/{format}",
            put(ingest::upload_file),
        )
        .route("/api/v1/books/{book_id}/ingest", put(ingest::atomic_ingest))
        .route("/api/v1/entitlements", post(ingest::create_entitlement))
        // Stremio addon (public-facing, auth via path key)
        .route(
            "/stremio/{api_key}/manifest.json",
            get(stremio_routes::manifest),
        )
        .route(
            "/stremio/{api_key}/stream/{type}/{id}",
            get(stremio_routes::stream),
        )
        .route(
            "/stremio/{api_key}/play/{option_id}",
            get(stremio_routes::play),
        )
        .route(
            "/stremio/{api_key}/file/{session_id}",
            get(stremio_routes::stream_file),
        )
        .layer(axum::middleware::from_fn_with_state(
            state.clone(),
            kids::deny_kids_surfaces,
        ))
        .layer(axum::middleware::from_fn_with_state(
            state.clone(),
            library_edits::require_editor,
        ))
        .layer(DefaultBodyLimit::max(state.config.http_max_body_bytes))
        .layer(cors)
        .with_state(state)
}

/// Routes addressed to one playback session, rebuilt from its row when a restart lost it.
fn playback_session_routes(state: &AppState) -> Router<AppState> {
    Router::new()
        .route(
            "/api/v1/admin/playback/sessions/{id}/heartbeat",
            get(streaming_routes::heartbeat),
        )
        .route(
            "/api/v1/admin/playback/sessions/{id}/status",
            get(streaming_routes::status),
        )
        .route(
            "/api/v1/admin/playback/sessions/{id}/master.m3u8",
            get(streaming_routes::master_playlist),
        )
        .route(
            "/api/v1/admin/playback/sessions/{id}/stream.m3u8",
            get(streaming_routes::stream_playlist),
        )
        .route(
            "/api/v1/admin/playback/sessions/{id}/segments/{*path}",
            get(streaming_routes::serve_segment),
        )
        .route(
            "/api/v1/admin/playback/sessions/{id}/seek",
            post(streaming_routes::seek),
        )
        .route(
            "/api/v1/admin/playback/sessions/{id}/decode-error",
            post(streaming_routes::decode_error),
        )
        .route(
            "/api/v1/admin/playback/sessions/{id}/switch-audio",
            post(streaming_routes::switch_audio),
        )
        .route(
            "/api/v1/admin/playback/sessions/{id}/direct",
            get(streaming_routes::direct_play),
        )
        .route(
            "/api/v1/admin/playback/sessions/{id}/subtitles",
            post(streaming_routes::download_subtitle_to_session),
        )
        .route(
            "/api/v1/admin/playback/sessions/{id}/subtitles/{track_filename}",
            get(streaming_routes::serve_subtitle),
        )
        .route(
            "/api/v1/admin/playback/sessions/{id}/thumbnails.vtt",
            get(streaming_routes::serve_thumbnails_vtt),
        )
        .route(
            "/api/v1/admin/playback/sessions/{id}/thumbnails/{filename}",
            get(streaming_routes::serve_thumbnail_sprite),
        )
        .route_layer(axum::middleware::from_fn_with_state(
            state.clone(),
            routes::admin::session_restore::restore_on_miss,
        ))
}

/// Explicit override for the standalone GPU-scheduler fallback.
///
/// `KOBBLER_GPU_STANDALONE` forces the local gate's seed mode regardless of the
/// koblem-routing config:
/// - `1` / `true` / `yes` / `on`  → force STANDALONE (seed the static defaults).
/// - `0` / `false` / `no` / `off` → force DELEGATE (never seed; stay a no-op).
/// - unset / unrecognised         → auto (derive from `use_local_gpu_lock()`).
fn gpu_standalone_override() -> Option<bool> {
    let raw = std::env::var("KOBBLER_GPU_STANDALONE").ok()?;
    match raw.trim().to_ascii_lowercase().as_str() {
        "1" | "true" | "yes" | "on" => Some(true),
        "0" | "false" | "no" | "off" => Some(false),
        other => {
            tracing::warn!("KOBBLER_GPU_STANDALONE={other:?} not understood; using auto mode");
            None
        }
    }
}

/// Opt-in VRAM reservation for a co-located LLM server (`KOBBLER_GPU_RESERVE_LLM`).
///
/// kobbler never acquires `Llm` itself; the reservation only exists so a
/// long-lived LLM sharing the card is not admitted on top of. Off by default:
/// reserving a peak nothing holds would starve STT/TTS/vision on a single card.
fn gpu_reserve_llm() -> bool {
    std::env::var("KOBBLER_GPU_RESERVE_LLM").is_ok_and(|raw| {
        matches!(
            raw.trim().to_ascii_lowercase().as_str(),
            "1" | "true" | "yes" | "on"
        )
    })
}

/// Create an AppState with the given pool, config, and runtime config manager.
#[allow(clippy::too_many_arguments)]
pub fn create_app_state(
    pool: DbPool,
    config: Config,
    runtime_config: RuntimeConfigManager,
    nats_state: nats::NatsState,
    search_backend: Arc<dyn search::SearchBackend>,
    hw_accel: ffmpeg::detect::HwAccel,
    encoder_caps: ffmpeg::detect::EncoderCaps,
    decoder_caps: ffmpeg::detect::DecoderCaps,
    cancel_registry: nats_jobs::CancelRegistry,
) -> AppState {
    // Lazily-built GPU device context for hardware-accelerated decoding. The
    // context is created on first transcode and released when idle, so VRAM is
    // only held while transcoding is actually happening.
    let hw_device_spec = match &hw_accel {
        ffmpeg::detect::HwAccel::Nvenc => Some(kob_ffmpeg::hw::device::HwDeviceSpec::cuda()),
        ffmpeg::detect::HwAccel::Vaapi { device } => Some(
            kob_ffmpeg::hw::device::HwDeviceSpec::vaapi(Some(device.clone())),
        ),
        ffmpeg::detect::HwAccel::None => None,
    };
    let hw_device = Arc::new(kob_ffmpeg::hw::device::LazyHwDevice::new(hw_device_spec));

    // When primary accel is NVENC, pre-create a VAAPI fallback device for encoder
    // fallback chains. Creating VAAPI contexts per-request races with concurrent
    // CUDA operations and can segfault.
    let vaapi_fallback_device: Option<kob_ffmpeg::hw::device::SharedHwDevice> =
        if matches!(&hw_accel, ffmpeg::detect::HwAccel::Nvenc) {
            match kob_ffmpeg::hw::device::HwDeviceContext::vaapi() {
                Ok(ctx) => {
                    tracing::info!("VAAPI fallback device created for NVENC encoder fallback");
                    Some(std::sync::Arc::new(ctx))
                }
                Err(e) => {
                    tracing::debug!("No VAAPI fallback available: {e}");
                    None
                }
            }
        } else {
            None
        };

    let (library_events, _) = broadcast::channel(config.library_events_buffer);
    let (job_events, _) = broadcast::channel(2048);
    let (ingest_queue_events, _) = broadcast::channel(64);
    let gpu_lock = {
        let lock = gpu::GpuLock::new();
        if let Some(rt) = runtime_config.try_get() {
            // Local lock disables itself when koblem owns the authoritative
            // lock (avoids double-gating when routing via koblem). Still
            // respects gpu_preempt_tts as the master switch.
            let enabled = rt.gpu_preempt_tts && rt.use_local_gpu_lock();
            lock.set_enabled(enabled);
            lock.set_service_urls(
                rt.stt_service_url.clone(),
                rt.tts_service_url.clone(),
                rt.vision_service_url.clone(),
            );

            // Standalone: koblem is not the GPU authority, so kobbler talks to
            // the GPU services directly and there is nothing to delegate
            // coordination to. Seed the local gate from the detected cards so
            // kobbler's own GPU jobs go through VRAM-budget admission instead of
            // racing. When koblem IS the authority the gate seeds nothing and
            // stays a no-op: every acquire_for_* returns a no-op guard.
            //
            // `KOBBLER_GPU_STANDALONE` forces either mode.
            let standalone = match gpu_standalone_override() {
                Some(forced) => forced,
                None => rt.use_local_gpu_lock(),
            };
            if standalone {
                let cards = gpu_defaults::detect_cards();
                let engines = gpu_defaults::default_engines(&cards);
                let card_count = cards.len();
                lock.set_cards(cards);
                for (kind, eng) in engines {
                    lock.set_engine_config(kind, eng);
                }
                if gpu_reserve_llm() {
                    lock.seed_resident(gpu::ServiceKind::Llm);
                }
                tracing::info!(
                    "GPU lock: STANDALONE mode — seeded local scheduler from {card_count} \
                     detected GPU card(s); gate enabled={enabled}"
                );
            } else {
                tracing::info!(
                    "GPU lock: DELEGATE mode — koblem owns the authoritative GPU \
                     scheduler; local gate seeds no cards (no-op)"
                );
            }
        }
        lock
    };
    let livetv_pool = pool.clone();
    let media_root_io_gate = jobs::MediaRootIoGate::new(pool.clone());
    let nas_health_monitor = nas_health::NasHealthMonitor::new();
    nas_health_monitor.clone().spawn(
        config.nas_health_monitoring,
        config.nas_health_stats_path.clone(),
    );
    AppState {
        pool,
        config: Arc::new(config),
        runtime_config,
        library_events,
        job_events,
        ingest_queue_events,
        nats: nats_state,
        search: search_backend,
        playback_sessions: Arc::new(dashmap::DashMap::new()),
        cue_trim_cache: Arc::new(ffmpeg::mkv_cue_cache::CueTrimCache::new()),
        busy_sessions: Arc::new(dashmap::DashSet::new()),
        watch_handoffs: Arc::new(dashmap::DashMap::new()),
        hw_accel: Arc::new(hw_accel),
        encoder_caps: Arc::new(encoder_caps),
        decoder_caps,
        hw_device,
        vaapi_fallback_device,
        gpu_lock,
        login_limiter: Arc::new(rate_limit::RateLimiter::new(
            10,
            std::time::Duration::from_secs(60),
        )),
        cancel_registry,
        live_streams: Arc::new(dashmap::DashMap::new()),
        livetv: livetv::live_session::LivetvState::new(livetv::tuner_lease::TunerLeases::new(
            livetv_pool,
        )),
        stremio_play_cache: Arc::new(dashmap::DashMap::new()),
        stremio_resolve_locks: Arc::new(dashmap::DashMap::new()),
        debrid_session_index: Arc::new(dashmap::DashMap::new()),
        debrid_resolve_locks: Arc::new(dashmap::DashMap::new()),
        stream_prewarms: Arc::default(),
        initial_scan_complete: Arc::new(AtomicBool::new(false)),
        media_root_io_gate,
        nas_health: nas_health_monitor,
    }
}
