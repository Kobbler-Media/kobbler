//! Discover endpoints — curated trending/popular content from TMDB.

use std::collections::HashMap;

use axum::{
    Json,
    extract::{Path, Query, State},
    http::StatusCode,
    response::IntoResponse,
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tracing::{info, warn};
use uuid::Uuid;

use crate::AppState;
use crate::LibraryEvent;
use crate::images::ImageEntry;
use crate::jobs::handlers::discover_sync::{DISCOVER_PROVIDER, DiscoverItem, DiscoverSection};
use crate::jobs::{self, JobContext};
use crate::repositories::{media_roots, metadata_cache, tv_episodes, tv_seasons, tv_shows};
use crate::routes::AuthUser;
use crate::routes::admin::dashboard::{RecentItem, WatchNextItem, fetch_home_sections};
use crate::routes::admin::library_status::{batch_movie_status, batch_tv_status};

/// Response for `GET /api/v1/admin/discover`.
#[derive(Serialize)]
pub struct DiscoverResponse {
    pub watch_next: Vec<WatchNextItem>,
    pub recently_added: Vec<RecentItem>,
    pub sections: Vec<DiscoverSectionWithStatus>,
}

/// A discover section with library status merged into each item.
#[derive(Serialize)]
pub struct DiscoverSectionWithStatus {
    pub title: String,
    pub section_key: String,
    pub sort_order: i32,
    pub items: Vec<DiscoverItemWithStatus>,
}

/// A discover item with library status merged in.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DiscoverItemWithStatus {
    #[serde(flatten)]
    pub item: DiscoverItem,
    /// UUID of the library entity if it exists.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub library_id: Option<Uuid>,
    /// Current want status if in library.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub want_status: Option<String>,
    /// Whether the item is in the user's library.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub is_in_library: Option<bool>,
}

/// GET /api/v1/admin/discover
///
/// Returns cached trending/popular content with library status flags.
pub async fn list_discover(State(state): State<AppState>, auth: AuthUser) -> impl IntoResponse {
    let pool = &state.pool;

    // Load library sections (watch next + recently added)
    let (watch_next, recently_added) = fetch_home_sections(&state, auth.user_id, auth.is_kids)
        .await
        .unwrap_or_default();

    // Load all cached discover sections
    let cached = match metadata_cache::list_by_provider(pool, DISCOVER_PROVIDER).await {
        Ok(entries) => entries,
        Err(e) => {
            warn!("Failed to load discover cache: {e}");
            vec![]
        }
    };

    // Deserialize all sections
    let mut sections: Vec<DiscoverSection> = cached
        .into_iter()
        .filter_map(|(_key, value)| serde_json::from_value(value).ok())
        .collect();
    sections.retain(|s| s.kids == auth.is_kids);
    sections.sort_by_key(|s| s.sort_order);

    // Collect all TMDB IDs across all sections for batch library lookup
    let mut movie_tmdb_ids = Vec::new();
    let mut tv_tmdb_ids = Vec::new();
    for section in &sections {
        for item in &section.items {
            if let Some(tmdb_id) = extract_tmdb_id(&item.identifiers) {
                match item.media_type.as_str() {
                    "movie" => movie_tmdb_ids.push(tmdb_id),
                    "tv" => tv_tmdb_ids.push(tmdb_id),
                    _ => {}
                }
            }
        }
    }

    // Batch lookup library status
    let movie_status = batch_movie_status(pool, &movie_tmdb_ids).await;
    let tv_status = batch_tv_status(pool, &tv_tmdb_ids).await;

    // Merge status into sections
    let sections_with_status: Vec<DiscoverSectionWithStatus> = sections
        .into_iter()
        .map(|section| {
            let items = section
                .items
                .into_iter()
                .map(|item| {
                    let status = extract_tmdb_id(&item.identifiers).and_then(|id| {
                        match item.media_type.as_str() {
                            "movie" => movie_status.get(&id),
                            "tv" => tv_status.get(&id),
                            _ => None,
                        }
                    });
                    DiscoverItemWithStatus {
                        item,
                        library_id: status.map(|s| s.id),
                        want_status: status.and_then(|s| s.want_status.clone()),
                        is_in_library: status.and_then(|s| s.is_in_library),
                    }
                })
                .collect();
            DiscoverSectionWithStatus {
                title: section.title,
                section_key: section.section_key,
                sort_order: section.sort_order,
                items,
            }
        })
        .collect();

    Json(DiscoverResponse {
        watch_next,
        recently_added,
        sections: sections_with_status,
    })
    .into_response()
}

