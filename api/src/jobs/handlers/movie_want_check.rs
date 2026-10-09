//! Movie want check job handler - periodic re-check of wanted movies.
//!
//! Daily scheduled job that re-enqueues `movie_want` for movies still in
//! `want_status = 'wanted'` state. The timing guard in `MovieWantHandler`
//! decides whether to proceed or skip based on release date proximity.

use crate::jobs::worker::JobHandler;
use crate::jobs::{JobError, JobResult};
use async_trait::async_trait;
use nats_jobs::Publisher;
use serde_json::{Value, json};
use sqlx::PgPool;
use tokio_util::sync::CancellationToken;
use tracing::info;
use uuid::Uuid;

/// Handler for periodic re-check of wanted movies.
pub struct MovieWantCheckHandler {
    pool: PgPool,
    nats_publisher: Publisher,
}

impl MovieWantCheckHandler {
    pub fn new(pool: PgPool, nats_publisher: Publisher) -> Self {
        Self {
            pool,
            nats_publisher,
        }
    }
}

#[async_trait]
impl JobHandler for MovieWantCheckHandler {
    fn job_type(&self) -> &'static str {
        "movie_want_check"
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
        info!(job_id = %job_id, "Running movie want check");

        let rows = sqlx::query_scalar!(
            "SELECT id FROM movies WHERE want_status = 'wanted' AND NOT EXISTS (SELECT 1 FROM movie_files mf WHERE mf.movie_id = movies.id)"
        )
        .fetch_all(&self.pool)
        .await
        .map_err(|e| JobError::ExecutionFailed(format!("Failed to query wanted movies: {e}")))?;

        if rows.is_empty() {
            info!("No wanted movies to re-check");
            return Ok(json!({ "movies_checked": 0 }));
        }

        let mut enqueued = 0;
        for movie_id in &rows {
            nats_jobs::check_cancelled(&cancel)?;
            if let Err(e) = self
                .nats_publisher
                .enqueue_job("kobbler", "movie_want", json!({"movie_id": movie_id}))
                .await
            {
                info!(movie_id = %movie_id, error = %e, "Failed to enqueue movie_want re-check");
            } else {
                enqueued += 1;
            }
        }

        info!(total = rows.len(), enqueued, "Movie want check complete");
        Ok(json!({ "movies_checked": rows.len(), "enqueued": enqueued }))
    }
}
