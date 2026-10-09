//! Per-user "want to watch" list.

use axum::{
    Json,
    extract::{Path, State},
    http::StatusCode,
    response::IntoResponse,
};
use serde::{Deserialize, Serialize};
use serde_json::json;
use time::OffsetDateTime;
use uuid::Uuid;

use crate::AppState;
use crate::images::{ImageMap, endpoint_url};
use crate::repositories::watchlist;
use crate::routes::AuthUser;

pub enum AppError {
    NotFound(String),
    BadRequest(String),
    Internal(String),
}

impl IntoResponse for AppError {
    fn into_response(self) -> axum::response::Response {
        let (status, message) = match self {
            AppError::NotFound(msg) => (StatusCode::NOT_FOUND, msg),
            AppError::BadRequest(msg) => (StatusCode::BAD_REQUEST, msg),
            AppError::Internal(msg) => (StatusCode::INTERNAL_SERVER_ERROR, msg),
        };
        (status, Json(json!({ "error": message }))).into_response()
    }
}

#[derive(Debug, Deserialize)]
pub struct WatchlistRequest {
    pub media_type: String,
    pub media_id: Uuid,
}

#[derive(Debug, Serialize)]
pub struct WatchlistEntry {
    pub media_type: String,
    pub media_id: String,
    pub title: String,
    pub subtitle: Option<String>,
    pub image_url: Option<String>,
    pub link: String,
    #[serde(with = "time::serde::rfc3339")]
    pub added_at: OffsetDateTime,
}

/// GET /api/v1/admin/watchlist
pub async fn list_watchlist(
    State(state): State<AppState>,
    auth: AuthUser,
) -> Result<Json<Vec<WatchlistEntry>>, AppError> {
    let rows = watchlist::list_for_user(&state.pool, auth.user_id)
        .await
        .map_err(|e| AppError::Internal(format!("Watchlist query failed: {e}")))?;

    let entries = rows
        .into_iter()
        .map(|r| {
            let images: ImageMap = serde_json::from_value(r.images.clone()).unwrap_or_default();
            let is_show = r.media_type == watchlist::SHOW;
            let base = if is_show {
                "/api/v1/admin/tv/shows/"
            } else {
                "/api/v1/admin/movies/"
            };
            let link = if is_show {
                format!("/tv/{}", r.media_id)
            } else {
                format!("/movies/{}", r.media_id)
            };
            WatchlistEntry {
                media_type: r.media_type,
                media_id: r.media_id.to_string(),
                title: r.title,
                subtitle: r.subtitle,
                image_url: endpoint_url(&images, "poster", base, r.media_id, "/poster"),
                link,
                added_at: r.added_at,
            }
        })
        .collect();

    Ok(Json(entries))
}

/// POST /api/v1/admin/watchlist
pub async fn add_to_watchlist(
    State(state): State<AppState>,
    auth: AuthUser,
    Json(body): Json<WatchlistRequest>,
) -> Result<impl IntoResponse, AppError> {
    if !watchlist::is_valid_media_type(&body.media_type) {
        return Err(AppError::BadRequest(format!(
            "Invalid media_type: {}. Must be 'show' or 'movie'.",
            body.media_type
        )));
    }

    ensure_media_exists(&state, &body.media_type, body.media_id).await?;

    watchlist::add(&state.pool, auth.user_id, &body.media_type, body.media_id)
        .await
        .map_err(|e| AppError::Internal(format!("Failed to add to watchlist: {e}")))?;

    Ok(StatusCode::NO_CONTENT)
}

/// DELETE /api/v1/admin/watchlist/{media_type}/{media_id}
pub async fn remove_from_watchlist(
    State(state): State<AppState>,
    auth: AuthUser,
    Path((media_type, media_id)): Path<(String, Uuid)>,
) -> Result<impl IntoResponse, AppError> {
    if !watchlist::is_valid_media_type(&media_type) {
        return Err(AppError::BadRequest(format!(
            "Invalid media_type: {media_type}. Must be 'show' or 'movie'."
        )));
    }

    watchlist::remove(&state.pool, auth.user_id, &media_type, media_id)
        .await
        .map_err(|e| AppError::Internal(format!("Failed to remove from watchlist: {e}")))?;

    Ok(StatusCode::NO_CONTENT)
}

async fn ensure_media_exists(
    state: &AppState,
    media_type: &str,
    media_id: Uuid,
) -> Result<(), AppError> {
    let exists = match media_type {
        watchlist::SHOW => crate::repositories::tv_shows::get(&state.pool, media_id)
            .await
            .map(|r| r.is_some()),
        _ => crate::repositories::movies::get(&state.pool, media_id)
            .await
            .map(|r| r.is_some()),
    }
    .map_err(|e| AppError::Internal(format!("Failed to look up media: {e}")))?;

    if exists {
        Ok(())
    } else {
        Err(AppError::NotFound(format!("{media_type} not found")))
    }
}
