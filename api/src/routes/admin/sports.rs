//! Sports admin route handlers.
//!
//! Thin wrappers over TV show/season/episode repositories with
//! `content_category='sport'` enforced. Sports reuse the TV data model
//! but get their own API routes, UI pages, and search.

use axum::{
    Json,
    extract::{Path, Query, State},
    http::StatusCode,
    response::IntoResponse,
};
use serde::{Deserialize, Serialize};
use serde_json::json;
use time::OffsetDateTime;
use uuid::Uuid;

use crate::AppState;
use crate::LibraryEvent;
use crate::file_link_mode::{AcquireQuery, FileLinkMode};
use crate::images::{ImageEntry, endpoint_url, source_url};
use crate::jobs::{self, JobContext};
use crate::kobload::CreateDownloadRequest as KobloadDownloadRequest;
use crate::repositories::{
    PaginationDirection, media_roots, tv_episode_files, tv_episodes, tv_seasons, tv_shows,
    tv_watch_states, wanted_attempts,
};
use crate::routes::AuthUser;
use crate::routes::helpers::build_kobload_client;
use crate::routes::library::helpers::{
    NameCursor, decode_name_cursor, encode_name_cursor, paginated_window,
};
use crate::routes::library::types::PaginatedResponse;
use crate::sport_want::{EventWant, event_want};
use crate::video_metadata::{TvQuery, VideoProviderRegistry};
use tracing::{info, warn};

// =============================================================================
// Error Type (reuse pattern from tv.rs)
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
// Request/Response Types — Leagues (Shows with content_category='sport')
// =============================================================================

#[derive(Debug, Deserialize)]
pub struct ListLeaguesQuery {
    pub monitored: Option<bool>,
    pub search: Option<String>,
    pub limit: Option<i64>,
    pub cursor: Option<String>,
    pub direction: Option<String>,
    pub around: Option<String>,
    pub around_before: Option<i64>,
    pub sort: Option<String>,
}

#[derive(Debug, Serialize)]
struct LeagueListResponse {
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
    #[serde(with = "time::serde::rfc3339")]
    created_at: OffsetDateTime,
    #[serde(with = "time::serde::rfc3339")]
    updated_at: OffsetDateTime,
}

#[derive(Debug, Serialize)]
struct LeagueDetailResponse {
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
    network: Option<String>,
    is_monitored: bool,
    auto_want_mode: String,
    quality_profile_id: Option<Uuid>,
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
    event_count: i64,
    in_library_event_count: i64,
    wanted_event_count: i64,
    /// Watched by the requesting user.
    watched_event_count: i64,
    #[serde(with = "time::serde::rfc3339")]
    created_at: OffsetDateTime,
    #[serde(with = "time::serde::rfc3339")]
    updated_at: OffsetDateTime,
    seasons: Vec<SeasonResponse>,
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
    episodes: Vec<EventResponse>,
}

#[derive(Debug, Serialize)]
struct EventResponse {
    id: Uuid,
    season_id: Uuid,
    episode_identifier: String,
    episode_sort_order: i32,
    title: Option<String>,
    description: Option<String>,
    still_url: Option<String>,
    #[serde(with = "crate::serde_date::option_date")]
    air_date: Option<time::Date>,
    runtime: Option<i32>,
    is_in_library: bool,
    want_status: String,
    /// The viewer's watch state; only the season detail fills it in.
    #[serde(skip_serializing_if = "Option::is_none")]
    watched: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    watch_position_seconds: Option<i32>,
    #[serde(with = "time::serde::rfc3339")]
    created_at: OffsetDateTime,
    #[serde(with = "time::serde::rfc3339")]
    updated_at: OffsetDateTime,
}

#[derive(Debug, Deserialize)]
pub struct CreateLeagueRequest {
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
    pub media_root_id: Option<Uuid>,
    #[serde(default, with = "crate::serde_date::option_date")]
    pub first_aired: Option<time::Date>,
    pub runtime: Option<i32>,
    pub sport_metadata: Option<serde_json::Value>,
}

#[derive(Debug, Deserialize)]
pub struct UpdateLeagueRequest {
    pub title: Option<String>,
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
    #[serde(default, with = "crate::serde_date::option_date")]
    pub first_aired: Option<time::Date>,
    #[serde(default, with = "crate::serde_date::option_date")]
    pub last_aired: Option<time::Date>,
    #[serde(default, with = "crate::serde_date::option_date")]
    pub next_airing: Option<time::Date>,
    pub runtime: Option<i32>,
    pub total_episode_count: Option<i32>,
    pub quality_profile_id: Option<Option<Uuid>>,
    pub media_root_id: Option<Option<Uuid>>,
    pub sport_metadata: Option<serde_json::Value>,
}

#[derive(Debug, Deserialize)]
pub struct MonitorRequest {
    pub is_monitored: bool,
    pub auto_want_mode: Option<String>,
}

// =============================================================================
// Request/Response Types — Events (Episodes)
// =============================================================================

