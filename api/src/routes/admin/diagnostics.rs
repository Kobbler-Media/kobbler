//! Playback failures reported by a client.
//!
//! The player sees things the server cannot: which URL its decoder was actually
//! handed, what the device is, which app build. A hard failure there used to
//! leave no trace anywhere, so the only record of one was a screenshot.

use crate::AppState;
use crate::repositories::client_playback_reports;
use crate::routes::AuthUser;
use axum::{Json, extract::State, http::StatusCode, response::IntoResponse};
use serde::{Deserialize, Serialize};
use serde_json::json;
use time::OffsetDateTime;
use tracing::warn;
use uuid::Uuid;

use super::livetv::{AppError, require_admin, require_contributor};

/// Generous, but not a hole: `detail` carries a stack trace and the diagnostic
/// block the dialog showed, and anything past this is a client bug rather than a
/// report worth keeping.
const MAX_DETAIL: usize = 16 * 1024;

#[derive(Debug, Deserialize)]
pub struct PlaybackReportInput {
    pub detail: String,
    #[serde(default)]
    pub client_kind: Option<String>,
    #[serde(default)]
    pub app_version: Option<String>,
    #[serde(default)]
    pub device_label: Option<String>,
    #[serde(default)]
    pub media_type: Option<String>,
    #[serde(default)]
    pub media_id: Option<String>,
    #[serde(default)]
    pub stream_url: Option<String>,
    #[serde(default)]
    pub session_id: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct PlaybackReportResponse {
    id: Uuid,
    client_kind: String,
    app_version: Option<String>,
    device_label: Option<String>,
    media_type: Option<String>,
    media_id: Option<String>,
    stream_url: Option<String>,
    session_id: Option<String>,
    detail: String,
    #[serde(with = "time::serde::rfc3339")]
    created_at: OffsetDateTime,
}

fn trimmed(value: Option<String>) -> Option<String> {
    value
        .map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty())
}

pub async fn report_playback_failure(
    State(state): State<AppState>,
    user: AuthUser,
    Json(input): Json<PlaybackReportInput>,
) -> Result<impl IntoResponse, AppError> {
    require_contributor(&user)?;

    let detail = input.detail.trim();
    if detail.is_empty() {
        return Err(AppError::BadRequest("detail is required".to_string()));
    }
    let detail: String = detail.chars().take(MAX_DETAIL).collect();
    let client_kind = trimmed(input.client_kind).unwrap_or_else(|| "unknown".to_string());
    let stream_url = trimmed(input.stream_url);

    // Worth a log line on its own: a report means somebody was staring at a
    // failure, and the log is where that gets noticed without opening the table.
    warn!(
        client_kind = %client_kind,
        app_version = input.app_version.as_deref().unwrap_or("?"),
        device = input.device_label.as_deref().unwrap_or("?"),
        media_type = input.media_type.as_deref().unwrap_or("?"),
        stream_url = stream_url.as_deref().unwrap_or("?"),
        "Client reported a playback failure"
    );

    let id = client_playback_reports::create(
        &state.pool,
        client_playback_reports::CreateReport {
            user_id: Some(user.user_id),
            client_kind,
            app_version: trimmed(input.app_version),
            device_label: trimmed(input.device_label),
            media_type: trimmed(input.media_type),
            media_id: trimmed(input.media_id),
            stream_url,
            session_id: trimmed(input.session_id),
            detail,
        },
    )
    .await?;

    Ok((StatusCode::CREATED, Json(json!({ "id": id }))))
}

#[derive(Debug, Deserialize)]
pub struct ListQuery {
    pub limit: Option<i64>,
}

pub async fn list_playback_failures(
    State(state): State<AppState>,
    user: AuthUser,
    axum::extract::Query(q): axum::extract::Query<ListQuery>,
) -> Result<impl IntoResponse, AppError> {
    require_admin(&user)?;

    let limit = q.limit.unwrap_or(50).clamp(1, 500);
    let items: Vec<PlaybackReportResponse> =
        client_playback_reports::list_recent(&state.pool, limit)
            .await?
            .into_iter()
            .map(|r| PlaybackReportResponse {
                id: r.id,
                client_kind: r.client_kind,
                app_version: r.app_version,
                device_label: r.device_label,
                media_type: r.media_type,
                media_id: r.media_id,
                stream_url: r.stream_url,
                session_id: r.session_id,
                detail: r.detail,
                created_at: r.created_at,
            })
            .collect();

    Ok(Json(json!({ "items": items })))
}
