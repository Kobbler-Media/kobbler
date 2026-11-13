//! Backup and restore functionality for Kobbler.
//!
//! Creates tar.gz archives containing:
//! - manifest.json with metadata
//! - database/ directory with CSV exports of all tables (using PostgreSQL COPY)
//! - storage/ directory with book files and covers

use crate::DbPool;
use chrono::{DateTime, Utc};
use flate2::Compression;
use flate2::read::GzDecoder;
use flate2::write::GzEncoder;
use futures::TryStreamExt;
use serde::{Deserialize, Serialize};
use sqlx::postgres::PgPoolCopyExt;
use std::collections::HashSet;
use std::fs::{self, File};
use std::io::{self, BufRead, BufReader, Write};
use std::path::{Component, Path};
use tar::{Archive, Builder};
use thiserror::Error;
use tracing::{debug, info, warn};

/// Manifest file included in every backup archive.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BackupManifest {
    /// Manifest format version
    pub version: String,
    /// Kobbler version that created this backup
    pub kobbler_version: String,
    /// When the backup was created
    pub created_at: DateTime<Utc>,
    /// Number of database tables exported
    pub table_count: usize,
    /// Number of books in the library
    pub books_count: i64,
    /// Number of files (books + covers) included
    pub files_count: usize,
    /// Whether proxy blobs are included
    pub includes_proxy_blobs: bool,
}

impl BackupManifest {
    pub fn new(books_count: i64, files_count: usize, includes_proxy_blobs: bool) -> Self {
        Self {
            version: "1.0".to_string(),
            kobbler_version: env!("CARGO_PKG_VERSION").to_string(),
            created_at: Utc::now(),
            table_count: TABLES_IN_ORDER.len(),
            books_count,
            files_count,
            includes_proxy_blobs,
        }
    }
}

