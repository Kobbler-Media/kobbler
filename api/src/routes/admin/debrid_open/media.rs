use axum::Json;
use axum::http::StatusCode;
use axum::response::IntoResponse;
use serde_json::json;
use tracing::{error, info, warn};
use uuid::Uuid;

use crate::AppState;
use crate::jobs::JobContext;
use crate::repositories::{movies, tv_episodes, tv_seasons, tv_shows};
use crate::routes::admin::debrid_stream::StreamResolveBody;

/// The library item a stream plays, created from the Discover payload when there is none
/// yet, and marked as streamed.
pub(crate) async fn commit_media(
    state: &AppState,
    body: &StreamResolveBody,
) -> Result<(Uuid, String), axum::response::Response> {
    let (media_id, media_type) = resolve_or_create_media_id(state, body).await?;
    enqueue_stream_enrich_for_media(state, media_id, &media_type).await;
    mark_streamed(state, media_id, &media_type).await;
    Ok((media_id, media_type))
}

async fn mark_streamed(state: &AppState, media_id: Uuid, media_type: &str) {
    match media_type {
        "movie" => {
            let _ = sqlx::query!(
                "UPDATE movies SET streamed_at = COALESCE(streamed_at, NOW()) WHERE id = $1",
                media_id,
            )
            .execute(&state.pool)
            .await;
        }
        "episode" => {
            let _ = sqlx::query!(
                r#"
                UPDATE tv_shows SET streamed_at = COALESCE(streamed_at, NOW())
                WHERE id = (
                    SELECT ts.show_id FROM tv_episodes te
                    JOIN tv_seasons ts ON ts.id = te.season_id
                    WHERE te.id = $1
                )
                "#,
                media_id,
            )
            .execute(&state.pool)
            .await;
        }
        _ => {}
    }
}

/// Library identifiers are matched as text (`identifiers->>'tmdb_id'`), but Discover
/// clients send TMDB ids as JSON numbers; stored as numbers they never match again and
/// every stream of the title creates another copy.
fn string_identifiers(identifiers: Option<serde_json::Value>) -> serde_json::Value {
    let mut identifiers = identifiers.unwrap_or_else(|| json!({}));
    if let Some(map) = identifiers.as_object_mut() {
        for value in map.values_mut() {
            if let Some(n) = value.as_i64() {
                *value = json!(n.to_string());
            }
        }
    }
    identifiers
}

/// Stream entities carry only the thin discover payload (title, year, poster,
/// genres). Enrichment is what fills in cast, crew, tagline, content rating,
/// logos, and the full season/episode list, so a streamed title's detail page
/// is as complete as a library one's.
///
/// Runs on every resolve, not just on create: titles streamed before this was
/// wired have `enriched_at IS NULL` forever, and pressing Stream on one is the
/// natural moment to catch it up. Enqueued rather than awaited — stream resolve
/// is latency-sensitive and the page picks the data up over SSE.
async fn enqueue_stream_enrich(state: &AppState, entity_type: &str, entity_id: Uuid) {
    let (job_type, payload_key) = match entity_type {
        "movie" => ("movie_enrich", "movie_id"),
        "tv_show" => ("tv_show_enrich", "show_id"),
        _ => return,
    };

    // Unenriched, and identifiable by a provider the enrich job can actually use.
    let title = match entity_type {
        "movie" => {
            sqlx::query_scalar!(
                "SELECT title FROM movies
             WHERE id = $1 AND enriched_at IS NULL
               AND (identifiers->>'tmdb_id' IS NOT NULL OR identifiers->>'imdb_id' IS NOT NULL)",
                entity_id,
            )
            .fetch_optional(&state.pool)
            .await
        }
        _ => {
            sqlx::query_scalar!(
                "SELECT title FROM tv_shows
             WHERE id = $1 AND enriched_at IS NULL
               AND (identifiers->>'tmdb_id' IS NOT NULL OR identifiers->>'imdb_id' IS NOT NULL)",
                entity_id,
            )
            .fetch_optional(&state.pool)
            .await
        }
    };
    let Ok(Some(title)) = title else {
        return;
    };

    if crate::jobs::has_active_job(&state.pool, job_type, payload_key, &entity_id.to_string())
        .await
        .unwrap_or(false)
    {
        return;
    }
    let display = crate::jobs::fetch_job_display(&state.pool, entity_type, entity_id).await;
    let context = JobContext::for_entity(entity_type, entity_id, format!("Enriching '{title}'"))
        .with_display_opt(display);
    if let Err(e) = crate::jobs::enqueue(
        &state.pool,
        state.nats.publisher(),
        job_type,
        json!({ payload_key: entity_id }),
        context,
    )
    .await
    {
        warn!(error = %e, entity_id = %entity_id, "Failed to enqueue stream enrichment");
    }
}

