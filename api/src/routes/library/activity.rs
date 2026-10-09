//! Activity-related route handlers.

use axum::{
    Json as AxumJson,
    extract::{Query, State},
    http::StatusCode,
    response::IntoResponse,
};

use crate::AppState;
use crate::repositories::activity_log;
use crate::routes::AuthUser;

use super::helpers::{Cursor, decode_cursor, encode_cursor};
use super::types::{ActivityEvent, PaginatedResponse, PaginationParams};

pub async fn list_activity(
    State(state): State<AppState>,
    _auth: AuthUser,
    Query(params): Query<PaginationParams>,
) -> impl IntoResponse {
    let runtime_config = state.runtime_config.get().await;
    let limit = params
        .limit
        .unwrap_or(runtime_config.default_page_size)
        .min(runtime_config.activity_limit);
    let cursor = params.cursor.as_deref().and_then(decode_cursor);

    // Only count on first page (no cursor)
    let total_count = if cursor.is_none() {
        activity_log::count(&state.pool).await.ok()
    } else {
        None
    };

    let rows = match activity_log::list(
        &state.pool,
        cursor.as_ref().map(|c| c.ts),
        cursor.as_ref().map(|c| c.id),
        limit + 1,
    )
    .await
    {
        Ok(rows) => rows,
        Err(e) => {
            tracing::error!(error = %e, "failed to list activity log");
            return (StatusCode::INTERNAL_SERVER_ERROR, "database error").into_response();
        }
    };

    let has_more = rows.len() as i64 > limit;
    let rows: Vec<_> = if has_more {
        rows.into_iter().take(limit as usize).collect()
    } else {
        rows
    };

    let events: Vec<ActivityEvent> = rows
        .into_iter()
        .map(|row| ActivityEvent {
            id: row.id,
            event_kind: row.event_kind,
            entity_type: row.entity_type,
            entity_id: row.entity_id,
            title: row.title,
            subtitle: row.subtitle,
            image_url: row.image_url,
            url: row.url,
            created_at: row.created_at,
        })
        .collect();

    let next_cursor = if has_more {
        events.last().map(|e| {
            encode_cursor(&Cursor {
                id: e.id,
                ts: e.created_at,
            })
        })
    } else {
        None
    };

    AxumJson(PaginatedResponse {
        items: events,
        next_cursor,
        prev_cursor: None,
        total_count,
    })
    .into_response()
}
