//! Per-movie enrichment job handler.
//!
//! Fetches full metadata from TMDB for a single movie, including cast, crew, and images.
//! Enqueued by media_root_match (on auto-add) and media_root_enrich (catch-up).

use crate::images::{self, ImageEntry};
use crate::jobs::worker::JobHandler;
use crate::jobs::{JobContext, JobError, JobResult};
use crate::repositories::{movies, people};
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
use tracing::{debug, info};
use uuid::Uuid;

/// Payload for movie_enrich jobs.
#[derive(Debug, Deserialize, Serialize)]
pub struct MovieEnrichPayload {
    pub movie_id: Uuid,
}

pub struct MovieEnrichHandler {
    pool: PgPool,
    nats: Publisher,
    storage_root: String,
    library_events: tokio::sync::broadcast::Sender<crate::LibraryEvent>,
    runtime_config: crate::RuntimeConfigManager,
}

impl MovieEnrichHandler {
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
impl JobHandler for MovieEnrichHandler {
    fn job_type(&self) -> &'static str {
        "movie_enrich"
    }

    fn max_concurrent(&self) -> usize {
        self.runtime_config.job_concurrency("movie_enrich", 4)
    }

    async fn execute(
        &self,
        job_id: Uuid,
        payload: Value,
        cancel: CancellationToken,
    ) -> JobResult<Value> {
        let payload: MovieEnrichPayload = serde_json::from_value(payload)
            .map_err(|e| JobError::ExecutionFailed(format!("Invalid payload: {}", e)))?;
        nats_jobs::check_cancelled(&cancel)?;

        info!(job_id = %job_id, movie_id = %payload.movie_id, "Starting movie enrichment");

        enrich_movie(&self.pool, payload.movie_id, false).await?;
        images::warm_entity_images(
            &self.pool,
            &self.storage_root,
            "movies",
            payload.movie_id,
            images::MOVIE_ART,
        )
        .await;

        nats_jobs::check_cancelled(&cancel)?;

        // Mark as enriched so catch-up handler doesn't re-enqueue
        let _ = sqlx::query("UPDATE movies SET enriched_at = NOW() WHERE id = $1")
            .bind(payload.movie_id)
            .execute(&self.pool)
            .await;

        // Write sidecars (fast, local file writes)
        crate::sidecar::write_back::write_movie_sidecars(&self.pool, payload.movie_id).await;

        // Enqueue image download as separate job so metadata appears in UI immediately
        let display = crate::jobs::fetch_job_display(&self.pool, "movie", payload.movie_id).await;
        let img_context = JobContext::for_entity(
            "movie",
            payload.movie_id,
            "Downloading movie images".to_string(),
        )
        .with_display_opt(display);
        let _ = crate::jobs::enqueue(
            &self.pool,
            &self.nats,
            "entity_write_images",
            json!({"entity_type": "movie", "entity_id": payload.movie_id}),
            img_context,
        )
        .await;

        // Broadcast update event for SSE
        let _ = self
            .library_events
            .send(crate::LibraryEvent::movie_updated(payload.movie_id));

        info!(job_id = %job_id, movie_id = %payload.movie_id, "Movie enrichment complete");

        Ok(json!({ "movie_id": payload.movie_id }))
    }
}

/// Shared movie enrichment logic used by both the job handler and admin routes.
///
/// Fetches full TMDB details and populates metadata, cast, crew, and images.
/// When `invalidate_cache` is true, the provider cache is cleared before fetching
/// (used for manual refresh). Automatic enrichment should pass `false` to reuse
/// cached provider data.
pub async fn enrich_movie(
    pool: &PgPool,
    movie_id: Uuid,
    invalidate_cache: bool,
) -> Result<(), JobError> {
    // Resolve provider from entity identifiers
    let identifiers: serde_json::Value = sqlx::query_scalar(
        "SELECT identifiers FROM movies WHERE id = $1 AND identifiers IS NOT NULL",
    )
    .bind(movie_id)
    .fetch_optional(pool)
    .await
    .map_err(|e| JobError::ExecutionFailed(format!("DB error: {e}")))?
    .ok_or_else(|| JobError::ExecutionFailed("Movie has no identifiers".to_string()))?;

    let registry = VideoProviderRegistry::new(pool.clone());
    let (provider_type, provider_id) = registry
        .resolve_provider_for_identifiers(&identifiers, VideoCapability::MovieDetails)
        .await
        .ok_or_else(|| {
            JobError::ExecutionFailed("No enabled provider found for movie identifiers".into())
        })?;

    if invalidate_cache {
        registry
            .invalidate_movie_cache(&provider_type, &provider_id)
            .await;
    }
    let details = registry
        .get_movie(&provider_type, &provider_id)
        .await
        .map_err(|e| {
            JobError::retryable(format!("Failed to fetch movie from {provider_type}: {e}"))
        })?;

    let release_date = details.release_date.as_deref().and_then(parse_date);

    // Build images map
    let mut img_map = images::image_map_from_media_images(&details.images, Default::default());
    if let Some(url) = details.poster_url {
        img_map.entry("poster".to_string()).or_insert(ImageEntry {
            source_url: Some(url),
            cached_at: None,
        });
    }
    if let Some(url) = details.backdrop_url {
        img_map.entry("backdrop".to_string()).or_insert(ImageEntry {
            source_url: Some(url),
            cached_at: None,
        });
    }
    if let Some(url) = details.logo_url {
        img_map.entry("logo".to_string()).or_insert(ImageEntry {
            source_url: Some(url),
            cached_at: None,
        });
    }

    // Fetch alternate titles from TMDB (not on the generic trait)
    let alternate_titles = if provider_type == "tmdb" {
        match registry
            .get_movie_alternate_titles(&provider_id, &details.title)
            .await
        {
            Ok(alts) if !alts.is_empty() => Some(alts),
            Ok(_) => None,
            Err(e) => {
                debug!("Failed to fetch movie alternate titles: {e}");
                None
            }
        }
    } else {
        None
    };

    let update = movies::MovieUpdate {
        title: Some(details.title),
        original_title: details.original_title,
        year: details.year.map(|y| y as i32),
        runtime: details.runtime.map(|r| r as i32),
        description: details.description,
        tagline: details.tagline,
        images: if img_map.is_empty() {
            None
        } else {
            Some(img_map)
        },
        identifiers: Some(serde_json::to_value(&details.identifiers).unwrap_or_default()),
        genres: Some(details.genres),
        rating: details.rating,
        rating_count: details.rating_count,
        content_rating: details.content_rating,
        release_date,
        alternate_titles,
        ..Default::default()
    };

    movies::update(pool, movie_id, update)
        .await
        .map_err(|e| JobError::ExecutionFailed(format!("Failed to update movie: {e}")))?;

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

        cast_inputs.push(movies::CastInput {
            person_id,
            character_name: member.character.clone(),
            cast_order: member.order,
        });
    }
    movies::replace_cast(pool, movie_id, cast_inputs)
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

        crew_inputs.push(movies::CrewInput {
            person_id,
            department: member.department.clone(),
            job: member.job.clone(),
        });
    }
    movies::replace_crew(pool, movie_id, crew_inputs)
        .await
        .map_err(|e| JobError::ExecutionFailed(format!("Failed to replace crew: {e}")))?;

    debug!(movie_id = %movie_id, "Movie enrichment: metadata, cast, and crew updated");

    Ok(())
}

fn parse_date(s: &str) -> Option<Date> {
    let format = format_description!("[year]-[month]-[day]");
    Date::parse(s, &format).ok()
}
