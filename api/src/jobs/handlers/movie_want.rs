//! Movie want job handler - enqueues want_request job to Kobload via NATS.
//!
//! This job is enqueued when a movie is created/updated with want_status='wanted'.
//! It builds a MediaWant::Movie and enqueues a want_request job to Kobload.
//! The response comes back via events.want.response.kobbler subscriber.

use crate::LibraryEvent;
use crate::file_link_mode::FileLinkMode;
use crate::jobs::worker::JobHandler;
use crate::jobs::{JobError, JobResult};
use crate::repositories::{movie_files, movies};
use crate::runtime_config::RuntimeConfigManager;
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

/// Payload for movie_want jobs.
#[derive(Debug, Deserialize, Serialize)]
pub struct MovieWantPayload {
    pub movie_id: Uuid,
    /// Who asked, when a person did. Schedulers leave this unset.
    #[serde(default)]
    pub user_id: Option<Uuid>,
    /// A regrab: the resulting file joins whatever is already there instead of
    /// replacing it. Everything else — schedulers, RSS, the UI — leaves this false.
    #[serde(default)]
    pub additional_copy: bool,
    /// A person asked for this film again right now. Kobload reopens an
    /// already-imported film instead of skipping it, and still picks under the
    /// quality profile.
    #[serde(default)]
    pub manual: bool,
}

/// Handler for wanting a movie via Kobload (search + watch).
pub struct MovieWantHandler {
    pool: PgPool,
    runtime_config: RuntimeConfigManager,
    nats_publisher: Publisher,
    library_events: broadcast::Sender<LibraryEvent>,
}

impl MovieWantHandler {
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
impl JobHandler for MovieWantHandler {
    fn job_type(&self) -> &'static str {
        "movie_want"
    }

