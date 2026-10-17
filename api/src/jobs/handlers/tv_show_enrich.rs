//! Per-TV-show enrichment job handler.
//!
//! Fetches full metadata from TMDB for a single TV show, including seasons, episodes,
//! cast, crew, and images. Enqueued by media_root_match (on auto-add) and
//! media_root_enrich (catch-up).

use crate::images::{self, ImageEntry};
use crate::jobs::worker::JobHandler;
use crate::jobs::{JobContext, JobError, JobResult};
use crate::repositories::{people, tv_episodes, tv_seasons, tv_shows};
use crate::video_metadata::{VideoProviderRegistry, types::VideoCapability};
use async_trait::async_trait;
use nats_jobs::Publisher;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sqlx::PgPool;
use std::collections::HashMap;
use time::Date;
use time::macros::format_description;
use tokio_util::sync::CancellationToken;
use tracing::{debug, info, warn};
use uuid::Uuid;

/// Payload for tv_show_enrich jobs.
#[derive(Debug, Deserialize, Serialize)]
pub struct TvShowEnrichPayload {
    pub show_id: Uuid,
    /// When true, invalidate provider cache before fetching (manual refresh only).
    #[serde(default)]
    pub invalidate_cache: bool,
}

pub struct TvShowEnrichHandler {
    pool: PgPool,
    nats: Publisher,
    storage_root: String,
    library_events: tokio::sync::broadcast::Sender<crate::LibraryEvent>,
    runtime_config: crate::RuntimeConfigManager,
}

impl TvShowEnrichHandler {
    pub fn new(
        pool: PgPool,
        nats: Publisher,
        storage_root: String,
        library_events: tokio::sync::broadcast::Sender<crate::LibraryEvent>,
        runtime_config: crate::RuntimeConfigManager,
    ) -> Self {
        Self {
            pool,
            nats,
            storage_root,
            library_events,
            runtime_config,
        }
    }
}

#[async_trait]
impl JobHandler for TvShowEnrichHandler {
    fn job_type(&self) -> &'static str {
        "tv_show_enrich"
    }

    fn max_concurrent(&self) -> usize {
        self.runtime_config.job_concurrency("tv_show_enrich", 4)
    }

    async fn execute(
        &self,
        job_id: Uuid,
        payload: Value,
        cancel: CancellationToken,
    ) -> JobResult<Value> {
        let payload: TvShowEnrichPayload = serde_json::from_value(payload)
            .map_err(|e| JobError::ExecutionFailed(format!("Invalid payload: {}", e)))?;
        nats_jobs::check_cancelled(&cancel)?;

        info!(job_id = %job_id, show_id = %payload.show_id, "Starting TV show enrichment");

        // Seasons and episodes are committed to Postgres as they are fetched, and a
        // single season the provider will not serve aborts the rest. Indexing only on
        // success is what left shows sitting in the library and absent from search —
        // visible in the UI, invisible to anything reading the index, which is how a
        // request for one season came back offering two others. Say what was written
        // either way; the error still propagates, so the retry is unchanged.
        let enriched = enrich_tv_show(&self.pool, payload.show_id, payload.invalidate_cache).await;
        crate::images::warm_entity_images(
            &self.pool,
            &self.storage_root,
            "tv_shows",
            payload.show_id,
            crate::images::SHOW_ART,
        )
        .await;
        let _ = self
            .library_events
            .send(crate::LibraryEvent::tv_show_updated(payload.show_id));

        // The episodes only exist as of a moment ago. A show asked for before it had any
        // — everything added from Discover, where creation and the want are one press —
        // had nothing to apply that want to when it was set. Runs before the error
        // propagates: seasons are committed as they are fetched, so even a partial
        // enrichment has episodes for the standing want to apply to, and a failed
        // TMDB call does not delay the wants until the hourly sweep.
        match crate::jobs::handlers::auto_want::apply_show_auto_want(
            &self.pool,
            &self.nats,
            payload.show_id,
        )
        .await
        {
            Ok(count) if count > 0 => {
                info!(show_id = %payload.show_id, episodes = count, "Applied the show's standing want after enrichment");
            }
            Err(e) => {
                warn!(show_id = %payload.show_id, error = %e, "Failed to apply auto-want after enrichment");
            }
            _ => {}
        }

        enriched?;

        nats_jobs::check_cancelled(&cancel)?;

        // Mark as enriched so catch-up handler doesn't re-enqueue
        let _ = sqlx::query("UPDATE tv_shows SET enriched_at = NOW() WHERE id = $1")
            .bind(payload.show_id)
            .execute(&self.pool)
            .await;

        // Write sidecars (fast, local file writes)
        crate::sidecar::write_back::write_tvshow_sidecars(&self.pool, payload.show_id).await;

        // Enqueue image download as separate job so metadata appears in UI immediately
        let display = crate::jobs::fetch_job_display(&self.pool, "tv_show", payload.show_id).await;
        let img_context = JobContext::for_entity(
            "tv_show",
            payload.show_id,
            "Downloading TV show images".to_string(),
        )
        .with_display_opt(display);
        let _ = crate::jobs::enqueue(
            &self.pool,
            &self.nats,
            "entity_write_images",
            json!({"entity_type": "tv_show", "entity_id": payload.show_id}),
            img_context,
        )
        .await;

        // Broadcast update event for SSE
        let _ = self
            .library_events
            .send(crate::LibraryEvent::tv_show_updated(payload.show_id));

        info!(job_id = %job_id, show_id = %payload.show_id, "TV show enrichment complete");

        Ok(json!({ "show_id": payload.show_id }))
    }
}