/// Tables in dependency order for export/import.
/// Tables with no foreign keys first, then tables that depend on them.
/// Note: book_series was re-added in migration 0123 (many-to-many junction table for books↔series).
/// Note: tracked_authors, tracked_author_books, wanted_items were consolidated into
/// authors, series, books in migration 0019.
/// Note: ingest_queue was merged into books + book_ingest_source in migration 0030.
pub const TABLES_IN_ORDER: &[&str] = &[
    // No dependencies
    "users",
    "authors",
    "series",
    "metadata_providers",
    "video_metadata_providers",
    "system_config",
    "people",
    "notification_agents",
    "webpush_subscriptions",
    // Depends on notification_agents
    "discord_show_threads",
    "discord_message_map",
    // Depends on users
    "chat_sessions",
    // Depends on chat_sessions
    "chat_messages",
    // Depends on users
    "chat_notification_prefs",
    "chat_identities",
    // Depends on chat_identities
    "chat_pairing_codes",
    // Depends on users
    "chat_watches",
    // Depends on users and chat_sessions
    "chat_profile_facts",
    "user_sync_providers",
    "books",
    "movies",
    "devices",
    // Depends on devices
    "device_sync_providers",
    "auth_tokens",
    "ingest_tokens",
    "shelves",
    // Depends on books, authors
    "book_authors",
    // Depends on books, series
    "book_series",
    // Polymorphic entity_id (authors, series, books), no FK
    "provider_link_issues",
    // Depends on books
    "book_files",
    "book_ingest_source",
    // Depends on books
    "audiobook_files",
    // Depends on audiobook_files
    "audiobook_chapters",
    "audiobook_transcripts",
    // Depends on audiobook_files, users, books
    "audiobook_bookmarks",
    // Depends on book_files
    "ebook_segments",
    // Depends on books, audiobook_files, book_files
    "cross_format_sync",
    // Depends on books, book_files, users, audiobook_files
    "book_casts",
    // Depends on users, and on book_casts for a cast's own presets
    "user_voice_presets",
    // Depends on book_casts
    "book_cast_chapters",
    "book_cast_characters",
    "book_cast_renders",
    // Depends on book_cast_characters
    "book_cast_lines",
    // Depends on book_cast_characters, user_voice_presets
    "book_cast_voices",
    // Depends on movies
    "movie_files",
    // Subtitle/audio providers and track tables (depend on movie_files, tv_episode_files)
    "subtitle_providers",
    // Media roots (no FK deps beyond themselves)
    "media_roots",
    // Depends on media_roots
    "media_root_files",
    "media_root_dir_mtimes",
    // Depends on movies, people
    "movie_cast",
    "movie_crew",
    "tv_shows",
    // Depends on tv_shows
    "tv_seasons",
    // Depends on tv_seasons
    "tv_episodes",
    // Depends on tv_episodes
    "tv_episode_files",
    // Depends on tv_episode_files, tv_episodes
    "tv_episode_file_episodes",
    // Depends on tv_seasons, movie_files
    "skip_detect_candidates",
    // Depends on file_id from movie_files / tv_episode_files / audiobook_files (logical FK only)
    "media_cache_runs",
    // Depends on movie_files, tv_episode_files
    "audio_tracks",
    "subtitle_tracks",
    "external_subtitles",
    // Depends on tv_shows, people
    "tv_cast",
    "tv_crew",
    // Depends on tv_episodes, people
    "tv_episode_cast",
    // Depends on tv_episodes, users
    "tv_watch_states",
    // Depends on tv_shows, users
    "tv_show_ratings",
    // Depends on books, users, devices
    "entitlements",
    "reading_states",
    "device_sync_state",
    // Depends on movies, users
    "movie_watch_states",
    "for_you_dismissals",
    "watch_history_exclusions",
    "up_next_dismissals",
    // Depends on shelves, books
    "shelf_items",
    "shelf_archives",
    // Depends on users
    "kindle_accounts",
    // Depends on kindle_accounts
    "kindle_targets",
    // Depends on reading_states
    "reading_status",
    "reading_statistics",
    "reading_bookmark",
    // Depends on kindle_targets, books, book_files
    "kindle_deliveries",
    // Depends on books
    "ingest_events",
    // Depends on books, movies (download attempts for wanted books/movies)
    "wanted_attempts",
    // Depends on users, books
    "user_book_data",
    "search_recents",
    // Depends on users
    "user_notifications",
    // Depends on people
    "person_dismissed_credits",
    // IPTV
    "iptv_providers",
    "iptv_channels",
    "iptv_channel_groups",
    "iptv_config",
    "iptv_sources",
    "iptv_unmatched_sources",
    "iptv_channel_aliases",
    "iptv_epg",
    "iptv_recordings",
    "iptv_series_rules",
    "iptv_programme_watches",
    "iptv_favourites",
    "iptv_watch_history",
    "iptv_sync_log",
    // Live TV (HDHomeRun) — tuners, then channels, then everything keyed off them
    "livetv_tuners",
    "livetv_channels",
    "livetv_guide",
    "livetv_series_rules",
    "livetv_recordings",
    "livetv_tuner_leases",
    "livetv_sessions",
    "livetv_watch_events",
    "livetv_affinity",
    "livetv_ad_fingerprints",
    "livetv_ad_breaks",
    // Debrid streaming (depends on users)
    "stremio_api_keys",
    "stremio_accounts",
    "stremio_continue_items",
    "stremio_history_items",
    "stremio_history_imports",
    "debrid_stream_sessions",
    "debrid_cache_files",
    "debrid_keeps",
    // Logs and caches (can be large, optional for restore)
    "client_playback_reports",
    "proxy_logs",
    "metadata_cache",
    "search_embeddings",
    // Depends on users and chat_sessions
    "chat_turn_logs",
];

