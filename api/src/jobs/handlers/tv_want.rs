//! TV want job handler - enqueues want_request jobs to Kobload via NATS.
//!
//! This job is enqueued when a user wants specific TV episodes or an entire season.
//! It builds MediaWant::TvEpisode or MediaWant::TvSeason and enqueues want_request
//! jobs to Kobload. Responses come back via events.want.response.kobbler subscriber.

use crate::LibraryEvent;
use crate::file_link_mode::FileLinkMode;
use crate::jobs::handlers::auto_want;
use crate::jobs::worker::JobHandler;
use crate::jobs::{JobError, JobResult};
use crate::repositories::{tv_episode_files, tv_episodes, tv_seasons, tv_shows};
use crate::runtime_config::RuntimeConfigManager;
use async_trait::async_trait;
use nats_jobs::Publisher;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sqlx::PgPool;
use std::collections::HashMap;
use tokio::sync::broadcast;
use tokio_util::sync::CancellationToken;
use tracing::{info, warn};
use uuid::Uuid;

use super::want_helpers::build_excluded_releases;

/// Payload for tv_want jobs.
#[derive(Debug, Deserialize, Serialize)]
pub struct TvWantPayload {
    pub show_id: Uuid,
    pub season_id: Uuid,
    pub episode_ids: Option<Vec<Uuid>>,
    /// Who asked, when a person did. Schedulers leave this unset.
    #[serde(default)]
    pub user_id: Option<Uuid>,
    /// A regrab: the resulting file joins whatever is already there instead of
    /// replacing it. Everything else — schedulers, RSS, the UI — leaves this false.
    #[serde(default)]
    pub additional_copy: bool,
    /// A person asked for this episode again right now. Kobload reopens an
    /// already-imported episode instead of skipping it, and still picks the
    /// release under the show's quality profile.
    #[serde(default)]
    pub manual: bool,
    /// Push the current library state to Kobload even when there is nothing left
    /// to acquire. The normal "all acquired, upgrade window closed" skip returns
    /// before enqueuing, which means a season that fills up can never tell
    /// Kobload it is full — leaving Kobload's `episodes_needed` frozen at the
    /// last state it was sent, and its backlog search re-grabbing episodes that
    /// are already on disk. The drift reconciler sets this to correct that.
    #[serde(default)]
    pub refresh_only: bool,
}

/// Handler for wanting TV episodes/seasons via Kobload (search + watch).
pub struct TvWantHandler {
    pool: PgPool,
    runtime_config: RuntimeConfigManager,
    nats_publisher: Publisher,
    library_events: broadcast::Sender<LibraryEvent>,
}

impl TvWantHandler {
    pub fn new(
        pool: PgPool,
        runtime_config: RuntimeConfigManager,
        nats_publisher: Publisher,
        library_events: broadcast::Sender<LibraryEvent>,
    ) -> Self {
        Self {
            pool,
            runtime_config,
            nats_publisher,
            library_events,
        }
    }
}

