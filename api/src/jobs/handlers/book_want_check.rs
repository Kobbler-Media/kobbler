//! Daily re-send of every wanted book through `book_want`, the one path a
//! book's Kobload patterns are written by. It refreshes what each pattern
//! carries (alternate titles), recreates a pattern Kobload has lost, and
//! removes a wanted book's patterns under keys its format no longer uses.

use std::collections::{HashMap, HashSet};

use crate::jobs::worker::JobHandler;
use crate::jobs::{JobError, JobResult};
use crate::kobload::KobloadClient;
use crate::repositories::books;
use crate::runtime_config::RuntimeConfigManager;
use crate::wanted::SOURCE_SYSTEM;
use crate::wanted::book_source;
use async_trait::async_trait;
use nats_jobs::{JobContext, Publisher};
use serde_json::{Value, json};
use sqlx::PgPool;
use tokio_util::sync::CancellationToken;
use tracing::{info, warn};
use uuid::Uuid;

pub struct BookWantCheckHandler {
    pool: PgPool,
    runtime_config: RuntimeConfigManager,
    nats_publisher: Publisher,
}

impl BookWantCheckHandler {
    pub fn new(
        pool: PgPool,
        runtime_config: RuntimeConfigManager,
        nats_publisher: Publisher,
    ) -> Self {
        Self {
            pool,
            runtime_config,
            nats_publisher,
        }
    }

    /// `None` when Kobload cannot be listed.
    async fn stale_sources(&self, wanted: &HashMap<Uuid, String>) -> Option<Vec<String>> {
        let config = self.runtime_config.get().await;
        let (Some(base_url), Some(api_key)) = (
            config.kobload_base_url.as_deref().filter(|s| !s.is_empty()),
            config.kobload_api_key.as_deref().filter(|s| !s.is_empty()),
        ) else {
            return None;
        };
        let client = KobloadClient::new(base_url.to_string(), api_key.to_string());
        let patterns = match client.list_watch_patterns(Some(SOURCE_SYSTEM)).await {
            Ok(p) => p,
            Err(e) => {
                warn!(error = %e, "Failed to list Kobload patterns for the book want check");
                return None;
            }
        };
        Some(
            patterns
                .into_iter()
                .filter_map(|p| {
                    let book_id = book_source::entity_id(&p.source_id)?;
                    let want_format = wanted.get(&book_id)?;
                    let current: HashSet<String> = book_source::want_sources(book_id, want_format)
                        .into_iter()
                        .map(|(_, id)| id)
                        .collect();
                    (!current.contains(&p.source_id)).then_some(p.source_id)
                })
                .collect(),
        )
    }
}

#[async_trait]
impl JobHandler for BookWantCheckHandler {
    fn job_type(&self) -> &'static str {
        "book_want_check"
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
        info!(job_id = %job_id, "Running book want check");

        let wanted: HashMap<Uuid, String> = books::list_syncable_wanted(&self.pool)
            .await
            .map_err(|e| JobError::ExecutionFailed(format!("Failed to query wanted books: {e}")))?
            .into_iter()
            .map(|row| (row.id, row.want_format))
            .collect();

        let mut stale_removed = 0;
        for source_id in self.stale_sources(&wanted).await.unwrap_or_default() {
            let delete = nats_jobs::WatchPatternDelete {
                source_system: SOURCE_SYSTEM.to_string(),
                source_id: source_id.clone(),
            };
            match self
                .nats_publisher
                .enqueue_job("kobload", "watch_pattern_delete", json!(delete))
                .await
            {
                Ok(_) => stale_removed += 1,
                Err(e) => warn!(source_id, error = %e, "Failed to remove a stale book pattern"),
            }
        }

        let mut enqueued = 0;
        let mut already_queued = 0;
        for book_id in wanted.keys() {
            nats_jobs::check_cancelled(&cancel)?;
            if crate::jobs::has_active_job(&self.pool, "book_want", "book_id", &book_id.to_string())
                .await
                .unwrap_or(false)
            {
                already_queued += 1;
                continue;
            }
            match crate::jobs::enqueue_routine(
                &self.pool,
                &self.nats_publisher,
                "book_want",
                json!({ "book_id": book_id }),
                JobContext::for_entity("book", *book_id, "Re-checking wanted book".to_string()),
            )
            .await
            {
                Ok(_) => enqueued += 1,
                Err(e) => {
                    warn!(book_id = %book_id, error = %e, "Failed to enqueue book_want re-check")
                }
            }
        }

        info!(
            total = wanted.len(),
            enqueued, already_queued, stale_removed, "Book want check complete"
        );
        Ok(json!({
            "books_checked": wanted.len(),
            "enqueued": enqueued,
            "already_queued": already_queued,
            "stale_patterns_removed": stale_removed,
        }))
    }
}
