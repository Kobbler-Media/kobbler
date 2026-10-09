//! IPTV EPG cleanup job handler.
//!
//! Scheduled job (every 6 hours) that prunes expired EPG entries older
//! than 1 day.

use crate::jobs::worker::JobHandler;
use crate::jobs::{JobError, JobResult};
use crate::repositories::iptv_epg;
use async_trait::async_trait;
use serde_json::{Value, json};
use sqlx::PgPool;
use tokio_util::sync::CancellationToken;
use tracing::info;
use uuid::Uuid;

/// Handler for periodic EPG cleanup.
pub struct IptvEpgCleanupHandler {
    pool: PgPool,
}

impl IptvEpgCleanupHandler {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}

#[async_trait]
impl JobHandler for IptvEpgCleanupHandler {
    fn job_type(&self) -> &'static str {
        "iptv_epg_cleanup"
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
        info!(job_id = %job_id, "Running IPTV EPG cleanup");

        // Read configured retention, default 7 days for favourites, 1 day for rest
        let retention_days = iptv_epg::get_retention_days(&self.pool).await;
        let pruned = iptv_epg::prune_tiered(&self.pool, retention_days, 1)
            .await
            .map_err(|e| JobError::ExecutionFailed(format!("EPG cleanup failed: {e}")))?;

        if pruned > 0 {
            info!(pruned, "Pruned expired EPG entries");
        }

        Ok(json!({ "pruned": pruned }))
    }
}