#[derive(Debug, Serialize)]
struct EventFileResponse {
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

#[derive(Debug, Deserialize)]
pub struct UpdateSeasonRequest {
    pub title: Option<String>,
    pub description: Option<String>,
    #[serde(default, with = "crate::serde_date::option_date")]
    pub air_date: Option<time::Date>,
    pub episode_count: Option<i32>,
    pub is_special: Option<bool>,
}

#[derive(Debug, Deserialize)]
pub struct SeasonAutoWantRequest {
    pub auto_want_mode: String,
}

#[derive(Debug, Deserialize)]
pub struct CreateEventRequest {
    pub episode_identifier: String,
    pub episode_sort_order: i32,
    pub title: Option<String>,
    pub description: Option<String>,
    #[serde(default, with = "crate::serde_date::option_date")]
    pub air_date: Option<time::Date>,
    pub runtime: Option<i32>,
}

#[derive(Debug, Deserialize)]
pub struct UpdateEventRequest {
    pub title: Option<String>,
    pub description: Option<String>,
    pub still_url: Option<String>,
    #[serde(default, with = "crate::serde_date::option_date")]
    pub air_date: Option<time::Date>,
    pub runtime: Option<i32>,
}

// =============================================================================
// Constants
// =============================================================================

const CONTENT_CATEGORY: &str = "sport";
const VALID_SHOW_AUTO_WANT_MODES: &[&str] = &["none", "new_only", "all", "fill_gaps"];
const VALID_SEASON_AUTO_WANT_MODES: &[&str] = &["inherit", "none", "new_only", "all", "fill_gaps"];

/// Image URL base for sports leagues (reuses tv_shows table so same image endpoints).
const LEAGUE_IMAGE_BASE: &str = "/api/v1/admin/sports/leagues/";
const SEASON_IMAGE_BASE: &str = "/api/v1/admin/sports/seasons/";
const EVENT_IMAGE_BASE: &str = "/api/v1/admin/sports/events/";

// =============================================================================
// List Leagues
// =============================================================================

/// GET /api/v1/admin/sports/leagues
pub async fn list_leagues(
    State(state): State<AppState>,
    auth: AuthUser,
    Query(params): Query<ListLeaguesQuery>,
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
            .map_err(|e| AppError::Internal(format!("Failed to get league: {e}")))?
            .ok_or_else(|| AppError::NotFound("League not found".into()))?;

        let target_sort = target
            .sort_title
            .as_deref()
            .unwrap_or(&target.title)
            .to_string();

        // One row past `before_count` doubles as the "is anything before this window"
        // probe, so around_before=0 still costs a single LIMIT 1 index lookup.
        let before_filters = tv_shows::TvShowListFilters {
            monitored: params.monitored,
            search: params.search.clone(),
            limit: before_count + 1,
            cursor_title: Some(target_sort.clone()),
            cursor_id: Some(around_id),
            direction: PaginationDirection::Backward,
            content_category: Some(CONTENT_CATEGORY.to_string()),
            kids_only: auth.is_kids,
            ..Default::default()
        };
        let mut before_rows = tv_shows::list(&state.pool, &before_filters)
            .await
            .map_err(|e| AppError::Internal(format!("Failed to list leagues: {e}")))?;

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
            search: params.search.clone(),
            limit: after_limit + 1,
            cursor_title: Some(cursor_title),
            cursor_id: Some(cursor_id),
            direction: PaginationDirection::Forward,
            cursor_inclusive,
            content_category: Some(CONTENT_CATEGORY.to_string()),
            kids_only: auth.is_kids,
            ..Default::default()
        };

        let after_rows = tv_shows::list(&state.pool, &after_filters)
            .await
            .map_err(|e| AppError::Internal(format!("Failed to list leagues: {e}")))?;

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

        let items = to_league_list_responses(combined);
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

    // The repository only applies cursors to the name-keyed index, so cursors are
    // only meaningful when sorting by title. Under any other sort (e.g. created_at)
    // the repository ignores the cursor and every page returns the same first rows,
    // which would turn "load more" into a dead end.
    let cursor_sortable = params.sort.as_deref().is_none_or(|s| s == "title");

    let filters = tv_shows::TvShowListFilters {
        monitored: params.monitored,
        search: params.search,
        limit: limit + 1,
        cursor_title,
        cursor_id,
        direction,
        content_category: Some(CONTENT_CATEGORY.to_string()),
        sort: params.sort,
        kids_only: auth.is_kids,
        ..Default::default()
    };

    let total_count = if is_first_page {
        Some(
            tv_shows::count(&state.pool, &filters)
                .await
                .map_err(|e| AppError::Internal(format!("Failed to count leagues: {e}")))?,
        )
    } else {
        None
    };

    let rows = tv_shows::list(&state.pool, &filters)
        .await
        .map_err(|e| AppError::Internal(format!("Failed to list leagues: {e}")))?;

    let (items, has_previous, has_next) = paginated_window(rows, limit, direction, is_first_page);

    let next_cursor = if has_next && cursor_sortable {
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
    let prev_cursor = if has_previous && cursor_sortable {
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

    let response_items = to_league_list_responses(items);

    Ok(Json(PaginatedResponse {
        items: response_items,
        next_cursor,
        prev_cursor,
        total_count,
    }))
}

fn to_league_list_responses(rows: Vec<tv_shows::TvShowRow>) -> Vec<LeagueListResponse> {
    rows.into_iter()
        .map(|r| {
            let poster_url = endpoint_url(&r.images, "poster", LEAGUE_IMAGE_BASE, r.id, "/poster");
            let backdrop_url =
                endpoint_url(&r.images, "backdrop", LEAGUE_IMAGE_BASE, r.id, "/backdrop");
            let logo_url = endpoint_url(&r.images, "logo", LEAGUE_IMAGE_BASE, r.id, "/logo");
            LeagueListResponse {
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
                created_at: r.created_at,
                updated_at: r.updated_at,
            }
        })
        .collect()
}

// =============================================================================
// Create League
// =============================================================================

/// POST /api/v1/admin/sports/leagues
pub async fn create_league(
    State(state): State<AppState>,
    _auth: AuthUser,
    Json(body): Json<CreateLeagueRequest>,
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

    // Auto-assign default media root for TV type if none provided
    let media_root_id = match body.media_root_id {
        Some(id) => Some(id),
        None => media_roots::get_default_for_type(&state.pool, "sport")
            .await
            .ok()
            .flatten()
            .map(|r| r.id),
    };

    let has_provider_id = body
        .identifiers
        .as_ref()
        .and_then(|ids| ids.get("thesportsdb"))
        .is_some();

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
        media_root_id,
        first_aired: body.first_aired,
        runtime: body.runtime,
        content_category: Some(CONTENT_CATEGORY.to_string()),
        sport_metadata: body.sport_metadata,
        ..Default::default()
    };

    let id = tv_shows::create(&state.pool, input)
        .await
        .map_err(|e| AppError::Internal(format!("Failed to create league: {e}")))?;

    // Enqueue async enrichment from TheSportsDB if identifier present
    if has_provider_id {
        let display = crate::jobs::fetch_job_display(&state.pool, "tv_show", id).await;
        let context = JobContext::for_entity(
            "tv_show",
            id,
            format!("Enriching sports league '{}'", input_title),
        )
        .with_display_opt(display);
        if let Err(e) = jobs::enqueue(
            &state.pool,
            state.nats.publisher(),
            "sports_league_enrich",
            json!({ "league_id": id }),
            context,
        )
        .await
        {
            warn!(league_id = %id, error = %e, "Failed to enqueue sports league enrichment");
        }
    }

    let _ = state.library_events.send(LibraryEvent::tv_show_updated(id));

    Ok((StatusCode::CREATED, Json(json!({ "id": id }))))
}

// =============================================================================
// Get League
// =============================================================================

/// GET /api/v1/admin/sports/leagues/:id
pub async fn get_league(
    State(state): State<AppState>,
    auth: AuthUser,
    Path(id): Path<Uuid>,
) -> Result<impl IntoResponse, AppError> {
    let detail = tv_shows::get_metadata(&state.pool, id)
        .await
        .map_err(|e| AppError::Internal(format!("Failed to get league: {e}")))?
        .filter(|d| !auth.is_kids || d.kids_allowed)
        .ok_or_else(|| AppError::NotFound("League not found".into()))?;
    let tallies = tv_shows::episode_tallies(&state.pool, id, auth.user_id)
        .await
        .map_err(|e| AppError::Internal(format!("Failed to count league events: {e}")))?;

    Ok(Json(league_detail_to_response(detail, tallies)))
}

// =============================================================================
// Update League
// =============================================================================

/// PATCH /api/v1/admin/sports/leagues/:id
pub async fn update_league(
    State(state): State<AppState>,
    _auth: AuthUser,
    Path(id): Path<Uuid>,
    Json(body): Json<UpdateLeagueRequest>,
) -> Result<impl IntoResponse, AppError> {
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
        alternate_titles: None,
        preferred_languages: None,
        media_root_id: body.media_root_id,
        sport_metadata: body.sport_metadata,
        streamed_at: None,
    };

