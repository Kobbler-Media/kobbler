//! Media root reorganize job handler.
//!
//! Renames files in a media root to match the configured naming template.
//! For each added file, computes the expected path from entity metadata
//! and renames the file on disk + updates the DB if it differs.
//! Optionally also processes review/ambiguous files using their top match candidate.

use crate::jobs::worker::JobHandler;
use crate::jobs::{JobError, JobResult};
use crate::media_root_naming::{self, NamingContext};
use crate::repositories::media_root_files::{self, MediaRootFileRow};
use crate::repositories::media_roots;
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sqlx::{PgPool, Row};
use std::path::Path;
use std::sync::Arc;
use tokio::sync::Semaphore;
use tokio_util::sync::CancellationToken;
use tracing::{error, info, warn};
use uuid::Uuid;

/// Payload for media_root_reorganize jobs.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MediaRootReorganizePayload {
    pub media_root_id: Uuid,
    /// If true, only preview changes without executing them.
    #[serde(default)]
    pub dry_run: bool,
    /// If true, also process review/ambiguous files using their top match candidate.
    #[serde(default)]
    pub include_unconfirmed: bool,
}

pub struct MediaRootReorganizeHandler {
    pool: PgPool,
    fs_semaphore: Arc<Semaphore>,
}

impl MediaRootReorganizeHandler {
    pub fn new(pool: PgPool, fs_semaphore: Arc<Semaphore>) -> Self {
        Self { pool, fs_semaphore }
    }
}

/// Metadata needed to build a NamingContext for a TV episode.
struct TvEpisodeMeta {
    show_title: String,
    show_year: Option<i32>,
    season_number: i32,
    episode_number: i32,
    episode_title: Option<String>,
    content_category: Option<String>,
    round: Option<i32>,
}

/// Metadata needed to build a NamingContext for a movie.
struct MovieMeta {
    title: String,
    year: Option<i32>,
}

/// Metadata needed to build a NamingContext for a book.
struct BookMeta {
    title: String,
    author: Option<String>,
    series: Option<String>,
    series_index: Option<f64>,
}

/// Outcome of processing a single file rename.
enum RenameOutcome {
    Renamed,
    AlreadyCorrect,
    Error,
}

/// Process a single file: compute target path, optionally rename on disk, update DB.
#[allow(clippy::too_many_arguments)]
async fn process_rename(
    pool: &PgPool,
    root_path: &str,
    template: &str,
    file: &MediaRootFileRow,
    ctx: NamingContext,
    dry_run: bool,
    unconfirmed: bool,
    changes: &mut Vec<Value>,
    max_changes: usize,
) -> RenameOutcome {
    let target = media_root_naming::compute_target_path(root_path, template, &ctx);
    let target_str = target.to_string_lossy().to_string();

    if target_str == file.file_path {
        return RenameOutcome::AlreadyCorrect;
    }

    if dry_run {
        info!(from = %file.file_path, to = %target_str, "Would rename (dry run)");
        if changes.len() < max_changes {
            let mut entry = json!({"from": &file.file_path, "to": &target_str});
            if unconfirmed {
                entry["unconfirmed"] = json!(true);
            }
            changes.push(entry);
        }
        return RenameOutcome::Renamed;
    }

    // Create parent directories and rename
    if let Some(parent) = target.parent()
        && let Err(e) = tokio::fs::create_dir_all(parent).await
    {
        error!(path = %parent.display(), error = %e, "Failed to create parent directory");
        return RenameOutcome::Error;
    }

    if let Err(e) = tokio::fs::rename(&file.file_path, &target).await {
        error!(from = %file.file_path, to = %target_str, error = %e, "Failed to rename file");
        return RenameOutcome::Error;
    }

    // Update DB path
    let new_name = target
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("")
        .to_string();

    if let Err(e) = media_root_files::update_file_path(pool, file.id, &target_str, &new_name).await
    {
        error!(file_id = %file.id, error = %e, "Renamed on disk but failed to update DB");
        return RenameOutcome::Error;
    }

    info!(from = %file.file_path, to = %target_str, "Renamed file");
    RenameOutcome::Renamed
}

