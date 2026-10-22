//! Sports want job handler - enqueues want_request jobs to Kobload via NATS.
//!
//! Similar to tv_want but constructs MediaWant::SportEvent for sport-specific
//! search term generation (e.g., "MotoGP 2025 R01 Thailand Race").

use crate::LibraryEvent;
use crate::file_link_mode::FileLinkMode;
use crate::jobs::worker::JobHandler;
use crate::jobs::{JobError, JobResult};
use crate::repositories::{tv_episode_files, tv_episodes, tv_seasons, tv_shows};
use crate::runtime_config::RuntimeConfigManager;
use crate::sport_want::{EventWant, event_want};
use crate::video_metadata::VideoProviderRegistry;
use async_trait::async_trait;
use nats_jobs::Publisher;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sqlx::PgPool;
use tokio::sync::broadcast;
use tokio_util::sync::CancellationToken;
use tracing::{info, warn};
use uuid::Uuid;

use super::want_helpers::build_excluded_releases;

/// Payload for sports_want jobs.
#[derive(Debug, Deserialize, Serialize)]
pub struct SportsWantPayload {
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
    /// A person asked for this event again right now. Kobload reopens an
    /// already-imported event instead of skipping it, and still picks under the
    /// quality profile.
    #[serde(default)]
    pub manual: bool,
}

/// Handler for wanting sport events via Kobload.
pub struct SportsWantHandler {
    pool: PgPool,
    runtime_config: RuntimeConfigManager,
    nats_publisher: Publisher,
    library_events: broadcast::Sender<LibraryEvent>,
}

impl SportsWantHandler {
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
impl JobHandler for SportsWantHandler {
    fn job_type(&self) -> &'static str {
        "sports_want"
    }

