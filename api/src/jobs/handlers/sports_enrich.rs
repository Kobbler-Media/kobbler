//! Per-sports-league enrichment job handler.
//!
//! Fetches full metadata from TheSportsDB for a single league, including
//! seasons and events. Enqueued by create_league and refresh_metadata routes.

use crate::images::ImageEntry;
use crate::jobs::worker::JobHandler;
use crate::jobs::{JobError, JobResult};
use crate::repositories::{tv_episodes, tv_seasons, tv_shows};
use crate::video_metadata::VideoProviderRegistry;
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sqlx::PgPool;
use std::collections::HashMap;
use tokio::time::{Duration, sleep};
use tokio_util::sync::CancellationToken;
use tracing::{info, warn};
use uuid::Uuid;

/// Payload for sports_league_enrich jobs.
#[derive(Debug, Deserialize, Serialize)]
pub struct SportsLeagueEnrichPayload {
    pub league_id: Uuid,
    /// When true, invalidate provider cache before fetching (manual refresh only).
    #[serde(default)]
    pub invalidate_cache: bool,
}

pub struct SportsLeagueEnrichHandler {
    pool: PgPool,
    library_events: tokio::sync::broadcast::Sender<crate::LibraryEvent>,
}

impl SportsLeagueEnrichHandler {
    pub fn new(
        pool: PgPool,
        library_events: tokio::sync::broadcast::Sender<crate::LibraryEvent>,
    ) -> Self {
        Self {
            pool,
            library_events,
        }
    }
}

#[async_trait]
impl JobHandler for SportsLeagueEnrichHandler {
    fn job_type(&self) -> &'static str {
        "sports_league_enrich"
    }

    fn max_concurrent(&self) -> usize {
        2
    }

    async fn execute(
        &self,
        job_id: Uuid,
        payload: Value,
        cancel: CancellationToken,
    ) -> JobResult<Value> {
        let payload: SportsLeagueEnrichPayload = serde_json::from_value(payload)
            .map_err(|e| JobError::ExecutionFailed(format!("Invalid payload: {e}")))?;
        nats_jobs::check_cancelled(&cancel)?;

        info!(job_id = %job_id, league_id = %payload.league_id, "Starting sports league enrichment");

        enrich_league(&self.pool, payload.league_id, payload.invalidate_cache).await?;

        // Broadcast update event for SSE — leagues ARE tv_shows, so reuse tv_show_updated
        let _ = self
            .library_events
            .send(crate::LibraryEvent::tv_show_updated(payload.league_id));

        info!(job_id = %job_id, league_id = %payload.league_id, "Sports league enrichment complete");

        Ok(json!({ "league_id": payload.league_id }))
    }
}

