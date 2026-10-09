//! Library provider endpoints for disambiguation.
//!
//! Search shows and retrieve episode titles from the local library,
//! used by external services (e.g. koblem) for cross-referencing.

use axum::{
    Json, Router,
    extract::{Path, Query, State},
    http::{HeaderMap, StatusCode},
    response::IntoResponse,
    routing::{get, post},
};
use serde::{Deserialize, Serialize};
use serde_json::json;
use uuid::Uuid;

use crate::AppState;
use crate::repositories::{
    auth_tokens, ingest_tokens, tv_episode_files, tv_episodes, tv_seasons, tv_shows,
};
use crate::routes::extract_bearer;

// =============================================================================
// Route Setup
// =============================================================================

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/shows/search", get(search_shows))
        .route("/shows/{id}", get(get_show_detail))
        .route(
            "/shows/{id}/episodes/{season}/{episode}/title",
            get(get_episode_title),
        )
        .route("/build-media-want", post(build_media_want))
}

// =============================================================================
// Types
// =============================================================================

#[derive(Debug, Deserialize)]
struct SearchQuery {
    q: String,
}

#[derive(Debug, Serialize)]
struct ShowSummary {
    id: String,
    title: String,
    year: Option<i32>,
    status: Option<String>,
    episode_count: Option<i32>,
}

// =============================================================================
// Handlers
// =============================================================================

async fn search_shows(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(params): Query<SearchQuery>,
) -> Result<impl IntoResponse, (StatusCode, Json<serde_json::Value>)> {
    require_provider_auth(&state, &headers).await.map_err(|_| {
        (
            StatusCode::UNAUTHORIZED,
            Json(json!({ "error": "unauthorized" })),
        )
    })?;

    let pattern = format!("%{}%", params.q);

    let rows = sqlx::query_as!(
        ShowSummary,
        r#"
        SELECT
            id::text AS "id!",
            title AS "title!",
            year,
            status,
            total_episode_count AS episode_count
        FROM tv_shows
        WHERE title ILIKE $1
        ORDER BY title
        LIMIT 20
        "#,
        pattern,
    )
    .fetch_all(&state.pool)
    .await
    .map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({ "error": format!("query failed: {e}") })),
        )
    })?;

    Ok(Json(rows))
}

async fn get_show_detail(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
) -> Result<impl IntoResponse, (StatusCode, Json<serde_json::Value>)> {
    require_provider_auth(&state, &headers).await.map_err(|_| {
        (
            StatusCode::UNAUTHORIZED,
            Json(json!({ "error": "unauthorized" })),
        )
    })?;

    // Fetch show
    let show = sqlx::query!(
        r#"SELECT id, title, year, status, network, content_category, quality_profile_id
        FROM tv_shows WHERE id = $1"#,
        id,
    )
    .fetch_optional(&state.pool)
    .await
    .map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({ "error": format!("query failed: {e}") })),
        )
    })?;

    let show = match show {
        Some(s) => s,
        None => {
            return Err((
                StatusCode::NOT_FOUND,
                Json(json!({ "error": "show not found" })),
            ));
        }
    };

    // Fetch seasons with episodes
    let episodes = sqlx::query!(
        r#"SELECT s.id AS "season_id!", s.season_identifier,
                  e.id AS "episode_id!", e.episode_identifier,
                  e.title AS "episode_title?", e.air_date, e.want_status
        FROM tv_seasons s
        JOIN tv_episodes e ON e.season_id = s.id
        WHERE s.show_id = $1
        ORDER BY s.season_sort_order, e.episode_sort_order"#,
        id,
    )
    .fetch_all(&state.pool)
    .await
    .map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({ "error": format!("query failed: {e}") })),
        )
    })?;

    // Group episodes by season
    let mut seasons: Vec<serde_json::Value> = Vec::new();
    let mut current_season = String::new();
    let mut current_season_id = String::new();
    let mut current_episodes: Vec<serde_json::Value> = Vec::new();

    for ep in &episodes {
        if ep.season_identifier != current_season {
            if !current_season.is_empty() {
                seasons.push(json!({
                    "id": current_season_id,
                    "season_identifier": current_season,
                    "episodes": current_episodes,
                }));
                current_episodes = Vec::new();
            }
            current_season = ep.season_identifier.clone();
            current_season_id = ep.season_id.to_string();
        }
        current_episodes.push(json!({
            "id": ep.episode_id.to_string(),
            "episode_identifier": ep.episode_identifier,
            "title": ep.episode_title,
            "air_date": ep.air_date.map(|d| d.to_string()),
            "want_status": ep.want_status,
        }));
    }
    if !current_season.is_empty() {
        seasons.push(json!({
            "id": current_season_id,
            "season_identifier": current_season,
            "episodes": current_episodes,
        }));
    }

    Ok(Json(json!({
        "id": show.id.to_string(),
        "title": show.title,
        "year": show.year,
        "status": show.status,
        "network": show.network,
        "content_category": show.content_category,
        "quality_profile_id": show.quality_profile_id.map(|id| id.to_string()),
        "seasons": seasons,
    })))
}