#[async_trait]
impl JobHandler for MediaRootReorganizeHandler {
    fn job_type(&self) -> &'static str {
        "media_root_reorganize"
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
        let _fs_permit = self
            .fs_semaphore
            .acquire()
            .await
            .map_err(|_| JobError::ExecutionFailed("FS semaphore closed".into()))?;
        let payload: MediaRootReorganizePayload = serde_json::from_value(payload)?;

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

        let template =
            root.naming_template
                .as_deref()
                .unwrap_or_else(|| match root.media_type.as_str() {
                    "tv" => media_root_naming::default_tv_template(),
                    "book" => media_root_naming::default_book_template(),
                    "audiobook" => media_root_naming::default_audiobook_template(),
                    "sport" => media_root_naming::default_sport_template(),
                    _ => media_root_naming::default_movie_template(),
                });

        let batch_size = 500i64;
        let mut renamed = 0u64;
        let mut already_correct = 0u64;
        let mut errors = 0u64;

        const MAX_CHANGES: usize = 200;
        let mut changes: Vec<Value> = Vec::new();

        // Process confirmed (added) files
        let mut offset = 0i64;
        loop {
            let files = media_root_files::list_for_root(
                &self.pool,
                root.id,
                Some("added"),
                batch_size,
                offset,
            )
            .await?;

            if files.is_empty() {
                break;
            }

            let batch_len = files.len() as i64;

            for file in &files {
                nats_jobs::check_cancelled(&cancel)?;
                let entity_type = match &file.entity_type {
                    Some(t) => t.as_str(),
                    None => {
                        warn!(file_id = %file.id, "Matched file missing entity_type, skipping");
                        errors += 1;
                        continue;
                    }
                };
                let entity_id = match file.entity_id {
                    Some(id) => id,
                    None => {
                        warn!(file_id = %file.id, "Matched file missing entity_id, skipping");
                        errors += 1;
                        continue;
                    }
                };

                let ctx =
                    match build_naming_context(&self.pool, entity_type, entity_id, &file.file_path)
                        .await
                    {
                        Ok(ctx) => ctx,
                        Err(e) => {
                            warn!(
                                file_id = %file.id,
                                entity_type = entity_type,
                                entity_id = %entity_id,
                                error = %e,
                                "Failed to build naming context"
                            );
                            errors += 1;
                            continue;
                        }
                    };

                match process_rename(
                    &self.pool,
                    &root.path,
                    template,
                    file,
                    ctx,
                    payload.dry_run,
                    false,
                    &mut changes,
                    MAX_CHANGES,
                )
                .await
                {
                    RenameOutcome::Renamed => renamed += 1,
                    RenameOutcome::AlreadyCorrect => already_correct += 1,
                    RenameOutcome::Error => errors += 1,
                }
            }

            offset += batch_len;
        }

        // Process unconfirmed (review + ambiguous) files
        let mut renamed_unconfirmed = 0u64;
        let mut already_correct_unconfirmed = 0u64;
        let mut errors_unconfirmed = 0u64;

        if payload.include_unconfirmed {
            for status in ["review", "ambiguous"] {
                let mut offset = 0i64;
                loop {
                    let files = media_root_files::list_for_root(
                        &self.pool,
                        root.id,
                        Some(status),
                        batch_size,
                        offset,
                    )
                    .await?;

                    if files.is_empty() {
                        break;
                    }

                    let batch_len = files.len() as i64;

                    for file in &files {
                        let candidates = match &file.match_candidates {
                            Some(c) => c,
                            None => {
                                warn!(file_id = %file.id, "Unconfirmed file missing match_candidates, skipping");
                                errors_unconfirmed += 1;
                                continue;
                            }
                        };

                        let top = match candidates.as_array().and_then(|a| a.first()) {
                            Some(c) => c,
                            None => {
                                warn!(file_id = %file.id, "Empty match_candidates array, skipping");
                                errors_unconfirmed += 1;
                                continue;
                            }
                        };

                        let ctx = match build_naming_context_from_candidate(
                            &root.media_type,
                            top,
                            file.interpretation.as_ref(),
                            &file.file_path,
                        ) {
                            Ok(ctx) => ctx,
                            Err(e) => {
                                warn!(
                                    file_id = %file.id,
                                    status = status,
                                    error = %e,
                                    "Failed to build naming context from candidate"
                                );
                                errors_unconfirmed += 1;
                                continue;
                            }
                        };

                        match process_rename(
                            &self.pool,
                            &root.path,
                            template,
                            file,
                            ctx,
                            payload.dry_run,
                            true,
                            &mut changes,
                            MAX_CHANGES,
                        )
                        .await
                        {
                            RenameOutcome::Renamed => renamed_unconfirmed += 1,
                            RenameOutcome::AlreadyCorrect => already_correct_unconfirmed += 1,
                            RenameOutcome::Error => errors_unconfirmed += 1,
                        }
                    }

                    offset += batch_len;
                }
            }
        }

