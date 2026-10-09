//! Media root write-sidecars job handler.
//!
//! Walks all added files in a media root and writes sidecar files (NFO, metadata.json,
//! metadata.opf) for entities that don't already have them on disk. Delegates to the
//! shared builders in `sidecar::write_back` for metadata construction and merge.

use crate::jobs::worker::JobHandler;
use crate::jobs::{JobError, JobResult};
use crate::repositories::{media_root_files, media_roots};
use crate::sidecar::SidecarFormat;
use crate::sidecar::write_back;
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sqlx::PgPool;
use std::collections::HashSet;
use std::path::Path;
use std::sync::Arc;
use tokio::sync::Semaphore;
use tokio_util::sync::CancellationToken;
use tracing::{debug, info};
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MediaRootWriteSidecarsPayload {
    pub media_root_id: Uuid,
}

pub struct MediaRootWriteSidecarsHandler {
    pool: PgPool,
    fs_semaphore: Arc<Semaphore>,
}

impl MediaRootWriteSidecarsHandler {
    pub fn new(pool: PgPool, fs_semaphore: Arc<Semaphore>) -> Self {
        Self { pool, fs_semaphore }
    }
}

#[async_trait]
impl JobHandler for MediaRootWriteSidecarsHandler {
    fn job_type(&self) -> &'static str {
        "media_root_write_sidecars"
    }

    async fn execute(
        &self,
        _job_id: Uuid,
        payload: Value,
        cancel: CancellationToken,
    ) -> JobResult<Value> {
        let _fs_permit = self
            .fs_semaphore
            .acquire()
            .await
            .map_err(|_| JobError::ExecutionFailed("FS semaphore closed".into()))?;
        let payload: MediaRootWriteSidecarsPayload = serde_json::from_value(payload)?;

        let root = match media_roots::get_by_id(&self.pool, payload.media_root_id).await? {
            Some(r) => r,
            None => {
                tracing::warn!(
                    "Media root {} no longer exists, skipping",
                    payload.media_root_id
                );
                return Ok(json!({"skipped": true, "reason": "media_root_deleted"}));
            }
        };

        if !media_roots::check_available(&self.pool, &root).await? {
            return Ok(json!({"deferred": true, "reason": "media_root_unavailable"}));
        }

        let files =
            media_root_files::list_for_root(&self.pool, root.id, Some("added"), 10000, 0).await?;

        let mut written = 0u64;
        let mut skipped = 0u64;

        match root.media_type.as_str() {
            "movie" => {
                let mut seen_entities = HashSet::new();
                for file in &files {
                    nats_jobs::check_cancelled(&cancel)?;
                    let Some(entity_id) = file.entity_id else {
                        continue;
                    };
                    if !seen_entities.insert(entity_id) {
                        continue;
                    }
                    match write_movie_sidecar(&self.pool, entity_id, file).await {
                        Ok(true) => written += 1,
                        Ok(false) => skipped += 1,
                        Err(e) => {
                            debug!(error = %e, "Failed to write movie sidecar");
                            skipped += 1;
                        }
                    }
                }
            }
            "tv" => {
                let mut seen_shows = HashSet::new();
                for file in &files {
                    let Some(entity_id) = file.entity_id else {
                        continue;
                    };
                    let show_id = match get_show_id_for_episode(&self.pool, entity_id).await {
                        Some(id) => id,
                        None => continue,
                    };
                    if !seen_shows.insert(show_id) {
                        continue;
                    }
                    match write_tvshow_sidecar(&self.pool, show_id, file).await {
                        Ok(true) => written += 1,
                        Ok(false) => skipped += 1,
                        Err(e) => {
                            debug!(error = %e, "Failed to write tvshow.nfo");
                            skipped += 1;
                        }
                    }
                }
            }
            "book" | "audiobook" => {
                let mut seen_entities = HashSet::new();
                let mut seen_dirs = HashSet::new();
                for file in &files {
                    let Some(entity_id) = file.entity_id else {
                        continue;
                    };
                    if !seen_entities.insert(entity_id) {
                        continue;
                    }
                    let dir = Path::new(&file.file_path).parent().map(|p| p.to_path_buf());
                    let Some(dir) = dir else { continue };
                    if !seen_dirs.insert(dir.clone()) {
                        continue;
                    }
                    match write_book_sidecar(&self.pool, entity_id, &file.file_path).await {
                        Ok(true) => written += 1,
                        Ok(false) => skipped += 1,
                        Err(e) => {
                            debug!(error = %e, "Failed to write book sidecar");
                            skipped += 1;
                        }
                    }
                }
            }
            _ => {}
        }

        info!(
            root = %root.name,
            written = written,
            skipped = skipped,
            "Sidecar writing complete"
        );

        Ok(json!({"written": written, "skipped": skipped}))
    }
}

