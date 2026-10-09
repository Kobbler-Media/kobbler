//! Trakt import history job handler.
//!
//! Fetches watch history from Trakt and matches items to local movies/episodes
//! by TMDB ID. Updates local watch states accordingly.

use crate::jobs::worker::JobHandler;
use crate::jobs::{JobError, JobResult};
use crate::repositories::{
    movie_watch_states, movies, tv_episodes, tv_watch_states, user_sync_providers,
};
use crate::video_sync_providers::{VideoSyncError, VideoSyncProvider, VideoSyncProviderRegistry};
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sqlx::PgPool;
use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;
use tokio_util::sync::CancellationToken;
use tracing::{info, warn};
use uuid::Uuid;

/// Payload for trakt_import_history jobs.
#[derive(Debug, Deserialize, Serialize)]
pub struct TraktImportHistoryPayload {
    pub provider: String,
    #[serde(default = "default_media_type")]
    pub media_type: String,
    pub user_id: Uuid,
}

fn default_media_type() -> String {
    "all".to_string()
}

pub struct TraktImportHistoryHandler {
    pool: PgPool,
}

impl TraktImportHistoryHandler {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}

/// Try to refresh tokens and create a new provider. Returns (new_provider, new_config).
async fn refresh_and_recreate(
    pool: &PgPool,
    user_id: Uuid,
    config: &Value,
    registry: &VideoSyncProviderRegistry,
) -> Result<(Box<dyn VideoSyncProvider>, Value), JobError> {
    warn!("Trakt token expired during import, attempting refresh");
    let new_config =
        crate::video_sync_providers::trakt::try_refresh_stored_tokens(pool, user_id, config, None)
            .await
            .map_err(|e| JobError::ExecutionFailed(format!("Token refresh failed: {e}")))?;

    let new_provider = registry
        .create_provider("trakt", &new_config)
        .ok_or_else(|| {
            JobError::ExecutionFailed("Failed to create Trakt provider after refresh".into())
        })?;

    Ok((new_provider, new_config))
}

#[async_trait]
impl JobHandler for TraktImportHistoryHandler {
    fn job_type(&self) -> &'static str {
        "trakt_import_history"
    }

    async fn execute(
        &self,
        job_id: Uuid,
        payload: Value,
        cancel: CancellationToken,
    ) -> JobResult<Value> {
        let payload: TraktImportHistoryPayload = serde_json::from_value(payload)
            .map_err(|e| JobError::ExecutionFailed(format!("Invalid payload: {}", e)))?;
        nats_jobs::check_cancelled(&cancel)?;

        info!(job_id = %job_id, user_id = %payload.user_id, "Importing watch history from Trakt");

        // Get Trakt provider config for this user
        let provider_row = user_sync_providers::get_by_type(&self.pool, payload.user_id, "trakt")
            .await
            .map_err(|e| JobError::ExecutionFailed(format!("DB error: {}", e)))?
            .ok_or_else(|| {
                JobError::ExecutionFailed("Trakt provider not configured for user".into())
            })?;

        if !provider_row.enabled {
            return Ok(json!({ "status": "skipped", "reason": "provider_disabled" }));
        }

        let registry = VideoSyncProviderRegistry::new();
        let trakt: Box<dyn VideoSyncProvider> = registry
            .create_provider("trakt", &provider_row.config)
            .ok_or_else(|| {
                JobError::ExecutionFailed(
                    "Failed to create Trakt provider (missing access_token or client_id)".into(),
                )
            })?;

        let user_id = payload.user_id;
        let mut movies_imported = 0usize;
        let mut episodes_imported = 0usize;
        let mut movie_errors = 0usize;
        let mut episode_errors = 0usize;

        // Track current provider/config (may change after token refresh)
        let mut current_provider = trakt;
        let mut current_config = provider_row.config.clone();

        // Import movie history
        if payload.media_type == "all" || payload.media_type == "movies" {
            let items = match current_provider.import_movie_history().await {
                Ok(items) => items,
                Err(VideoSyncError::TokenExpired) => {
                    let (new_provider, new_config) =
                        refresh_and_recreate(&self.pool, user_id, &current_config, &registry)
                            .await?;
                    current_provider = new_provider;
                    current_config = new_config;
                    current_provider.import_movie_history().await.map_err(|e| {
                        JobError::retryable(format!("Trakt API error after refresh: {e}"))
                    })?
                }
                Err(e) => {
                    return Err(JobError::retryable(format!(
                        "Failed to import movie history: {e}"
                    )));
                }
            };

            for item in &items {
                if let Some(tmdb_id) = &item.tmdb_id
                    && let Ok(Some(movie)) =
                        movies::get_by_identifier(&self.pool, "tmdb_id", tmdb_id).await
                {
                    let watched_at = item
                        .watched_at
                        .as_deref()
                        .and_then(|s| OffsetDateTime::parse(s, &Rfc3339).ok());

                    let input = movie_watch_states::WatchStateInput {
                        movie_id: movie.id,
                        user_id,
                        watched: true,
                        watched_at,
                        position_seconds: None,
                        rating: None,
                    };
                    if let Err(e) = movie_watch_states::upsert(&self.pool, input).await {
                        warn!(error = %e, movie_id = %movie.id, "Failed to upsert movie watch state");
                        movie_errors += 1;
                    } else {
                        movies_imported += 1;
                    }
                }
            }
            info!(
                count = items.len(),
                matched = movies_imported,
                errors = movie_errors,
                "Imported movie history"
            );
        }

        // Import TV show history
        if payload.media_type == "all" || payload.media_type == "shows" {
            let items = match current_provider.import_show_history().await {
                Ok(items) => items,
                Err(VideoSyncError::TokenExpired) => {
                    let (new_provider, new_config) =
                        refresh_and_recreate(&self.pool, user_id, &current_config, &registry)
                            .await?;
                    current_provider = new_provider;
                    let _ = new_config; // not needed further
                    current_provider.import_show_history().await.map_err(|e| {
                        JobError::retryable(format!("Trakt API error after refresh: {e}"))
                    })?
                }
                Err(e) => {
                    return Err(JobError::retryable(format!(
                        "Failed to import TV history: {e}"
                    )));
                }
            };

            for item in &items {
                if let (Some(tmdb_id), Some(season_num), Some(episode_num)) =
                    (&item.tmdb_id, item.season_number, item.episode_number)
                    && let Ok(Some(episode)) = tv_episodes::get_by_show_tmdb_and_numbers(
                        &self.pool,
                        tmdb_id,
                        season_num as i32,
                        episode_num as i32,
                    )
                    .await
                {
                    let watched_at = item
                        .watched_at
                        .as_deref()
                        .and_then(|s| OffsetDateTime::parse(s, &Rfc3339).ok());

                    let input = tv_watch_states::TvWatchStateInput {
                        episode_id: episode.id,
                        user_id,
                        watched: true,
                        watched_at,
                        position_seconds: None,
                        rating: None,
                    };
                    if let Err(e) = tv_watch_states::upsert(&self.pool, input).await {
                        warn!(error = %e, episode_id = %episode.id, "Failed to upsert episode watch state");
                        episode_errors += 1;
                    } else {
                        episodes_imported += 1;
                    }
                }
            }
            info!(
                count = items.len(),
                matched = episodes_imported,
                errors = episode_errors,
                "Imported TV episode history"
            );
        }

        Ok(json!({
            "status": "completed",
            "movies_imported": movies_imported,
            "episodes_imported": episodes_imported,
            "movie_errors": movie_errors,
            "episode_errors": episode_errors,
        }))
    }
}
