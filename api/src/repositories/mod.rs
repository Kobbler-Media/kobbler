//! Repository layer for database operations.
//!
//! This module provides typed inputs and centralized functions for database operations.
//! All database queries MUST go through this layer - no sqlx calls outside of repositories.
//!
//! All functions accept `impl Executor<'_, Database = Postgres>` to support both
//! pool and transaction usage.

pub mod activity_log;
pub mod audio_tracks;
pub mod audiobook_bookmarks;
pub mod audiobook_chapters;
pub mod audiobook_editions;
pub mod audiobook_files;
pub mod audiobook_transcripts;
pub mod auth_tokens;
pub mod authors;
pub mod book_files;
pub mod book_ingest_source;
pub mod book_series;
pub mod books;
pub mod cached_segments;
pub mod chat_credits;
pub mod chat_identities;
pub mod chat_memory;
pub mod chat_notification_prefs;
pub mod chat_pending;
pub mod chat_profile_facts;
pub mod chat_seasons;
pub mod chat_sessions;
pub mod chat_tools;
pub mod chat_turn_logs;
pub mod chat_watches;
pub mod client_playback_reports;
pub mod cross_format_sync;
pub mod dashboard;
pub mod debrid_cache_files;
pub mod debrid_keeps;
pub mod debrid_stream_sessions;
pub mod deferred_ingests;
pub mod device_sync_state;
pub mod devices;
pub mod discord;
pub mod ebook_segments;
pub mod enrichment_attempts;
pub mod entitlements;
pub mod episode_skip_segments;
pub mod external_subtitles;
pub mod for_you_dismissals;
pub mod genres;
pub mod hardcover_titles;
pub mod ingest_events;
pub mod ingest_queue;
pub mod ingest_tokens;
pub mod iptv_channel_aliases;
pub mod iptv_channels;
pub mod iptv_epg;
pub mod iptv_favourites;
pub mod iptv_groups;
pub mod iptv_programme_watches;
pub mod iptv_providers;
pub mod iptv_recordings;
pub mod iptv_series_rules;
pub mod iptv_sources;
pub mod iptv_sync_log;
pub mod iptv_unmatched;
pub mod iptv_watch_history;
pub mod job_status;
pub mod kindle;
pub mod letters;
pub mod livetv_ad_breaks;
pub mod livetv_ad_fingerprints;
pub mod livetv_affinity;
pub mod livetv_channels;
pub mod livetv_guide;
pub mod livetv_leases;
pub mod livetv_recordings;
pub mod livetv_series_rules;
pub mod livetv_sessions;
pub mod livetv_tuners;
pub mod livetv_watch_events;
pub mod media_cache_runs;
pub mod media_fingerprints;
pub mod media_root_dir_mtimes;
pub mod media_root_files;
pub mod media_roots;
pub mod metadata_cache;
pub mod metadata_providers;
pub mod movie_files;
pub mod movie_skip_segments;
pub mod movie_watch_states;
pub mod movies;
pub mod notification_agents;
pub mod offline_exports;
pub mod people;
pub mod playback_sessions;
pub mod playback_tokens;
pub mod provider_reconcile;
pub mod proxy_logs;
pub mod reading_states;
pub mod search_meta;
pub mod search_recents;
pub mod search_suggestions;
pub mod series;
pub mod shelf_items;
pub mod shelves;
pub mod skip_detect_candidates;
pub mod stremio_accounts;
pub mod stremio_continue;
pub mod stremio_history;
pub mod subtitle_providers;
pub mod subtitle_tracks;
pub mod sync_log;
pub mod tmdb_titles;
pub mod tv_episode_cast;
pub mod tv_episode_files;
pub mod tv_episodes;
pub mod tv_seasons;
pub mod tv_show_ratings;
pub mod tv_shows;
pub mod tv_watch_states;
pub mod user_book_data;
pub mod user_notifications;
pub mod user_sync_providers;
pub mod user_voice_presets;
pub mod users;
pub mod video_chapters;
pub mod video_metadata_providers;
pub mod wanted_attempts;
pub mod watch_history;
pub mod watchlist;
pub mod webpush_subscriptions;
use thiserror::Error;

