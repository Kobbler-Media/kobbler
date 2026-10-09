//! Handler for sync provider jobs.
//!
//! Syncs reading activity to external providers (Hardcover, etc.).

use crate::jobs::JobError;
use crate::jobs::worker::JobHandler;
use crate::repositories::{books, devices, sync_log, user_sync_providers};
use crate::sync_providers::{SyncAction, SyncProviderRegistry};
use async_trait::async_trait;
use nats_jobs::Publisher;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sqlx::PgPool;
use tokio_util::sync::CancellationToken;
use tracing::{debug, info, warn};
use uuid::Uuid;

/// Payload for sync provider jobs.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SyncProviderPayload {
    pub device_id: Uuid,
    pub book_id: Uuid,
    pub provider_id: String,
    pub action: SyncAction,
    /// User ID for API key lookup
    #[serde(default)]
    pub user_id: Option<Uuid>,
}

/// Handler for sync_provider jobs.
pub struct SyncProviderHandler {
    pool: PgPool,
    registry: SyncProviderRegistry,
}

impl SyncProviderHandler {
    pub fn new(pool: PgPool) -> Self {
        let registry = SyncProviderRegistry::new();
        Self { pool, registry }
    }

    /// Get API key for a sync provider using per-user config:
    /// 1. Check device_sync_providers (if device_id is not nil)
    /// 2. Check user_sync_providers (enabled + api_key)
    async fn get_provider_api_key(&self, payload: &SyncProviderPayload) -> Result<String, Value> {
        // Check if device sync is enabled for this provider (skip for UI syncs with nil device)
        if payload.device_id != Uuid::nil() {
            let device_enabled = user_sync_providers::is_device_enabled(
                &self.pool,
                payload.device_id,
                &payload.provider_id,
            )
            .await
            .ok()
            .unwrap_or(true);

            if !device_enabled {
                debug!(
                    device_id = %payload.device_id,
                    provider = %payload.provider_id,
                    "Sync disabled for device"
                );
                return Err(serde_json::json!({
                    "skipped": true,
                    "reason": "device_sync_disabled"
                }));
            }
        }

        // Get user ID - either from payload or from device
        let user_id = match payload.user_id {
            Some(id) => id,
            None if payload.device_id != Uuid::nil() => {
                match devices::get_user_id(&self.pool, payload.device_id).await {
                    Ok(Some(id)) => id,
                    _ => {
                        warn!("No user_id and couldn't get user from device");
                        return Err(serde_json::json!({
                            "skipped": true,
                            "reason": "no_user"
                        }));
                    }
                }
            }
            None => {
                warn!(provider = %payload.provider_id, "No user_id for sync");
                return Err(serde_json::json!({
                    "skipped": true,
                    "reason": "no_user"
                }));
            }
        };

        // Get user's API key from user_sync_providers
        match user_sync_providers::get_api_key(&self.pool, user_id, &payload.provider_id).await {
            Ok(Some(key)) if !key.is_empty() => Ok(key),
            Ok(_) => {
                debug!(user_id = %user_id, provider = %payload.provider_id, "No API key configured for user");
                Err(serde_json::json!({
                    "skipped": true,
                    "reason": "no_api_key"
                }))
            }
            Err(e) => {
                warn!(error = %e, "Failed to get sync provider config");
                Err(serde_json::json!({
                    "skipped": true,
                    "reason": "config_error"
                }))
            }
        }
    }

    /// Resolve the user_id from payload.
    fn resolve_user_id_sync(&self, payload: &SyncProviderPayload) -> Option<Uuid> {
        payload.user_id
    }

    /// Look up the stored read ID for this provider+user+book.
    async fn get_existing_read_id(&self, payload: &SyncProviderPayload) -> Option<i64> {
        let user_id = self.resolve_user_id_sync(payload)?;
        user_sync_providers::get_provider_read_id(
            &self.pool,
            user_id,
            payload.book_id,
            &payload.provider_id,
        )
        .await
        .ok()
        .flatten()
    }

    /// Store the read ID returned from a sync.
    async fn store_read_id(&self, payload: &SyncProviderPayload, read_id: i64) {
        let Some(user_id) = self.resolve_user_id_sync(payload) else {
            return;
        };
        if let Err(e) = user_sync_providers::set_provider_read_id(
            &self.pool,
            user_id,
            payload.book_id,
            &payload.provider_id,
            read_id,
        )
        .await
        {
            warn!(error = %e, provider = %payload.provider_id, "Failed to store provider read_id");
        }
    }

    /// Clear the stored read ID (e.g., after finishing a book).
    async fn clear_read_id(&self, payload: &SyncProviderPayload) {
        let Some(user_id) = self.resolve_user_id_sync(payload) else {
            return;
        };
        if let Err(e) = user_sync_providers::clear_provider_read_id(
            &self.pool,
            user_id,
            payload.book_id,
            &payload.provider_id,
        )
        .await
        {
            warn!(error = %e, provider = %payload.provider_id, "Failed to clear provider read_id");
        }
    }
}

