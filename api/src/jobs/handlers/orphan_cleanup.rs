//! Orphan cleanup handler.
//!
//! Scans storage directories and removes files/directories for entities that
//! no longer exist in the database. Covers image caches, book covers, book
//! file directories, thumbnail directories, subtitle caches, and stale
//! transcode session directories.

use crate::RuntimeConfigManager;
use crate::jobs::worker::JobHandler;
use crate::jobs::{JobError, JobResult};
use async_trait::async_trait;
use serde_json::{Value, json};
use sqlx::PgPool;
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use tokio_util::sync::CancellationToken;
use tracing::{info, warn};
use uuid::Uuid;

pub struct OrphanCleanupHandler {
    pool: PgPool,
    storage_root: String,
    runtime_config: RuntimeConfigManager,
}

impl OrphanCleanupHandler {
    pub fn new(pool: PgPool, storage_root: String, runtime_config: RuntimeConfigManager) -> Self {
        Self {
            pool,
            storage_root,
            runtime_config,
        }
    }
}

/// Directory-to-table mapping for image cleanup.
struct CleanupTarget {
    /// Path relative to storage_root (e.g. "images/authors").
    dir: &'static str,
    /// DB table name.
    table: &'static str,
}

const IMAGE_TARGETS: &[CleanupTarget] = &[
    CleanupTarget {
        dir: "images/authors",
        table: "authors",
    },
    CleanupTarget {
        dir: "images/series",
        table: "series",
    },
    CleanupTarget {
        dir: "images/movies",
        table: "movies",
    },
    CleanupTarget {
        dir: "images/tv_shows",
        table: "tv_shows",
    },
    CleanupTarget {
        dir: "images/seasons",
        table: "tv_seasons",
    },
    CleanupTarget {
        dir: "images/episodes",
        table: "tv_episodes",
    },
    CleanupTarget {
        dir: "images/people",
        table: "people",
    },
    CleanupTarget {
        dir: "covers",
        table: "books",
    },
];

/// Maximum age for transcode session directories before they're considered stale.
const TRANSCODE_MAX_AGE: std::time::Duration = std::time::Duration::from_secs(24 * 3600);