/// POST /api/v1/admin/discover/refresh
///
/// Triggers an immediate discover sync job.
pub async fn refresh_discover(State(state): State<AppState>, auth: AuthUser) -> impl IntoResponse {
    if let Err(e) = auth.require_contributor() {
        return e.into_response();
    }

    if let Err(e) = state
        .nats
        .publisher()
        .enqueue_job("kobbler", "discover_sync", json!({}))
        .await
    {
        return (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({ "error": format!("Failed to enqueue discover sync: {e}") })),
        )
            .into_response();
    }

    StatusCode::ACCEPTED.into_response()
}

/// GET /api/v1/admin/discover/:media_type/:provider/:provider_id
///
/// Fetches full details for a single discover item directly from the provider.
/// Does NOT create a library entry.
pub async fn get_discover_detail(
    State(state): State<AppState>,
    auth: AuthUser,
    Path((media_type, provider, provider_id)): Path<(String, String, String)>,
) -> impl IntoResponse {
    let pool = &state.pool;

    if auth.is_kids
        && !(provider == "tmdb"
            && crate::kids::discover_title_allowed(pool, &media_type, &provider_id)
                .await
                .unwrap_or(false))
    {
        return (StatusCode::NOT_FOUND, "not available on a Kids profile").into_response();
    }

    match media_type.as_str() {
        "movie" | "tv" => get_video_detail(pool, &media_type, &provider, &provider_id).await,
        _ => (
            StatusCode::BAD_REQUEST,
            Json(json!({ "error": format!("Unknown media_type: {media_type}") })),
        )
            .into_response(),
    }
}

// ============================================================================
// Watch Providers
// ============================================================================

#[derive(Debug, Deserialize)]
pub struct WatchProvidersQuery {
    pub region: Option<String>,
}

/// GET /api/v1/admin/discover/watch-providers
///
/// Returns available streaming services from TMDB for a given region.
pub async fn list_watch_providers(
    State(state): State<AppState>,
    _auth: AuthUser,
    Query(params): Query<WatchProvidersQuery>,
) -> impl IntoResponse {
    let region = params.region.unwrap_or_else(|| {
        state
            .runtime_config
            .try_get()
            .map(|c| c.discover_region.clone())
            .unwrap_or_else(|| "US".to_string())
    });

    let pool = &state.pool;

    // Get TMDB config
    let tmdb_config = match crate::repositories::video_metadata_providers::list_enabled(pool).await
    {
        Ok(providers) => providers
            .into_iter()
            .find(|p| p.provider_type == "tmdb")
            .map(|p| p.config),
        Err(e) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({ "error": format!("Failed to query providers: {e}") })),
            )
                .into_response();
        }
    };

    let config = match tmdb_config {
        Some(c) => c,
        None => {
            return (
                StatusCode::SERVICE_UNAVAILABLE,
                Json(json!({ "error": "TMDB not enabled" })),
            )
                .into_response();
        }
    };

    let client = crate::http::new_client();
    let tmdb = crate::video_metadata::tmdb::TmdbProvider::from_config(client, &config);

    match tmdb.watch_providers_for_region(&region).await {
        Ok(providers) => Json(json!({ "providers": providers, "region": region })).into_response(),
        Err(e) => (
            StatusCode::BAD_GATEWAY,
            Json(json!({ "error": format!("TMDB error: {e}") })),
        )
            .into_response(),
    }
}

// ============================================================================
// Internals
// ============================================================================

fn extract_tmdb_id(identifiers: &Value) -> Option<String> {
    identifiers
        .get("tmdb_id")
        .and_then(|v| v.as_str())
        .map(String::from)
}

