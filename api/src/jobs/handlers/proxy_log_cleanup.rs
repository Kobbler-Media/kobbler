//! Proxy log cleanup job handler.
//!
//! Cleans up old proxy logs and their associated blob files based on retention settings.

use crate::RuntimeConfigManager;
use crate::blob_storage::BlobStorage;
use crate::jobs::worker::JobHandler;
use crate::jobs::{JobError, JobResult};
use crate::repositories::proxy_logs;
use async_trait::async_trait;
use serde_json::{Value, json};
use sqlx::PgPool;
use tokio_util::sync::CancellationToken;
use tracing::{error, info};
use uuid::Uuid;

/// Handler for proxy log cleanup jobs.
pub struct ProxyLogCleanupHandler {
    pool: PgPool,
    blob_storage: BlobStorage,
    runtime_config: RuntimeConfigManager,
}

impl ProxyLogCleanupHandler {
    pub fn new(pool: PgPool, storage_root: &str, runtime_config: RuntimeConfigManager) -> Self {
        Self {
            pool,
            blob_storage: BlobStorage::new(storage_root),
            runtime_config,
        }
    }
}

#[async_trait]
impl JobHandler for ProxyLogCleanupHandler {
    fn job_type(&self) -> &'static str {
        "proxy_log_cleanup"
    }

    /// Only 1 cleanup job at a time.
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
        // Get current retention days from runtime config
        let retention_days = self.runtime_config.get().await.proxy_log_retention_days;

        // First, get log IDs that will be deleted (to clean up blobs)
        let old_log_ids = proxy_logs::list_old_ids(&self.pool, retention_days)
            .await
            .map_err(|e| {
                error!("Failed to query old proxy logs: {e}");
                JobError::Repository(e)
            })?;

        if old_log_ids.is_empty() {
            return Ok(json!({ "deleted": 0, "retention_days": retention_days }));
        }

        // Delete blob files first
        self.blob_storage.delete_for_logs(&old_log_ids).await;

        // Then delete log entries
        let deleted = proxy_logs::delete_older_than_days(&self.pool, retention_days)
            .await
            .map_err(|e| {
                error!("Proxy log cleanup failed: {e}");
                JobError::Repository(e)
            })?;

        if deleted > 0 {
            info!(
                "Proxy log cleanup: deleted {} old entries (retention: {} days)",
                deleted, retention_days
            );
        }

        Ok(json!({
            "deleted": deleted,
            "retention_days": retention_days
        }))
    }
}
