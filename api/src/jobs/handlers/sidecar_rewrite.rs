//! One-shot sidecar rewrite job.
//!
//! Regenerates ALL sidecar files across the library. For TV shows, this enqueues
//! `tv_show_enrich` jobs (which populate episode-level identifiers and then write
//! sidecars). For movies and books, sidecars are written directly.

use crate::jobs::worker::JobHandler;
use crate::jobs::{self, JobError, JobResult};
use crate::sidecar::write_back;
use async_trait::async_trait;
use nats_jobs::Publisher;
use serde_json::{Value, json};
use sqlx::PgPool;
use tokio::sync::broadcast;
use tokio_util::sync::CancellationToken;
use tracing::{info, warn};
use uuid::Uuid;

pub struct SidecarRewriteHandler {
    pool: PgPool,
    nats: Publisher,
    job_events: broadcast::Sender<jobs::JobEvent>,
}

impl SidecarRewriteHandler {
    pub fn new(
        pool: PgPool,
        nats: Publisher,
        job_events: broadcast::Sender<jobs::JobEvent>,
    ) -> Self {
        Self {
            pool,
            nats,
            job_events,
        }
    }
}

#[async_trait]
impl JobHandler for SidecarRewriteHandler {
    fn job_type(&self) -> &'static str {
        "sidecar_rewrite"
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
        info!(job_id = %job_id, "Starting full sidecar rewrite");

        // ── Phase 1: Enqueue tv_show_enrich for every TV show ──
        // This populates episode-level identifiers AND writes all TV sidecars.
        let show_ids: Vec<Uuid> = sqlx::query_scalar("SELECT id FROM tv_shows")
            .fetch_all(&self.pool)
            .await
            .map_err(JobError::Database)?;

        let mut tv_enqueued = 0u64;
        for show_id in &show_ids {
            let display = crate::jobs::fetch_job_display(&self.pool, "tv_show", *show_id).await;
            let context = nats_jobs::JobContext::for_entity(
                "tv_show",
                *show_id,
                "Re-enriching TV show for sidecar rewrite",
            )
            .with_display_opt(display);
            match jobs::enqueue(
                &self.pool,
                &self.nats,
                "tv_show_enrich",
                json!({"show_id": show_id}),
                context,
            )
            .await
            {
                Ok(_) => tv_enqueued += 1,
                Err(e) => warn!(show_id = %show_id, error = %e, "Failed to enqueue tv_show_enrich"),
            }
        }

        info!(tv_enqueued, "TV show enrich jobs enqueued");

        // ── Phase 2: Rewrite movie sidecars directly ──
        let movie_ids: Vec<Uuid> = sqlx::query_scalar("SELECT id FROM movies")
            .fetch_all(&self.pool)
            .await
            .map_err(JobError::Database)?;

        let total_movies = movie_ids.len() as u64;
        let mut movies_written = 0u64;
        for (i, movie_id) in movie_ids.iter().enumerate() {
            nats_jobs::check_cancelled(&cancel)?;
            write_back::write_movie_sidecars(&self.pool, *movie_id).await;
            movies_written += 1;

            // Report progress: TV phase is "done" (enqueued), movies are the active phase
            if i % 50 == 0 {
                let progress = ((i as f64 / total_movies.max(1) as f64) * 50.0) as i32 + 25;
                let _ = jobs::update_progress(
                    &self.pool,
                    &self.job_events,
                    job_id,
                    "sidecar_rewrite",
                    progress.min(74),
                    None,
                )
                .await;
            }
        }

        info!(movies_written, "Movie sidecars rewritten");

        // ── Phase 3: Rewrite book sidecars directly ──
        let book_ids: Vec<Uuid> = sqlx::query_scalar("SELECT id FROM books")
            .fetch_all(&self.pool)
            .await
            .map_err(JobError::Database)?;

        let total_books = book_ids.len() as u64;
        let mut books_written = 0u64;
        for (i, book_id) in book_ids.iter().enumerate() {
            write_back::write_book_sidecars(&self.pool, *book_id).await;
            books_written += 1;

            if i % 50 == 0 {
                let progress = ((i as f64 / total_books.max(1) as f64) * 25.0) as i32 + 75;
                let _ = jobs::update_progress(
                    &self.pool,
                    &self.job_events,
                    job_id,
                    "sidecar_rewrite",
                    progress.min(99),
                    None,
                )
                .await;
            }
        }

        info!(books_written, "Book sidecars rewritten");

        let _ = jobs::update_progress(
            &self.pool,
            &self.job_events,
            job_id,
            "sidecar_rewrite",
            100,
            None,
        )
        .await;

        Ok(json!({
            "tv_shows_enqueued": tv_enqueued,
            "movies_written": movies_written,
            "books_written": books_written,
        }))
    }
}
