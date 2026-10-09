//! IPTV recording scheduler job handler.
//!
//! Scheduled job (every 1 min) that checks for due recordings and enqueues
//! `iptv_record` jobs for each one.

use crate::jobs::worker::JobHandler;
use crate::jobs::{JobError, JobResult};
use crate::repositories::iptv_recordings;
use async_trait::async_trait;
use nats_jobs::Publisher;
use serde_json::{Value, json};
use sqlx::PgPool;
use std::time::Duration;
use time::OffsetDateTime;
use tokio_util::sync::CancellationToken;
use tracing::{debug, info, warn};
use uuid::Uuid;

/// How far ahead of capture-start to enqueue a recording. Must exceed the
/// scheduler poll interval (60s) so a recording is always picked up before its
/// start; the `iptv_record` handler then waits for the precise moment.
const RECORDING_LOOKAHEAD: Duration = Duration::from_secs(120);

/// Handler for periodic recording scheduling.
pub struct IptvRecordingSchedulerHandler {
    pool: PgPool,
    nats_publisher: Publisher,
}

impl IptvRecordingSchedulerHandler {
    pub fn new(pool: PgPool, nats_publisher: Publisher) -> Self {
        Self {
            pool,
            nats_publisher,
        }
    }
}

#[async_trait]
impl JobHandler for IptvRecordingSchedulerHandler {
    fn job_type(&self) -> &'static str {
        "iptv_recording_scheduler"
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
        // Debug, not info: this fires every minute and says only that it fired.
        // At info it was the single most frequent line in the API log — 93 of
        // the 193 IPTV lines in a six-hour sample, ~8% of all output — which
        // buries the lines that do carry something. What this job actually did
        // is logged below, and only when it did something.
        debug!(job_id = %job_id, "Running IPTV recording scheduler");

        let now = OffsetDateTime::now_utc();
        let due = iptv_recordings::list_due(&self.pool, now, RECORDING_LOOKAHEAD)
            .await
            .map_err(|e| {
                JobError::ExecutionFailed(format!("Failed to query due recordings: {e}"))
            })?;

        if due.is_empty() {
            return Ok(json!({ "due": 0, "enqueued": 0 }));
        }

        let mut enqueued = 0;
        for recording in &due {
            if let Err(e) = self
                .nats_publisher
                .enqueue_job(
                    "kobbler",
                    "iptv_record",
                    json!({"recording_id": recording.id}),
                )
                .await
            {
                warn!(
                    recording_id = %recording.id,
                    error = %e,
                    "Failed to enqueue iptv_record job"
                );
            } else {
                enqueued += 1;
            }
        }

        // Only worth a line when there was something to do. An idle tick every
        // minute reporting due=0 enqueued=0 is the same noise as the start line.
        if due.is_empty() {
            debug!(
                due = due.len(),
                enqueued, "IPTV recording scheduler complete"
            );
        } else {
            info!(
                due = due.len(),
                enqueued, "IPTV recording scheduler complete"
            );
        }
        Ok(json!({ "due": due.len(), "enqueued": enqueued }))
    }
}