#[async_trait]
impl JobHandler for OrphanCleanupHandler {
    fn job_type(&self) -> &'static str {
        "orphan_cleanup"
    }

    fn max_concurrent(&self) -> usize {
        1
    }

    async fn execute(
        &self,
        job_id: Uuid,
        _payload: Value,
        cancel: CancellationToken,
    ) -> JobResult<Value> {
        info!(job_id = %job_id, "Running orphan cleanup");

        let mut total_scanned: u64 = 0;
        let mut total_deleted: u64 = 0;
        let mut total_bytes_freed: u64 = 0;
        let mut per_dir = Vec::new();

        // Phase 1: Image/cover file cleanup
        for target in IMAGE_TARGETS {
            nats_jobs::check_cancelled(&cancel)?;
            let dir_path = Path::new(&self.storage_root).join(target.dir);
            if !dir_path.exists() {
                continue;
            }

            let (file_uuids, file_map) = scan_directory_files(&dir_path).await?;
            if file_uuids.is_empty() {
                continue;
            }

            let existing = query_existing_ids(&self.pool, target.table, &file_uuids).await?;
            let orphan_ids: Vec<&Uuid> = file_uuids
                .iter()
                .filter(|id| !existing.contains(id))
                .collect();

            let mut dir_deleted: u64 = 0;
            for orphan_id in &orphan_ids {
                if let Some(paths) = file_map.get(orphan_id) {
                    for path in paths {
                        if let Err(e) = tokio::fs::remove_file(path).await {
                            warn!(path = %path.display(), error = %e, "Failed to delete orphaned image");
                        } else {
                            dir_deleted += 1;
                        }
                    }
                }
            }

            total_scanned += file_uuids.len() as u64;
            total_deleted += dir_deleted;

            if dir_deleted > 0 {
                info!(
                    dir = target.dir,
                    deleted = dir_deleted,
                    "Cleaned up orphaned images"
                );
            }

            per_dir.push(json!({
                "dir": target.dir,
                "scanned": file_uuids.len(),
                "deleted": dir_deleted,
            }));
        }

        // Phase 2: Book file directory cleanup
        // Book files are stored as {storage_root}/books/{book_uuid}/...
        let books_dir = Path::new(&self.storage_root).join("books");
        if books_dir.exists() {
            let (dir_uuids, dir_map) = scan_directory_subdirs(&books_dir).await?;
            if !dir_uuids.is_empty() {
                let existing = query_existing_ids(&self.pool, "books", &dir_uuids).await?;
                let orphan_ids: Vec<&Uuid> = dir_uuids
                    .iter()
                    .filter(|id| !existing.contains(id))
                    .collect();

                let mut dirs_deleted: u64 = 0;
                for orphan_id in &orphan_ids {
                    if let Some(path) = dir_map.get(orphan_id) {
                        if let Err(e) = tokio::fs::remove_dir_all(path).await {
                            warn!(path = %path.display(), error = %e, "Failed to delete orphaned book directory");
                        } else {
                            dirs_deleted += 1;
                        }
                    }
                }

                total_scanned += dir_uuids.len() as u64;
                total_deleted += dirs_deleted;

                if dirs_deleted > 0 {
                    info!(
                        deleted = dirs_deleted,
                        "Cleaned up orphaned book directories"
                    );
                }

                per_dir.push(json!({
                    "dir": "books",
                    "scanned": dir_uuids.len(),
                    "deleted": dirs_deleted,
                }));
            }
        }

        // Query all valid file IDs once for phases 3 and 4
        nats_jobs::check_cancelled(&cancel)?;
        let valid_file_ids = query_all_file_ids(&self.pool).await?;

        // Phase 3: Thumbnail directory cleanup
        // Thumbnails are stored as {storage_root}/thumbnails/{file_id}/
        // File IDs come from movie_files, tv_episode_files, media_root_files
        let thumb_dir = Path::new(&self.storage_root).join("thumbnails");
        if thumb_dir.exists() {
            let (dir_uuids, dir_map) = scan_directory_subdirs(&thumb_dir).await?;
            if !dir_uuids.is_empty() {
                let orphan_ids: Vec<&Uuid> = dir_uuids
                    .iter()
                    .filter(|id| !valid_file_ids.contains(id))
                    .collect();

                let mut dirs_deleted: u64 = 0;
                let mut bytes_freed: u64 = 0;
                for orphan_id in &orphan_ids {
                    nats_jobs::check_cancelled(&cancel)?;
                    if let Some(path) = dir_map.get(orphan_id) {
                        let size = dir_size(path).await;
                        if let Err(e) = tokio::fs::remove_dir_all(path).await {
                            warn!(path = %path.display(), error = %e, "Failed to delete orphaned thumbnail directory");
                        } else {
                            dirs_deleted += 1;
                            bytes_freed += size;
                        }
                    }
                }

                total_scanned += dir_uuids.len() as u64;
                total_deleted += dirs_deleted;
                total_bytes_freed += bytes_freed;

                if dirs_deleted > 0 {
                    info!(
                        deleted = dirs_deleted,
                        bytes_freed = bytes_freed,
                        "Cleaned up orphaned thumbnail directories"
                    );
                }

                per_dir.push(json!({
                    "dir": "thumbnails",
                    "scanned": dir_uuids.len(),
                    "deleted": dirs_deleted,
                    "bytes_freed": bytes_freed,
                }));
            }
        }

        // Phase 3b: media_cache directory cleanup
        // Single-pass analysis artefacts live at {storage_root}/media_cache/{file_id}/
        // (trickplay, vhash, chromaprint, loudness, embeddings, black_frames).
        // When a file is replaced at a different path the old row is deleted but
        // its media_cache dir is left behind, so reap any whose id has no file row.
        let media_cache_dir = Path::new(&self.storage_root).join("media_cache");
        if media_cache_dir.exists() {
            let (dir_uuids, dir_map) = scan_directory_subdirs(&media_cache_dir).await?;
            if !dir_uuids.is_empty() {
                let orphan_ids: Vec<&Uuid> = dir_uuids
                    .iter()
                    .filter(|id| !valid_file_ids.contains(id))
                    .collect();

                let mut dirs_deleted: u64 = 0;
                let mut bytes_freed: u64 = 0;
                for orphan_id in &orphan_ids {
                    nats_jobs::check_cancelled(&cancel)?;
                    if let Some(path) = dir_map.get(orphan_id) {
                        let size = dir_size(path).await;
                        if let Err(e) = tokio::fs::remove_dir_all(path).await {
                            warn!(path = %path.display(), error = %e, "Failed to delete orphaned media_cache directory");
                        } else {
                            dirs_deleted += 1;
                            bytes_freed += size;
                        }
                    }
                }

                total_scanned += dir_uuids.len() as u64;
                total_deleted += dirs_deleted;
                total_bytes_freed += bytes_freed;

                if dirs_deleted > 0 {
                    info!(
                        deleted = dirs_deleted,
                        bytes_freed = bytes_freed,
                        "Cleaned up orphaned media_cache directories"
                    );
                }

                per_dir.push(json!({
                    "dir": "media_cache",
                    "scanned": dir_uuids.len(),
                    "deleted": dirs_deleted,
                    "bytes_freed": bytes_freed,
                }));
            }
        }

        // Phase 4: Subtitle cache cleanup
        // Subtitles cached at {cache_root}/subtitles/{file_id}/
        // Check both the configured cache_dir and the default location
        nats_jobs::check_cancelled(&cancel)?;
        let config = self.runtime_config.get().await;
        let default_cache = PathBuf::from(&self.storage_root).join("cache");
        let configured_cache = config.cache_dir.as_ref().map(PathBuf::from);

        let mut subtitle_dirs_to_scan: Vec<PathBuf> = Vec::new();
        // Always scan the configured (or default) location
        let active_cache = configured_cache.as_ref().unwrap_or(&default_cache);
        subtitle_dirs_to_scan.push(active_cache.join("subtitles"));
        // If configured cache differs from default, also scan the old default
        if configured_cache.is_some() {
            let old_default = default_cache.join("subtitles");
            if old_default.exists() && old_default != subtitle_dirs_to_scan[0] {
                subtitle_dirs_to_scan.push(old_default);
            }
        }

        for sub_dir in &subtitle_dirs_to_scan {
            if !sub_dir.exists() {
                continue;
            }
            nats_jobs::check_cancelled(&cancel)?;
            let (dir_uuids, dir_map) = scan_directory_subdirs(sub_dir).await?;
            if dir_uuids.is_empty() {
                continue;
            }

            let orphan_ids: Vec<&Uuid> = dir_uuids
                .iter()
                .filter(|id| !valid_file_ids.contains(id))
                .collect();

            let mut dirs_deleted: u64 = 0;
            let mut bytes_freed: u64 = 0;
            for orphan_id in &orphan_ids {
                nats_jobs::check_cancelled(&cancel)?;
                if let Some(path) = dir_map.get(orphan_id) {
                    let size = dir_size(path).await;
                    if let Err(e) = tokio::fs::remove_dir_all(path).await {
                        warn!(path = %path.display(), error = %e, "Failed to delete orphaned subtitle cache");
                    } else {
                        dirs_deleted += 1;
                        bytes_freed += size;
                    }
                }
            }

            total_scanned += dir_uuids.len() as u64;
            total_deleted += dirs_deleted;
            total_bytes_freed += bytes_freed;

            let label = if sub_dir == &subtitle_dirs_to_scan[0] {
                "subtitles"
            } else {
                "subtitles (old default)"
            };

            if dirs_deleted > 0 {
                info!(
                    dir = label,
                    deleted = dirs_deleted,
                    bytes_freed = bytes_freed,
                    "Cleaned up orphaned subtitle caches"
                );
            }

            per_dir.push(json!({
                "dir": label,
                "scanned": dir_uuids.len(),
                "deleted": dirs_deleted,
                "bytes_freed": bytes_freed,
            }));
        }

        // Phase 5: Stale transcode session directory cleanup
        // Transcode dirs at {transcode_cache_dir}/{session_id}/ (or default {storage_root}/cache/transcode/)
        // Sessions are in-memory, so we use age-based cleanup (>24h = stale)
        nats_jobs::check_cancelled(&cancel)?;
        let default_transcode = default_cache.join("transcode");
        let configured_transcode = config.transcode_cache_dir.as_ref().map(PathBuf::from);

        let mut transcode_dirs_to_scan: Vec<PathBuf> = Vec::new();
        let active_transcode = configured_transcode.as_ref().unwrap_or(&default_transcode);
        transcode_dirs_to_scan.push(active_transcode.clone());
        // If configured dir differs from default, also clean the old default
        if configured_transcode.is_some()
            && default_transcode.exists()
            && default_transcode != *active_transcode
        {
            transcode_dirs_to_scan.push(default_transcode);
        }

        for tc_dir in &transcode_dirs_to_scan {
            if !tc_dir.exists() {
                continue;
            }
            nats_jobs::check_cancelled(&cancel)?;

            let (stale_count, stale_bytes) =
                cleanup_stale_dirs(tc_dir, TRANSCODE_MAX_AGE, &cancel).await?;

            total_deleted += stale_count;
            total_bytes_freed += stale_bytes;

            let label = if tc_dir == &transcode_dirs_to_scan[0] {
                "transcode"
            } else {
                "transcode (old default)"
            };

            if stale_count > 0 {
                info!(
                    dir = label,
                    deleted = stale_count,
                    bytes_freed = stale_bytes,
                    "Cleaned up stale transcode directories"
                );
            }

            per_dir.push(json!({
                "dir": label,
                "deleted": stale_count,
                "bytes_freed": stale_bytes,
            }));
        }

        info!(
            total_scanned = total_scanned,
            total_deleted = total_deleted,
            total_bytes_freed = total_bytes_freed,
            "Orphan cleanup complete"
        );

        Ok(json!({
            "total_scanned": total_scanned,
            "total_deleted": total_deleted,
            "total_bytes_freed": total_bytes_freed,
            "directories": per_dir,
        }))
    }
}

