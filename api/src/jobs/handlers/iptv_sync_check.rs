//! IPTV sync check job handler.
//!
//! Scheduled job (every 5 min) that checks all providers and enqueues
//! `iptv_sync` jobs for any that are overdue for a sync.

use crate::jobs::worker::JobHandler;
use crate::jobs::{JobError, JobResult};
use crate::repositories::iptv_providers;
use async_trait::async_trait;
use nats_jobs::Publisher;
use serde_json::{Value, json};
use sqlx::PgPool;
use tokio_util::sync::CancellationToken;
use tracing::{debug, info, warn};
use uuid::Uuid;

/// Handler for periodic IPTV provider sync check.
pub struct IptvSyncCheckHandler {
    pool: PgPool,
    nats_publisher: Publisher,
}

impl IptvSyncCheckHandler {
    pub fn new(pool: PgPool, nats_publisher: Publisher) -> Self {
        Self {
            pool,
            nats_publisher,
        }
    }
}

#[async_trait]
impl JobHandler for IptvSyncCheckHandler {
    fn job_type(&self) -> &'static str {
        "iptv_sync_check"
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
        nats_jobs::check_cancelled(&cancel)?;
        // Debug, not info: fires every five minutes and reports only that it
        // fired. The "complete" line below is already gated on there being
        // providers due, so an idle check now says nothing at all.
        debug!(job_id = %job_id, "Running IPTV sync check");

        let due_providers = iptv_providers::list_due_for_sync(&self.pool)
            .await
            .map_err(|e| {
                JobError::ExecutionFailed(format!("Failed to query due providers: {e}"))
            })?;

        if due_providers.is_empty() {
            return Ok(json!({ "providers_checked": 0, "enqueued": 0 }));
        }

        let mut enqueued = 0;
        for provider in &due_providers {
            if let Err(e) = self
                .nats_publisher
                .enqueue_job("kobbler", "iptv_sync", json!({"provider_id": provider.id}))
                .await
            {
                warn!(
                    provider_id = %provider.id,
                    provider_name = %provider.name,
                    error = %e,
                    "Failed to enqueue iptv_sync job"
                );
            } else {
                enqueued += 1;
            }
        }

        info!(
            total = due_providers.len(),
            enqueued, "IPTV sync check complete"
        );
        Ok(json!({
            "providers_checked": due_providers.len(),
            "enqueued": enqueued,
        }))
    }
}
