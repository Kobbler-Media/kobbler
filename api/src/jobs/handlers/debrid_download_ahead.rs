use std::path::Path;

use async_trait::async_trait;
use nats_jobs::StreamResolveResponse;
use serde::Deserialize;
use serde_json::{Value, json};
use tokio_util::sync::CancellationToken;
use tracing::{info, warn};
use uuid::Uuid;

use crate::AppState;
use crate::download_ahead::DOWNLOAD_AHEAD_JOB;
use crate::jobs::worker::JobHandler;
use crate::jobs::{JobError, JobResult};
use crate::repositories::debrid_cache_files::{self as cache_files, CacheRelease};
use crate::routes::admin::debrid_errors::StreamFailure;
use crate::routes::admin::debrid_open::{SessionRow, VIDEO_CONTAINERS, episode, release_key};
use crate::routes::admin::debrid_stream::StreamResolveBody;
use crate::routes::admin::stream_prewarm::TriedReleases;
use crate::routes::admin::{stream_local, stream_prewarm};
use crate::routes::helpers::build_kobload_client;
use crate::streaming::cache_download;
use crate::streaming::download_lane::DownloadLane;
use crate::streaming::download_meter::DownloadMeter;
use crate::streaming::progressive::{DownloadError, DownloadTarget};

const MAX_ATTEMPTS: usize = 3;

#[derive(Deserialize)]
struct Payload {
    user_id: Uuid,
    episode_id: Uuid,
    #[serde(default)]
    hold_days: Option<i64>,
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
            hold_days,
        } = serde_json::from_value(payload)?;
        let state = &self.state;
        if stream_local::local_option(state, "episode", episode_id)
            .await?
            .is_some()
        {
            return Ok(json!({ "skipped": "already on disk" }));
        }
        let (body, resolved, key) = match first_servable(state, user_id, episode_id).await? {
            Walked::Servable(servable) => *servable,
            Walked::Skipped(reason) => return Ok(json!({ "skipped": reason })),
        };
        let runtime = state.runtime_config.get().await;

        let session_id = Uuid::new_v4();
        let Some(cache_path) = cache_download::cache_file(
            &cache_download::cache_dir(state, &runtime),
            &key,
            &resolved.filename,
        ) else {
            warn!(%episode_id, release_key = ?key, filename = ?resolved.filename, "refused an unsafe cache path");
            return Ok(json!({ "skipped": "unsafe cache path" }));
        };
        let meter = DownloadMeter::new(resolved.filesize.max(0) as u64, on_disk(&cache_path).await);
        let lane = DownloadLane::warm(resolved.cache_url.is_some());
        let Some(claim) =
            state
                .downloads_ahead
                .claim(&key, &cancel, meter.clone(), session_id, lane.clone())
        else {
            return Ok(json!({ "skipped": "already downloading" }));
        };
        // The row is what analysis and a later Play find the file by.
        SessionRow {
            session_id,
            user_id,
            release_title: body.release_title.clone(),
            debrid_provider: body.debrid_provider.clone(),
            debrid_provider_id: body.debrid_provider_id,
            release_key: body.release_key.clone(),
            nzb_ref: body.nzb_ref.clone(),
            file_id: body.file_id,
            quality: body.quality.clone(),
            filename: resolved.filename.clone(),
            filesize: resolved.filesize,
            cache_path: Some(cache_path.to_string_lossy().to_string()),
            debrid_url: Some(resolved.stream_url.clone()),
            debrid_cache_url: resolved.cache_url.clone(),
            debrid_url_expires_at: resolved.expires_at.as_deref().and_then(|s| {
                time::OffsetDateTime::parse(s, &time::format_description::well_known::Rfc3339).ok()
            }),
            chapters: Vec::new(),
            downloaded: false,
        }
        .insert(&state.pool, "episode", episode_id, Default::default(), None)
        .await?;
        claim.row_written();

        let target = DownloadTarget {
            session_id,
            url: resolved.cache_url.unwrap_or(resolved.stream_url),
            cache_path: cache_path.clone(),
            filesize: resolved.filesize,
            release: CacheRelease {
                debrid_provider_id: Some(body.debrid_provider_id),
                release_key: body.release_key.clone(),
                file_id: body.file_id,
                filename: resolved.filename.clone(),
            },
            lane,
        };
        let result = cache_download::download(state, target, claim.cancel(), meter).await;
        let handed_over = claim.cancel().is_cancelled() && !cancel.is_cancelled();
        drop(claim);
        match result {
            Ok(()) => {
                if let Some(days) = hold_days {
                    let until = time::OffsetDateTime::now_utc() + time::Duration::days(days);
                    cache_files::hold_until(&state.pool, &cache_path.to_string_lossy(), until)
                        .await?;
                }
                info!(%episode_id, %session_id, "downloaded the next episode ahead");
                Ok(json!({ "episode_id": episode_id, "session_id": session_id }))
            }
            Err(_) if handed_over => Ok(json!({ "handed_over": session_id })),
            Err(DownloadError::Dead(reason)) => {
                state.downloads_ahead.mark_damaged(&key);
                Err(JobError::ExecutionFailed(reason))
            }
            Err(e) => Err(JobError::retryable(e.to_string())),
        }
    }
}