/// Repository errors.
#[derive(Error, Debug)]
pub enum RepositoryError {
    #[error("database error: {0}")]
    Database(#[from] sqlx::Error),

    #[error("validation error: {0}")]
    Validation(String),
}

/// Result type for repository operations.
pub type Result<T> = std::result::Result<T, RepositoryError>;

/// Best-effort deletion of physical files from disk.
/// Logs warnings on failure but never propagates errors.
pub async fn cleanup_replaced_files(paths: &[String]) {
    for file_path in paths {
        let path = std::path::Path::new(file_path);
        if path.exists()
            && let Err(e) = tokio::fs::remove_file(path).await
        {
            tracing::warn!(error = %e, path = %file_path, "Failed to delete replaced file from disk");
        }
    }
}

/// Best-effort deletion of a thumbnail directory for a media file.
pub async fn cleanup_thumbnail_dir(storage_root: &str, file_id: uuid::Uuid) {
    let dir = std::path::PathBuf::from(storage_root)
        .join("thumbnails")
        .join(file_id.to_string());
    if dir.exists()
        && let Err(e) = tokio::fs::remove_dir_all(&dir).await
    {
        tracing::warn!(error = %e, dir = %dir.display(), "Failed to delete thumbnail directory");
    }
}

/// Video ratings keep their providers' 0–10 scale. Anything above 10 can only be a
/// 0–100 percentage (Rotten Tomatoes, Metacritic) and is brought onto that scale.
pub fn normalize_video_rating(r: f64) -> f64 {
    if r > 10.0 { r / 10.0 } else { r }.clamp(0.0, 10.0)
}

/// Normalize a provider-supplied TV show status to the canonical vocabulary
/// stored in `tv_shows.status`: "continuing", "ended", "canceled", "upcoming".
///
/// TMDB sends "Returning Series", "Ended", "Canceled", "In Production", "Planned",
/// "Pilot". TVDB sends "Continuing", "Ended", "Upcoming". TVmaze sends "Running",
/// "Ended", "To Be Determined", "In Development". Unknown values are lowercased and
/// passed through so a new provider vocabulary degrades instead of failing the write.
pub fn normalize_show_status(raw: Option<String>) -> Option<String> {
    raw.map(|s| match s.to_lowercase().as_str() {
        "returning series" | "continuing" | "running" => "continuing".to_string(),
        "ended" => "ended".to_string(),
        "canceled" | "cancelled" => "canceled".to_string(),
        "upcoming" | "in production" | "planned" | "pilot" | "in development" => {
            "upcoming".to_string()
        }
        other => other.to_string(),
    })
}

/// Pagination direction for cursor-based queries.
/// Used across authors, series, and books for bidirectional pagination.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum PaginationDirection {
    /// Forward pagination: items AFTER cursor, sorted ASC
    #[default]
    Forward,
    /// Backward pagination: items BEFORE cursor, sorted DESC (results need reversing by caller)
    Backward,
}

/// Check if a sqlx error is the Postgres "no unique or exclusion constraint
/// matching the ON CONFLICT specification" error (42P10).
pub fn is_no_unique_constraint_error(e: &sqlx::Error) -> bool {
    if let sqlx::Error::Database(db_err) = e {
        db_err.code().as_deref() == Some("42601")
            || db_err
                .message()
                .contains("there is no unique or exclusion constraint")
    } else {
        false
    }
}

/// Deterministic advisory lock key from table + provider key + provider id.
/// Uses FNV-like mixing to get an i64 for `pg_advisory_xact_lock`.
pub fn advisory_lock_key(table: &str, id_key: &str, provider_id: &str) -> i64 {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    table.hash(&mut hasher);
    id_key.hash(&mut hasher);
    provider_id.hash(&mut hasher);
    hasher.finish() as i64
}

/// Total order for rows that share a non-unique provider id.
///
/// Migration 0093 dropped the unique `identifiers->>'tvdb_id'` indexes on
/// purpose: TVDB models a show reboot as one entity where TMDB has one entity
/// per reboot, so a tvdb_id matches several rows by design. `created_at` alone
/// is not a total order (a bulk insert shares one `NOW()`), so `id` breaks the
/// remaining ties — without it the winner can change after a VACUUM. Every site
/// that resolves an entity from an ambiguous provider id uses this same order.
pub const AMBIGUOUS_ROW_ORDER: &str = "ORDER BY created_at ASC, id ASC";