    let updated = tv_shows::update(&state.pool, id, input)
        .await
        .map_err(|e| AppError::Internal(format!("Failed to update league: {e}")))?;

    if !updated {
        return Err(AppError::NotFound("League not found".into()));
    }

    let _ = state.library_events.send(LibraryEvent::tv_show_updated(id));

    Ok(StatusCode::NO_CONTENT)
}

// =============================================================================
// Delete League
// =============================================================================

/// DELETE /api/v1/admin/sports/leagues/:id
pub async fn delete_league(
    State(state): State<AppState>,
    _auth: AuthUser,
    Path(id): Path<Uuid>,
) -> Result<impl IntoResponse, AppError> {
    let deleted = tv_shows::delete(&state.pool, id)
        .await
        .map_err(|e| AppError::Internal(format!("Failed to delete league: {e}")))?;

    if !deleted {
        return Err(AppError::NotFound("League not found".into()));
    }

    let _ = state.library_events.send(LibraryEvent::tv_show_updated(id));

    Ok(StatusCode::NO_CONTENT)
}

// =============================================================================
// Set Monitor
// =============================================================================

/// POST /api/v1/admin/sports/leagues/:id/monitor
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
        return Err(AppError::NotFound("League not found".into()));
    }

    let _ = state.library_events.send(LibraryEvent::tv_show_updated(id));

    Ok(StatusCode::NO_CONTENT)
}

// =============================================================================
// Refresh Metadata (from TheSportsDB)
// =============================================================================