/// Parse a UUID from the first 36 characters of a filename or directory name.
///
/// Handles `{uuid}`, `{uuid}_backdrop`, and legacy `{uuid}.jpg` patterns.
fn parse_uuid_from_name(name: &str) -> Option<Uuid> {
    if name.len() < 36 {
        return None;
    }
    Uuid::parse_str(&name[..36]).ok()
}

/// Scan a directory for files and return (unique UUIDs, map of UUID -> file paths).
async fn scan_directory_files(
    dir: &Path,
) -> Result<(HashSet<Uuid>, std::collections::HashMap<Uuid, Vec<PathBuf>>), JobError> {
    let mut uuids = HashSet::new();
    let mut file_map: std::collections::HashMap<Uuid, Vec<PathBuf>> =
        std::collections::HashMap::new();

    let mut entries = tokio::fs::read_dir(dir).await.map_err(|e| {
        JobError::ExecutionFailed(format!("Failed to read directory {}: {}", dir.display(), e))
    })?;

    while let Some(entry) = entries
        .next_entry()
        .await
        .map_err(|e| JobError::ExecutionFailed(format!("Failed to read dir entry: {}", e)))?
    {
        let path = entry.path();
        if !path.is_file() {
            continue;
        }
        if let Some(name) = path.file_name().and_then(|n| n.to_str())
            && let Some(id) = parse_uuid_from_name(name)
        {
            uuids.insert(id);
            file_map.entry(id).or_default().push(path);
        }
    }

    Ok((uuids, file_map))
}

