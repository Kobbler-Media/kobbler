//! Job handler trait for implementing job processors.

use super::JobError;
use async_trait::async_trait;
use serde_json::Value;
use std::time::Duration;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

/// Trait for job handlers.
#[async_trait]
pub trait JobHandler: Send + Sync {
    /// The job type this handler processes.
    fn job_type(&self) -> &'static str;

    /// Maximum concurrent jobs for this handler. Default is 3.
    fn max_concurrent(&self) -> usize {
        3
    }

    /// Execute the job with the given payload.
    ///
    /// The `cancel` token is triggered when the job is cancelled externally.
    /// Handlers should call [`nats_jobs::check_cancelled`] at natural checkpoints.
    async fn execute(
        &self,
        job_id: Uuid,
        payload: Value,
        cancel: CancellationToken,
    ) -> Result<Value, JobError>;

    /// Optional delay after successful completion (before NATS ack).
    /// Payload is passed so handlers can decide per-invocation.
    fn post_job_delay(&self, _payload: &Value) -> Option<Duration> {
        None
    }

    /// Optional per-handler max_deliver override. When set, this handler's
    /// NATS consumer uses this limit instead of the WorkerConfig default.
    fn max_deliver(&self) -> Option<i64> {
        None
    }
}
