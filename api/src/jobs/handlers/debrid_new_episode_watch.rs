//! Up Next holds back a debrid episode that came out after the viewer caught up until it is
//! downloaded, so this keeps looking for a cached release of it and downloads it ahead.

use async_trait::async_trait;
use serde_json::{Value, json};
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use crate::AppState;
use crate::download_ahead::{self, DOWNLOAD_AHEAD_JOB, NEW_EPISODE_WAIT_DAYS};
use crate::jobs::JobResult;
use crate::jobs::worker::JobHandler;
use crate::repositories::debrid_new_episodes;
use crate::routes::helpers::build_kobload_client;

pub const NEW_EPISODE_WATCH_JOB: &str = "debrid_new_episode_watch";

/// As long as Up Next keeps a show without a watch.
const HOLD_DAYS: i64 = 14;

pub struct DebridNewEpisodeWatchHandler {
    state: AppState,
}

impl DebridNewEpisodeWatchHandler {
    pub fn new(state: AppState) -> Self {
        Self { state }
    }
}

#[async_trait]
impl JobHandler for DebridNewEpisodeWatchHandler {
    fn job_type(&self) -> &'static str {
        NEW_EPISODE_WATCH_JOB
    }

    async fn execute(
        &self,
        _job_id: Uuid,
        _payload: Value,
        _cancel: CancellationToken,
    ) -> JobResult<Value> {
        let state = &self.state;
        let runtime = state.runtime_config.get().await;
        if build_kobload_client(&runtime).is_err() {
            return Ok(json!({ "skipped": "kobload not configured" }));
        }
        let due = debrid_new_episodes::due(&state.pool, DOWNLOAD_AHEAD_JOB, NEW_EPISODE_WAIT_DAYS)
            .await?;
        for d in &due {
            download_ahead::queue(state, d.user_id, d.episode_id, Some(HOLD_DAYS)).await;
        }
        Ok(json!({ "queued": due.len() }))
    }
}
