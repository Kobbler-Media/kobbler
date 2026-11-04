use crate::jobs::{JobContext, JobResult};
use crate::repositories::audiobook_transcripts::StaleBook;
use nats_jobs::Publisher;
use serde_json::json;
use sqlx::PgPool;
use tokio_util::sync::CancellationToken;

/// Audio enqueued per batch. Each batch waits for the previous one to drain,
/// which bounds the GPU share a re-transcription takes.
pub const BATCH_AUDIO_HOURS: f64 = 24.0;

#[derive(Debug, Default)]
pub struct Batch {
    pub enqueued: usize,
    pub skipped_failed: usize,
    pub seconds: f64,
}

pub async fn enqueue_batch(
    pool: &PgPool,
    publisher: &Publisher,
    books: &[StaleBook],
    outdated: bool,
    cancel: &CancellationToken,
) -> JobResult<Batch> {
    let mut budget_seconds = BATCH_AUDIO_HOURS * 3600.0;
    let mut batch = Batch::default();
    let label = if outdated {
        "Re-transcribe"
    } else {
        "Transcribe backfill"
    };

    for book in books {
        nats_jobs::check_cancelled(cancel)?;
        if budget_seconds <= 0.0 {
            break;
        }
        if crate::jobs::recent_failure_count(
            pool,
            "audiobook_transcribe",
            "book_id",
            &book.id.to_string(),
            24,
        )
        .await
        .unwrap_or(0)
            >= 3
        {
            batch.skipped_failed += 1;
            continue;
        }

        let display = crate::jobs::fetch_job_display(pool, "book", book.id).await;
        let context = JobContext::for_entity("book", book.id, format!("{label} — {}", book.title))
            .with_display_opt(display);
        match crate::jobs::enqueue_routine(
            pool,
            publisher,
            "audiobook_transcribe",
            json!({"book_id": book.id, "outdated": outdated}),
            context,
        )
        .await
        {
            Ok(_) => {
                batch.enqueued += 1;
                batch.seconds += book.seconds;
                budget_seconds -= book.seconds;
            }
            Err(e) => {
                tracing::warn!(error = %e, book_id = %book.id, "Failed to enqueue transcription");
            }
        }
    }
    Ok(batch)
}