/// Scan a directory for subdirectories and return (unique UUIDs, map of UUID -> dir path).
async fn scan_directory_subdirs(
    dir: &Path,
) -> Result<(HashSet<Uuid>, std::collections::HashMap<Uuid, PathBuf>), JobError> {
    let mut uuids = HashSet::new();
    let mut dir_map: std::collections::HashMap<Uuid, PathBuf> = std::collections::HashMap::new();

    let mut entries = tokio::fs::read_dir(dir).await.map_err(|e| {
        JobError::ExecutionFailed(format!("Failed to read directory {}: {}", dir.display(), e))
    })?;

    while let Some(entry) = entries
        .next_entry()
        .await
        .map_err(|e| JobError::ExecutionFailed(format!("Failed to read dir entry: {}", e)))?
    {
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        if let Some(name) = path.file_name().and_then(|n| n.to_str())
            && let Some(id) = parse_uuid_from_name(name)
        {
            uuids.insert(id);
            dir_map.insert(id, path);
        }
    }

    Ok((uuids, dir_map))
}

/// Query which of the given UUIDs exist in the specified table.
///
/// Uses explicit match arms since sqlx cannot parameterize table names.
async fn query_existing_ids(
    pool: &PgPool,
    table: &str,
    ids: &HashSet<Uuid>,
) -> Result<HashSet<Uuid>, JobError> {
    let id_vec: Vec<Uuid> = ids.iter().copied().collect();

    let rows: Vec<Uuid> = match table {
        "authors" => {
            sqlx::query_scalar!("SELECT id FROM authors WHERE id = ANY($1)", &id_vec)
                .fetch_all(pool)
                .await
        }
        "series" => {
            sqlx::query_scalar!("SELECT id FROM series WHERE id = ANY($1)", &id_vec)
                .fetch_all(pool)
                .await
        }
        "movies" => {
            sqlx::query_scalar!("SELECT id FROM movies WHERE id = ANY($1)", &id_vec)
                .fetch_all(pool)
                .await
        }
        "tv_shows" => {
            sqlx::query_scalar!("SELECT id FROM tv_shows WHERE id = ANY($1)", &id_vec)
                .fetch_all(pool)
                .await
        }
        "tv_seasons" => {
            sqlx::query_scalar!("SELECT id FROM tv_seasons WHERE id = ANY($1)", &id_vec)
                .fetch_all(pool)
                .await
        }
        "tv_episodes" => {
            sqlx::query_scalar!("SELECT id FROM tv_episodes WHERE id = ANY($1)", &id_vec)
                .fetch_all(pool)
                .await
        }
        "people" => {
            sqlx::query_scalar!("SELECT id FROM people WHERE id = ANY($1)", &id_vec)
                .fetch_all(pool)
                .await
        }
        "books" => {
            sqlx::query_scalar!("SELECT id FROM books WHERE id = ANY($1)", &id_vec)
                .fetch_all(pool)
                .await
        }
        _ => {
            return Err(JobError::ExecutionFailed(format!(
                "Unknown table: {}",
                table
            )));
        }
    }
    .map_err(|e| JobError::ExecutionFailed(format!("DB query for {} failed: {}", table, e)))?;

    Ok(rows.into_iter().collect())
}

