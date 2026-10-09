//! Delivery sweep for `chat_watches`.
//!
//! Retries verdicts that failed to land, then closes off watches that ran out of time
//! with a final "I've stopped watching this". A watch that simply vanishes is the
//! silence this whole feature exists to avoid.

use crate::chatbot::watches::WatchDispatcher;
use crate::jobs::JobResult;
use crate::jobs::worker::JobHandler;
use async_trait::async_trait;
use serde_json::{Value, json};
use sqlx::PgPool;
use tokio_util::sync::CancellationToken;
use tracing::info;
use uuid::Uuid;

pub struct ChatWatchCleanupHandler {
    pool: PgPool,
}

impl ChatWatchCleanupHandler {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}

#[async_trait]
impl JobHandler for ChatWatchCleanupHandler {
    fn job_type(&self) -> &'static str {
        "chat_watch_cleanup"
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

        let outcome = WatchDispatcher::new(self.pool.clone()).sweep().await;

        if outcome.retried > 0 || outcome.expired > 0 || outcome.abandoned > 0 {
            info!(
                retried = outcome.retried,
                expired = outcome.expired,
                abandoned = outcome.abandoned,
                "Closed off chat watches",
            );
        }

        Ok(json!({
            "retried": outcome.retried,
            "expired": outcome.expired,
            "abandoned": outcome.abandoned,
        }))
    }
}
