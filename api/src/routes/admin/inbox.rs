//! In-app notification inbox route handlers.

use axum::{
    Json,
    extract::{Path, Query, State},
    http::StatusCode,
    response::IntoResponse,
};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::AppState;
use crate::repositories::user_notifications;
use crate::routes::AuthUser;

// =============================================================================
// Query / Response Types
// =============================================================================

#[derive(Deserialize)]
pub struct ListQuery {
    #[serde(default = "default_limit")]
    pub limit: i64,
    #[serde(default)]
    pub offset: i64,
    #[serde(default)]
    pub unread_only: bool,
}

fn default_limit() -> i64 {
    20
}

#[derive(Serialize)]
struct NotificationResponse {
    id: Uuid,
    title: String,
    message: String,
    entity_type: Option<String>,
    entity_id: Option<Uuid>,
    entity_url: Option<String>,
    read: bool,
    created_at: String,
}

#[derive(Serialize)]
struct ListResponse {
    notifications: Vec<NotificationResponse>,
    total: i64,
}

#[derive(Serialize)]
struct UnreadCountResponse {
    count: i64,
}

// =============================================================================
// Handlers
// =============================================================================

/// GET /api/v1/admin/notifications/inbox
pub async fn list_inbox(
    State(state): State<AppState>,
    auth: AuthUser,
    Query(query): Query<ListQuery>,
) -> impl IntoResponse {
    let limit = query.limit.clamp(1, 100);
    match user_notifications::list(
        &state.pool,
        auth.user_id,
        limit,
        query.offset,
        query.unread_only,
    )
    .await
    {
        Ok((rows, total)) => {
            let notifications = rows
                .into_iter()
                .map(|r| NotificationResponse {
                    id: r.id,
                    title: r.title,
                    message: r.message,
                    entity_type: r.entity_type,
                    entity_id: r.entity_id,
                    entity_url: r.entity_url,
                    read: r.read,
                    created_at: r
                        .created_at
                        .format(&time::format_description::well_known::Rfc3339)
                        .unwrap_or_default(),
                })
                .collect();
            Json(ListResponse {
                notifications,
                total,
            })
            .into_response()
        }
        Err(e) => {
            tracing::error!(error = %e, "Failed to list notifications");
            StatusCode::INTERNAL_SERVER_ERROR.into_response()
        }
    }
}

/// GET /api/v1/admin/notifications/inbox/unread-count
pub async fn unread_count(State(state): State<AppState>, auth: AuthUser) -> impl IntoResponse {
    match user_notifications::count_unread(&state.pool, auth.user_id).await {
        Ok(count) => Json(UnreadCountResponse { count }).into_response(),
        Err(e) => {
            tracing::error!(error = %e, "Failed to count unread notifications");
            StatusCode::INTERNAL_SERVER_ERROR.into_response()
        }
    }
}

/// PATCH /api/v1/admin/notifications/inbox/{id}/read
pub async fn mark_read(
    State(state): State<AppState>,
    auth: AuthUser,
    Path(id): Path<Uuid>,
) -> impl IntoResponse {
    match user_notifications::mark_read(&state.pool, id, auth.user_id).await {
        Ok(true) => StatusCode::NO_CONTENT.into_response(),
        Ok(false) => StatusCode::NOT_FOUND.into_response(),
        Err(e) => {
            tracing::error!(error = %e, "Failed to mark notification read");
            StatusCode::INTERNAL_SERVER_ERROR.into_response()
        }
    }
}

/// POST /api/v1/admin/notifications/inbox/mark-all-read
pub async fn mark_all_read(State(state): State<AppState>, auth: AuthUser) -> impl IntoResponse {
    match user_notifications::mark_all_read(&state.pool, auth.user_id).await {
        Ok(_) => StatusCode::NO_CONTENT.into_response(),
        Err(e) => {
            tracing::error!(error = %e, "Failed to mark all notifications read");
            StatusCode::INTERNAL_SERVER_ERROR.into_response()
        }
    }
}