/// Map a resolved stream target onto the entity enrichment actually applies to:
/// an episode's metadata comes from its show, not from the episode row.
pub(crate) async fn enqueue_stream_enrich_for_media(
    state: &AppState,
    media_id: Uuid,
    media_type: &str,
) {
    match media_type {
        "movie" => enqueue_stream_enrich(state, "movie", media_id).await,
        "tv" => enqueue_stream_enrich(state, "tv_show", media_id).await,
        "episode" => {
            let show_id = sqlx::query_scalar!(
                "SELECT s.show_id FROM tv_episodes e
                 JOIN tv_seasons s ON s.id = e.season_id
                 WHERE e.id = $1",
                media_id,
            )
            .fetch_optional(&state.pool)
            .await
            .ok()
            .flatten();
            if let Some(show_id) = show_id {
                enqueue_stream_enrich(state, "tv_show", show_id).await;
            }
        }
        _ => {}
    }
}

/// Resolve the media_id from the request. If `auto_create` is set and no
/// `media_id` is provided, find-or-create the movie/show/episode from metadata.
/// Returns `(media_id, effective_media_type)` — media_type may be upgraded
/// from "tv" to "episode" when an episode entity is auto-created.
pub(crate) async fn resolve_or_create_media_id(
    state: &AppState,
    body: &StreamResolveBody,
) -> Result<(Uuid, String), axum::response::Response> {
    // If a media_id was provided directly, use it
    if let Some(id) = body.media_id
        && id != Uuid::nil()
    {
        return Ok((id, body.media_type.clone()));
    }

    // Need auto_create metadata to proceed
    let ac = match &body.auto_create {
        Some(ac) => ac,
        None => {
            return Err((
                StatusCode::BAD_REQUEST,
                Json(json!({"error": "media_id or auto_create required"})),
            )
                .into_response());
        }
    };

    let identifiers = string_identifiers(ac.identifiers.clone());

    match body.media_type.as_str() {
        "movie" => {
            // Check if movie already exists by TMDB or IMDB ID
            if let Some(tmdb_id) = identifiers.get("tmdb_id").and_then(|v| v.as_str())
                && let Ok(Some(existing)) =
                    movies::get_by_identifier(&state.pool, "tmdb_id", tmdb_id).await
            {
                return Ok((existing.id, "movie".to_string()));
            }
            if let Some(imdb_id) = identifiers.get("imdb_id").and_then(|v| v.as_str())
                && let Ok(Some(existing)) =
                    movies::get_by_identifier(&state.pool, "imdb_id", imdb_id).await
            {
                return Ok((existing.id, "movie".to_string()));
            }

            // Build images from poster/backdrop
            let mut images = crate::images::ImageMap::new();
            if let Some(ref url) = ac.poster_url {
                images.insert(
                    "poster".to_string(),
                    crate::images::ImageEntry {
                        source_url: Some(url.clone()),
                        cached_at: None,
                    },
                );
            }
            if let Some(ref url) = ac.backdrop_url {
                images.insert(
                    "backdrop".to_string(),
                    crate::images::ImageEntry {
                        source_url: Some(url.clone()),
                        cached_at: None,
                    },
                );
            }

            let input = movies::MovieInput {
                title: ac.title.clone(),
                year: ac.year,
                description: ac.description.clone(),
                images: Some(images),
                identifiers: Some(identifiers),
                genres: ac.genres.clone(),
                rating: ac.rating,
                rating_count: ac.rating_count,
                runtime: ac.runtime,
                streamed_at: Some(time::OffsetDateTime::now_utc()),
                ..Default::default()
            };
            match movies::create(&state.pool, input).await {
                Ok(id) => {
                    info!(movie_id = %id, title = %ac.title, "Auto-created movie from stream");
                    Ok((id, "movie".to_string()))
                }
                Err(e) => {
                    error!(error = %e, "Failed to auto-create movie");
                    Err((
                        StatusCode::INTERNAL_SERVER_ERROR,
                        Json(json!({"error": "failed to create movie"})),
                    )
                        .into_response())
                }
            }
        }
        "tv" | "episode" => {
            // Find or create the TV show by TMDB/IMDB ID
            let show_id = if let Some(tmdb_id) = identifiers.get("tmdb_id").and_then(|v| v.as_str())
                && let Ok(Some(existing)) =
                    tv_shows::get_by_identifier(&state.pool, "tmdb_id", tmdb_id).await
            {
                existing.id
            } else if let Some(imdb_id) = identifiers.get("imdb_id").and_then(|v| v.as_str())
                && let Ok(Some(existing)) =
                    tv_shows::get_by_identifier(&state.pool, "imdb_id", imdb_id).await
            {
                existing.id
            } else {
                let mut images = crate::images::ImageMap::new();
                if let Some(ref url) = ac.poster_url {
                    images.insert(
                        "poster".to_string(),
                        crate::images::ImageEntry {
                            source_url: Some(url.clone()),
                            cached_at: None,
                        },
                    );
                }
                if let Some(ref url) = ac.backdrop_url {
                    images.insert(
                        "backdrop".to_string(),
                        crate::images::ImageEntry {
                            source_url: Some(url.clone()),
                            cached_at: None,
                        },
                    );
                }
                let input = tv_shows::TvShowInput {
                    title: ac.title.clone(),
                    year: ac.year,
                    description: ac.description.clone(),
                    images: Some(images),
                    identifiers: Some(identifiers.clone()),
                    genres: ac.genres.clone(),
                    rating: ac.rating,
                    rating_count: ac.rating_count,
                    network: ac.network.clone(),
                    is_monitored: Some(true),
                    auto_want_mode: Some("none".to_string()),
                    runtime: ac.runtime,
                    streamed_at: Some(time::OffsetDateTime::now_utc()),
                    ..Default::default()
                };
                match tv_shows::create(&state.pool, input).await {
                    Ok(id) => {
                        info!(show_id = %id, title = %ac.title, "Auto-created TV show from stream");
                        id
                    }
                    Err(e) => {
                        error!(error = %e, "Failed to auto-create TV show");
                        return Err((
                            StatusCode::INTERNAL_SERVER_ERROR,
                            Json(json!({"error": "failed to create TV show"})),
                        )
                            .into_response());
                    }
                }
            };

            // If season/episode info is provided, create the full chain and
            // return the episode ID so the session uses media_type="episode".
            if let Some(season_num) = ac.season_number
                && let Some(episode_num) = ac.episode_number
            {
                let pool = &state.pool;

                // Find or create the season
                let season_str = season_num.to_string();
                let season_id = match tv_seasons::list_by_show(pool, show_id).await {
                    Ok(seasons) => {
                        if let Some(s) = seasons.iter().find(|s| s.season_identifier == season_str)
                        {
                            s.id
                        } else {
                            match tv_seasons::create(
                                pool,
                                tv_seasons::SeasonInput {
                                    show_id,
                                    season_identifier: season_str.clone(),
                                    season_sort_order: season_num as i32,
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
                            {
                                Ok(id) => id,
                                Err(e) => {
                                    warn!(error = %e, "Failed to create season, falling back to show ID");
                                    return Ok((show_id, "tv".to_string()));
                                }
                            }
                        }
                    }
                    Err(e) => {
                        warn!(error = %e, "Failed to list seasons, falling back to show ID");
                        return Ok((show_id, "tv".to_string()));
                    }
                };

                // Sync episode data from metadata provider
                let provider_episodes = if let Some(ref provider) = ac.provider
                    && let Some(ref provider_id) = ac.provider_id
                {
                    let registry =
                        crate::video_metadata::VideoProviderRegistry::new(state.pool.clone());
                    match registry.get_season(provider, provider_id, season_num).await {
                        Ok(details) => details.episodes,
                        Err(e) => {
                            warn!(error = %e, "Failed to fetch season from provider");
                            vec![]
                        }
                    }
                } else {
                    vec![]
                };

                // Find or create the episode
                let ep_str = episode_num.to_string();
                let provider_ep = provider_episodes
                    .iter()
                    .find(|e| e.episode_number == episode_num);

                let episode_id = match tv_episodes::get_by_season_and_identifier(
                    pool, season_id, &ep_str,
                )
                .await
                {
                    Ok(Some(ep)) => ep.id,
                    Ok(None) => {
                        let air_date =
                            provider_ep
                                .and_then(|e| e.air_date.as_deref())
                                .and_then(|d| {
                                    time::Date::parse(
                                        d,
                                        &time::format_description::well_known::Iso8601::DEFAULT,
                                    )
                                    .ok()
                                });
                        let input = tv_episodes::EpisodeInput {
                            season_id,
                            episode_identifier: ep_str,
                            episode_sort_order: episode_num as i32,
                            title: provider_ep.and_then(|e| e.title.clone()),
                            description: provider_ep.and_then(|e| e.description.clone()),
                            images: None,
                            air_date,
                            air_time: None,
                            runtime: provider_ep.and_then(|e| e.runtime.map(|r| r as i32)),
                            identifiers: serde_json::json!({}),
                            country: None,
                        };
                        match tv_episodes::create(pool, input).await {
                            Ok(id) => id,
                            Err(e) => {
                                warn!(error = %e, "Failed to create episode, falling back to show ID");
                                return Ok((show_id, "tv".to_string()));
                            }
                        }
                    }
                    Err(e) => {
                        warn!(error = %e, "Failed to look up episode, falling back to show ID");
                        return Ok((show_id, "tv".to_string()));
                    }
                };

                info!(show_id = %show_id, episode_id = %episode_id, "Auto-created TV episode for stream");
                Ok((episode_id, "episode".to_string()))
            } else {
                // No episode info — return show ID (fallback, won't track per-episode progress)
                Ok((show_id, "tv".to_string()))
            }
        }
        _ => Err((
            StatusCode::BAD_REQUEST,
            Json(json!({"error": format!("unsupported media_type: {}", body.media_type)})),
        )
            .into_response()),
    }
}
