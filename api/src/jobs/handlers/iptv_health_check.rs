//! IPTV health check job handler.
//!
//! Scheduled job that passively recovers health scores for sources that
//! haven't been checked recently. Does NOT actively probe sources — health
//! scores are updated during playback (passive tracking).

use crate::iptv::health;
use crate::jobs::worker::JobHandler;
use crate::jobs::{JobError, JobResult};
use async_trait::async_trait;
use serde_json::{Value, json};
use sqlx::PgPool;
use tokio_util::sync::CancellationToken;
use tracing::info;
use uuid::Uuid;

/// Handler for periodic IPTV source health recovery.
pub struct IptvHealthCheckHandler {
    pool: PgPool,
    runtime_config: crate::RuntimeConfigManager,
}

impl IptvHealthCheckHandler {
    pub fn new(pool: PgPool, runtime_config: crate::RuntimeConfigManager) -> Self {
        Self {
            pool,
            runtime_config,
        }
    }
}

#[async_trait]
impl JobHandler for IptvHealthCheckHandler {
    fn job_type(&self) -> &'static str {
        "iptv_health_check"
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
        let config = self.runtime_config.get().await;
        if !config.iptv_health_check_enabled {
            return Ok(json!({ "skipped": true }));
        }

        let recovered = health::passive_recovery(&self.pool)
            .await
            .map_err(|e| JobError::ExecutionFailed(format!("Health recovery failed: {e}")))?;

        if recovered > 0 {
            info!(job_id = %job_id, recovered, "IPTV health recovery complete");
        }
        Ok(json!({ "recovered": recovered }))
    }
}
