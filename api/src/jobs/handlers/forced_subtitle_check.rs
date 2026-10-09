//! Forced subtitle check job handler.
//!
//! Periodic check that finds video files with non-English default audio but
//! no forced subtitles (embedded or external), then enqueues `subtitle_search`
//! jobs with `forced_only: true` and `auto_download: true`.

use crate::jobs::worker::JobHandler;
use crate::jobs::{JobResult, enqueue_routine};
use crate::repositories::subtitle_providers;
use async_trait::async_trait;
use nats_jobs::{JobContext, Publisher};
use serde_json::{Value, json};
use sqlx::PgPool;
use tokio_util::sync::CancellationToken;
use tracing::{debug, info, warn};
use uuid::Uuid;

const MAX_SEARCHES_PER_RUN: usize = 25;

pub struct ForcedSubtitleCheckHandler {
    pool: PgPool,
    nats_publisher: Publisher,
}

impl ForcedSubtitleCheckHandler {
    pub fn new(pool: PgPool, nats_publisher: Publisher) -> Self {
        Self {
            pool,
            nats_publisher,
        }
    }
}

struct ForcedSubCandidate {
    content_id: Uuid,
    title: String,
    content_type: String,
}

#[async_trait]
impl JobHandler for ForcedSubtitleCheckHandler {
    fn job_type(&self) -> &'static str {
        "forced_subtitle_check"
    }

    fn max_concurrent(&self) -> usize {
        1
    }

    async fn execute(
        &self,
        _job_id: Uuid,
        _payload: Value,
        cancel: CancellationToken,
    ) -> JobResult<Value> {
        nats_jobs::check_cancelled(&cancel)?;
        // Check if any subtitle providers are enabled
        let enabled = subtitle_providers::list_enabled(&self.pool).await?;
        if enabled.is_empty() {
            return Ok(json!({"skipped": "no_providers_enabled"}));
        }

        // Find video files with non-English default audio but no forced subtitles
        let candidates = find_missing_forced_subs(&self.pool).await?;

        let candidates_found = candidates.len();
        let mut enqueued = 0u64;
        let mut skipped_active = 0u64;

        for candidate in candidates.iter().take(MAX_SEARCHES_PER_RUN) {
            // Determine the payload key for dedup checking
            let (payload_key, payload_value) = match candidate.content_type.as_str() {
                "movie" => ("movie_id", candidate.content_id.to_string()),
                "episode" => ("episode_id", candidate.content_id.to_string()),
                _ => continue,
            };

            // Check for already-active subtitle_search job
            match crate::jobs::has_active_job(
                &self.pool,
                "subtitle_search",
                payload_key,
                &payload_value,
            )
            .await
            {
                Ok(true) => {
                    debug!(
                        content_id = %candidate.content_id,
                        content_type = %candidate.content_type,
                        title = %candidate.title,
                        "Skipping - subtitle search already active"
                    );
                    skipped_active += 1;
                    continue;
                }
                Ok(false) => {}
                Err(e) => {
                    warn!(
                        content_id = %candidate.content_id,
                        error = %e,
                        "Failed to check for active subtitle search job"
                    );
                }
            }

            let (entity_type, payload) = match candidate.content_type.as_str() {
                "movie" => (
                    "movie",
                    json!({
                        "movie_id": candidate.content_id,
                        "forced_only": true,
                        "auto_download": true,
                        "languages": ["eng"],
                    }),
                ),
                "episode" => (
                    "episode",
                    json!({
                        "episode_id": candidate.content_id,
                        "forced_only": true,
                        "auto_download": true,
                        "languages": ["eng"],
                    }),
                ),
                _ => continue,
            };

            let display =
                crate::jobs::fetch_job_display(&self.pool, entity_type, candidate.content_id).await;
            let context = JobContext::for_entity(
                entity_type,
                candidate.content_id,
                format!("Forced subtitle search: {}", candidate.title),
            )
            .with_display_opt(display);

            match enqueue_routine(
                &self.pool,
                &self.nats_publisher,
                "subtitle_search",
                payload,
                context,
            )
            .await
            {
                Ok(job_id) => {
                    debug!(
                        job_id = %job_id,
                        content_id = %candidate.content_id,
                        title = %candidate.title,
                        "Enqueued forced subtitle search"
                    );
                    enqueued += 1;
                }
                Err(e) => {
                    warn!(
                        content_id = %candidate.content_id,
                        title = %candidate.title,
                        error = %e,
                        "Failed to enqueue forced subtitle search"
                    );
                }
            }
        }

        if enqueued > 0 {
            info!(
                candidates_found = candidates_found,
                enqueued = enqueued,
                skipped_active = skipped_active,
                "Forced subtitle check complete"
            );
        }

        Ok(json!({
            "candidates_found": candidates_found,
            "enqueued": enqueued,
            "skipped_active": skipped_active,
        }))
    }
}

/// Find video files with non-English default audio but no forced subtitles.
async fn find_missing_forced_subs(pool: &PgPool) -> JobResult<Vec<ForcedSubCandidate>> {
    let rows = sqlx::query!(
        r#"
        -- Movies missing forced subs
        SELECT DISTINCT m.id AS "content_id!", m.title AS "title!",
               'movie' AS "content_type!"
        FROM movie_files mf
        JOIN movies m ON mf.movie_id = m.id
        JOIN audio_tracks at ON at.movie_file_id = mf.id AND at.is_default = TRUE
        LEFT JOIN subtitle_tracks st ON st.movie_file_id = mf.id AND st.is_forced = TRUE
        LEFT JOIN external_subtitles es ON es.movie_file_id = mf.id
            AND es.is_forced = TRUE AND es.available = TRUE
        WHERE mf.available = TRUE
          AND at.language IS NOT NULL
          AND at.language NOT IN ('eng', 'en', 'english')
          AND st.id IS NULL
          AND es.id IS NULL

        UNION ALL

        -- Episodes missing forced subs
        SELECT DISTINCT e.id AS "content_id!",
               s.title || ' S' || sea.season_identifier || 'E' || e.episode_identifier AS "title!",
               'episode' AS "content_type!"
        FROM tv_episode_files ef
        JOIN tv_episode_file_episodes tefe ON tefe.episode_file_id = ef.id
        JOIN tv_episodes e ON tefe.episode_id = e.id
        JOIN tv_seasons sea ON e.season_id = sea.id
        JOIN tv_shows s ON sea.show_id = s.id
        JOIN audio_tracks at ON at.episode_file_id = ef.id AND at.is_default = TRUE
        LEFT JOIN subtitle_tracks st ON st.episode_file_id = ef.id AND st.is_forced = TRUE
        LEFT JOIN external_subtitles es ON es.episode_file_id = ef.id
            AND es.is_forced = TRUE AND es.available = TRUE
        WHERE ef.available = TRUE
          AND at.language IS NOT NULL
          AND at.language NOT IN ('eng', 'en', 'english')
          AND st.id IS NULL
          AND es.id IS NULL
        "#
    )
    .fetch_all(pool)
    .await?;

    Ok(rows
        .into_iter()
        .map(|r| ForcedSubCandidate {
            content_id: r.content_id,
            title: r.title,
            content_type: r.content_type,
        })
        .collect())
}