    async fn execute(
        &self,
        job_id: Uuid,
        payload: Value,
        cancel: CancellationToken,
    ) -> JobResult<Value> {
        let payload: MovieWantPayload = serde_json::from_value(payload)
            .map_err(|e| JobError::ExecutionFailed(format!("Invalid payload: {}", e)))?;
        nats_jobs::check_cancelled(&cancel)?;

        let movie_id = payload.movie_id;
        info!(job_id = %job_id, movie_id = %movie_id, "Processing movie_want job");

        // Get runtime config
        let runtime = self.runtime_config.get().await;

        // Get callback URL - required for want requests
        let callback_url = match &runtime.kobload_ingest_url {
            Some(url) if !url.trim().is_empty() => url.clone(),
            _ => {
                info!(movie_id = %movie_id, "Kobload ingest URL not configured, skipping movie_want");
                return Ok(json!({
                    "movie_id": movie_id,
                    "status": "skipped",
                    "reason": "ingest_url_not_configured"
                }));
            }
        };

        let min_score = runtime.wanted_min_score.unwrap_or(0.65);

        // Fetch movie data
        let movie_data = movies::get(&self.pool, movie_id)
            .await
            .map_err(|e| JobError::ExecutionFailed(format!("Failed to fetch movie: {}", e)))?
            .ok_or_else(|| JobError::ExecutionFailed("Movie not found".to_string()))?;

        // Don't want movies until they're close to being available
        let today = time::OffsetDateTime::now_utc().date();
        let current_year = today.year();

        if movie_data.year.is_none_or(|y| y <= 0) {
            info!(movie_id = %movie_id, "Movie has no valid year, skipping want");
            return Ok(json!({
                "movie_id": movie_id,
                "status": "skipped",
                "reason": "no_valid_year"
            }));
        }

        if let Some(release_date) = movie_data.release_date {
            let cutoff = today + time::Duration::days(90);
            if release_date > cutoff {
                info!(movie_id = %movie_id, release_date = %release_date, "Movie release date too far in the future, skipping want");
                return Ok(json!({
                    "movie_id": movie_id,
                    "status": "skipped",
                    "reason": "release_too_far_future"
                }));
            }
        } else if movie_data.year.is_some_and(|y| y > current_year) {
            info!(movie_id = %movie_id, year = ?movie_data.year, "Movie year is in the future with no release date, skipping want");
            return Ok(json!({
                "movie_id": movie_id,
                "status": "skipped",
                "reason": "future_year_no_release_date"
            }));
        }

        let identifiers = nats_jobs::MovieIdentifiers {
            tmdb_id: movie_data
                .identifiers
                .get("tmdb_id")
                .and_then(|v| v.as_str())
                .map(String::from),
            imdb_id: movie_data
                .identifiers
                .get("imdb_id")
                .and_then(|v| v.as_str())
                .map(String::from),
        };

        // Fetch library quality for library-aware download gating
        let (in_library, library_video_height) = if movie_data.is_in_library {
            match movie_files::get_library_video_height(&self.pool, movie_id).await {
                Ok(Some(height)) => (Some(true), height),
                Ok(None) => (Some(true), None), // in library but no available files
                Err(_) => (Some(true), None),
            }
        } else {
            (Some(false), None)
        };

        let media_want = nats_jobs::MediaWant::Movie {
            title: movie_data.title.clone(),
            year: movie_data.year.map(|y| y as u32),
            release_date: movie_data.release_date.map(|d| d.to_string()),
            identifiers: Some(identifiers),
            alternate_titles: crate::wanted::alternate_titles::alternate_titles(
                &movie_data.title,
                movie_data.alternate_titles.iter().flatten(),
            ),
            quality_profile_id: movie_data.quality_profile_id,
            quality_overrides: movie_data.preferred_languages.as_ref().map(|langs| {
                nats_jobs::QualityOverrides {
                    preferred_languages: Some(langs.clone()),
                    ..Default::default()
                }
            }),
            in_library,
            library_video_height,
        };

        // Read rejected releases to pass as exclusions
        let rejected = movies::get_rejected_releases(&self.pool, movie_id)
            .await
            .unwrap_or(json!([]));
        let excluded_releases = build_excluded_releases(&rejected);

        // Build the want_request job payload
        let want_request = nats_jobs::WantRequest {
            source_system: "kobbler".to_string(),
            source_id: movie_id.to_string(),
            callback_url,
            auth_token: None,
            query: None,
            patterns: Vec::new(),
            match_targets: Vec::new(),
            match_mode: "fuzzy".to_string(),
            min_score,
            early_exit_score: 0.85,
            media_type: Some("movie".to_string()),
            auto_download: true,
            limit: 100,
            metadata: Some(crate::file_link_mode::tag_metadata(
                json!({
                    "movie_id": movie_id,
                    "source_type": "movie",
                    "title": movie_data.title,
                    "year": movie_data.year,
                    "user_id": payload.user_id,
                }),
                FileLinkMode::from_additional_copy(payload.additional_copy),
            )),
            media_want: Some(media_want),
            upgrade_allowed: Some(movie_data.release_date.is_none_or(|d| {
                let cutoff = time::OffsetDateTime::now_utc().date() - time::Duration::days(180);
                d >= cutoff
            })),
            manual: payload.manual,
            excluded_releases,
            search_not_before: movie_data.release_date.and_then(|d| {
                let today = time::OffsetDateTime::now_utc().date();
                if d > today { Some(d.to_string()) } else { None }
            }),
        };

        // Enqueue want_request job to Kobload via NATS
        let job_payload = serde_json::to_value(&want_request).map_err(|e| {
            JobError::ExecutionFailed(format!("Failed to serialize request: {}", e))
        })?;

        match self
            .nats_publisher
            .enqueue_job("kobload", "want_request", job_payload)
            .await
        {
            Ok(_) => {
                info!(
                    movie_id = %movie_id,
                    "Want request job enqueued to Kobload for movie"
                );
                // Update want_status to "searching" now that the request is in flight
                if let Err(e) = movies::set_want_status(&self.pool, movie_id, "searching").await {
                    warn!(movie_id = %movie_id, error = %e, "Failed to set want_status to searching");
                }
                let _ = self
                    .library_events
                    .send(LibraryEvent::movie_updated(movie_id));
                Ok(json!({
                    "movie_id": movie_id,
                    "status": "enqueued",
                }))
            }
            Err(e) => {
                warn!(movie_id = %movie_id, error = %e, "Failed to enqueue want_request job for movie");
                Err(JobError::retryable(format!(
                    "Failed to enqueue want_request: {}",
                    e
                )))
            }
        }
    }
}
