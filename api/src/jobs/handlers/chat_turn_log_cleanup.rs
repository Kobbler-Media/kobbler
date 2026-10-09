//! Retention sweep for `chat_turn_logs`.

use crate::jobs::worker::JobHandler;
use crate::jobs::{JobError, JobResult};
use crate::repositories::chat_turn_logs;
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sqlx::PgPool;
use tokio_util::sync::CancellationToken;
use tracing::{error, info};
use uuid::Uuid;

pub const DEFAULT_RETENTION_DAYS: i32 = 30;

#[derive(Debug, Deserialize, Serialize)]
struct ChatTurnLogCleanupPayload {
    #[serde(default = "default_retention_days")]
    retention_days: i32,
}

fn default_retention_days() -> i32 {
    DEFAULT_RETENTION_DAYS
}

impl Default for ChatTurnLogCleanupPayload {
    fn default() -> Self {
        Self {
            retention_days: DEFAULT_RETENTION_DAYS,
        }
    }
}

pub struct ChatTurnLogCleanupHandler {
    pool: PgPool,
}

impl ChatTurnLogCleanupHandler {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}

#[async_trait]
impl JobHandler for ChatTurnLogCleanupHandler {
    fn job_type(&self) -> &'static str {
        "chat_turn_log_cleanup"
    }

    fn max_concurrent(&self) -> usize {
        1
    }

    async fn execute(
        &self,
        _job_id: Uuid,
        payload: Value,
        cancel: CancellationToken,
    ) -> JobResult<Value> {
        nats_jobs::check_cancelled(&cancel)?;

        let payload: ChatTurnLogCleanupPayload =
            serde_json::from_value(payload).unwrap_or_default();
        let retention_days = payload.retention_days.max(1);

        let deleted = chat_turn_logs::delete_older_than_days(&self.pool, retention_days)
            .await
            .map_err(|e| {
                error!("Chat turn log cleanup failed: {e}");
                JobError::Repository(e)
            })?;

        if deleted > 0 {
            info!(
                deleted = deleted,
                retention_days = retention_days,
                "Deleted old chat turn logs"
            );
        }

        Ok(json!({ "deleted": deleted, "retention_days": retention_days }))
    }
}
