//! Media root backfill job handler.
//!
//! Cross-references entity file tables (book_files, movie_files, tv_episode_files)
//! against a media root's path and creates missing `media_root_files` entries
//! directly as 'added', bypassing the scan→interpret→match pipeline.

use crate::jobs::worker::JobHandler;
use crate::jobs::{JobError, JobResult};
use crate::repositories::{media_root_files, media_roots};
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sqlx::{PgPool, Row};
use std::path::Path;
use tokio_util::sync::CancellationToken;
use tracing::{info, warn};
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MediaRootBackfillPayload {
    pub media_root_id: Uuid,
}

pub struct MediaRootBackfillHandler {
    pool: PgPool,
}

impl MediaRootBackfillHandler {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}

#[async_trait]
impl JobHandler for MediaRootBackfillHandler {
    fn job_type(&self) -> &'static str {
        "media_root_backfill"
    }

    fn max_concurrent(&self) -> usize {
        1
    }

    async fn execute(
        &self,
        _job_id: Uuid,
        payload: Value,
        cancel: CancellationToken,
    ) -> JobResult<Value> {
        let payload: MediaRootBackfillPayload = serde_json::from_value(payload)?;

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

        let path_prefix = if root.path.ends_with('/') {
            root.path.clone()
        } else {
            format!("{}/", root.path)
        };

        nats_jobs::check_cancelled(&cancel)?;

        let (backfilled, already_added, errors) = match root.media_type.as_str() {
            "book" | "audiobook" => backfill_books(&self.pool, root.id, &path_prefix).await?,
            "movie" => backfill_movies(&self.pool, root.id, &path_prefix).await?,
            "tv" => backfill_tv_episodes(&self.pool, root.id, &path_prefix).await?,
            other => {
                return Err(JobError::ExecutionFailed(format!(
                    "Unsupported media type for backfill: {other}"
                )));
            }
        };

        let total = backfilled + already_added + errors;
        info!(
            root = %root.name,
            media_type = %root.media_type,
            backfilled,
            already_added,
            errors,
            total,
            "Media root backfill complete"
        );

        Ok(json!({
            "backfilled": backfilled,
            "already_added": already_added,
            "errors": errors,
            "total": total,
        }))
    }
}

/// Backfill book_files + audiobook_files → media_root_files.
async fn backfill_books(
    pool: &PgPool,
    media_root_id: Uuid,
    path_prefix: &str,
) -> JobResult<(u64, u64, u64)> {
    let pattern = format!("{path_prefix}%");

    // Query both book_files and audiobook_files
    let rows = sqlx::query(
        r#"
        SELECT book_id, file_path, size_bytes AS file_size FROM book_files
        WHERE file_path LIKE $1 AND available = TRUE
        UNION ALL
        SELECT book_id, file_path, file_size FROM audiobook_files
        WHERE file_path LIKE $1 AND available = TRUE
        "#,
    )
    .bind(&pattern)
    .fetch_all(pool)
    .await?;

    let mut backfilled = 0u64;
    let mut already_added = 0u64;
    let mut errors = 0u64;

    for row in rows {
        let book_id: Uuid = row.get("book_id");
        let path: String = row.get("file_path");
        let size_bytes: i64 = row.get("file_size");
        let file_name = Path::new(&path)
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("")
            .to_string();

        // Check if already 'added' to avoid unnecessary upserts
        let existing =
            sqlx::query("SELECT match_status FROM media_root_files WHERE file_path = $1")
                .bind(&path)
                .fetch_optional(pool)
                .await;

        match existing {
            Ok(Some(r)) if r.get::<String, _>("match_status") == "added" => {
                already_added += 1;
                continue;
            }
            _ => {}
        }

        match media_root_files::upsert_added(
            pool,
            media_root_id,
            &path,
            &file_name,
            Some(size_bytes),
            "book",
            book_id,
        )
        .await
        {
            Ok(_) => {
                info!(path = %path, book_id = %book_id, "Backfilled book file");
                backfilled += 1;
            }
            Err(e) => {
                warn!(path = %path, error = %e, "Failed to backfill book file");
                errors += 1;
            }
        }
    }

    Ok((backfilled, already_added, errors))
}

