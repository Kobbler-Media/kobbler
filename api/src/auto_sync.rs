//! Auto-sync: create per-device entitlements for devices with auto_sync_new_books enabled.

use nats_jobs::Publisher;
use sqlx::PgPool;
use uuid::Uuid;

use crate::repositories::{devices, entitlements};

/// Create DEVICE-scoped entitlements for all devices that have auto_sync_new_books enabled.
/// Returns the list of device IDs that received entitlements.
/// Also triggers Kindle delivery for any Kindle devices in the list.
pub async fn create_auto_sync_entitlements(
    pool: &PgPool,
    nats: &Publisher,
    book_id: Uuid,
) -> Vec<Uuid> {
    let device_ids = match devices::list_auto_sync_device_ids(pool).await {
        Ok(ids) => ids,
        Err(e) => {
            tracing::warn!(error = %e, book_id = %book_id, "Failed to list auto-sync devices");
            return vec![];
        }
    };

    if device_ids.is_empty() {
        tracing::debug!(book_id = %book_id, "No devices with auto-sync enabled, skipping entitlements");
        return vec![];
    }

    if let Err(e) = entitlements::create_for_auto_sync_devices(pool, book_id, &device_ids).await {
        tracing::warn!(error = %e, book_id = %book_id, "Failed to create auto-sync entitlements");
        return vec![];
    }

    tracing::info!(
        book_id = %book_id,
        device_count = device_ids.len(),
        "Created auto-sync entitlements for devices"
    );

    // Trigger Kindle delivery for each device
    for &device_id in &device_ids {
        crate::routes::admin::trigger_kindle_delivery_if_needed(pool, nats, book_id, device_id)
            .await;
    }

    device_ids
}
