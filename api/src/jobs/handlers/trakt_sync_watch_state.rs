//! Trakt watch state sync handler.
//!
//! Pushes local watch state changes to Trakt when movies/episodes
//! are marked as watched or unwatched.

use crate::jobs::worker::JobHandler;
use crate::jobs::{JobError, JobResult};
use crate::repositories::{movies, tv_episodes, tv_seasons, tv_shows, user_sync_providers};
use crate::video_sync_providers::{VideoSyncError, VideoSyncProvider, VideoSyncProviderRegistry};
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sqlx::PgPool;
use tokio_util::sync::CancellationToken;
use tracing::{info, warn};
use uuid::Uuid;

/// Payload for trakt_sync_watch_state jobs.
#[derive(Debug, Deserialize, Serialize)]
pub struct TraktSyncWatchStatePayload {
    /// The user whose Trakt config to use
    pub user_id: Uuid,
    /// "movie" or "episode"
    pub media_type: String,
    /// The movie ID or episode ID
    pub media_id: Uuid,
    /// Whether it was marked as watched (true) or unwatched (false)
    pub watched: bool,
    /// When it was watched (ISO 8601), if applicable
    pub watched_at: Option<String>,
}

pub struct TraktSyncWatchStateHandler {
    pool: PgPool,
}

impl TraktSyncWatchStateHandler {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}

/// Execute a Trakt API operation with automatic token refresh on 401.
///
/// Calls `op` on the provider. If TokenExpired, refreshes tokens in DB,
/// creates a new provider, and retries once.
async fn with_refresh<F>(
    pool: &PgPool,
    user_id: Uuid,
    config: &Value,
    registry: &VideoSyncProviderRegistry,
    provider: &dyn VideoSyncProvider,
    op: F,
) -> Result<(), JobError>
where
    F: Fn(
        &dyn VideoSyncProvider,
    ) -> std::pin::Pin<
        Box<
            dyn std::future::Future<
                    Output = Result<crate::video_sync_providers::VideoSyncResult, VideoSyncError>,
                > + Send
                + '_,
        >,
    >,
{
    let result = op(provider).await;
    match result {
        Ok(_) => Ok(()),
        Err(VideoSyncError::TokenExpired) => {
            warn!("Trakt token expired, attempting refresh");
            let new_config = crate::video_sync_providers::trakt::try_refresh_stored_tokens(
                pool, user_id, config, None,
            )
            .await
            .map_err(|e| JobError::ExecutionFailed(format!("Token refresh failed: {e}")))?;

            let new_provider = registry
                .create_provider("trakt", &new_config)
                .ok_or_else(|| {
                    JobError::ExecutionFailed(
                        "Failed to create Trakt provider after refresh".into(),
                    )
                })?;

            op(new_provider.as_ref())
                .await
                .map_err(|e| JobError::retryable(format!("Trakt API error after refresh: {e}")))?;
            Ok(())
        }
        Err(e) => Err(JobError::retryable(format!("Trakt API error: {e}"))),
    }
}