/// Query all valid file IDs across the playback + scanner file tables.
/// audiobook_files is included so the media_cache phase doesn't reap audiobook
/// caches; it's harmless for the thumbnail/subtitle phases (audiobooks have none).
async fn query_all_file_ids(pool: &PgPool) -> Result<HashSet<Uuid>, JobError> {
    let rows: Vec<(Uuid,)> = sqlx::query_as(
        "SELECT id FROM movie_files \
         UNION ALL \
         SELECT id FROM tv_episode_files \
         UNION ALL \
         SELECT id FROM audiobook_files \
         UNION ALL \
         SELECT id FROM media_root_files",
    )
    .fetch_all(pool)
    .await
    .map_err(|e| JobError::ExecutionFailed(format!("Failed to query file IDs: {}", e)))?;

    Ok(rows.into_iter().map(|(id,)| id).collect())
}

/// Approximate size of a directory tree (best-effort, doesn't fail on errors).
async fn dir_size(path: &Path) -> u64 {
    let path = path.to_path_buf();
    tokio::task::spawn_blocking(move || dir_size_sync(&path))
        .await
        .unwrap_or(0)
}

fn dir_size_sync(path: &Path) -> u64 {
    let mut total: u64 = 0;
    if let Ok(entries) = std::fs::read_dir(path) {
        for entry in entries.flatten() {
            let ft = match entry.file_type() {
                Ok(ft) => ft,
                Err(_) => continue,
            };
            if ft.is_file() {
                total += entry.metadata().map(|m| m.len()).unwrap_or(0);
            } else if ft.is_dir() {
                total += dir_size_sync(&entry.path());
            }
        }
    }
    total
}