async fn get_episode_title(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path((id, season, episode)): Path<(Uuid, String, String)>,
) -> Result<impl IntoResponse, (StatusCode, Json<serde_json::Value>)> {
    require_provider_auth(&state, &headers).await.map_err(|_| {
        (
            StatusCode::UNAUTHORIZED,
            Json(json!({ "error": "unauthorized" })),
        )
    })?;

    let title: Option<String> = sqlx::query_scalar!(
        r#"
        SELECT e.title AS "title?"
        FROM tv_episodes e
        JOIN tv_seasons s ON s.id = e.season_id
        JOIN tv_shows sh ON sh.id = s.show_id
        WHERE sh.id = $1
          AND s.season_identifier = $2
          AND e.episode_identifier = $3
        LIMIT 1
        "#,
        id,
        season,
        episode,
    )
    .fetch_optional(&state.pool)
    .await
    .map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({ "error": format!("query failed: {e}") })),
        )
    })?
    .flatten();

    match title {
        Some(t) => Ok(Json(json!({ "title": t }))),
        None => Err((
            StatusCode::NOT_FOUND,
            Json(json!({ "error": "episode not found" })),
        )),
    }
}

// ── Build MediaWant ─────────────────────────────────────────────────

#[derive(Deserialize)]
struct BuildMediaWantRequest {
    show_id: Uuid,
    season_id: Option<Uuid>,
    #[serde(default)]
    episode_ids: Vec<Uuid>,
}

/// Build a MediaWant identical to what kobbler sends kobload when wanting episodes.
/// Used by the playground for ad-hoc testing without creating watch patterns.
async fn build_media_want(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(payload): Json<BuildMediaWantRequest>,
) -> Result<impl IntoResponse, (StatusCode, Json<serde_json::Value>)> {
    require_provider_auth(&state, &headers).await.map_err(|_| {
        (
            StatusCode::UNAUTHORIZED,
            Json(json!({ "error": "unauthorized" })),
        )
    })?;

    let map_err = |e: crate::repositories::RepositoryError| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({ "error": format!("query failed: {e}") })),
        )
    };

    // Fetch show
    let show = tv_shows::get(&state.pool, payload.show_id)
        .await
        .map_err(map_err)?
        .ok_or_else(|| {
            (
                StatusCode::NOT_FOUND,
                Json(json!({ "error": "show not found" })),
            )
        })?;

    // Extract identifiers
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

    let season_id = payload.season_id.ok_or_else(|| {
        (
            StatusCode::BAD_REQUEST,
            Json(json!({ "error": "season_id is required" })),
        )
    })?;

    let season = tv_seasons::get(&state.pool, season_id)
        .await
        .map_err(map_err)?
        .ok_or_else(|| {
            (
                StatusCode::NOT_FOUND,
                Json(json!({ "error": "season not found" })),
            )
        })?;

    // Get episodes — specific ones if requested, otherwise all in the season
    let episodes = if payload.episode_ids.is_empty() {
        tv_episodes::list_for_season(&state.pool, season_id)
            .await
            .map_err(map_err)?
    } else {
        let mut eps = Vec::new();
        for ep_id in &payload.episode_ids {
            if let Some(ep) = tv_episodes::get(&state.pool, *ep_id)
                .await
                .map_err(map_err)?
            {
                eps.push(ep);
            }
        }
        eps
    };

    // Batch-fetch library quality
    let ep_ids: Vec<Uuid> = episodes.iter().map(|e| e.id).collect();
    let quality_map: std::collections::HashMap<Uuid, tv_episode_files::EpisodeLibraryQuality> =
        tv_episode_files::get_library_quality_batch(&state.pool, &ep_ids)
            .await
            .unwrap_or_default()
            .into_iter()
            .map(|q| (q.episode_id, q))
            .collect();

    // Build episodes_needed — same logic as tv_want.rs
    let mut episodes_needed = Vec::new();
    for ep in &episodes {
        let (in_library, library_video_height) = if ep.is_in_library {
            match quality_map.get(&ep.id) {
                Some(q) => (Some(true), q.video_height),
                None => (Some(true), None),
            }
        } else {
            (Some(false), None)
        };
        episodes_needed.push(nats_jobs::EpisodeRef {
            season: season.season_identifier.clone(),
            episode: ep.episode_identifier.clone(),
            sort_order: Some(ep.episode_sort_order),
            air_date: ep.air_date.map(|d| d.to_string()),
            in_library,
            library_video_height,
            title: ep.title.clone(),
        });
    }

    let last_air_date = episodes
        .iter()
        .filter_map(|ep| ep.air_date)
        .max()
        .map(|d| d.to_string());

    let media_want = nats_jobs::MediaWant::TvSeason {
        show_name: show.title.clone(),
        season: season.season_identifier.clone(),
        year: show.year.map(|y| y as u32),
        episodes_needed: Some(episodes_needed),
        total_episodes: season.episode_count.map(|c| c as u32),
        pack_preference: "auto".to_string(),
        last_air_date,
        identifiers: Some(identifiers),
        alternate_titles: show.alternate_titles.clone().filter(|v| !v.is_empty()),
        quality_profile_id: show.quality_profile_id,
        quality_overrides: show.preferred_languages.as_ref().map(|langs| {
            nats_jobs::QualityOverrides {
                preferred_languages: Some(langs.clone()),
                ..Default::default()
            }
        }),
    };

    let value = serde_json::to_value(&media_want).map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({ "error": format!("serialization failed: {e}") })),
        )
    })?;

    Ok(Json(value))
}

/// Authenticate provider requests: accepts admin session tokens OR API (ingest) tokens.
async fn require_provider_auth(state: &AppState, headers: &HeaderMap) -> Result<(), ()> {
    let token = extract_bearer(headers).unwrap_or_default();
    if token.is_empty() {
        return Err(());
    }

    // Try admin session token first
    if auth_tokens::validate_admin_token(&state.pool, &token)
        .await
        .ok()
        .flatten()
        .is_some()
    {
        return Ok(());
    }

    // Fall back to API (ingest) token
    if ingest_tokens::is_valid(&state.pool, &token)
        .await
        .unwrap_or(false)
    {
        return Ok(());
    }

    Err(())
}
