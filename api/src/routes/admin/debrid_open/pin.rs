use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use nats_jobs::{StreamPriority, StreamResolveRequest, StreamResolveResponse};
use tracing::{error, warn};

use crate::AppState;
use crate::repositories::debrid_stream_sessions;
use crate::routes::admin::debrid_errors::StreamFailure;
use crate::routes::admin::debrid_stream::StreamResolveBody;
use crate::routes::helpers::build_kobload_client;
use crate::runtime_config::RuntimeConfig;

/// An NZB names no file until kobload picks one, and a season pack holds one per episode,
/// so until then its key would find any episode's session or download. The file is taken
/// from what kobbler already knows of the episode (an earlier session, a warm one, a
/// finished download), so a cached episode plays with kobload down; else from kobload,
/// whose answer is kept so opening the session does not ask again.
pub(crate) async fn pin_file(
    state: &AppState,
    body: &mut StreamResolveBody,
    priority: StreamPriority,
) -> Result<(), Response> {
    if body.nzb_ref.is_none() || body.file_id.is_some() {
        return Ok(());
    }
    body.episode = super::episode::of_body(&state.pool, body).await;
    if let Some(file_id) = known_file(state, body).await {
        body.file_id = Some(file_id);
        return Ok(());
    }
    let runtime = state.runtime_config.get().await;
    let resolved = resolve(state, &runtime, body, priority).await?;
    if body.release_key.is_none() {
        body.release_key = resolved.release_key.clone();
    }
    body.file_id = resolved.file_id;
    body.resolved = Some(resolved);
    Ok(())
}

async fn known_file(state: &AppState, body: &StreamResolveBody) -> Option<i32> {
    let key = body.release_key.as_deref()?;
    if let Some(media_id) = body.media_id.filter(|id| !id.is_nil()) {
        match debrid_stream_sessions::file_played(&state.pool, key, &body.media_type, media_id)
            .await
        {
            Ok(Some(file_id)) => return Some(file_id),
            Ok(None) => {}
            Err(e) => warn!(error = %e, "earlier file of the release unreadable"),
        }
    }
    if let Some(file_id) = state.stream_prewarms.warm_file(key, body.episode) {
        return Some(file_id);
    }
    let episode = body.episode?;
    debrid_stream_sessions::downloaded_episode_file(&state.pool, key, episode)
        .await
        .unwrap_or_else(|e| {
            warn!(error = %e, "downloaded file of the episode unreadable");
            None
        })
}

pub(crate) async fn resolve(
    state: &AppState,
    runtime: &RuntimeConfig,
    body: &StreamResolveBody,
    priority: StreamPriority,
) -> Result<StreamResolveResponse, Response> {
    let client = build_kobload_client(runtime).map_err(IntoResponse::into_response)?;
    let request = StreamResolveRequest {
        debrid_provider_id: body.debrid_provider_id,
        release_key: body.release_key.clone(),
        magnet_url: body.magnet_url.clone(),
        nzb_ref: body.nzb_ref.clone(),
        file_id: body.file_id,
        priority: Some(priority),
        release_title: Some(body.release_title.clone()).filter(|t| !t.is_empty()),
        file_name: None,
        episode: super::episode::of_body(&state.pool, body).await,
    };
    client.stream_resolve(&request).await.map_err(|e| {
        error!(error = %e, "stream resolve failed");
        StreamFailure::from_kobload(&e).response(StatusCode::BAD_GATEWAY, &e.to_string())
    })
}
