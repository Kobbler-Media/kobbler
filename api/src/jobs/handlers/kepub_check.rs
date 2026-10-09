//! KEPUB catch-up for books a Kobo can see.
//!
//! The Kobo prefers KEPUB over EPUB (`kobo::metadata` only offers one format,
//! and picks KEPUB when the book has one), so a synced book without a KEPUB is
//! reading a plain EPUB: no koboSpan progress anchors, and none of the
//! kepubifier's transforms.
//!
//! Conversion used to hang off the tail of the media-root ingest chain
//! (`media_root_scan` → … → `media_root_enrich` → `media_root_book_convert`),
//! which only runs when a scan turns up *pending* files. A library that has
//! settled has no pending files, so that chain never fires again and every
//! book already on the shelf stays unconverted for good. This job is the part
//! that does not depend on a file having just arrived: it asks the only
//! question that matters — is this book entitled to a Kobo, and does it have a
//! KEPUB? — and fixes the answer.
//!
//! Cheap to run: one indexed query, and nothing to do once the library has
//! caught up.

use crate::jobs::worker::JobHandler;
use crate::jobs::{JobContext, JobResult};
use crate::repositories::entitlements;
use async_trait::async_trait;
use nats_jobs::Publisher;
use serde_json::{Value, json};
use sqlx::PgPool;
use tokio_util::sync::CancellationToken;
use tracing::{info, warn};
use uuid::Uuid;

/// Ceiling on how many conversions one pass will queue.
///
/// A first run over a library that has never converted anything could other-
/// wise queue hundreds at once and crowd out everything else on the worker.
/// The job runs on a short interval, so the rest arrive on the next pass.
const MAX_PER_PASS: usize = 25;

pub struct KepubCheckHandler {
    pool: PgPool,
    nats: Publisher,
}

impl KepubCheckHandler {
    pub fn new(pool: PgPool, nats: Publisher) -> Self {
        Self { pool, nats }
    }
}

#[async_trait]
impl JobHandler for KepubCheckHandler {
    fn job_type(&self) -> &'static str {
        "kepub_check"
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
        nats_jobs::check_cancelled(&cancel)?;

        let pending = entitlements::list_synced_without_kepub(&self.pool).await?;
        let outstanding = pending.len();
        if outstanding == 0 {
            return Ok(json!({ "queued": 0, "outstanding": 0 }));
        }

        let mut queued = 0u64;
        for (book_id, title) in pending.into_iter().take(MAX_PER_PASS) {
            nats_jobs::check_cancelled(&cancel)?;

            // A book whose conversion keeps failing must not be retried every
            // few minutes forever — it would bury the job list and pin a
            // worker on a file that is never going to convert.
            let recent_failures = crate::jobs::recent_failure_count(
                &self.pool,
                "format_conversion",
                "book_id",
                &book_id.to_string(),
                24,
            )
            .await
            .unwrap_or(0);
            if recent_failures >= 3 {
                continue;
            }

            let display = crate::jobs::fetch_job_display(&self.pool, "book", book_id).await;
            let context =
                JobContext::for_entity("book", book_id, format!("Building KEPUB — {title}"))
                    .with_display_opt(display);

            if let Err(e) = crate::jobs::enqueue(
                &self.pool,
                &self.nats,
                "format_conversion",
                json!({ "book_id": book_id, "target_format": "KEPUB" }),
                context,
            )
            .await
            {
                warn!(error = %e, book_id = %book_id, "Failed to enqueue KEPUB conversion");
                continue;
            }
            queued += 1;
        }

        if queued > 0 {
            info!(
                queued,
                outstanding, "Queued KEPUB conversions for synced books"
            );
        }

        Ok(json!({ "queued": queued, "outstanding": outstanding }))
    }
}
