//! The stream search kobload is asked for a title: built the same way whether a viewer
//! searches, a prewarm picks for them, or the next episode is warmed ahead of time, so
//! each of those hits the caches the others filled.

use axum::Json;
use axum::http::StatusCode;
use axum::response::IntoResponse;
use serde_json::json;
use tracing::error;
use uuid::Uuid;

use crate::AppState;
use crate::debrid_outcomes::{self, SearchPreference};
use crate::repositories::{movies, tv_episodes, tv_seasons, tv_shows};
use crate::sport_want::{EventWant, event_want};
use crate::streaming::decision::search_audio_languages;
use crate::video_metadata::VideoProviderRegistry;

pub enum SearchTarget {
    Movie(Uuid),
    Episode(Uuid),
    /// A Discover title that has no library item yet.
    Discover {
        media_type: String,
        title: String,
        year: Option<i32>,
        imdb_id: Option<String>,
        tmdb_id: Option<String>,
        season: Option<u32>,
        episode: Option<u32>,
    },
}

impl SearchTarget {
    fn library_ids(&self) -> (Option<Uuid>, Option<Uuid>) {
        match self {
            Self::Movie(id) => (Some(*id), None),
            Self::Episode(id) => (None, Some(*id)),
            Self::Discover { .. } => (None, None),
        }
    }
}

pub struct PreparedSearch {
    pub request: nats_jobs::StreamSearchRequest,
    pub preference: SearchPreference,
}

/// `audio_language` overrides the server's preferred audio language for this viewer.
pub async fn prepare(
    state: &AppState,
    user_id: Uuid,
    target: &SearchTarget,
    audio_language: Option<&str>,
) -> Result<PreparedSearch, axum::response::Response> {
    let mut request = match target {
        SearchTarget::Movie(id) => movie_request(state, *id).await?,
        SearchTarget::Episode(id) => episode_request(state, *id).await?,
        SearchTarget::Discover {
            media_type,
            title,
            year,
            imdb_id,
            tmdb_id,
            season,
            episode,
        } => {
            discover_request(
                state,
                media_type,
                title.clone(),
                *year,
                imdb_id.clone(),
                tmdb_id.clone(),
                *season,
                *episode,
            )
            .await?
        }
    };
    let (movie_id, episode_id) = target.library_ids();
    let preference =
        debrid_outcomes::search_preference(&state.pool, user_id, movie_id, episode_id).await;
    request.prefer = preference.prefer.clone().or(request.prefer);
    request.audio_languages = audio_languages(state, audio_language).await;
    Ok(PreparedSearch {
        request,
        preference,
    })
}

pub async fn audio_languages(state: &AppState, audio_language: Option<&str>) -> Vec<String> {
    match audio_language.filter(|l| !l.trim().is_empty()) {
        Some(lang) => search_audio_languages(lang),
        None => search_audio_languages(&state.runtime_config.get().await.preferred_audio_language),
    }
}

/// TV requires an explicit season+episode: show-level searches can never return a
/// specific playable file, so they are rejected rather than downgraded to a movie search.
#[allow(clippy::too_many_arguments)]
async fn discover_request(
    state: &AppState,
    media_type: &str,
    title: String,
    year: Option<i32>,
    imdb_id: Option<String>,
    tmdb_id: Option<String>,
    season: Option<u32>,
    episode: Option<u32>,
) -> Result<nats_jobs::StreamSearchRequest, axum::response::Response> {
    let registry = VideoProviderRegistry::new(state.pool.clone());
    let known = match tmdb_id.as_deref() {
        Some(id) if media_type == "tv" => registry.get_tv_alternate_titles(id, &title).await,
        Some(id) => registry.get_movie_alternate_titles(id, &title).await,
        None => Ok(Vec::new()),
    }
    .unwrap_or_default();
    let alternate_titles = crate::wanted::alternate_titles::alternate_titles(&title, &known);
    let media_want = if media_type == "tv" {
        let (Some(season), Some(episode)) = (season, episode) else {
            return Err((
                StatusCode::BAD_REQUEST,
                Json(json!({
                    "error": "tv stream search requires both 'season' and 'episode' — show-level searches are not playable"
                })),
            )
                .into_response());
        };
        nats_jobs::MediaWant::TvEpisode {
            show_name: title,
            season: season.to_string(),
            episode: episode.to_string(),
            year: year.map(|y| y as u32),
            air_date: None,
            identifiers: Some(nats_jobs::TvIdentifiers {
                tmdb_id,
                tvdb_id: None,
                imdb_id,
            }),
            alternate_titles,
            episode_title: None,
            quality_profile_id: None,
            quality_overrides: None,
        }
    } else {
        nats_jobs::MediaWant::Movie {
            title,
            year: year.map(|y| y as u32),
            release_date: None,
            identifiers: Some(nats_jobs::MovieIdentifiers { tmdb_id, imdb_id }),
            alternate_titles,
            quality_profile_id: None,
            quality_overrides: None,
            in_library: None,
            library_video_height: None,
        }
    };
    Ok(nats_jobs::StreamSearchRequest {
        media_want: Some(media_want),
        query: String::new(),
        media_type: Some(if media_type == "tv" { "tv" } else { "movie" }.to_string()),
        quality_profile_id: None,
        limit: None,
        include_uncached: None,
        prefer: None,
        audio_languages: Vec::new(),
    })
}

