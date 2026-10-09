//! Per-episode credits.
//!
//! Series regulars answer from the show-level `tv_cast` that enrichment already
//! populates. Guest stars do not exist there at all, so they are fetched from
//! the metadata provider on first ask and cached in `tv_episode_cast` —
//! deliberately lazy, because hammering TMDB for every episode nobody asks
//! about is the thing to avoid.

use axum::{
    Json,
    extract::{Path, State},
    http::StatusCode,
    response::IntoResponse,
};
use serde::Serialize;
use serde_json::{Value, json};
use std::collections::{HashMap, HashSet};
use tracing::warn;
use uuid::Uuid;

use crate::AppState;
use crate::images::ImageEntry;
use crate::repositories::{people, tv_episode_cast, tv_episodes, tv_seasons, tv_shows};
use crate::routes::AuthUser;
use crate::video_metadata::{VideoProviderRegistry, types::VideoCapability};

#[derive(Debug, Serialize)]
pub struct EpisodeCastMember {
    pub person_id: Uuid,
    pub name: String,
    pub character: Option<String>,
    pub image_url: Option<String>,
    pub cast_order: Option<i32>,
}

#[derive(Debug, Serialize)]
pub struct EpisodeCrewMember {
    pub person_id: Uuid,
    pub name: String,
    pub department: Option<String>,
    pub job: Option<String>,
    pub image_url: Option<String>,
}

/// Where the guest list came from, so a caller can tell "nobody guest-starred"
/// from "we could not find out".
#[derive(Debug, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum GuestSource {
    /// Read from `tv_episode_cast`.
    Cache,
    /// Fetched from the metadata provider on this request and cached.
    Provider,
    /// No provider could answer, so the guest list is unknown, not empty.
    Unavailable,
}

#[derive(Debug, Serialize)]
pub struct EpisodeCastResponse {
    pub episode_id: Uuid,
    pub show_id: Uuid,
    pub show_title: String,
    pub season_identifier: String,
    pub episode_identifier: String,
    pub episode_title: Option<String>,
    pub guest_source: GuestSource,
    /// Show-level regulars — these answer "who plays Saul" and are always
    /// available for an enriched show.
    pub series_regulars: Vec<EpisodeCastMember>,
    /// People credited on this episode only. A regular never appears here.
    pub guest_stars: Vec<EpisodeCastMember>,
    pub crew: Vec<EpisodeCrewMember>,
}

fn err(status: StatusCode, msg: String) -> (StatusCode, Json<Value>) {
    (status, Json(json!({ "error": msg })))
}

/// GET /api/v1/admin/tv/episodes/{id}/cast
pub async fn get_episode_cast(
    State(state): State<AppState>,
    _auth: AuthUser,
    Path(episode_id): Path<Uuid>,
) -> impl IntoResponse {
    let pool = &state.pool;

    let episode = match tv_episodes::get(pool, episode_id).await {
        Ok(Some(e)) => e,
        Ok(None) => return err(StatusCode::NOT_FOUND, "Episode not found".into()).into_response(),
        Err(e) => {
            return err(
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("Failed to load episode: {e}"),
            )
            .into_response();
        }
    };

    let season = match tv_seasons::get(pool, episode.season_id).await {
        Ok(Some(s)) => s,
        Ok(None) => {
            return err(StatusCode::NOT_FOUND, "Season not found for episode".into())
                .into_response();
        }
        Err(e) => {
            return err(
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("Failed to load season: {e}"),
            )
            .into_response();
        }
    };

    let show = match tv_shows::get(pool, season.show_id).await {
        Ok(Some(s)) => s,
        Ok(None) => {
            return err(StatusCode::NOT_FOUND, "Show not found for episode".into()).into_response();
        }
        Err(e) => {
            return err(
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("Failed to load show: {e}"),
            )
            .into_response();
        }
    };

    let series_regulars: Vec<EpisodeCastMember> = match tv_shows::get_cast(pool, show.id).await {
        Ok(rows) => rows
            .into_iter()
            .map(|c| EpisodeCastMember {
                person_id: c.person_id,
                name: c.person_name,
                character: c.character_name,
                image_url: c.person_image_url,
                cast_order: c.cast_order,
            })
            .collect(),
        Err(e) => {
            return err(
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("Failed to load show cast: {e}"),
            )
            .into_response();
        }
    };

    let fetched_at = tv_episode_cast::fetched_at(pool, episode_id)
        .await
        .unwrap_or(None);

    let mut guest_source = if fetched_at.is_some() {
        GuestSource::Cache
    } else {
        GuestSource::Unavailable
    };

    if fetched_at.is_none()
        && let Some(numbers) =
            episode_numbers(&season.season_identifier, &episode.episode_identifier)
        && fetch_and_cache(&state, &show, episode_id, numbers).await
    {
        guest_source = GuestSource::Provider;
    }

    let credits = match tv_episode_cast::list_for_episode(pool, episode_id).await {
        Ok(rows) => rows,
        Err(e) => {
            return err(
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("Failed to load episode credits: {e}"),
            )
            .into_response();
        }
    };

    let regular_ids: HashSet<Uuid> = series_regulars.iter().map(|c| c.person_id).collect();

    let mut guest_stars = Vec::new();
    let mut crew = Vec::new();
    for c in credits {
        match c.credit_type.as_str() {
            tv_episode_cast::CREDIT_CREW => crew.push(EpisodeCrewMember {
                person_id: c.person_id,
                name: c.person_name,
                department: c.department,
                job: c.job,
                image_url: c.person_image_url,
            }),
            // A provider occasionally lists a regular among the episode's
            // credits; the whole point of this endpoint is the distinction, so
            // the regular stays a regular.
            _ if regular_ids.contains(&c.person_id) => {}
            _ => guest_stars.push(EpisodeCastMember {
                person_id: c.person_id,
                name: c.person_name,
                character: c.character_name,
                image_url: c.person_image_url,
                cast_order: c.cast_order,
            }),
        }
    }

    Json(EpisodeCastResponse {
        episode_id,
        show_id: show.id,
        show_title: show.title,
        season_identifier: season.season_identifier,
        episode_identifier: episode.episode_identifier,
        episode_title: episode.title,
        guest_source,
        series_regulars,
        guest_stars,
        crew,
    })
    .into_response()
}