async fn get_video_detail(
    pool: &sqlx::PgPool,
    media_type: &str,
    provider: &str,
    provider_id: &str,
) -> axum::response::Response {
    let registry = crate::video_metadata::VideoProviderRegistry::new(pool.clone());

    match media_type {
        "movie" => match registry.get_movie(provider, provider_id).await {
            Ok(details) => {
                let library_status = batch_movie_status(pool, &[provider_id.to_string()]).await;
                let status = library_status.get(provider_id);
                Json(json!({
                    "media_type": "movie",
                    "provider": provider,
                    "details": details,
                    "library_id": status.map(|s| s.id),
                    "want_status": status.and_then(|s| s.want_status.clone()),
                    "is_in_library": status.and_then(|s| s.is_in_library),
                }))
                .into_response()
            }
            Err(e) => (
                StatusCode::BAD_GATEWAY,
                Json(json!({ "error": format!("{provider} error: {e}") })),
            )
                .into_response(),
        },
        "tv" => match registry.get_tv_show(provider, provider_id).await {
            Ok(details) => {
                let library_status = batch_tv_status(pool, &[provider_id.to_string()]).await;
                let status = library_status.get(provider_id);
                Json(json!({
                    "media_type": "tv",
                    "provider": provider,
                    "details": details,
                    "library_id": status.map(|s| s.id),
                    "want_status": status.and_then(|s| s.want_status.clone()),
                    "is_in_library": status.and_then(|s| s.is_in_library),
                }))
                .into_response()
            }
            Err(e) => (
                StatusCode::BAD_GATEWAY,
                Json(json!({ "error": format!("{provider} error: {e}") })),
            )
                .into_response(),
        },
        _ => (
            StatusCode::BAD_REQUEST,
            Json(json!({ "error": format!("Invalid media_type: {media_type}") })),
        )
            .into_response(),
    }
}

/// GET /api/v1/admin/discover/tv/{provider}/{provider_id}/season/{season_number}
///
/// Returns season detail with episodes from the metadata provider.
pub async fn get_discover_season(
    State(state): State<AppState>,
    _auth: AuthUser,
    Path((provider, provider_id, season_number)): Path<(String, String, u32)>,
) -> impl IntoResponse {
    let registry = crate::video_metadata::VideoProviderRegistry::new(state.pool.clone());

    match registry
        .get_season(&provider, &provider_id, season_number)
        .await
    {
        Ok(details) => Json(json!(details)).into_response(),
        Err(e) => (
            StatusCode::BAD_GATEWAY,
            Json(json!({ "error": format!("{provider} error: {e}") })),
        )
            .into_response(),
    }
}

// =============================================================================
// Want from Discover — shared show/season resolution
// =============================================================================

