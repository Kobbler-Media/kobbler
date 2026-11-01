//! Movie admin route handlers.
//!
//! CRUD, want/unwant, file management, metadata refresh, and watch state
//! for the movie library.

use axum::{
    Json,
    extract::{Path, Query, State},
    http::{HeaderMap, StatusCode},
    response::IntoResponse,
};
use serde::{Deserialize, Serialize};
use serde_json::json;
use time::OffsetDateTime;
use uuid::Uuid;

use tracing::{info, warn};

use crate::AppState;
use crate::LibraryEvent;
use crate::file_link_mode::{AcquireQuery, FileLinkMode};
use crate::images::{self, ImageEntry, endpoint_url};
use crate::kobload::CreateDownloadRequest as KobloadDownloadRequest;
use crate::repositories::{
    PaginationDirection, audio_tracks, external_subtitles, media_roots, movie_files,
    movie_watch_states, movies, subtitle_tracks, wanted_attempts,
};
use crate::routes::helpers::build_kobload_client;
use crate::routes::library::helpers::{
    NameCursor, decode_name_cursor, encode_name_cursor, paginated_window,
};
use crate::routes::library::types::PaginatedResponse;
use crate::routes::{AuthUser, PlaybackAuth};
use crate::video_metadata::release_dates::{self, RegionalReleaseDates};

// =============================================================================
// Error Type
// =============================================================================

pub enum AppError {
    Forbidden,
    NotFound(String),
    BadRequest(String),
    Internal(String),
}