/// Delete subdirectories older than `max_age` (by modification time).
/// Returns (count_deleted, bytes_freed).
async fn cleanup_stale_dirs(
    parent: &Path,
    max_age: std::time::Duration,
    cancel: &CancellationToken,
) -> Result<(u64, u64), JobError> {
    let now = std::time::SystemTime::now();
    let mut deleted: u64 = 0;
    let mut bytes_freed: u64 = 0;

    let mut entries = tokio::fs::read_dir(parent).await.map_err(|e| {
        JobError::ExecutionFailed(format!(
            "Failed to read directory {}: {}",
            parent.display(),
            e
        ))
    })?;

    while let Some(entry) = entries
        .next_entry()
        .await
        .map_err(|e| JobError::ExecutionFailed(format!("Failed to read dir entry: {}", e)))?
    {
        nats_jobs::check_cancelled(cancel)?;

        let path = entry.path();
        if !path.is_dir() {
            continue;
        }

        let mtime = match entry.metadata().await.and_then(|m| m.modified()) {
            Ok(t) => t,
            Err(_) => continue,
        };

        if let Ok(age) = now.duration_since(mtime)
            && age > max_age
        {
            let size = dir_size(&path).await;
            if let Err(e) = tokio::fs::remove_dir_all(&path).await {
                warn!(path = %path.display(), error = %e, "Failed to delete stale directory");
            } else {
                deleted += 1;
                bytes_freed += size;
            }
        }
    }

    Ok((deleted, bytes_freed))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_uuid_from_name() {
        let uuid = Uuid::nil();
        let uuid_str = uuid.to_string();

        // Plain UUID (new format)
        assert_eq!(parse_uuid_from_name(&uuid_str), Some(uuid));

        // UUID with backdrop suffix (new format)
        assert_eq!(
            parse_uuid_from_name(&format!("{}_backdrop", uuid_str)),
            Some(uuid)
        );

        // Legacy .jpg format
        assert_eq!(
            parse_uuid_from_name(&format!("{}.jpg", uuid_str)),
            Some(uuid)
        );

        // Legacy backdrop .jpg
        assert_eq!(
            parse_uuid_from_name(&format!("{}_backdrop.jpg", uuid_str)),
            Some(uuid)
        );

        // Too short
        assert_eq!(parse_uuid_from_name("short"), None);

        // Not a UUID
        assert_eq!(
            parse_uuid_from_name("not-a-real-uuid-string-but-36chars!"),
            None
        );
    }
}
