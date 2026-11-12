use axum::{Json, extract::State, http::StatusCode, response::IntoResponse};
use serde::Deserialize;
use uuid::Uuid;

use super::watchlist::AppError;
use crate::AppState;
use crate::repositories::{movies, stremio_continue, tv_episodes, up_next_dismissals, watchlist};
use crate::routes::AuthUser;

#[derive(Debug, Deserialize)]
pub struct DismissRequest {
    pub media_type: String,
    pub media_id: String,
}

enum Target {
    Library(&'static str, Uuid),
    Stremio(String),
}

/// POST /api/v1/admin/up-next/dismiss
pub async fn dismiss(
    State(state): State<AppState>,
    auth: AuthUser,
    Json(body): Json<DismissRequest>,
) -> Result<impl IntoResponse, AppError> {
    let target = resolve(&state, auth.user_id, body).await?;
    let mut tx = state.pool.begin().await.map_err(internal)?;
    match &target {
        Target::Library(kind, id) => {
            up_next_dismissals::dismiss(&mut *tx, auth.user_id, kind, &id.to_string())
                .await
                .map_err(internal)?;
            // A watchlisted title would come straight back through the watchlist branches.
            watchlist::remove(&mut *tx, auth.user_id, kind, *id)
                .await
                .map_err(internal)?;
        }
        Target::Stremio(stremio_id) => {
            up_next_dismissals::dismiss(
                &mut *tx,
                auth.user_id,
                up_next_dismissals::STREMIO,
                stremio_id,
            )
            .await
            .map_err(internal)?;
        }
    }
    tx.commit().await.map_err(internal)?;
    Ok(StatusCode::NO_CONTENT)
}

async fn resolve(
    state: &AppState,
    user_id: Uuid,
    body: DismissRequest,
) -> Result<Target, AppError> {
    let not_found = || AppError::NotFound(format!("{} not found", body.media_type));
    match body.media_type.as_str() {
        "movie" => {
            let id = body.media_id.parse().map_err(|_| not_found())?;
            movies::get(&state.pool, id)
                .await
                .map_err(internal)?
                .ok_or_else(not_found)?;
            Ok(Target::Library(up_next_dismissals::MOVIE, id))
        }
        "episode" | "event" => {
            let id = body.media_id.parse().map_err(|_| not_found())?;
            let scope = tv_episodes::list_scopes(&state.pool, &[id])
                .await
                .map_err(internal)?
                .into_iter()
                .next()
                .ok_or_else(not_found)?;
            Ok(Target::Library(up_next_dismissals::SHOW, scope.show_id))
        }
        "stremio" => {
            if stremio_continue::visible_to_user(&state.pool, user_id, &body.media_id)
                .await
                .map_err(internal)?
            {
                Ok(Target::Stremio(body.media_id))
            } else {
                Err(not_found())
            }
        }
        other => Err(AppError::BadRequest(format!(
            "Invalid media_type: {other}. Must be 'movie', 'episode', 'event' or 'stremio'."
        ))),
    }
}

fn internal(e: impl std::fmt::Display) -> AppError {
    AppError::Internal(format!("Failed to dismiss from Up Next: {e}"))
}