impl IntoResponse for AppError {
    fn into_response(self) -> axum::response::Response {
        let (status, message) = match self {
            AppError::Forbidden => (StatusCode::FORBIDDEN, "forbidden".to_string()),
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
pub struct ListMoviesQuery {
    pub want_status: Option<String>,
    pub is_in_library: Option<bool>,
    pub search: Option<String>,
    pub limit: Option<i64>,
    pub cursor: Option<String>,
    pub direction: Option<String>,
    pub around: Option<String>,
    pub around_before: Option<i64>,
    pub sort: Option<String>,
    pub genre: Option<String>,
}

#[derive(Debug, Serialize)]
struct MovieListResponse {
    id: Uuid,
    title: String,
    sort_title: Option<String>,
    year: Option<i32>,
    poster_url: Option<String>,
    backdrop_url: Option<String>,
    logo_url: Option<String>,
    identifiers: serde_json::Value,
    is_in_library: bool,
    want_status: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    watched: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    watch_position_seconds: Option<i32>,
    #[serde(with = "time::serde::rfc3339::option")]
    streamed_at: Option<OffsetDateTime>,
    #[serde(with = "time::serde::rfc3339")]
    created_at: OffsetDateTime,
    #[serde(with = "time::serde::rfc3339")]
    updated_at: OffsetDateTime,
}

#[derive(Debug, Serialize)]
struct CastResponse {
    id: Uuid,
    person_id: Uuid,
    person_name: String,
    person_image_url: Option<String>,
    character_name: Option<String>,
    cast_order: Option<i32>,
}

#[derive(Debug, Serialize)]
struct CrewResponse {
    person_id: Uuid,
    person_name: String,
    department: String,
    job: String,
}

#[derive(Debug, Serialize)]
struct MovieDetailResponse {
    id: Uuid,
    title: String,
    sort_title: Option<String>,
    original_title: Option<String>,
    year: Option<i32>,
    runtime: Option<i32>,
    description: Option<String>,
    tagline: Option<String>,
    poster_url: Option<String>,
    backdrop_url: Option<String>,
    logo_url: Option<String>,
    identifiers: serde_json::Value,
    genres: Option<Vec<String>>,
    rating: Option<f64>,
    rating_count: Option<i32>,
    content_rating: Option<String>,
    kids_override: Option<bool>,
    kids_allowed: bool,
    #[serde(with = "crate::serde_date::option_date")]
    release_date: Option<time::Date>,
    release_dates: Option<RegionalReleaseDates>,
    is_released: Option<bool>,
    quality_profile_id: Option<Uuid>,
    preferred_languages: Option<Vec<String>>,
    media_root_id: Option<Uuid>,
    is_in_library: bool,
    want_status: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    watched: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    watch_position_seconds: Option<i32>,
    in_watchlist: bool,
    #[serde(with = "time::serde::rfc3339::option")]
    streamed_at: Option<OffsetDateTime>,
    #[serde(with = "time::serde::rfc3339")]
    created_at: OffsetDateTime,
    #[serde(with = "time::serde::rfc3339")]
    updated_at: OffsetDateTime,
    cast: Vec<CastResponse>,
    crew: Vec<CrewResponse>,
}

#[derive(Debug, Deserialize)]
pub struct CreateMovieRequest {
    pub title: String,
    pub sort_title: Option<String>,
    pub original_title: Option<String>,
    pub year: Option<i32>,
    pub runtime: Option<i32>,
    pub description: Option<String>,
    pub tagline: Option<String>,
    pub poster_url: Option<String>,
    pub backdrop_url: Option<String>,
    pub identifiers: Option<serde_json::Value>,
    pub genres: Option<Vec<String>>,
    pub rating: Option<f64>,
    pub rating_count: Option<i32>,
    pub content_rating: Option<String>,
    #[serde(default, with = "crate::serde_date::option_date")]
    pub release_date: Option<time::Date>,
    pub is_released: Option<bool>,
    pub quality_profile_id: Option<Uuid>,
    pub preferred_languages: Option<Vec<String>>,
    pub media_root_id: Option<Uuid>,
}

#[derive(Debug, Deserialize)]
pub struct UpdateMovieRequest {
    /// Whether a Kids profile may see this title regardless of its rating; `null` defers
    /// to the rating again. Admin only.
    #[serde(default, deserialize_with = "crate::serde_patch::double_option")]
    pub kids_override: Option<Option<bool>>,
    pub title: Option<String>,
    pub sort_title: Option<String>,
    pub original_title: Option<String>,
    pub year: Option<i32>,
    pub runtime: Option<i32>,
    pub description: Option<String>,
    pub tagline: Option<String>,
    pub poster_url: Option<String>,
    pub backdrop_url: Option<String>,
    pub logo_url: Option<String>,
    pub identifiers: Option<serde_json::Value>,
    pub genres: Option<Vec<String>>,
    pub rating: Option<f64>,
    pub rating_count: Option<i32>,
    pub content_rating: Option<String>,
    #[serde(default, with = "crate::serde_date::option_date")]
    pub release_date: Option<time::Date>,
    pub is_released: Option<bool>,
    pub quality_profile_id: Option<Option<Uuid>>,
    pub preferred_languages: Option<Option<Vec<String>>>,
    pub media_root_id: Option<Option<Uuid>>,
}

#[derive(Debug, Serialize)]
struct MovieFileResponse {
    id: Uuid,
    movie_id: Uuid,
    file_path: String,
    file_size: Option<i64>,
    format: String,
    quality: Option<String>,
    languages: Option<Vec<String>>,
    subtitles: Option<Vec<String>>,
    source_type: Option<String>,
    source_metadata: Option<serde_json::Value>,
    available: bool,
    #[serde(with = "time::serde::rfc3339")]
    created_at: OffsetDateTime,
}

#[derive(Debug, Deserialize)]
pub struct AddMovieFileRequest {
    pub file_path: String,
    pub file_size: Option<i64>,
    pub format: String,
    pub quality: Option<serde_json::Value>,
    pub languages: Option<Vec<String>>,
    pub subtitles: Option<Vec<String>>,
    pub source_type: Option<String>,
    pub source_metadata: Option<serde_json::Value>,
}

#[derive(Debug, Serialize)]
struct WatchStateResponse {
    id: Uuid,
    movie_id: Uuid,
    user_id: Uuid,
    watched: bool,
    #[serde(with = "time::serde::rfc3339::option")]
    watched_at: Option<OffsetDateTime>,
    position_seconds: Option<i32>,
    rating: Option<f64>,
    #[serde(with = "time::serde::rfc3339")]
    created_at: OffsetDateTime,
    #[serde(with = "time::serde::rfc3339")]
    updated_at: OffsetDateTime,
}

#[derive(Debug, Deserialize)]
pub struct UpdateWatchStateRequest {
    pub watched: bool,
    #[serde(default, with = "time::serde::rfc3339::option")]
    pub watched_at: Option<OffsetDateTime>,
    pub position_seconds: Option<i32>,
    pub rating: Option<f64>,
}

#[derive(Debug, Deserialize)]
pub struct MovieFilePath {
    pub id: Uuid,
    pub file_id: Uuid,
}

#[derive(Debug, Deserialize)]
pub struct CastInputRequest {
    pub person_id: Uuid,
    pub character_name: Option<String>,
    pub cast_order: Option<i32>,
}

#[derive(Debug, Deserialize)]
pub struct CrewInputRequest {
    pub person_id: Uuid,
    pub department: String,
    pub job: String,
}

// =============================================================================
// List Movies
// =============================================================================

/// GET /api/v1/admin/movies
pub async fn list_movies(
    State(state): State<AppState>,
    auth: AuthUser,
    Query(params): Query<ListMoviesQuery>,
) -> Result<impl IntoResponse, AppError> {
    let limit = params.limit.unwrap_or(50).min(200);

    // Handle "around" mode: fetch items before + target + items after
    if let Some(ref around_id_str) = params.around {
        let around_id: Uuid = around_id_str
            .parse()
            .map_err(|_| AppError::BadRequest("Invalid around ID".into()))?;
        let before_count = params.around_before.unwrap_or(0).max(0);
        let after_limit = limit - before_count;

        // Get the target movie for cursor info
        let target = movies::get(&state.pool, around_id)
            .await
            .map_err(|e| AppError::Internal(format!("Failed to get movie: {e}")))?
            .ok_or_else(|| AppError::NotFound("Movie not found".into()))?;

        let target_sort = target
            .sort_title
            .as_deref()
            .unwrap_or(&target.title)
            .to_string();

        // One row past `before_count` doubles as the "is anything before this window"
        // probe, so around_before=0 still costs a single LIMIT 1 index lookup.
        let before_filters = movies::MovieListFilters {
            want_status: params.want_status.clone(),
            is_in_library: params.is_in_library,
            search: params.search.clone(),
            limit: before_count + 1,
            cursor_title: Some(target_sort.clone()),
            cursor_id: Some(around_id),
            direction: PaginationDirection::Backward,
            cursor_inclusive: false,
            sort: None,
            genre: params.genre.clone(),
            kids_only: auth.is_kids,
        };
        let mut before_rows = movies::list(&state.pool, &before_filters)
            .await
            .map_err(|e| AppError::Internal(format!("Failed to list movies: {e}")))?;

        let has_previous = before_rows.len() as i64 > before_count;
        let overshoot = before_rows.len().saturating_sub(before_count as usize);
        before_rows.drain(..overshoot);

        // Forward window resumes after the last before-item; with no before-items it
        // starts *at* the target, so the cursor has to be inclusive.
        let (cursor_title, cursor_id, cursor_inclusive) = match before_rows.last() {
            Some(last_before) => (
                last_before
                    .sort_title
                    .as_deref()
                    .unwrap_or(&last_before.title)
                    .to_string(),
                last_before.id,
                false,
            ),
            None => (target_sort.clone(), around_id, true),
        };

        let after_filters = movies::MovieListFilters {
            want_status: params.want_status.clone(),
            is_in_library: params.is_in_library,
            search: params.search.clone(),
            limit: after_limit + 1,
            cursor_title: Some(cursor_title),
            cursor_id: Some(cursor_id),
            direction: PaginationDirection::Forward,
            cursor_inclusive,
            sort: None,
            genre: params.genre.clone(),
            kids_only: auth.is_kids,
        };

        let after_rows = movies::list(&state.pool, &after_filters)
            .await
            .map_err(|e| AppError::Internal(format!("Failed to list movies: {e}")))?;

        let has_more = after_rows.len() as i64 > after_limit;
        let after_items: Vec<_> = after_rows.into_iter().take(after_limit as usize).collect();

        let mut combined = before_rows;
        combined.extend(after_items);

        let prev_cursor = if has_previous {
            combined.first().map(|first| {
                let sort = first.sort_title.as_deref().unwrap_or(&first.title);
                encode_name_cursor(&NameCursor {
                    id: first.id,
                    name: sort.to_string(),
                })
            })
        } else {
            None
        };

        let next_cursor = if has_more {
            combined.last().map(|last| {
                let sort = last.sort_title.as_deref().unwrap_or(&last.title);
                encode_name_cursor(&NameCursor {
                    id: last.id,
                    name: sort.to_string(),
                })
            })
        } else {
            None
        };

        let mut items = to_movie_list_responses(combined);
        let movie_ids: Vec<Uuid> = items.iter().map(|m| m.id).collect();
        if let Ok(ws) =
            movie_watch_states::list_for_movie_ids(&state.pool, &movie_ids, auth.user_id).await
        {
            annotate_movie_list_with_watch_states(&mut items, &ws);
        }
        return Ok(Json(PaginatedResponse {
            items,
            next_cursor,
            prev_cursor,
            total_count: None,
        }));
    }

    // Normal cursor-based pagination
    let direction = match params.direction.as_deref() {
        Some("backward") => PaginationDirection::Backward,
        _ => PaginationDirection::Forward,
    };

    let (cursor_title, cursor_id) = params
        .cursor
        .as_deref()
        .and_then(decode_name_cursor)
        .map(|c| (Some(c.name), Some(c.id)))
        .unwrap_or((None, None));

    let is_first_page = cursor_title.is_none();

    // Cursor pagination only exists for title sort; the repo ignores cursors under
    // any other sort, so a cursor there would make "load more" re-fetch page one.
    let sort_by_title = params.sort.as_deref() != Some("created_at");

    let filters = movies::MovieListFilters {
        want_status: params.want_status,
        is_in_library: params.is_in_library,
        search: params.search,
        limit: limit + 1, // +1 for has_more detection
        cursor_title,
        cursor_id,
        direction,
        cursor_inclusive: false,
        sort: params.sort,
        genre: params.genre.clone(),
        kids_only: auth.is_kids,
    };

    // Count on first page only
    let total_count = if is_first_page {
        Some(
            movies::count(&state.pool, &filters)
                .await
                .map_err(|e| AppError::Internal(format!("Failed to count movies: {e}")))?,
        )
    } else {
        None
    };

    let rows = movies::list(&state.pool, &filters)
        .await
        .map_err(|e| AppError::Internal(format!("Failed to list movies: {e}")))?;

    let (items, has_previous, has_next) = paginated_window(rows, limit, direction, is_first_page);

    let next_cursor = if has_next && sort_by_title {
        items.last().map(|last| {
            let sort = last.sort_title.as_deref().unwrap_or(&last.title);
            encode_name_cursor(&NameCursor {
                id: last.id,
                name: sort.to_string(),
            })
        })
    } else {
        None
    };

    let prev_cursor = if has_previous && sort_by_title {
        items.first().map(|first| {
            let sort = first.sort_title.as_deref().unwrap_or(&first.title);
            encode_name_cursor(&NameCursor {
                id: first.id,
                name: sort.to_string(),
            })
        })
    } else {
        None
    };

    let mut response_items = to_movie_list_responses(items);
    let movie_ids: Vec<Uuid> = response_items.iter().map(|m| m.id).collect();
    if let Ok(ws) =
        movie_watch_states::list_for_movie_ids(&state.pool, &movie_ids, auth.user_id).await
    {
        annotate_movie_list_with_watch_states(&mut response_items, &ws);
    }

    Ok(Json(PaginatedResponse {
        items: response_items,
        next_cursor,
        prev_cursor,
        total_count,
    }))
}

fn to_movie_list_responses(rows: Vec<movies::MovieRow>) -> Vec<MovieListResponse> {
    rows.into_iter()
        .map(|r| {
            let poster_url = endpoint_url(
                &r.images,
                "poster",
                "/api/v1/admin/movies/",
                r.id,
                "/poster",
            );
            let backdrop_url = endpoint_url(
                &r.images,
                "backdrop",
                "/api/v1/admin/movies/",
                r.id,
                "/backdrop",
            );
            let logo_url = endpoint_url(&r.images, "logo", "/api/v1/admin/movies/", r.id, "/logo");
            MovieListResponse {
                id: r.id,
                title: r.title,
                sort_title: r.sort_title,
                year: r.year,
                poster_url,
                backdrop_url,
                logo_url,
                identifiers: r.identifiers,
                is_in_library: r.is_in_library,
                want_status: r.want_status,
                watched: None,
                watch_position_seconds: None,
                streamed_at: r.streamed_at,
                created_at: r.created_at,
                updated_at: r.updated_at,
            }
        })
        .collect()
}

fn annotate_movie_list_with_watch_states(
    items: &mut [MovieListResponse],
    watch_states: &[movie_watch_states::WatchStateRow],
) {
    let ws_map: std::collections::HashMap<Uuid, &movie_watch_states::WatchStateRow> =
        watch_states.iter().map(|ws| (ws.movie_id, ws)).collect();
    for item in items.iter_mut() {
        if let Some(ws) = ws_map.get(&item.id) {
            item.watched = Some(ws.watched);
            item.watch_position_seconds = ws.position_seconds;
        }
    }
}

// =============================================================================
// Create Movie
// =============================================================================

/// POST /api/v1/admin/movies
pub async fn create_movie(
    State(state): State<AppState>,
    _auth: AuthUser,
    Json(body): Json<CreateMovieRequest>,
) -> Result<impl IntoResponse, AppError> {
    if body.title.trim().is_empty() {
        return Err(AppError::BadRequest("Title is required".into()));
    }

    let mut img_map = std::collections::HashMap::new();
    if let Some(url) = body.poster_url {
        img_map.insert(
            "poster".to_string(),
            ImageEntry {
                source_url: Some(url),
                cached_at: None,
            },
        );
    }
    if let Some(url) = body.backdrop_url {
        img_map.insert(
            "backdrop".to_string(),
            ImageEntry {
                source_url: Some(url),
                cached_at: None,
            },
        );
    }

    // Extract provider ID from identifiers for upsert + auto-enrich
    // Try known provider keys in priority order
    let provider_info = body.identifiers.as_ref().and_then(|ids| {
        for provider in &["tmdb", "tvdb"] {
            let key = format!("{provider}_id");
            if let Some(val) = ids.get(&key).and_then(|v| v.as_str()) {
                return Some((provider.to_string(), val.to_string()));
            }
        }
        None
    });

    // Auto-assign default media root if none provided
    let media_root_id = match body.media_root_id {
        Some(id) => Some(id),
        None => media_roots::get_default_for_type(&state.pool, "movie")
            .await
            .ok()
            .flatten()
            .map(|r| r.id),
    };

    let input = movies::MovieInput {
        title: body.title,
        sort_title: body.sort_title,
        original_title: body.original_title,
        year: body.year,
        runtime: body.runtime,
        description: body.description,
        tagline: body.tagline,
        images: if img_map.is_empty() {
            None
        } else {
            Some(img_map)
        },
        identifiers: body.identifiers,
        genres: body.genres,
        rating: body.rating,
        rating_count: body.rating_count,
        content_rating: body.content_rating,
        release_date: body.release_date,
        is_released: body.is_released,
        quality_profile_id: body.quality_profile_id,
        preferred_languages: body.preferred_languages,
        media_root_id,
        streamed_at: None,
    };

    // Use upsert when a provider ID is available to prevent duplicates
    let id = if let Some((ref ptype, ref pid)) = provider_info {
        movies::upsert_by_provider(&state.pool, ptype, pid, input)
            .await
            .map_err(|e| AppError::Internal(format!("Failed to upsert movie: {e}")))?
    } else {
        movies::create(&state.pool, input)
            .await
            .map_err(|e| AppError::Internal(format!("Failed to create movie: {e}")))?
    };

    // Auto-enrich (cast, crew, images) via shared enrichment logic
    if provider_info.is_some()
        && crate::jobs::handlers::movie_enrich::enrich_movie(&state.pool, id, false)
            .await
            .is_err()
    {
        warn!(movie_id = %id, "Auto-enrich failed, movie created with basic metadata");
    }

    let _ = state.library_events.send(LibraryEvent::movie_updated(id));

    Ok((StatusCode::CREATED, Json(json!({ "id": id }))))
}

// =============================================================================
// Get Movie
// =============================================================================

/// GET /api/v1/admin/movies/:id
pub async fn get_movie(
    State(state): State<AppState>,
    auth: AuthUser,
    Path(id): Path<Uuid>,
) -> Result<impl IntoResponse, AppError> {
    let detail = movies::get_with_details(&state.pool, id)
        .await
        .map_err(|e| AppError::Internal(format!("Failed to get movie: {e}")))?
        .filter(|d| !auth.is_kids || d.kids_allowed)
        .ok_or_else(|| AppError::NotFound("Movie not found".into()))?;

    let region = state.runtime_config.discover_region().await;
    let mut response = movie_detail_response(detail, &region);

    if let Ok(Some(ws)) = movie_watch_states::get(&state.pool, id, auth.user_id).await {
        response.watched = Some(ws.watched);
        response.watch_position_seconds = ws.position_seconds;
    }
    response.in_watchlist =
        crate::repositories::watchlist::contains(&state.pool, auth.user_id, "movie", id)
            .await
            .unwrap_or(false);

    Ok(Json(response))
}

// =============================================================================
// Update Movie
// =============================================================================

/// PATCH /api/v1/admin/movies/:id
pub async fn update_movie(
    State(state): State<AppState>,
    auth: AuthUser,
    Path(id): Path<Uuid>,
    Json(body): Json<UpdateMovieRequest>,
) -> Result<impl IntoResponse, AppError> {
    if body.kids_override.is_some() && auth.require_admin().is_err() {
        return Err(AppError::Forbidden);
    }
    let kids_override = body.kids_override;
    let mut img_map = std::collections::HashMap::new();
    if let Some(url) = body.poster_url {
        img_map.insert(
            "poster".to_string(),
            ImageEntry {
                source_url: Some(url),
                cached_at: None,
            },
        );
    }
    if let Some(url) = body.backdrop_url {
        img_map.insert(
            "backdrop".to_string(),
            ImageEntry {
                source_url: Some(url),
                cached_at: None,
            },
        );
    }
    if let Some(url) = body.logo_url {
        img_map.insert(
            "logo".to_string(),
            ImageEntry {
                source_url: Some(url),
                cached_at: None,
            },
        );
    }

    let input = movies::MovieUpdate {
        title: body.title,
        sort_title: body.sort_title,
        original_title: body.original_title,
        year: body.year,
        runtime: body.runtime,
        description: body.description,
        tagline: body.tagline,
        images: if img_map.is_empty() {
            None
        } else {
            Some(img_map)
        },
        identifiers: body.identifiers,
        genres: body.genres,
        rating: body.rating,
        rating_count: body.rating_count,
        content_rating: body.content_rating,
        release_date: body.release_date,
        release_dates: None,
        is_released: body.is_released,
        quality_profile_id: body.quality_profile_id,
        alternate_titles: None,
        preferred_languages: body.preferred_languages,
        media_root_id: body.media_root_id,
        streamed_at: None,
    };

    let updated = movies::update(&state.pool, id, input)
        .await
        .map_err(|e| AppError::Internal(format!("Failed to update movie: {e}")))?;

    if !updated {
        return Err(AppError::NotFound("Movie not found".into()));
    }

    if let Some(kids_override) = kids_override {
        movies::set_kids_override(&state.pool, id, kids_override)
            .await
            .map_err(|e| AppError::Internal(format!("Failed to set kids override: {e}")))?;
    }

    let _ = state.library_events.send(LibraryEvent::movie_updated(id));

    Ok(StatusCode::NO_CONTENT)
}

// =============================================================================
// Delete Movie
// =============================================================================

/// DELETE /api/v1/admin/movies/:id
pub async fn delete_movie(
    State(state): State<AppState>,
    auth: AuthUser,
    Path(id): Path<Uuid>,
) -> Result<impl IntoResponse, AppError> {
    // Capture title before delete for activity log
    let movie_title = sqlx::query_scalar!("SELECT title FROM movies WHERE id = $1", id)
        .fetch_optional(&state.pool)
        .await
        .ok()
        .flatten();

    // Collect file paths before CASCADE delete removes movie_files rows
    let files = movie_files::list_by_movie(&state.pool, id)
        .await
        .unwrap_or_default();
    let file_paths: Vec<String> = files.iter().map(|f| f.file_path.clone()).collect();
    let file_ids: Vec<Uuid> = files.iter().map(|f| f.id).collect();

    let deleted = movies::delete(&state.pool, id)
        .await
        .map_err(|e| AppError::Internal(format!("Failed to delete movie: {e}")))?;

    if !deleted {
        return Err(AppError::NotFound("Movie not found".into()));
    }

    // Watchlist rows are polymorphic, so nothing cascades them.
    if let Err(e) = crate::repositories::watchlist::remove_for_media(&state.pool, "movie", id).await
    {
        tracing::warn!(error = %e, movie_id = %id, "failed to clear watchlist rows for deleted movie");
    }

    // Drop the persistent episode_cache/movie/{id}/ copy. Best-effort — the
    // periodic gc job catches any stragglers.
    if let Err(e) = crate::media_cache::episode_cache::remove(
        &state.config.storage_root,
        &crate::media_cache::episode_cache::MediaKind::Movie { movie_id: id },
    ) {
        tracing::warn!(error = %e, movie_id = %id, "failed to remove episode_cache for deleted movie");
    }

    let _ = state.library_events.send(LibraryEvent::movie_updated(id));

    // Record activity
    if let Some(title) = &movie_title {
        crate::routes::library::helpers::record_activity(
            &state.pool,
            "deleted",
            Some("movie"),
            Some(id),
            title,
            None,
            None,
            None,
            Some(auth.user_id),
        )
        .await;
    }

    // Best-effort cleanup of physical files, thumbnails, and per-file media_cache
    crate::repositories::cleanup_replaced_files(&file_paths).await;
    for fid in file_ids {
        crate::repositories::cleanup_thumbnail_dir(&state.config.storage_root, fid).await;
        if let Err(e) = crate::media_cache::remove_per_file(&state.config.storage_root, fid) {
            tracing::warn!(error = %e, file_id = %fid, "failed to remove per-file media_cache");
        }
    }

    Ok(StatusCode::NO_CONTENT)
}

// =============================================================================
// Want Movie
// =============================================================================

/// POST /api/v1/admin/movies/:id/want
pub async fn want_movie(
    State(state): State<AppState>,
    auth: AuthUser,
    Path(id): Path<Uuid>,
    Query(copy): Query<AcquireQuery>,
) -> Result<impl IntoResponse, AppError> {
    // Verify movie exists
    movies::get(&state.pool, id)
        .await
        .map_err(|e| AppError::Internal(format!("Failed to get movie: {e}")))?
        .ok_or_else(|| AppError::NotFound("Movie not found".into()))?;

    movies::set_want_status(&state.pool, id, "wanted")
        .await
        .map_err(|e| AppError::Internal(format!("Failed to set want status: {e}")))?;

    // Enqueue movie_want job via NATS
    let _ = state
        .nats
        .publisher()
        .enqueue_job(
            "kobbler",
            "movie_want",
            json!({
                "movie_id": id,
                "user_id": auth.user_id,
                "additional_copy": copy.additional_copy,
                "manual": copy.manual,
            }),
        )
        .await;

    let _ = state.library_events.send(LibraryEvent::movie_updated(id));

    // Check if we have a complete debrid cache for this movie
    if let Ok(Some(session)) = sqlx::query!(
        r#"
        SELECT id, cache_path AS "cache_path?", filesize
        FROM debrid_stream_sessions
        WHERE media_type = 'movie' AND media_id = $1 AND cache_path IS NOT NULL
        ORDER BY updated_at DESC
        LIMIT 1
        "#,
        id,
    )
    .fetch_optional(&state.pool)
    .await
        && let Some(ref cache_path) = session.cache_path
    {
        let path = std::path::PathBuf::from(cache_path);
        if let Ok(meta) = tokio::fs::metadata(&path).await
            && meta.len() as i64 >= session.filesize - 1024
        {
            info!(movie_id = %id, cache_path = %cache_path, "Complete debrid cache available for wanted movie");
            return Ok(Json(json!({
                "cached_stream_available": true,
                "stream_session_id": session.id,
            }))
            .into_response());
        }
    }

    Ok(StatusCode::NO_CONTENT.into_response())
}

// =============================================================================
// Unwant Movie
// =============================================================================

/// DELETE /api/v1/admin/movies/:id/want
pub async fn unwant_movie(
    State(state): State<AppState>,
    _auth: AuthUser,
    Path(id): Path<Uuid>,
) -> Result<impl IntoResponse, AppError> {
    // Verify movie exists
    movies::get(&state.pool, id)
        .await
        .map_err(|e| AppError::Internal(format!("Failed to get movie: {e}")))?
        .ok_or_else(|| AppError::NotFound("Movie not found".into()))?;

    movies::set_want_status(&state.pool, id, "none")
        .await
        .map_err(|e| AppError::Internal(format!("Failed to set want status: {e}")))?;

    let _ = state.library_events.send(LibraryEvent::movie_updated(id));

    Ok(StatusCode::NO_CONTENT)
}

// =============================================================================
// List Movie Files
// =============================================================================

/// GET /api/v1/admin/movies/:id/files
pub async fn list_movie_files(
    State(state): State<AppState>,
    _auth: AuthUser,
    Path(id): Path<Uuid>,
) -> Result<impl IntoResponse, AppError> {
    let files = movie_files::list_by_movie(&state.pool, id)
        .await
        .map_err(|e| AppError::Internal(format!("Failed to list movie files: {e}")))?;

    let response: Vec<MovieFileResponse> = files
        .into_iter()
        .map(|f| MovieFileResponse {
            id: f.id,
            movie_id: f.movie_id,
            file_path: f.file_path,
            file_size: f.file_size,
            format: f.format,
            quality: flatten_quality(f.quality.as_ref()),
            languages: f.languages,
            subtitles: f.subtitles,
            source_type: f.source_type,
            source_metadata: f.source_metadata,
            available: f.available,
            created_at: f.created_at,
        })
        .collect();

    Ok(Json(response))
}

// =============================================================================
// Add Movie File
// =============================================================================

/// POST /api/v1/admin/movies/:id/files
pub async fn add_movie_file(
    State(state): State<AppState>,
    _auth: AuthUser,
    Path(movie_id): Path<Uuid>,
    Json(body): Json<AddMovieFileRequest>,
) -> Result<impl IntoResponse, AppError> {
    if body.file_path.trim().is_empty() {
        return Err(AppError::BadRequest("file_path is required".into()));
    }
    if body.format.trim().is_empty() {
        return Err(AppError::BadRequest("format is required".into()));
    }

    // Verify movie exists
    movies::get(&state.pool, movie_id)
        .await
        .map_err(|e| AppError::Internal(format!("Failed to get movie: {e}")))?
        .ok_or_else(|| AppError::NotFound("Movie not found".into()))?;

    let input = movie_files::MovieFileInput {
        movie_id,
        file_path: body.file_path,
        file_size: body.file_size,
        file_fingerprint: None,
        format: body.format,
        quality: body.quality,
        languages: body.languages,
        subtitles: body.subtitles,
        source_type: body.source_type,
        source_metadata: body.source_metadata,
        video_codec: None,
        video_width: None,
        video_height: None,
        video_bitrate_kbps: None,
        framerate: None,
        bit_depth: None,
        is_hdr: false,
        hdr_format: None,
        is_interlaced: false,
        duration_seconds: None,
        container: None,
        overall_bitrate_kbps: None,
    };

    let file_id = movie_files::create(&state.pool, input)
        .await
        .map_err(|e| AppError::Internal(format!("Failed to add movie file: {e}")))?;

    let replaced = movie_files::delete_other_files_for_movie(&state.pool, movie_id, file_id)
        .await
        .unwrap_or_default();
    crate::repositories::cleanup_replaced_files(&replaced).await;

    // Enqueue media probe job to populate video metadata via ffprobe
    let probe_display = crate::jobs::fetch_job_display(&state.pool, "movie_file", file_id).await;
    let probe_context = crate::jobs::JobContext::for_entity(
        "movie_file",
        file_id,
        "Probing media file".to_string(),
    )
    .with_display_opt(probe_display);
    let _ = crate::jobs::enqueue(
        &state.pool,
        state.nats.publisher(),
        "media_probe",
        json!({"file_id": file_id, "media_type": "movie"}),
        probe_context,
    )
    .await;

    Ok((StatusCode::CREATED, Json(json!({ "id": file_id }))))
}

// =============================================================================
// Delete Movie File
// =============================================================================

/// DELETE /api/v1/admin/movies/:id/files/:file_id
pub async fn delete_movie_file(
    State(state): State<AppState>,
    _auth: AuthUser,
    Path(path): Path<MovieFilePath>,
) -> Result<impl IntoResponse, AppError> {
    // Fetch file path before deleting
    let file = movie_files::get(&state.pool, path.file_id)
        .await
        .map_err(|e| AppError::Internal(format!("Failed to get movie file: {e}")))?
        .ok_or_else(|| AppError::NotFound("Movie file not found".into()))?;

    let mut tx = state
        .pool
        .begin()
        .await
        .map_err(|e| AppError::Internal(format!("Failed to begin transaction: {e}")))?;

    movie_files::delete(&mut *tx, path.file_id)
        .await
        .map_err(|e| AppError::Internal(format!("Failed to delete movie file: {e}")))?
        .ok_or_else(|| AppError::NotFound("Movie file not found".into()))?;

    tx.commit()
        .await
        .map_err(|e| AppError::Internal(format!("Failed to commit transaction: {e}")))?;

    // Best-effort cleanup of physical file, thumbnails, and per-file media_cache
    crate::repositories::cleanup_replaced_files(&[file.file_path]).await;
    crate::repositories::cleanup_thumbnail_dir(&state.config.storage_root, path.file_id).await;
    if let Err(e) = crate::media_cache::remove_per_file(&state.config.storage_root, path.file_id) {
        tracing::warn!(error = %e, file_id = %path.file_id, "failed to remove per-file media_cache");
    }

    Ok(StatusCode::NO_CONTENT)
}

// =============================================================================
// Move Movie File
// =============================================================================

#[derive(Debug, Deserialize)]
pub struct MoveMovieFileBody {
    pub target_movie_id: Uuid,
}

/// POST /api/v1/admin/movies/:id/files/:file_id/move
pub async fn move_movie_file(
    State(state): State<AppState>,
    _auth: AuthUser,
    Path(path): Path<MovieFilePath>,
    Json(body): Json<MoveMovieFileBody>,
) -> Result<impl IntoResponse, AppError> {
    // Verify target movie exists
    movies::get(&state.pool, body.target_movie_id)
        .await
        .map_err(|e| AppError::Internal(format!("Failed to check target movie: {e}")))?
        .ok_or_else(|| AppError::NotFound("Target movie not found".into()))?;

    let mut tx = state
        .pool
        .begin()
        .await
        .map_err(|e| AppError::Internal(format!("Failed to begin transaction: {e}")))?;

    let moved = movie_files::move_to_movie(&mut *tx, path.file_id, body.target_movie_id)
        .await
        .map_err(|e| AppError::Internal(format!("Failed to move movie file: {e}")))?;

    if !moved {
        return Err(AppError::NotFound("Movie file not found".into()));
    }

    tx.commit()
        .await
        .map_err(|e| AppError::Internal(format!("Failed to commit transaction: {e}")))?;

    Ok(StatusCode::NO_CONTENT)
}

// =============================================================================
// Replace Movie Cast
// =============================================================================

/// PUT /api/v1/admin/movies/:id/cast
pub async fn replace_movie_cast(
    State(state): State<AppState>,
    _auth: AuthUser,
    Path(id): Path<Uuid>,
    Json(body): Json<Vec<CastInputRequest>>,
) -> Result<impl IntoResponse, AppError> {
    let cast_inputs: Vec<movies::CastInput> = body
        .into_iter()
        .map(|c| movies::CastInput {
            person_id: c.person_id,
            character_name: c.character_name,
            cast_order: c.cast_order,
        })
        .collect();

    movies::replace_cast(&state.pool, id, cast_inputs)
        .await
        .map_err(|e| AppError::Internal(format!("Failed to replace cast: {e}")))?;

    Ok(StatusCode::NO_CONTENT)
}

// =============================================================================
// Replace Movie Crew
// =============================================================================

/// PUT /api/v1/admin/movies/:id/crew
pub async fn replace_movie_crew(
    State(state): State<AppState>,
    _auth: AuthUser,
    Path(id): Path<Uuid>,
    Json(body): Json<Vec<CrewInputRequest>>,
) -> Result<impl IntoResponse, AppError> {
    let crew_inputs: Vec<movies::CrewInput> = body
        .into_iter()
        .map(|c| movies::CrewInput {
            person_id: c.person_id,
            department: c.department,
            job: c.job,
        })
        .collect();

    movies::replace_crew(&state.pool, id, crew_inputs)
        .await
        .map_err(|e| AppError::Internal(format!("Failed to replace crew: {e}")))?;

    Ok(StatusCode::NO_CONTENT)
}

// =============================================================================
// Refresh Metadata
// =============================================================================

/// POST /api/v1/admin/movies/:id/refresh-metadata
pub async fn refresh_metadata(
    State(state): State<AppState>,
    _auth: AuthUser,
    Path(id): Path<Uuid>,
) -> Result<impl IntoResponse, AppError> {
    let movie = movies::get_with_details(&state.pool, id)
        .await
        .map_err(|e| AppError::Internal(format!("Failed to get movie: {e}")))?
        .ok_or_else(|| AppError::NotFound("Movie not found".into()))?;

    // Verify TMDB ID exists before attempting enrichment
    movie
        .identifiers
        .get("tmdb_id")
        .and_then(|v| v.as_str())
        .ok_or_else(|| AppError::BadRequest("Movie has no TMDB identifier".into()))?;

    crate::jobs::handlers::movie_enrich::enrich_movie(&state.pool, id, true)
        .await
        .map_err(|e| AppError::Internal(format!("Failed to enrich movie: {e}")))?;
    images::warm_entity_images(
        &state.pool,
        &state.config.storage_root,
        "movies",
        id,
        images::MOVIE_ART,
    )
    .await;

    let _ = state.library_events.send(LibraryEvent::movie_updated(id));

    let updated = movies::get_with_details(&state.pool, id)
        .await
        .map_err(|e| AppError::Internal(format!("Failed to get updated movie: {e}")))?
        .ok_or_else(|| AppError::Internal("Movie disappeared after update".into()))?;

    let region = state.runtime_config.discover_region().await;
    Ok(Json(movie_detail_response(updated, &region)))
}

// =============================================================================
// Get Watch State
// =============================================================================

/// GET /api/v1/admin/movies/:id/watch-state
pub async fn get_watch_state(
    State(state): State<AppState>,
    auth: PlaybackAuth,
    Path(id): Path<Uuid>,
) -> Result<impl IntoResponse, AppError> {
    let user_id = auth.user_id;

    let ws = movie_watch_states::get(&state.pool, id, user_id)
        .await
        .map_err(|e| AppError::Internal(format!("Failed to get watch state: {e}")))?;

    match ws {
        Some(row) => Ok(Json(json!(WatchStateResponse {
            id: row.id,
            movie_id: row.movie_id,
            user_id: row.user_id,
            watched: row.watched,
            watched_at: row.watched_at,
            position_seconds: row.position_seconds,
            rating: row.rating,
            created_at: row.created_at,
            updated_at: row.updated_at,
        }))
        .into_response()),
        None => Ok(Json(json!(null)).into_response()),
    }
}

// =============================================================================
// Update Watch State
// =============================================================================

/// PUT /api/v1/admin/movies/:id/watch-state
pub async fn update_watch_state(
    State(state): State<AppState>,
    auth: PlaybackAuth,
    Path(id): Path<Uuid>,
    Json(body): Json<UpdateWatchStateRequest>,
) -> Result<impl IntoResponse, AppError> {
    let user_id = auth.user_id;

    let input = movie_watch_states::WatchStateInput {
        movie_id: id,
        user_id,
        watched: body.watched,
        watched_at: body.watched_at,
        position_seconds: body.position_seconds,
        rating: body.rating,
    };

    let ws_id = movie_watch_states::upsert(&state.pool, input)
        .await
        .map_err(|e| AppError::Internal(format!("Failed to update watch state: {e}")))?;

    // Re-index movie in search (watched/rating changed)
    crate::jobs::enqueue_movie_index(&state.pool, state.nats.publisher(), id).await;

    // Fire-and-forget: sync watch state to Trakt (if configured)
    let _ = crate::jobs::enqueue_routine(
        &state.pool,
        state.nats.publisher(),
        "trakt_sync_watch_state",
        serde_json::json!({
            "user_id": user_id,
            "media_type": "movie",
            "media_id": id,
            "watched": body.watched,
            "watched_at": body.watched_at.map(|t| t.format(&time::format_description::well_known::Rfc3339).unwrap()),
        }),
        crate::jobs::JobContext::new("Syncing movie watch state to Trakt"),
    )
    .await;

    Ok(Json(json!({ "id": ws_id })))
}

// =============================================================================
// Update Position Only
// =============================================================================

#[derive(Debug, Deserialize)]
pub struct UpdatePositionRequest {
    pub position_seconds: i32,
    /// The device saving, so the user's other open players can offer to follow it.
    #[serde(flatten)]
    pub client: crate::watch_handoff::PlaybackClient,
}

/// PATCH /api/v1/admin/movies/:id/watch-state/position
pub async fn update_position(
    State(state): State<AppState>,
    auth: PlaybackAuth,
    Path(id): Path<Uuid>,
    Json(body): Json<UpdatePositionRequest>,
) -> Result<impl IntoResponse, AppError> {
    let user_id = auth.user_id;

    let ws_id =
        movie_watch_states::update_position(&state.pool, id, user_id, body.position_seconds)
            .await
            .map_err(|e| AppError::Internal(format!("Failed to update position: {e}")))?;

    let extent = crate::watch_completion::for_movie(&state.pool, id).await;

    if let Some(extent) = extent
        && extent.is_finished_at(body.position_seconds)
    {
        let input = movie_watch_states::WatchStateInput {
            movie_id: id,
            user_id,
            watched: true,
            watched_at: Some(OffsetDateTime::now_utc()),
            position_seconds: Some(body.position_seconds),
            rating: None,
        };
        let _ = movie_watch_states::upsert(&state.pool, input).await;

        // Re-index movie in search
        crate::jobs::enqueue_movie_index(&state.pool, state.nats.publisher(), id).await;

        // Sync to Trakt
        let _ = crate::jobs::enqueue_routine(
            &state.pool,
            state.nats.publisher(),
            "trakt_sync_watch_state",
            serde_json::json!({
                "user_id": user_id,
                "media_type": "movie",
                "media_id": id,
                "watched": true,
            }),
            crate::jobs::JobContext::new("Syncing movie watch state to Trakt"),
        )
        .await;
    } else if extent.is_some() {
        // Seeking back out of the ending: if auto-watched, reset to unwatched so it
        // re-appears in dashboard "in progress".
        if let Ok(Some(ws)) = movie_watch_states::get(&state.pool, id, user_id).await
            && ws.watched
        {
            let input = movie_watch_states::WatchStateInput {
                movie_id: id,
                user_id,
                watched: false,
                watched_at: None,
                position_seconds: Some(body.position_seconds),
                rating: None,
            };
            let _ = movie_watch_states::upsert(&state.pool, input).await;

            crate::jobs::enqueue_movie_index(&state.pool, state.nats.publisher(), id).await;
        }
    }

    crate::watch_handoff::movie_saved(
        &state,
        user_id,
        id,
        body.position_seconds,
        extent,
        &body.client,
    )
    .await;

    Ok(Json(json!({
        "id": ws_id,
        "watched": extent.map(|e| e.is_finished_at(body.position_seconds)),
    })))
}

// =============================================================================
// Image Endpoints
// =============================================================================

/// POST /api/v1/admin/movies/:id/poster
pub async fn upload_poster(
    State(state): State<AppState>,
    _auth: AuthUser,
    Path(id): Path<Uuid>,
    body: axum::body::Bytes,
) -> Result<impl IntoResponse, AppError> {
    // Verify movie exists
    movies::get(&state.pool, id)
        .await
        .map_err(|e| AppError::Internal(format!("Failed to get movie: {e}")))?
        .ok_or_else(|| AppError::NotFound("Movie not found".into()))?;

    crate::images::save_image_bytes(
        &state.config.storage_root,
        crate::images::ImageType::MoviePoster,
        id,
        &body,
    )
    .await
    .map_err(|e| AppError::Internal(format!("Failed to save poster: {e}")))?;

    let _ = images::mark_cached(&state.pool, "movies", id, "poster").await;

    Ok(StatusCode::NO_CONTENT)
}

/// GET /api/v1/admin/movies/:id/poster
///
/// Artwork reads are unauthenticated across the media routes: the id is the only
/// way in and it is only handed out by authenticated listings. Media3 fetches the
/// now-playing bitmap on its own, with no way to attach a header, so a gate here
/// bought nothing and cost the lock-screen artwork. Uploads stay authenticated.
pub async fn get_poster(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
) -> Result<impl IntoResponse, AppError> {
    let movie = movies::get(&state.pool, id)
        .await
        .map_err(|e| AppError::Internal(format!("Failed to get movie: {e}")))?
        .ok_or_else(|| AppError::NotFound("Movie not found".into()))?;

    images::serve_entity_image(
        &state.pool,
        &state.config.storage_root,
        "movies",
        "poster",
        images::ImageType::MoviePoster,
        id,
        &movie.images,
        &headers,
    )
    .await
    .map_err(|e| match e {
        images::ImageError::NotFound(msg) => AppError::NotFound(msg),
        other => AppError::Internal(format!("Failed to load poster: {other}")),
    })
}

/// POST /api/v1/admin/movies/:id/backdrop
pub async fn upload_backdrop(
    State(state): State<AppState>,
    _auth: AuthUser,
    Path(id): Path<Uuid>,
    body: axum::body::Bytes,
) -> Result<impl IntoResponse, AppError> {
    // Verify movie exists
    movies::get(&state.pool, id)
        .await
        .map_err(|e| AppError::Internal(format!("Failed to get movie: {e}")))?
        .ok_or_else(|| AppError::NotFound("Movie not found".into()))?;

    crate::images::save_image_bytes(
        &state.config.storage_root,
        crate::images::ImageType::MovieBackdrop,
        id,
        &body,
    )
    .await
    .map_err(|e| AppError::Internal(format!("Failed to save backdrop: {e}")))?;

    let _ = images::mark_cached(&state.pool, "movies", id, "backdrop").await;

    Ok(StatusCode::NO_CONTENT)
}

/// GET /api/v1/admin/movies/:id/backdrop
pub async fn get_backdrop(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
) -> Result<impl IntoResponse, AppError> {
    let movie = movies::get(&state.pool, id)
        .await
        .map_err(|e| AppError::Internal(format!("Failed to get movie: {e}")))?
        .ok_or_else(|| AppError::NotFound("Movie not found".into()))?;

    images::serve_entity_image(
        &state.pool,
        &state.config.storage_root,
        "movies",
        "backdrop",
        images::ImageType::MovieBackdrop,
        id,
        &movie.images,
        &headers,
    )
    .await
    .map_err(|e| match e {
        images::ImageError::NotFound(msg) => AppError::NotFound(msg),
        other => AppError::Internal(format!("Failed to load backdrop: {other}")),
    })
}

/// POST /api/v1/admin/movies/:id/logo
pub async fn upload_logo(
    State(state): State<AppState>,
    _auth: AuthUser,
    Path(id): Path<Uuid>,
    body: axum::body::Bytes,
) -> Result<impl IntoResponse, AppError> {
    movies::get(&state.pool, id)
        .await
        .map_err(|e| AppError::Internal(format!("Failed to get movie: {e}")))?
        .ok_or_else(|| AppError::NotFound("Movie not found".into()))?;

    images::save_image_bytes(
        &state.config.storage_root,
        images::ImageType::MovieLogo,
        id,
        &body,
    )
    .await
    .map_err(|e| AppError::Internal(format!("Failed to save logo: {e}")))?;

    let _ = images::mark_cached(&state.pool, "movies", id, "logo").await;

    Ok(StatusCode::NO_CONTENT)
}

/// GET /api/v1/admin/movies/:id/logo
pub async fn get_logo(
    State(state): State<AppState>,
    _auth: AuthUser,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
) -> Result<impl IntoResponse, AppError> {
    let movie = movies::get(&state.pool, id)
        .await
        .map_err(|e| AppError::Internal(format!("Failed to get movie: {e}")))?
        .ok_or_else(|| AppError::NotFound("Movie not found".into()))?;

    images::serve_entity_image(
        &state.pool,
        &state.config.storage_root,
        "movies",
        "logo",
        images::ImageType::MovieLogo,
        id,
        &movie.images,
        &headers,
    )
    .await
    .map_err(|e| match e {
        images::ImageError::NotFound(msg) => AppError::NotFound(msg),
        other => AppError::Internal(format!("Failed to load logo: {other}")),
    })
}

// =============================================================================
// Alphabet Navigation
// =============================================================================

#[derive(Debug, Deserialize)]
pub struct FirstByLetterQuery {
    pub letter: String,
}

#[derive(Debug, Serialize)]
struct FirstByLetterResponse {
    id: Uuid,
    title: String,
}

/// GET /api/v1/admin/movies/first-by-letter?letter=A
pub async fn first_by_letter(
    State(state): State<AppState>,
    auth: AuthUser,
    Query(params): Query<FirstByLetterQuery>,
) -> Result<impl IntoResponse, AppError> {
    let letter = params.letter.trim().to_uppercase();
    if letter.is_empty() {
        return Err(AppError::BadRequest("letter parameter required".into()));
    }
    if letter != "#" && (letter.len() != 1 || !letter.chars().next().unwrap().is_ascii_alphabetic())
    {
        return Err(AppError::BadRequest("letter must be A-Z or #".into()));
    }

    match movies::first_by_letter(&state.pool, &letter, auth.is_kids).await {
        Ok(Some(result)) => Ok(Json(FirstByLetterResponse {
            id: result.id,
            title: result.title,
        })
        .into_response()),
        Ok(None) => Err(AppError::NotFound("no movies found for letter".into())),
        Err(e) => Err(AppError::Internal(format!(
            "Failed to find movie by letter: {e}"
        ))),
    }
}

#[derive(Debug, Serialize)]
struct AvailableLettersResponse {
    letters: Vec<String>,
}

/// GET /api/v1/admin/movies/available-letters
pub async fn available_letters(
    State(state): State<AppState>,
    auth: AuthUser,
) -> Result<impl IntoResponse, AppError> {
    use crate::search::MovieSearchFilters;

    // The index knows nothing of Kids profiles; their bar comes from the rule in SQL.
    if auth.is_kids {
        let letters = crate::repositories::letters::available_letters(
            &state.pool,
            "movies b",
            &[crate::kids::KIDS_ALLOWED_B],
        )
        .await
        .map_err(|e| AppError::Internal(format!("Failed to get available letters: {e}")))?;
        return Ok(Json(AvailableLettersResponse { letters }));
    }

    let filters = MovieSearchFilters::default();
    let letters = match state.search.get_available_movie_letters(filters).await {
        Ok(letters) => letters,
        // The bar is furniture, not an answer to a question: losing the search index
        // should cost typo tolerance, not make the page's alphabet vanish.
        Err(e) => {
            tracing::debug!(error = %e, "Movie letters from search unavailable, using SQL");
            crate::repositories::letters::available_letters(&state.pool, "movies b", &[])
                .await
                .map_err(|e| AppError::Internal(format!("Failed to get available letters: {e}")))?
        }
    };
    Ok(Json(AvailableLettersResponse { letters }))
}

/// GET /api/v1/admin/movies/facets
/// Contextual genre + want_status counts for drill-down filtering.
pub async fn facets(
    State(state): State<AppState>,
    _auth: AuthUser,
) -> Result<impl IntoResponse, AppError> {
    use crate::search::MovieSearchFilters;

    let filters = MovieSearchFilters::default();
    match state.search.get_movie_facets(filters).await {
        Ok(facets) => Ok(Json(facets)),
        Err(e) => Err(AppError::Internal(format!(
            "Failed to get movie facets: {e}"
        ))),
    }
}

// =============================================================================
// Get Movie Tracks
// =============================================================================

#[derive(Debug, Serialize)]
struct FileTracksResponse {
    file_id: Uuid,
    file_path: String,
    video_codec: Option<String>,
    video_width: Option<i32>,
    video_height: Option<i32>,
    audio_tracks: Vec<audio_tracks::AudioTrackRow>,
    subtitle_tracks: Vec<subtitle_tracks::SubtitleTrackRow>,
    external_subtitles: Vec<external_subtitles::ExternalSubtitleRow>,
}

#[derive(Debug, Serialize)]
struct TracksResponse {
    files: Vec<FileTracksResponse>,
}

/// GET /api/v1/admin/movies/:id/tracks
/// Returns all audio tracks, subtitle tracks, and external subs across all files.
pub async fn get_movie_tracks(
    State(state): State<AppState>,
    _auth: AuthUser,
    Path(movie_id): Path<Uuid>,
) -> Result<impl IntoResponse, AppError> {
    // Verify movie exists
    movies::get(&state.pool, movie_id)
        .await
        .map_err(|e| AppError::Internal(format!("Failed to get movie: {e}")))?
        .ok_or_else(|| AppError::NotFound("Movie not found".into()))?;

    let files = movie_files::list_by_movie(&state.pool, movie_id)
        .await
        .map_err(|e| AppError::Internal(format!("Failed to list movie files: {e}")))?;

    let mut file_tracks = Vec::with_capacity(files.len());
    for f in files {
        let audio = audio_tracks::list_by_movie_file(&state.pool, f.id)
            .await
            .map_err(|e| AppError::Internal(format!("Failed to list audio tracks: {e}")))?;
        let subs = subtitle_tracks::list_by_movie_file(&state.pool, f.id)
            .await
            .map_err(|e| AppError::Internal(format!("Failed to list subtitle tracks: {e}")))?;
        let ext_subs = external_subtitles::list_by_movie_file(&state.pool, f.id)
            .await
            .map_err(|e| AppError::Internal(format!("Failed to list external subtitles: {e}")))?;

        file_tracks.push(FileTracksResponse {
            file_id: f.id,
            file_path: f.file_path,
            video_codec: f.video_codec,
            video_width: f.video_width,
            video_height: f.video_height,
            audio_tracks: audio,
            subtitle_tracks: subs,
            external_subtitles: ext_subs,
        });
    }

    Ok(Json(TracksResponse { files: file_tracks }))
}

// =============================================================================
// Helpers
// =============================================================================

fn movie_detail_response(detail: movies::MovieDetail, region: &str) -> MovieDetailResponse {
    let base = "/api/v1/admin/movies/";
    let poster_url = endpoint_url(&detail.images, "poster", base, detail.id, "/poster");
    let backdrop_url = endpoint_url(&detail.images, "backdrop", base, detail.id, "/backdrop");
    let logo_url = endpoint_url(&detail.images, "logo", base, detail.id, "/logo");
    MovieDetailResponse {
        id: detail.id,
        title: detail.title,
        sort_title: detail.sort_title,
        original_title: detail.original_title,
        year: detail.year,
        runtime: detail.runtime,
        description: detail.description,
        tagline: detail.tagline,
        poster_url,
        backdrop_url,
        logo_url,
        identifiers: detail.identifiers,
        genres: detail.genres,
        rating: detail.rating,
        rating_count: detail.rating_count,
        content_rating: detail.content_rating,
        kids_override: detail.kids_override,
        kids_allowed: detail.kids_allowed,
        release_date: detail.release_date,
        release_dates: release_dates::select(&detail.release_dates, region),
        is_released: detail.is_released,
        quality_profile_id: detail.quality_profile_id,
        preferred_languages: detail.preferred_languages,
        media_root_id: detail.media_root_id,
        is_in_library: detail.is_in_library,
        want_status: detail.want_status,
        watched: None,
        watch_position_seconds: None,
        in_watchlist: false,
        streamed_at: detail.streamed_at,
        created_at: detail.created_at,
        updated_at: detail.updated_at,
        cast: detail
            .cast
            .into_iter()
            .map(|c| CastResponse {
                id: c.id,
                person_id: c.person_id,
                person_name: c.person_name,
                person_image_url: c.person_image_url,
                character_name: c.character_name,
                cast_order: c.cast_order,
            })
            .collect(),
        crew: detail
            .crew
            .into_iter()
            .map(|c| CrewResponse {
                person_id: c.person_id,
                person_name: c.person_name,
                department: c.department,
                job: c.job,
            })
            .collect(),
    }
}

/// Flatten a JSONB quality value to a human-readable string.
/// If it's already a string, return it directly.
/// If it's an object, build a label from known keys (resolution, codec, source).
fn flatten_quality(value: Option<&serde_json::Value>) -> Option<String> {
    let v = value?;
    match v {
        serde_json::Value::String(s) => Some(s.clone()),
        serde_json::Value::Object(obj) => {
            let parts: Vec<&str> = ["resolution", "source", "codec"]
                .iter()
                .filter_map(|key| obj.get(*key).and_then(|v| v.as_str()))
                .collect();
            if parts.is_empty() {
                None
            } else {
                Some(parts.join(" "))
            }
        }
        _ => Some(v.to_string()),
    }
}

// =============================================================================
// Search Movie Releases
// =============================================================================

#[derive(Debug, Deserialize)]
pub struct MovieReleasesQuery {
    pub q: Option<String>,
    /// Category scope for the search. `None`/"default" keeps the movie category
    /// filter; "any" drops it so releases filed elsewhere are visible.
    pub scope: Option<String>,
}

/// GET /api/v1/admin/movies/{id}/releases
pub async fn search_movie_releases(
    State(state): State<AppState>,
    _auth: AuthUser,
    Path(id): Path<Uuid>,
    Query(query): Query<MovieReleasesQuery>,
) -> impl IntoResponse {
    let movie = match movies::get(&state.pool, id).await {
        Ok(Some(m)) => m,
        Ok(None) => return (StatusCode::NOT_FOUND, "movie not found").into_response(),
        Err(e) => {
            tracing::error!(error = %e, "Failed to fetch movie");
            return (StatusCode::INTERNAL_SERVER_ERROR, "database error").into_response();
        }
    };

    let override_query = query
        .q
        .as_deref()
        .map(str::trim)
        .filter(|v| !v.is_empty())
        .map(str::to_string);

    let identifiers = nats_jobs::MovieIdentifiers {
        tmdb_id: movie
            .identifiers
            .get("tmdb_id")
            .and_then(|v| v.as_str())
            .map(String::from),
        imdb_id: movie
            .identifiers
            .get("imdb_id")
            .and_then(|v| v.as_str())
            .map(String::from),
    };

    let media_want = nats_jobs::MediaWant::Movie {
        title: movie.title.clone(),
        year: movie.year.map(|y| y as u32),
        release_date: movie.release_date.map(|d| d.to_string()),
        identifiers: Some(identifiers),
        alternate_titles: crate::wanted::alternate_titles::alternate_titles(
            &movie.title,
            movie.alternate_titles.iter().flatten(),
        ),
        quality_profile_id: movie.quality_profile_id,
        quality_overrides: movie.preferred_languages.as_ref().map(|langs| {
            nats_jobs::QualityOverrides {
                preferred_languages: Some(langs.clone()),
                ..Default::default()
            }
        }),
        in_library: None,
        library_video_height: None,
    };

    // Free text overrides the structured terms, but must not drop the media
    // type with them: Kobload derives its category filter from one or the
    // other, and with neither the search goes out uncategorised. "any" drops
    // it deliberately, for releases filed under Other/Misc.
    let any_scope = query.scope.as_deref() == Some("any");

    let (primary_query, media_want, media_type) = if let Some(override_query) = override_query {
        let media_type = (!any_scope).then(|| "movies".to_string());
        (override_query, None, media_type)
    } else {
        (String::new(), Some(media_want), None)
    };

    let runtime = state.runtime_config.get().await;
    let client = match build_kobload_client(&runtime) {
        Ok(client) => client,
        Err((status, message)) => return (status, message).into_response(),
    };

    match client
        .search_scoped(&primary_query, None, media_want, media_type)
        .await
    {
        Ok(mut results) => {
            let rejected = movies::get_rejected_releases(&state.pool, id)
                .await
                .unwrap_or(json!([]));
            crate::routes::library::search_download::annotate_rejected_releases(
                &mut results,
                &rejected,
            );
            Json(json!({ "results": results })).into_response()
        }
        Err(err) => {
            tracing::error!(error = ?err, "Failed to search Kobload for movie releases");
            let (status, message) = match &err {
                crate::kobload::KobloadError::Status(s, body) => {
                    let status =
                        StatusCode::from_u16(s.as_u16()).unwrap_or(StatusCode::BAD_GATEWAY);
                    let msg = serde_json::from_str::<serde_json::Value>(body)
                        .ok()
                        .and_then(|v| v.get("message").and_then(|m| m.as_str()).map(String::from))
                        .unwrap_or_else(|| format!("{err}"));
                    (status, msg)
                }
                crate::kobload::KobloadError::Request(_) => {
                    (StatusCode::BAD_GATEWAY, format!("{err}"))
                }
            };
            (status, Json(json!({ "message": message }))).into_response()
        }
    }
}

// =============================================================================
// Queue Movie Release
// =============================================================================

#[derive(Debug, Deserialize)]
pub struct QueueMovieReleaseRequest {
    pub source: String,
    pub release: serde_json::Value,
    /// A regrab: keep whatever file is already on the movie instead of
    /// replacing it. The chat bot's `regrab` tool sets this.
    #[serde(default)]
    pub additional_copy: bool,
}

/// POST /api/v1/admin/movies/{id}/queue
pub async fn queue_movie_release(
    State(state): State<AppState>,
    auth: AuthUser,
    Path(id): Path<Uuid>,
    Json(payload): Json<QueueMovieReleaseRequest>,
) -> impl IntoResponse {
    let movie = match movies::get(&state.pool, id).await {
        Ok(Some(m)) => m,
        Ok(None) => return (StatusCode::NOT_FOUND, "movie not found").into_response(),
        Err(e) => {
            tracing::error!(error = %e, "Failed to fetch movie");
            return (StatusCode::INTERNAL_SERVER_ERROR, "database error").into_response();
        }
    };

    let release = payload.release.clone();
    let protocol = match release.get("protocol").and_then(|v| v.as_str()) {
        Some(p) => p.to_lowercase(),
        None => return (StatusCode::BAD_REQUEST, "release protocol missing").into_response(),
    };
    let download_url = match release.get("download_url").and_then(|v| v.as_str()) {
        Some(u) => u.to_string(),
        None => return (StatusCode::BAD_REQUEST, "release download_url missing").into_response(),
    };

    let runtime = state.runtime_config.get().await;
    let client = match build_kobload_client(&runtime) {
        Ok(client) => client,
        Err((status, message)) => return (status, message).into_response(),
    };

    let callback_url = match runtime.kobload_ingest_url.as_deref() {
        Some(value) if !value.trim().is_empty() => value.trim().to_string(),
        _ => {
            return (
                StatusCode::BAD_REQUEST,
                "Kobload Ingest URL not configured. Go to Settings > System > Kobload and set the Ingest URL.",
            )
                .into_response();
        }
    };

    let source = if payload.source.trim().is_empty() {
        "manual".to_string()
    } else {
        payload.source.clone()
    };

    let metadata = crate::file_link_mode::tag_metadata(
        json!({
            "movie_id": id,
            "source_type": "movie",
            "title": movie.title,
            "year": movie.year,
            "release_title": release.get("title").cloned().unwrap_or(serde_json::Value::Null),
            "release_file_name": release.get("file_name").cloned().unwrap_or(serde_json::Value::Null),
        }),
        FileLinkMode::from_additional_copy(payload.additional_copy),
    );

    let name = release
        .get("file_name")
        .and_then(|v| v.as_str())
        .or_else(|| release.get("title").and_then(|v| v.as_str()))
        .unwrap_or("download")
        .to_string();

    let request = KobloadDownloadRequest {
        source: source.clone(),
        protocol,
        name,
        download_url: download_url.clone(),
        callback_url,
        auth_token: None,
        metadata,
        release: Some(release.clone()),
    };

    let (status, message, download_id) = match client.create_download(&request).await {
        Ok(response) => (response.status, None, Some(response.id)),
        Err(err) => (
            "failed".to_string(),
            Some(format!("Kobload error: {err}")),
            None,
        ),
    };

    let mut tx = match state.pool.begin().await {
        Ok(tx) => tx,
        Err(e) => {
            tracing::error!(error = %e, "Failed to begin transaction");
            return (StatusCode::INTERNAL_SERVER_ERROR, "database error").into_response();
        }
    };

    if let Err(e) = wanted_attempts::create(
        &mut *tx,
        wanted_attempts::WantedAttemptInput {
            movie_id: Some(id),
            source,
            external_id: download_id.map(|id| id.to_string()),
            download_url: Some(download_url),
            status: status.clone(),
            message: message.clone(),
            user_id: Some(auth.user_id),
            ..Default::default()
        },
    )
    .await
    {
        tracing::error!(error = %e, "Failed to insert wanted attempt");
        return (StatusCode::INTERNAL_SERVER_ERROR, "database error").into_response();
    }

    if let Err(e) = tx.commit().await {
        tracing::error!(error = %e, "Failed to commit transaction");
        return (StatusCode::INTERNAL_SERVER_ERROR, "database error").into_response();
    }

    if status == "failed" {
        let error_msg = message.unwrap_or_else(|| "kobload download failed".to_string());
        return (
            StatusCode::BAD_GATEWAY,
            Json(json!({ "message": error_msg })),
        )
            .into_response();
    }

    StatusCode::NO_CONTENT.into_response()
}

// =============================================================================
// Skip Segment Manual Override
// =============================================================================

#[derive(Debug, Deserialize)]
pub struct MovieSkipSegmentOverride {
    credits_start: Option<f64>,
    credits_end: Option<f64>,
    post_credits_start: Option<f64>,
    post_credits_end: Option<f64>,
}

/// PUT /api/v1/admin/movies/files/{file_id}/skip-segments
pub async fn set_movie_skip_segments(
    State(state): State<AppState>,
    _auth: AuthUser,
    Path(file_id): Path<Uuid>,
    Json(body): Json<MovieSkipSegmentOverride>,
) -> impl IntoResponse {
    use crate::repositories::movie_skip_segments;

    if let (Some(start), Some(end)) = (body.credits_start, body.credits_end) {
        let result = if start == 0.0 && end == 0.0 {
            movie_skip_segments::suppress(&state.pool, file_id, "credits").await
        } else {
            movie_skip_segments::force_upsert(
                &state.pool,
                file_id,
                "credits",
                start,
                end,
                "manual",
                None,
                None,
            )
            .await
            .map(|_| ())
        };
        if let Err(e) = result {
            warn!(error = %e, "Failed to set movie credits skip segment");
            return StatusCode::INTERNAL_SERVER_ERROR.into_response();
        }
    }

    if let (Some(start), Some(end)) = (body.post_credits_start, body.post_credits_end) {
        let result = if start == 0.0 && end == 0.0 {
            movie_skip_segments::suppress(&state.pool, file_id, "post_credits_scene").await
        } else {
            movie_skip_segments::force_upsert(
                &state.pool,
                file_id,
                "post_credits_scene",
                start,
                end,
                "manual",
                None,
                None,
            )
            .await
            .map(|_| ())
        };
        if let Err(e) = result {
            warn!(error = %e, "Failed to set movie post-credits skip segment");
            return StatusCode::INTERNAL_SERVER_ERROR.into_response();
        }
    }

    StatusCode::NO_CONTENT.into_response()
}

// =============================================================================
// V2 Media Analyze + Media Cache Runs
// =============================================================================

/// Query params for the "Re-analyze Media" movie endpoint — `force=true` makes
/// `media_analyze` rebuild every sidecar from scratch even if the manifest
/// says they're already at the current pipeline version.
#[derive(Debug, Deserialize, Default)]
pub struct MediaAnalyzeMovieQuery {
    #[serde(default)]
    pub force: bool,
}

/// POST /api/v1/admin/movies/{id}/media-analyze?force=bool
pub async fn run_media_analyze_movie(
    State(state): State<AppState>,
    _auth: AuthUser,
    Path(movie_id): Path<Uuid>,
    Query(query): Query<MediaAnalyzeMovieQuery>,
) -> impl IntoResponse {
    let files = match movie_files::list_by_movie(&state.pool, movie_id).await {
        Ok(f) => f,
        Err(e) => {
            warn!(error = %e, "Failed to list movie files for media-analyze");
            return StatusCode::INTERNAL_SERVER_ERROR.into_response();
        }
    };

    let display = crate::jobs::fetch_job_display(&state.pool, "movie", movie_id).await;
    let mut enqueued = 0u32;
    let mut run_ids = Vec::with_capacity(files.len());

    for file in &files {
        let run_id = Uuid::now_v7();

        let analyze_payload = crate::jobs::handlers::MediaAnalyzePayload {
            file_id: file.id,
            media_type: "movie".to_string(),
            force_regen: query.force,
            run_id: Some(run_id),
            triggered_by: "manual".to_string(),
        };
        let analyze_ctx = crate::jobs::JobContext::for_entity(
            "movie",
            movie_id,
            "Media analyze + skip-detect".to_string(),
        )
        .with_display_opt(display.clone());
        // media_analyze embeds frames inline and chains skip_detect itself
        // once the cache is durable — don't enqueue skip_detect in parallel.
        if let Err(e) = crate::jobs::enqueue(
            &state.pool,
            state.nats.publisher(),
            "media_analyze",
            serde_json::to_value(&analyze_payload).unwrap(),
            analyze_ctx,
        )
        .await
        {
            warn!(error = %e, file_id = %file.id, "Failed to enqueue media_analyze");
            continue;
        }

        run_ids.push(run_id);
        enqueued += 1;
    }

    (
        StatusCode::OK,
        Json(json!({ "enqueued": enqueued, "run_ids": run_ids })),
    )
        .into_response()
}

#[derive(Debug, Serialize)]
struct GenreCount {
    name: String,
    count: i64,
    backdrop_url: Option<String>,
}

#[derive(Debug, Serialize)]
struct GenresResponse {
    genres: Vec<GenreCount>,
}

/// GET /api/v1/admin/movies/genres
pub async fn genres(
    State(state): State<AppState>,
    auth: AuthUser,
) -> Result<impl IntoResponse, AppError> {
    let genres = movies::genre_counts(&state.pool, auth.is_kids)
        .await
        .map_err(|e| AppError::Internal(format!("Failed to list genres: {e}")))?
        .into_iter()
        .map(|(name, count, backdrop_id)| GenreCount {
            name,
            count,
            backdrop_url: backdrop_id.map(|id| format!("/api/v1/admin/movies/{id}/backdrop")),
        })
        .collect();
    Ok(Json(GenresResponse { genres }))
}