        let total = renamed
            + already_correct
            + errors
            + renamed_unconfirmed
            + already_correct_unconfirmed
            + errors_unconfirmed;

        info!(
            root = %root.name,
            renamed = renamed,
            renamed_unconfirmed = renamed_unconfirmed,
            already_correct = already_correct + already_correct_unconfirmed,
            errors = errors + errors_unconfirmed,
            total = total,
            dry_run = payload.dry_run,
            include_unconfirmed = payload.include_unconfirmed,
            "Media root reorganize complete"
        );

        let mut result = json!({
            "renamed": renamed,
            "renamed_unconfirmed": renamed_unconfirmed,
            "already_correct": already_correct + already_correct_unconfirmed,
            "errors": errors + errors_unconfirmed,
            "total": total,
            "dry_run": payload.dry_run,
            "include_unconfirmed": payload.include_unconfirmed,
        });

        if payload.dry_run {
            let total_renamed = renamed + renamed_unconfirmed;
            let truncated = total_renamed as usize > changes.len();
            result["changes"] = json!(changes);
            result["truncated"] = json!(truncated);
        }

        Ok(result)
    }
}

/// Build a `NamingContext` by looking up entity metadata.
async fn build_naming_context(
    pool: &PgPool,
    entity_type: &str,
    entity_id: Uuid,
    file_path: &str,
) -> Result<NamingContext, JobError> {
    let ext = Path::new(file_path)
        .extension()
        .and_then(|e| e.to_str())
        .map(|s| s.to_string());

    match entity_type {
        "tv_episode" => {
            let meta = fetch_tv_episode_meta(pool, entity_id).await?;
            Ok(NamingContext {
                show_title: Some(meta.show_title),
                year: meta.show_year.map(|y| y as u32),
                season: Some(meta.season_number as u32),
                episode: Some(meta.episode_number as u32),
                episode_title: meta.episode_title,
                round: meta.round.map(|r| r as u32),
                ext,
                ..Default::default()
            })
        }
        "movie" => {
            let meta = fetch_movie_meta(pool, entity_id).await?;
            Ok(NamingContext {
                title: Some(meta.title),
                year: meta.year.map(|y| y as u32),
                ext,
                ..Default::default()
            })
        }
        "book" => {
            let meta = fetch_book_meta(pool, entity_id).await?;
            // Look up track number from audiobook_files if this is an audio file
            let track_number = sqlx::query_scalar::<_, Option<i32>>(
                "SELECT file_index FROM audiobook_files WHERE book_id = $1 AND file_path = $2",
            )
            .bind(entity_id)
            .bind(file_path)
            .fetch_optional(pool)
            .await?
            .flatten()
            .map(|n| (n + 1) as u32); // file_index is 0-based, track display is 1-based

            Ok(NamingContext {
                title: Some(meta.title),
                author: meta.author,
                series: meta.series.clone(),
                series_index: meta.series_index.map(|i| i as u32),
                track_number,
                ext,
                ..Default::default()
            })
        }
        other => Err(JobError::ExecutionFailed(format!(
            "Unsupported entity type: {other}"
        ))),
    }
}

