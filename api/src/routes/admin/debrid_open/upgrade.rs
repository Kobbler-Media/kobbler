use std::sync::Arc;
use std::time::Duration;

use nats_jobs::{StreamPriority, StreamResolveResponse};
use tokio_util::sync::CancellationToken;
use tracing::{info, warn};
use uuid::Uuid;

use crate::AppState;
use crate::download_ahead::Joined;
use crate::repositories::debrid_stream_sessions;
use crate::routes::admin::debrid_stream::StreamResolveBody;
use crate::streaming::download_lane::DownloadLane;

const FIRST_RETRY: Duration = Duration::from_secs(1);
const LONGEST_RETRY: Duration = Duration::from_secs(30);

pub(crate) struct Upgrade {
    pub state: AppState,
    pub body: StreamResolveBody,
    /// The session whose row the writer re-reads its URL from.
    pub writer: Uuid,
    pub viewer: Uuid,
    pub lane: Arc<DownloadLane>,
    pub writer_done: CancellationToken,
}

/// A URL resolved for a viewer and stored for the writer, so a reconnect stays on it. A
/// download with a Warm cache URL only moves to another cache URL: the Play URL would compete
/// with playback. `answer` is a viewer resolve already in hand.
pub(crate) async fn viewer_url(
    state: &AppState,
    body: &StreamResolveBody,
    writer: Uuid,
    lane: &DownloadLane,
    answer: Option<StreamResolveResponse>,
) -> Option<String> {
    let resolved = match answer {
        Some(resolved) => resolved,
        None => {
            let runtime = state.runtime_config.get().await;
            super::resolve(state, &runtime, body, StreamPriority::User)
                .await
                .ok()?
        }
    };
    let cache_url = resolved.cache_url.clone().filter(|u| !u.is_empty());
    let url = if lane.needs_upgrade() {
        cache_url?
    } else {
        cache_url
            .or_else(|| Some(resolved.stream_url.clone()))
            .filter(|u| !u.is_empty())?
    };
    if let Err(e) = debrid_stream_sessions::set_urls(&state.pool, writer, &resolved).await {
        warn!(session_id = %writer, error = %e, "failed to store the viewer's URLs");
        return None;
    }
    Some(url)
}

/// Until it works, the download carries on through the Warm URL it has.
pub(crate) async fn keep_trying(upgrade: Upgrade, mut answer: Option<StreamResolveResponse>) {
    let Upgrade {
        state,
        body,
        writer,
        viewer,
        lane,
        writer_done,
    } = upgrade;
    let Some(_running) = lane.begin_upgrade() else {
        return;
    };
    let key = super::release_key(body.release_key.as_deref(), body.file_id);
    let mut delay = FIRST_RETRY;
    while lane.needs_upgrade() && !writer_done.is_cancelled() {
        if let Some(url) = viewer_url(&state, &body, writer, &lane, answer.take()).await {
            info!(session_id = %writer, %viewer, "moved the download onto the viewer's cache URL");
            lane.upgrade(url);
            return;
        }
        tokio::select! {
            _ = writer_done.cancelled() => return,
            _ = tokio::time::sleep(delay) => {}
        }
        // Another viewer on the file keeps the retry going after this one leaves.
        let watched = state
            .playback_sessions
            .iter()
            .any(|s| s.id == viewer || (key.is_some() && s.debrid_cache_key == key));
        if !watched {
            return;
        }
        delay = (delay * 2).min(LONGEST_RETRY);
    }
}

/// The download ahead this viewer joined moves to a cache URL resolved for them.
pub(super) fn for_joined(
    state: &AppState,
    body: &StreamResolveBody,
    viewer: Uuid,
    joined: Joined,
    answer: StreamResolveResponse,
) {
    if !joined.lane.needs_upgrade() {
        return;
    }
    let upgrade = Upgrade {
        state: state.clone(),
        body: body.clone(),
        writer: joined.session_id,
        viewer,
        lane: joined.lane,
        writer_done: joined.stopped,
    };
    let row_written = joined.row_written;
    tokio::spawn(async move {
        tokio::select! {
            _ = upgrade.writer_done.cancelled() => return,
            _ = row_written.cancelled() => {}
        }
        keep_trying(upgrade, Some(answer)).await;
    });
}
