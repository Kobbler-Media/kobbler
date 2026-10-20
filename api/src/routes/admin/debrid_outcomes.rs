use axum::{
    Json,
    extract::{Path, State},
    http::StatusCode,
    response::IntoResponse,
};
use serde::Deserialize;
use tracing::error;
use uuid::Uuid;

use crate::AppState;
use crate::debrid_outcomes;
use crate::repositories::debrid_stream_sessions;
use crate::routes::AuthUser;
use crate::routes::admin::debrid_stream::session_owner;

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