/// Backfill movie_files → media_root_files.
async fn backfill_movies(
    pool: &PgPool,
    media_root_id: Uuid,
    path_prefix: &str,
) -> JobResult<(u64, u64, u64)> {
    let pattern = format!("{path_prefix}%");
    let rows = sqlx::query(
        r#"
        SELECT mf.movie_id, mf.file_path, mf.file_size
        FROM movie_files mf
        WHERE mf.file_path LIKE $1 AND mf.available = TRUE
        "#,
    )
    .bind(&pattern)
    .fetch_all(pool)
    .await?;

    let mut backfilled = 0u64;
    let mut already_added = 0u64;
    let mut errors = 0u64;

    for row in rows {
        let movie_id: Uuid = row.get("movie_id");
        let path: String = row.get("file_path");
        let file_size: Option<i64> = row.get("file_size");
        let file_name = Path::new(&path)
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("")
            .to_string();

        let existing =
            sqlx::query("SELECT match_status FROM media_root_files WHERE file_path = $1")
                .bind(&path)
                .fetch_optional(pool)
                .await;

        match existing {
            Ok(Some(r)) if r.get::<String, _>("match_status") == "added" => {
                already_added += 1;
                continue;
            }
            _ => {}
        }

        match media_root_files::upsert_added(
            pool,
            media_root_id,
            &path,
            &file_name,
            file_size,
            "movie",
            movie_id,
        )
        .await
        {
            Ok(_) => {
                info!(path = %path, movie_id = %movie_id, "Backfilled movie file");
                backfilled += 1;
            }
            Err(e) => {
                warn!(path = %path, error = %e, "Failed to backfill movie file");
                errors += 1;
            }
        }
    }

    Ok((backfilled, already_added, errors))
}

/// Backfill tv_episode_files → media_root_files.
async fn backfill_tv_episodes(
    pool: &PgPool,
    media_root_id: Uuid,
    path_prefix: &str,
) -> JobResult<(u64, u64, u64)> {
    let pattern = format!("{path_prefix}%");
    let rows = sqlx::query(
        r#"
        SELECT tefe.episode_id, ef.file_path, ef.file_size
        FROM tv_episode_files ef
        JOIN tv_episode_file_episodes tefe ON tefe.episode_file_id = ef.id
        WHERE ef.file_path LIKE $1 AND ef.available = TRUE
        "#,
    )
    .bind(&pattern)
    .fetch_all(pool)
    .await?;

    let mut backfilled = 0u64;
    let mut already_added = 0u64;
    let mut errors = 0u64;

    for row in rows {
        let episode_id: Uuid = row.get("episode_id");
        let path: String = row.get("file_path");
        let file_size: Option<i64> = row.get("file_size");
        let file_name = Path::new(&path)
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("")
            .to_string();

        let existing =
            sqlx::query("SELECT match_status FROM media_root_files WHERE file_path = $1")
                .bind(&path)
                .fetch_optional(pool)
                .await;

        match existing {
            Ok(Some(r)) if r.get::<String, _>("match_status") == "added" => {
                already_added += 1;
                continue;
            }
            _ => {}
        }

        match media_root_files::upsert_added(
            pool,
            media_root_id,
            &path,
            &file_name,
            file_size,
            "tv_episode",
            episode_id,
        )
        .await
        {
            Ok(_) => {
                info!(path = %path, episode_id = %episode_id, "Backfilled TV episode file");
                backfilled += 1;
            }
            Err(e) => {
                warn!(path = %path, error = %e, "Failed to backfill TV episode file");
                errors += 1;
            }
        }
    }

    Ok((backfilled, already_added, errors))
}