#[async_trait]
impl JobHandler for SyncProviderHandler {
    fn job_type(&self) -> &'static str {
        "sync_provider"
    }

    /// Only 1 at a time - external API calls may be rate limited.
    fn max_concurrent(&self) -> usize {
        1
    }

    async fn execute(
        &self,
        job_id: Uuid,
        payload: Value,
        cancel: CancellationToken,
    ) -> Result<Value, JobError> {
        let payload: SyncProviderPayload = serde_json::from_value(payload)?;
        nats_jobs::check_cancelled(&cancel)?;

        info!(
            job_id = %job_id,
            device_id = %payload.device_id,
            book_id = %payload.book_id,
            provider = %payload.provider_id,
            action = ?payload.action,
            user_id = ?payload.user_id,
            "Processing sync job"
        );

        // Get API key using per-user provider config
        let api_key = match self.get_provider_api_key(&payload).await {
            Ok(key) => key,
            Err(skip_value) => return Ok(skip_value),
        };

        // Get book identifiers and provider metadata
        let (identifiers, provider_metadata) =
            books::get_identifiers_and_metadata(&self.pool, payload.book_id)
                .await?
                .unwrap_or_else(|| (serde_json::json!({}), serde_json::json!({})));

        // Look up stored read ID for insert/update pattern
        let existing_read_id = self.get_existing_read_id(&payload).await;

        // Execute sync
        let result = self
            .registry
            .execute_sync(
                &payload.provider_id,
                &api_key,
                &identifiers,
                &payload.action,
                existing_read_id,
                &provider_metadata,
            )
            .await;

        // Log the sync attempt
        let (response_json, error_text) = match &result {
            Ok(Some(res)) => (serde_json::to_value(res).ok(), None),
            Ok(None) => (None, None),
            Err(e) => (None, Some(e.to_string())),
        };

        let log_input = sync_log::SyncLogInput {
            device_id: payload.device_id,
            book_id: payload.book_id,
            provider: payload.provider_id.clone(),
            action: payload.action.action_name().to_string(),
            payload: serde_json::to_value(&payload.action).ok(),
            response: response_json,
            error: error_text,
        };
        let _ = sync_log::create(&self.pool, log_input).await; // Ignore log failures

        match result {
            Ok(Some(sync_result)) => {
                // Store returned read ID for future updates
                if let Some(read_id) = sync_result.read_id {
                    self.store_read_id(&payload, read_id).await;
                }

                // On Finished, clear the stored read ID so next read starts fresh
                if matches!(payload.action, SyncAction::Finished) {
                    self.clear_read_id(&payload).await;
                }

                info!(
                    job_id = %job_id,
                    provider = %payload.provider_id,
                    action = ?payload.action,
                    "Sync completed successfully"
                );
                Ok(serde_json::json!({
                    "success": true,
                    "provider": payload.provider_id,
                    "action": payload.action.action_name(),
                    "result": sync_result
                }))
            }
            Ok(None) => {
                debug!(
                    job_id = %job_id,
                    provider = %payload.provider_id,
                    "Book not linked to provider, skipped"
                );
                Ok(serde_json::json!({
                    "skipped": true,
                    "reason": "book_not_linked"
                }))
            }
            Err(e) => {
                warn!(
                    job_id = %job_id,
                    provider = %payload.provider_id,
                    error = %e,
                    "Sync failed"
                );
                // Network errors are typically transient - retry
                Err(JobError::retryable(e.to_string()))
            }
        }
    }
}

/// Queue a sync job for a device and book.
pub async fn queue_sync_job(
    pool: &PgPool,
    nats: &Publisher,
    device_id: Uuid,
    book_id: Uuid,
    provider_id: &str,
    action: SyncAction,
) -> Result<Uuid, JobError> {
    // Look up user_id from device
    let user_id = devices::get_user_id(pool, device_id).await.ok().flatten();

    queue_sync_job_for_user(pool, nats, user_id, device_id, book_id, provider_id, action).await
}

/// Queue a sync job for a specific user and book.
/// Use this for UI-triggered syncs where user_id is known.
pub async fn queue_sync_job_for_user(
    pool: &PgPool,
    nats: &Publisher,
    user_id: Option<Uuid>,
    device_id: Uuid,
    book_id: Uuid,
    provider_id: &str,
    action: SyncAction,
) -> Result<Uuid, JobError> {
    let display = crate::jobs::fetch_job_display(pool, "book", book_id).await;
    let context = nats_jobs::JobContext::for_entity(
        "book",
        book_id,
        format!("Syncing {} to {}", action.action_name(), provider_id),
    )
    .with_display_opt(display);
    let payload = SyncProviderPayload {
        device_id,
        book_id,
        provider_id: provider_id.to_string(),
        action,
        user_id,
    };

    crate::jobs::enqueue(
        pool,
        nats,
        "sync_provider",
        serde_json::to_value(payload)?,
        context,
    )
    .await
}