/// POST /api/v1/admin/sports/leagues/:id/refresh
pub async fn refresh_metadata(
    State(state): State<AppState>,
    _auth: AuthUser,
    Path(id): Path<Uuid>,
) -> Result<impl IntoResponse, AppError> {
    // Verify league exists and has TheSportsDB identifier
    let show = tv_shows::get_with_details(&state.pool, id)
        .await
        .map_err(|e| AppError::Internal(format!("Failed to get league: {e}")))?
        .ok_or_else(|| AppError::NotFound("League not found".into()))?;

    show.identifiers
        .get("thesportsdb")
        .and_then(|v| v.as_str())
        .ok_or_else(|| AppError::BadRequest("League has no TheSportsDB identifier".into()))?;

    let display = crate::jobs::fetch_job_display(&state.pool, "tv_show", id).await;
    let context = JobContext::for_entity(
        "tv_show",
        id,
        format!("Refreshing sports league '{}'", show.title),
    )
    .with_display_opt(display);
    jobs::enqueue(
        &state.pool,
        state.nats.publisher(),
        "sports_league_enrich",
        json!({ "league_id": id, "invalidate_cache": true }),
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

/// POST /api/v1/admin/sports/leagues/:id/want-all-missing
pub async fn want_all_missing(
    State(state): State<AppState>,
    auth: AuthUser,
    Path(id): Path<Uuid>,
) -> Result<impl IntoResponse, AppError> {
    tv_shows::get(&state.pool, id)
        .await
        .map_err(|e| AppError::Internal(format!("Failed to get league: {e}")))?
        .ok_or_else(|| AppError::NotFound("League not found".into()))?;

    let seasons = tv_seasons::list_by_show(&state.pool, id)
        .await
        .map_err(|e| AppError::Internal(format!("Failed to list seasons: {e}")))?;

    for season in &seasons {
        let missing = tv_episodes::list_missing(&state.pool, season.id)
            .await
            .map_err(|e| AppError::Internal(format!("Failed to list missing events: {e}")))?;

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
                "sports_want",
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
// Unwant All Missing
// =============================================================================

/// DELETE /api/v1/admin/sports/leagues/:id/want-all-missing
///
/// The inverse of `want_all_missing`: drop every outstanding want on the league.
/// Events already in the library keep their status.
pub async fn unwant_all_missing(
    State(state): State<AppState>,
    _auth: AuthUser,
    Path(id): Path<Uuid>,
) -> Result<impl IntoResponse, AppError> {
    let league = tv_shows::get(&state.pool, id)
        .await
        .map_err(|e| AppError::Internal(format!("Failed to get league: {e}")))?
        .ok_or_else(|| AppError::NotFound("League not found".into()))?;

    let cleared = tv_episodes::clear_wanted_in_show(&state.pool, id)
        .await
        .map_err(|e| AppError::Internal(format!("Failed to clear want status: {e}")))?;

    // sports_monitor re-applies auto-want on its own schedule, so a refilling
    // mode has to come off or the events are back within the hour.
    let auto_want_disabled = crate::jobs::handlers::auto_want::mode_refills(&league.auto_want_mode);
    if auto_want_disabled {
        tv_shows::set_auto_want_mode(&state.pool, id, "none")
            .await
            .map_err(|e| AppError::Internal(format!("Failed to disable auto-want: {e}")))?;
    }
    let seasons_disabled = tv_seasons::disable_refilling_auto_want_for_show(&state.pool, id)
        .await
        .map_err(|e| AppError::Internal(format!("Failed to disable season auto-want: {e}")))?;

    info!(
        league_id = %id,
        events = cleared.len(),
        auto_want_disabled,
        seasons_disabled,
        "Unwanted all events for league"
    );

    let _ = state.library_events.send(LibraryEvent::tv_show_updated(id));

    Ok(Json(json!({
        "unwanted_count": cleared.len(),
        "auto_want_disabled": auto_want_disabled || seasons_disabled > 0,
    })))
}

// =============================================================================
// League Images (poster, backdrop, logo)
// =============================================================================

/// GET /api/v1/admin/sports/leagues/:id/poster
pub async fn get_league_poster(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
) -> Result<impl IntoResponse, AppError> {
    use crate::images::{ImageType, cache_image, cached_image_exists, load_cached_image};

    let image_type = ImageType::TvShowPoster;

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

    let show = tv_shows::get(&state.pool, id)
        .await
        .map_err(|e| AppError::Internal(format!("Failed to get league: {e}")))?
        .ok_or_else(|| AppError::NotFound("League not found".into()))?;

    if let Some(url) = source_url(&show.images, "poster") {
        cache_image(&state.config.storage_root, image_type, id, &url)
            .await
            .map_err(|e| AppError::Internal(format!("Failed to cache poster: {e}")))?;
        let _ = crate::images::mark_cached(&state.pool, "tv_shows", id, "poster").await;
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

/// GET /api/v1/admin/sports/leagues/:id/backdrop
pub async fn get_league_backdrop(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
) -> Result<impl IntoResponse, AppError> {
    use crate::images::{ImageType, cache_image, cached_image_exists, load_cached_image};

    let image_type = ImageType::TvShowBackdrop;

    if cached_image_exists(&state.config.storage_root, image_type, id) {
        let bytes = load_cached_image(&state.config.storage_root, image_type, id)
            .await
            .map_err(|e| AppError::Internal(format!("Failed to load backdrop: {e}")))?;
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

    let show = tv_shows::get(&state.pool, id)
        .await
        .map_err(|e| AppError::Internal(format!("Failed to get league: {e}")))?
        .ok_or_else(|| AppError::NotFound("League not found".into()))?;

    if let Some(url) = source_url(&show.images, "backdrop") {
        cache_image(&state.config.storage_root, image_type, id, &url)
            .await
            .map_err(|e| AppError::Internal(format!("Failed to cache backdrop: {e}")))?;
        let _ = crate::images::mark_cached(&state.pool, "tv_shows", id, "backdrop").await;
        let bytes = load_cached_image(&state.config.storage_root, image_type, id)
            .await
            .map_err(|e| AppError::Internal(format!("Failed to load backdrop: {e}")))?;
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

    Err(AppError::NotFound("No backdrop available".into()))
}

/// GET /api/v1/admin/sports/leagues/:id/logo
pub async fn get_league_logo(
    State(state): State<AppState>,
    _auth: AuthUser,
    Path(id): Path<Uuid>,
) -> Result<impl IntoResponse, AppError> {
    use crate::images::{ImageType, cache_image, cached_image_exists, load_cached_image};

    let image_type = ImageType::TvShowLogo;

    if cached_image_exists(&state.config.storage_root, image_type, id) {
        let bytes = load_cached_image(&state.config.storage_root, image_type, id)
            .await
            .map_err(|e| AppError::Internal(format!("Failed to load logo: {e}")))?;
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

    let show = tv_shows::get(&state.pool, id)
        .await
        .map_err(|e| AppError::Internal(format!("Failed to get league: {e}")))?
        .ok_or_else(|| AppError::NotFound("League not found".into()))?;

    if let Some(url) = source_url(&show.images, "logo") {
        cache_image(&state.config.storage_root, image_type, id, &url)
            .await
            .map_err(|e| AppError::Internal(format!("Failed to cache logo: {e}")))?;
        let _ = crate::images::mark_cached(&state.pool, "tv_shows", id, "logo").await;
        let bytes = load_cached_image(&state.config.storage_root, image_type, id)
            .await
            .map_err(|e| AppError::Internal(format!("Failed to load logo: {e}")))?;
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

    Err(AppError::NotFound("No logo available".into()))
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

/// GET /api/v1/admin/sports/leagues/first-by-letter?letter=A
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

    match tv_shows::first_by_letter(&state.pool, &letter, Some(CONTENT_CATEGORY), auth.is_kids)
        .await
    {
        Ok(Some(result)) => Ok(Json(FirstByLetterResponse {
            id: result.id,
            title: result.title,
        })
        .into_response()),
        Ok(None) => Err(AppError::NotFound("no leagues found for letter".into())),
        Err(e) => Err(AppError::Internal(format!(
            "Failed to find league by letter: {e}"
        ))),
    }
}

#[derive(Debug, Serialize)]
struct AvailableLettersResponse {
    letters: Vec<String>,
}

/// GET /api/v1/admin/sports/leagues/available-letters
pub async fn available_letters(
    State(state): State<AppState>,
    auth: AuthUser,
) -> Result<impl IntoResponse, AppError> {
    let conditions: &[&'static str] = if auth.is_kids {
        &["b.content_category = 'sport'", crate::kids::KIDS_ALLOWED_B]
    } else {
        &["b.content_category = 'sport'"]
    };
    let letters =
        crate::repositories::letters::available_letters(&state.pool, "tv_shows b", conditions)
            .await
            .map_err(|e| AppError::Internal(format!("Failed to get available letters: {e}")))?;

    Ok(Json(AvailableLettersResponse { letters }))
}

// =============================================================================
// List League Seasons (Paginated)
// =============================================================================

#[derive(Debug, Deserialize)]
pub struct ListSeasonsQuery {
    pub limit: Option<i64>,
    pub offset: Option<i64>,
    pub sort: Option<String>,
}

#[derive(Debug, Serialize)]
struct PaginatedSeasonsResponse {
    items: Vec<SeasonListResponse>,
    total_count: i64,
}

#[derive(Debug, Serialize)]
struct SeasonListResponse {
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
    actual_episode_count: i64,
    /// Events still outstanding, so the UI knows whether an unwant would do anything.
    wanted_event_count: i64,
    in_library_event_count: i64,
    watched_event_count: i64,
    is_monitored: bool,
    is_special: bool,
    auto_want_mode: String,
    #[serde(with = "time::serde::rfc3339")]
    created_at: OffsetDateTime,
}

/// GET /api/v1/admin/sports/leagues/:id/seasons?limit=20&offset=0&sort=desc
pub async fn list_league_seasons(
    State(state): State<AppState>,
    auth: AuthUser,
    Path(id): Path<Uuid>,
    Query(params): Query<ListSeasonsQuery>,
) -> Result<impl IntoResponse, AppError> {
    let limit = params.limit.unwrap_or(20).min(100);
    let offset = params.offset.unwrap_or(0).max(0);
    let sort_desc = params.sort.as_deref() != Some("asc");

    let (seasons, total_count) =
        tv_seasons::list_by_show_paginated(&state.pool, id, auth.user_id, limit, offset, sort_desc)
            .await
            .map_err(|e| AppError::Internal(format!("Failed to list seasons: {e}")))?;

    let items: Vec<SeasonListResponse> = seasons
        .into_iter()
        .map(|ps| {
            let poster_url = endpoint_url(
                &ps.season.images,
                "poster",
                SEASON_IMAGE_BASE,
                ps.season.id,
                "/poster",
            );
            SeasonListResponse {
                id: ps.season.id,
                show_id: ps.season.show_id,
                season_identifier: ps.season.season_identifier,
                season_sort_order: ps.season.season_sort_order,
                title: ps.season.title,
                description: ps.season.description,
                poster_url,
                air_date: ps.season.air_date,
                episode_count: ps.season.episode_count,
                actual_episode_count: ps.actual_episode_count,
                wanted_event_count: ps.wanted_episode_count,
                in_library_event_count: ps.in_library_episode_count,
                watched_event_count: ps.watched_episode_count,
                is_monitored: ps.season.is_monitored,
                is_special: ps.season.is_special,
                auto_want_mode: ps.season.auto_want_mode,
                created_at: ps.season.created_at,
            }
        })
        .collect();

    Ok(Json(PaginatedSeasonsResponse { items, total_count }))
}

// =============================================================================
// Get Season
// =============================================================================

/// GET /api/v1/admin/sports/seasons/:id
pub async fn get_season(
    State(state): State<AppState>,
    auth: AuthUser,
    Path(id): Path<Uuid>,
) -> Result<impl IntoResponse, AppError> {
    let season = tv_seasons::get(&state.pool, id)
        .await
        .map_err(|e| AppError::Internal(format!("Failed to get season: {e}")))?
        .ok_or_else(|| AppError::NotFound("Season not found".into()))?;

    let episodes = tv_episodes::list_by_season(&state.pool, id)
        .await
        .map_err(|e| AppError::Internal(format!("Failed to list events: {e}")))?;
    let watch_states = tv_watch_states::list_for_season(&state.pool, id, auth.user_id)
        .await
        .map_err(|e| AppError::Internal(format!("Failed to get watch states: {e}")))?;
    let watch_by_event: std::collections::HashMap<Uuid, &tv_watch_states::TvWatchStateRow> =
        watch_states.iter().map(|ws| (ws.episode_id, ws)).collect();

    let response = SeasonResponse {
        id: season.id,
        show_id: season.show_id,
        season_identifier: season.season_identifier,
        season_sort_order: season.season_sort_order,
        title: season.title,
        description: season.description,
        poster_url: endpoint_url(
            &season.images,
            "poster",
            SEASON_IMAGE_BASE,
            season.id,
            "/poster",
        ),
        air_date: season.air_date,
        episode_count: season.episode_count,
        is_monitored: season.is_monitored,
        is_special: season.is_special,
        auto_want_mode: season.auto_want_mode,
        created_at: season.created_at,
        episodes: episodes
            .into_iter()
            .map(|ep| {
                let ws = watch_by_event.get(&ep.id);
                EventResponse {
                    watched: Some(ws.is_some_and(|ws| ws.watched)),
                    watch_position_seconds: ws.and_then(|ws| ws.position_seconds),
                    ..event_row_to_response(ep)
                }
            })
            .collect(),
    };

    Ok(Json(response))
}

// =============================================================================
// Create Season
// =============================================================================

/// POST /api/v1/admin/sports/leagues/:id/seasons
pub async fn create_season(
    State(state): State<AppState>,
    _auth: AuthUser,
    Path(show_id): Path<Uuid>,
    Json(body): Json<CreateSeasonRequest>,
) -> Result<impl IntoResponse, AppError> {
    if body.season_identifier.trim().is_empty() {
        return Err(AppError::BadRequest("season_identifier is required".into()));
    }

    tv_shows::get(&state.pool, show_id)
        .await
        .map_err(|e| AppError::Internal(format!("Failed to get league: {e}")))?
        .ok_or_else(|| AppError::NotFound("League not found".into()))?;

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

    Ok((StatusCode::CREATED, Json(json!({ "id": id }))))
}

// =============================================================================
// Update Season
// =============================================================================

/// PATCH /api/v1/admin/sports/seasons/:id
pub async fn update_season(
    State(state): State<AppState>,
    _auth: AuthUser,
    Path(id): Path<Uuid>,
    Json(body): Json<UpdateSeasonRequest>,
) -> Result<impl IntoResponse, AppError> {
    let season = tv_seasons::get(&state.pool, id)
        .await
        .map_err(|e| AppError::Internal(format!("Failed to get season: {e}")))?
        .ok_or_else(|| AppError::NotFound("Season not found".into()))?;

    let input = tv_seasons::SeasonUpdate {
        title: body.title,
        description: body.description,
        air_date: body.air_date,
        episode_count: body.episode_count,
        is_special: body.is_special,
        ..Default::default()
    };

    let updated = tv_seasons::update(&state.pool, id, input)
        .await
        .map_err(|e| AppError::Internal(format!("Failed to update season: {e}")))?;

    if !updated {
        return Err(AppError::NotFound("Season not found".into()));
    }

    let _ = state
        .library_events
        .send(LibraryEvent::tv_show_updated(season.show_id));

    Ok(StatusCode::NO_CONTENT)
}

// =============================================================================
// Want Season
// =============================================================================

/// POST /api/v1/admin/sports/seasons/:id/want
pub async fn want_season(
    State(state): State<AppState>,
    auth: AuthUser,
    Path(id): Path<Uuid>,
) -> Result<impl IntoResponse, AppError> {
    let season = tv_seasons::get(&state.pool, id)
        .await
        .map_err(|e| AppError::Internal(format!("Failed to get season: {e}")))?
        .ok_or_else(|| AppError::NotFound("Season not found".into()))?;

    let _ = state
        .nats
        .publisher()
        .enqueue_job(
            "kobbler",
            "sports_want",
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

    Ok(StatusCode::NO_CONTENT)
}

// =============================================================================
// Unwant Season
// =============================================================================

/// DELETE /api/v1/admin/sports/seasons/:id/want
///
/// The inverse of `want_season`: drop every outstanding want in the season.
/// Events already in the library keep their status.
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

    // Pinning the season to 'none' overrides the league default for this season
    // only, which is what unwanting one season should mean.
    let league = tv_shows::get(&state.pool, season.show_id)
        .await
        .map_err(|e| AppError::Internal(format!("Failed to get league: {e}")))?
        .ok_or_else(|| AppError::Internal("League not found for season".into()))?;
    let effective_mode =
        crate::jobs::handlers::auto_want::resolve_effective_mode(&season, &league.auto_want_mode);
    let auto_want_disabled = crate::jobs::handlers::auto_want::mode_refills(effective_mode);
    if auto_want_disabled {
        tv_seasons::set_auto_want_mode(&state.pool, id, "none")
            .await
            .map_err(|e| AppError::Internal(format!("Failed to disable auto-want: {e}")))?;
    }

    info!(
        season_id = %id,
        events = cleared.len(),
        auto_want_disabled,
        "Unwanted all events for season"
    );

    let _ = state
        .library_events
        .send(LibraryEvent::tv_show_updated(season.show_id));

    Ok(Json(json!({
        "unwanted_count": cleared.len(),
        "auto_want_disabled": auto_want_disabled,
    })))
}

// =============================================================================
// Set Season Auto-Want
// =============================================================================

/// POST /api/v1/admin/sports/seasons/:id/auto-want
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

    let _ = state
        .library_events
        .send(LibraryEvent::tv_show_updated(season.show_id));

    Ok(StatusCode::NO_CONTENT)
}

// =============================================================================
// Get Event
// =============================================================================

/// GET /api/v1/admin/sports/events/:id
pub async fn get_event(
    State(state): State<AppState>,
    _auth: AuthUser,
    Path(id): Path<Uuid>,
) -> Result<impl IntoResponse, AppError> {
    let episode = tv_episodes::get(&state.pool, id)
        .await
        .map_err(|e| AppError::Internal(format!("Failed to get event: {e}")))?
        .ok_or_else(|| AppError::NotFound("Event not found".into()))?;

    Ok(Json(event_row_to_response(episode)))
}

// =============================================================================
// Create Event
// =============================================================================

/// POST /api/v1/admin/sports/seasons/:id/events
pub async fn create_event(
    State(state): State<AppState>,
    _auth: AuthUser,
    Path(season_id): Path<Uuid>,
    Json(body): Json<CreateEventRequest>,
) -> Result<impl IntoResponse, AppError> {
    if body.episode_identifier.trim().is_empty() {
        return Err(AppError::BadRequest(
            "episode_identifier is required".into(),
        ));
    }

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
        .map_err(|e| AppError::Internal(format!("Failed to create event: {e}")))?;

    let _ = state
        .library_events
        .send(LibraryEvent::tv_show_updated(season.show_id));

    Ok((StatusCode::CREATED, Json(json!({ "id": id }))))
}

// =============================================================================
// Update Event
// =============================================================================

/// PATCH /api/v1/admin/sports/events/:id
pub async fn update_event(
    State(state): State<AppState>,
    _auth: AuthUser,
    Path(id): Path<Uuid>,
    Json(body): Json<UpdateEventRequest>,
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
        .map_err(|e| AppError::Internal(format!("Failed to get event: {e}")))?
        .ok_or_else(|| AppError::NotFound("Event not found".into()))?;

    let season = tv_seasons::get(&state.pool, episode.season_id)
        .await
        .map_err(|e| AppError::Internal(format!("Failed to get season: {e}")))?
        .ok_or_else(|| AppError::Internal("Season not found for event".into()))?;

    let updated = tv_episodes::update(&state.pool, id, input)
        .await
        .map_err(|e| AppError::Internal(format!("Failed to update event: {e}")))?;

    if !updated {
        return Err(AppError::NotFound("Event not found".into()));
    }

    let _ = state
        .library_events
        .send(LibraryEvent::tv_show_updated(season.show_id));

    Ok(StatusCode::NO_CONTENT)
}

// =============================================================================
// Delete Event
// =============================================================================

/// DELETE /api/v1/admin/sports/events/:id
pub async fn delete_event(
    State(state): State<AppState>,
    _auth: AuthUser,
    Path(id): Path<Uuid>,
) -> Result<impl IntoResponse, AppError> {
    let episode = tv_episodes::get(&state.pool, id)
        .await
        .map_err(|e| AppError::Internal(format!("Failed to get event: {e}")))?
        .ok_or_else(|| AppError::NotFound("Event not found".into()))?;

    let season = tv_seasons::get(&state.pool, episode.season_id)
        .await
        .map_err(|e| AppError::Internal(format!("Failed to get season: {e}")))?
        .ok_or_else(|| AppError::Internal("Season not found for event".into()))?;

    let deleted = tv_episodes::delete(&state.pool, id)
        .await
        .map_err(|e| AppError::Internal(format!("Failed to delete event: {e}")))?;

    if !deleted {
        return Err(AppError::NotFound("Event not found".into()));
    }

    let _ = state
        .library_events
        .send(LibraryEvent::tv_show_updated(season.show_id));

    Ok(StatusCode::NO_CONTENT)
}

// =============================================================================
// Want / Unwant Event
// =============================================================================

/// POST /api/v1/admin/sports/events/:id/want
pub async fn want_event(
    State(state): State<AppState>,
    auth: AuthUser,
    Path(id): Path<Uuid>,
    Query(copy): Query<AcquireQuery>,
) -> Result<impl IntoResponse, AppError> {
    let episode = tv_episodes::get(&state.pool, id)
        .await
        .map_err(|e| AppError::Internal(format!("Failed to get event: {e}")))?
        .ok_or_else(|| AppError::NotFound("Event not found".into()))?;

    let season = tv_seasons::get(&state.pool, episode.season_id)
        .await
        .map_err(|e| AppError::Internal(format!("Failed to get season: {e}")))?
        .ok_or_else(|| AppError::Internal("Season not found for event".into()))?;

    tv_episodes::set_want_status(&state.pool, id, "wanted")
        .await
        .map_err(|e| AppError::Internal(format!("Failed to set want status: {e}")))?;

    let _ = state
        .nats
        .publisher()
        .enqueue_job(
            "kobbler",
            "sports_want",
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

    Ok(StatusCode::NO_CONTENT)
}

/// DELETE /api/v1/admin/sports/events/:id/want
pub async fn unwant_event(
    State(state): State<AppState>,
    _auth: AuthUser,
    Path(id): Path<Uuid>,
) -> Result<impl IntoResponse, AppError> {
    let episode = tv_episodes::get(&state.pool, id)
        .await
        .map_err(|e| AppError::Internal(format!("Failed to get event: {e}")))?
        .ok_or_else(|| AppError::NotFound("Event not found".into()))?;

    let season = tv_seasons::get(&state.pool, episode.season_id)
        .await
        .map_err(|e| AppError::Internal(format!("Failed to get season: {e}")))?
        .ok_or_else(|| AppError::Internal("Season not found for event".into()))?;

    tv_episodes::set_want_status(&state.pool, id, "none")
        .await
        .map_err(|e| AppError::Internal(format!("Failed to set want status: {e}")))?;

    let _ = state
        .library_events
        .send(LibraryEvent::tv_show_updated(season.show_id));

    Ok(StatusCode::NO_CONTENT)
}

// =============================================================================
// List Event Files
// =============================================================================

/// GET /api/v1/admin/sports/events/:id/files
pub async fn list_event_files(
    State(state): State<AppState>,
    _auth: AuthUser,
    Path(id): Path<Uuid>,
) -> Result<impl IntoResponse, AppError> {
    let files = tv_episode_files::list_by_episode(&state.pool, id)
        .await
        .map_err(|e| AppError::Internal(format!("Failed to list event files: {e}")))?;

    let mut response = Vec::new();
    for f in files {
        let episode_ids = tv_episode_files::get_episode_ids(&state.pool, f.id)
            .await
            .unwrap_or_default();
        response.push(EventFileResponse {
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
// Move Event File
// =============================================================================

#[derive(Debug, Deserialize)]
pub struct EventFilePath {
    pub event_id: Uuid,
    pub file_id: Uuid,
}

#[derive(Debug, Deserialize)]
pub struct MoveEventFileBody {
    pub target_event_id: Uuid,
}

/// POST /api/v1/admin/sports/events/:event_id/files/:file_id/move
pub async fn move_event_file(
    State(state): State<AppState>,
    _auth: AuthUser,
    Path(path): Path<EventFilePath>,
    Json(body): Json<MoveEventFileBody>,
) -> Result<impl IntoResponse, AppError> {
    // Verify target event exists
    tv_episodes::get(&state.pool, body.target_event_id)
        .await
        .map_err(|e| AppError::Internal(format!("Failed to check target event: {e}")))?
        .ok_or_else(|| AppError::NotFound("Target event not found".into()))?;

    let moved = tv_episode_files::set_episodes(&state.pool, path.file_id, &[body.target_event_id])
        .await
        .map_err(|e| AppError::Internal(format!("Failed to move event file: {e}")))?;

    if !moved {
        return Err(AppError::NotFound("Event file not found".into()));
    }

    Ok(StatusCode::NO_CONTENT)
}

// =============================================================================
// Search Event Releases
// =============================================================================

#[derive(Debug, Deserialize)]
pub struct EventReleasesQuery {
    pub q: Option<String>,
    /// Category scope for the search. `None`/"default" keeps the category
    /// filter; "any" drops it so releases filed elsewhere are visible.
    pub scope: Option<String>,
}

/// GET /api/v1/admin/sports/events/:id/releases
pub async fn search_event_releases(
    State(state): State<AppState>,
    _auth: AuthUser,
    Path(id): Path<Uuid>,
    Query(query): Query<EventReleasesQuery>,
) -> impl IntoResponse {
    let episode = match tv_episodes::get(&state.pool, id).await {
        Ok(Some(e)) => e,
        Ok(None) => return (StatusCode::NOT_FOUND, "event not found").into_response(),
        Err(e) => {
            tracing::error!(error = %e, "Failed to fetch event");
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
        Ok(None) => return (StatusCode::NOT_FOUND, "league not found").into_response(),
        Err(e) => {
            tracing::error!(error = %e, "Failed to fetch league");
            return (StatusCode::INTERNAL_SERVER_ERROR, "database error").into_response();
        }
    };

    let override_query = query
        .q
        .as_deref()
        .map(str::trim)
        .filter(|v| !v.is_empty())
        .map(str::to_string);

    let media_want = event_want(
        &VideoProviderRegistry::new(state.pool.clone()),
        EventWant {
            show: &show,
            season: &season,
            episode: &episode,
            in_library: None,
            library_video_height: None,
        },
    )
    .await;

    // Free text overrides the structured terms, but must not drop the media
    // type with them: Kobload derives its category filter from one or the
    // other, and with neither the search goes out uncategorised. "any" drops
    // it deliberately, for releases filed under Other/Misc.
    //
    // Sport wants resolve to the "tv" media type in Kobload
    // (media_type_key_from_media_want), so scoping uses the same key the
    // structured path would have derived.
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
            tracing::error!(error = ?err, "Failed to search Kobload for event releases");
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
// Queue Event Release
// =============================================================================

#[derive(Debug, Deserialize)]
pub struct QueueEventReleaseRequest {
    pub source: String,
    pub release: serde_json::Value,
    /// A regrab: keep whatever file is already on the event instead of
    /// replacing it. The chat bot's `regrab` tool sets this.
    #[serde(default)]
    pub additional_copy: bool,
}

/// POST /api/v1/admin/sports/events/:id/queue
pub async fn queue_event_release(
    State(state): State<AppState>,
    auth: AuthUser,
    Path(id): Path<Uuid>,
    Json(payload): Json<QueueEventReleaseRequest>,
) -> impl IntoResponse {
    let episode = match tv_episodes::get(&state.pool, id).await {
        Ok(Some(e)) => e,
        Ok(None) => return (StatusCode::NOT_FOUND, "event not found").into_response(),
        Err(e) => {
            tracing::error!(error = %e, "Failed to fetch event");
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
        Ok(None) => return (StatusCode::NOT_FOUND, "league not found").into_response(),
        Err(e) => {
            tracing::error!(error = %e, "Failed to fetch league");
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
            "source_type": "sport_event",
            "title": format!("{} - {} {}", show.title, season.season_identifier, episode.title.as_deref().unwrap_or(&episode.episode_identifier)),
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
            status,
            message,
            user_id: Some(auth.user_id),
            ..Default::default()
        },
    )
    .await
    {
        tracing::error!(error = %e, "Failed to record wanted attempt");
    }

    if let Err(e) = tx.commit().await {
        tracing::error!(error = %e, "Failed to commit transaction");
        return (StatusCode::INTERNAL_SERVER_ERROR, "database error").into_response();
    }

    let _ = state
        .library_events
        .send(LibraryEvent::tv_show_updated(show.id));

    (StatusCode::OK, Json(json!({ "download_id": download_id }))).into_response()
}

// =============================================================================
// Search Leagues (via TheSportsDB)
// =============================================================================

#[derive(Debug, Deserialize)]
pub struct SearchQuery {
    pub q: String,
}

#[derive(Debug, Serialize)]
pub struct SearchResponse {
    pub results: Vec<crate::video_metadata::types::TvShowResult>,
}

/// GET /api/v1/admin/sports/search?q=MotoGP
pub async fn search_leagues(
    State(state): State<AppState>,
    _auth: AuthUser,
    Query(params): Query<SearchQuery>,
) -> impl IntoResponse {
    if params.q.is_empty() {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({ "error": "Query parameter 'q' is required" })),
        )
            .into_response();
    }

    let query = TvQuery {
        title: params.q,
        ..Default::default()
    };

    let registry = VideoProviderRegistry::new(state.pool.clone());
    let outcome = registry.search_tv(&query, Some("thesportsdb")).await;

    Json(SearchResponse {
        results: outcome.results,
    })
    .into_response()
}

// =============================================================================
// Helpers
// =============================================================================

fn event_row_to_response(ep: tv_episodes::EpisodeRow) -> EventResponse {
    let still_url = endpoint_url(&ep.images, "still", EVENT_IMAGE_BASE, ep.id, "/still");
    EventResponse {
        id: ep.id,
        season_id: ep.season_id,
        episode_identifier: ep.episode_identifier,
        episode_sort_order: ep.episode_sort_order,
        title: ep.title,
        description: ep.description,
        still_url,
        air_date: ep.air_date,
        runtime: ep.runtime,
        is_in_library: ep.is_in_library,
        want_status: ep.want_status,
        watched: None,
        watch_position_seconds: None,
        created_at: ep.created_at,
        updated_at: ep.updated_at,
    }
}

fn league_detail_to_response(
    detail: tv_shows::TvShowDetail,
    tallies: tv_shows::EpisodeTallies,
) -> LeagueDetailResponse {
    let show_id = detail.id;
    let poster_url = endpoint_url(
        &detail.images,
        "poster",
        LEAGUE_IMAGE_BASE,
        detail.id,
        "/poster",
    );
    let backdrop_url = endpoint_url(
        &detail.images,
        "backdrop",
        LEAGUE_IMAGE_BASE,
        detail.id,
        "/backdrop",
    );
    let logo_url = endpoint_url(
        &detail.images,
        "logo",
        LEAGUE_IMAGE_BASE,
        detail.id,
        "/logo",
    );
    LeagueDetailResponse {
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
        network: detail.network,
        is_monitored: detail.is_monitored,
        auto_want_mode: detail.auto_want_mode,
        quality_profile_id: detail.quality_profile_id,
        media_root_id: detail.media_root_id,
        content_category: detail.content_category,
        sport_metadata: detail.sport_metadata,
        first_aired: detail.first_aired,
        last_aired: detail.last_aired,
        next_airing: detail.next_airing,
        runtime: detail.runtime,
        total_episode_count: detail.total_episode_count,
        event_count: tallies.total,
        in_library_event_count: tallies.in_library,
        wanted_event_count: tallies.wanted,
        watched_event_count: tallies.watched,
        created_at: detail.created_at,
        updated_at: detail.updated_at,
        seasons: detail
            .seasons
            .into_iter()
            .map(|s| {
                let season_poster =
                    endpoint_url(&s.images, "poster", SEASON_IMAGE_BASE, s.id, "/poster");
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
                    episodes: s.episodes.into_iter().map(event_row_to_response).collect(),
                }
            })
            .collect(),
    }
}
