//! TV admin route handlers.
//!
//! CRUD for shows, seasons, episodes; want/unwant; file management;
//! metadata refresh; and watch state for the TV library.

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
use crate::images::{self, ImageEntry, endpoint_url, source_url};
use crate::jobs::{self, JobContext};
use crate::kobload::CreateDownloadRequest as KobloadDownloadRequest;
use crate::repositories::{
    PaginationDirection, audio_tracks, episode_skip_segments, external_subtitles, media_roots,
    subtitle_tracks, tv_episode_files, tv_episodes, tv_seasons, tv_show_ratings, tv_shows,
    tv_watch_states, wanted_attempts,
};
use crate::routes::helpers::build_kobload_client;
use crate::routes::library::helpers::{
    NameCursor, decode_name_cursor, encode_name_cursor, paginated_window,
};
use crate::routes::library::types::PaginatedResponse;
use crate::routes::{AuthUser, PlaybackAuth};
use crate::sport_want::{EventWant, event_want};
use crate::video_metadata::VideoProviderRegistry;

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
// Request/Response Types — Shows
// =============================================================================

#[derive(Debug, Deserialize)]
pub struct ListShowsQuery {
    pub monitored: Option<bool>,
    pub status: Option<String>,
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
struct ShowListResponse {
    id: Uuid,
    title: String,
    sort_title: Option<String>,
    year: Option<i32>,
    poster_url: Option<String>,
    backdrop_url: Option<String>,
    logo_url: Option<String>,
    identifiers: serde_json::Value,
    status: Option<String>,
    network: Option<String>,
    is_monitored: bool,
    auto_want_mode: String,
    content_category: String,
    #[serde(with = "time::serde::rfc3339::option")]
    streamed_at: Option<OffsetDateTime>,
    #[serde(with = "time::serde::rfc3339")]
    created_at: OffsetDateTime,
    #[serde(with = "time::serde::rfc3339")]
    updated_at: OffsetDateTime,
    aired_episode_count: i64,
    in_library_count: i64,
}

#[derive(Debug, Serialize)]
struct ShowDetailResponse {
    id: Uuid,
    title: String,
    sort_title: Option<String>,
    original_title: Option<String>,
    year: Option<i32>,
    status: Option<String>,
    description: Option<String>,
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
    network: Option<String>,
    is_monitored: bool,
    auto_want_mode: String,
    quality_profile_id: Option<Uuid>,
    preferred_languages: Option<Vec<String>>,
    media_root_id: Option<Uuid>,
    content_category: String,
    sport_metadata: Option<serde_json::Value>,
    #[serde(with = "crate::serde_date::option_date")]
    first_aired: Option<time::Date>,
    #[serde(with = "crate::serde_date::option_date")]
    last_aired: Option<time::Date>,
    #[serde(with = "crate::serde_date::option_date")]
    next_airing: Option<time::Date>,
    runtime: Option<i32>,
    total_episode_count: Option<i32>,
    #[serde(with = "time::serde::rfc3339::option")]
    streamed_at: Option<OffsetDateTime>,
    #[serde(with = "time::serde::rfc3339")]
    created_at: OffsetDateTime,
    #[serde(with = "time::serde::rfc3339")]
    updated_at: OffsetDateTime,
    seasons: Vec<SeasonResponse>,
    #[serde(skip_serializing_if = "Option::is_none")]
    continue_episode: Option<ContinueEpisodeResponse>,
    cast: Vec<CastResponse>,
    crew: Vec<CrewResponse>,
    in_watchlist: bool,
}

#[derive(Debug, Serialize)]
struct SeasonResponse {
    id: Uuid,
    show_id: Uuid,
    season_identifier: String,
    season_sort_order: i32,
    title: Option<String>,
    description: Option<String>,
    poster_url: Option<String>,
    #[serde(with = "crate::serde_date::option_date")]
    air_date: Option<time::Date>,
    episode_count: Option<i32>,
    is_monitored: bool,
    is_special: bool,
    auto_want_mode: String,
    #[serde(with = "time::serde::rfc3339")]
    created_at: OffsetDateTime,
    episodes: Vec<EpisodeResponse>,
}

#[derive(Debug, Serialize)]
struct EpisodeResponse {
    id: Uuid,
    season_id: Uuid,
    episode_identifier: String,
    episode_sort_order: i32,
    title: Option<String>,
    description: Option<String>,
    still_url: Option<String>,
    #[serde(with = "crate::serde_date::option_date")]
    air_date: Option<time::Date>,
    /// Absent when the provider only publishes the broadcast day, which is the
    /// common case: TMDB has no air time at all. Callers must not invent one.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "time::serde::rfc3339::option"
    )]
    air_time: Option<OffsetDateTime>,
    runtime: Option<i32>,
    is_in_library: bool,
    want_status: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    watched: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    watch_position_seconds: Option<i32>,
    #[serde(with = "time::serde::rfc3339")]
    created_at: OffsetDateTime,
    #[serde(with = "time::serde::rfc3339")]
    updated_at: OffsetDateTime,
}

