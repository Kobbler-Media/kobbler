//! Transcribe backfill job handler.
//!
//! Scheduled job that finds audiobooks with untranscribed or stale-config
//! files and enqueues audiobook_transcribe jobs for them, a bounded batch at a
//! time.

use crate::RuntimeConfigManager;
use crate::enrichment_config::stt_config_tag;
use crate::jobs::worker::JobHandler;
use crate::jobs::{JobError, JobResult};
use async_trait::async_trait;
use nats_jobs::Publisher;
use serde_json::{Value, json};
use sqlx::PgPool;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use tokio_util::sync::CancellationToken;
use tracing::info;
use uuid::Uuid;

/// Audio enqueued per run. The next run waits for the queue to drain, so with
/// the 30-minute schedule this bounds the GPU share a re-transcription takes.
const BATCH_AUDIO_HOURS: f64 = 24.0;

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

        let stale = sqlx::query!(
            r#"
            SELECT b.id, b.title, COALESCE(SUM(af.duration_seconds), 0) AS "seconds!"
            FROM books b
            JOIN audiobook_files af ON af.book_id = b.id
            WHERE af.available = TRUE
              AND EXISTS (
                  SELECT 1 FROM media_root_files mrf
                  WHERE (mrf.file_path = af.file_path OR mrf.file_path = af.source_path)
                    AND mrf.available = TRUE
              )
              AND NOT EXISTS (
                  SELECT 1 FROM audiobook_transcripts at
                  WHERE at.audiobook_file_id = af.id AND at.config_tag = $1
              )
            GROUP BY b.id, b.title
            ORDER BY b.id
            "#,
            current_tag,
        )
        .fetch_all(&self.pool)
        .await
        .map_err(|e| JobError::ExecutionFailed(format!("DB error: {e}")))?;

        let found = stale.len();
        let mut budget_seconds = BATCH_AUDIO_HOURS * 3600.0;
        let mut enqueued = 0usize;
        let mut skipped_failed = 0usize;

        for book in &stale {
            nats_jobs::check_cancelled(&cancel)?;
            if budget_seconds <= 0.0 {
                break;
            }
            if crate::jobs::recent_failure_count(
                &self.pool,
                "audiobook_transcribe",
                "book_id",
                &book.id.to_string(),
                24,
            )
            .await
            .unwrap_or(0)
                >= 3
            {
                skipped_failed += 1;
                continue;
            }

            let display = crate::jobs::fetch_job_display(&self.pool, "book", book.id).await;
            let context = crate::jobs::JobContext::for_entity(
                "book",
                book.id,
                format!("Transcribe backfill — {}", book.title),
            )
            .with_display_opt(display);
            match crate::jobs::enqueue_routine(
                &self.pool,
                &self.publisher,
                "audiobook_transcribe",
                json!({"book_id": book.id}),
                context,
            )
            .await
            {
                Ok(_) => {
                    enqueued += 1;
                    budget_seconds -= book.seconds;
                }
                Err(e) => {
                    tracing::warn!(error = %e, book_id = %book.id, "Failed to enqueue transcribe backfill");
                }
            }
        }

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
