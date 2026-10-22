//! Per-user edits to the For You shelf. The shelf is ranked on every request from cached
//! related lists, so a dismissal takes effect on the next load with nothing to invalidate.

use axum::{Json, extract::State, http::StatusCode, response::IntoResponse};

use super::watch_history::{AppError, TitleRef};
use crate::AppState;
use crate::repositories::for_you_dismissals;
use crate::routes::AuthUser;

/// POST /api/v1/admin/discover/for-you/dismiss
pub async fn dismiss(
    State(state): State<AppState>,
    auth: AuthUser,
    Json(title): Json<TitleRef>,
) -> Result<impl IntoResponse, AppError> {
    let (media_type, tmdb_id) = title.key()?;
    for_you_dismissals::dismiss(&state.pool, auth.user_id, &media_type, &tmdb_id).await?;
    Ok(StatusCode::NO_CONTENT)
}

/// DELETE /api/v1/admin/discover/for-you/dismiss
pub async fn undismiss(
    State(state): State<AppState>,
    auth: AuthUser,
    Json(title): Json<TitleRef>,
) -> Result<impl IntoResponse, AppError> {
    let (media_type, tmdb_id) = title.key()?;
    for_you_dismissals::undismiss(&state.pool, auth.user_id, &media_type, &tmdb_id).await?;
    Ok(StatusCode::NO_CONTENT)
}
