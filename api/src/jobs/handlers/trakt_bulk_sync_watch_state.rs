//! Bulk Trakt watch state sync handler.
//!
//! Syncs multiple episodes to Trakt in a single API call, used by
//! bulk show/season watch state updates.

use crate::jobs::worker::JobHandler;
use crate::jobs::{JobError, JobResult};
use crate::repositories::{tv_shows, user_sync_providers};
use crate::video_sync_providers::{VideoSyncError, VideoSyncProvider, VideoSyncProviderRegistry};
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sqlx::PgPool;
use tokio_util::sync::CancellationToken;
use tracing::{info, warn};
use uuid::Uuid;

/// Payload for trakt_bulk_sync_watch_state jobs.
#[derive(Debug, Deserialize, Serialize)]
pub struct TraktBulkSyncWatchStatePayload {
    pub user_id: Uuid,
    pub show_id: Uuid,
    pub episode_ids: Vec<Uuid>,
    pub watched: bool,
    pub watched_at: Option<String>,
}

pub struct TraktBulkSyncWatchStateHandler {
    pool: PgPool,
}

impl TraktBulkSyncWatchStateHandler {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}

/// Execute a Trakt API operation with automatic token refresh on 401.
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

/// Resolved episode info for Trakt sync.
struct ResolvedEpisode {
    season_number: u32,
    episode_number: u32,
}

#[async_trait]
impl JobHandler for TraktBulkSyncWatchStateHandler {
    fn job_type(&self) -> &'static str {
        "trakt_bulk_sync_watch_state"
    }

    async fn execute(
        &self,
        job_id: Uuid,
        payload: Value,
        cancel: CancellationToken,
    ) -> JobResult<Value> {
        let payload: TraktBulkSyncWatchStatePayload = serde_json::from_value(payload)
            .map_err(|e| JobError::ExecutionFailed(format!("Invalid payload: {}", e)))?;
        nats_jobs::check_cancelled(&cancel)?;

        if payload.episode_ids.is_empty() {
            return Ok(json!({ "status": "skipped", "reason": "no_episodes" }));
        }

        info!(
            job_id = %job_id,
            user_id = %payload.user_id,
            show_id = %payload.show_id,
            episode_count = payload.episode_ids.len(),
            watched = payload.watched,
            "Bulk syncing watch state to Trakt"
        );

        // Get Trakt provider config for this user
        let provider_row = user_sync_providers::get_by_type(&self.pool, payload.user_id, "trakt")
            .await
            .map_err(|e| JobError::ExecutionFailed(format!("DB error: {}", e)))?;

        let Some(provider_row) = provider_row else {
            return Ok(json!({ "status": "skipped", "reason": "not_configured" }));
        };

        if !provider_row.enabled {
            return Ok(json!({ "status": "skipped", "reason": "provider_disabled" }));
        }

        // Get show TMDB ID
        let show = tv_shows::get(&self.pool, payload.show_id)
            .await
            .map_err(|e| JobError::ExecutionFailed(format!("DB error: {}", e)))?
            .ok_or_else(|| JobError::ExecutionFailed("Show not found".into()))?;

        let show_tmdb_id = show
            .identifiers
            .get("tmdb_id")
            .and_then(|v| v.as_str())
            .ok_or_else(|| JobError::ExecutionFailed("Show has no TMDB identifier".into()))?
            .to_string();

        // Batch-resolve all episode season/episode numbers
        let rows = sqlx::query!(
            r#"
            SELECT te.id, te.episode_identifier, tsn.season_identifier
            FROM tv_episodes te
            JOIN tv_seasons tsn ON te.season_id = tsn.id
            WHERE te.id = ANY($1)
            "#,
            &payload.episode_ids,
        )
        .fetch_all(&self.pool)
        .await
        .map_err(|e| JobError::ExecutionFailed(format!("DB error resolving episodes: {}", e)))?;

        let resolved: Vec<ResolvedEpisode> = rows
            .iter()
            .map(|r| ResolvedEpisode {
                season_number: r.season_identifier.parse().unwrap_or(0),
                episode_number: r.episode_identifier.parse().unwrap_or(0),
            })
            .collect();

        if resolved.is_empty() {
            return Ok(json!({ "status": "skipped", "reason": "no_episodes_found" }));
        }

        let registry = VideoSyncProviderRegistry::new();
        let trakt = registry
            .create_provider("trakt", &provider_row.config)
            .ok_or_else(|| JobError::ExecutionFailed("Failed to create Trakt provider".into()))?;

        let watched_at = payload.watched_at.clone();
        let watched = payload.watched;
        let episode_count = resolved.len();

        if watched {
            let episodes: Vec<(u32, u32, Option<String>)> = resolved
                .iter()
                .map(|r| (r.season_number, r.episode_number, watched_at.clone()))
                .collect();

            with_refresh(
                &self.pool,
                payload.user_id,
                &provider_row.config,
                &registry,
                trakt.as_ref(),
                |p| {
                    let episodes = episodes.clone();
                    let show_tmdb_id = show_tmdb_id.clone();
                    Box::pin(async move { p.mark_episodes_watched(&show_tmdb_id, &episodes).await })
                },
            )
            .await?;
        } else {
            let episodes: Vec<(u32, u32)> = resolved
                .iter()
                .map(|r| (r.season_number, r.episode_number))
                .collect();

            with_refresh(
                &self.pool,
                payload.user_id,
                &provider_row.config,
                &registry,
                trakt.as_ref(),
                |p| {
                    let episodes = episodes.clone();
                    let show_tmdb_id = show_tmdb_id.clone();
                    Box::pin(
                        async move { p.mark_episodes_unwatched(&show_tmdb_id, &episodes).await },
                    )
                },
            )
            .await?;
        }

        Ok(json!({
            "status": "synced",
            "show_id": payload.show_id,
            "episode_count": episode_count,
        }))
    }
}
