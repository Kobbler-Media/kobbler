//! Migration episode reconciliation job handler.
//!
//! After a TV show is enriched from TMDB (which creates seasons and episodes),
//! this job fetches episode data from Sonarr and marks episodes that have files
//! as in-library. It also wants missing episodes that Sonarr has monitored,
//! and enqueues `tv_want` jobs for the affected seasons.

use crate::jobs::handlers::auto_want;
use crate::jobs::worker::JobHandler;
use crate::jobs::{JobError, JobResult};
use crate::repositories::{tv_seasons, tv_shows};
use async_trait::async_trait;
use nats_jobs::Publisher;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sqlx::PgPool;
use std::collections::{HashMap, HashSet};
use tokio_util::sync::CancellationToken;
use tracing::{info, warn};
use uuid::Uuid;

#[derive(Debug, Deserialize, Serialize)]
pub struct MigrationEpisodeReconcilePayload {
    pub show_id: Uuid,
    pub sonarr_url: String,
    pub sonarr_api_key: String,
    pub sonarr_series_id: i64,
    #[serde(default)]
    pub want_missing: Option<bool>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SonarrEpisode {
    episode_number: i32,
    season_number: i32,
    has_file: bool,
    monitored: bool,
}

pub struct MigrationEpisodeReconcileHandler {
    pool: PgPool,
    publisher: Publisher,
}

impl MigrationEpisodeReconcileHandler {
    pub fn new(pool: PgPool, publisher: Publisher) -> Self {
        Self { pool, publisher }
    }
}

#[async_trait]
impl JobHandler for MigrationEpisodeReconcileHandler {
    fn job_type(&self) -> &'static str {
        "migration_episode_reconcile"
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
        let payload: MigrationEpisodeReconcilePayload = serde_json::from_value(payload)
            .map_err(|e| JobError::ExecutionFailed(format!("Invalid payload: {e}")))?;
        nats_jobs::check_cancelled(&cancel)?;

        info!(
            job_id = %job_id,
            show_id = %payload.show_id,
            sonarr_series_id = payload.sonarr_series_id,
            "Reconciling episodes from Sonarr"
        );

        // Fetch episodes from Sonarr
        let client = crate::http::new_client();
        let url = format!(
            "{}/api/v3/episode?seriesId={}",
            payload.sonarr_url.trim_end_matches('/'),
            payload.sonarr_series_id
        );
        let sonarr_episodes: Vec<SonarrEpisode> = client
            .get(&url)
            .header("X-Api-Key", &payload.sonarr_api_key)
            .send()
            .await
            .map_err(|e| JobError::retryable(format!("Failed to fetch Sonarr episodes: {e}")))?
            .json()
            .await
            .map_err(|e| JobError::retryable(format!("Failed to parse Sonarr episodes: {e}")))?;

        // Get all Kobbler episodes for this show with season/episode sort orders
        let kobbler_episodes: Vec<KobblerEpisodeRow> = sqlx::query_as(
            r#"
            SELECT e.id, e.episode_sort_order, s.season_sort_order
            FROM tv_episodes e
            JOIN tv_seasons s ON s.id = e.season_id
            WHERE s.show_id = $1
            "#,
        )
        .bind(payload.show_id)
        .fetch_all(&self.pool)
        .await
        .map_err(|e| JobError::ExecutionFailed(format!("Failed to fetch Kobbler episodes: {e}")))?;

        // If enrichment hasn't created episodes yet, retry later
        if kobbler_episodes.is_empty() && !sonarr_episodes.is_empty() {
            return Err(JobError::retryable(format!(
                "Show {} has {} Sonarr episodes but 0 Kobbler episodes — enrichment likely still running",
                payload.show_id,
                sonarr_episodes.len()
            )));
        }

        // Build a map: (season_number, episode_number) -> episode_id
        let mut kobbler_map: std::collections::HashMap<(i32, i32), Uuid> =
            std::collections::HashMap::new();
        for ep in &kobbler_episodes {
            kobbler_map.insert((ep.season_sort_order, ep.episode_sort_order), ep.id);
        }

        // Collect episode IDs that Sonarr says have files
        let in_library_ids: HashSet<Uuid> = sonarr_episodes
            .iter()
            .filter(|ep| ep.has_file)
            .filter_map(|ep| {
                kobbler_map
                    .get(&(ep.season_number, ep.episode_number))
                    .copied()
            })
            .collect();

        let episode_ids_to_mark: Vec<Uuid> = in_library_ids.iter().copied().collect();
        let marked_count = episode_ids_to_mark.len();

        if !episode_ids_to_mark.is_empty() {
            // Set want_status='none' for matched episodes (is_in_library is derived from file existence)
            if let Err(e) = sqlx::query(
                "UPDATE tv_episodes SET want_status = 'none', updated_at = NOW() WHERE id = ANY($1)",
            )
            .bind(&episode_ids_to_mark)
            .execute(&self.pool)
            .await
            {
                warn!(error = %e, "Failed to batch-update episode library status");
            }
        }