async fn movie_request(
    state: &AppState,
    movie_id: Uuid,
) -> Result<nats_jobs::StreamSearchRequest, axum::response::Response> {
    let movie = match movies::get(&state.pool, movie_id).await {
        Ok(Some(m)) => m,
        Ok(None) => {
            return Err((
                StatusCode::NOT_FOUND,
                Json(json!({"error": "movie not found"})),
            )
                .into_response());
        }
        Err(e) => {
            error!(error = %e, "failed to fetch movie");
            return Err((
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({"error": "database error"})),
            )
                .into_response());
        }
    };

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

    Ok(nats_jobs::StreamSearchRequest {
        media_want: Some(media_want),
        query: String::new(),
        media_type: Some("movie".to_string()),
        quality_profile_id: movie.quality_profile_id,
        limit: None,
        include_uncached: None,
        prefer: None,
        audio_languages: Vec::new(),
    })
}

async fn episode_request(
    state: &AppState,
    episode_id: Uuid,
) -> Result<nats_jobs::StreamSearchRequest, axum::response::Response> {
    let episode = match tv_episodes::get(&state.pool, episode_id).await {
        Ok(Some(e)) => e,
        Ok(None) => {
            return Err((
                StatusCode::NOT_FOUND,
                Json(json!({"error": "episode not found"})),
            )
                .into_response());
        }
        Err(e) => {
            error!(error = %e, "failed to fetch episode");
            return Err((
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({"error": "database error"})),
            )
                .into_response());
        }
    };

    let season = match tv_seasons::get(&state.pool, episode.season_id).await {
        Ok(Some(s)) => s,
        Ok(None) => {
            return Err((
                StatusCode::NOT_FOUND,
                Json(json!({"error": "season not found"})),
            )
                .into_response());
        }
        Err(e) => {
            error!(error = %e, "failed to fetch season");
            return Err((
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({"error": "database error"})),
            )
                .into_response());
        }
    };

    let show = match tv_shows::get(&state.pool, season.show_id).await {
        Ok(Some(s)) => s,
        Ok(None) => {
            return Err((
                StatusCode::NOT_FOUND,
                Json(json!({"error": "show not found"})),
            )
                .into_response());
        }
        Err(e) => {
            error!(error = %e, "failed to fetch show");
            return Err((
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({"error": "database error"})),
            )
                .into_response());
        }
    };

    let media_want = if show.content_category == "sport" {
        // Sports leagues store events as episodes, but a bare TV-episode search
        // degrades to the league name ("MotoGP") and matches unrelated releases —
        // the 2026 San Marino race search returned a MotoGP video-game repack.
        // The structured SportEvent want searches "MotoGP San Marino Race" instead,
        // the same shape the want pipeline sends.
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
            season: season.season_identifier,
            episode: episode.episode_identifier,
            year: show.year.map(|y| y as u32),
            air_date: episode.air_date.map(|d| d.to_string()),
            identifiers: Some(identifiers),
            alternate_titles: crate::wanted::alternate_titles::alternate_titles(
                &show.title,
                show.alternate_titles.iter().flatten(),
            ),
            episode_title: episode.title,
            quality_profile_id: show.quality_profile_id,
            quality_overrides: show.preferred_languages.as_ref().map(|langs| {
                nats_jobs::QualityOverrides {
                    preferred_languages: Some(langs.clone()),
                    ..Default::default()
                }
            }),
        }
    };

    Ok(nats_jobs::StreamSearchRequest {
        media_want: Some(media_want),
        query: String::new(),
        media_type: Some("tv".to_string()),
        quality_profile_id: show.quality_profile_id,
        limit: None,
        include_uncached: None,
        prefer: None,
        audio_languages: Vec::new(),
    })
}