/// Everything needed to land a not-yet-in-library show and one of its seasons.
/// Shared by the episode and season handlers so the two cannot drift.
#[derive(Debug, Deserialize)]
pub struct DiscoverShowSpec {
    pub provider: String,
    pub provider_id: String,
    pub season_number: u32,
    pub title: String,
    pub year: Option<i32>,
    pub identifiers: Option<serde_json::Value>,
    pub description: Option<String>,
    pub poster_url: Option<String>,
    pub backdrop_url: Option<String>,
    pub genres: Option<Vec<String>>,
    pub rating: Option<f64>,
    pub rating_count: Option<i32>,
    pub runtime: Option<i32>,
    pub network: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct WantEpisodeRequest {
    #[serde(flatten)]
    pub show: DiscoverShowSpec,
    pub episode_number: u32,
}

#[derive(Debug, Deserialize)]
pub struct WantSeasonRequest {
    #[serde(flatten)]
    pub show: DiscoverShowSpec,
}

struct ResolvedSeason {
    show_id: Uuid,
    season_id: Uuid,
    title: String,
    season_number: u32,
    provider_episodes: Vec<crate::video_metadata::EpisodeDetails>,
}

fn internal(msg: String) -> (StatusCode, Json<Value>) {
    (
        StatusCode::INTERNAL_SERVER_ERROR,
        Json(json!({ "error": msg })),
    )
}

/// Find or create the show, find or create the season, and fetch the season's
/// episode list from the metadata provider.
async fn resolve_discover_season(
    state: &AppState,
    spec: DiscoverShowSpec,
) -> Result<ResolvedSeason, (StatusCode, Json<Value>)> {
    let pool = &state.pool;

    let identifier_key = format!("{}_id", spec.provider);
    let show_id = match tv_shows::get_by_identifier(pool, &identifier_key, &spec.provider_id).await
    {
        Ok(Some(show)) => {
            info!(show_id = %show.id, "Found existing show for discover want");
            show.id
        }
        Ok(None) => {
            let mut img_map = HashMap::new();
            if let Some(url) = spec.poster_url.clone() {
                img_map.insert(
                    "poster".to_string(),
                    ImageEntry {
                        source_url: Some(url),
                        cached_at: None,
                    },
                );
            }
            if let Some(url) = spec.backdrop_url.clone() {
                img_map.insert(
                    "backdrop".to_string(),
                    ImageEntry {
                        source_url: Some(url),
                        cached_at: None,
                    },
                );
            }

            let media_root_id = media_roots::get_default_for_type(pool, "tv")
                .await
                .ok()
                .flatten()
                .map(|r| r.id);

            let input = tv_shows::TvShowInput {
                title: spec.title.clone(),
                year: spec.year,
                description: spec.description.clone(),
                images: if img_map.is_empty() {
                    None
                } else {
                    Some(img_map)
                },
                identifiers: spec.identifiers.clone(),
                genres: spec.genres.clone(),
                rating: spec.rating,
                rating_count: spec.rating_count,
                network: spec.network.clone(),
                runtime: spec.runtime,
                is_monitored: Some(true),
                auto_want_mode: Some("none".to_string()),
                media_root_id,
                ..Default::default()
            };

            match tv_shows::upsert_by_provider(pool, &spec.provider, &spec.provider_id, input).await
            {
                Ok(id) => {
                    info!(show_id = %id, "Created show for discover want");
                    id
                }
                Err(e) => return Err(internal(format!("Failed to create show: {e}"))),
            }
        }
        Err(e) => return Err(internal(format!("Failed to look up show: {e}"))),
    };

    let season_str = spec.season_number.to_string();
    let season_id = {
        let seasons = tv_seasons::list_by_show(pool, show_id)
            .await
            .map_err(|e| internal(format!("Failed to list seasons: {e}")))?;

        match seasons.iter().find(|s| s.season_identifier == season_str) {
            Some(s) => s.id,
            None => tv_seasons::create(
                pool,
                tv_seasons::SeasonInput {
                    show_id,
                    season_identifier: season_str,
                    season_sort_order: spec.season_number as i32,
                    title: None,
                    description: None,
                    images: None,
                    air_date: None,
                    episode_count: None,
                    is_monitored: true,
                    is_special: false,
                    auto_want_mode: "none".to_string(),
                },
            )
            .await
            .map_err(|e| internal(format!("Failed to create season: {e}")))?,
        }
    };

    let registry = crate::video_metadata::VideoProviderRegistry::new(pool.clone());
    let identifiers = spec.identifiers.clone().unwrap_or_else(|| json!({}));
    let provider_episodes = match registry
        .get_season_merged(
            &spec.provider,
            &spec.provider_id,
            spec.season_number,
            &identifiers,
        )
        .await
    {
        Ok(details) => details.episodes,
        Err(e) => {
            warn!(error = %e, "Failed to fetch season from provider, continuing without it");
            vec![]
        }
    };

    Ok(ResolvedSeason {
        show_id,
        season_id,
        title: spec.title,
        season_number: spec.season_number,
        provider_episodes,
    })
}

fn episode_input(
    season_id: Uuid,
    number: u32,
    provider_ep: Option<&crate::video_metadata::EpisodeDetails>,
) -> tv_episodes::EpisodeInput {
    tv_episodes::EpisodeInput {
        season_id,
        episode_identifier: number.to_string(),
        episode_sort_order: number as i32,
        title: provider_ep.and_then(|e| e.title.clone()),
        description: provider_ep.and_then(|e| e.description.clone()),
        images: provider_ep.and_then(|e| {
            e.still_url.as_ref().map(|url| {
                let mut map = HashMap::new();
                map.insert(
                    "still".to_string(),
                    ImageEntry {
                        source_url: Some(url.clone()),
                        cached_at: None,
                    },
                );
                map
            })
        }),
        air_date: provider_ep
            .and_then(|e| e.air_date.as_deref())
            .and_then(|d| {
                time::Date::parse(d, &time::format_description::well_known::Iso8601::DEFAULT).ok()
            }),
        air_time: provider_ep
            .and_then(|e| e.air_time.as_deref())
            .and_then(crate::video_metadata::parse_air_time),
        runtime: provider_ep.and_then(|e| e.runtime.map(|r| r as i32)),
        identifiers: json!({}),
        country: provider_ep.and_then(|e| e.country.clone()),
    }
}

/// Enqueue the catch-up enrichment that fills in the rest of the show.
async fn enqueue_show_enrich(state: &AppState, show_id: Uuid, title: &str) {
    let display = jobs::fetch_job_display(&state.pool, "tv_show", show_id).await;
    let context =
        JobContext::for_entity("tv_show", show_id, format!("Enriching TV show '{title}'"))
            .with_display_opt(display);
    if let Err(e) = jobs::enqueue(
        &state.pool,
        state.nats.publisher(),
        "tv_show_enrich",
        json!({ "show_id": show_id }),
        context,
    )
    .await
    {
        warn!(show_id = %show_id, error = %e, "Failed to enqueue TV show enrichment");
    }
}

// =============================================================================
// Want Episode from Discover
// =============================================================================

/// POST /api/v1/admin/discover/want-episode
///
/// Auto-creates a TV show (if not exists), syncs a specific season from the
/// metadata provider, creates the episode, and wants it — all synchronously.
pub async fn want_discover_episode(
    State(state): State<AppState>,
    auth: AuthUser,
    Json(body): Json<WantEpisodeRequest>,
) -> impl IntoResponse {
    if let Err(e) = auth.require_contributor() {
        return e.into_response();
    }

    let episode_number = body.episode_number;
    let resolved = match resolve_discover_season(&state, body.show).await {
        Ok(r) => r,
        Err((status, body)) => return (status, body).into_response(),
    };
    let pool = &state.pool;

    let ep_str = episode_number.to_string();
    let provider_ep = resolved
        .provider_episodes
        .iter()
        .find(|e| e.episode_number == episode_number);

    let episode_id =
        match tv_episodes::get_by_season_and_identifier(pool, resolved.season_id, &ep_str).await {
            Ok(Some(ep)) => ep.id,
            Ok(None) => {
                match tv_episodes::create(
                    pool,
                    episode_input(resolved.season_id, episode_number, provider_ep),
                )
                .await
                {
                    Ok(id) => id,
                    Err(e) => {
                        return internal(format!("Failed to create episode: {e}")).into_response();
                    }
                }
            }
            Err(e) => return internal(format!("Failed to look up episode: {e}")).into_response(),
        };

    if let Err(e) = tv_episodes::set_want_status(pool, episode_id, "wanted").await {
        return internal(format!("Failed to set want status: {e}")).into_response();
    }

    let _ = state
        .nats
        .publisher()
        .enqueue_job(
            "kobbler",
            "tv_want",
            json!({
                "show_id": resolved.show_id,
                "season_id": resolved.season_id,
                "episode_ids": [episode_id],
                "user_id": auth.user_id,
            }),
        )
        .await;

    enqueue_show_enrich(&state, resolved.show_id, &resolved.title).await;

    let _ = state
        .library_events
        .send(LibraryEvent::tv_show_updated(resolved.show_id));
    let _ = state
        .library_events
        .send(LibraryEvent::tv_episode_updated(episode_id));

    (
        StatusCode::OK,
        Json(json!({
            "show_id": resolved.show_id,
            "season_id": resolved.season_id,
            "episode_id": episode_id,
        })),
    )
        .into_response()
}

// =============================================================================
// Want Season from Discover
// =============================================================================

#[derive(Debug, Serialize)]
pub struct WantSeasonEpisode {
    pub episode_id: Uuid,
    pub episode_number: i32,
    pub title: Option<String>,
}

/// Counts are structured rather than prose so a caller can say "you already
/// have 22 of those, grabbing the other 2" without re-deriving anything.
#[derive(Debug, Serialize)]
pub struct WantSeasonResponse {
    pub show_id: Uuid,
    pub season_id: Uuid,
    pub season_number: u32,
    pub total_episodes: usize,
    pub already_have: usize,
    pub already_wanted: usize,
    pub newly_wanted: usize,
    pub newly_wanted_episodes: Vec<WantSeasonEpisode>,
    /// False when every episode was already present or already wanted, so
    /// nothing was re-queued.
    pub queued: bool,
}

/// POST /api/v1/admin/discover/want-season
///
/// The sibling of `want_discover_episode` for a whole season: creates the show
/// if needed, syncs every episode of the season from the metadata provider, and
/// wants only the ones that are neither present nor already wanted.
pub async fn want_discover_season(
    State(state): State<AppState>,
    auth: AuthUser,
    Json(body): Json<WantSeasonRequest>,
) -> impl IntoResponse {
    if let Err(e) = auth.require_contributor() {
        return e.into_response();
    }

    let resolved = match resolve_discover_season(&state, body.show).await {
        Ok(r) => r,
        Err((status, body)) => return (status, body).into_response(),
    };
    let pool = &state.pool;

    if !resolved.provider_episodes.is_empty() {
        let inputs: Vec<tv_episodes::EpisodeInput> = resolved
            .provider_episodes
            .iter()
            .map(|ep| episode_input(resolved.season_id, ep.episode_number, Some(ep)))
            .collect();
        if let Err(e) = tv_episodes::bulk_upsert(pool, resolved.season_id, inputs).await {
            return internal(format!("Failed to sync season episodes: {e}")).into_response();
        }
    }

    let all = match tv_episodes::list_for_season(pool, resolved.season_id).await {
        Ok(rows) => rows,
        Err(e) => return internal(format!("Failed to list season episodes: {e}")).into_response(),
    };
    if all.is_empty() {
        return (
            StatusCode::NOT_FOUND,
            Json(json!({
                "error": format!(
                    "No episodes found for season {} — the provider returned none",
                    resolved.season_number
                )
            })),
        )
            .into_response();
    }

    let already_have = all.iter().filter(|e| e.is_in_library).count();
    let already_wanted = all
        .iter()
        .filter(|e| !e.is_in_library && e.want_status != "none")
        .count();

    let missing = match tv_episodes::list_missing(pool, resolved.season_id).await {
        Ok(rows) => rows,
        Err(e) => {
            return internal(format!("Failed to list missing episodes: {e}")).into_response();
        }
    };

    let newly_wanted_episodes: Vec<WantSeasonEpisode> = missing
        .iter()
        .map(|ep| WantSeasonEpisode {
            episode_id: ep.id,
            episode_number: ep.episode_sort_order,
            title: ep.title.clone(),
        })
        .collect();

    let queued = !missing.is_empty();
    if queued {
        let episode_ids: Vec<Uuid> = missing.iter().map(|ep| ep.id).collect();
        if let Err(e) = tv_episodes::set_want_status_batch(pool, &episode_ids, "wanted").await {
            return internal(format!("Failed to set want status: {e}")).into_response();
        }

        let _ = state
            .nats
            .publisher()
            .enqueue_job(
                "kobbler",
                "tv_want",
                json!({
                    "show_id": resolved.show_id,
                    "season_id": resolved.season_id,
                    "episode_ids": episode_ids,
                    "user_id": auth.user_id,
                }),
            )
            .await;

        for id in &episode_ids {
            let _ = state
                .library_events
                .send(LibraryEvent::tv_episode_updated(*id));
        }
    }

    enqueue_show_enrich(&state, resolved.show_id, &resolved.title).await;

    let _ = state
        .library_events
        .send(LibraryEvent::tv_show_updated(resolved.show_id));
    let _ = state
        .library_events
        .send(LibraryEvent::tv_season_updated(resolved.season_id));

    (
        StatusCode::OK,
        Json(WantSeasonResponse {
            show_id: resolved.show_id,
            season_id: resolved.season_id,
            season_number: resolved.season_number,
            total_episodes: all.len(),
            already_have,
            already_wanted,
            newly_wanted: newly_wanted_episodes.len(),
            newly_wanted_episodes,
            queued,
        }),
    )
        .into_response()
}