#[async_trait]
impl JobHandler for TvWantHandler {
    fn job_type(&self) -> &'static str {
        "tv_want"
    }

    async fn execute(
        &self,
        job_id: Uuid,
        payload: Value,
        cancel: CancellationToken,
    ) -> JobResult<Value> {
        let payload: TvWantPayload = serde_json::from_value(payload)
            .map_err(|e| JobError::ExecutionFailed(format!("Invalid payload: {}", e)))?;
        nats_jobs::check_cancelled(&cancel)?;

        info!(
            job_id = %job_id,
            show_id = %payload.show_id,
            season_id = %payload.season_id,
            "Processing tv_want job"
        );

        // Get runtime config
        let runtime = self.runtime_config.get().await;

        // Get callback URL - required for want requests
        let callback_url = match &runtime.kobload_ingest_url {
            Some(url) if !url.trim().is_empty() => url.clone(),
            _ => {
                info!(show_id = %payload.show_id, "Kobload ingest URL not configured, skipping tv_want");
                return Ok(json!({
                    "show_id": payload.show_id,
                    "season_id": payload.season_id,
                    "status": "skipped",
                    "reason": "ingest_url_not_configured"
                }));
            }
        };

        let min_score = runtime.wanted_min_score.unwrap_or(0.65);

        // Fetch show data
        let show = tv_shows::get(&self.pool, payload.show_id)
            .await
            .map_err(|e| JobError::ExecutionFailed(format!("Failed to fetch show: {}", e)))?
            .ok_or_else(|| JobError::ExecutionFailed("Show not found".to_string()))?;

        // Sport shows must go through sports_want (TvSeason generates nonsensical
        // queries like "MotoGP S2026" instead of per-event searches).
        if show.content_category == "sport" {
            info!(
                show_id = %payload.show_id,
                "Redirecting sport show from tv_want to sports_want"
            );
            self.nats_publisher
                .enqueue_job(
                    "kobbler",
                    "sports_want",
                    json!({
                        "show_id": payload.show_id,
                        "season_id": payload.season_id,
                        "episode_ids": payload.episode_ids,
                        "user_id": payload.user_id,
                        "additional_copy": payload.additional_copy,
                        "manual": payload.manual,
                    }),
                )
                .await
                .map_err(|e| JobError::retryable(format!("Failed to enqueue sports_want: {e}")))?;
            return Ok(json!({
                "show_id": payload.show_id,
                "season_id": payload.season_id,
                "status": "redirected_to_sports_want",
            }));
        }

        // Fetch season data
        let season = tv_seasons::get(&self.pool, payload.season_id)
            .await
            .map_err(|e| JobError::ExecutionFailed(format!("Failed to fetch season: {}", e)))?
            .ok_or_else(|| JobError::ExecutionFailed("Season not found".to_string()))?;

        // Extract identifiers from show
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

        let mut affected_episode_ids = Vec::new();

        if let Some(episode_ids) = payload.episode_ids {
            // Want specific episodes, but merge in all other season episodes
            // that need pattern coverage (since kobload upserts by season_id,
            // sending only the new episodes would clobber the existing pattern)
            let mut episodes = Vec::new();
            for ep_id in &episode_ids {
                let episode = tv_episodes::get(&self.pool, *ep_id)
                    .await
                    .map_err(|e| {
                        JobError::ExecutionFailed(format!("Failed to fetch episode: {}", e))
                    })?
                    .ok_or_else(|| {
                        JobError::ExecutionFailed(format!("Episode not found: {}", ep_id))
                    })?;
                episodes.push(episode);
                affected_episode_ids.push(*ep_id);
            }

            // Merge in other episodes that need pattern coverage
            let all_pattern_episodes =
                tv_episodes::list_for_season_pattern(&self.pool, payload.season_id)
                    .await
                    .map_err(|e| {
                        JobError::ExecutionFailed(format!(
                            "Failed to list episodes for pattern: {}",
                            e
                        ))
                    })?;
            let merged_count = {
                let before = episodes.len();
                for ep in all_pattern_episodes {
                    if !episodes.iter().any(|e| e.id == ep.id) {
                        // Promote merged 'wanted' episodes to 'searching' since
                        // they're now covered by the pattern. Don't regress
                        // episodes already in 'downloading'/'ingesting'.
                        if !ep.is_in_library && ep.want_status == "wanted" {
                            affected_episode_ids.push(ep.id);
                        }
                        episodes.push(ep);
                    }
                }
                episodes.len() - before
            };
            if merged_count > 0 {
                info!(
                    season_id = %payload.season_id,
                    primary = episode_ids.len(),
                    merged = merged_count,
                    total = episodes.len(),
                    "Merged existing season episodes into pattern to preserve kobload state"
                );
            }

            let mut episodes_needed = Vec::new();
            let mut episode_map: HashMap<String, Uuid> = HashMap::new();

            // Kobload handles air_date filtering (with its air_date - 1 logic),
            // so we send all episodes and let kobload decide when to search.

            // Batch-fetch library quality for all episodes
            let ep_ids: Vec<Uuid> = episodes.iter().map(|e| e.id).collect();
            let quality_map: HashMap<Uuid, tv_episode_files::EpisodeLibraryQuality> =
                tv_episode_files::get_library_quality_batch(&self.pool, &ep_ids)
                    .await
                    .unwrap_or_default()
                    .into_iter()
                    .map(|q| (q.episode_id, q))
                    .collect();

            for ep in &episodes {
                let (in_library, library_video_height) = if ep.is_in_library {
                    match quality_map.get(&ep.id) {
                        Some(q) => (Some(true), q.video_height),
                        None => (Some(true), None), // in library but no files found
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
                episode_map.insert(ep.episode_identifier.clone(), ep.id);
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
                identifiers: Some(identifiers.clone()),
                alternate_titles: crate::wanted::alternate_titles::alternate_titles(
                    &show.title,
                    show.alternate_titles.iter().flatten(),
                ),
                quality_profile_id: show.quality_profile_id,
                quality_overrides: show.preferred_languages.as_ref().map(|langs| {
                    nats_jobs::QualityOverrides {
                        preferred_languages: Some(langs.clone()),
                        ..Default::default()
                    }
                }),
            };

            let episode_map_value = serde_json::to_value(&episode_map).map_err(|e| {
                JobError::ExecutionFailed(format!("Failed to serialize episode_map: {}", e))
            })?;

            let upgrade_allowed = auto_want::upgrade_window_open(&episodes);

            // Aggregate rejected releases across all episodes in the pattern
            let mut all_rejected = Vec::new();
            for ep in &episodes {
                if let Ok(rejected) = tv_episodes::get_rejected_releases(&self.pool, ep.id).await
                    && let Some(arr) = rejected.as_array()
                {
                    all_rejected.extend(arr.iter().cloned());
                }
            }
            let excluded_releases =
                build_excluded_releases(&serde_json::Value::Array(all_rejected));

            let want_request = nats_jobs::WantRequest {
                source_system: "kobbler".to_string(),
                source_id: payload.season_id.to_string(),
                callback_url: callback_url.clone(),
                auth_token: None,
                query: None,
                patterns: Vec::new(),
                match_targets: Vec::new(),
                match_mode: "fuzzy".to_string(),
                min_score,
                early_exit_score: 0.85,
                media_type: Some("tv".to_string()),
                auto_download: true,
                limit: 100,
                metadata: Some(crate::file_link_mode::tag_metadata(
                    json!({
                        "source_type": "tv_season",
                        "title": show.title,
                        "show_id": payload.show_id,
                        "season_id": payload.season_id,
                        "episode_map": episode_map_value,
                        "user_id": payload.user_id,
                    }),
                    FileLinkMode::from_additional_copy(payload.additional_copy),
                )),
                media_want: Some(media_want),
                upgrade_allowed: Some(upgrade_allowed),
                manual: payload.manual,
                excluded_releases,
                search_not_before: episodes.iter().filter_map(|ep| ep.air_date).min().and_then(
                    |d| {
                        let today = time::OffsetDateTime::now_utc().date();
                        if d > today { Some(d.to_string()) } else { None }
                    },
                ),
            };

            let job_payload = serde_json::to_value(&want_request).map_err(|e| {
                JobError::ExecutionFailed(format!("Failed to serialize request: {}", e))
            })?;

            self.nats_publisher
                .enqueue_job("kobload", "want_request", job_payload)
                .await
                .map_err(|e| {
                    JobError::retryable(format!("Failed to enqueue want_request: {}", e))
                })?;
        } else {
            // Want season — only include episodes that are wanted/searching/in-library.
            // Episodes still in 'none' status are not wanted and should not be sent
            // to kobload or marked as 'searching'.
            let pattern_episodes =
                tv_episodes::list_for_season_pattern(&self.pool, payload.season_id)
                    .await
                    .map_err(|e| {
                        JobError::ExecutionFailed(format!(
                            "Failed to list episodes for season: {}",
                            e
                        ))
                    })?;

            if pattern_episodes.is_empty() {
                info!(
                    season_id = %payload.season_id,
                    "No episodes need pattern coverage, nothing to want"
                );
                return Ok(json!({
                    "show_id": payload.show_id,
                    "season_id": payload.season_id,
                    "status": "skipped",
                    "reason": "no_episodes_need_pattern"
                }));
            }

            let mut episodes_needed = Vec::new();
            let mut episode_map: HashMap<String, Uuid> = HashMap::new();

            // Kobload handles air_date filtering (with its air_date - 1 logic),
            // so we send all episodes and let kobload decide when to search.

            // Batch-fetch library quality for all pattern episodes
            let ep_ids: Vec<Uuid> = pattern_episodes.iter().map(|e| e.id).collect();
            let quality_map: HashMap<Uuid, tv_episode_files::EpisodeLibraryQuality> =
                tv_episode_files::get_library_quality_batch(&self.pool, &ep_ids)
                    .await
                    .unwrap_or_default()
                    .into_iter()
                    .map(|q| (q.episode_id, q))
                    .collect();

            for ep in &pattern_episodes {
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
                episode_map.insert(ep.episode_identifier.clone(), ep.id);
                // Only promote 'wanted' episodes to 'searching' — don't touch
                // episodes already in 'searching'/'downloading'/'ingesting'
                if !ep.is_in_library && ep.want_status == "wanted" {
                    affected_episode_ids.push(ep.id);
                }
            }

            let upgrade_allowed = auto_want::upgrade_window_open(&pattern_episodes);

            // All episodes in library and upgrade window closed — nothing to do.
            // A refresh_only push still goes out: Kobload's stored episodes_needed
            // is the input to its backlog search, so it has to learn the season is
            // full or it keeps re-grabbing episodes we already hold.
            if affected_episode_ids.is_empty() && !upgrade_allowed && !payload.refresh_only {
                info!(
                    season_id = %payload.season_id,
                    "All episodes in library and upgrade window closed, skipping"
                );
                return Ok(json!({
                    "show_id": payload.show_id,
                    "season_id": payload.season_id,
                    "status": "skipped",
                    "reason": "all_acquired_no_upgrade"
                }));
            }

            let last_air_date = pattern_episodes
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
                identifiers: Some(identifiers.clone()),
                alternate_titles: crate::wanted::alternate_titles::alternate_titles(
                    &show.title,
                    show.alternate_titles.iter().flatten(),
                ),
                quality_profile_id: show.quality_profile_id,
                quality_overrides: show.preferred_languages.as_ref().map(|langs| {
                    nats_jobs::QualityOverrides {
                        preferred_languages: Some(langs.clone()),
                        ..Default::default()
                    }
                }),
            };

            let episode_map_value = serde_json::to_value(&episode_map).map_err(|e| {
                JobError::ExecutionFailed(format!("Failed to serialize episode_map: {}", e))
            })?;

            // Aggregate rejected releases across all pattern episodes
            let mut all_rejected = Vec::new();
            for ep in &pattern_episodes {
                if let Ok(rejected) = tv_episodes::get_rejected_releases(&self.pool, ep.id).await
                    && let Some(arr) = rejected.as_array()
                {
                    all_rejected.extend(arr.iter().cloned());
                }
            }
            let excluded_releases =
                build_excluded_releases(&serde_json::Value::Array(all_rejected));

            let want_request = nats_jobs::WantRequest {
                source_system: "kobbler".to_string(),
                source_id: payload.season_id.to_string(),
                callback_url,
                auth_token: None,
                query: None,
                patterns: Vec::new(),
                match_targets: Vec::new(),
                match_mode: "fuzzy".to_string(),
                min_score,
                early_exit_score: 0.85,
                media_type: Some("tv".to_string()),
                auto_download: true,
                limit: 100,
                metadata: Some(crate::file_link_mode::tag_metadata(
                    json!({
                        "source_type": "tv_season",
                        "title": show.title,
                        "show_id": payload.show_id,
                        "season_id": payload.season_id,
                        "episode_map": episode_map_value,
                        "user_id": payload.user_id,
                    }),
                    FileLinkMode::from_additional_copy(payload.additional_copy),
                )),
                media_want: Some(media_want),
                upgrade_allowed: Some(upgrade_allowed),
                manual: payload.manual,
                excluded_releases,
                search_not_before: pattern_episodes
                    .iter()
                    .filter_map(|ep| ep.air_date)
                    .min()
                    .and_then(|d| {
                        let today = time::OffsetDateTime::now_utc().date();
                        if d > today { Some(d.to_string()) } else { None }
                    }),
            };

            let job_payload = serde_json::to_value(&want_request).map_err(|e| {
                JobError::ExecutionFailed(format!("Failed to serialize request: {}", e))
            })?;

            self.nats_publisher
                .enqueue_job("kobload", "want_request", job_payload)
                .await
                .map_err(|e| {
                    JobError::retryable(format!("Failed to enqueue want_request: {}", e))
                })?;
        }

        // Update want_status to "searching" for all affected episodes
        if !affected_episode_ids.is_empty()
            && let Err(e) =
                tv_episodes::set_want_status_batch(&self.pool, &affected_episode_ids, "searching")
                    .await
        {
            warn!(error = %e, "Failed to set episode want_status to searching");
        }

        let _ = self
            .library_events
            .send(LibraryEvent::tv_show_updated(payload.show_id));

        info!(
            show_id = %payload.show_id,
            season_id = %payload.season_id,
            new_episodes = affected_episode_ids.len(),
            "TV want request enqueued to Kobload"
        );

        Ok(json!({
            "show_id": payload.show_id,
            "season_id": payload.season_id,
            "status": "enqueued",
            "episodes_count": affected_episode_ids.len(),
        }))
    }
}
