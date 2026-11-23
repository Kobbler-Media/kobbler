use axum::{
    Json,
    extract::{Path, State},
    http::StatusCode,
    response::IntoResponse,
};
use serde::{Deserialize, Serialize};
use serde_json::json;
use tracing::error;
use uuid::Uuid;

use crate::AppState;
use crate::debrid_outcomes;
use crate::jobs::handlers::debrid_cache_cleanup;
use crate::repositories::debrid_stream_sessions;
use crate::routes::admin::debrid_open;
use crate::routes::admin::debrid_stream::session_owner;
use crate::routes::helpers::build_kobload_client;
use crate::routes::{AuthUser, PlaybackAuth};

#[derive(Debug, Deserialize)]
pub struct OutcomeBody {
    pub ok: bool,
    #[serde(default)]
    pub reason: Option<String>,
}

/// POST /api/v1/admin/stream/{id}/outcome
pub async fn stream_outcome(
    State(state): State<AppState>,
    auth: AuthUser,
    Path(session_id): Path<Uuid>,
    Json(body): Json<OutcomeBody>,
) -> impl IntoResponse {
    let owner = session_owner(auth.user_id, &auth.role);
    let release = match debrid_stream_sessions::release(&state.pool, session_id, owner).await {
        Ok(Some(r)) => r,
        Ok(None) => return (StatusCode::NOT_FOUND, "session not found").into_response(),
        Err(e) => {
            error!(error = %e, "stream outcome session lookup failed");
            return (StatusCode::INTERNAL_SERVER_ERROR, "database error").into_response();
        }
    };

    if body.ok {
        debrid_outcomes::record_played(&state, session_id).await;
    } else {
        let reason = body
            .reason
            .as_deref()
            .map(str::trim)
            .filter(|r| !r.is_empty())
            .unwrap_or("client_reported");
        debrid_outcomes::report_failure(&state, &release.into(), reason).await;
    }
    StatusCode::NO_CONTENT.into_response()
}

#[derive(Debug, Deserialize, Serialize)]
pub struct BadCopyBody {
    pub debrid_provider_id: Uuid,
    #[serde(deserialize_with = "super::release_key_param::required")]
    pub release_key: String,
    #[serde(default)]
    pub file_id: Option<i32>,
    pub bad: bool,
}

/// POST /api/v1/admin/stream/bad
///
/// kobload ranks by it; the copy on disk goes at once, so nothing offers it meanwhile.
pub async fn stream_bad(
    State(state): State<AppState>,
    auth: PlaybackAuth,
    Json(body): Json<BadCopyBody>,
) -> impl IntoResponse {
    if auth.is_kids {
        return StatusCode::NOT_FOUND.into_response();
    }
    let runtime = state.runtime_config.get().await;
    let client = match build_kobload_client(&runtime) {
        Ok(c) => c,
        Err(e) => return e.into_response(),
    };
    if let Err(e) = client.stream_bad(&json!(body)).await {
        error!(error = %e, "bad copy report failed");
        return (
            StatusCode::BAD_GATEWAY,
            Json(json!({"error": format!("bad copy report failed: {e}")})),
        )
            .into_response();
    }
    if body.bad
        && let Some(cached) = debrid_open::complete_cache_entry(
            &state,
            body.debrid_provider_id,
            Some(&body.release_key),
            body.file_id,
        )
        .await
    {
        debrid_cache_cleanup::evict(&state.pool, &cached).await;
    }
    StatusCode::NO_CONTENT.into_response()
}
