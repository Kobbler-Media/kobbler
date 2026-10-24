//! Sweep for playback sessions a restart orphaned.
//!
//! A live session's row goes when the reaper reaps it. One the API lost to a restart
//! has nothing in memory to reap, so its row and segments wait here for the same idle
//! limit the reaper would have applied.

use crate::jobs::JobResult;
use crate::jobs::worker::JobHandler;
use crate::repositories::playback_sessions;
use crate::streaming::persist::VOD_IDLE_TTL;
use async_trait::async_trait;
use serde_json::{Value, json};
use sqlx::PgPool;
use tokio_util::sync::CancellationToken;
use tracing::{info, warn};
use uuid::Uuid;

pub struct PlaybackSessionExpiryHandler {
    pool: PgPool,
}

impl PlaybackSessionExpiryHandler {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}

#[async_trait]
impl JobHandler for PlaybackSessionExpiryHandler {
    fn job_type(&self) -> &'static str {
        "playback_session_expiry"
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
        let expired =
            playback_sessions::take_idle_since(&self.pool, VOD_IDLE_TTL.as_secs_f64()).await?;
        for session in &expired {
            if let Err(e) = tokio::fs::remove_dir_all(&session.dir).await
                && e.kind() != std::io::ErrorKind::NotFound
            {
                warn!(session_id = %session.id, dir = %session.dir, error = %e, "failed to remove expired session dir");
            }
        }
        if !expired.is_empty() {
            info!(count = expired.len(), "Expired orphaned playback sessions");
        }
        Ok(json!({ "expired": expired.len() }))
    }
}
