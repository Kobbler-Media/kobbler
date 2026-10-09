//! Handler for sync provider pull jobs.
//!
//! Periodically fetches user's book data from sync providers and updates local state.

use crate::jobs::JobError;
use crate::jobs::worker::JobHandler;
use crate::repositories::{devices, reading_states, user_sync_providers};
use crate::sync_providers::SyncProviderRegistry;
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sqlx::PgPool;
use tokio_util::sync::CancellationToken;
use tracing::info;
use uuid::Uuid;

/// Payload for sync_pull jobs.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SyncPullPayload {
    /// Which provider to pull from (e.g., "hardcover")
    pub provider_type: String,
    /// Optional user ID to sync (if not specified, sync all users with this provider configured)
    pub user_id: Option<Uuid>,
}

/// Handler for sync_pull jobs.
pub struct SyncPullHandler {
    pool: PgPool,
    registry: SyncProviderRegistry,
}

impl SyncPullHandler {
    pub fn new(pool: PgPool) -> Self {
        let registry = SyncProviderRegistry::new();
        Self { pool, registry }
    }

    /// Sync a specific user's data from a provider.
    async fn sync_user(
        &self,
        user_id: Uuid,
        api_key: &str,
        provider_type: &str,
    ) -> Result<SyncStats, JobError> {
        let provider = self
            .registry
            .create_provider(provider_type, api_key)
            .ok_or_else(|| {
                JobError::ExecutionFailed(format!("Unknown sync provider: {}", provider_type))
            })?;

        let identifier_key = provider.identifier_key();

        let pull_books = provider
            .pull_user_books()
            .await
            .map_err(|e| JobError::ExecutionFailed(e.to_string()))?;

        info!(
            user_id = %user_id,
            provider = provider_type,
            book_count = pull_books.len(),
            "Fetched user books from provider"
        );

        let mut stats = SyncStats::default();

        for pb in pull_books {
            stats.total += 1;

            // Find book in our library by provider's identifier key
            let book_id =
                match find_book_by_identifier(&self.pool, identifier_key, &pb.external_id).await? {
                    Some(id) => id,
                    None => {
                        stats.skipped += 1;
                        continue;
                    }
                };

            // Parse the updated_at timestamp
            let updated_at = pb.updated_at.as_ref().and_then(|s| {
                time::OffsetDateTime::parse(s, &time::format_description::well_known::Rfc3339).ok()
            });

            // Convert status to our status string
            let status = pb.status.as_ref().map(|s| s.as_str());

            // Hardcover progress is already 0-100 percentage, clamp to valid range.
            let progress = pb.progress.map(|p| p.clamp(0.0, 100.0));

            // Check if provider progress is meaningfully ahead of what we have
            // locally. Shared `is_meaningful_delta` matches the Kobo write gate so
            // Hardcover's page-based % vs Kobo's epub-position % rounding noise
            // doesn't trip spurious cross-device prompts.
            let (progress_changed, existing_progress) = if let Some(percent) = progress {
                let existing =
                    reading_states::get_aggregated_for_user_book(&self.pool, user_id, book_id)
                        .await
                        .ok()
                        .flatten();
                let changed = match &existing {
                    Some(agg) => {
                        let local = agg.progress_percent.unwrap_or(0.0);
                        percent > local
                            && crate::kobo::progress::is_meaningful_delta(
                                Some(local),
                                Some(percent),
                            )
                    }
                    None => true,
                };
                (changed, existing)
            } else {
                (false, None)
            };

            let updated = crate::repositories::user_book_data::update_from_sync_provider(
                &self.pool,
                user_id,
                book_id,
                pb.rating,
                status,
                progress_changed,
                updated_at,
                provider_type,
            )
            .await
            .map_err(|e| JobError::ExecutionFailed(e.to_string()))?;

            // Store provider progress in a reading_state so it's visible in the UI.
            if let Some(percent) = progress
                && progress_changed
            {
                let device_id = devices::get_or_create_web_reader(&self.pool, user_id)
                    .await
                    .map_err(|e| JobError::ExecutionFailed(e.to_string()))?;

                let state_id = existing_progress.map(|agg| agg.reading_state_id).or(
                    reading_states::upsert_with_children(&self.pool, user_id, device_id, book_id)
                        .await
                        .map_err(|e| JobError::ExecutionFailed(e.to_string()))?,
                );

                if let Some(state_id) = state_id {
                    let _ = reading_states::update_bookmark(
                        &self.pool,
                        state_id,
                        Some(percent),
                        None,
                        None,
                        None,
                        None,
                    )
                    .await;
                    let _ = reading_states::touch(&self.pool, state_id).await;
                }
            }

            // Store the active read ID for insert/update pattern on push sync
            if let Some(read_id) = pb.active_read_id {
                let _ = user_sync_providers::set_provider_read_id(
                    &self.pool,
                    user_id,
                    book_id,
                    provider_type,
                    read_id,
                )
                .await;
            } else {
                // No active read — clear any stale stored ID
                let _ = user_sync_providers::clear_provider_read_id(
                    &self.pool,
                    user_id,
                    book_id,
                    provider_type,
                )
                .await;
            }

            if updated {
                stats.updated += 1;
            }
        }

        Ok(stats)
    }
}

