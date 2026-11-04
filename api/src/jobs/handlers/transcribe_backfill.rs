//! Transcribe backfill job handler.
//!
//! Scheduled job that finds audiobooks with untranscribed files and enqueues
//! audiobook_transcribe jobs for them, a bounded batch at a time. Files whose
//! transcript is merely outdated wait for a user-started `transcribe_refresh`.

use super::transcribe_batch::enqueue_batch;
use crate::RuntimeConfigManager;
use crate::enrichment_config::stt_config_tag;
use crate::jobs::worker::JobHandler;
use crate::jobs::{JobError, JobResult};
use crate::repositories::audiobook_transcripts;
use async_trait::async_trait;
use nats_jobs::Publisher;
use serde_json::{Value, json};
use sqlx::PgPool;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use tokio_util::sync::CancellationToken;
use tracing::info;
use uuid::Uuid;

pub struct TranscribeBackfillHandler {
    pool: PgPool,
    publisher: Publisher,
    runtime_config: RuntimeConfigManager,
    initial_scan_complete: Arc<AtomicBool>,
}

impl TranscribeBackfillHandler {
    pub fn new(
        pool: PgPool,
        publisher: Publisher,
        runtime_config: RuntimeConfigManager,
        initial_scan_complete: Arc<AtomicBool>,
    ) -> Self {
        Self {
            pool,
            publisher,
            runtime_config,
            initial_scan_complete,
        }
    }
}

#[async_trait]
impl JobHandler for TranscribeBackfillHandler {
    fn job_type(&self) -> &'static str {
        "transcribe_backfill"
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
            info!("Skipping transcribe backfill — initial media root scan not yet complete");
            return Ok(json!({"status": "skipped", "reason": "initial_scan_pending"}));
        }

        if crate::jobs::has_active_jobs_of_type(&self.pool, "audiobook_transcribe").await? {
            info!("Transcribe backfill waiting for queued transcriptions to finish");
            return Ok(json!({"status": "queue_busy", "enqueued": 0}));
        }

        let config = self.runtime_config.get().await;
        let current_tag = stt_config_tag(&config);

        let stale = audiobook_transcripts::stale_books(&self.pool, &current_tag, false)
            .await
            .map_err(|e| JobError::ExecutionFailed(format!("DB error: {e}")))?;

        let found = stale.len();
        let batch = enqueue_batch(&self.pool, &self.publisher, &stale, false, &cancel).await?;
        let (enqueued, skipped_failed) = (batch.enqueued, batch.skipped_failed);

        info!(
            found,
            enqueued, skipped_failed, "Transcribe backfill check complete"
        );

        Ok(json!({
            "books_found": found,
            "books_remaining": found - enqueued,
            "enqueued": enqueued,
            "skipped_failed": skipped_failed,
        }))
    }
}
