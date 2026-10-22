//! The watch history behind For You, and the bulk edits that clean it up — chiefly
//! imported Trakt or Stremio history that belongs to someone else.

use axum::{
    Json,
    extract::{Query, State},
    http::StatusCode,
    response::IntoResponse,
};
use serde::{Deserialize, Serialize};
use serde_json::json;
use time::OffsetDateTime;
use uuid::Uuid;

use crate::AppState;
use crate::images::{ImageMap, endpoint_url};
use crate::repositories::{movie_watch_states, tv_watch_states, watch_history};
use crate::routes::AuthUser;

pub enum AppError {
    BadRequest(String),
    Internal(String),
}

impl IntoResponse for AppError {
    fn into_response(self) -> axum::response::Response {
        let (status, message) = match self {
            AppError::BadRequest(msg) => (StatusCode::BAD_REQUEST, msg),
            AppError::Internal(msg) => (StatusCode::INTERNAL_SERVER_ERROR, msg),
        };
        (status, Json(json!({ "error": message }))).into_response()
    }
}

impl From<crate::repositories::RepositoryError> for AppError {
    fn from(e: crate::repositories::RepositoryError) -> Self {
        AppError::Internal(e.to_string())
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct TitleRef {
    pub media_type: String,
    pub tmdb_id: i64,
}

impl TitleRef {
    pub fn key(&self) -> Result<(String, String), AppError> {
        match self.media_type.as_str() {
            "movie" | "tv" => Ok((self.media_type.clone(), self.tmdb_id.to_string())),
            other => Err(AppError::BadRequest(format!(
                "`media_type` must be movie or tv (got `{other}`)"
            ))),
        }
    }
}

fn split(items: &[TitleRef]) -> Result<(Vec<String>, Vec<String>), AppError> {
    let keys = items
        .iter()
        .map(TitleRef::key)
        .collect::<Result<Vec<_>, _>>()?;
    Ok(keys.into_iter().unzip())
}

#[derive(Debug, Deserialize)]
pub struct HistoryQuery {
    pub limit: Option<i64>,
    pub offset: Option<i64>,
    pub search: Option<String>,
    pub source: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct HistoryItem {
    media_type: String,
    tmdb_id: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    movie_id: Option<Option<Uuid>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    show_id: Option<Option<Uuid>>,
    title: String,
    year: Option<i32>,
    poster_url: Option<String>,
    #[serde(with = "time::serde::rfc3339::option")]
    last_watched_at: Option<OffsetDateTime>,
    sources: Vec<String>,
    excluded: bool,
}

fn history_item(row: watch_history::HistoryRow) -> HistoryItem {
    let is_movie = row.media_type == "movie";
    let images: ImageMap = row
        .images
        .and_then(|v| serde_json::from_value(v).ok())
        .unwrap_or_default();
    let library_poster = row.library_id.and_then(|id| {
        let base = if is_movie {
            "/api/v1/admin/movies/"
        } else {
            "/api/v1/admin/tv/shows/"
        };
        endpoint_url(&images, "poster", base, id, "/poster")
    });
    HistoryItem {
        tmdb_id: row.tmdb_id.as_deref().and_then(|t| t.parse().ok()),
        movie_id: is_movie.then_some(row.library_id),
        show_id: (!is_movie).then_some(row.library_id),
        title: row.title,
        year: row.year,
        poster_url: library_poster.or(row.poster_url),
        last_watched_at: row.last_watched_at,
        sources: row.sources,
        excluded: row.excluded,
        media_type: row.media_type,
    }
}

/// GET /api/v1/admin/watch-history
pub async fn list_history(
    State(state): State<AppState>,
    auth: AuthUser,
    Query(q): Query<HistoryQuery>,
) -> Result<impl IntoResponse, AppError> {
    let source = q.source.as_deref().filter(|s| !s.is_empty());
    if let Some(s) = source
        && !watch_history::SOURCES.contains(&s)
    {
        return Err(AppError::BadRequest(format!(
            "`source` must be one of {} (got `{s}`)",
            watch_history::SOURCES.join(", ")
        )));
    }
    let search = q.search.as_deref().map(str::trim).filter(|s| !s.is_empty());
    let (rows, total) = watch_history::list(
        &state.pool,
        auth.user_id,
        search,
        source,
        q.limit.unwrap_or(50).clamp(1, 500),
        q.offset.unwrap_or(0).max(0),
    )
    .await?;
    let items: Vec<HistoryItem> = rows.into_iter().map(history_item).collect();
    Ok(Json(json!({ "items": items, "total": total })))
}

#[derive(Debug, Deserialize)]
pub struct ExcludeInput {
    pub items: Vec<TitleRef>,
    pub excluded: bool,
}

/// POST /api/v1/admin/watch-history/exclude
pub async fn exclude(
    State(state): State<AppState>,
    auth: AuthUser,
    Json(input): Json<ExcludeInput>,
) -> Result<impl IntoResponse, AppError> {
    let (media_types, tmdb_ids) = split(&input.items)?;
    watch_history::set_excluded(
        &state.pool,
        auth.user_id,
        &media_types,
        &tmdb_ids,
        input.excluded,
    )
    .await?;
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Debug, Deserialize)]
pub struct RemoveInput {
    pub items: Vec<TitleRef>,
}

/// POST /api/v1/admin/watch-history/remove
///
/// Local only: the titles are not unmarked on Trakt, since the history being removed is
/// usually someone else's and the account it came from is theirs.
pub async fn remove(
    State(state): State<AppState>,
    auth: AuthUser,
    Json(input): Json<RemoveInput>,
) -> Result<impl IntoResponse, AppError> {
    let (media_types, tmdb_ids) = split(&input.items)?;
    let of = |kind: &str| -> Vec<String> {
        media_types
            .iter()
            .zip(&tmdb_ids)
            .filter(|(m, _)| *m == kind)
            .map(|(_, t)| t.clone())
            .collect()
    };

    let mut tx = state
        .pool
        .begin()
        .await
        .map_err(|e| AppError::Internal(e.to_string()))?;
    movie_watch_states::clear_for_tmdb(&mut *tx, auth.user_id, &of("movie")).await?;
    for show_id in watch_history::show_ids_for_tmdb(&mut *tx, &of("tv")).await? {
        tv_watch_states::bulk_mark_show_unwatched(&mut *tx, show_id, auth.user_id).await?;
    }
    // Stremio's own history cannot be deleted from here, so it is kept out of For You.
    let (stremio_types, stremio_ids): (Vec<String>, Vec<String>) =
        watch_history::stremio_titles(&mut *tx, auth.user_id, &media_types, &tmdb_ids)
            .await?
            .into_iter()
            .unzip();
    watch_history::set_excluded(&mut *tx, auth.user_id, &stremio_types, &stremio_ids, true).await?;
    tx.commit()
        .await
        .map_err(|e| AppError::Internal(e.to_string()))?;
    Ok(StatusCode::NO_CONTENT)
}