fn episode_numbers(season_identifier: &str, episode_identifier: &str) -> Option<(u32, u32)> {
    Some((
        season_identifier.parse().ok()?,
        episode_identifier.parse().ok()?,
    ))
}

/// Fetch credits from the provider and cache them. Returns false when no
/// provider could answer, which must stay distinguishable from "no guests".
async fn fetch_and_cache(
    state: &AppState,
    show: &tv_shows::TvShowRow,
    episode_id: Uuid,
    (season, episode): (u32, u32),
) -> bool {
    let pool = &state.pool;
    let registry = VideoProviderRegistry::new(pool.clone());

    let Some((provider_type, provider_id)) = registry
        .resolve_provider_for_identifiers(&show.identifiers, VideoCapability::EpisodeCredits)
        .await
    else {
        return false;
    };

    // TMDB puts guest stars on every episode of the season payload, which
    // enrichment already fetched and cached — so the common ask costs no API
    // call at all. The per-episode endpoint is only for providers or cache
    // states where that is not true.
    let credits =
        match season_credits(&registry, &provider_type, &provider_id, season, episode).await {
            Some(c) => c,
            None => match registry
                .get_episode_credits(&provider_type, &provider_id, season, episode)
                .await
            {
                Ok(c) => c,
                Err(e) => {
                    warn!(show_id = %show.id, error = %e, "Failed to fetch episode credits");
                    return false;
                }
            },
        };

    let mut inputs = Vec::new();
    for member in &credits.guest_stars {
        let Some(ext_id) = member.external_id.as_deref() else {
            continue;
        };
        match upsert_person(
            pool,
            &provider_type,
            ext_id,
            &member.name,
            member.profile_url.as_deref(),
        )
        .await
        {
            Ok(person_id) => inputs.push(tv_episode_cast::EpisodeCreditInput {
                person_id,
                credit_type: tv_episode_cast::CREDIT_GUEST.to_string(),
                character_name: member.character.clone(),
                cast_order: member.order,
                department: None,
                job: None,
            }),
            Err(e) => warn!(error = %e, "Failed to upsert guest star"),
        }
    }
    for member in &credits.crew {
        let Some(ext_id) = member.external_id.as_deref() else {
            continue;
        };
        match upsert_person(
            pool,
            &provider_type,
            ext_id,
            &member.name,
            member.profile_url.as_deref(),
        )
        .await
        {
            Ok(person_id) => inputs.push(tv_episode_cast::EpisodeCreditInput {
                person_id,
                credit_type: tv_episode_cast::CREDIT_CREW.to_string(),
                character_name: None,
                cast_order: None,
                department: Some(member.department.clone()),
                job: Some(member.job.clone()),
            }),
            Err(e) => warn!(error = %e, "Failed to upsert episode crew member"),
        }
    }

    if let Err(e) = tv_episode_cast::replace_for_episode(pool, episode_id, inputs).await {
        warn!(error = %e, "Failed to cache episode credits");
        return false;
    }
    true
}

async fn season_credits(
    registry: &VideoProviderRegistry,
    provider_type: &str,
    provider_id: &str,
    season: u32,
    episode: u32,
) -> Option<crate::video_metadata::EpisodeCredits> {
    let details = registry
        .get_season(provider_type, provider_id, season)
        .await
        .ok()?;
    details
        .episodes
        .into_iter()
        .find(|e| e.episode_number == episode)?
        .credits
}

async fn upsert_person(
    pool: &sqlx::PgPool,
    provider_type: &str,
    external_id: &str,
    name: &str,
    profile_url: Option<&str>,
) -> crate::repositories::Result<Uuid> {
    people::upsert_by_provider(
        pool,
        provider_type,
        external_id,
        people::PersonInput {
            name: name.to_string(),
            images: profile_url.map(|url| {
                let mut m = HashMap::new();
                m.insert(
                    "profile".to_string(),
                    ImageEntry {
                        source_url: Some(url.to_string()),
                        cached_at: None,
                    },
                );
                m
            }),
            ..Default::default()
        },
    )
    .await
}