        // Want missing episodes: monitored in Sonarr, no file, matching Kobbler episode exists
        let mut wanted_count = 0usize;
        if payload.want_missing != Some(false) {
            let want_ids: Vec<Uuid> = sonarr_episodes
                .iter()
                .filter(|ep| ep.monitored && !ep.has_file)
                .filter_map(|ep| {
                    kobbler_map
                        .get(&(ep.season_number, ep.episode_number))
                        .copied()
                })
                .filter(|id| !in_library_ids.contains(id))
                .collect();

            wanted_count = want_ids.len();

            if !want_ids.is_empty() {
                if let Err(e) = sqlx::query(
                    "UPDATE tv_episodes SET want_status = 'wanted', updated_at = NOW() WHERE id = ANY($1) AND want_status = 'none'",
                )
                .bind(&want_ids)
                .execute(&self.pool)
                .await
                {
                    warn!(error = %e, "Failed to batch-update episode want status");
                }

                // Build season_number -> season_id map from Kobbler
                let season_rows: Vec<SeasonIdRow> = sqlx::query_as(
                    "SELECT id, season_sort_order FROM tv_seasons WHERE show_id = $1",
                )
                .bind(payload.show_id)
                .fetch_all(&self.pool)
                .await
                .unwrap_or_default();

                let season_map: HashMap<i32, Uuid> = season_rows
                    .into_iter()
                    .map(|r| (r.season_sort_order, r.id))
                    .collect();

                // Collect unique season numbers that have missing wanted episodes
                let want_season_numbers: HashSet<i32> = sonarr_episodes
                    .iter()
                    .filter(|ep| ep.monitored && !ep.has_file)
                    .filter_map(|ep| {
                        kobbler_map
                            .get(&(ep.season_number, ep.episode_number))
                            .map(|_| ep.season_number)
                    })
                    .collect();

                // Enqueue tv_want jobs per season
                for season_number in &want_season_numbers {
                    if let Some(&season_id) = season_map.get(season_number) {
                        // Collect wanted episode IDs for this season
                        let season_episode_ids: Vec<Uuid> = sonarr_episodes
                            .iter()
                            .filter(|ep| {
                                ep.season_number == *season_number && ep.monitored && !ep.has_file
                            })
                            .filter_map(|ep| {
                                kobbler_map
                                    .get(&(ep.season_number, ep.episode_number))
                                    .copied()
                            })
                            .collect();

                        if let Err(e) = self
                            .publisher
                            .enqueue_job(
                                "kobbler",
                                "tv_want",
                                json!({
                                    "show_id": payload.show_id,
                                    "season_id": season_id,
                                    "episode_ids": season_episode_ids,
                                }),
                            )
                            .await
                        {
                            warn!(
                                error = %e,
                                season_number,
                                "Failed to enqueue tv_want job for season"
                            );
                        }
                    }
                }
            }
        }

        // Apply auto_want_mode for the show (handles new_only/all/fill_gaps)
        let mut auto_wanted = 0u32;
        if let Ok(Some(show)) = tv_shows::get(&self.pool, payload.show_id).await {
            let show_mode = show.auto_want_mode.as_str();
            if show_mode != "none"
                && let Ok(seasons) = tv_seasons::list_by_show(&self.pool, payload.show_id).await
            {
                for season in &seasons {
                    let effective_mode = auto_want::resolve_effective_mode(season, show_mode);
                    match auto_want::apply_auto_want(
                        &self.pool,
                        &self.publisher,
                        payload.show_id,
                        season.id,
                        effective_mode,
                    )
                    .await
                    {
                        Ok(count) => auto_wanted += count,
                        Err(e) => warn!(
                            show_id = %payload.show_id,
                            season_id = %season.id,
                            error = %e,
                            "Failed to apply auto_want after reconciliation"
                        ),
                    }
                }
            }
        }

        info!(
            show_id = %payload.show_id,
            sonarr_episodes = sonarr_episodes.len(),
            kobbler_episodes = kobbler_episodes.len(),
            marked_in_library = marked_count,
            marked_wanted = wanted_count,
            auto_wanted,
            "Episode reconciliation complete"
        );

        Ok(json!({
            "show_id": payload.show_id,
            "sonarr_episodes": sonarr_episodes.len(),
            "kobbler_episodes": kobbler_episodes.len(),
            "marked_in_library": marked_count,
            "marked_wanted": wanted_count,
            "auto_wanted": auto_wanted,
        }))
    }
}

#[derive(Debug, sqlx::FromRow)]
struct KobblerEpisodeRow {
    id: Uuid,
    episode_sort_order: i32,
    season_sort_order: i32,
}

#[derive(Debug, sqlx::FromRow)]
struct SeasonIdRow {
    id: Uuid,
    season_sort_order: i32,
}
