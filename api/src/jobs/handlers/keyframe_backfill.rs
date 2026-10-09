//! Media backfill job handler.
//!
//! Scheduled job that finds video files needing work and routes them correctly:
//! - Unprobed files (no metadata) → media_probe (which chains to media_extract)
//! - Already-probed files missing keyframes → media_extract directly

use crate::jobs::worker::JobHandler;
use crate::jobs::{JobError, JobResult};
use crate::repositories::{movie_files, tv_episode_files};
use async_trait::async_trait;
use nats_jobs::Publisher;
use serde_json::{Value, json};
use sqlx::PgPool;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use tokio_util::sync::CancellationToken;
use tracing::info;
use uuid::Uuid;

use super::{MediaExtractPayload, MediaProbePayload};

pub struct KeyframeBackfillHandler {
    pool: PgPool,
    publisher: Publisher,
    initial_scan_complete: Arc<AtomicBool>,
}

impl KeyframeBackfillHandler {
    pub fn new(pool: PgPool, publisher: Publisher, initial_scan_complete: Arc<AtomicBool>) -> Self {
        Self {
            pool,
            publisher,
            initial_scan_complete,
        }
    }
}

#[async_trait]
impl JobHandler for KeyframeBackfillHandler {
    fn job_type(&self) -> &'static str {
        "keyframe_backfill"
    }

    fn max_concurrent(&self) -> usize {
        1
    }

    async fn execute(
        &self,
        _job_id: Uuid,
        _payload: Value,
        cancel: CancellationToken,
    ) -> JobResult<Value> {
        if !self.initial_scan_complete.load(Ordering::Acquire) {
            info!("Skipping keyframe backfill — initial media root scan not yet complete");
            return Ok(json!({"status": "skipped", "reason": "initial_scan_pending"}));
        }

        let mut enqueued = 0usize;
        let mut skipped_active = 0usize;
        let mut skipped_failed = 0usize;

        // Unprobed files need full media_probe (fast metadata, then chains to media_extract)
        let unprobed_movies = movie_files::list_needing_probe(&self.pool)
            .await
            .map_err(|e| JobError::ExecutionFailed(format!("DB error: {e}")))?;
        let unprobed_episodes = tv_episode_files::list_needing_probe(&self.pool)
            .await
            .map_err(|e| JobError::ExecutionFailed(format!("DB error: {e}")))?;

        for file_id in &unprobed_movies {
            nats_jobs::check_cancelled(&cancel)?;
            let (e, sa, sf) = enqueue_probe(&self.pool, &self.publisher, *file_id, "movie").await;
            enqueued += e;
            skipped_active += sa;
            skipped_failed += sf;
        }
        for file_id in &unprobed_episodes {
            let (e, sa, sf) = enqueue_probe(&self.pool, &self.publisher, *file_id, "episode").await;
            enqueued += e;
            skipped_active += sa;
            skipped_failed += sf;
        }

        // Already-probed files missing keyframes go straight to media_extract
        let extract_movies = movie_files::list_needing_extract(&self.pool)
            .await
            .map_err(|e| JobError::ExecutionFailed(format!("DB error: {e}")))?;
        let extract_episodes = tv_episode_files::list_needing_extract(&self.pool)
            .await
            .map_err(|e| JobError::ExecutionFailed(format!("DB error: {e}")))?;

        for file_id in &extract_movies {
            let (e, sa, sf) = enqueue_extract(&self.pool, &self.publisher, *file_id, "movie").await;
            enqueued += e;
            skipped_active += sa;
            skipped_failed += sf;
        }
        for file_id in &extract_episodes {
            let (e, sa, sf) =
                enqueue_extract(&self.pool, &self.publisher, *file_id, "episode").await;
            enqueued += e;
            skipped_active += sa;
            skipped_failed += sf;
        }

        let probe_found = unprobed_movies.len() + unprobed_episodes.len();
        let extract_found = extract_movies.len() + extract_episodes.len();

        info!(
            unprobed = probe_found,
            missing_keyframes = extract_found,
            enqueued,
            skipped_active,
            skipped_failed,
            "Media backfill check complete"
        );

        Ok(json!({
            "unprobed_found": probe_found,
            "missing_keyframes_found": extract_found,
            "enqueued": enqueued,
            "skipped_already_active": skipped_active,
            "skipped_failed": skipped_failed,
        }))
    }
}

/// Returns (enqueued, skipped_active, skipped_failed).
async fn enqueue_probe(
    pool: &PgPool,
    publisher: &Publisher,
    file_id: Uuid,
    media_type: &str,
) -> (usize, usize, usize) {
    if let Ok(true) =
        crate::jobs::has_active_job(pool, "media_probe", "file_id", &file_id.to_string()).await
    {
        return (0, 1, 0);
    }
    if crate::jobs::recent_failure_count(pool, "media_probe", "file_id", &file_id.to_string(), 24)
        .await
        .unwrap_or(0)
        >= 3
    {
        return (0, 0, 1);
    }
    let payload = MediaProbePayload {
        file_id,
        media_type: media_type.to_string(),
    };
    let context = crate::jobs::JobContext::new("Media backfill — probe");
    match crate::jobs::enqueue_routine(
        pool,
        publisher,
        "media_probe",
        serde_json::to_value(&payload).unwrap(),
        context,
    )
    .await
    {
        Ok(_) => (1, 0, 0),
        Err(e) => {
            tracing::warn!(error = %e, file_id = %file_id, "Failed to enqueue media_probe backfill");
            (0, 0, 0)
        }
    }
}

/// Returns (enqueued, skipped_active, skipped_failed).
async fn enqueue_extract(
    pool: &PgPool,
    publisher: &Publisher,
    file_id: Uuid,
    media_type: &str,
) -> (usize, usize, usize) {
    if let Ok(true) =
        crate::jobs::has_active_job(pool, "media_extract", "file_id", &file_id.to_string()).await
    {
        return (0, 1, 0);
    }
    if crate::jobs::recent_failure_count(pool, "media_extract", "file_id", &file_id.to_string(), 24)
        .await
        .unwrap_or(0)
        >= 3
    {
        return (0, 0, 1);
    }
    let payload = MediaExtractPayload {
        file_id,
        media_type: media_type.to_string(),
    };
    let context = crate::jobs::JobContext::new("Media backfill — extract");
    match crate::jobs::enqueue_routine(
        pool,
        publisher,
        "media_extract",
        serde_json::to_value(&payload).unwrap(),
        context,
    )
    .await
    {
        Ok(_) => (1, 0, 0),
        Err(e) => {
            tracing::warn!(error = %e, file_id = %file_id, "Failed to enqueue media_extract backfill");
            (0, 0, 0)
        }
    }
}
