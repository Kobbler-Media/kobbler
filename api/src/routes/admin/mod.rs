//! Admin routes module.
//!
//! This module contains all route handlers for the admin API endpoints.

pub mod audiobook;
mod auth;
mod backup;
pub mod book_moves;
pub mod calendar;
mod cast;
pub mod chat;
pub mod chat_identities;
pub mod chat_logs;
pub mod chat_memory;
mod config;
pub mod dashboard;
pub mod debrid_errors;
pub mod debrid_open;
pub mod debrid_outcomes;
pub mod debrid_play;
pub mod debrid_stream;
mod devices;
pub mod diagnostics;
pub mod discover;
pub(crate) mod downloads;
pub mod episode_cast;
mod filesystem;
pub mod for_you;
pub mod inbox;
pub mod iptv;
mod kindle;
pub mod library_status;
pub mod livetv;
pub mod livetv_play;
pub mod livetv_recordings;
pub mod livetv_series_rules;
pub mod livetv_watch;
pub mod media_roots;
pub mod migration;
pub mod movies;
pub mod notification_prefs;
pub mod notifications;
pub mod people;
mod pin_login;
mod proxy_logs;
mod reader;
mod reading;
mod recents;
mod search;
pub mod session_restore;
pub mod sessions;
pub mod sports;
pub mod stream_keep;
pub mod stream_prewarm;
pub mod stream_search_request;
pub mod streaming;
pub mod stremio_account;
pub mod stremio_history;
pub mod subtitle_providers;
pub mod subtitles;
mod tokens;
pub mod tts;
pub mod tv;
mod users;
pub mod video_metadata;
pub mod video_sync;
pub mod watch_history;
pub mod watchlist;

// Re-export all handlers used in the router

// Auth
pub use auth::{
    change_password, clear_profile_pin, delete_avatar, get_avatar, login, logout, me,
    set_profile_pin, update_display_name, update_timezone, upload_avatar, verify_profile_pin,
};

// Backup
pub use backup::{
    clear_metadata_cache, delete_dev_snapshot, download_backup, get_dev_snapshot_status,
    metadata_write_all, restore_backup, save_dev_snapshot, trigger_library_cleanup,
    trigger_orphan_cleanup,
};

// Cast (playback tokens)
pub use cast::{create_playback_token, revoke_playback_token};

// Config
pub use config::{get_config, update_config};

// Devices
pub use devices::{
    add_book_all_devices, add_book_device, add_device_books, create_device, delete_device,
    get_active_device_types, get_device_activity, list_book_devices, list_device_books,
    list_devices, remove_book_device, remove_book_entitlements, remove_device_book,
    restore_book_entitlements, resync_device, rotate_device_token, update_device,
    update_device_sync_provider,
};

// Chat turn logs
pub use chat_logs::{get_chat_log, list_chat_logs};
pub use chat_memory::{
    create_chat_fact, delete_chat_fact, forget_chat_memory, get_chat_memory,
    list_chat_memory_users, update_chat_fact,
};

// Proxy Logs
pub use proxy_logs::{
    clear_proxy_logs, download_proxy_log_blob, export_proxy_logs, list_proxy_logs, resolve_uuids,
};

// Recents (command palette frecency)
pub use recents::{clear_recents, list_recents, record_recent};

// Search
pub use search::{search, search_status, sync_static, test_connection, trigger_reindex};

// Tokens
pub use tokens::{
    create_ingest_token, list_ingest_tokens, revoke_ingest_token, update_ingest_token,
};

// Users
pub use users::{
    create_user, delete_user, get_sync_providers, list_users, mint_device_token,
    update_sync_provider, update_user,
};

// Linked sessions (auth tokens)
pub use sessions::{list_sessions, refresh_session, revoke_session};

// Downloads (Kobload proxy)
pub use downloads::{
    cancel_download, list_downloads, reject_download, replace_file, retry_callback, retry_download,
};

// Filesystem browsing
pub use filesystem::{browse_directories, detect_media_type};

// Reader (web EPUB reader)
pub use reader::{download_book_file, get_reader_progress, save_reader_progress};

// Reading (reading status, progress, rating)
pub use reading::{get_reading_data, log_reading_time, update_reading_data};

// People
pub use people::get_person;

// Kindle
pub use kindle::{
    complete_oauth, disconnect_account, get_account, get_book_deliveries, get_oauth_url,
    refresh_devices, send_to_kindle, trigger_kindle_delivery_all_devices,
    trigger_kindle_delivery_if_needed, update_target,
};

// PIN login
pub use pin_login::pin_login;

// Notification preferences (the caller's own)
pub use notification_prefs::{get_notification_prefs, update_notification_prefs};

// Notifications
pub use notifications::{
    create_agent, delete_agent, get_vapid_key, list_agents, list_types, test_agent, update_agent,
    webpush_subscribe, webpush_unsubscribe,
};

// Inbox (in-app notifications)
pub use inbox::{list_inbox, mark_all_read, mark_read, unread_count};

// Chat
pub use chat::{chat_completions, chat_status};

// Chat identities
pub use chat_identities::{
    approve_chat_pairing, create_chat_identity, delete_chat_identity, list_chat_identities,
    reject_chat_pairing, update_chat_identity,
};