#[derive(Debug, Serialize)]
struct ContinueEpisodeResponse {
    season_idx: usize,
    episode_idx: usize,
    episode_id: Uuid,
    position_seconds: i32,
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

#[derive(Debug, Deserialize)]
pub struct CreateShowRequest {
    pub title: String,
    pub sort_title: Option<String>,
    pub original_title: Option<String>,
    pub year: Option<i32>,
    pub status: Option<String>,
    pub description: Option<String>,
    pub poster_url: Option<String>,
    pub backdrop_url: Option<String>,
    pub identifiers: Option<serde_json::Value>,
    pub genres: Option<Vec<String>>,
    pub rating: Option<f64>,
    pub rating_count: Option<i32>,
    pub content_rating: Option<String>,
    pub network: Option<String>,
    pub quality_profile_id: Option<Uuid>,
    pub preferred_languages: Option<Vec<String>>,
    pub media_root_id: Option<Uuid>,
    #[serde(default, with = "crate::serde_date::option_date")]
    pub first_aired: Option<time::Date>,
    pub runtime: Option<i32>,
}

#[derive(Debug, Deserialize)]
pub struct UpdateShowRequest {
    /// Whether a Kids profile may see this title regardless of its rating; `null` defers
    /// to the rating again. Admin only.
    #[serde(default, deserialize_with = "crate::serde_patch::double_option")]
    pub kids_override: Option<Option<bool>>,
    pub title: Option<String>,
    pub sort_title: Option<String>,
    pub original_title: Option<String>,
    pub year: Option<i32>,
    pub status: Option<String>,
    pub description: Option<String>,
    pub poster_url: Option<String>,
    pub backdrop_url: Option<String>,
    pub logo_url: Option<String>,
    pub identifiers: Option<serde_json::Value>,
    pub genres: Option<Vec<String>>,
    pub rating: Option<f64>,
    pub rating_count: Option<i32>,
    pub content_rating: Option<String>,
    pub network: Option<String>,
    #[serde(default, with = "crate::serde_date::option_date")]
    pub first_aired: Option<time::Date>,
    #[serde(default, with = "crate::serde_date::option_date")]
    pub last_aired: Option<time::Date>,
    #[serde(default, with = "crate::serde_date::option_date")]
    pub next_airing: Option<time::Date>,
    pub runtime: Option<i32>,
    pub total_episode_count: Option<i32>,
    pub quality_profile_id: Option<Option<Uuid>>,
    pub preferred_languages: Option<Option<Vec<String>>>,
    pub media_root_id: Option<Option<Uuid>>,
}

#[derive(Debug, Deserialize)]
pub struct MonitorRequest {
    pub is_monitored: bool,
    pub auto_want_mode: Option<String>,
}

// =============================================================================
// Request/Response Types — Cast/Crew
// =============================================================================

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
// Request/Response Types — Episodes
// =============================================================================

#[derive(Debug, Serialize)]
struct EpisodeFileResponse {
    id: Uuid,
    episode_ids: Vec<Uuid>,
    file_path: String,
    file_size: Option<i64>,
    format: String,
    quality: Option<serde_json::Value>,
    languages: Option<Vec<String>>,
    subtitles: Option<Vec<String>>,
    source_type: Option<String>,
    source_metadata: Option<serde_json::Value>,
    available: bool,
    #[serde(with = "time::serde::rfc3339")]
    created_at: OffsetDateTime,
}

#[derive(Debug, Deserialize)]
pub struct AddEpisodeFileRequest {
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
struct TvWatchStateResponse {
    id: Uuid,
    episode_id: Uuid,
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
pub struct BulkWatchStateRequest {
    pub watched: bool,
}

#[derive(Debug, Deserialize)]
pub struct UpdateShowRatingRequest {
    pub rating: f64,
}

#[derive(Debug, Serialize)]
struct ShowRatingResponse {
    rating: Option<f64>,
}

#[derive(Debug, Deserialize)]
pub struct EpisodeFilePath {
    pub id: Uuid,
    pub file_id: Uuid,
}

// =============================================================================
// List Shows
// =============================================================================

/// GET /api/v1/admin/tv/shows
pub async fn list_shows(
    State(state): State<AppState>,
    auth: AuthUser,
    Query(params): Query<ListShowsQuery>,
) -> Result<impl IntoResponse, AppError> {
    let limit = params.limit.unwrap_or(50).min(200);

    // Handle "around" mode
    if let Some(ref around_id_str) = params.around {
        let around_id: Uuid = around_id_str
            .parse()
            .map_err(|_| AppError::BadRequest("Invalid around ID".into()))?;
        let before_count = params.around_before.unwrap_or(0).max(0);
        let after_limit = limit - before_count;

        let target = tv_shows::get(&state.pool, around_id)
            .await
            .map_err(|e| AppError::Internal(format!("Failed to get show: {e}")))?
            .ok_or_else(|| AppError::NotFound("Show not found".into()))?;

        let target_sort = target
            .sort_title
            .as_deref()
            .unwrap_or(&target.title)
            .to_string();

        // One row past `before_count` doubles as the "is anything before this window"
        // probe, so around_before=0 still costs a single LIMIT 1 index lookup.
        let before_filters = tv_shows::TvShowListFilters {
            monitored: params.monitored,
            status: params.status.clone(),
            search: params.search.clone(),
            limit: before_count + 1,
            cursor_title: Some(target_sort.clone()),
            cursor_id: Some(around_id),
            direction: PaginationDirection::Backward,
            content_category: Some("tv".to_string()),
            genre: params.genre.clone(),
            kids_only: auth.is_kids,
            ..Default::default()
        };
        let mut before_rows = tv_shows::list(&state.pool, &before_filters)
            .await
            .map_err(|e| AppError::Internal(format!("Failed to list shows: {e}")))?;

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

        let after_filters = tv_shows::TvShowListFilters {
            monitored: params.monitored,
            status: params.status.clone(),
            search: params.search.clone(),
            limit: after_limit + 1,
            cursor_title: Some(cursor_title),
            cursor_id: Some(cursor_id),
            direction: PaginationDirection::Forward,
            cursor_inclusive,
            content_category: Some("tv".to_string()),
            genre: params.genre.clone(),
            kids_only: auth.is_kids,
            ..Default::default()
        };

        let after_rows = tv_shows::list(&state.pool, &after_filters)
            .await
            .map_err(|e| AppError::Internal(format!("Failed to list shows: {e}")))?;

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

        let items = to_show_list_responses(combined);
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
    let sort_by_title = matches!(params.sort.as_deref(), None | Some("title"));

    let filters = tv_shows::TvShowListFilters {
        monitored: params.monitored,
        status: params.status,
        search: params.search,
        limit: limit + 1,
        cursor_title,
        cursor_id,
        direction,
        cursor_inclusive: false,
        content_category: Some("tv".to_string()),
        sort: params.sort,
        genre: params.genre.clone(),
        kids_only: auth.is_kids,
    };

    let total_count = if is_first_page {
        Some(
            tv_shows::count(&state.pool, &filters)
                .await
                .map_err(|e| AppError::Internal(format!("Failed to count shows: {e}")))?,
        )
    } else {
        None
    };

    let rows = tv_shows::list(&state.pool, &filters)
        .await
        .map_err(|e| AppError::Internal(format!("Failed to list shows: {e}")))?;

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

    let response_items = to_show_list_responses(items);

    Ok(Json(PaginatedResponse {
        items: response_items,
        next_cursor,
        prev_cursor,
        total_count,
    }))
}

fn to_show_list_responses(rows: Vec<tv_shows::TvShowRow>) -> Vec<ShowListResponse> {
    rows.into_iter()
        .map(|r| {
            let poster_url = endpoint_url(
                &r.images,
                "poster",
                "/api/v1/admin/tv/shows/",
                r.id,
                "/poster",
            );
            let backdrop_url = endpoint_url(
                &r.images,
                "backdrop",
                "/api/v1/admin/tv/shows/",
                r.id,
                "/backdrop",
            );
            let logo_url =
                endpoint_url(&r.images, "logo", "/api/v1/admin/tv/shows/", r.id, "/logo");
            ShowListResponse {
                id: r.id,
                title: r.title,
                sort_title: r.sort_title,
                year: r.year,
                poster_url,
                backdrop_url,
                logo_url,
                identifiers: r.identifiers,
                status: r.status,
                network: r.network,
                is_monitored: r.is_monitored,
                auto_want_mode: r.auto_want_mode,
                content_category: r.content_category,
                streamed_at: r.streamed_at,
                created_at: r.created_at,
                updated_at: r.updated_at,
                aired_episode_count: r.aired_episode_count,
                in_library_count: r.in_library_count,
            }
        })
        .collect()
}

// =============================================================================
// Create Show
// =============================================================================

/// POST /api/v1/admin/tv/shows
pub async fn create_show(
    State(state): State<AppState>,
    _auth: AuthUser,
    Json(body): Json<CreateShowRequest>,
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
        None => media_roots::get_default_for_type(&state.pool, "tv")
            .await
            .ok()
            .flatten()
            .map(|r| r.id),
    };

    let input_title = body.title.clone();
    let input = tv_shows::TvShowInput {
        title: body.title,
        sort_title: body.sort_title,
        original_title: body.original_title,
        year: body.year,
        status: body.status,
        description: body.description,
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
        network: body.network,
        quality_profile_id: body.quality_profile_id,
        preferred_languages: body.preferred_languages,
        media_root_id,
        first_aired: body.first_aired,
        runtime: body.runtime,
        ..Default::default()
    };

    // Use upsert when a provider ID is available to prevent duplicates
    let id = if let Some((ref ptype, ref pid)) = provider_info {
        tv_shows::upsert_by_provider(&state.pool, ptype, pid, input)
            .await
            .map_err(|e| AppError::Internal(format!("Failed to upsert show: {e}")))?
    } else {
        tv_shows::create(&state.pool, input)
            .await
            .map_err(|e| AppError::Internal(format!("Failed to create show: {e}")))?
    };

    // Enqueue async enrichment if provider identifier present
    if provider_info.is_some() {
        let display = crate::jobs::fetch_job_display(&state.pool, "tv_show", id).await;
        let context = JobContext::for_entity(
            "tv_show",
            id,
            format!("Enriching TV show '{}'", input_title),
        )
        .with_display_opt(display);
        if let Err(e) = jobs::enqueue(
            &state.pool,
            state.nats.publisher(),
            "tv_show_enrich",
            json!({ "show_id": id }),
            context,
        )
        .await
        {
            warn!(show_id = %id, error = %e, "Failed to enqueue TV show enrichment");
        }
    }

    let _ = state.library_events.send(LibraryEvent::tv_show_updated(id));

    Ok((StatusCode::CREATED, Json(json!({ "id": id }))))
}

// =============================================================================
// Get Show
// =============================================================================

/// GET /api/v1/admin/tv/shows/:id
pub async fn get_show(
    State(state): State<AppState>,
    auth: AuthUser,
    Path(id): Path<Uuid>,
) -> Result<impl IntoResponse, AppError> {
    let detail = tv_shows::get_with_details(&state.pool, id)
        .await
        .map_err(|e| AppError::Internal(format!("Failed to get show: {e}")))?
        .filter(|d| !auth.is_kids || d.kids_allowed)
        .ok_or_else(|| AppError::NotFound("Show not found".into()))?;

    let mut response = show_detail_to_response(detail);

    // Compute continue_episode from watch states
    let watch_states = tv_watch_states::list_for_show(&state.pool, id, auth.user_id)
        .await
        .map_err(|e| AppError::Internal(format!("Failed to get watch states: {e}")))?;

    response.continue_episode = compute_continue_episode(
        &response.seasons,
        &watch_states,
        response.content_category == "sport",
    );
    annotate_episodes_with_watch_states(&mut response.seasons, &watch_states);
    response.in_watchlist =
        crate::repositories::watchlist::contains(&state.pool, auth.user_id, "show", id)
            .await
            .unwrap_or(false);

    Ok(Json(response))
}

// =============================================================================
// Update Show
// =============================================================================

/// PATCH /api/v1/admin/tv/shows/:id
pub async fn update_show(
    State(state): State<AppState>,
    auth: AuthUser,
    Path(id): Path<Uuid>,
    Json(body): Json<UpdateShowRequest>,
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

    let input = tv_shows::TvShowUpdate {
        title: body.title,
        sort_title: body.sort_title,
        original_title: body.original_title,
        year: body.year,
        status: body.status,
        description: body.description,
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
        network: body.network,
        first_aired: body.first_aired,
        last_aired: body.last_aired,
        next_airing: body.next_airing,
        runtime: body.runtime,
        total_episode_count: body.total_episode_count,
        quality_profile_id: body.quality_profile_id,
        preferred_languages: body.preferred_languages,
        alternate_titles: None,
        media_root_id: body.media_root_id,
        sport_metadata: None,
        streamed_at: None,
    };

    let updated = tv_shows::update(&state.pool, id, input)
        .await
        .map_err(|e| AppError::Internal(format!("Failed to update show: {e}")))?;

    if !updated {
        return Err(AppError::NotFound("Show not found".into()));
    }

    if let Some(kids_override) = kids_override {
        tv_shows::set_kids_override(&state.pool, id, kids_override)
            .await
            .map_err(|e| AppError::Internal(format!("Failed to set kids override: {e}")))?;
    }

    let _ = state.library_events.send(LibraryEvent::tv_show_updated(id));

    Ok(StatusCode::NO_CONTENT)
}

// =============================================================================
// Delete Show
// =============================================================================

/// DELETE /api/v1/admin/tv/shows/:id
pub async fn delete_show(
    State(state): State<AppState>,
    auth: AuthUser,
    Path(id): Path<Uuid>,
) -> Result<impl IntoResponse, AppError> {
    // Capture title before delete for activity log
    let show_title = sqlx::query_scalar!("SELECT title FROM tv_shows WHERE id = $1", id)
        .fetch_optional(&state.pool)
        .await
        .ok()
        .flatten();

    // Collect all episode file paths before CASCADE delete
    let file_entries = tv_episode_files::list_paths_by_show(&state.pool, id)
        .await
        .unwrap_or_default();

    let deleted = tv_shows::delete(&state.pool, id)
        .await
        .map_err(|e| AppError::Internal(format!("Failed to delete show: {e}")))?;

    if !deleted {
        return Err(AppError::NotFound("Show not found".into()));
    }

    // Watchlist rows are polymorphic, so nothing cascades them.
    if let Err(e) = crate::repositories::watchlist::remove_for_media(&state.pool, "show", id).await
    {
        tracing::warn!(error = %e, show_id = %id, "failed to clear watchlist rows for deleted show");
    }

    let _ = state.library_events.send(LibraryEvent::tv_show_updated(id));

    // Record activity
    if let Some(title) = &show_title {
        crate::routes::library::helpers::record_activity(
            &state.pool,
            "deleted",
            Some("show"),
            Some(id),
            title,
            None,
            None,
            None,
            Some(auth.user_id),
        )
        .await;
    }

    // Best-effort cleanup of physical files and thumbnails
    let file_paths: Vec<String> = file_entries.iter().map(|f| f.file_path.clone()).collect();
    crate::repositories::cleanup_replaced_files(&file_paths).await;
    for entry in &file_entries {
        crate::repositories::cleanup_thumbnail_dir(&state.config.storage_root, entry.id).await;
    }

    Ok(StatusCode::NO_CONTENT)
}

// =============================================================================
// Set Monitor
// =============================================================================

const VALID_SHOW_AUTO_WANT_MODES: &[&str] = &["none", "new_only", "all", "fill_gaps"];
const VALID_SEASON_AUTO_WANT_MODES: &[&str] = &["inherit", "none", "new_only", "all", "fill_gaps"];

/// POST /api/v1/admin/tv/shows/:id/monitor
pub async fn set_monitor(
    State(state): State<AppState>,
    _auth: AuthUser,
    Path(id): Path<Uuid>,
    Json(body): Json<MonitorRequest>,
) -> Result<impl IntoResponse, AppError> {
    let mode = body.auto_want_mode.as_deref().unwrap_or("none");

    if !VALID_SHOW_AUTO_WANT_MODES.contains(&mode) {
        return Err(AppError::BadRequest(format!(
            "Invalid auto_want_mode: {mode}. Must be one of: {}",
            VALID_SHOW_AUTO_WANT_MODES.join(", ")
        )));
    }

    let updated = tv_shows::set_monitored(&state.pool, id, body.is_monitored, mode)
        .await
        .map_err(|e| AppError::Internal(format!("Failed to set monitor: {e}")))?;

    if !updated {
        return Err(AppError::NotFound("Show not found".into()));
    }

    // Applied to the seasons that exist now; enrichment applies it again to the ones that
    // arrive later, which for a show added from Discover is all of them.
    match crate::jobs::handlers::auto_want::apply_show_auto_want(
        &state.pool,
        state.nats.publisher(),
        id,
    )
    .await
    {
        Ok(count) if count > 0 => {
            info!(show_id = %id, episodes = count, "Auto-want applied immediately on monitor change");
        }
        Err(e) => {
            warn!(show_id = %id, error = %e, "Failed to apply auto-want on monitor change");
        }
        _ => {}
    }

    let _ = state.library_events.send(LibraryEvent::tv_show_updated(id));

    Ok(StatusCode::NO_CONTENT)
}

// =============================================================================
// Bulk Update
// =============================================================================

#[derive(Debug, Deserialize)]
pub struct BulkShowUpdateRequest {
    pub show_ids: Vec<Uuid>,
    pub auto_want_mode: Option<String>,
    /// Outer Option = field provided; inner Option = value or null (clear).
    pub quality_profile_id: Option<Option<Uuid>>,
}

/// POST /api/v1/admin/tv/shows/bulk
pub async fn bulk_update_shows(
    State(state): State<AppState>,
    _auth: AuthUser,
    Json(body): Json<BulkShowUpdateRequest>,
) -> Result<impl IntoResponse, AppError> {
    if body.show_ids.is_empty() {
        return Err(AppError::BadRequest("show_ids must not be empty".into()));
    }

    if let Some(ref mode) = body.auto_want_mode
        && !VALID_SHOW_AUTO_WANT_MODES.contains(&mode.as_str())
    {
        return Err(AppError::BadRequest(format!(
            "Invalid auto_want_mode: {mode}. Must be one of: {}",
            VALID_SHOW_AUTO_WANT_MODES.join(", ")
        )));
    }

    tv_shows::bulk_update(
        &state.pool,
        &body.show_ids,
        body.auto_want_mode.as_deref(),
        body.quality_profile_id,
    )
    .await
    .map_err(|e| AppError::Internal(format!("Failed to bulk update shows: {e}")))?;

    // Apply auto_want immediately if mode was changed to something active
    if let Some(ref mode) = body.auto_want_mode
        && mode != "none"
    {
        for show_id in &body.show_ids {
            let seasons = tv_seasons::list_by_show(&state.pool, *show_id)
                .await
                .unwrap_or_default();
            for season in &seasons {
                let effective_mode =
                    crate::jobs::handlers::auto_want::resolve_effective_mode(season, mode);
                if let Err(e) = crate::jobs::handlers::auto_want::apply_auto_want(
                    &state.pool,
                    state.nats.publisher(),
                    *show_id,
                    season.id,
                    effective_mode,
                )
                .await
                {
                    warn!(
                        show_id = %show_id,
                        season_id = %season.id,
                        error = %e,
                        "Failed to apply auto-want on bulk update"
                    );
                }
            }
        }
    }

    let _ = state
        .library_events
        .send(LibraryEvent::tv_show_updated(body.show_ids[0]));

    Ok(StatusCode::NO_CONTENT)
}

// =============================================================================
// Refresh Metadata
// =============================================================================

/// POST /api/v1/admin/tv/shows/:id/refresh-metadata
pub async fn refresh_metadata(
    State(state): State<AppState>,
    _auth: AuthUser,
    Path(id): Path<Uuid>,
) -> Result<impl IntoResponse, AppError> {
    // Get show from DB
    let show = tv_shows::get_with_details(&state.pool, id)
        .await
        .map_err(|e| AppError::Internal(format!("Failed to get show: {e}")))?
        .ok_or_else(|| AppError::NotFound("Show not found".into()))?;

    // Verify TMDB ID exists before attempting enrichment
    show.identifiers
        .get("tmdb_id")
        .and_then(|v| v.as_str())
        .ok_or_else(|| AppError::BadRequest("Show has no TMDB identifier".into()))?;

    let display = crate::jobs::fetch_job_display(&state.pool, "tv_show", id).await;
    let context = JobContext::for_entity(
        "tv_show",
        id,
        format!("Refreshing TV show '{}'", show.title),
    )
    .with_display_opt(display);
    jobs::enqueue(
        &state.pool,
        state.nats.publisher(),
        "tv_show_enrich",
        json!({ "show_id": id, "invalidate_cache": true }),
        context,
    )
    .await
    .map_err(|e| AppError::Internal(format!("Failed to enqueue enrichment: {e}")))?;

    Ok((
        StatusCode::ACCEPTED,
        Json(json!({ "message": "Enrichment queued" })),
    ))
}

// =============================================================================
// Want All Missing
// =============================================================================

/// POST /api/v1/admin/tv/shows/:id/want-all-missing
pub async fn want_all_missing(
    State(state): State<AppState>,
    auth: AuthUser,
    Path(id): Path<Uuid>,
) -> Result<impl IntoResponse, AppError> {
    // Verify show exists
    tv_shows::get(&state.pool, id)
        .await
        .map_err(|e| AppError::Internal(format!("Failed to get show: {e}")))?
        .ok_or_else(|| AppError::NotFound("Show not found".into()))?;

    let seasons = tv_seasons::list_by_show(&state.pool, id)
        .await
        .map_err(|e| AppError::Internal(format!("Failed to list seasons: {e}")))?;

    for season in &seasons {
        let missing = tv_episodes::list_missing(&state.pool, season.id)
            .await
            .map_err(|e| AppError::Internal(format!("Failed to list missing episodes: {e}")))?;

        if missing.is_empty() {
            continue;
        }

        let episode_ids: Vec<Uuid> = missing.iter().map(|ep| ep.id).collect();

        tv_episodes::set_want_status_batch(&state.pool, &episode_ids, "wanted")
            .await
            .map_err(|e| AppError::Internal(format!("Failed to set want status: {e}")))?;

        let _ = state
            .nats
            .publisher()
            .enqueue_job(
                "kobbler",
                "tv_want",
                json!({
                    "show_id": id,
                    "season_id": season.id,
                    "episode_ids": episode_ids,
                    "user_id": auth.user_id,
                }),
            )
            .await;
    }

    let _ = state.library_events.send(LibraryEvent::tv_show_updated(id));

    Ok(StatusCode::NO_CONTENT)
}

// =============================================================================
// Unwant All
// =============================================================================

/// DELETE /api/v1/admin/tv/shows/:id/want-all-missing
///
/// The inverse of `want_all_missing`: drop every outstanding want on the show.
/// Episodes already in the library keep their status.
pub async fn unwant_all(
    State(state): State<AppState>,
    _auth: AuthUser,
    Path(id): Path<Uuid>,
) -> Result<impl IntoResponse, AppError> {
    let show = tv_shows::get(&state.pool, id)
        .await
        .map_err(|e| AppError::Internal(format!("Failed to get show: {e}")))?
        .ok_or_else(|| AppError::NotFound("Show not found".into()))?;

    let cleared = tv_episodes::clear_wanted_in_show(&state.pool, id)
        .await
        .map_err(|e| AppError::Internal(format!("Failed to clear want status: {e}")))?;

    // Leaving a refilling auto-want mode on would have tv_monitor re-want
    // everything within the hour, so the unwant has to turn it off to stick.
    let auto_want_disabled = crate::jobs::handlers::auto_want::mode_refills(&show.auto_want_mode);
    if auto_want_disabled {
        tv_shows::set_auto_want_mode(&state.pool, id, "none")
            .await
            .map_err(|e| AppError::Internal(format!("Failed to disable auto-want: {e}")))?;
    }
    let seasons_disabled = tv_seasons::disable_refilling_auto_want_for_show(&state.pool, id)
        .await
        .map_err(|e| AppError::Internal(format!("Failed to disable season auto-want: {e}")))?;

    info!(
        show_id = %id,
        episodes = cleared.len(),
        auto_want_disabled,
        seasons_disabled,
        "Unwanted all episodes for show"
    );

    let _ = state.library_events.send(LibraryEvent::tv_show_updated(id));

    Ok(Json(json!({
        "unwanted_count": cleared.len(),
        "auto_want_disabled": auto_want_disabled || seasons_disabled > 0,
    })))
}

// =============================================================================
// Get Season
// =============================================================================

/// GET /api/v1/admin/tv/seasons/:id
pub async fn get_season(
    State(state): State<AppState>,
    auth: AuthUser,
    Path(id): Path<Uuid>,
) -> Result<impl IntoResponse, AppError> {
    let season = tv_seasons::get(&state.pool, id)
        .await
        .map_err(|e| AppError::Internal(format!("Failed to get season: {e}")))?
        .ok_or_else(|| AppError::NotFound("Season not found".into()))?;
    if auth.is_kids
        && !crate::kids::show_allowed(&state.pool, season.show_id)
            .await
            .map_err(|e| AppError::Internal(format!("Failed to check season: {e}")))?
    {
        return Err(AppError::NotFound("Season not found".into()));
    }

    let episodes = tv_episodes::list_by_season(&state.pool, id)
        .await
        .map_err(|e| AppError::Internal(format!("Failed to list episodes: {e}")))?;

    let watch_states = tv_watch_states::list_for_season(&state.pool, id, auth.user_id)
        .await
        .map_err(|e| AppError::Internal(format!("Failed to get watch states: {e}")))?;

    let mut response = SeasonResponse {
        id: season.id,
        show_id: season.show_id,
        season_identifier: season.season_identifier,
        season_sort_order: season.season_sort_order,
        title: season.title,
        description: season.description,
        poster_url: endpoint_url(
            &season.images,
            "poster",
            "/api/v1/admin/tv/seasons/",
            season.id,
            "/poster",
        ),
        air_date: season.air_date,
        episode_count: season.episode_count,
        is_monitored: season.is_monitored,
        is_special: season.is_special,
        auto_want_mode: season.auto_want_mode,
        created_at: season.created_at,
        episodes: episodes.into_iter().map(episode_row_to_response).collect(),
    };

    let ws_map: std::collections::HashMap<Uuid, &tv_watch_states::TvWatchStateRow> =
        watch_states.iter().map(|ws| (ws.episode_id, ws)).collect();
    for ep in response.episodes.iter_mut() {
        if let Some(ws) = ws_map.get(&ep.id) {
            ep.watched = Some(ws.watched);
            ep.watch_position_seconds = ws.position_seconds;
        }
    }

    Ok(Json(response))
}

// =============================================================================
// Want Season
// =============================================================================

/// POST /api/v1/admin/tv/seasons/:id/want
pub async fn want_season(
    State(state): State<AppState>,
    auth: AuthUser,
    Path(id): Path<Uuid>,
) -> Result<impl IntoResponse, AppError> {
    let season = tv_seasons::get(&state.pool, id)
        .await
        .map_err(|e| AppError::Internal(format!("Failed to get season: {e}")))?
        .ok_or_else(|| AppError::NotFound("Season not found".into()))?;

    // tv_want only covers episodes that are already wanted (or in library), so
    // the missing ones have to be flipped here or the job finds nothing to do.
    let missing = tv_episodes::list_missing(&state.pool, id)
        .await
        .map_err(|e| AppError::Internal(format!("Failed to list missing episodes: {e}")))?;
    if !missing.is_empty() {
        let episode_ids: Vec<Uuid> = missing.iter().map(|ep| ep.id).collect();
        tv_episodes::set_want_status_batch(&state.pool, &episode_ids, "wanted")
            .await
            .map_err(|e| AppError::Internal(format!("Failed to set want status: {e}")))?;
    }

    let _ = state
        .nats
        .publisher()
        .enqueue_job(
            "kobbler",
            "tv_want",
            json!({
                "show_id": season.show_id,
                "season_id": season.id,
                "user_id": auth.user_id,
            }),
        )
        .await;

    let _ = state
        .library_events
        .send(LibraryEvent::tv_show_updated(season.show_id));
    let _ = state
        .library_events
        .send(LibraryEvent::tv_season_updated(id));

    Ok(StatusCode::NO_CONTENT)
}

// =============================================================================
// Unwant Season
// =============================================================================

/// DELETE /api/v1/admin/tv/seasons/:id/want
///
/// The inverse of `want_season`: drop every outstanding want in the season.
/// Episodes already in the library keep their status.
pub async fn unwant_season(
    State(state): State<AppState>,
    _auth: AuthUser,
    Path(id): Path<Uuid>,
) -> Result<impl IntoResponse, AppError> {
    let season = tv_seasons::get(&state.pool, id)
        .await
        .map_err(|e| AppError::Internal(format!("Failed to get season: {e}")))?
        .ok_or_else(|| AppError::NotFound("Season not found".into()))?;

    let cleared = tv_episodes::clear_wanted_in_season(&state.pool, id)
        .await
        .map_err(|e| AppError::Internal(format!("Failed to clear want status: {e}")))?;

    // As with the show-level unwant, a refilling auto-want mode would put the
    // episodes straight back. Pinning the season to 'none' overrides the show
    // default without changing it for the other seasons.
    let show = tv_shows::get(&state.pool, season.show_id)
        .await
        .map_err(|e| AppError::Internal(format!("Failed to get show: {e}")))?
        .ok_or_else(|| AppError::Internal("Show not found for season".into()))?;
    let effective_mode =
        crate::jobs::handlers::auto_want::resolve_effective_mode(&season, &show.auto_want_mode);
    let auto_want_disabled = crate::jobs::handlers::auto_want::mode_refills(effective_mode);
    if auto_want_disabled {
        tv_seasons::set_auto_want_mode(&state.pool, id, "none")
            .await
            .map_err(|e| AppError::Internal(format!("Failed to disable auto-want: {e}")))?;
    }

    info!(
        season_id = %id,
        episodes = cleared.len(),
        auto_want_disabled,
        "Unwanted all episodes for season"
    );

    let _ = state
        .library_events
        .send(LibraryEvent::tv_show_updated(season.show_id));
    let _ = state
        .library_events
        .send(LibraryEvent::tv_season_updated(id));

    Ok(Json(json!({
        "unwanted_count": cleared.len(),
        "auto_want_disabled": auto_want_disabled,
    })))
}

// =============================================================================
// Set Season Auto-Want
// =============================================================================

#[derive(Debug, Deserialize)]
pub struct SeasonAutoWantRequest {
    pub auto_want_mode: String,
}

/// POST /api/v1/admin/tv/seasons/:id/auto-want
pub async fn set_season_auto_want(
    State(state): State<AppState>,
    _auth: AuthUser,
    Path(id): Path<Uuid>,
    Json(body): Json<SeasonAutoWantRequest>,
) -> Result<impl IntoResponse, AppError> {
    if !VALID_SEASON_AUTO_WANT_MODES.contains(&body.auto_want_mode.as_str()) {
        return Err(AppError::BadRequest(format!(
            "Invalid auto_want_mode: {}. Must be one of: {}",
            body.auto_want_mode,
            VALID_SEASON_AUTO_WANT_MODES.join(", ")
        )));
    }

    let season = tv_seasons::get(&state.pool, id)
        .await
        .map_err(|e| AppError::Internal(format!("Failed to get season: {e}")))?
        .ok_or_else(|| AppError::NotFound("Season not found".into()))?;

    let updated = tv_seasons::set_auto_want_mode(&state.pool, id, &body.auto_want_mode)
        .await
        .map_err(|e| AppError::Internal(format!("Failed to set season auto-want: {e}")))?;

    if !updated {
        return Err(AppError::NotFound("Season not found".into()));
    }

    // Apply immediately — resolve effective mode and enqueue wants now
    let show = tv_shows::get(&state.pool, season.show_id)
        .await
        .map_err(|e| AppError::Internal(format!("Failed to get show: {e}")))?
        .ok_or_else(|| AppError::Internal("Show not found for season".into()))?;

    // Re-read season to get the just-saved auto_want_mode
    let updated_season = tv_seasons::get(&state.pool, id)
        .await
        .map_err(|e| AppError::Internal(format!("Failed to re-read season: {e}")))?
        .ok_or_else(|| AppError::Internal("Season disappeared".into()))?;
    let effective_mode = crate::jobs::handlers::auto_want::resolve_effective_mode(
        &updated_season,
        &show.auto_want_mode,
    );
    match crate::jobs::handlers::auto_want::apply_auto_want(
        &state.pool,
        state.nats.publisher(),
        season.show_id,
        id,
        effective_mode,
    )
    .await
    {
        Ok(count) if count > 0 => {
            info!(season_id = %id, episodes = count, "Auto-want applied immediately");
        }
        Err(e) => {
            warn!(season_id = %id, error = %e, "Failed to apply auto-want immediately");
        }
        _ => {}
    }

    let _ = state
        .library_events
        .send(LibraryEvent::tv_show_updated(season.show_id));

    Ok(StatusCode::NO_CONTENT)
}

// =============================================================================
// Get Episode
// =============================================================================

/// GET /api/v1/admin/tv/episodes/:id
pub async fn get_episode(
    State(state): State<AppState>,
    auth: AuthUser,
    Path(id): Path<Uuid>,
) -> Result<impl IntoResponse, AppError> {
    let episode = tv_episodes::get(&state.pool, id)
        .await
        .map_err(|e| AppError::Internal(format!("Failed to get episode: {e}")))?
        .ok_or_else(|| AppError::NotFound("Episode not found".into()))?;
    if auth.is_kids
        && !crate::kids::episode_allowed(&state.pool, id)
            .await
            .map_err(|e| AppError::Internal(format!("Failed to check episode: {e}")))?
    {
        return Err(AppError::NotFound("Episode not found".into()));
    }

    Ok(Json(episode_row_to_response(episode)))
}

// =============================================================================
// Next Episode
// =============================================================================

/// GET /api/v1/admin/tv/episodes/:id/next
pub async fn next_episode(
    State(state): State<AppState>,
    _auth: PlaybackAuth,
    Path(episode_id): Path<Uuid>,
) -> impl IntoResponse {
    match tv_episodes::get_next_episode(&state.pool, episode_id).await {
        Ok(Some(info)) => Json(info).into_response(),
        Ok(None) => StatusCode::NOT_FOUND.into_response(),
        Err(e) => {
            warn!(error = %e, "Failed to get next episode");
            StatusCode::INTERNAL_SERVER_ERROR.into_response()
        }
    }
}

// =============================================================================
// Update Episode
// =============================================================================

#[derive(Debug, Deserialize)]
pub struct UpdateEpisodeRequest {
    pub title: Option<String>,
    pub description: Option<String>,
    pub still_url: Option<String>,
    #[serde(default, with = "crate::serde_date::option_date")]
    pub air_date: Option<time::Date>,
    pub runtime: Option<i32>,
}

/// PATCH /api/v1/admin/tv/episodes/:id
pub async fn update_episode(
    State(state): State<AppState>,
    _auth: AuthUser,
    Path(id): Path<Uuid>,
    Json(body): Json<UpdateEpisodeRequest>,
) -> Result<impl IntoResponse, AppError> {
    let ep_images = body.still_url.map(|url| {
        let mut m = std::collections::HashMap::new();
        m.insert(
            "still".to_string(),
            ImageEntry {
                source_url: Some(url),
                cached_at: None,
            },
        );
        m
    });

    let input = tv_episodes::EpisodeUpdate {
        title: body.title,
        description: body.description,
        images: ep_images,
        air_date: body.air_date,
        runtime: body.runtime,
    };

    let episode = tv_episodes::get(&state.pool, id)
        .await
        .map_err(|e| AppError::Internal(format!("Failed to get episode: {e}")))?
        .ok_or_else(|| AppError::NotFound("Episode not found".into()))?;

    let season = tv_seasons::get(&state.pool, episode.season_id)
        .await
        .map_err(|e| AppError::Internal(format!("Failed to get season: {e}")))?
        .ok_or_else(|| AppError::Internal("Season not found for episode".into()))?;

    let updated = tv_episodes::update(&state.pool, id, input)
        .await
        .map_err(|e| AppError::Internal(format!("Failed to update episode: {e}")))?;

    if !updated {
        return Err(AppError::NotFound("Episode not found".into()));
    }

    let _ = state
        .library_events
        .send(LibraryEvent::tv_show_updated(season.show_id));
    let _ = state
        .library_events
        .send(LibraryEvent::tv_episode_updated(id));

    Ok(StatusCode::NO_CONTENT)
}

// =============================================================================
// Want Episode
// =============================================================================

/// POST /api/v1/admin/tv/episodes/:id/want
pub async fn want_episode(
    State(state): State<AppState>,
    auth: AuthUser,
    Path(id): Path<Uuid>,
    Query(copy): Query<AcquireQuery>,
) -> Result<impl IntoResponse, AppError> {
    let episode = tv_episodes::get(&state.pool, id)
        .await
        .map_err(|e| AppError::Internal(format!("Failed to get episode: {e}")))?
        .ok_or_else(|| AppError::NotFound("Episode not found".into()))?;

    let season = tv_seasons::get(&state.pool, episode.season_id)
        .await
        .map_err(|e| AppError::Internal(format!("Failed to get season: {e}")))?
        .ok_or_else(|| AppError::Internal("Season not found for episode".into()))?;

    tv_episodes::set_want_status(&state.pool, id, "wanted")
        .await
        .map_err(|e| AppError::Internal(format!("Failed to set want status: {e}")))?;

    let _ = state
        .nats
        .publisher()
        .enqueue_job(
            "kobbler",
            "tv_want",
            json!({
                "show_id": season.show_id,
                "season_id": season.id,
                "episode_ids": [id],
                "user_id": auth.user_id,
                "additional_copy": copy.additional_copy,
                "manual": copy.manual,
            }),
        )
        .await;

    let _ = state
        .library_events
        .send(LibraryEvent::tv_show_updated(season.show_id));
    let _ = state
        .library_events
        .send(LibraryEvent::tv_episode_updated(id));

    // Check if we have a complete debrid cache for this episode
    if let Ok(Some(session)) = sqlx::query!(
        r#"
        SELECT id, cache_path AS "cache_path?", filesize
        FROM debrid_stream_sessions
        WHERE media_type = 'episode' AND media_id = $1 AND cache_path IS NOT NULL
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
            info!(episode_id = %id, cache_path = %cache_path, "Complete debrid cache available for wanted episode");
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
// Unwant Episode
// =============================================================================

/// DELETE /api/v1/admin/tv/episodes/:id/want
pub async fn unwant_episode(
    State(state): State<AppState>,
    _auth: AuthUser,
    Path(id): Path<Uuid>,
) -> Result<impl IntoResponse, AppError> {
    let episode = tv_episodes::get(&state.pool, id)
        .await
        .map_err(|e| AppError::Internal(format!("Failed to get episode: {e}")))?
        .ok_or_else(|| AppError::NotFound("Episode not found".into()))?;

    let season = tv_seasons::get(&state.pool, episode.season_id)
        .await
        .map_err(|e| AppError::Internal(format!("Failed to get season: {e}")))?
        .ok_or_else(|| AppError::Internal("Season not found for episode".into()))?;

    tv_episodes::set_want_status(&state.pool, id, "none")
        .await
        .map_err(|e| AppError::Internal(format!("Failed to set want status: {e}")))?;

    let _ = state
        .library_events
        .send(LibraryEvent::tv_show_updated(season.show_id));
    let _ = state
        .library_events
        .send(LibraryEvent::tv_episode_updated(id));

    Ok(StatusCode::NO_CONTENT)
}

// =============================================================================
// List Episode Files
// =============================================================================

/// GET /api/v1/admin/tv/episodes/:id/files
pub async fn list_episode_files(
    State(state): State<AppState>,
    _auth: AuthUser,
    Path(id): Path<Uuid>,
) -> Result<impl IntoResponse, AppError> {
    let files = tv_episode_files::list_by_episode(&state.pool, id)
        .await
        .map_err(|e| AppError::Internal(format!("Failed to list episode files: {e}")))?;

    let mut response = Vec::new();
    for f in files {
        let episode_ids = tv_episode_files::get_episode_ids(&state.pool, f.id)
            .await
            .unwrap_or_default();
        response.push(EpisodeFileResponse {
            id: f.id,
            episode_ids,
            file_path: f.file_path,
            file_size: f.file_size,
            format: f.format,
            quality: f.quality,
            languages: f.languages,
            subtitles: f.subtitles,
            source_type: f.source_type,
            source_metadata: f.source_metadata,
            available: f.available,
            created_at: f.created_at,
        });
    }

    Ok(Json(response))
}

// =============================================================================
// Add Episode File
// =============================================================================

/// POST /api/v1/admin/tv/episodes/:id/files
pub async fn add_episode_file(
    State(state): State<AppState>,
    _auth: AuthUser,
    Path(episode_id): Path<Uuid>,
    Json(body): Json<AddEpisodeFileRequest>,
) -> Result<impl IntoResponse, AppError> {
    if body.file_path.trim().is_empty() {
        return Err(AppError::BadRequest("file_path is required".into()));
    }
    if body.format.trim().is_empty() {
        return Err(AppError::BadRequest("format is required".into()));
    }

    // Verify episode exists
    tv_episodes::get(&state.pool, episode_id)
        .await
        .map_err(|e| AppError::Internal(format!("Failed to get episode: {e}")))?
        .ok_or_else(|| AppError::NotFound("Episode not found".into()))?;

    let input = tv_episode_files::EpisodeFileInput {
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
        is_interlaced: false,
        hdr_format: None,
        duration_seconds: None,
        container: None,
        overall_bitrate_kbps: None,
    };

    let file_id = tv_episode_files::create(&state.pool, input)
        .await
        .map_err(|e| AppError::Internal(format!("Failed to add episode file: {e}")))?;

    let replaced = tv_episode_files::link_episodes(&state.pool, file_id, &[episode_id])
        .await
        .map_err(|e| AppError::Internal(format!("Failed to link episode file: {e}")))?;
    crate::repositories::cleanup_replaced_files(&replaced).await;

    // Enqueue media probe job to populate video metadata via ffprobe
    let probe_display =
        crate::jobs::fetch_job_display(&state.pool, "tv_episode_file", file_id).await;
    let probe_context = crate::jobs::JobContext::for_entity(
        "tv_episode_file",
        file_id,
        "Probing media file".to_string(),
    )
    .with_display_opt(probe_display);
    let _ = crate::jobs::enqueue(
        &state.pool,
        state.nats.publisher(),
        "media_probe",
        json!({"file_id": file_id, "media_type": "episode"}),
        probe_context,
    )
    .await;

    Ok((StatusCode::CREATED, Json(json!({ "id": file_id }))))
}

// =============================================================================
// File Artifact Cleanup
// =============================================================================

/// Drop everything derived from a set of episode files: the persistent
/// episode_cache copy, the file on disk, its thumbnails and its per-file
/// media_cache. Every step is best-effort — a missing artifact is not an error,
/// and one failure must not strand the rest.
///
/// The `tv_episode_files` rows themselves are the caller's business: single
/// deletes remove one row, a whole-episode delete lets CASCADE take the
/// junction and sweeps the rows separately.
async fn purge_file_artifacts(state: &AppState, files: &[tv_episode_files::FilePathEntry]) {
    for f in files {
        if let Err(e) = crate::media_cache::episode_cache::remove(
            &state.config.storage_root,
            &crate::media_cache::episode_cache::MediaKind::TvFile { file_id: f.id },
        ) {
            tracing::warn!(error = %e, file_id = %f.id, "failed to remove episode_cache for deleted tv_file");
        }
    }

    let paths: Vec<String> = files.iter().map(|f| f.file_path.clone()).collect();
    crate::repositories::cleanup_replaced_files(&paths).await;

    for f in files {
        crate::repositories::cleanup_thumbnail_dir(&state.config.storage_root, f.id).await;
        if let Err(e) = crate::media_cache::remove_per_file(&state.config.storage_root, f.id) {
            tracing::warn!(error = %e, file_id = %f.id, "failed to remove per-file media_cache");
        }
    }
}

// =============================================================================
// Delete Episode File
// =============================================================================

/// DELETE /api/v1/admin/tv/episodes/:id/files/:file_id
pub async fn delete_episode_file(
    State(state): State<AppState>,
    _auth: AuthUser,
    Path(path): Path<EpisodeFilePath>,
) -> Result<impl IntoResponse, AppError> {
    // Fetch file path before deleting
    let file = tv_episode_files::get(&state.pool, path.file_id)
        .await
        .map_err(|e| AppError::Internal(format!("Failed to get episode file: {e}")))?
        .ok_or_else(|| AppError::NotFound("Episode file not found".into()))?;

    let linked = tv_episode_files::delete(&state.pool, path.file_id)
        .await
        .map_err(|e| AppError::Internal(format!("Failed to delete episode file: {e}")))?
        .ok_or_else(|| AppError::NotFound("Episode file not found".into()))?;

    // Same reset as the bulk path: an episode that just lost its last copy has
    // to become wantable again instead of sitting at 'imported' forever.
    if let Err(e) = tv_episodes::clear_imported_without_files(&state.pool, &linked).await {
        tracing::warn!(error = %e, file_id = %path.file_id, "failed to reset want_status after file delete");
    }

    purge_file_artifacts(
        &state,
        &[tv_episode_files::FilePathEntry {
            id: path.file_id,
            file_path: file.file_path,
        }],
    )
    .await;

    Ok(StatusCode::NO_CONTENT)
}

// =============================================================================
// Move Episode File
// =============================================================================

#[derive(Debug, Deserialize)]
pub struct MoveEpisodeFileBody {
    pub target_episode_id: Uuid,
}

/// POST /api/v1/admin/tv/episodes/:id/files/:file_id/move
pub async fn move_episode_file(
    State(state): State<AppState>,
    _auth: AuthUser,
    Path(path): Path<EpisodeFilePath>,
    Json(body): Json<MoveEpisodeFileBody>,
) -> Result<impl IntoResponse, AppError> {
    // Verify target episode exists
    tv_episodes::get(&state.pool, body.target_episode_id)
        .await
        .map_err(|e| AppError::Internal(format!("Failed to check target episode: {e}")))?
        .ok_or_else(|| AppError::NotFound("Target episode not found".into()))?;

    let moved =
        tv_episode_files::set_episodes(&state.pool, path.file_id, &[body.target_episode_id])
            .await
            .map_err(|e| AppError::Internal(format!("Failed to move episode file: {e}")))?;

    if !moved {
        return Err(AppError::NotFound("Episode file not found".into()));
    }

    Ok(StatusCode::NO_CONTENT)
}

// =============================================================================
// Get Watch State
// =============================================================================

/// GET /api/v1/admin/tv/episodes/:id/watch-state
pub async fn get_watch_state(
    State(state): State<AppState>,
    auth: PlaybackAuth,
    Path(id): Path<Uuid>,
) -> Result<impl IntoResponse, AppError> {
    let user_id = auth.user_id;

    let ws = tv_watch_states::get(&state.pool, id, user_id)
        .await
        .map_err(|e| AppError::Internal(format!("Failed to get watch state: {e}")))?;

    match ws {
        Some(row) => Ok(Json(json!(TvWatchStateResponse {
            id: row.id,
            episode_id: row.episode_id,
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

/// PUT /api/v1/admin/tv/episodes/:id/watch-state
pub async fn update_watch_state(
    State(state): State<AppState>,
    auth: PlaybackAuth,
    Path(id): Path<Uuid>,
    Json(body): Json<UpdateWatchStateRequest>,
) -> Result<impl IntoResponse, AppError> {
    let user_id = auth.user_id;

    let input = tv_watch_states::TvWatchStateInput {
        episode_id: id,
        user_id,
        watched: body.watched,
        watched_at: body.watched_at,
        position_seconds: body.position_seconds,
        rating: body.rating,
    };

    let ws_id = tv_watch_states::upsert(&state.pool, input)
        .await
        .map_err(|e| AppError::Internal(format!("Failed to update watch state: {e}")))?;

    // Re-index the parent TV show in search (episodes_watched changed)
    if let Ok(Some(show_id)) = sqlx::query_scalar!(
        r#"SELECT tsn.show_id FROM tv_episodes te JOIN tv_seasons tsn ON te.season_id = tsn.id WHERE te.id = $1"#,
        id,
    )
    .fetch_optional(&state.pool)
    .await
    {
        crate::jobs::enqueue_tv_show_index(&state.pool, state.nats.publisher(), show_id).await;
    }

    // Fire-and-forget: sync watch state to Trakt (if configured)
    let _ = crate::jobs::enqueue_routine(
        &state.pool,
        state.nats.publisher(),
        "trakt_sync_watch_state",
        serde_json::json!({
            "user_id": user_id,
            "media_type": "episode",
            "media_id": id,
            "watched": body.watched,
            "watched_at": body.watched_at.map(|t| t.format(&time::format_description::well_known::Rfc3339).unwrap()),
        }),
        crate::jobs::JobContext::new("Syncing episode watch state to Trakt"),
    )
    .await;

    Ok(Json(json!({ "id": ws_id })))
}

// =============================================================================
// Bulk Watch State (Show / Season)
// =============================================================================

/// PUT /api/v1/admin/tv/shows/:id/watch-state
pub async fn bulk_update_show_watch_state(
    State(state): State<AppState>,
    auth: AuthUser,
    Path(id): Path<Uuid>,
    Json(body): Json<BulkWatchStateRequest>,
) -> Result<impl IntoResponse, AppError> {
    let user_id = auth.user_id;

    // Verify show exists
    tv_shows::get(&state.pool, id)
        .await
        .map_err(|e| AppError::Internal(format!("Failed to query show: {e}")))?
        .ok_or_else(|| AppError::NotFound("Show not found".to_string()))?;

    let affected_episode_ids = if body.watched {
        tv_watch_states::bulk_mark_show_watched(&state.pool, id, user_id, OffsetDateTime::now_utc())
            .await
    } else {
        tv_watch_states::bulk_mark_show_unwatched(&state.pool, id, user_id).await
    }
    .map_err(|e| AppError::Internal(format!("Failed to bulk update watch states: {e}")))?;

    let count = affected_episode_ids.len();

    after_bulk_watch(&state, user_id, id, &affected_episode_ids, body.watched).await;

    info!(show_id = %id, user_id = %user_id, watched = body.watched, count, "Bulk updated show watch state");

    Ok(Json(json!({ "updated_count": count })))
}

/// Re-indexes the show and syncs the changed episodes to Trakt in one call.
async fn after_bulk_watch(
    state: &AppState,
    user_id: Uuid,
    show_id: Uuid,
    episode_ids: &[Uuid],
    watched: bool,
) {
    crate::jobs::enqueue_tv_show_index(&state.pool, state.nats.publisher(), show_id).await;
    if episode_ids.is_empty() {
        return;
    }
    let watched_at = watched.then(|| {
        OffsetDateTime::now_utc()
            .format(&time::format_description::well_known::Rfc3339)
            .unwrap()
    });
    let _ = crate::jobs::enqueue_routine(
        &state.pool,
        state.nats.publisher(),
        "trakt_bulk_sync_watch_state",
        serde_json::json!({
            "user_id": user_id,
            "show_id": show_id,
            "episode_ids": episode_ids,
            "watched": watched,
            "watched_at": watched_at,
        }),
        crate::jobs::JobContext::new("Syncing bulk watch state to Trakt"),
    )
    .await;
}

/// PUT /api/v1/admin/tv/episodes/:id/watch-state/earlier
///
/// Marks every episode of the show before this one watched, specials aside: a long-running
/// sport otherwise keeps offering rounds the viewer has long moved past.
pub async fn mark_earlier_episodes_watched(
    State(state): State<AppState>,
    auth: AuthUser,
    Path(id): Path<Uuid>,
    Json(body): Json<BulkWatchStateRequest>,
) -> Result<impl IntoResponse, AppError> {
    if !body.watched {
        return Err(AppError::BadRequest(
            "Only marking earlier episodes watched is supported".to_string(),
        ));
    }
    let user_id = auth.user_id;
    let episode = tv_episodes::get(&state.pool, id)
        .await
        .map_err(|e| AppError::Internal(format!("Failed to query episode: {e}")))?
        .ok_or_else(|| AppError::NotFound("Episode not found".to_string()))?;
    let season = tv_seasons::get(&state.pool, episode.season_id)
        .await
        .map_err(|e| AppError::Internal(format!("Failed to query season: {e}")))?
        .ok_or_else(|| AppError::NotFound("Season not found".to_string()))?;

    let affected_episode_ids = tv_watch_states::bulk_mark_earlier_watched(
        &state.pool,
        season.show_id,
        season.season_sort_order,
        episode.episode_sort_order,
        user_id,
        OffsetDateTime::now_utc(),
    )
    .await
    .map_err(|e| AppError::Internal(format!("Failed to bulk update watch states: {e}")))?;

    let count = affected_episode_ids.len();
    after_bulk_watch(&state, user_id, season.show_id, &affected_episode_ids, true).await;
    info!(episode_id = %id, user_id = %user_id, count, "Marked earlier episodes watched");

    Ok(Json(json!({ "updated_count": count })))
}

/// PUT /api/v1/admin/tv/seasons/:id/watch-state
pub async fn bulk_update_season_watch_state(
    State(state): State<AppState>,
    auth: AuthUser,
    Path(id): Path<Uuid>,
    Json(body): Json<BulkWatchStateRequest>,
) -> Result<impl IntoResponse, AppError> {
    let user_id = auth.user_id;

    // Verify season exists and get show_id for search re-indexing
    let season = tv_seasons::get(&state.pool, id)
        .await
        .map_err(|e| AppError::Internal(format!("Failed to query season: {e}")))?
        .ok_or_else(|| AppError::NotFound("Season not found".to_string()))?;

    let affected_episode_ids = if body.watched {
        tv_watch_states::bulk_mark_season_watched(
            &state.pool,
            id,
            user_id,
            OffsetDateTime::now_utc(),
        )
        .await
    } else {
        tv_watch_states::bulk_mark_season_unwatched(&state.pool, id, user_id).await
    }
    .map_err(|e| AppError::Internal(format!("Failed to bulk update watch states: {e}")))?;

    let count = affected_episode_ids.len();

    after_bulk_watch(
        &state,
        user_id,
        season.show_id,
        &affected_episode_ids,
        body.watched,
    )
    .await;

    info!(season_id = %id, user_id = %user_id, watched = body.watched, count, "Bulk updated season watch state");

    Ok(Json(json!({ "updated_count": count })))
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

/// PATCH /api/v1/admin/tv/episodes/:id/watch-state/position
pub async fn update_episode_position(
    State(state): State<AppState>,
    auth: PlaybackAuth,
    Path(id): Path<Uuid>,
    Json(body): Json<UpdatePositionRequest>,
) -> Result<impl IntoResponse, AppError> {
    let user_id = auth.user_id;

    let ws_id = tv_watch_states::update_position(&state.pool, id, user_id, body.position_seconds)
        .await
        .map_err(|e| AppError::Internal(format!("Failed to update position: {e}")))?;

    let extent = crate::watch_completion::for_episode(&state.pool, id).await;

    if let Some(extent) = extent
        && extent.is_finished_at(body.position_seconds)
    {
        let input = tv_watch_states::TvWatchStateInput {
            episode_id: id,
            user_id,
            watched: true,
            watched_at: Some(OffsetDateTime::now_utc()),
            position_seconds: Some(body.position_seconds),
            rating: None,
        };
        let _ = tv_watch_states::upsert(&state.pool, input).await;

        // Re-index the parent TV show in search
        if let Ok(Some(show_id)) = sqlx::query_scalar!(
            r#"SELECT tsn.show_id FROM tv_episodes te JOIN tv_seasons tsn ON te.season_id = tsn.id WHERE te.id = $1"#,
            id,
        )
        .fetch_optional(&state.pool)
        .await
        {
            crate::jobs::enqueue_tv_show_index(&state.pool, state.nats.publisher(), show_id)
                .await;
        }

        // Sync to Trakt
        let _ = crate::jobs::enqueue_routine(
            &state.pool,
            state.nats.publisher(),
            "trakt_sync_watch_state",
            serde_json::json!({
                "user_id": user_id,
                "media_type": "episode",
                "media_id": id,
                "watched": true,
            }),
            crate::jobs::JobContext::new("Syncing episode watch state to Trakt"),
        )
        .await;
    } else if extent.is_some() {
        // Seeking back out of the ending: if auto-watched, reset to unwatched so it
        // re-appears in dashboard "in progress".
        if let Ok(Some(ws)) = tv_watch_states::get(&state.pool, id, user_id).await
            && ws.watched
        {
            let input = tv_watch_states::TvWatchStateInput {
                episode_id: id,
                user_id,
                watched: false,
                watched_at: None,
                position_seconds: Some(body.position_seconds),
                rating: None,
            };
            let _ = tv_watch_states::upsert(&state.pool, input).await;

            if let Ok(Some(show_id)) = sqlx::query_scalar!(
                r#"SELECT tsn.show_id FROM tv_episodes te JOIN tv_seasons tsn ON te.season_id = tsn.id WHERE te.id = $1"#,
                id,
            )
            .fetch_optional(&state.pool)
            .await
            {
                crate::jobs::enqueue_tv_show_index(
                    &state.pool,
                    state.nats.publisher(),
                    show_id,
                )
                .await;
            }
        }
    }

    crate::watch_handoff::episode_saved(
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
// Create Season
// =============================================================================

#[derive(Debug, Deserialize)]
pub struct CreateSeasonRequest {
    pub season_identifier: String,
    pub season_sort_order: i32,
    pub title: Option<String>,
    pub description: Option<String>,
    #[serde(default, with = "crate::serde_date::option_date")]
    pub air_date: Option<time::Date>,
    pub episode_count: Option<i32>,
    pub is_special: Option<bool>,
}

/// POST /api/v1/admin/tv/shows/:show_id/seasons
pub async fn create_season(
    State(state): State<AppState>,
    _auth: AuthUser,
    Path(show_id): Path<Uuid>,
    Json(body): Json<CreateSeasonRequest>,
) -> Result<impl IntoResponse, AppError> {
    if body.season_identifier.trim().is_empty() {
        return Err(AppError::BadRequest("season_identifier is required".into()));
    }

    // Verify show exists
    tv_shows::get(&state.pool, show_id)
        .await
        .map_err(|e| AppError::Internal(format!("Failed to get show: {e}")))?
        .ok_or_else(|| AppError::NotFound("Show not found".into()))?;

    let input = tv_seasons::SeasonInput {
        show_id,
        season_identifier: body.season_identifier,
        season_sort_order: body.season_sort_order,
        title: body.title,
        description: body.description,
        images: None,
        air_date: body.air_date,
        episode_count: body.episode_count,
        is_monitored: true,
        is_special: body.is_special.unwrap_or(false),
        auto_want_mode: "inherit".to_string(),
    };

    let id = tv_seasons::create(&state.pool, input)
        .await
        .map_err(|e| AppError::Internal(format!("Failed to create season: {e}")))?;

    let _ = state
        .library_events
        .send(LibraryEvent::tv_show_updated(show_id));
    let _ = state
        .library_events
        .send(LibraryEvent::tv_season_updated(id));

    Ok((StatusCode::CREATED, Json(json!({ "id": id }))))
}

// =============================================================================
// Update Season
// =============================================================================

#[derive(Debug, Deserialize)]
pub struct UpdateSeasonRequest {
    pub title: Option<String>,
    pub description: Option<String>,
    pub poster_url: Option<String>,
    #[serde(default, with = "crate::serde_date::option_date")]
    pub air_date: Option<time::Date>,
    pub episode_count: Option<i32>,
    pub is_special: Option<bool>,
}

/// PATCH /api/v1/admin/tv/seasons/:id
pub async fn update_season(
    State(state): State<AppState>,
    _auth: AuthUser,
    Path(id): Path<Uuid>,
    Json(body): Json<UpdateSeasonRequest>,
) -> Result<impl IntoResponse, AppError> {
    let season_images = body.poster_url.map(|url| {
        let mut m = std::collections::HashMap::new();
        m.insert(
            "poster".to_string(),
            ImageEntry {
                source_url: Some(url),
                cached_at: None,
            },
        );
        m
    });

    let input = tv_seasons::SeasonUpdate {
        title: body.title,
        description: body.description,
        images: season_images,
        air_date: body.air_date,
        episode_count: body.episode_count,
        is_special: body.is_special,
        ..Default::default()
    };

    let season = tv_seasons::get(&state.pool, id)
        .await
        .map_err(|e| AppError::Internal(format!("Failed to get season: {e}")))?
        .ok_or_else(|| AppError::NotFound("Season not found".into()))?;

    let updated = tv_seasons::update(&state.pool, id, input)
        .await
        .map_err(|e| AppError::Internal(format!("Failed to update season: {e}")))?;

    if !updated {
        return Err(AppError::NotFound("Season not found".into()));
    }

    let _ = state
        .library_events
        .send(LibraryEvent::tv_show_updated(season.show_id));
    let _ = state
        .library_events
        .send(LibraryEvent::tv_season_updated(id));

    Ok(StatusCode::NO_CONTENT)
}

// =============================================================================
// Delete Season
// =============================================================================

/// DELETE /api/v1/admin/tv/seasons/:id
pub async fn delete_season(
    State(state): State<AppState>,
    _auth: AuthUser,
    Path(id): Path<Uuid>,
) -> Result<impl IntoResponse, AppError> {
    let season = tv_seasons::get(&state.pool, id)
        .await
        .map_err(|e| AppError::Internal(format!("Failed to get season: {e}")))?
        .ok_or_else(|| AppError::NotFound("Season not found".into()))?;

    // Collect all episode file paths before CASCADE delete
    let file_entries = tv_episode_files::list_paths_by_season(&state.pool, id)
        .await
        .unwrap_or_default();

    let deleted = tv_seasons::delete(&state.pool, id)
        .await
        .map_err(|e| AppError::Internal(format!("Failed to delete season: {e}")))?;

    if !deleted {
        return Err(AppError::NotFound("Season not found".into()));
    }

    let _ = state
        .library_events
        .send(LibraryEvent::tv_show_updated(season.show_id));
    // Triggers search index cleanup (handler will see season is gone and delete from index)
    let _ = state
        .library_events
        .send(LibraryEvent::tv_season_updated(id));

    // Best-effort cleanup of physical files and thumbnails
    let file_paths: Vec<String> = file_entries.iter().map(|f| f.file_path.clone()).collect();
    crate::repositories::cleanup_replaced_files(&file_paths).await;
    for entry in &file_entries {
        crate::repositories::cleanup_thumbnail_dir(&state.config.storage_root, entry.id).await;
    }

    Ok(StatusCode::NO_CONTENT)
}

// =============================================================================
// Create Episode
// =============================================================================

#[derive(Debug, Deserialize)]
pub struct CreateEpisodeRequest {
    pub episode_identifier: String,
    pub episode_sort_order: i32,
    pub title: Option<String>,
    pub description: Option<String>,
    #[serde(default, with = "crate::serde_date::option_date")]
    pub air_date: Option<time::Date>,
    pub runtime: Option<i32>,
}

/// POST /api/v1/admin/tv/seasons/:season_id/episodes
pub async fn create_episode(
    State(state): State<AppState>,
    _auth: AuthUser,
    Path(season_id): Path<Uuid>,
    Json(body): Json<CreateEpisodeRequest>,
) -> Result<impl IntoResponse, AppError> {
    if body.episode_identifier.trim().is_empty() {
        return Err(AppError::BadRequest(
            "episode_identifier is required".into(),
        ));
    }

    // Verify season exists
    let season = tv_seasons::get(&state.pool, season_id)
        .await
        .map_err(|e| AppError::Internal(format!("Failed to get season: {e}")))?
        .ok_or_else(|| AppError::NotFound("Season not found".into()))?;

    let input = tv_episodes::EpisodeInput {
        season_id,
        episode_identifier: body.episode_identifier,
        episode_sort_order: body.episode_sort_order,
        title: body.title,
        description: body.description,
        images: None,
        air_date: body.air_date,
        air_time: None,
        runtime: body.runtime,
        identifiers: serde_json::json!({}),
        country: None,
    };

    let id = tv_episodes::create(&state.pool, input)
        .await
        .map_err(|e| AppError::Internal(format!("Failed to create episode: {e}")))?;

    let _ = state
        .library_events
        .send(LibraryEvent::tv_show_updated(season.show_id));
    let _ = state
        .library_events
        .send(LibraryEvent::tv_episode_updated(id));

    Ok((StatusCode::CREATED, Json(json!({ "id": id }))))
}

// =============================================================================
// Delete Episode
// =============================================================================

/// DELETE /api/v1/admin/tv/episodes/:id
pub async fn delete_episode(
    State(state): State<AppState>,
    _auth: AuthUser,
    Path(id): Path<Uuid>,
) -> Result<impl IntoResponse, AppError> {
    let episode = tv_episodes::get(&state.pool, id)
        .await
        .map_err(|e| AppError::Internal(format!("Failed to get episode: {e}")))?
        .ok_or_else(|| AppError::NotFound("Episode not found".into()))?;

    let season = tv_seasons::get(&state.pool, episode.season_id)
        .await
        .map_err(|e| AppError::Internal(format!("Failed to get season: {e}")))?
        .ok_or_else(|| AppError::Internal("Season not found for episode".into()))?;

    // Snapshot the files before the delete: CASCADE takes the junction rows, so
    // afterwards there is no way back from the episode to what it pointed at.
    let files = tv_episode_files::list_paths_by_episodes(&state.pool, &[id])
        .await
        .unwrap_or_default();

    let deleted = tv_episodes::delete(&state.pool, id)
        .await
        .map_err(|e| AppError::Internal(format!("Failed to delete episode: {e}")))?;

    if !deleted {
        return Err(AppError::NotFound("Episode not found".into()));
    }

    purge_file_artifacts(&state, &files).await;

    // debrid_episode/{ep_id}/ — the debrid history is now stale.
    if let Err(e) = crate::media_cache::episode_cache::remove(
        &state.config.storage_root,
        &crate::media_cache::episode_cache::MediaKind::DebridEpisode { episode_id: id },
    ) {
        tracing::warn!(error = %e, episode_id = %id, "failed to remove debrid_episode episode_cache");
    }

    let _ = state
        .library_events
        .send(LibraryEvent::tv_show_updated(season.show_id));
    // Triggers search index cleanup (handler will see episode is gone and delete from index)
    let _ = state
        .library_events
        .send(LibraryEvent::tv_episode_updated(id));

    Ok(StatusCode::NO_CONTENT)
}

// =============================================================================
// Bulk Episode Actions
// =============================================================================

#[derive(Debug, Deserialize, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum BulkEpisodeAction {
    /// Delete the media, keep the episode rows. The fix for "the importer
    /// grabbed the wrong show" — the episodes stay so they can be re-acquired.
    DeleteFiles,
    /// Delete the media *and* the episode rows. The fix for episodes that
    /// should not exist at all, e.g. left behind by a bad show match.
    DeleteEpisodes,
    Want,
    Unwant,
    MarkWatched,
    MarkUnwatched,
}

#[derive(Debug, Deserialize)]
pub struct BulkEpisodeRequest {
    pub episode_ids: Vec<Uuid>,
    pub action: BulkEpisodeAction,
}

#[derive(Debug, Serialize)]
pub struct BulkEpisodeResponse {
    /// Episode rows the action actually changed. For `unwant` this counts only
    /// the ones that had an outstanding want, so it can be lower than the
    /// selection — the same convention as the season-level unwant.
    pub episodes_affected: usize,
    pub files_deleted: usize,
    /// Set when an `unwant` had to pin a season's auto-want to 'none' to stop
    /// the hourly monitor refilling what was just cleared.
    pub auto_want_disabled: bool,
}

/// A single request may not touch more than this many episodes. Well above any
/// real show, and low enough that one call cannot lock the table for minutes.
const BULK_EPISODE_LIMIT: usize = 2000;

/// POST /api/v1/admin/tv/episodes/bulk
///
/// Apply one action to an arbitrary set of episodes, which may span seasons and
/// shows. Unknown IDs are ignored rather than failing the batch: the client's
/// selection can race a delete from another tab.
pub async fn bulk_episodes(
    State(state): State<AppState>,
    auth: AuthUser,
    Json(body): Json<BulkEpisodeRequest>,
) -> Result<impl IntoResponse, AppError> {
    let own_watch_marks = matches!(
        body.action,
        BulkEpisodeAction::MarkWatched | BulkEpisodeAction::MarkUnwatched
    );
    if !own_watch_marks && auth.require_contributor().is_err() {
        return Err(AppError::Forbidden);
    }
    if body.episode_ids.is_empty() {
        return Err(AppError::BadRequest("episode_ids must not be empty".into()));
    }
    if body.episode_ids.len() > BULK_EPISODE_LIMIT {
        return Err(AppError::BadRequest(format!(
            "Too many episodes in one request: {} (limit {BULK_EPISODE_LIMIT})",
            body.episode_ids.len()
        )));
    }

    let scopes = tv_episodes::list_scopes(&state.pool, &body.episode_ids)
        .await
        .map_err(|e| AppError::Internal(format!("Failed to resolve episodes: {e}")))?;

    if scopes.is_empty() {
        return Err(AppError::NotFound(
            "None of those episodes exist".to_string(),
        ));
    }

    let episode_ids: Vec<Uuid> = scopes.iter().map(|s| s.id).collect();
    let mut show_ids: Vec<Uuid> = scopes.iter().map(|s| s.show_id).collect();
    show_ids.sort_unstable();
    show_ids.dedup();

    let mut response = BulkEpisodeResponse {
        episodes_affected: 0,
        files_deleted: 0,
        auto_want_disabled: false,
    };

    match body.action {
        BulkEpisodeAction::DeleteFiles | BulkEpisodeAction::DeleteEpisodes => {
            // Snapshot first: deleting either the files or the episodes takes the
            // junction rows with it, and then nothing points at the paths.
            let files = tv_episode_files::list_paths_by_episodes(&state.pool, &episode_ids)
                .await
                .map_err(|e| AppError::Internal(format!("Failed to list episode files: {e}")))?;
            let file_ids: Vec<Uuid> = files.iter().map(|f| f.id).collect();

            if !file_ids.is_empty() {
                response.files_deleted = tv_episode_files::delete_many(&state.pool, &file_ids)
                    .await
                    .map_err(|e| {
                        AppError::Internal(format!("Failed to delete episode files: {e}"))
                    })? as usize;
            }

            if body.action == BulkEpisodeAction::DeleteEpisodes {
                response.episodes_affected = tv_episodes::delete_many(&state.pool, &episode_ids)
                    .await
                    .map_err(|e| AppError::Internal(format!("Failed to delete episodes: {e}")))?
                    as usize;
            } else {
                // The rows survive, so they have to be wantable again.
                tv_episodes::clear_imported_without_files(&state.pool, &episode_ids)
                    .await
                    .map_err(|e| AppError::Internal(format!("Failed to reset want status: {e}")))?;
                response.episodes_affected = episode_ids.len();
            }

            purge_file_artifacts(&state, &files).await;

            // Debrid history for these episodes now points at nothing.
            for id in &episode_ids {
                if let Err(e) = crate::media_cache::episode_cache::remove(
                    &state.config.storage_root,
                    &crate::media_cache::episode_cache::MediaKind::DebridEpisode {
                        episode_id: *id,
                    },
                ) {
                    tracing::warn!(error = %e, episode_id = %id, "failed to remove debrid_episode episode_cache");
                }
            }

            info!(
                episodes = episode_ids.len(),
                files = response.files_deleted,
                rows_deleted = matches!(body.action, BulkEpisodeAction::DeleteEpisodes),
                "Bulk episode delete"
            );
        }

        BulkEpisodeAction::Want => {
            // tv_want only looks at episodes that are already wanted or in the
            // library, so the flag has to be set before the job is queued.
            let to_want: Vec<Uuid> = scopes
                .iter()
                .filter(|s| !s.is_in_library)
                .map(|s| s.id)
                .collect();

            if !to_want.is_empty() {
                tv_episodes::set_want_status_batch(&state.pool, &to_want, "wanted")
                    .await
                    .map_err(|e| AppError::Internal(format!("Failed to set want status: {e}")))?;
            }
            response.episodes_affected = to_want.len();

            // One job per season, carrying that season's episodes — the shape
            // the tv_want handler already expects from the single-episode path.
            let mut by_season: std::collections::BTreeMap<(Uuid, Uuid), Vec<Uuid>> =
                std::collections::BTreeMap::new();
            for scope in scopes.iter().filter(|s| !s.is_in_library) {
                by_season
                    .entry((scope.show_id, scope.season_id))
                    .or_default()
                    .push(scope.id);
            }
            for ((show_id, season_id), ids) in by_season {
                let _ = state
                    .nats
                    .publisher()
                    .enqueue_job(
                        "kobbler",
                        "tv_want",
                        json!({
                            "show_id": show_id,
                            "season_id": season_id,
                            "episode_ids": ids,
                            "user_id": auth.user_id,
                            "manual": true,
                        }),
                    )
                    .await;
            }
        }

        BulkEpisodeAction::Unwant => {
            let cleared = tv_episodes::clear_wanted_in_episodes(&state.pool, &episode_ids)
                .await
                .map_err(|e| AppError::Internal(format!("Failed to clear want status: {e}")))?;
            response.episodes_affected = cleared.len();

            // As with the season-level unwant: a refilling auto-want mode would
            // put these straight back within the hour. Pin the affected seasons
            // to 'none' so the user's choice survives the next monitor run.
            let mut season_ids: Vec<Uuid> = scopes.iter().map(|s| s.season_id).collect();
            season_ids.sort_unstable();
            season_ids.dedup();

            for season_id in season_ids {
                let Some(season) = tv_seasons::get(&state.pool, season_id)
                    .await
                    .map_err(|e| AppError::Internal(format!("Failed to get season: {e}")))?
                else {
                    continue;
                };
                let Some(show) = tv_shows::get(&state.pool, season.show_id)
                    .await
                    .map_err(|e| AppError::Internal(format!("Failed to get show: {e}")))?
                else {
                    continue;
                };
                let effective_mode = crate::jobs::handlers::auto_want::resolve_effective_mode(
                    &season,
                    &show.auto_want_mode,
                );
                if crate::jobs::handlers::auto_want::mode_refills(effective_mode) {
                    tv_seasons::set_auto_want_mode(&state.pool, season_id, "none")
                        .await
                        .map_err(|e| {
                            AppError::Internal(format!("Failed to disable auto-want: {e}"))
                        })?;
                    response.auto_want_disabled = true;
                }
            }
        }

        BulkEpisodeAction::MarkWatched | BulkEpisodeAction::MarkUnwatched => {
            let watched = body.action == BulkEpisodeAction::MarkWatched;
            let affected = if watched {
                tv_watch_states::bulk_mark_episodes_watched(
                    &state.pool,
                    &episode_ids,
                    auth.user_id,
                    OffsetDateTime::now_utc(),
                )
                .await
            } else {
                tv_watch_states::bulk_mark_episodes_unwatched(
                    &state.pool,
                    &episode_ids,
                    auth.user_id,
                )
                .await
            }
            .map_err(|e| AppError::Internal(format!("Failed to bulk update watch states: {e}")))?;

            response.episodes_affected = affected.len();

            if !affected.is_empty() {
                let watched_at_str = watched.then(|| {
                    OffsetDateTime::now_utc()
                        .format(&time::format_description::well_known::Rfc3339)
                        .unwrap_or_default()
                });
                let affected: std::collections::HashSet<Uuid> = affected.into_iter().collect();
                // Trakt syncs per show, so a cross-show selection needs one job each.
                for show_id in &show_ids {
                    let ids: Vec<Uuid> = scopes
                        .iter()
                        .filter(|s| s.show_id == *show_id && affected.contains(&s.id))
                        .map(|s| s.id)
                        .collect();
                    if ids.is_empty() {
                        continue;
                    }
                    let _ = crate::jobs::enqueue_routine(
                        &state.pool,
                        state.nats.publisher(),
                        "trakt_bulk_sync_watch_state",
                        json!({
                            "user_id": auth.user_id,
                            "show_id": show_id,
                            "episode_ids": ids,
                            "watched": watched,
                            "watched_at": watched_at_str,
                        }),
                        JobContext::new("Syncing bulk watch state to Trakt"),
                    )
                    .await;
                }
            }
        }
    }

    // One event per show, not per episode — the frontend refetches the whole
    // show on each one.
    // The search indexing subscriber turns each event into the show's index jobs.
    for show_id in &show_ids {
        let _ = state
            .library_events
            .send(LibraryEvent::tv_show_updated(*show_id));
    }
    // Episode-level events drive per-episode search index cleanup.
    if matches!(body.action, BulkEpisodeAction::DeleteEpisodes) {
        for id in &episode_ids {
            let _ = state
                .library_events
                .send(LibraryEvent::tv_episode_updated(*id));
        }
    }

    Ok(Json(response))
}

// =============================================================================
// Season Poster
// =============================================================================

/// POST /api/v1/admin/tv/seasons/:id/poster
pub async fn upload_season_poster(
    State(state): State<AppState>,
    _auth: AuthUser,
    Path(id): Path<Uuid>,
    body: axum::body::Bytes,
) -> Result<impl IntoResponse, AppError> {
    tv_seasons::get(&state.pool, id)
        .await
        .map_err(|e| AppError::Internal(format!("Failed to get season: {e}")))?
        .ok_or_else(|| AppError::NotFound("Season not found".into()))?;

    crate::images::save_image_bytes(
        &state.config.storage_root,
        crate::images::ImageType::SeasonPoster,
        id,
        &body,
    )
    .await
    .map_err(|e| AppError::Internal(format!("Failed to save poster: {e}")))?;

    let _ = images::mark_cached(&state.pool, "tv_seasons", id, "poster").await;

    Ok(StatusCode::NO_CONTENT)
}

/// GET /api/v1/admin/tv/seasons/:id/poster
pub async fn get_season_poster(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
) -> Result<impl IntoResponse, AppError> {
    use crate::images::{ImageType, cache_image, cached_image_exists, load_cached_image};

    let image_type = ImageType::SeasonPoster;

    if cached_image_exists(&state.config.storage_root, image_type, id) {
        let bytes = load_cached_image(&state.config.storage_root, image_type, id)
            .await
            .map_err(|e| AppError::Internal(format!("Failed to load poster: {e}")))?;
        return Ok((
            [
                (
                    axum::http::header::CONTENT_TYPE,
                    crate::images::detect_content_type(&bytes),
                ),
                (axum::http::header::CACHE_CONTROL, "public, max-age=86400"),
            ],
            bytes,
        )
            .into_response());
    }

    let season = tv_seasons::get(&state.pool, id)
        .await
        .map_err(|e| AppError::Internal(format!("Failed to get season: {e}")))?
        .ok_or_else(|| AppError::NotFound("Season not found".into()))?;

    if let Some(url) = source_url(&season.images, "poster") {
        cache_image(&state.config.storage_root, image_type, id, &url)
            .await
            .map_err(|e| AppError::Internal(format!("Failed to cache poster: {e}")))?;
        let _ = images::mark_cached(&state.pool, "tv_seasons", id, "poster").await;
        let bytes = load_cached_image(&state.config.storage_root, image_type, id)
            .await
            .map_err(|e| AppError::Internal(format!("Failed to load poster: {e}")))?;
        return Ok((
            [
                (
                    axum::http::header::CONTENT_TYPE,
                    crate::images::detect_content_type(&bytes),
                ),
                (axum::http::header::CACHE_CONTROL, "public, max-age=86400"),
            ],
            bytes,
        )
            .into_response());
    }

    Err(AppError::NotFound("No poster available".into()))
}

// =============================================================================
// Image Endpoints
// =============================================================================

/// POST /api/v1/admin/tv/shows/:id/poster
pub async fn upload_show_poster(
    State(state): State<AppState>,
    _auth: AuthUser,
    Path(id): Path<Uuid>,
    body: axum::body::Bytes,
) -> Result<impl IntoResponse, AppError> {
    // Verify show exists
    tv_shows::get(&state.pool, id)
        .await
        .map_err(|e| AppError::Internal(format!("Failed to get show: {e}")))?
        .ok_or_else(|| AppError::NotFound("Show not found".into()))?;

    crate::images::save_image_bytes(
        &state.config.storage_root,
        crate::images::ImageType::TvShowPoster,
        id,
        &body,
    )
    .await
    .map_err(|e| AppError::Internal(format!("Failed to save poster: {e}")))?;

    let _ = images::mark_cached(&state.pool, "tv_shows", id, "poster").await;

    Ok(StatusCode::NO_CONTENT)
}

/// GET /api/v1/admin/tv/shows/:id/poster
pub async fn get_show_poster(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
) -> Result<impl IntoResponse, AppError> {
    let show = tv_shows::get(&state.pool, id)
        .await
        .map_err(|e| AppError::Internal(format!("Failed to get show: {e}")))?
        .ok_or_else(|| AppError::NotFound("Show not found".into()))?;

    images::serve_entity_image(
        &state.pool,
        &state.config.storage_root,
        "tv_shows",
        "poster",
        images::ImageType::TvShowPoster,
        id,
        &show.images,
        &headers,
    )
    .await
    .map_err(|e| match e {
        images::ImageError::NotFound(msg) => AppError::NotFound(msg),
        other => AppError::Internal(format!("Failed to load poster: {other}")),
    })
}

/// POST /api/v1/admin/tv/shows/:id/backdrop
pub async fn upload_show_backdrop(
    State(state): State<AppState>,
    _auth: AuthUser,
    Path(id): Path<Uuid>,
    body: axum::body::Bytes,
) -> Result<impl IntoResponse, AppError> {
    tv_shows::get(&state.pool, id)
        .await
        .map_err(|e| AppError::Internal(format!("Failed to get show: {e}")))?
        .ok_or_else(|| AppError::NotFound("Show not found".into()))?;

    images::save_image_bytes(
        &state.config.storage_root,
        images::ImageType::TvShowBackdrop,
        id,
        &body,
    )
    .await
    .map_err(|e| AppError::Internal(format!("Failed to save backdrop: {e}")))?;

    let _ = images::mark_cached(&state.pool, "tv_shows", id, "backdrop").await;

    Ok(StatusCode::NO_CONTENT)
}

/// GET /api/v1/admin/tv/shows/:id/backdrop
pub async fn get_show_backdrop(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
) -> Result<impl IntoResponse, AppError> {
    let show = tv_shows::get(&state.pool, id)
        .await
        .map_err(|e| AppError::Internal(format!("Failed to get show: {e}")))?
        .ok_or_else(|| AppError::NotFound("Show not found".into()))?;

    images::serve_entity_image(
        &state.pool,
        &state.config.storage_root,
        "tv_shows",
        "backdrop",
        images::ImageType::TvShowBackdrop,
        id,
        &show.images,
        &headers,
    )
    .await
    .map_err(|e| match e {
        images::ImageError::NotFound(msg) => AppError::NotFound(msg),
        other => AppError::Internal(format!("Failed to load backdrop: {other}")),
    })
}

/// POST /api/v1/admin/tv/shows/:id/logo
pub async fn upload_show_logo(
    State(state): State<AppState>,
    _auth: AuthUser,
    Path(id): Path<Uuid>,
    body: axum::body::Bytes,
) -> Result<impl IntoResponse, AppError> {
    tv_shows::get(&state.pool, id)
        .await
        .map_err(|e| AppError::Internal(format!("Failed to get show: {e}")))?
        .ok_or_else(|| AppError::NotFound("Show not found".into()))?;

    images::save_image_bytes(
        &state.config.storage_root,
        images::ImageType::TvShowLogo,
        id,
        &body,
    )
    .await
    .map_err(|e| AppError::Internal(format!("Failed to save logo: {e}")))?;

    let _ = images::mark_cached(&state.pool, "tv_shows", id, "logo").await;

    Ok(StatusCode::NO_CONTENT)
}

/// GET /api/v1/admin/tv/shows/:id/logo
pub async fn get_show_logo(
    State(state): State<AppState>,
    _auth: AuthUser,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
) -> Result<impl IntoResponse, AppError> {
    let show = tv_shows::get(&state.pool, id)
        .await
        .map_err(|e| AppError::Internal(format!("Failed to get show: {e}")))?
        .ok_or_else(|| AppError::NotFound("Show not found".into()))?;

    images::serve_entity_image(
        &state.pool,
        &state.config.storage_root,
        "tv_shows",
        "logo",
        images::ImageType::TvShowLogo,
        id,
        &show.images,
        &headers,
    )
    .await
    .map_err(|e| match e {
        images::ImageError::NotFound(msg) => AppError::NotFound(msg),
        other => AppError::Internal(format!("Failed to load logo: {other}")),
    })
}

// =============================================================================
// Replace Show Cast
// =============================================================================

/// PUT /api/v1/admin/tv/shows/:id/cast
pub async fn replace_show_cast(
    State(state): State<AppState>,
    _auth: AuthUser,
    Path(id): Path<Uuid>,
    Json(body): Json<Vec<CastInputRequest>>,
) -> Result<impl IntoResponse, AppError> {
    let cast_inputs: Vec<tv_shows::CastInput> = body
        .into_iter()
        .map(|c| tv_shows::CastInput {
            person_id: c.person_id,
            character_name: c.character_name,
            cast_order: c.cast_order,
        })
        .collect();

    tv_shows::replace_cast(&state.pool, id, cast_inputs)
        .await
        .map_err(|e| AppError::Internal(format!("Failed to replace cast: {e}")))?;

    Ok(StatusCode::NO_CONTENT)
}

// =============================================================================
// Replace Show Crew
// =============================================================================

/// PUT /api/v1/admin/tv/shows/:id/crew
pub async fn replace_show_crew(
    State(state): State<AppState>,
    _auth: AuthUser,
    Path(id): Path<Uuid>,
    Json(body): Json<Vec<CrewInputRequest>>,
) -> Result<impl IntoResponse, AppError> {
    let crew_inputs: Vec<tv_shows::CrewInput> = body
        .into_iter()
        .map(|c| tv_shows::CrewInput {
            person_id: c.person_id,
            department: c.department,
            job: c.job,
        })
        .collect();

    tv_shows::replace_crew(&state.pool, id, crew_inputs)
        .await
        .map_err(|e| AppError::Internal(format!("Failed to replace crew: {e}")))?;

    Ok(StatusCode::NO_CONTENT)
}

// =============================================================================
// Episode Still Image
// =============================================================================

/// POST /api/v1/admin/tv/episodes/:id/still
pub async fn upload_episode_still(
    State(state): State<AppState>,
    _auth: AuthUser,
    Path(id): Path<Uuid>,
    body: axum::body::Bytes,
) -> Result<impl IntoResponse, AppError> {
    tv_episodes::get(&state.pool, id)
        .await
        .map_err(|e| AppError::Internal(format!("Failed to get episode: {e}")))?
        .ok_or_else(|| AppError::NotFound("Episode not found".into()))?;

    crate::images::save_image_bytes(
        &state.config.storage_root,
        crate::images::ImageType::EpisodeStill,
        id,
        &body,
    )
    .await
    .map_err(|e| AppError::Internal(format!("Failed to save still: {e}")))?;

    let _ = images::mark_cached(&state.pool, "tv_episodes", id, "still").await;

    Ok(StatusCode::NO_CONTENT)
}

/// GET /api/v1/admin/tv/episodes/:id/still
pub async fn get_episode_still(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
) -> Result<impl IntoResponse, AppError> {
    use crate::images::{ImageType, cache_image, cached_image_exists, load_cached_image};

    let image_type = ImageType::EpisodeStill;

    if cached_image_exists(&state.config.storage_root, image_type, id) {
        let bytes = load_cached_image(&state.config.storage_root, image_type, id)
            .await
            .map_err(|e| AppError::Internal(format!("Failed to load still: {e}")))?;
        return Ok((
            [
                (
                    axum::http::header::CONTENT_TYPE,
                    crate::images::detect_content_type(&bytes),
                ),
                (axum::http::header::CACHE_CONTROL, "public, max-age=86400"),
            ],
            bytes,
        )
            .into_response());
    }

    let episode = tv_episodes::get(&state.pool, id)
        .await
        .map_err(|e| AppError::Internal(format!("Failed to get episode: {e}")))?
        .ok_or_else(|| AppError::NotFound("Episode not found".into()))?;

    if let Some(url) = source_url(&episode.images, "still") {
        cache_image(&state.config.storage_root, image_type, id, &url)
            .await
            .map_err(|e| AppError::Internal(format!("Failed to cache still: {e}")))?;
        let _ = images::mark_cached(&state.pool, "tv_episodes", id, "still").await;
        let bytes = load_cached_image(&state.config.storage_root, image_type, id)
            .await
            .map_err(|e| AppError::Internal(format!("Failed to load still: {e}")))?;
        return Ok((
            [
                (
                    axum::http::header::CONTENT_TYPE,
                    crate::images::detect_content_type(&bytes),
                ),
                (axum::http::header::CACHE_CONTROL, "public, max-age=86400"),
            ],
            bytes,
        )
            .into_response());
    }

    Err(AppError::NotFound("No still available".into()))
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

/// GET /api/v1/admin/tv/shows/first-by-letter?letter=A
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

    match tv_shows::first_by_letter(&state.pool, &letter, Some("tv"), auth.is_kids).await {
        Ok(Some(result)) => Ok(Json(FirstByLetterResponse {
            id: result.id,
            title: result.title,
        })
        .into_response()),
        Ok(None) => Err(AppError::NotFound("no shows found for letter".into())),
        Err(e) => Err(AppError::Internal(format!(
            "Failed to find show by letter: {e}"
        ))),
    }
}

#[derive(Debug, Serialize)]
struct AvailableLettersResponse {
    letters: Vec<String>,
}

/// GET /api/v1/admin/tv/shows/available-letters
pub async fn available_letters(
    State(state): State<AppState>,
    auth: AuthUser,
) -> Result<impl IntoResponse, AppError> {
    use crate::search::TvShowSearchFilters;

    // The index knows nothing of Kids profiles; their bar comes from the rule in SQL.
    if auth.is_kids {
        let letters = crate::repositories::letters::available_letters(
            &state.pool,
            "tv_shows b",
            &["b.content_category = 'tv'", crate::kids::KIDS_ALLOWED_B],
        )
        .await
        .map_err(|e| AppError::Internal(format!("Failed to get available letters: {e}")))?;
        return Ok(Json(AvailableLettersResponse { letters }));
    }

    let filters = TvShowSearchFilters::default();
    let letters = match state.search.get_available_tv_show_letters(filters).await {
        Ok(letters) => letters,
        // The bar is furniture, not an answer to a question: losing the search index
        // should cost typo tolerance, not make the page's alphabet vanish.
        Err(e) => {
            tracing::debug!(error = %e, "Show letters from search unavailable, using SQL");
            crate::repositories::letters::available_letters(&state.pool, "tv_shows b", &[])
                .await
                .map_err(|e| AppError::Internal(format!("Failed to get available letters: {e}")))?
        }
    };
    Ok(Json(AvailableLettersResponse { letters }))
}

/// GET /api/v1/admin/tv/shows/facets
/// Contextual genre + show-status counts for drill-down filtering.
pub async fn facets(
    State(state): State<AppState>,
    _auth: AuthUser,
) -> Result<impl IntoResponse, AppError> {
    use crate::search::TvShowSearchFilters;

    let filters = TvShowSearchFilters::default();
    match state.search.get_tv_show_facets(filters).await {
        Ok(facets) => Ok(Json(facets)),
        Err(e) => Err(AppError::Internal(format!("Failed to get TV facets: {e}"))),
    }
}

// =============================================================================
// Show Rating
// =============================================================================

/// GET /api/v1/admin/tv/shows/:id/rating
pub async fn get_show_rating(
    State(state): State<AppState>,
    auth: AuthUser,
    Path(id): Path<Uuid>,
) -> Result<impl IntoResponse, AppError> {
    let user_id = auth.user_id;

    let rating_row = tv_show_ratings::get(&state.pool, id, user_id)
        .await
        .map_err(|e| AppError::Internal(format!("Failed to get show rating: {e}")))?;

    Ok(Json(ShowRatingResponse {
        rating: rating_row.map(|r| r.rating),
    }))
}

/// PUT /api/v1/admin/tv/shows/:id/rating
pub async fn update_show_rating(
    State(state): State<AppState>,
    auth: AuthUser,
    Path(id): Path<Uuid>,
    Json(body): Json<UpdateShowRatingRequest>,
) -> Result<impl IntoResponse, AppError> {
    let user_id = auth.user_id;

    // Verify show exists
    tv_shows::get(&state.pool, id)
        .await
        .map_err(|e| AppError::Internal(format!("Failed to get show: {e}")))?
        .ok_or_else(|| AppError::NotFound("Show not found".into()))?;

    let rating_id = tv_show_ratings::upsert(&state.pool, id, user_id, body.rating)
        .await
        .map_err(|e| AppError::Internal(format!("Failed to update show rating: {e}")))?;

    Ok(Json(json!({ "id": rating_id })))
}

/// DELETE /api/v1/admin/tv/shows/:id/rating
pub async fn delete_show_rating(
    State(state): State<AppState>,
    auth: AuthUser,
    Path(id): Path<Uuid>,
) -> Result<impl IntoResponse, AppError> {
    let user_id = auth.user_id;

    tv_show_ratings::delete(&state.pool, id, user_id)
        .await
        .map_err(|e| AppError::Internal(format!("Failed to delete show rating: {e}")))?;

    Ok(StatusCode::NO_CONTENT)
}

// =============================================================================
// Get Episode Tracks
// =============================================================================

#[derive(Debug, Serialize)]
struct EpisodeFileTracksResponse {
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
struct EpisodeTracksResponse {
    files: Vec<EpisodeFileTracksResponse>,
}

/// GET /api/v1/admin/tv/episodes/:id/tracks
/// Returns all audio tracks, subtitle tracks, and external subs across all files.
pub async fn get_episode_tracks(
    State(state): State<AppState>,
    _auth: AuthUser,
    Path(episode_id): Path<Uuid>,
) -> Result<impl IntoResponse, AppError> {
    // Verify episode exists
    tv_episodes::get(&state.pool, episode_id)
        .await
        .map_err(|e| AppError::Internal(format!("Failed to get episode: {e}")))?
        .ok_or_else(|| AppError::NotFound("Episode not found".into()))?;

    let files = tv_episode_files::list_by_episode(&state.pool, episode_id)
        .await
        .map_err(|e| AppError::Internal(format!("Failed to list episode files: {e}")))?;

    let mut file_tracks = Vec::with_capacity(files.len());
    for f in files {
        let audio = audio_tracks::list_by_episode_file(&state.pool, f.id)
            .await
            .map_err(|e| AppError::Internal(format!("Failed to list audio tracks: {e}")))?;
        let subs = subtitle_tracks::list_by_episode_file(&state.pool, f.id)
            .await
            .map_err(|e| AppError::Internal(format!("Failed to list subtitle tracks: {e}")))?;
        let ext_subs = external_subtitles::list_by_episode_file(&state.pool, f.id)
            .await
            .map_err(|e| AppError::Internal(format!("Failed to list external subtitles: {e}")))?;

        file_tracks.push(EpisodeFileTracksResponse {
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

    Ok(Json(EpisodeTracksResponse { files: file_tracks }))
}

// =============================================================================
// Helpers
// =============================================================================

/// Convert an EpisodeRow to EpisodeResponse.
fn episode_row_to_response(ep: tv_episodes::EpisodeRow) -> EpisodeResponse {
    let still_url = endpoint_url(
        &ep.images,
        "still",
        "/api/v1/admin/tv/episodes/",
        ep.id,
        "/still",
    );
    EpisodeResponse {
        id: ep.id,
        season_id: ep.season_id,
        episode_identifier: ep.episode_identifier,
        episode_sort_order: ep.episode_sort_order,
        title: ep.title,
        description: ep.description,
        still_url,
        air_date: ep.air_date,
        air_time: ep.air_time,
        runtime: ep.runtime,
        is_in_library: ep.is_in_library,
        want_status: ep.want_status,
        watched: None,
        watch_position_seconds: None,
        created_at: ep.created_at,
        updated_at: ep.updated_at,
    }
}

/// Annotate episode responses with watch state data from a bulk query.
fn annotate_episodes_with_watch_states(
    seasons: &mut [SeasonResponse],
    watch_states: &[tv_watch_states::TvWatchStateRow],
) {
    let ws_map: std::collections::HashMap<Uuid, &tv_watch_states::TvWatchStateRow> =
        watch_states.iter().map(|ws| (ws.episode_id, ws)).collect();
    for season in seasons.iter_mut() {
        for ep in season.episodes.iter_mut() {
            if let Some(ws) = ws_map.get(&ep.id) {
                ep.watched = Some(ws.watched);
                ep.watch_position_seconds = ws.position_seconds;
            }
        }
    }
}

// =============================================================================
// Search Episode Releases
// =============================================================================

#[derive(Debug, Deserialize)]
pub struct EpisodeReleasesQuery {
    pub q: Option<String>,
    /// Category scope for the search. `None`/"default" keeps the TV category
    /// filter; "any" drops it so releases filed elsewhere are visible.
    pub scope: Option<String>,
}

/// GET /api/v1/admin/tv/episodes/{id}/releases
pub async fn search_episode_releases(
    State(state): State<AppState>,
    _auth: AuthUser,
    Path(id): Path<Uuid>,
    Query(query): Query<EpisodeReleasesQuery>,
) -> impl IntoResponse {
    let episode = match tv_episodes::get(&state.pool, id).await {
        Ok(Some(e)) => e,
        Ok(None) => return (StatusCode::NOT_FOUND, "episode not found").into_response(),
        Err(e) => {
            tracing::error!(error = %e, "Failed to fetch episode");
            return (StatusCode::INTERNAL_SERVER_ERROR, "database error").into_response();
        }
    };

    let season = match tv_seasons::get(&state.pool, episode.season_id).await {
        Ok(Some(s)) => s,
        Ok(None) => return (StatusCode::NOT_FOUND, "season not found").into_response(),
        Err(e) => {
            tracing::error!(error = %e, "Failed to fetch season");
            return (StatusCode::INTERNAL_SERVER_ERROR, "database error").into_response();
        }
    };

    let show = match tv_shows::get(&state.pool, season.show_id).await {
        Ok(Some(s)) => s,
        Ok(None) => return (StatusCode::NOT_FOUND, "show not found").into_response(),
        Err(e) => {
            tracing::error!(error = %e, "Failed to fetch show");
            return (StatusCode::INTERNAL_SERVER_ERROR, "database error").into_response();
        }
    };

    let override_query = query
        .q
        .as_deref()
        .map(str::trim)
        .filter(|v| !v.is_empty())
        .map(str::to_string);

    let media_want = if show.content_category == "sport" {
        // Sport events are stored as episodes; as a TV-episode want the search
        // degrades to the bare league name and matches unrelated releases.
        // Send the structured SportEvent want, as the want pipeline does.
        event_want(
            &VideoProviderRegistry::new(state.pool.clone()),
            EventWant {
                show: &show,
                season: &season,
                episode: &episode,
                in_library: None,
                library_video_height: None,
            },
        )
        .await
        .want
    } else {
        let identifiers = nats_jobs::TvIdentifiers {
            tmdb_id: show
                .identifiers
                .get("tmdb_id")
                .and_then(|v| v.as_str())
                .map(String::from),
            tvdb_id: show
                .identifiers
                .get("tvdb_id")
                .and_then(|v| v.as_str())
                .map(String::from),
            imdb_id: show
                .identifiers
                .get("imdb_id")
                .and_then(|v| v.as_str())
                .map(String::from),
        };

        nats_jobs::MediaWant::TvEpisode {
            show_name: show.title.clone(),
            season: season.season_identifier.clone(),
            episode: episode.episode_identifier.clone(),
            year: show.year.map(|y| y as u32),
            air_date: episode.air_date.map(|d| d.to_string()),
            identifiers: Some(identifiers),
            alternate_titles: crate::wanted::alternate_titles::alternate_titles(
                &show.title,
                show.alternate_titles.iter().flatten(),
            ),
            episode_title: episode.title.clone(),
            quality_profile_id: show.quality_profile_id,
            quality_overrides: show.preferred_languages.as_ref().map(|langs| {
                nats_jobs::QualityOverrides {
                    preferred_languages: Some(langs.clone()),
                    ..Default::default()
                }
            }),
        }
    };

    // Free text overrides the structured terms, but must not drop the media
    // type with them: Kobload derives its category filter from one or the
    // other, and with neither the search goes out uncategorised. "any" drops
    // it deliberately, for releases filed under Other/Misc.
    let any_scope = query.scope.as_deref() == Some("any");

    let (primary_query, media_want, media_type) = if let Some(override_query) = override_query {
        let media_type = (!any_scope).then(|| "tv".to_string());
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
            let rejected = tv_episodes::get_rejected_releases(&state.pool, id)
                .await
                .unwrap_or(json!([]));
            crate::routes::library::search_download::annotate_rejected_releases(
                &mut results,
                &rejected,
            );
            Json(json!({ "results": results })).into_response()
        }
        Err(err) => {
            tracing::error!(error = ?err, "Failed to search Kobload for episode releases");
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
// Queue Episode Release
// =============================================================================

#[derive(Debug, Deserialize)]
pub struct QueueEpisodeReleaseRequest {
    pub source: String,
    pub release: serde_json::Value,
    /// A regrab: keep whatever file is already on the episode instead of
    /// replacing it. The chat bot's `regrab` tool sets this.
    #[serde(default)]
    pub additional_copy: bool,
}

/// POST /api/v1/admin/tv/episodes/{id}/queue
pub async fn queue_episode_release(
    State(state): State<AppState>,
    auth: AuthUser,
    Path(id): Path<Uuid>,
    Json(payload): Json<QueueEpisodeReleaseRequest>,
) -> impl IntoResponse {
    let episode = match tv_episodes::get(&state.pool, id).await {
        Ok(Some(e)) => e,
        Ok(None) => return (StatusCode::NOT_FOUND, "episode not found").into_response(),
        Err(e) => {
            tracing::error!(error = %e, "Failed to fetch episode");
            return (StatusCode::INTERNAL_SERVER_ERROR, "database error").into_response();
        }
    };

    let season = match tv_seasons::get(&state.pool, episode.season_id).await {
        Ok(Some(s)) => s,
        Ok(None) => return (StatusCode::NOT_FOUND, "season not found").into_response(),
        Err(e) => {
            tracing::error!(error = %e, "Failed to fetch season");
            return (StatusCode::INTERNAL_SERVER_ERROR, "database error").into_response();
        }
    };

    let show = match tv_shows::get(&state.pool, season.show_id).await {
        Ok(Some(s)) => s,
        Ok(None) => return (StatusCode::NOT_FOUND, "show not found").into_response(),
        Err(e) => {
            tracing::error!(error = %e, "Failed to fetch show");
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
            "episode_id": id,
            "show_id": show.id,
            "season_id": season.id,
            "source_type": "tv_episode",
            "title": format!("{} - S{}E{}", show.title, season.season_identifier, episode.episode_identifier),
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
            episode_id: Some(id),
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

    // Update episode want_status to downloading
    if status != "failed"
        && let Err(e) = tv_episodes::set_want_status(&mut *tx, id, "downloading").await
    {
        tracing::error!(error = %e, "Failed to set episode want_status to downloading");
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
// Helpers
// =============================================================================

/// Convert a TvShowDetail to ShowDetailResponse.
fn show_detail_to_response(detail: tv_shows::TvShowDetail) -> ShowDetailResponse {
    let show_id = detail.id;
    let base = "/api/v1/admin/tv/shows/";
    let poster_url = endpoint_url(&detail.images, "poster", base, detail.id, "/poster");
    let backdrop_url = endpoint_url(&detail.images, "backdrop", base, detail.id, "/backdrop");
    let logo_url = endpoint_url(&detail.images, "logo", base, detail.id, "/logo");
    ShowDetailResponse {
        id: detail.id,
        title: detail.title,
        sort_title: detail.sort_title,
        original_title: detail.original_title,
        year: detail.year,
        status: detail.status,
        description: detail.description,
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
        network: detail.network,
        is_monitored: detail.is_monitored,
        auto_want_mode: detail.auto_want_mode,
        quality_profile_id: detail.quality_profile_id,
        preferred_languages: detail.preferred_languages,
        media_root_id: detail.media_root_id,
        content_category: detail.content_category,
        sport_metadata: detail.sport_metadata,
        first_aired: detail.first_aired,
        last_aired: detail.last_aired,
        next_airing: detail.next_airing,
        runtime: detail.runtime,
        total_episode_count: detail.total_episode_count,
        streamed_at: detail.streamed_at,
        created_at: detail.created_at,
        updated_at: detail.updated_at,
        seasons: detail
            .seasons
            .into_iter()
            .map(|s| {
                let season_poster = endpoint_url(
                    &s.images,
                    "poster",
                    "/api/v1/admin/tv/seasons/",
                    s.id,
                    "/poster",
                );
                SeasonResponse {
                    id: s.id,
                    show_id,
                    season_identifier: s.season_identifier,
                    season_sort_order: s.season_sort_order,
                    title: s.title,
                    description: s.description,
                    poster_url: season_poster,
                    air_date: s.air_date,
                    episode_count: s.episode_count,
                    is_monitored: s.is_monitored,
                    is_special: s.is_special,
                    auto_want_mode: s.auto_want_mode,
                    created_at: s.created_at,
                    episodes: s
                        .episodes
                        .into_iter()
                        .map(episode_row_to_response)
                        .collect(),
                }
            })
            .collect(),
        continue_episode: None,
        in_watchlist: false,
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

struct ContinueCandidate {
    season_idx: usize,
    episode_idx: usize,
    episode_id: Uuid,
    available: bool,
    aired: (Option<time::Date>, Option<OffsetDateTime>),
}

impl ContinueCandidate {
    fn response(&self, position_seconds: i32) -> ContinueEpisodeResponse {
        ContinueEpisodeResponse {
            season_idx: self.season_idx,
            episode_idx: self.episode_idx,
            episode_id: self.episode_id,
            position_seconds,
        }
    }
}

/// Compute the continue/next-up episode for a show.
///
/// Priority:
/// 1. In-progress episode (position_seconds > 0, not watched) — most recently updated.
/// 2. Next unwatched available episode after the most recently watched one.
/// 3. First available episode if no watch history.
/// 4. None if nothing qualifies.
///
/// A sport league is followed live, so steps 2 and 3 take the newest available event
/// instead, and only when it is newer than every watched one.
///
/// Available is a local file, or an aired episode nobody has wanted, which the client
/// offers as "Stream" rather than "Resume".
fn compute_continue_episode(
    seasons: &[SeasonResponse],
    watch_states: &[tv_watch_states::TvWatchStateRow],
    is_sport: bool,
) -> Option<ContinueEpisodeResponse> {
    let today = time::OffsetDateTime::now_utc().date();

    // A local file is always playable. Without one, an episode still counts if it has
    // aired and nobody has wanted it — a show being streamed rather than collected.
    // While an episode is wanted the file is on its way, so offering it here sends the
    // viewer off to debrid for something they already asked the library to fetch; air
    // dates run early often enough that "aired" alone meant Resume pointed at an
    // episode nobody could play. Same rule the Home row's `watch_next_episodes` applies.
    // Skip specials — they shouldn't be candidates for continue/next-up.
    let episodes: Vec<ContinueCandidate> = seasons
        .iter()
        .enumerate()
        .filter(|(_, season)| !season.is_special)
        .flat_map(|(si, season)| {
            season.episodes.iter().enumerate().map(move |(ei, ep)| {
                let streamable =
                    ep.want_status == "none" && ep.air_date.is_some_and(|d| d <= today);
                ContinueCandidate {
                    season_idx: si,
                    episode_idx: ei,
                    episode_id: ep.id,
                    available: ep.is_in_library || streamable,
                    aired: (ep.air_date, ep.air_time),
                }
            })
        })
        .collect();

    let in_progress = watch_states
        .iter()
        .filter(|ws| !ws.watched && ws.position_seconds.unwrap_or(0) > 0)
        .max_by_key(|ws| ws.updated_at);

    if let Some(ws) = in_progress
        && let Some(c) = episodes.iter().find(|c| c.episode_id == ws.episode_id)
    {
        return Some(c.response(ws.position_seconds.unwrap_or(0)));
    }

    let watched_ids: std::collections::HashSet<Uuid> = watch_states
        .iter()
        .filter(|ws| ws.watched)
        .map(|ws| ws.episode_id)
        .collect();

    if is_sport {
        return newest_unwatched_event(&episodes, &watched_ids).map(|c| c.response(0));
    }

    let last_watched = watch_states
        .iter()
        .filter(|ws| ws.watched)
        .max_by_key(|ws| ws.watched_at);

    if let Some(ws) = last_watched {
        let pos = episodes
            .iter()
            .position(|c| c.episode_id == ws.episode_id)?;
        return episodes[pos + 1..]
            .iter()
            .find(|c| c.available && !watched_ids.contains(&c.episode_id))
            .map(|c| c.response(0));
    }

    episodes.iter().find(|c| c.available).map(|c| c.response(0))
}

/// Events in air order, season order breaking ties: the newest available one, unless a
/// watched event is at least as new.
fn newest_unwatched_event<'a>(
    episodes: &'a [ContinueCandidate],
    watched_ids: &std::collections::HashSet<Uuid>,
) -> Option<&'a ContinueCandidate> {
    let order = |c: &&ContinueCandidate| (c.aired, c.season_idx, c.episode_idx);
    let newest = episodes.iter().filter(|c| c.available).max_by_key(order)?;
    let newest_watched = episodes
        .iter()
        .filter(|c| watched_ids.contains(&c.episode_id))
        .max_by_key(order);
    match newest_watched {
        Some(w) if order(&w) >= order(&newest) => None,
        _ => Some(newest),
    }
}
// =============================================================================
// Skip Segment Manual Override
// =============================================================================

#[derive(Debug, Deserialize)]
pub struct SkipSegmentOverride {
    intro_start: Option<f64>,
    intro_end: Option<f64>,
    credits_start: Option<f64>,
    credits_end: Option<f64>,
}

// =============================================================================
// V2 Media Analyze + Media Cache Runs
// =============================================================================

/// Query params for the "Re-analyze Media" endpoints. `force=true` makes
/// `media_analyze` rebuild all sidecars from scratch even if the manifest
/// says they're up to date (click-through is the UI's "I don't trust this
/// cache, re-decode the file" escape hatch).
#[derive(Debug, Deserialize, Default)]
pub struct MediaAnalyzeQuery {
    #[serde(default)]
    pub force: bool,
}

/// Enqueue `media_analyze` + chained `skip_detect` for each file_id.
/// Returns the count enqueued and the run_ids generated.
async fn enqueue_v2_for_files(
    state: &AppState,
    entity_type: &str,
    entity_id: Uuid,
    media_type: &str,
    file_ids: &[Uuid],
    force: bool,
) -> (u32, Vec<Uuid>) {
    let mut enqueued = 0u32;
    let mut run_ids = Vec::with_capacity(file_ids.len());
    let display = crate::jobs::fetch_job_display(&state.pool, entity_type, entity_id).await;

    for file_id in file_ids {
        let run_id = Uuid::now_v7();

        let analyze_payload = crate::jobs::handlers::MediaAnalyzePayload {
            file_id: *file_id,
            media_type: media_type.to_string(),
            force_regen: force,
            run_id: Some(run_id),
            triggered_by: "manual".to_string(),
            gaps_only: false,
        };
        let analyze_ctx = JobContext::for_entity(
            entity_type,
            entity_id,
            "Media analyze + skip-detect".to_string(),
        )
        .with_display_opt(display.clone());
        // We only enqueue media_analyze here. It runs the vision service
        // inline (no more generate_embeddings hop) and chains skip_detect
        // once the cache is durable — enqueueing skip_detect in parallel
        // would race the sidecar writes and bail on first run.
        if let Err(e) = crate::jobs::enqueue(
            &state.pool,
            state.nats.publisher(),
            "media_analyze",
            serde_json::to_value(&analyze_payload).unwrap(),
            analyze_ctx,
        )
        .await
        {
            warn!(error = %e, file_id = %file_id, "Failed to enqueue media_analyze");
            continue;
        }

        run_ids.push(run_id);
        enqueued += 1;
    }

    (enqueued, run_ids)
}

/// POST /api/v1/admin/tv/shows/{id}/media-analyze?force=bool
pub async fn run_media_analyze_show(
    State(state): State<AppState>,
    _auth: AuthUser,
    Path(show_id): Path<Uuid>,
    Query(query): Query<MediaAnalyzeQuery>,
) -> impl IntoResponse {
    let seasons = match tv_seasons::list_by_show(&state.pool, show_id).await {
        Ok(s) => s,
        Err(e) => {
            warn!(error = %e, "Failed to list seasons for media-analyze show");
            return StatusCode::INTERNAL_SERVER_ERROR.into_response();
        }
    };

    let mut all_file_ids = Vec::new();
    for season in &seasons {
        match tv_episode_files::list_file_ids_by_season(&state.pool, season.id).await {
            Ok(ids) => all_file_ids.extend(ids),
            Err(e) => {
                warn!(error = %e, season_id = %season.id, "Failed to list episode files");
            }
        }
    }

    let (enqueued, run_ids) = enqueue_v2_for_files(
        &state,
        "tv_show",
        show_id,
        "episode",
        &all_file_ids,
        query.force,
    )
    .await;
    (
        StatusCode::OK,
        Json(json!({ "enqueued": enqueued, "run_ids": run_ids })),
    )
        .into_response()
}

/// POST /api/v1/admin/tv/seasons/{id}/media-analyze?force=bool
pub async fn run_media_analyze_season(
    State(state): State<AppState>,
    _auth: AuthUser,
    Path(season_id): Path<Uuid>,
    Query(query): Query<MediaAnalyzeQuery>,
) -> impl IntoResponse {
    let file_ids = match tv_episode_files::list_file_ids_by_season(&state.pool, season_id).await {
        Ok(ids) => ids,
        Err(e) => {
            warn!(error = %e, season_id = %season_id, "Failed to list episode files");
            return StatusCode::INTERNAL_SERVER_ERROR.into_response();
        }
    };

    let (enqueued, run_ids) = enqueue_v2_for_files(
        &state,
        "tv_season",
        season_id,
        "episode",
        &file_ids,
        query.force,
    )
    .await;
    (
        StatusCode::OK,
        Json(json!({ "enqueued": enqueued, "run_ids": run_ids })),
    )
        .into_response()
}

/// PUT /api/v1/admin/tv/episodes/files/{file_id}/skip-segments
pub async fn set_skip_segments(
    State(state): State<AppState>,
    _auth: AuthUser,
    Path(file_id): Path<Uuid>,
    Json(body): Json<SkipSegmentOverride>,
) -> impl IntoResponse {
    if let (Some(start), Some(end)) = (body.intro_start, body.intro_end) {
        let result = if start == 0.0 && end == 0.0 {
            episode_skip_segments::suppress(&state.pool, file_id, "intro").await
        } else {
            episode_skip_segments::force_upsert(
                &state.pool,
                file_id,
                "intro",
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
            warn!(error = %e, "Failed to set intro skip segment");
            return StatusCode::INTERNAL_SERVER_ERROR.into_response();
        }
    }

    if let (Some(start), Some(end)) = (body.credits_start, body.credits_end) {
        let result = if start == 0.0 && end == 0.0 {
            episode_skip_segments::suppress(&state.pool, file_id, "credits").await
        } else {
            episode_skip_segments::force_upsert(
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
            warn!(error = %e, "Failed to set credits skip segment");
            return StatusCode::INTERNAL_SERVER_ERROR.into_response();
        }
    }

    StatusCode::NO_CONTENT.into_response()
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

/// GET /api/v1/admin/tv/shows/genres
pub async fn genres(
    State(state): State<AppState>,
    auth: AuthUser,
) -> Result<impl IntoResponse, AppError> {
    let genres = tv_shows::genre_counts(&state.pool, auth.is_kids)
        .await
        .map_err(|e| AppError::Internal(format!("Failed to list genres: {e}")))?
        .into_iter()
        .map(|(name, count, backdrop_id)| GenreCount {
            name,
            count,
            backdrop_url: backdrop_id.map(|id| format!("/api/v1/admin/tv/shows/{id}/backdrop")),
        })
        .collect();
    Ok(Json(GenresResponse { genres }))
}
