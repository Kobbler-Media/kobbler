//! Background session directory cleanup handler.
//!
//! Deletes session transcode directories with retries and exponential backoff.
//! Enqueued when sessions are destroyed or reaped, avoiding blocking I/O
//! on the request path.

use crate::jobs::worker::JobHandler;
use crate::jobs::{JobError, JobResult};
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::path::Path;
use tokio_util::sync::CancellationToken;
use tracing::{info, warn};
use uuid::Uuid;

#[derive(Debug, Serialize, Deserialize)]
pub struct SessionCleanupPayload {
    pub dir: String,
}

pub struct SessionCleanupHandler;

#[async_trait]
impl JobHandler for SessionCleanupHandler {
    fn job_type(&self) -> &'static str {
        "session_cleanup"
    }

    fn max_concurrent(&self) -> usize {
        2
    }

    async fn execute(
        &self,
        job_id: Uuid,
        payload: Value,
        cancel: CancellationToken,
    ) -> JobResult<Value> {
        let payload: SessionCleanupPayload = serde_json::from_value(payload)?;
        let dir = Path::new(&payload.dir);

        if !dir.exists() {
            info!(job_id = %job_id, dir = %payload.dir, "Session dir already removed");
            return Ok(json!({"status": "already_removed"}));
        }

        let mut last_err = None;
        let delays = [1, 2, 4]; // exponential backoff in seconds

        for (attempt, delay_secs) in delays.iter().enumerate() {
            nats_jobs::check_cancelled(&cancel)?;
            match tokio::fs::remove_dir_all(dir).await {
                Ok(()) => {
                    info!(
                        job_id = %job_id,
                        dir = %payload.dir,
                        attempt = attempt + 1,
                        "Session directory cleaned up"
                    );
                    return Ok(json!({"status": "removed", "attempts": attempt + 1}));
                }
                Err(e) => {
                    warn!(
                        job_id = %job_id,
                        dir = %payload.dir,
                        attempt = attempt + 1,
                        error = %e,
                        "Failed to remove session directory, retrying"
                    );
                    last_err = Some(e);
                    tokio::time::sleep(std::time::Duration::from_secs(*delay_secs)).await;
                }
            }
        }

        // If the directory disappeared between retries, that's fine
        if !dir.exists() {
            return Ok(json!({"status": "removed_between_retries"}));
        }

        Err(JobError::ExecutionFailed(format!(
            "Failed to remove session directory {} after 3 attempts: {}",
            payload.dir,
            last_err.map(|e| e.to_string()).unwrap_or_default()
        )))
    }
}
