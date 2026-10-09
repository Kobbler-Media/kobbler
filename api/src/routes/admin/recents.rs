//! Recents endpoints for command palette frecency tracking.

use axum::{Json, extract::State, http::StatusCode, response::IntoResponse};
use serde::Deserialize;

use crate::AppState;
use crate::repositories::search_recents;
use crate::routes::AuthUser;

/// GET /api/v1/admin/recents — list recent items for the authenticated user.
pub async fn list_recents(State(state): State<AppState>, auth: AuthUser) -> impl IntoResponse {
    match search_recents::list_for_user(&state.pool, auth.user_id, 10).await {
        Ok(items) => Json(items).into_response(),
        Err(e) => {
            tracing::error!(error = %e, "Failed to list recents");
            (StatusCode::INTERNAL_SERVER_ERROR, "database error").into_response()
        }
    }
}

#[derive(Deserialize)]
pub struct RecordAccessRequest {
    pub entity_type: String,
    pub entity_id: String,
}

/// DELETE /api/v1/admin/recents — clear all recent items for the authenticated user.
pub async fn clear_recents(State(state): State<AppState>, auth: AuthUser) -> impl IntoResponse {
    match search_recents::clear_for_user(&state.pool, auth.user_id).await {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(e) => {
            tracing::error!(error = %e, "Failed to clear recents");
            (StatusCode::INTERNAL_SERVER_ERROR, "database error").into_response()
        }
    }
}

/// POST /api/v1/admin/recents — record a recent access.
pub async fn record_recent(
    State(state): State<AppState>,
    auth: AuthUser,
    Json(payload): Json<RecordAccessRequest>,
) -> impl IntoResponse {
    if let Err(e) = search_recents::record_access(
        &state.pool,
        auth.user_id,
        &payload.entity_type,
        &payload.entity_id,
    )
    .await
    {
        tracing::error!(error = %e, "Failed to record recent access");
        return (StatusCode::INTERNAL_SERVER_ERROR, "database error").into_response();
    }

    StatusCode::NO_CONTENT.into_response()
}