/// Shared sports league enrichment logic used by the job handler.
///
/// Fetches league details from TheSportsDB and populates metadata, seasons, and events.
/// When `invalidate_cache` is true, the provider cache is cleared before fetching
/// (used for manual refresh). Automatic enrichment should pass `false`.
pub async fn enrich_league(
    pool: &PgPool,
    show_id: Uuid,
    invalidate_cache: bool,
) -> Result<(), JobError> {
    let show = tv_shows::get_with_details(pool, show_id)
        .await
        .map_err(|e| JobError::ExecutionFailed(format!("Failed to get league: {e}")))?
        .ok_or_else(|| JobError::ExecutionFailed("League not found".to_string()))?;

    let provider_id = show
        .identifiers
        .get("thesportsdb")
        .and_then(|v| v.as_str())
        .ok_or_else(|| {
            JobError::ExecutionFailed("League has no TheSportsDB identifier".to_string())
        })?
        .to_string();

    let registry = VideoProviderRegistry::new(pool.clone());
    if invalidate_cache {
        registry
            .invalidate_tv_show_detail_cache("thesportsdb", &provider_id)
            .await;
    }
    let details = registry
        .get_tv_show("thesportsdb", &provider_id)
        .await
        .map_err(|e| JobError::retryable(format!("Failed to fetch from TheSportsDB: {e}")))?;

    // Update the league with fresh metadata
    let mut img_map = HashMap::new();
    if let Some(url) = &details.poster_url {
        img_map.insert(
            "poster".to_string(),
            ImageEntry {
                source_url: Some(url.clone()),
                cached_at: None,
            },
        );
    }
    if let Some(url) = &details.backdrop_url {
        img_map.insert(
            "backdrop".to_string(),
            ImageEntry {
                source_url: Some(url.clone()),
                cached_at: None,
            },
        );
    }

    let update = tv_shows::TvShowUpdate {
        title: Some(details.title),
        description: details.description,
        images: if img_map.is_empty() {
            None
        } else {
            Some(img_map)
        },
        status: details.status,
        ..Default::default()
    };

    tv_shows::update(pool, show_id, update)
        .await
        .map_err(|e| JobError::ExecutionFailed(format!("Failed to update league: {e}")))?;

    // Upsert seasons and fetch events from metadata
    let mut seasons_fetched = 0u32;
    for season_summary in &details.seasons {
        let season_id_str = season_summary
            .title
            .clone()
            .unwrap_or_else(|| season_summary.season_number.to_string());

        // Upsert season (creates if new, returns existing ID if already present)
        let season_input = tv_seasons::SeasonInput {
            show_id,
            season_identifier: season_id_str.clone(),
            season_sort_order: season_summary.season_number as i32,
            title: season_summary.title.clone(),
            description: None,
            images: None,
            air_date: None,
            episode_count: season_summary.episode_count.map(|n| n as i32),
            is_monitored: true,
            is_special: false,
            auto_want_mode: "inherit".to_string(),
        };

        let season_id = tv_seasons::upsert(pool, season_input)
            .await
            .map_err(|e| JobError::ExecutionFailed(format!("Failed to upsert season: {e}")))?;

        // Throttle API calls: thesportsdb free tier allows 30 req/min.
        // Sleep 2.5s between each season fetch to stay well under the limit.
        if seasons_fetched > 0 {
            sleep(Duration::from_millis(2500)).await;
        }
        seasons_fetched += 1;

        info!(season = %season_id_str, progress = %format!("{seasons_fetched}/{}", details.seasons.len()), "Enriching season");

        // Fetch and upsert events — always re-fetch to pick up newly added events
        match registry
            .get_season("thesportsdb", &provider_id, season_summary.season_number)
            .await
        {
            Ok(season_detail) => {
                for ep in &season_detail.episodes {
                    // Natural-key identifier for recognised motorsport sessions so two
                    // upstream event IDs for one race collapse to a single row.
                    let identifier = crate::video_metadata::thesportsdb::sport_episode_identifier(
                        ep.episode_number as i32,
                        ep.title.as_deref(),
                        ep.provider_id.as_deref(),
                    );

                    // Skip if episode already exists (idempotent), but backfill country if missing
                    if let Ok(Some(existing)) =
                        tv_episodes::get_by_season_and_identifier(pool, season_id, &identifier)
                            .await
                    {
                        if existing.country.is_none() && ep.country.is_some() {
                            tv_episodes::update_country(pool, existing.id, ep.country.as_deref())
                                .await
                                .ok();
                        }
                        continue;
                    }

                    let air_date = ep.air_date.as_deref().and_then(|d| {
                        time::Date::parse(
                            d,
                            &time::format_description::well_known::Iso8601::DEFAULT,
                        )
                        .ok()
                    });
                    let ep_input = tv_episodes::EpisodeInput {
                        season_id,
                        episode_identifier: identifier,
                        episode_sort_order: ep.episode_number as i32,
                        title: ep.title.clone(),
                        description: ep.description.clone(),
                        images: None,
                        air_date,
                        air_time: None,
                        runtime: None,
                        identifiers: match ep.provider_id.as_ref() {
                            Some(pid) => serde_json::json!({ "thesportsdb_id": pid }),
                            None => serde_json::json!({}),
                        },
                        country: ep.country.clone(),
                    };
                    let _ = tv_episodes::create(pool, ep_input).await;
                }
            }
            Err(crate::video_metadata::VideoProviderError::RateLimited) => {
                // Abort the whole enrichment — continuing will just hit more 429s.
                // The job system will retry later.
                return Err(JobError::retryable(format!(
                    "Rate limited while fetching season {season_id_str} events"
                )));
            }
            Err(e) => {
                warn!(season = %season_id_str, error = %e, "Failed to fetch season events");
            }
        }
    }

    Ok(())
}
