//! Searches the next episode while the current one plays, so kobload probes and warms its
//! top streams before the viewer gets there. The request is built exactly as the viewer's
//! own search will be, which is what lets that search hit kobload's caches.

use async_trait::async_trait;
use serde::Deserialize;
use serde_json::{Value, json};
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use crate::AppState;
use crate::debrid_outcomes::{self, PREWARM_JOB};
use crate::jobs::worker::JobHandler;
use crate::jobs::{JobError, JobResult};
use crate::repositories::tv_episodes;
use crate::routes::admin::debrid_stream::build_episode_search_request;
use crate::routes::helpers::build_kobload_client;

#[derive(Deserialize)]
struct Payload {
    user_id: Uuid,
    after_episode_id: Uuid,
}

pub struct DebridPrewarmHandler {
    state: AppState,
}

impl DebridPrewarmHandler {
    pub fn new(state: AppState) -> Self {
        Self { state }
    }
}

#[async_trait]
impl JobHandler for DebridPrewarmHandler {
    fn job_type(&self) -> &'static str {
        PREWARM_JOB
    }

    fn max_concurrent(&self) -> usize {
        2
    }

    async fn execute(
        &self,
        _job_id: Uuid,
        payload: Value,
        cancel: CancellationToken,
    ) -> JobResult<Value> {
        nats_jobs::check_cancelled(&cancel)?;
        let payload: Payload = serde_json::from_value(payload)?;
        let pool = &self.state.pool;
        let Some(episode_id) =
            tv_episodes::next_streamable_episode(pool, payload.after_episode_id).await?
        else {
            return Ok(json!({ "skipped": "no next episode" }));
        };

        let mut request = build_episode_search_request(&self.state, episode_id)
            .await
            .map_err(|_| JobError::ExecutionFailed(format!("episode {episode_id} not found")))?;
        request.prefer =
            debrid_outcomes::search_preference(pool, payload.user_id, None, Some(episode_id))
                .await
                .prefer;

        let runtime = self.state.runtime_config.get().await;
        let client =
            build_kobload_client(&runtime).map_err(|(_, msg)| JobError::ExecutionFailed(msg))?;
        let response = client
            .stream_search(&request)
            .await
            .map_err(|e| JobError::retryable(e.to_string()))?;
        Ok(json!({ "episode_id": episode_id, "options": response.options.len() }))
    }
}
