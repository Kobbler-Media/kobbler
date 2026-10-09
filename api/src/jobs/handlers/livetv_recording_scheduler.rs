use crate::jobs::worker::JobHandler;
use crate::jobs::{JobError, JobResult};
use crate::repositories::livetv_recordings;
use async_trait::async_trait;
use nats_jobs::Publisher;
use serde_json::{Value, json};
use sqlx::PgPool;
use std::time::Duration;
use time::OffsetDateTime;
use tokio_util::sync::CancellationToken;
use tracing::{debug, info, warn};
use uuid::Uuid;

/// Must exceed the 60 s poll interval so a recording is always picked up before
/// its capture start; `livetv_record` then waits for the precise moment.
const RECORDING_LOOKAHEAD: Duration = Duration::from_secs(120);

pub struct LivetvRecordingSchedulerHandler {
    pool: PgPool,
    nats_publisher: Publisher,
}

impl LivetvRecordingSchedulerHandler {
    pub fn new(pool: PgPool, nats_publisher: Publisher) -> Self {
        Self {
            pool,
            nats_publisher,
        }
    }
}

#[async_trait]
impl JobHandler for LivetvRecordingSchedulerHandler {
    fn job_type(&self) -> &'static str {
        "livetv_recording_scheduler"
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
        debug!(job_id = %job_id, "Running Live TV recording scheduler");

        let due =
            livetv_recordings::list_due(&self.pool, OffsetDateTime::now_utc(), RECORDING_LOOKAHEAD)
                .await
                .map_err(|e| {
                    JobError::ExecutionFailed(format!("Failed to query due recordings: {e}"))
                })?;

        if due.is_empty() {
            return Ok(json!({ "due": 0, "enqueued": 0 }));
        }

        let mut enqueued = 0;
        for recording in &due {
            match self
                .nats_publisher
                .enqueue_job(
                    "kobbler",
                    "livetv_record",
                    json!({ "recording_id": recording.id }),
                )
                .await
            {
                Ok(_) => enqueued += 1,
                Err(e) => {
                    warn!(recording_id = %recording.id, error = %e, "Failed to enqueue livetv_record job")
                }
            }
        }

        info!(
            due = due.len(),
            enqueued, "Live TV recording scheduler complete"
        );
        Ok(json!({ "due": due.len(), "enqueued": enqueued }))
    }
}
