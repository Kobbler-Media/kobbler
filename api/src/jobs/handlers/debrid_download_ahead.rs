use std::path::Path;

use async_trait::async_trait;
use serde::Deserialize;
use serde_json::{Value, json};
use tokio_util::sync::CancellationToken;
use tracing::info;
use uuid::Uuid;

use crate::AppState;
use crate::download_ahead::DOWNLOAD_AHEAD_JOB;
use crate::jobs::worker::JobHandler;
use crate::jobs::{JobError, JobResult};
use crate::repositories::debrid_cache_files::CacheRelease;
use crate::routes::admin::debrid_open::{SessionRow, VIDEO_CONTAINERS, release_key};
use crate::routes::admin::{stream_local, stream_prewarm};
use crate::routes::helpers::build_kobload_client;
use crate::streaming::cache_download;
use crate::streaming::download_meter::DownloadMeter;
use crate::streaming::progressive::DownloadTarget;

#[derive(Deserialize)]
struct Payload {
    user_id: Uuid,
    episode_id: Uuid,
}

pub struct DebridDownloadAheadHandler {
    state: AppState,
}

impl DebridDownloadAheadHandler {
    pub fn new(state: AppState) -> Self {
        Self { state }
    }
}

#[async_trait]
impl JobHandler for DebridDownloadAheadHandler {
    fn job_type(&self) -> &'static str {
        DOWNLOAD_AHEAD_JOB
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
        let Payload {
            user_id,
            episode_id,
        } = serde_json::from_value(payload)?;
        let state = &self.state;
        if stream_local::local_option(state, "episode", episode_id)
            .await?
            .is_some()
        {
            return Ok(json!({ "skipped": "already on disk" }));
        }
        let Some(body) = stream_prewarm::pick_episode(state, user_id, episode_id).await else {
            return Ok(json!({ "skipped": "nothing streamable" }));
        };
        let Some(key) = release_key(body.info_hash.as_deref(), body.file_id) else {
            return Ok(json!({ "skipped": "release has no info hash" }));
        };
        if state.debrid_session_index.contains_key(&key) {
            return Ok(json!({ "skipped": "already playing" }));
        }
        let runtime = state.runtime_config.get().await;
        let client =
            build_kobload_client(&runtime).map_err(|(_, msg)| JobError::ExecutionFailed(msg))?;
        let resolved = client
            .stream_resolve(&nats_jobs::StreamResolveRequest {
                debrid_provider_id: body.debrid_provider_id,
                info_hash: body.info_hash.clone(),
                magnet_url: body.magnet_url.clone(),
                nzb_url: body.nzb_url.clone(),
                file_id: body.file_id,
                priority: Some(nats_jobs::StreamPriority::Warm),
                release_title: Some(body.release_title.clone()).filter(|t| !t.is_empty()),
                file_name: None,
            })
            .await
            .map_err(|e| JobError::retryable(e.to_string()))?;
        // Uncached on the provider means waiting on a torrent, which is the player's call.
        if !resolved.cached {
            return Ok(json!({ "skipped": "not cached on the provider" }));
        }
        let container = resolved.filename.rsplit('.').next().unwrap_or_default();
        if !VIDEO_CONTAINERS.contains(&container.to_lowercase().as_str()) {
            return Ok(json!({ "skipped": "not a video" }));
        }

        let session_id = Uuid::new_v4();
        let cache_path = cache_download::cache_dir(state, &runtime)
            .join(body.info_hash.as_deref().unwrap_or_default())
            .join(&resolved.filename);
        let meter = DownloadMeter::new(resolved.filesize.max(0) as u64, on_disk(&cache_path));
        let Some(claim) = state.downloads_ahead.claim(&key, &cancel, meter.clone()) else {
            return Ok(json!({ "skipped": "already downloading" }));
        };
        // The row is what analysis and a later Play find the file by.
        SessionRow {
            session_id,
            user_id,
            release_title: body.release_title.clone(),
            debrid_provider: body.debrid_provider.clone(),
            debrid_provider_id: body.debrid_provider_id,
            info_hash: body.info_hash.clone(),
            file_id: body.file_id,
            quality: body.quality.clone(),
            filename: resolved.filename.clone(),
            filesize: resolved.filesize,
            cache_path: Some(cache_path.to_string_lossy().to_string()),
            debrid_url: Some(resolved.stream_url.clone()),
            debrid_url_expires_at: resolved.expires_at.as_deref().and_then(|s| {
                time::OffsetDateTime::parse(s, &time::format_description::well_known::Rfc3339).ok()
            }),
            chapters: Vec::new(),
        }
        .insert(&state.pool, "episode", episode_id, &[], None)
        .await?;

        let target = DownloadTarget {
            session_id,
            url: resolved.stream_url,
            cache_path: cache_path.clone(),
            filesize: resolved.filesize,
            release: CacheRelease {
                debrid_provider_id: Some(body.debrid_provider_id),
                info_hash: body.info_hash.clone(),
                file_id: body.file_id,
                filename: resolved.filename.clone(),
            },
        };
        let result = cache_download::download(state, target, claim.cancel(), meter).await;
        let handed_over = claim.cancel().is_cancelled() && !cancel.is_cancelled();
        drop(claim);
        match result {
            Ok(()) => {
                info!(%episode_id, %session_id, "downloaded the next episode ahead");
                Ok(json!({ "episode_id": episode_id, "session_id": session_id }))
            }
            Err(_) if handed_over => Ok(json!({ "handed_over": session_id })),
            Err(e) => Err(JobError::retryable(e)),
        }
    }
}

fn on_disk(path: &Path) -> u64 {
    std::fs::metadata(path).map_or(0, |m| m.len())
}