/// Provider keys backed by a UNIQUE index, per table: a hit on one of these
/// identifies exactly one row, so it beats an ambiguous key. `tvdb_id` is
/// deliberately absent — see `AMBIGUOUS_ROW_ORDER`.
fn unique_identifier_keys(table: &str) -> &'static [&'static str] {
    match table {
        "tv_shows" => &["tmdb_id", "thesportsdb_id"],
        "movies" | "people" => &["tmdb_id"],
        _ => &[],
    }
}

fn identifier_value(identifiers: &serde_json::Value, key: &str) -> Option<String> {
    match identifiers.get(key)? {
        serde_json::Value::String(s) => Some(s.clone()),
        serde_json::Value::Number(n) => Some(n.to_string()),
        _ => None,
    }
}

/// Resolve the row a provider upsert should target when `id_key` has no unique
/// index.
///
/// Prefers a co-identifier that *is* unique — a reboot is a distinct TMDB entity
/// even though it shares the original's TVDB id — and only falls back to the
/// ambiguous key, under `AMBIGUOUS_ROW_ORDER`, when there is nothing narrower to
/// go on. Guessing here is what let two reboots overwrite each other's title,
/// year, status and images on every sync.
pub async fn resolve_provider_row(
    conn: &mut sqlx::PgConnection,
    table: &str,
    id_key: &str,
    provider_id: &str,
    identifiers: &serde_json::Value,
) -> std::result::Result<Option<uuid::Uuid>, sqlx::Error> {
    let sql =
        format!("SELECT id FROM {table} WHERE identifiers->>$1 = $2 {AMBIGUOUS_ROW_ORDER} LIMIT 1");

    for key in unique_identifier_keys(table) {
        if *key == id_key {
            continue;
        }
        let Some(value) = identifier_value(identifiers, key) else {
            continue;
        };
        let found: Option<(uuid::Uuid,)> = sqlx::query_as(sqlx::AssertSqlSafe(sql.as_str()))
            .bind(key)
            .bind(&value)
            .fetch_optional(&mut *conn)
            .await?;
        if let Some((id,)) = found {
            return Ok(Some(id));
        }
    }

    let found: Option<(uuid::Uuid,)> = sqlx::query_as(sqlx::AssertSqlSafe(sql.as_str()))
        .bind(id_key)
        .bind(provider_id)
        .fetch_optional(&mut *conn)
        .await?;
    Ok(found.map(|(id,)| id))
}

#[cfg(test)]
mod normalize_show_status_tests {
    use super::normalize_show_status;

    #[test]
    fn maps_provider_vocabularies_to_canonical_values() {
        let cases = [
            ("Returning Series", "continuing"),
            ("returning series", "continuing"),
            ("Continuing", "continuing"),
            ("Running", "continuing"),
            ("Ended", "ended"),
            ("ENDED", "ended"),
            ("Canceled", "canceled"),
            ("Cancelled", "canceled"),
            ("Upcoming", "upcoming"),
            ("In Production", "upcoming"),
            ("Planned", "upcoming"),
            ("Pilot", "upcoming"),
            ("In Development", "upcoming"),
        ];
        for (raw, expected) in cases {
            assert_eq!(
                normalize_show_status(Some(raw.to_string())).as_deref(),
                Some(expected),
                "{raw:?} must normalize to {expected:?}"
            );
        }
    }

    #[test]
    fn passes_unknown_values_through_lowercased() {
        assert_eq!(
            normalize_show_status(Some("To Be Determined".to_string())).as_deref(),
            Some("to be determined")
        );
    }

    #[test]
    fn preserves_none() {
        assert_eq!(normalize_show_status(None), None);
    }

    #[test]
    fn is_idempotent_over_canonical_values() {
        for canonical in ["continuing", "ended", "canceled", "upcoming"] {
            assert_eq!(
                normalize_show_status(Some(canonical.to_string())).as_deref(),
                Some(canonical)
            );
        }
    }
}
