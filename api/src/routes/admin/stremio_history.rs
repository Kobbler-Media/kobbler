//! A profile's pick of titles from the Stremio account's watch history, imported into its own
//! watch state and kept following Stremio on every sync until unpicked.

use axum::{
    Json,
    extract::{Path, State},
    http::StatusCode,
    response::IntoResponse,
};
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::AppState;
use crate::repositories::stremio_history::{self, HistoryRow};
use crate::routes::AuthUser;
use crate::routes::admin::dashboard::format_episode_label;

#[derive(Serialize)]
struct HistoryItem {
    stremio_id: String,
    media_type: String,
    provider: &'static str,
    provider_id: String,
    title: String,
    year: Option<i32>,
    poster_url: Option<String>,
    progress: String,
    #[serde(with = "time::serde::rfc3339")]
    last_watched: time::OffsetDateTime,
    imported: bool,
    import_error: Option<String>,
}

impl From<HistoryRow> for HistoryItem {
    fn from(r: HistoryRow) -> Self {
        let progress = match (r.season, r.episode) {
            (Some(s), Some(e)) => {
                let label = format_episode_label(&s.to_string(), &e.to_string(), None);
                if r.marker_watched {
                    format!("Watched through {label}")
                } else if r.position_ms > 0 {
                    format!("Partway through {label}")
                } else {
                    format!("Up to {label}")
                }
            }
            _ if r.watched => "Watched".into(),
            _ => "In progress".into(),
        };
        Self {
            stremio_id: r.stremio_id,
            media_type: r.media_type,
            provider: "tmdb",
            provider_id: r.tmdb_id,
            title: r.title,
            year: r.year,
            poster_url: r.poster_url,
            progress,
            last_watched: r.last_watched,
            imported: r.imported,
            import_error: r.import_error,
        }
    }
}

#[derive(Deserialize)]
pub struct SelectBody {
    pub stremio_ids: Vec<String>,
}

/// GET /api/v1/admin/stremio/history
pub async fn list_history(State(state): State<AppState>, auth: AuthUser) -> impl IntoResponse {
    match stremio_history::list_for_user(&state.pool, auth.user_id, auth.is_kids).await {
        Ok(rows) => {
            let linked = rows.is_some();
            let items: Vec<HistoryItem> = rows
                .unwrap_or_default()
                .into_iter()
                .map(HistoryItem::from)
                .collect();
            Json(json!({ "linked": linked, "items": items })).into_response()
        }
        Err(e) => internal(e),
    }
}

/// PUT /api/v1/admin/stremio/history/imports
pub async fn select_imports(
    State(state): State<AppState>,
    auth: AuthUser,
    Json(body): Json<SelectBody>,
) -> impl IntoResponse {
    match stremio_history::select(&state.pool, auth.user_id, auth.is_kids, &body.stremio_ids).await
    {
        Ok(selected) => {
            if selected > 0
                && let Err(e) = state
                    .nats
                    .publisher()
                    .enqueue_job("kobbler", "stremio_sync", json!({}))
                    .await
            {
                tracing::warn!("Failed to enqueue Stremio sync: {e}");
            }
            Json(json!({ "selected": selected })).into_response()
        }
        Err(e) => internal(e),
    }
}

/// DELETE /api/v1/admin/stremio/history/imports/{stremio_id}
///
/// Stops following Stremio; what was already imported stays watched.
pub async fn deselect_import(
    State(state): State<AppState>,
    auth: AuthUser,
    Path(stremio_id): Path<String>,
) -> impl IntoResponse {
    match stremio_history::deselect(&state.pool, auth.user_id, &stremio_id).await {
        Ok(_) => StatusCode::NO_CONTENT.into_response(),
        Err(e) => internal(e),
    }
}

fn internal(e: impl std::fmt::Display) -> axum::response::Response {
    (
        StatusCode::INTERNAL_SERVER_ERROR,
        Json(json!({ "error": e.to_string() })),
    )
        .into_response()
}
