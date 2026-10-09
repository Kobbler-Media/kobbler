//! Media root write-images job handler.
//!
//! Walks all added files in a media root and writes cached images to the filesystem
//! using conventional filenames (poster.jpg, fanart.jpg, cover.jpg, etc.).
//! Does not overwrite existing files.

use crate::images::{ImageMap, ImageType};
use crate::jobs::worker::JobHandler;
use crate::jobs::{JobError, JobResult};
use crate::repositories::{media_root_files, media_roots};
use crate::sidecar::images as sidecar_images;
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sqlx::PgPool;
use std::collections::HashSet;
use std::path::Path;
use std::sync::Arc;
use tokio::sync::Semaphore;
use tokio_util::sync::CancellationToken;
use tracing::{debug, info, warn};
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MediaRootWriteImagesPayload {
    pub media_root_id: Uuid,
}

pub struct MediaRootWriteImagesHandler {
    pool: PgPool,
    fs_semaphore: Arc<Semaphore>,
}

impl MediaRootWriteImagesHandler {
    pub fn new(pool: PgPool, fs_semaphore: Arc<Semaphore>) -> Self {
        Self { pool, fs_semaphore }
    }
}

#[async_trait]
impl JobHandler for MediaRootWriteImagesHandler {
    fn job_type(&self) -> &'static str {
        "media_root_write_images"
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
        let payload: MediaRootWriteImagesPayload = serde_json::from_value(payload)?;

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
                    let file_path = Path::new(&file.file_path);
                    let Some(dir) = file_path.parent() else {
                        continue;
                    };
                    let image_map = load_entity_images(&self.pool, "movies", entity_id).await;
                    let image_keys = [
                        ("poster", ImageType::MoviePoster),
                        ("backdrop", ImageType::MovieBackdrop),
                        ("logo", ImageType::MovieLogo),
                    ];
                    for (key, img_type) in &image_keys {
                        if image_map.contains_key(*key) {
                            match sidecar_images::write_image_sidecar(
                                &root.path, *img_type, entity_id, dir, key, "movie",
                            )
                            .await
                            {
                                Ok(true) => written += 1,
                                Ok(false) => skipped += 1,
                                Err(e) => {
                                    debug!(error = %e, key, "Failed to write movie image");
                                    skipped += 1;
                                }
                            }
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
                    // Entity is a tv_episode - find its show
                    let show_id = match get_show_id_for_episode(&self.pool, entity_id).await {
                        Some(id) => id,
                        None => continue,
                    };
                    if !seen_shows.insert(show_id) {
                        continue;
                    }
                    // Determine show dir (2 levels up from episode)
                    let file_path = Path::new(&file.file_path);
                    let Some(show_dir) = file_path.parent().and_then(|s| s.parent()) else {
                        continue;
                    };
                    let image_map = load_entity_images(&self.pool, "tv_shows", show_id).await;
                    let image_keys = [
                        ("poster", ImageType::TvShowPoster),
                        ("backdrop", ImageType::TvShowBackdrop),
                        ("logo", ImageType::TvShowLogo),
                    ];
                    for (key, img_type) in &image_keys {
                        if image_map.contains_key(*key) {
                            match sidecar_images::write_image_sidecar(
                                &root.path, *img_type, show_id, show_dir, key, "tv",
                            )
                            .await
                            {
                                Ok(true) => written += 1,
                                Ok(false) => skipped += 1,
                                Err(e) => {
                                    debug!(error = %e, key, "Failed to write TV image");
                                    skipped += 1;
                                }
                            }
                        }
                    }
                }
            }
            "book" => {
                let mut seen_entities = HashSet::new();
                let mut seen_dirs = HashSet::new();
                for file in &files {
                    let Some(entity_id) = file.entity_id else {
                        continue;
                    };
                    if !seen_entities.insert(entity_id) {
                        continue;
                    }
                    let file_path = Path::new(&file.file_path);
                    let Some(dir) = file_path.parent() else {
                        continue;
                    };
                    if !seen_dirs.insert(dir.to_path_buf()) {
                        continue;
                    }
                    let image_map = load_entity_images(&self.pool, "books", entity_id).await;
                    if image_map.contains_key("cover") {
                        // Books use ImageType::Series for cover caching (matches cover endpoint)
                        match sidecar_images::write_image_sidecar(
                            &root.path,
                            ImageType::Series,
                            entity_id,
                            dir,
                            "cover",
                            "book",
                        )
                        .await
                        {
                            Ok(true) => written += 1,
                            Ok(false) => skipped += 1,
                            Err(e) => {
                                debug!(error = %e, "Failed to write book cover image");
                                skipped += 1;
                            }
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
            "Image writing complete"
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

async fn load_entity_images(pool: &PgPool, table: &str, entity_id: Uuid) -> ImageMap {
    let sql =
        format!("SELECT COALESCE(images, '{{}}'::jsonb) AS images FROM {table} WHERE id = $1");
    let row: Option<(Value,)> = sqlx::query_as(sqlx::AssertSqlSafe(sql.as_str()))
        .bind(entity_id)
        .fetch_optional(pool)
        .await
        .unwrap_or(None);

    match row {
        Some((val,)) => serde_json::from_value(val).unwrap_or_default(),
        None => {
            warn!(entity_id = %entity_id, table, "Entity not found for image writing");
            ImageMap::new()
        }
    }
}