    async fn execute(
        &self,
        job_id: Uuid,
        payload: Value,
        cancel: CancellationToken,
    ) -> JobResult<Value> {
        let payload: SportsWantPayload = serde_json::from_value(payload)
            .map_err(|e| JobError::ExecutionFailed(format!("Invalid payload: {}", e)))?;

        info!(
            job_id = %job_id,
            show_id = %payload.show_id,
            season_id = %payload.season_id,
            "Processing sports_want job"
        );

        let runtime = self.runtime_config.get().await;

        let callback_url = match &runtime.kobload_ingest_url {
            Some(url) if !url.trim().is_empty() => url.clone(),
            _ => {
                info!(show_id = %payload.show_id, "Kobload ingest URL not configured, skipping sports_want");
                return Ok(json!({
                    "show_id": payload.show_id,
                    "season_id": payload.season_id,
                    "status": "skipped",
                    "reason": "ingest_url_not_configured"
                }));
            }
        };

        let min_score = runtime.wanted_min_score.unwrap_or(0.65);

        let show = tv_shows::get(&self.pool, payload.show_id)
            .await
            .map_err(|e| JobError::ExecutionFailed(format!("Failed to fetch league: {}", e)))?
            .ok_or_else(|| JobError::ExecutionFailed("League not found".to_string()))?;

        let season = tv_seasons::get(&self.pool, payload.season_id)
            .await
            .map_err(|e| JobError::ExecutionFailed(format!("Failed to fetch season: {}", e)))?
            .ok_or_else(|| JobError::ExecutionFailed("Season not found".to_string()))?;

        let registry = VideoProviderRegistry::new(self.pool.clone());

        let mut affected_episode_ids = Vec::new();

        // Get episodes to want
        let episodes_to_want = if let Some(ref episode_ids) = payload.episode_ids {
            let mut eps = Vec::new();
            for ep_id in episode_ids {
                let ep = tv_episodes::get(&self.pool, *ep_id)
                    .await
                    .map_err(|e| {
                        JobError::ExecutionFailed(format!("Failed to fetch event: {}", e))
                    })?
                    .ok_or_else(|| {
                        JobError::ExecutionFailed(format!("Event not found: {}", ep_id))
                    })?;
                eps.push(ep);
            }
            eps
        } else {
            tv_episodes::list_missing(&self.pool, payload.season_id)
                .await
                .map_err(|e| {
                    JobError::ExecutionFailed(format!("Failed to list missing events: {}", e))
                })?
        };

        if episodes_to_want.is_empty() {
            info!(
                season_id = %payload.season_id,
                "No events to want, nothing to do"
            );
            return Ok(json!({
                "show_id": payload.show_id,
                "season_id": payload.season_id,
                "status": "skipped",
                "reason": "no_events_to_want"
            }));
        }

        let mut failed_episode_ids = Vec::new();
        let mut called_off_ids = Vec::new();

        for episode in &episodes_to_want {
            nats_jobs::check_cancelled(&cancel)?;
            // Fetch library quality for this episode
            let (in_library, library_video_height) = if episode.is_in_library {
                let quality =
                    tv_episode_files::get_library_quality_batch(&self.pool, &[episode.id])
                        .await
                        .unwrap_or_default();
                match quality.first() {
                    Some(q) => (Some(true), q.video_height),
                    None => (Some(true), None),
                }
            } else {
                (Some(false), None)
            };

            let sport_want = event_want(
                &registry,
                EventWant {
                    show: &show,
                    season: &season,
                    episode,
                    in_library,
                    library_video_height,
                },
            )
            .await;
            if sport_want.called_off {
                info!(episode_id = %episode.id, "Event postponed or cancelled, not wanting it");
                called_off_ids.push(episode.id);
                continue;
            }
            let media_want = sport_want.want;

            // Read rejected releases for this episode
            let rejected = tv_episodes::get_rejected_releases(&self.pool, episode.id)
                .await
                .unwrap_or(json!([]));
            let excluded_releases = build_excluded_releases(&rejected);

            // Allow quality upgrades for ~30 days after the event aired (mirrors tv_want):
            // a 720p grab can be replaced when a 1080p release lands later. The have/quality
            // signal is already sent via in_library + library_video_height above.
            let upgrade_allowed = episode.air_date.is_none_or(|d| {
                let cutoff = time::OffsetDateTime::now_utc().date() - time::Duration::days(30);
                d >= cutoff
            });

            let want_request = nats_jobs::WantRequest {
                source_system: "kobbler".to_string(),
                source_id: episode.id.to_string(),
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
                        "source_type": "sport_event",
                        "title": format!(
                            "{} {} - {}",
                            show.title,
                            season.season_identifier,
                            episode.title.as_deref().unwrap_or(&episode.episode_identifier)
                        ),
                        "show_id": payload.show_id,
                        "season_id": payload.season_id,
                        "episode_id": episode.id,
                        "episode_identifier": episode.episode_identifier,
                        "user_id": payload.user_id,
                    }),
                    FileLinkMode::from_additional_copy(payload.additional_copy),
                )),
                media_want: Some(media_want),
                upgrade_allowed: Some(upgrade_allowed),
                manual: payload.manual,
                excluded_releases,
                search_not_before: None,
            };

            let job_payload = match serde_json::to_value(&want_request) {
                Ok(v) => v,
                Err(e) => {
                    warn!(
                        episode_id = %episode.id,
                        error = %e,
                        "Failed to serialize want_request, skipping"
                    );
                    failed_episode_ids.push(episode.id);
                    continue;
                }
            };

            match self
                .nats_publisher
                .enqueue_job("kobload", "want_request", job_payload)
                .await
            {
                Ok(_) => {
                    affected_episode_ids.push(episode.id);
                }
                Err(e) => {
                    warn!(
                        episode_id = %episode.id,
                        error = %e,
                        "Failed to enqueue want_request, continuing"
                    );
                    failed_episode_ids.push(episode.id);
                }
            }
        }

        // If ALL enqueues failed, return retryable error so NATS retries the whole job
        if affected_episode_ids.is_empty() && !failed_episode_ids.is_empty() {
            return Err(JobError::retryable(format!(
                "All {} enqueue attempts failed",
                episodes_to_want.len(),
            )));
        }

        // If some failed, re-enqueue a new sports_want job with just the failed IDs
        if !failed_episode_ids.is_empty() {
            let retry_payload = json!({
                "show_id": payload.show_id,
                "season_id": payload.season_id,
                "episode_ids": failed_episode_ids,
                "user_id": payload.user_id,
                "additional_copy": payload.additional_copy,
            });
            match self
                .nats_publisher
                .enqueue_job("kobbler", "sports_want", retry_payload)
                .await
            {
                Ok(_) => {
                    info!(
                        failed_count = failed_episode_ids.len(),
                        "Re-enqueued sports_want for failed episodes"
                    );
                }
                Err(e) => {
                    warn!(
                        error = %e,
                        failed_count = failed_episode_ids.len(),
                        "Failed to re-enqueue sports_want for failed episodes"
                    );
                }
            }
        }

        // Update want_status to "searching"
        if !affected_episode_ids.is_empty()
            && let Err(e) =
                tv_episodes::set_want_status_batch(&self.pool, &affected_episode_ids, "searching")
                    .await
        {
            warn!(error = %e, "Failed to set event want_status to searching");
        }

        let _ = self
            .library_events
            .send(LibraryEvent::tv_show_updated(payload.show_id));

        info!(
            show_id = %payload.show_id,
            season_id = %payload.season_id,
            events_enqueued = affected_episode_ids.len(),
            events_failed = failed_episode_ids.len(),
            events_called_off = called_off_ids.len(),
            "Sports want request(s) enqueued to Kobload"
        );

        Ok(json!({
            "show_id": payload.show_id,
            "season_id": payload.season_id,
            "status": "enqueued",
            "events_enqueued": affected_episode_ids.len(),
            "events_failed": failed_episode_ids.len(),
            "events_called_off": called_off_ids,
        }))
    }
}