#[async_trait]
impl JobHandler for TraktSyncWatchStateHandler {
    fn job_type(&self) -> &'static str {
        "trakt_sync_watch_state"
    }

    async fn execute(
        &self,
        job_id: Uuid,
        payload: Value,
        cancel: CancellationToken,
    ) -> JobResult<Value> {
        let payload: TraktSyncWatchStatePayload = serde_json::from_value(payload)
            .map_err(|e| JobError::ExecutionFailed(format!("Invalid payload: {}", e)))?;
        nats_jobs::check_cancelled(&cancel)?;

        info!(
            job_id = %job_id,
            user_id = %payload.user_id,
            media_type = %payload.media_type,
            media_id = %payload.media_id,
            watched = payload.watched,
            "Syncing watch state to Trakt"
        );

        // Get Trakt provider config for this user
        let provider_row = user_sync_providers::get_by_type(&self.pool, payload.user_id, "trakt")
            .await
            .map_err(|e| JobError::ExecutionFailed(format!("DB error: {}", e)))?;

        let Some(provider_row) = provider_row else {
            // User has no Trakt config — silently skip
            return Ok(json!({ "status": "skipped", "reason": "not_configured" }));
        };

        if !provider_row.enabled {
            return Ok(json!({ "status": "skipped", "reason": "provider_disabled" }));
        }

        let registry = VideoSyncProviderRegistry::new();
        let trakt = registry
            .create_provider("trakt", &provider_row.config)
            .ok_or_else(|| JobError::ExecutionFailed("Failed to create Trakt provider".into()))?;

        match payload.media_type.as_str() {
            "movie" => {
                // Get movie's TMDB ID
                let movie = movies::get(&self.pool, payload.media_id)
                    .await
                    .map_err(|e| JobError::ExecutionFailed(format!("DB error: {}", e)))?
                    .ok_or_else(|| JobError::ExecutionFailed("Movie not found".into()))?;

                let tmdb_id = movie
                    .identifiers
                    .get("tmdb_id")
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| {
                        JobError::ExecutionFailed("Movie has no TMDB identifier".into())
                    })?
                    .to_string();

                let watched_at = payload.watched_at.clone();
                let watched = payload.watched;

                with_refresh(
                    &self.pool,
                    payload.user_id,
                    &provider_row.config,
                    &registry,
                    trakt.as_ref(),
                    |p| {
                        let tmdb_id = tmdb_id.clone();
                        let watched_at = watched_at.clone();
                        Box::pin(async move {
                            if watched {
                                p.mark_movie_watched(&tmdb_id, watched_at.as_deref()).await
                            } else {
                                p.mark_movie_unwatched(&tmdb_id).await
                            }
                        })
                    },
                )
                .await?;
            }
            "episode" => {
                // Get episode's show TMDB ID + season/episode numbers
                let episode = tv_episodes::get(&self.pool, payload.media_id)
                    .await
                    .map_err(|e| JobError::ExecutionFailed(format!("DB error: {}", e)))?
                    .ok_or_else(|| JobError::ExecutionFailed("Episode not found".into()))?;

                let season = tv_seasons::get(&self.pool, episode.season_id)
                    .await
                    .map_err(|e| JobError::ExecutionFailed(format!("DB error: {}", e)))?
                    .ok_or_else(|| JobError::ExecutionFailed("Season not found".into()))?;

                let show = tv_shows::get(&self.pool, season.show_id)
                    .await
                    .map_err(|e| JobError::ExecutionFailed(format!("DB error: {}", e)))?
                    .ok_or_else(|| JobError::ExecutionFailed("Show not found".into()))?;

                let show_tmdb_id = show
                    .identifiers
                    .get("tmdb_id")
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| JobError::ExecutionFailed("Show has no TMDB identifier".into()))?
                    .to_string();

                let season_num: u32 = season.season_identifier.parse().unwrap_or(0);
                let episode_num: u32 = episode.episode_identifier.parse().unwrap_or(0);
                let watched_at = payload.watched_at.clone();
                let watched = payload.watched;

                with_refresh(
                    &self.pool,
                    payload.user_id,
                    &provider_row.config,
                    &registry,
                    trakt.as_ref(),
                    |p| {
                        let show_tmdb_id = show_tmdb_id.clone();
                        let watched_at = watched_at.clone();
                        Box::pin(async move {
                            if watched {
                                p.mark_episode_watched(
                                    &show_tmdb_id,
                                    season_num,
                                    episode_num,
                                    watched_at.as_deref(),
                                )
                                .await
                            } else {
                                p.mark_episode_unwatched(&show_tmdb_id, season_num, episode_num)
                                    .await
                            }
                        })
                    },
                )
                .await?;
            }
            other => {
                return Err(JobError::ExecutionFailed(format!(
                    "Unknown media_type: {}",
                    other
                )));
            }
        }

        Ok(json!({
            "status": "synced",
            "media_type": payload.media_type,
            "media_id": payload.media_id,
        }))
    }
}
