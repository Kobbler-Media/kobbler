//! Transcribe backfill job handler.
//!
//! Scheduled job that finds audiobooks with untranscribed or stale-config
//! files and enqueues audiobook_transcribe jobs for them.

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

        let config = self.runtime_config.get().await;
        let current_tag = stt_config_tag(&config.stt_service_url);

        // Find books with available audiobook files that either:
        // - have no transcript at all (NULL config_tag)
        // - have transcripts from a different STT config (stale config_tag)
        let books: Vec<(Uuid, String)> = sqlx::query_as(
            r#"
            SELECT DISTINCT b.id, b.title
            FROM books b
            JOIN audiobook_files af ON af.book_id = b.id
            JOIN media_root_files mrf
                ON (mrf.file_path = af.file_path OR mrf.file_path = af.source_path)
                AND mrf.available = TRUE
            LEFT JOIN audiobook_transcripts at
                ON at.audiobook_file_id = af.id AND at.config_tag = $1
            WHERE at.id IS NULL AND af.available = TRUE
            "#,
        )
        .bind(&current_tag)
        .fetch_all(&self.pool)
        .await
        .map_err(|e| JobError::ExecutionFailed(format!("DB error: {e}")))?;

        let found = books.len();
        let mut enqueued = 0usize;
        let mut skipped_active = 0usize;
        let mut skipped_failed = 0usize;

        for (book_id, title) in &books {
            nats_jobs::check_cancelled(&cancel)?;
            if let Ok(true) = crate::jobs::has_active_job(
                &self.pool,
                "audiobook_transcribe",
                "book_id",
                &book_id.to_string(),
            )
            .await
            {
                skipped_active += 1;
                continue;
            }

            if crate::jobs::recent_failure_count(
                &self.pool,
                "audiobook_transcribe",
                "book_id",
                &book_id.to_string(),
                24,
            )
            .await
            .unwrap_or(0)
                >= 3
            {
                skipped_failed += 1;
                continue;
            }

            let display = crate::jobs::fetch_job_display(&self.pool, "book", *book_id).await;
            let context = crate::jobs::JobContext::for_entity(
                "book",
                *book_id,
                format!("Transcribe backfill — {title}"),
            )
            .with_display_opt(display);
            match crate::jobs::enqueue_routine(
                &self.pool,
                &self.publisher,
                "audiobook_transcribe",
                json!({"book_id": book_id}),
                context,
            )
            .await
            {
                Ok(_) => enqueued += 1,
                Err(e) => {
                    tracing::warn!(error = %e, book_id = %book_id, "Failed to enqueue transcribe backfill");
                }
            }
        }

        info!(
            found,
            enqueued, skipped_active, skipped_failed, "Transcribe backfill check complete"
        );

        Ok(json!({
            "books_found": found,
            "enqueued": enqueued,
            "skipped_already_active": skipped_active,
            "skipped_failed": skipped_failed,
        }))
    }
}
