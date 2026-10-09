use crate::AppState;
use crate::jobs::JobResult;
use crate::jobs::worker::JobHandler;
use crate::livetv::{recorder, retention};
use crate::repositories::{
    livetv_ad_fingerprints, livetv_recordings, movie_files, tv_episode_files,
};
use async_trait::async_trait;
use serde_json::{Value, json};
use time::OffsetDateTime;
use tokio_util::sync::CancellationToken;
use tracing::{info, warn};
use uuid::Uuid;

const AD_PRINT_TTL_DAYS: i64 = 90;

pub struct LivetvRetentionHandler {
    state: AppState,
}

impl LivetvRetentionHandler {
    pub fn new(state: AppState) -> Self {
        Self { state }
    }
}

#[async_trait]
impl JobHandler for LivetvRetentionHandler {
    fn job_type(&self) -> &'static str {
        "livetv_retention"
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

        // The ad bank is rolling: an auto print nothing has matched again is an
        // ad that has stopped airing. A confirmed one is a user's correction and
        // is never swept.
        let prints_pruned =
            livetv_ad_fingerprints::prune_stale(&self.state.pool, AD_PRINT_TTL_DAYS).await?;

        let candidates = livetv_recordings::retention_candidates(&self.state.pool).await?;
        if candidates.is_empty() {
            return Ok(json!({ "considered": 0, "deleted": 0, "prints_pruned": prints_pruned }));
        }
        let ids: Vec<Uuid> = candidates.iter().map(|c| c.id).collect();
        let watched = livetv_recordings::watched_ids(&self.state.pool, &ids).await?;

        let runtime = self.state.runtime_config.get().await;
        let root = recorder::recording_root(&self.state.pool, &self.state.config, &runtime).await?;
        let disk = retention::disk_state(&root).unwrap_or_default();

        let expired = retention::expired(&candidates, &watched, OffsetDateTime::now_utc(), disk);
        let mut deleted = 0usize;
        for gone in &expired {
            nats_jobs::check_cancelled(&cancel)?;
            let Some(candidate) = candidates.iter().find(|c| c.id == gone.recording_id) else {
                continue;
            };
            if let Some(file_id) =
                livetv_recordings::find_episode_file_for_recording(&self.state.pool, candidate.id)
                    .await?
                && let Err(e) = tv_episode_files::delete(&self.state.pool, file_id).await
            {
                warn!(error = %e, recording_id = %candidate.id, "unlinking a retired recording failed");
            }
            if let Some(file_id) =
                livetv_recordings::find_movie_file_for_recording(&self.state.pool, candidate.id)
                    .await?
                && let Err(e) = movie_files::delete(&self.state.pool, file_id).await
            {
                warn!(error = %e, recording_id = %candidate.id, "unlinking a retired recording failed");
            }
            if let Some(dir) = candidate.output_dir.as_deref()
                && std::path::Path::new(dir)
                    .file_name()
                    .is_some_and(|n| n == candidate.id.to_string().as_str())
            {
                let _ = tokio::fs::remove_dir_all(dir).await;
            }
            if livetv_recordings::delete(&self.state.pool, candidate.id).await? {
                deleted += 1;
                info!(recording_id = %candidate.id, reason = gone.reason, "retired a recording");
                let _ = self.state.library_events.send(
                    crate::events::LibraryEvent::LivetvRecordingUpdated {
                        recording_id: candidate.id,
                        status: "deleted".to_string(),
                    },
                );
            }
        }

        Ok(json!({
            "prints_pruned": prints_pruned,
            "considered": candidates.len(),
            "expired": expired.len(),
            "deleted": deleted,
            "free_ratio": disk.free_ratio,
        }))
    }
}
