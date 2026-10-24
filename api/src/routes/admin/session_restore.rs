//! Rebuilding file playback sessions the API lost to a restart.
//!
//! Every session route goes through [restore_on_miss], so a player holding a
//! session's URLs from before the restart finds the session there again — same id,
//! same playlist, the segments it already had still served — instead of a 404 that
//! sends it off to mint a new one.

use axum::{
    body::Body,
    extract::{Request, State},
    middleware::Next,
    response::Response,
};
use serde::{Deserialize, Serialize};
use tracing::{info, warn};
use uuid::Uuid;

use super::streaming::{CreateSessionRequest, open_file_session};
use crate::AppState;
use crate::repositories::playback_sessions;
use crate::streaming::persist::Progress;

/// What opened the session, enough to open it again.
#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum Recipe {
    File {
        file_id: Uuid,
        request: CreateSessionRequest,
    },
}

// Players fire their playlist and segment requests together; one rebuild has to win
// and the rest find it done.
static RESTORING: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

pub(crate) async fn remember(
    state: &AppState,
    session_id: Uuid,
    user_id: Uuid,
    file_id: Uuid,
    request: &CreateSessionRequest,
) {
    let Some(dir) = state
        .playback_sessions
        .get(&session_id)
        .map(|s| s.segment_dir().to_string_lossy().into_owned())
    else {
        return;
    };
    let recipe = Recipe::File {
        file_id,
        request: request.clone(),
    };
    let saved = match serde_json::to_value(&recipe) {
        Ok(json) => playback_sessions::insert(&state.pool, session_id, user_id, &dir, &json)
            .await
            .map_err(|e| e.to_string()),
        Err(e) => Err(e.to_string()),
    };
    if let Err(e) = saved {
        warn!(session_id = %session_id, error = %e, "playback session will not survive a restart");
    }
}

/// Drop the session's row. Returns the directory it named, if there was one.
pub(crate) async fn forget(state: &AppState, session_id: Uuid) -> Option<String> {
    playback_sessions::delete(&state.pool, session_id)
        .await
        .inspect_err(|e| warn!(session_id = %session_id, error = %e, "failed to delete playback session row"))
        .ok()
        .flatten()
}

pub async fn restore_on_miss(
    State(state): State<AppState>,
    request: Request<Body>,
    next: Next,
) -> Response {
    if let Some(id) = session_id_of(request.uri().path()) {
        ensure_loaded(&state, id).await;
    }
    next.run(request).await
}

fn session_id_of(path: &str) -> Option<Uuid> {
    let rest = path.strip_prefix("/api/v1/admin/playback/sessions/")?;
    Uuid::parse_str(rest.split('/').next()?).ok()
}

pub async fn ensure_loaded(state: &AppState, session_id: Uuid) {
    if is_loaded(state, session_id) {
        return;
    }
    let _guard = RESTORING.lock().await;
    if is_loaded(state, session_id) {
        return;
    }
    let stored = match playback_sessions::get(&state.pool, session_id).await {
        Ok(Some(stored)) => stored,
        Ok(None) => return,
        Err(e) => {
            warn!(session_id = %session_id, error = %e, "failed to read playback session row");
            return;
        }
    };
    let Ok(Recipe::File { file_id, request }) = serde_json::from_value(stored.request) else {
        warn!(session_id = %session_id, "unreadable playback session row, dropping it");
        forget(state, session_id).await;
        return;
    };
    let progress: Option<Progress> = stored.progress.and_then(|p| serde_json::from_value(p).ok());
    match open_file_session(
        state,
        stored.user_id,
        &request,
        file_id,
        session_id,
        progress,
    )
    .await
    {
        Ok(_) => info!(session_id = %session_id, "restored playback session"),
        Err(resp) => {
            warn!(session_id = %session_id, status = %resp.status(), "could not restore playback session, dropping it");
            forget(state, session_id).await;
        }
    }
}

fn is_loaded(state: &AppState, session_id: Uuid) -> bool {
    state.playback_sessions.contains_key(&session_id) || state.busy_sessions.contains(&session_id)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_a_session_route_names_a_session() {
        let id = Uuid::new_v4();
        assert_eq!(
            session_id_of(&format!(
                "/api/v1/admin/playback/sessions/{id}/segments/gen_0/seg_00001.m4s"
            )),
            Some(id)
        );
        assert_eq!(
            session_id_of(&format!("/api/v1/admin/playback/sessions/{id}")),
            Some(id)
        );
        assert_eq!(
            session_id_of("/api/v1/admin/playback/sessions/not-a-uuid/status"),
            None
        );
        assert_eq!(session_id_of("/api/v1/admin/playback/handoff"), None);
    }
}
