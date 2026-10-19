use async_trait::async_trait;
use serde_json::{Value, json};
use sqlx::PgPool;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use crate::jobs::JobResult;
use crate::jobs::worker::JobHandler;

pub struct StremioSyncHandler {
    pool: PgPool,
    stremio_api_url: String,
}

impl StremioSyncHandler {
    pub fn new(pool: PgPool, stremio_api_url: String) -> Self {
        Self {
            pool,
            stremio_api_url,
        }
    }
}

#[async_trait]
impl JobHandler for StremioSyncHandler {
    fn job_type(&self) -> &'static str {
        "stremio_sync"
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
        let summary =
            crate::stremio_library::sync::sync_all(&self.pool, &self.stremio_api_url).await;
        Ok(json!(summary))
    }
}