async fn get_show_id_for_episode(pool: &PgPool, episode_id: Uuid) -> Option<Uuid> {
    sqlx::query_scalar::<_, Uuid>(
        "SELECT ts.show_id FROM tv_episodes te JOIN tv_seasons ts ON te.season_id = ts.id WHERE te.id = $1",
    )
    .bind(episode_id)
    .fetch_optional(pool)
    .await
    .ok()
    .flatten()
}

async fn write_movie_sidecar(
    pool: &PgPool,
    movie_id: Uuid,
    file: &media_root_files::MediaRootFileRow,
) -> Result<bool, Box<dyn std::error::Error + Send + Sync>> {
    let file_path = Path::new(&file.file_path);
    let dir = file_path.parent().ok_or("no parent dir")?;
    let stem = file_path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("movie");

    // Skip if sidecar already exists
    let movie_nfo = dir.join("movie.nfo");
    let stem_nfo = dir.join(format!("{stem}.nfo"));
    let already_exists =
        tokio::task::spawn_blocking(move || movie_nfo.exists() || stem_nfo.exists())
            .await
            .unwrap_or(false);
    if already_exists {
        return Ok(false);
    }

    let enriched = write_back::build_movie_sidecar(pool, movie_id)
        .await
        .ok_or("movie not found")?;

    // Read existing sidecar for merge (shouldn't exist given the skip above, but be safe)
    let existing = crate::sidecar::read_sidecar(file_path, crate::sidecar::MediaType::Movie).await;
    let merged = match existing {
        Some(ref ex) => write_back::merge_sidecar(ex, &enriched),
        None => enriched,
    };

    crate::sidecar::write_sidecar(dir, stem, &merged, &[SidecarFormat::Nfo]).await?;
    Ok(true)
}

async fn write_tvshow_sidecar(
    pool: &PgPool,
    show_id: Uuid,
    file: &media_root_files::MediaRootFileRow,
) -> Result<bool, Box<dyn std::error::Error + Send + Sync>> {
    let file_path = Path::new(&file.file_path);
    let show_dir = file_path
        .parent()
        .and_then(|s| s.parent())
        .ok_or("cannot determine show directory")?;

    let tvshow_nfo = show_dir.join("tvshow.nfo");
    let already_exists = tokio::task::spawn_blocking(move || tvshow_nfo.exists())
        .await
        .unwrap_or(false);
    if already_exists {
        return Ok(false);
    }

    let enriched = write_back::build_tvshow_sidecar(pool, show_id)
        .await
        .ok_or("tv show not found")?;

    let existing = crate::sidecar::read_tvshow_nfo(show_dir).await;
    let merged = match existing {
        Some(ref ex) => write_back::merge_sidecar(ex, &enriched),
        None => enriched,
    };

    crate::sidecar::write_sidecar(show_dir, "tvshow", &merged, &[SidecarFormat::Nfo]).await?;
    Ok(true)
}

async fn write_book_sidecar(
    pool: &PgPool,
    book_id: Uuid,
    file_path_str: &str,
) -> Result<bool, Box<dyn std::error::Error + Send + Sync>> {
    let file_path = Path::new(file_path_str);
    let dir = file_path.parent().ok_or("no parent dir")?;

    let meta_json = dir.join("metadata.json");
    let already_exists = tokio::task::spawn_blocking(move || meta_json.exists())
        .await
        .unwrap_or(false);
    if already_exists {
        return Ok(false);
    }

    let enriched = write_back::build_book_sidecar(pool, book_id)
        .await
        .ok_or("book not found")?;

    // metadata.json: merge-based
    let json_path = dir.join("metadata.json");
    crate::sidecar::book_json::write_metadata_json_merged(&json_path, &enriched).await?;

    // metadata.opf: full regeneration
    let stem = file_path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("book");
    crate::sidecar::write_sidecar(dir, stem, &enriched, &[SidecarFormat::CalibreOpf]).await?;

    Ok(true)
}