#[derive(Debug, Error)]
pub enum BackupError {
    #[error("IO error: {0}")]
    Io(#[from] io::Error),

    #[error("Database error: {0}")]
    Database(#[from] sqlx::Error),

    #[error("JSON error: {0}")]
    Json(#[from] serde_json::Error),

    #[error("CSV error: {0}")]
    Csv(#[from] csv::Error),

    #[error("Invalid backup: {0}")]
    Invalid(String),

    #[error("Version mismatch: backup version {backup} is not compatible with {current}")]
    VersionMismatch { backup: String, current: String },
}

/// Result type for backup operations.
pub type BackupResult<T> = Result<T, BackupError>;

/// Statistics returned after a restore operation.
#[derive(Debug, Clone, Serialize)]
pub struct RestoreStats {
    pub tables_restored: usize,
    pub books_restored: i64,
    pub files_restored: usize,
}

/// Create a backup archive at the specified path.
pub async fn create_backup(
    pool: &DbPool,
    storage_root: &str,
    output_path: &Path,
    include_proxy_blobs: bool,
) -> BackupResult<BackupManifest> {
    info!("Creating backup at {:?}", output_path);

    // Create temp directory for staging
    let temp_dir = tempfile::tempdir()?;
    let staging = temp_dir.path();

    // Create directory structure
    let db_dir = staging.join("database");
    let storage_dir = staging.join("storage");
    fs::create_dir_all(&db_dir)?;
    fs::create_dir_all(&storage_dir)?;

    // Export database tables
    for table in TABLES_IN_ORDER {
        if *table == "proxy_logs" && !include_proxy_blobs {
            // Skip proxy_logs content if not including blobs
            // Still create empty file to maintain structure
            debug!("Skipping proxy_logs table (include_proxy_blobs=false)");
        }
        export_table(pool, table, &db_dir).await?;
    }

    // Count books
    let books_count: i64 = sqlx::query_scalar!(r#"SELECT COUNT(*) as "count!" FROM books"#)
        .fetch_one(pool)
        .await?;

    let mut dirs = vec!["books", "covers", "voices"];
    if include_proxy_blobs {
        dirs.push("proxy_blobs");
    }
    let files_count = copy_trees(Path::new(storage_root), &storage_dir, dirs, false).await?;

    // Create manifest
    let manifest = BackupManifest::new(books_count, files_count, include_proxy_blobs);
    let manifest_path = staging.join("manifest.json");
    let manifest_json = serde_json::to_string_pretty(&manifest)?;
    fs::write(&manifest_path, &manifest_json)?;

    // Create tar.gz archive.
    //
    // gzip over the whole staged dump is CPU-bound and runs for as long as the
    // dump is large. `create_backup` is awaited straight from an HTTP handler,
    // so leaving this inline pins a runtime worker -- and the worker that gets
    // pinned is also the one driving tokio's I/O driver, which stops every
    // other request being served for the duration. Same reasoning as the
    // filesystem walks in the episode_cache jobs.
    {
        let output_path = output_path.to_path_buf();
        let staging = staging.to_path_buf();
        tokio::task::spawn_blocking(move || -> BackupResult<()> {
            let output_file = File::create(&output_path)?;
            let encoder = GzEncoder::new(output_file, Compression::default());
            let mut archive = Builder::new(encoder);
            archive.append_dir_all(".", &staging)?;
            let encoder = archive.into_inner()?;
            encoder.finish()?;
            Ok(())
        })
        .await
        .expect("backup archive task panicked")?;
    }

    info!(
        "Backup created: {} tables, {} books, {} files",
        TABLES_IN_ORDER.len(),
        books_count,
        files_count
    );

    Ok(manifest)
}

/// Export a single table to a CSV file using PostgreSQL COPY TO STDOUT.
async fn export_table(pool: &DbPool, table: &str, output_dir: &Path) -> BackupResult<()> {
    debug!("Exporting table: {}", table);

    let output_path = output_dir.join(format!("{}.csv", table));

    // Get column names from schema (validated source, not user input)
    let columns = get_table_columns(pool, table).await?;

    if columns.is_empty() {
        warn!("Table {} has no columns or doesn't exist, skipping", table);
        fs::write(&output_path, "")?;
        return Ok(());
    }

    let column_list = columns.join(", ");

    // Use PostgreSQL COPY TO STDOUT for high-performance export
    let copy_query = format!(
        "COPY {} ({}) TO STDOUT WITH (FORMAT CSV, HEADER)",
        table, column_list
    );

    let mut copy_stream = pool.copy_out_raw(&copy_query).await?;
    let mut output = File::create(&output_path)?;

    // Stream chunks directly to file
    while let Some(chunk) = copy_stream.try_next().await? {
        output.write_all(&chunk)?;
    }

    Ok(())
}

/// Get column names for a table from the database schema.
async fn get_table_columns(pool: &DbPool, table: &str) -> BackupResult<Vec<String>> {
    // Use parameterized query to safely get column names
    let columns: Vec<String> = sqlx::query_scalar!(
        "SELECT column_name::text as \"column_name!\" FROM information_schema.columns
         WHERE table_name = $1 ORDER BY ordinal_position",
        table,
    )
    .fetch_all(pool)
    .await?;

    Ok(columns)
}

async fn copy_trees(
    src: &Path,
    dst: &Path,
    dirs: Vec<&'static str>,
    replace: bool,
) -> io::Result<usize> {
    let (src, dst) = (src.to_path_buf(), dst.to_path_buf());
    tokio::task::spawn_blocking(move || {
        let mut count = 0;
        for dir in dirs {
            let (from, to) = (src.join(dir), dst.join(dir));
            if !from.exists() {
                continue;
            }
            if replace && to.exists() {
                fs::remove_dir_all(&to)?;
            }
            count += copy_dir_recursive(&from, &to)?;
        }
        Ok(count)
    })
    .await
    .map_err(io::Error::other)?
}

/// Recursively copy a directory. Returns number of files copied.
fn copy_dir_recursive(src: &Path, dst: &Path) -> io::Result<usize> {
    if !src.exists() {
        return Ok(0);
    }

    fs::create_dir_all(dst)?;
    let mut count = 0;

    for entry in fs::read_dir(src)? {
        let entry = entry?;
        let path = entry.path();
        let dest_path = dst.join(entry.file_name());

        if path.is_dir() {
            count += copy_dir_recursive(&path, &dest_path)?;
        } else {
            fs::copy(&path, &dest_path)?;
            count += 1;
        }
    }

    Ok(count)
}

/// Restore from a backup archive.
pub async fn restore_backup(
    pool: &DbPool,
    storage_root: &str,
    backup_path: &Path,
) -> BackupResult<RestoreStats> {
    info!("Restoring backup from {:?}", backup_path);

    // Create temp directory for extraction
    let temp_dir = tempfile::tempdir()?;
    let extract_dir = temp_dir.path();

    // Extract the archive safely (validate paths to prevent traversal attacks).
    // Decompress + unpack is blocking work proportional to the archive, so it
    // goes to the blocking pool for the same reason the compression side does.
    {
        let backup_path = backup_path.to_path_buf();
        let extract_dir = extract_dir.to_path_buf();
        tokio::task::spawn_blocking(move || -> BackupResult<()> {
            let file = File::open(&backup_path)?;
            let decoder = GzDecoder::new(BufReader::new(file));
            let mut archive = Archive::new(decoder);

            for entry in archive.entries()? {
                let mut entry = entry?;
                let path = entry.path()?;

                // Reject paths with parent directory references or absolute paths
                for component in path.components() {
                    if matches!(component, Component::ParentDir) {
                        return Err(BackupError::Invalid(
                            "path traversal detected in archive".to_string(),
                        ));
                    }
                    if matches!(component, Component::RootDir | Component::Prefix(_)) {
                        return Err(BackupError::Invalid(
                            "absolute path detected in archive".to_string(),
                        ));
                    }
                }

                entry.unpack_in(&extract_dir)?;
            }
            Ok(())
        })
        .await
        .expect("backup extract task panicked")?;
    }

    // Read and validate manifest
    let manifest_path = extract_dir.join("manifest.json");
    if !manifest_path.exists() {
        return Err(BackupError::Invalid("Missing manifest.json".to_string()));
    }

    let manifest_content = fs::read_to_string(&manifest_path)?;
    let manifest: BackupManifest = serde_json::from_str(&manifest_content)?;

    // Check version compatibility
    if !manifest.version.starts_with("1.") {
        return Err(BackupError::VersionMismatch {
            backup: manifest.version,
            current: "1.0".to_string(),
        });
    }

    info!(
        "Restoring backup from {} (created {})",
        manifest.kobbler_version, manifest.created_at
    );

    // Begin transaction
    let mut tx = pool.begin().await?;

    // Disable foreign key checks temporarily by truncating in reverse order
    // and then importing in forward order
    let tables_reversed: Vec<_> = TABLES_IN_ORDER.iter().rev().collect();

    for table in &tables_reversed {
        debug!("Truncating table: {}", table);
        let query = format!("TRUNCATE TABLE {} CASCADE", table);
        sqlx::query(sqlx::AssertSqlSafe(query.as_str()))
            .execute(&mut *tx)
            .await?;
    }

    // Import tables in order
    let db_dir = extract_dir.join("database");
    let mut tables_restored = 0;

    for table in TABLES_IN_ORDER {
        let csv_path = db_dir.join(format!("{}.csv", table));
        if csv_path.exists() {
            import_table(pool, &mut tx, table, &csv_path).await?;
            tables_restored += 1;
        }
    }

    // Commit database changes
    tx.commit().await?;

    let files_restored = copy_trees(
        &extract_dir.join("storage"),
        Path::new(storage_root),
        vec!["books", "covers", "voices", "proxy_blobs"],
        true,
    )
    .await?;

    // Get books count
    let books_restored: i64 = sqlx::query_scalar!(r#"SELECT COUNT(*) as "count!" FROM books"#)
        .fetch_one(pool)
        .await?;

    info!(
        "Restore completed: {} tables, {} books, {} files",
        tables_restored, books_restored, files_restored
    );

    Ok(RestoreStats {
        tables_restored,
        books_restored,
        files_restored,
    })
}

/// Import a table from a CSV file using PostgreSQL COPY FROM STDIN.
///
/// Handles schema evolution by only importing columns that exist in both
/// the backup CSV and the current database schema.
async fn import_table(
    pool: &DbPool,
    conn: &mut sqlx::PgConnection,
    table: &str,
    csv_path: &Path,
) -> BackupResult<()> {
    debug!("Importing table: {}", table);

    // Read CSV header to get backup columns
    let file = File::open(csv_path)?;
    let mut reader = BufReader::new(file);
    let mut header_line = String::new();
    reader.read_line(&mut header_line)?;

    let header_line = header_line.trim();
    if header_line.is_empty() {
        return Ok(()); // Empty file, nothing to import
    }

    let backup_columns: Vec<&str> = header_line.split(',').collect();
    if backup_columns.is_empty() {
        return Ok(());
    }

    // Get current schema columns (use pool for metadata query)
    let schema_columns = get_table_columns(pool, table).await?;
    let schema_set: HashSet<&str> = schema_columns.iter().map(|s| s.as_str()).collect();

    // Find columns that exist in both backup and current schema
    let mut import_columns: Vec<&str> = Vec::new();
    let mut import_indices: Vec<usize> = Vec::new();

    for (i, col) in backup_columns.iter().enumerate() {
        if schema_set.contains(col) {
            import_columns.push(col);
            import_indices.push(i);
        } else {
            debug!(
                "Table {}: skipping column '{}' (not in current schema)",
                table, col
            );
        }
    }

    if import_columns.is_empty() {
        warn!(
            "Table {}: no matching columns between backup and schema",
            table
        );
        return Ok(());
    }

    // Log schema evolution info
    let skipped_count = backup_columns.len() - import_columns.len();
    if skipped_count > 0 {
        info!(
            "Table {}: importing {} of {} columns ({} skipped)",
            table,
            import_columns.len(),
            backup_columns.len(),
            skipped_count
        );
    }

    let new_columns: Vec<&String> = schema_columns
        .iter()
        .filter(|c| !backup_columns.contains(&c.as_str()))
        .collect();
    if !new_columns.is_empty() {
        debug!(
            "Table {}: {} new columns will use defaults: {:?}",
            table,
            new_columns.len(),
            new_columns
        );
    }

    // Check if we need to filter columns or can stream directly
    let needs_filtering = import_indices.len() != backup_columns.len();

    let column_list = import_columns.join(", ");
    let copy_query = format!(
        "COPY {} ({}) FROM STDIN WITH (FORMAT CSV, HEADER)",
        table, column_list
    );

    // Use COPY FROM STDIN on the provided connection
    let mut copy_sink = conn.copy_in_raw(&copy_query).await?;

    if needs_filtering {
        // Filter CSV to only include matching columns
        let filtered = filter_csv_columns(csv_path, &import_indices)?;
        copy_sink.send(filtered).await?;
    } else {
        // Stream file directly - all columns match
        let content = fs::read(csv_path)?;
        copy_sink.send(content).await?;
    }

    copy_sink.finish().await?;

    Ok(())
}

/// Filter a CSV file to only include specified column indices.
/// Returns filtered CSV content as bytes (with header).
fn filter_csv_columns(csv_path: &Path, indices: &[usize]) -> BackupResult<Vec<u8>> {
    let file = File::open(csv_path)?;
    let mut reader = csv::ReaderBuilder::new()
        .has_headers(true)
        .from_reader(file);

    let mut output = Vec::new();
    let mut writer = csv::WriterBuilder::new().from_writer(&mut output);

    // Get headers and write filtered header
    let headers = reader.headers()?.clone();
    let filtered_headers: Vec<&str> = indices.iter().filter_map(|&i| headers.get(i)).collect();
    writer.write_record(&filtered_headers)?;

    // Process data rows
    for result in reader.records() {
        let record = result?;
        let filtered: Vec<&str> = indices.iter().filter_map(|&i| record.get(i)).collect();
        writer.write_record(&filtered)?;
    }

    writer.flush()?;
    drop(writer);

    Ok(output)
}

/// Get the path where a backup file should be created.
pub fn backup_filename() -> String {
    let now = Utc::now();
    format!("kobbler-backup-{}.tar.gz", now.format("%Y%m%d-%H%M%S"))
}

/// Read and return backup file as bytes for streaming.
pub fn read_backup_file(path: &Path) -> io::Result<Vec<u8>> {
    fs::read(path)
}