/// Shared TV show enrichment logic used by both the job handler and admin routes.
///
/// Fetches full TMDB details and populates metadata, seasons, episodes, cast, crew, and images.
/// When `invalidate_cache` is true, the provider cache is cleared before fetching
/// (used for manual refresh). Automatic enrichment should pass `false` to reuse
/// cached provider data.
pub async fn enrich_tv_show(
    pool: &PgPool,
    show_id: Uuid,
    invalidate_cache: bool,
) -> Result<(), JobError> {
    // Resolve provider from entity identifiers
    let identifiers: serde_json::Value = sqlx::query_scalar(
        "SELECT identifiers FROM tv_shows WHERE id = $1 AND identifiers IS NOT NULL",
    )
    .bind(show_id)
    .fetch_optional(pool)
    .await
    .map_err(|e| JobError::ExecutionFailed(format!("DB error: {e}")))?
    .ok_or_else(|| JobError::ExecutionFailed("TV show has no identifiers".to_string()))?;

    let registry = VideoProviderRegistry::new(pool.clone());
    let (provider_type, provider_id) = registry
        .resolve_provider_for_identifiers(&identifiers, VideoCapability::TvDetails)
        .await
        .ok_or_else(|| {
            JobError::ExecutionFailed("No enabled provider found for TV show identifiers".into())
        })?;

    if invalidate_cache {
        registry
            .invalidate_tv_show_cache(&provider_type, &provider_id)
            .await;
    }
    let details = registry
        .get_tv_show(&provider_type, &provider_id)
        .await
        .map_err(|e| {
            JobError::retryable(format!("Failed to fetch TV show from {provider_type}: {e}"))
        })?;

    let first_aired = details.first_aired.as_deref().and_then(parse_date);

    // Build images map
    let mut show_img_map = images::image_map_from_media_images(&details.images, Default::default());
    if let Some(url) = &details.poster_url {
        show_img_map
            .entry("poster".to_string())
            .or_insert(ImageEntry {
                source_url: Some(url.clone()),
                cached_at: None,
            });
    }
    if let Some(url) = &details.backdrop_url {
        show_img_map
            .entry("backdrop".to_string())
            .or_insert(ImageEntry {
                source_url: Some(url.clone()),
                cached_at: None,
            });
    }
    if let Some(url) = &details.logo_url {
        show_img_map
            .entry("logo".to_string())
            .or_insert(ImageEntry {
                source_url: Some(url.clone()),
                cached_at: None,
            });
    }

    // Fetch alternate titles from TMDB (not on the generic trait)
    let alternate_titles = if provider_type == "tmdb" {
        match registry
            .get_tv_alternate_titles(&provider_id, &details.title)
            .await
        {
            Ok(alts) if !alts.is_empty() => Some(alts),
            Ok(_) => None,
            Err(e) => {
                debug!("Failed to fetch TV alternate titles: {e}");
                None
            }
        }
    } else {
        None
    };

    let update = tv_shows::TvShowUpdate {
        title: Some(details.title),
        year: details.year.map(|y| y as i32),
        status: details.status,
        description: details.description,
        images: if show_img_map.is_empty() {
            None
        } else {
            Some(show_img_map)
        },
        identifiers: Some(serde_json::to_value(&details.identifiers).unwrap_or_default()),
        genres: Some(details.genres),
        rating: details.rating,
        rating_count: details.rating_count,
        content_rating: details.content_rating,
        network: details.network,
        first_aired,
        runtime: details.runtime.map(|r| r as i32),
        alternate_titles,
        ..Default::default()
    };

    tv_shows::update(pool, show_id, update)
        .await
        .map_err(|e| JobError::ExecutionFailed(format!("Failed to update show: {e}")))?;

    // Upsert seasons and episodes from TMDB
    for season_summary in &details.seasons {
        let season_air_date = season_summary.air_date.as_deref().and_then(parse_date);
        let is_special = season_summary.season_number == 0;

        let season_images = season_summary.poster_url.as_ref().map(|url| {
            let mut m = HashMap::new();
            m.insert(
                "poster".to_string(),
                ImageEntry {
                    source_url: Some(url.clone()),
                    cached_at: None,
                },
            );
            m
        });

        let season_input = tv_seasons::SeasonInput {
            show_id,
            season_identifier: season_summary.season_number.to_string(),
            season_sort_order: season_summary.season_number as i32,
            title: season_summary.title.clone(),
            description: None,
            images: season_images,
            air_date: season_air_date,
            episode_count: season_summary.episode_count.map(|c| c as i32),
            is_monitored: !is_special,
            is_special,
            auto_want_mode: "inherit".to_string(),
        };

        let season_id = tv_seasons::upsert(pool, season_input)
            .await
            .map_err(|e| JobError::ExecutionFailed(format!("Failed to upsert season: {e}")))?;

        // Fetch season details for episode data
        let season_details = registry
            .get_season_merged(
                &provider_type,
                &provider_id,
                season_summary.season_number,
                &identifiers,
            )
            .await
            .map_err(|e| {
                JobError::retryable(format!(
                    "Failed to fetch season {} from {provider_type}: {e}",
                    season_summary.season_number
                ))
            })?;

        let episode_inputs: Vec<tv_episodes::EpisodeInput> = season_details
            .episodes
            .iter()
            .map(|ep| {
                let ep_images = ep.still_url.as_ref().map(|url| {
                    let mut m = HashMap::new();
                    m.insert(
                        "still".to_string(),
                        ImageEntry {
                            source_url: Some(url.clone()),
                            cached_at: None,
                        },
                    );
                    m
                });
                tv_episodes::EpisodeInput {
                    season_id,
                    episode_identifier: ep.episode_number.to_string(),
                    episode_sort_order: ep.episode_number as i32,
                    title: ep.title.clone(),
                    description: ep.description.clone(),
                    images: ep_images,
                    air_date: ep.air_date.as_deref().and_then(parse_date),
                    air_time: ep
                        .air_time
                        .as_deref()
                        .and_then(crate::video_metadata::parse_air_time),
                    runtime: ep.runtime.map(|r| r as i32),
                    identifiers: match ep.provider_id.as_ref() {
                        Some(pid) => serde_json::json!({ format!("{provider_type}_id"): pid }),
                        None => serde_json::json!({}),
                    },
                    country: None,
                }
            })
            .collect();

        tv_episodes::bulk_upsert(pool, season_id, episode_inputs)
            .await
            .map_err(|e| {
                JobError::ExecutionFailed(format!("Failed to bulk upsert episodes: {e}"))
            })?;
    }

    // Upsert people and replace cast
    let mut cast_inputs = Vec::new();
    for member in &details.cast {
        let ext_id = match &member.external_id {
            Some(eid) => eid.clone(),
            None => continue,
        };
        let person_id = people::upsert_by_provider(
            pool,
            &provider_type,
            &ext_id,
            people::PersonInput {
                name: member.name.clone(),
                images: member.profile_url.as_ref().map(|url| {
                    let mut m = HashMap::new();
                    m.insert(
                        "profile".to_string(),
                        ImageEntry {
                            source_url: Some(url.clone()),
                            cached_at: None,
                        },
                    );
                    m
                }),
                ..Default::default()
            },
        )
        .await
        .map_err(|e| JobError::ExecutionFailed(format!("Failed to upsert person: {e}")))?;

        cast_inputs.push(tv_shows::CastInput {
            person_id,
            character_name: member.character.clone(),
            cast_order: member.order,
        });
    }
    tv_shows::replace_cast(pool, show_id, cast_inputs)
        .await
        .map_err(|e| JobError::ExecutionFailed(format!("Failed to replace cast: {e}")))?;

    // Upsert people and replace crew
    let mut crew_inputs = Vec::new();
    for member in &details.crew {
        let ext_id = match &member.external_id {
            Some(eid) => eid.clone(),
            None => continue,
        };
        let person_id = people::upsert_by_provider(
            pool,
            &provider_type,
            &ext_id,
            people::PersonInput {
                name: member.name.clone(),
                images: member.profile_url.as_ref().map(|url| {
                    let mut m = HashMap::new();
                    m.insert(
                        "profile".to_string(),
                        ImageEntry {
                            source_url: Some(url.clone()),
                            cached_at: None,
                        },
                    );
                    m
                }),
                ..Default::default()
            },
        )
        .await
        .map_err(|e| JobError::ExecutionFailed(format!("Failed to upsert person: {e}")))?;

        crew_inputs.push(tv_shows::CrewInput {
            person_id,
            department: member.department.clone(),
            job: member.job.clone(),
        });
    }
    tv_shows::replace_crew(pool, show_id, crew_inputs)
        .await
        .map_err(|e| JobError::ExecutionFailed(format!("Failed to replace crew: {e}")))?;

    debug!(show_id = %show_id, "TV show enrichment: metadata, seasons, episodes, cast, and crew updated");

    Ok(())
}

fn parse_date(s: &str) -> Option<Date> {
    let format = format_description!("[year]-[month]-[day]");
    Date::parse(s, &format).ok()
}