/// Find a book by an identifier key and value.
async fn find_book_by_identifier(
    pool: &PgPool,
    identifier_key: &str,
    identifier_value: &str,
) -> Result<Option<Uuid>, JobError> {
    let id = sqlx::query_scalar!(
        r#"
        SELECT id FROM books
        WHERE identifiers->>$1 = $2
        "#,
        identifier_key,
        identifier_value,
    )
    .fetch_optional(pool)
    .await
    .map_err(|e| JobError::ExecutionFailed(e.to_string()))?;

    Ok(id)
}

#[derive(Debug, Default)]
struct SyncStats {
    total: usize,
    updated: usize,
    skipped: usize,
}

#[async_trait]
impl JobHandler for SyncPullHandler {
    fn job_type(&self) -> &'static str {
        "sync_pull"
    }

    /// Only 1 at a time to avoid API rate limits.
    fn max_concurrent(&self) -> usize {
        1
    }

    async fn execute(
        &self,
        job_id: Uuid,
        payload: Value,
        cancel: CancellationToken,
    ) -> Result<Value, JobError> {
        let payload: SyncPullPayload = serde_json::from_value(payload)?;
        nats_jobs::check_cancelled(&cancel)?;

        info!(
            job_id = %job_id,
            provider = %payload.provider_type,
            "Starting sync pull"
        );

        let mut total_stats = SyncStats::default();

        if let Some(user_id) = payload.user_id {
            // Sync specific user
            if let Ok(Some(api_key)) =
                user_sync_providers::get_api_key(&self.pool, user_id, &payload.provider_type).await
            {
                let stats = self
                    .sync_user(user_id, &api_key, &payload.provider_type)
                    .await?;
                total_stats.total += stats.total;
                total_stats.updated += stats.updated;
                total_stats.skipped += stats.skipped;
            }
        } else {
            // Sync all users with this provider configured
            let users =
                user_sync_providers::list_users_with_provider(&self.pool, &payload.provider_type)
                    .await
                    .map_err(|e| JobError::ExecutionFailed(e.to_string()))?;

            for (user_id, api_key) in users {
                let stats = self
                    .sync_user(user_id, &api_key, &payload.provider_type)
                    .await?;
                total_stats.total += stats.total;
                total_stats.updated += stats.updated;
                total_stats.skipped += stats.skipped;
            }
        }

        info!(
            job_id = %job_id,
            provider = %payload.provider_type,
            total = total_stats.total,
            updated = total_stats.updated,
            skipped = total_stats.skipped,
            "Sync pull complete"
        );

        Ok(serde_json::json!({
            "provider": payload.provider_type,
            "total": total_stats.total,
            "updated": total_stats.updated,
            "skipped": total_stats.skipped
        }))
    }
}