/// Build a `NamingContext` from a match candidate JSON object (for unconfirmed files).
fn build_naming_context_from_candidate(
    media_type: &str,
    candidate: &Value,
    interpretation: Option<&Value>,
    file_path: &str,
) -> Result<NamingContext, JobError> {
    let ext = Path::new(file_path)
        .extension()
        .and_then(|e| e.to_str())
        .map(|s| s.to_string());

    match media_type {
        "book" | "audiobook" => Ok(NamingContext {
            title: candidate["title"].as_str().map(String::from),
            author: candidate["authors"]
                .as_array()
                .and_then(|a| a.first())
                .and_then(|v| v.as_str())
                .map(String::from),
            series: candidate["series"].as_str().map(String::from),
            series_index: candidate["series_index"].as_f64().map(|i| i as u32),
            ext,
            // track_number not available from candidate JSON for unconfirmed files
            ..Default::default()
        }),
        "movie" => Ok(NamingContext {
            title: candidate["title"].as_str().map(String::from),
            year: candidate["year"].as_u64().map(|y| y as u32),
            ext,
            ..Default::default()
        }),
        "tv" => {
            let show_title = candidate["title"].as_str().map(String::from);
            let year = candidate["year"].as_u64().map(|y| y as u32);
            let (season, episode, episode_title) = if let Some(interp) = interpretation {
                (
                    interp["season"].as_u64().map(|v| v as u32),
                    interp["episodes"]
                        .as_array()
                        .and_then(|a| a.first())
                        .and_then(|v| v.as_u64())
                        .map(|v| v as u32),
                    interp["episode_title"].as_str().map(String::from),
                )
            } else {
                (None, None, None)
            };
            Ok(NamingContext {
                show_title,
                year,
                season,
                episode,
                episode_title,
                ext,
                ..Default::default()
            })
        }
        other => Err(JobError::ExecutionFailed(format!(
            "Unsupported media type for candidate context: {other}"
        ))),
    }
}

/// Fetch TV episode metadata by joining episodes -> seasons -> shows.
async fn fetch_tv_episode_meta(pool: &PgPool, episode_id: Uuid) -> Result<TvEpisodeMeta, JobError> {
    let row = sqlx::query(
        r#"
        SELECT
            s.title AS show_title,
            s.year AS show_year,
            s.content_category,
            se.season_sort_order AS season_number,
            e.episode_sort_order AS episode_number,
            e.title AS episode_title
        FROM tv_episodes e
        JOIN tv_seasons se ON se.id = e.season_id
        JOIN tv_shows s ON s.id = se.show_id
        WHERE e.id = $1
        "#,
    )
    .bind(episode_id)
    .fetch_optional(pool)
    .await?
    .ok_or_else(|| JobError::ExecutionFailed(format!("TV episode {episode_id} not found")))?;

    let mut meta = TvEpisodeMeta {
        show_title: row.get("show_title"),
        show_year: row.get("show_year"),
        season_number: row.get("season_number"),
        episode_number: row.get("episode_number"),
        episode_title: row.get("episode_title"),
        content_category: row.get("content_category"),
        round: None,
    };
    // For sport episodes, derive round from episode_sort_order (round * 1000 + offset)
    if meta.content_category.as_deref() == Some("sport") {
        meta.round = Some(meta.episode_number / 1000);
    }
    Ok(meta)
}

/// Fetch book metadata by joining with authors and series.
async fn fetch_book_meta(pool: &PgPool, book_id: Uuid) -> Result<BookMeta, JobError> {
    let row = sqlx::query(
        r#"
        SELECT
            b.title,
            (SELECT a.name FROM book_authors ba JOIN authors a ON a.id = ba.author_id
             WHERE ba.book_id = b.id ORDER BY ba.author_id LIMIT 1) AS author,
            s.name AS series_name,
            bs.series_index
        FROM books b
        LEFT JOIN book_series bs ON bs.book_id = b.id AND bs.is_primary = TRUE
        LEFT JOIN series s ON s.id = bs.series_id
        WHERE b.id = $1
        "#,
    )
    .bind(book_id)
    .fetch_optional(pool)
    .await?
    .ok_or_else(|| JobError::ExecutionFailed(format!("Book {book_id} not found")))?;

    Ok(BookMeta {
        title: row.get("title"),
        author: row.get("author"),
        series: row.get("series_name"),
        series_index: row.get("series_index"),
    })
}

/// Fetch movie metadata.
async fn fetch_movie_meta(pool: &PgPool, movie_id: Uuid) -> Result<MovieMeta, JobError> {
    let row = sqlx::query(
        r#"
        SELECT title, year
        FROM movies
        WHERE id = $1
        "#,
    )
    .bind(movie_id)
    .fetch_optional(pool)
    .await?
    .ok_or_else(|| JobError::ExecutionFailed(format!("Movie {movie_id} not found")))?;

    Ok(MovieMeta {
        title: row.get("title"),
        year: row.get("year"),
    })
}
