//! Media root enrich job handler — catch-up orchestrator.
//!
//! Finds entities in a media root that lack enrichment and enqueues individual
//! `movie_enrich`, `tv_show_enrich`, or `book_enrich` jobs for each.
//! Primary enrichment happens via per-entity jobs enqueued from `media_root_match`;
//! this handler catches anything the match phase missed.

use crate::jobs::worker::JobHandler;
use crate::jobs::{JobContext, JobResult};
use crate::repositories::media_roots;
use async_trait::async_trait;
use nats_jobs::Publisher;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sqlx::PgPool;
use tokio_util::sync::CancellationToken;
use tracing::{info, warn};
use uuid::Uuid;

/// Payload for media_root_enrich jobs.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MediaRootEnrichPayload {
    pub media_root_id: Uuid,
}

pub struct MediaRootEnrichHandler {
    pool: PgPool,
    nats: Publisher,
    _storage_root: String,
}

impl MediaRootEnrichHandler {
    pub fn new(pool: PgPool, nats: Publisher, storage_root: String) -> Self {
        Self {
            pool,
            nats,
            _storage_root: storage_root,
        }
    }
}

#[async_trait]
impl JobHandler for MediaRootEnrichHandler {
    fn job_type(&self) -> &'static str {
        "media_root_enrich"
    }

    fn max_concurrent(&self) -> usize {
        2
    }

    async fn execute(
        &self,
        _job_id: Uuid,
        payload: Value,
        cancel: CancellationToken,
    ) -> JobResult<Value> {
        let payload: MediaRootEnrichPayload = serde_json::from_value(payload)?;

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

        let mut enqueued = 0u64;

        nats_jobs::check_cancelled(&cancel)?;

        match root.media_type.as_str() {
            "tv" => {
                enqueued = self.enqueue_unenriched_tv_shows(root.id).await;
            }
            "movie" => {
                enqueued = self.enqueue_unenriched_movies(root.id).await;
            }
            "book" => {
                enqueued = self.enqueue_unenriched_books(root.id).await;

                // For book roots, enqueue KEPUB conversion after enrichment
                let display =
                    crate::jobs::fetch_job_display(&self.pool, "media_root", root.id).await;
                let context = JobContext::for_entity(
                    "media_root",
                    root.id,
                    format!("Converting books to KEPUB for '{}'", root.name),
                )
                .with_display_opt(display);
                let _ = crate::jobs::enqueue_routine(
                    &self.pool,
                    &self.nats,
                    "media_root_book_convert",
                    json!({"media_root_id": root.id}),
                    context,
                )
                .await;
            }
            "audiobook" => {
                enqueued = self.enqueue_unenriched_books(root.id).await;
            }
            _ => {}
        }

        info!(root = %root.name, enqueued, "Media root enrichment orchestration complete");

        Ok(json!({"enqueued": enqueued}))
    }
}

impl MediaRootEnrichHandler {
    /// Find TV shows in this root that lack enrichment and enqueue individual jobs.
    async fn enqueue_unenriched_tv_shows(&self, root_id: Uuid) -> u64 {
        let rows: Vec<(Uuid, String)> = sqlx::query_as(
            r#"
            SELECT id, title
            FROM tv_shows
            WHERE media_root_id = $1
              AND identifiers->>'tmdb_id' IS NOT NULL
              AND enriched_at IS NULL
            LIMIT 20
            "#,
        )
        .bind(root_id)
        .fetch_all(&self.pool)
        .await
        .unwrap_or_default();

        let mut count = 0u64;
        for (show_id, title) in rows {
            if crate::jobs::has_active_job(
                &self.pool,
                "tv_show_enrich",
                "show_id",
                &show_id.to_string(),
            )
            .await
            .unwrap_or(false)
            {
                continue;
            }
            let display = crate::jobs::fetch_job_display(&self.pool, "tv_show", show_id).await;
            let context = JobContext::for_entity(
                "tv_show",
                show_id,
                format!("Enriching metadata for '{}'", title),
            )
            .with_display_opt(display);
            match crate::jobs::enqueue(
                &self.pool,
                &self.nats,
                "tv_show_enrich",
                json!({ "show_id": show_id }),
                context,
            )
            .await
            {
                Ok(_) => count += 1,
                Err(e) => {
                    warn!(show_id = %show_id, error = %e, "Failed to enqueue tv_show_enrich");
                }
            }
        }
        count
    }

    /// Find movies in this root that lack enrichment and enqueue individual jobs.
    async fn enqueue_unenriched_movies(&self, root_id: Uuid) -> u64 {
        let rows: Vec<(Uuid, String)> = sqlx::query_as(
            r#"
            SELECT id, title
            FROM movies
            WHERE media_root_id = $1
              AND identifiers->>'tmdb_id' IS NOT NULL
              AND enriched_at IS NULL
            LIMIT 20
            "#,
        )
        .bind(root_id)
        .fetch_all(&self.pool)
        .await
        .unwrap_or_default();

        let mut count = 0u64;
        for (movie_id, title) in rows {
            if crate::jobs::has_active_job(
                &self.pool,
                "movie_enrich",
                "movie_id",
                &movie_id.to_string(),
            )
            .await
            .unwrap_or(false)
            {
                continue;
            }
            let display = crate::jobs::fetch_job_display(&self.pool, "movie", movie_id).await;
            let context = JobContext::for_entity(
                "movie",
                movie_id,
                format!("Enriching metadata for '{}'", title),
            )
            .with_display_opt(display);
            match crate::jobs::enqueue(
                &self.pool,
                &self.nats,
                "movie_enrich",
                json!({ "movie_id": movie_id }),
                context,
            )
            .await
            {
                Ok(_) => count += 1,
                Err(e) => {
                    warn!(movie_id = %movie_id, error = %e, "Failed to enqueue movie_enrich");
                }
            }
        }
        count
    }

    /// Find books in this root that lack enrichment and enqueue individual jobs.
    async fn enqueue_unenriched_books(&self, root_id: Uuid) -> u64 {
        let rows: Vec<(Uuid, String)> = sqlx::query_as(
            r#"
            SELECT id, title
            FROM books
            WHERE media_root_id = $1
              AND enriched_at IS NULL
            LIMIT 20
            "#,
        )
        .bind(root_id)
        .fetch_all(&self.pool)
        .await
        .unwrap_or_default();

        let mut count = 0u64;
        for (book_id, title) in rows {
            if crate::jobs::has_active_job(
                &self.pool,
                "book_enrich",
                "book_id",
                &book_id.to_string(),
            )
            .await
            .unwrap_or(false)
            {
                continue;
            }
            let display = crate::jobs::fetch_job_display(&self.pool, "book", book_id).await;
            let context = JobContext::for_entity(
                "book",
                book_id,
                format!("Enriching metadata for '{}'", title),
            )
            .with_display_opt(display);
            match crate::jobs::enqueue(
                &self.pool,
                &self.nats,
                "book_enrich",
                json!({ "book_id": book_id }),
                context,
            )
            .await
            {
                Ok(_) => count += 1,
                Err(e) => {
                    warn!(book_id = %book_id, error = %e, "Failed to enqueue book_enrich");
                }
            }
        }
        count
    }
}