enum Walked {
    Servable(Box<(StreamResolveBody, StreamResolveResponse, String)>),
    Skipped(&'static str),
}

fn playing(state: &AppState, body: &StreamResolveBody) -> bool {
    release_key(body.release_key.as_deref(), body.file_id)
        .is_some_and(|key| state.debrid_session_index.contains_key(&key))
}

fn refusal(state: &AppState, key: &str, resolved: &StreamResolveResponse) -> Option<&'static str> {
    if state.downloads_ahead.is_damaged(key) {
        return Some("damaged");
    }
    // Uncached on the provider means waiting on a torrent, which is the player's call.
    if !resolved.cached {
        return Some("not cached on the provider");
    }
    let container = resolved.filename.rsplit('.').next().unwrap_or_default();
    (!VIDEO_CONTAINERS.contains(&container.to_lowercase().as_str())).then_some("not a video")
}

async fn first_servable(state: &AppState, user_id: Uuid, episode_id: Uuid) -> JobResult<Walked> {
    let runtime = state.runtime_config.get().await;
    let Ok(client) = build_kobload_client(&runtime) else {
        return Ok(Walked::Skipped("nothing streamable"));
    };
    let wanted = episode::of_media(&state.pool, "episode", episode_id).await;
    let mut tried = TriedReleases::default();
    let mut refused = None;
    for _ in 0..MAX_ATTEMPTS {
        let Some(mut body) =
            stream_prewarm::pick_episode(state, user_id, episode_id, &mut tried).await
        else {
            break;
        };
        if playing(state, &body) {
            return Ok(Walked::Skipped("already playing"));
        }
        let resolved = client
            .stream_resolve(&nats_jobs::StreamResolveRequest {
                debrid_provider_id: body.debrid_provider_id,
                release_key: body.release_key.clone(),
                magnet_url: body.magnet_url.clone(),
                nzb_ref: body.nzb_ref.clone(),
                file_id: body.file_id,
                priority: Some(nats_jobs::StreamPriority::Warm),
                release_title: Some(body.release_title.clone()).filter(|t| !t.is_empty()),
                file_name: None,
                episode: wanted,
            })
            .await;
        let resolved = match resolved {
            Ok(r) => r,
            Err(e) => {
                let failure = StreamFailure::from_kobload(&e);
                if !failure.is_permanent() {
                    return Err(JobError::retryable(e.to_string()));
                }
                info!(%episode_id, reason = failure.code(), error = %e, "the release will not be served ahead");
                tried.add(&body);
                refused = Some(failure.code());
                continue;
            }
        };
        if body.release_key.is_none() {
            body.release_key = resolved.release_key.clone();
        }
        if body.file_id.is_none() {
            body.file_id = resolved.file_id;
        }
        let Some(key) = release_key(body.release_key.as_deref(), body.file_id) else {
            return Ok(Walked::Skipped("release has no key"));
        };
        if playing(state, &body) {
            return Ok(Walked::Skipped("already playing"));
        }
        if let Some(reason) = refusal(state, &key, &resolved) {
            tried.add(&body);
            refused = Some(reason);
            continue;
        }
        return Ok(Walked::Servable(Box::new((body, resolved, key))));
    }
    Ok(Walked::Skipped(refused.unwrap_or("nothing streamable")))
}

async fn on_disk(path: &Path) -> u64 {
    tokio::fs::metadata(path).await.map_or(0, |m| m.len())
}
