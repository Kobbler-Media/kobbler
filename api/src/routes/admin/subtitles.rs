//! Subtitle search and download admin route handlers.

use axum::{
    Json,
    extract::{Path, State},
    http::StatusCode,
    response::IntoResponse,
};
use serde::{Deserialize, Serialize};
use serde_json::json;
use uuid::Uuid;

use crate::AppState;
use crate::repositories::{
    movie_files, movies, tv_episode_files, tv_episodes, tv_seasons, tv_shows,
};
use crate::routes::AuthUser;
use crate::subtitle_providers::{
    EpisodeSubtitleQuery, MovieSubtitleQuery, SubtitleProviderRegistry,
    download::download_and_save_subtitle,
};

// =============================================================================
// Error Type
// =============================================================================

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

// =============================================================================
// Request/Response Types
// =============================================================================

#[derive(Debug, Deserialize)]
pub struct SubtitleSearchRequest {
    pub languages: Vec<String>,
    #[serde(default)]
    pub forced_only: bool,
}

#[derive(Debug, Deserialize)]
pub struct SubtitleDownloadRequest {
    pub provider: String,
    pub subtitle_id: String,
    pub movie_file_id: Option<Uuid>,
    pub episode_file_id: Option<Uuid>,
    #[serde(default)]
    pub language: Option<String>,
    #[serde(default)]
    pub is_forced: bool,
    #[serde(default)]
    pub is_hearing_impaired: bool,
}

#[derive(Debug, Serialize)]
struct DownloadResult {
    id: Uuid,
    file_path: String,
}

// =============================================================================
// Search Movie Subtitles
// =============================================================================

/// POST /api/v1/admin/movies/:id/subtitles/search
pub async fn search_movie_subtitles(
    State(state): State<AppState>,
    _auth: AuthUser,
    Path(movie_id): Path<Uuid>,
    Json(body): Json<SubtitleSearchRequest>,
) -> Result<impl IntoResponse, AppError> {
    if body.languages.is_empty() {
        return Err(AppError::BadRequest(
            "At least one language is required".into(),
        ));
    }

    let movie = movies::get(&state.pool, movie_id)
        .await
        .map_err(|e| AppError::Internal(format!("Failed to get movie: {e}")))?
        .ok_or_else(|| AppError::NotFound("Movie not found".into()))?;

    let imdb_id = movie
        .identifiers
        .get("imdb_id")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());
    let tmdb_id = movie
        .identifiers
        .get("tmdb_id")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());

    let query = MovieSubtitleQuery {
        title: movie.title,
        year: movie.year.and_then(|y| u16::try_from(y).ok()),
        imdb_id,
        tmdb_id,
        languages: body.languages,
        forced_only: body.forced_only,
    };

    let registry = SubtitleProviderRegistry::new(state.pool.clone());
    let results = registry.search_movie(&query, None).await;

    Ok(Json(results))
}

// =============================================================================
// Search Episode Subtitles
// =============================================================================

/// POST /api/v1/admin/tv/episodes/:id/subtitles/search
pub async fn search_episode_subtitles(
    State(state): State<AppState>,
    _auth: AuthUser,
    Path(episode_id): Path<Uuid>,
    Json(body): Json<SubtitleSearchRequest>,
) -> Result<impl IntoResponse, AppError> {
    if body.languages.is_empty() {
        return Err(AppError::BadRequest(
            "At least one language is required".into(),
        ));
    }

    // Navigate episode -> season -> show to get identifiers
    let episode = tv_episodes::get(&state.pool, episode_id)
        .await
        .map_err(|e| AppError::Internal(format!("Failed to get episode: {e}")))?
        .ok_or_else(|| AppError::NotFound("Episode not found".into()))?;

    let season = tv_seasons::get(&state.pool, episode.season_id)
        .await
        .map_err(|e| AppError::Internal(format!("Failed to get season: {e}")))?
        .ok_or_else(|| AppError::Internal("Season not found for episode".into()))?;

    let show = tv_shows::get(&state.pool, season.show_id)
        .await
        .map_err(|e| AppError::Internal(format!("Failed to get show: {e}")))?
        .ok_or_else(|| AppError::Internal("Show not found for season".into()))?;

    let show_imdb_id = show
        .identifiers
        .get("imdb_id")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());
    let show_tmdb_id = show
        .identifiers
        .get("tmdb_id")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());

    let season_num: u32 = season
        .season_identifier
        .parse()
        .unwrap_or(season.season_sort_order as u32);
    let episode_num: u32 = episode
        .episode_identifier
        .parse()
        .unwrap_or(episode.episode_sort_order as u32);

    let query = EpisodeSubtitleQuery {
        show_title: show.title,
        show_imdb_id,
        show_tmdb_id,
        season: season_num,
        episode: episode_num,
        languages: body.languages,
        forced_only: body.forced_only,
    };

    let registry = SubtitleProviderRegistry::new(state.pool.clone());
    let results = registry.search_episode(&query, None).await;

    Ok(Json(results))
}

// =============================================================================
// Download Subtitle
// =============================================================================

/// POST /api/v1/admin/subtitles/download
pub async fn download_subtitle(
    State(state): State<AppState>,
    _auth: AuthUser,
    Json(body): Json<SubtitleDownloadRequest>,
) -> Result<impl IntoResponse, AppError> {
    if body.movie_file_id.is_none() && body.episode_file_id.is_none() {
        return Err(AppError::BadRequest(
            "Either movie_file_id or episode_file_id is required".into(),
        ));
    }

    // Look up the file to get its path
    let video_file_path = if let Some(mf_id) = body.movie_file_id {
        let file = movie_files::get(&state.pool, mf_id)
            .await
            .map_err(|e| AppError::Internal(format!("Failed to get movie file: {e}")))?
            .ok_or_else(|| AppError::NotFound("Movie file not found".into()))?;
        file.file_path
    } else if let Some(ef_id) = body.episode_file_id {
        let file = tv_episode_files::get(&state.pool, ef_id)
            .await
            .map_err(|e| AppError::Internal(format!("Failed to get episode file: {e}")))?
            .ok_or_else(|| AppError::NotFound("Episode file not found".into()))?;
        file.file_path
    } else {
        unreachable!()
    };

    let registry = SubtitleProviderRegistry::new(state.pool.clone());

    let (sub_id, file_path) = download_and_save_subtitle(
        &state.pool,
        &registry,
        &body.provider,
        &body.subtitle_id,
        &video_file_path,
        body.movie_file_id,
        body.episode_file_id,
        body.language.as_deref(),
        body.is_forced,
        body.is_hearing_impaired,
    )
    .await
    .map_err(|e| AppError::Internal(format!("Failed to download subtitle: {e}")))?;

    Ok((
        StatusCode::CREATED,
        Json(DownloadResult {
            id: sub_id,
            file_path,
        }),
    ))
}
