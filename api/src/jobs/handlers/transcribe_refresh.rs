//! User-started re-transcription of every audiobook file without a transcript
//! from the current STT config, a day of audio at a time so the GPU keeps up.

use super::transcribe_batch::enqueue_batch;
use crate::RuntimeConfigManager;
use crate::enrichment_config::stt_config_tag;
use crate::jobs::worker::JobHandler;
use crate::jobs::{JobError, JobEvent, JobResult, update_progress};
use crate::repositories::audiobook_transcripts;
use async_trait::async_trait;
use nats_jobs::Publisher;
use serde_json::{Value, json};
use sqlx::PgPool;
use std::time::Duration;
use tokio::sync::broadcast;
use tokio_util::sync::CancellationToken;
use tracing::info;
use uuid::Uuid;

fn db_error(e: impl std::fmt::Display) -> JobError {
    JobError::ExecutionFailed(format!("DB error: {e}"))
}

const DRAIN_POLL: Duration = Duration::from_secs(15);

pub struct TranscribeRefreshHandler {
    pool: PgPool,
    publisher: Publisher,
    runtime_config: RuntimeConfigManager,
    job_events: broadcast::Sender<JobEvent>,
}

impl TranscribeRefreshHandler {
    pub fn new(
        pool: PgPool,
        publisher: Publisher,
        runtime_config: RuntimeConfigManager,
        job_events: broadcast::Sender<JobEvent>,
    ) -> Self {
        Self {
            pool,
            publisher,
            runtime_config,
            job_events,
        }
    }

    async fn stop(&self, cancel: &CancellationToken) -> JobResult<Value> {
        crate::jobs::cancel_pending_jobs_with_field(
            &self.pool,
            "audiobook_transcribe",
            "outdated",
            "true",
            "book_id",
        )
        .await?;
        nats_jobs::check_cancelled(cancel)?;
        unreachable!("stop is only reached once cancelled")
    }

    async fn drained(&self, cancel: &CancellationToken) -> JobResult<bool> {
        while crate::jobs::has_active_jobs_of_type(&self.pool, "audiobook_transcribe").await? {
            tokio::select! {
                _ = cancel.cancelled() => return Ok(false),
                _ = tokio::time::sleep(DRAIN_POLL) => {}
            }
        }
        Ok(!cancel.is_cancelled())
    }
}

#[async_trait]
impl JobHandler for TranscribeRefreshHandler {
    fn job_type(&self) -> &'static str {
        "transcribe_refresh"
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
        let tag = stt_config_tag(&self.runtime_config.get().await);
        let total_seconds: f64 = audiobook_transcripts::stale_books(&self.pool, &tag, true)
            .await
            .map_err(db_error)?
            .iter()
            .map(|b| b.seconds)
            .sum();
        let mut batches = 0u32;
        let mut remaining_seconds = total_seconds;

        loop {
            if !self.drained(&cancel).await? {
                return self.stop(&cancel).await;
            }
            let stale = audiobook_transcripts::stale_books(&self.pool, &tag, true)
                .await
                .map_err(db_error)?;
            let batch =
                match enqueue_batch(&self.pool, &self.publisher, &stale, true, &cancel).await {
                    Ok(batch) => batch,
                    Err(_) if cancel.is_cancelled() => return self.stop(&cancel).await,
                    Err(e) => return Err(e),
                };
            if batch.enqueued == 0 {
                break;
            }
            batches += 1;
            remaining_seconds = stale.iter().map(|b| b.seconds).sum::<f64>() - batch.seconds;
            let done = (total_seconds - remaining_seconds) / total_seconds.max(1.0);
            let _ = update_progress(
                &self.pool,
                &self.job_events,
                job_id,
                "transcribe_refresh",
                (done * 100.0).clamp(0.0, 99.0) as i32,
                Some(json!({
                    "batches": batches,
                    "remaining_hours": remaining_seconds / 3600.0,
                })),
            )
            .await;
            info!(
                batches,
                enqueued = batch.enqueued,
                "Queued a re-transcription batch"
            );
        }

        Ok(json!({
            "batches": batches,
            "total_hours": total_seconds / 3600.0,
            "remaining_hours": remaining_seconds.max(0.0) / 3600.0,
        }))
    }
}
