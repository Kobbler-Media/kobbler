//! Library sync logic for Kobo sync protocol.

use serde_json::{Value, json};
use sqlx::PgPool;
use time::OffsetDateTime;
use tracing::{error, warn};
use uuid::Uuid;

use crate::kobo::device::DeviceContext;
use crate::kobo::entitlement::build_entitlement;
use crate::kobo::metadata::{MetadataInput, build_metadata_from_data};
use crate::kobo::reading_state::reading_state_sync_json;
use crate::kobo::tags::sync_tags;
use crate::kobo::types::max_dt;
use crate::repositories::{
    book_files, device_sync_state, entitlements, ingest_events, reading_states,
};

/// Response from building a sync.
pub struct SyncResponse {
    pub items: Vec<Value>,
    pub continue_sync: bool,
    /// Error during sync (if any) - items may still be partial
    pub error: Option<String>,
    /// The max timestamp processed in this batch (for deferred sync state update)
    pub max_processed_timestamp: Option<OffsetDateTime>,
    /// Device ID (for deferred sync state update)
    pub device_id: Uuid,
}

/// Advance the device's sync state after successful response delivery.
///
/// Call this only after the sync response has been written to the TCP socket,
/// so that a connection reset doesn't cause books to be marked as synced
/// without the device actually receiving them.
pub async fn commit_sync_state(pool: &PgPool, device_id: Uuid, timestamp: OffsetDateTime) {
    if let Err(e) = device_sync_state::upsert_last_sync_at(pool, device_id, timestamp).await {
        error!(error = %e, device_id = %device_id, "failed to update device sync state");
    }
}

/// Build sync response for our entitlements.
///
/// The `last_sync_at` parameter is the authoritative sync cursor, extracted
/// from the device-carried JWT sync token. The database sync state is only
/// updated best-effort for UI display.
pub async fn build_sync_response(
    pool: &PgPool,
    base_url: &str,
    device: DeviceContext,
    batch_size: i64,
    last_sync_at: OffsetDateTime,
) -> SyncResponse {
    // Fetch entitlements changed since last sync
    let entitlement_rows = match entitlements::list_for_sync(
        pool,
        device.user_id,
        device.device_id,
        last_sync_at,
        batch_size,
    )
    .await
    {
        Ok(rows) => rows,
        Err(e) => {
            error!(error = %e, device_id = %device.device_id, "Failed to fetch entitlements for sync");
            // Return error without advancing sync state - allows retry
            return SyncResponse {
                items: Vec::new(),
                continue_sync: false,
                error: Some(format!("Database error: {}", e)),
                max_processed_timestamp: None,
                device_id: device.device_id,
            };
        }
    };

    let total = match entitlements::count_pending_sync(
        pool,
        device.user_id,
        device.device_id,
        last_sync_at,
    )
    .await
    {
        Ok(count) => count,
        Err(e) => {
            warn!(error = %e, device_id = %device.device_id, "Failed to count entitlements, using fetched count");
            entitlement_rows.len() as i64
        }
    };

    let mut sync_results: Vec<Value> = Vec::new();
    // Track the max timestamp we've processed in this batch
    // This allows us to advance the sync cursor correctly even when batching
    let mut max_processed_timestamp = last_sync_at;

    for row in entitlement_rows {
        let created_at = max_dt(row.entitlement_created_at, row.book_created_at);
        let updated_at = max_dt(row.entitlement_updated_at, row.book_updated_at);

        // Track the max timestamp from items in this batch
        max_processed_timestamp = max_dt(max_processed_timestamp, updated_at);

        // Deserialize pre-fetched files and authors from JSONB
        let files: Vec<book_files::FileFormatInfo> =
            serde_json::from_value(row.files).unwrap_or_default();
        let author_names: Vec<String> = serde_json::from_value(row.authors).unwrap_or_default();

        let metadata = build_metadata_from_data(
            base_url,
            MetadataInput {
                book_id: row.book_id,
                title: row.title,
                description: row.description,
                language: row.language,
                publisher: row.publisher,
                pubdate: row.pubdate,
                series_id: row.series_id,
                series_name: row.series_name,
                series_index: row.series_index,
                identifiers: row.identifiers,
                page_count: row.page_count,
                rating: row.rating,
            },
            &files,
            &author_names,
        );

        let entitlement = build_entitlement(row.book_id, row.is_removed, created_at, updated_at);

        // Deserialize pre-fetched reading state - only include if updated since last sync
        let reading_state: Option<reading_states::ReadingStateRow> = row
            .reading_state
            .and_then(|v| serde_json::from_value(v).ok());
        let include_reading_state = reading_state
            .as_ref()
            .map(|rs| rs.updated_at > last_sync_at)
            .unwrap_or(false);

        // Factor reading state timestamp into sync cursor so books that enter
        // sync results only due to reading state changes advance the cursor.
        if let Some(rs) = &reading_state {
            max_processed_timestamp = max_dt(max_processed_timestamp, rs.updated_at);
        }

        let reading_state_json = if include_reading_state {
            reading_state.map(|rs| reading_state_sync_json(rs, device.device_id))
        } else {
            None
        };

        // Determine if this is new or changed based on timestamps vs last sync
        let is_new = created_at > last_sync_at;

        // Don't tell the device about a book it never had
        if is_new && row.is_removed {
            continue;
        }

        let wrapper_key = if is_new {
            "NewEntitlement"
        } else {
            "ChangedEntitlement"
        };

        let mut entry = json!({
            "BookEntitlement": entitlement,
            "BookMetadata": metadata,
        });
        if let Some(rs) = reading_state_json {
            entry["ReadingState"] = rs;
        }
        sync_results.push(json!({ wrapper_key: entry }));
    }

    // Emit standalone ChangedReadingState for reading-state-only changes.
    // These are reading states that changed since last sync but whose books
    // were NOT already included in the entitlement results above.
    let entitlement_book_ids: Vec<Uuid> = sync_results
        .iter()
        .filter_map(|v| {
            v.as_object()?
                .values()
                .next()?
                .get("BookMetadata")?
                .get("WorkId")?
                .as_str()?
                .parse()
                .ok()
        })
        .collect();

    match reading_states::list_changed_since(
        pool,
        device.user_id,
        last_sync_at,
        &entitlement_book_ids,
    )
    .await
    {
        Ok(changed_states) => {
            for rs in changed_states {
                max_processed_timestamp = max_dt(max_processed_timestamp, rs.updated_at);
                let reading_state_response = reading_state_sync_json(rs, device.device_id);
                sync_results.push(json!({
                    "ChangedReadingState": {
                        "ReadingState": reading_state_response
                    }
                }));
            }
        }
        Err(e) => {
            warn!(error = %e, device_id = %device.device_id, "Failed to fetch changed reading states");
        }
    }

    // Sync tags/shelves
    let tag_results = sync_tags(pool, device.user_id, last_sync_at).await;
    let tag_count = tag_results.items.len();
    sync_results.extend(tag_results.items);

    // Continue sync if we hit the batch limit for entitlements
    // Note: we check entitlements.len() (not sync_results) because tags are unbatched
    let entitlement_count = sync_results.len() - tag_count;
    let continue_sync = entitlement_count as i64 >= batch_size && total > batch_size;

    // Record sync event
    let count = sync_results.len();
    let name = device.device_name.as_deref().unwrap_or("Unknown device");
    let message = format!(
        "Synced {} - {} item{}",
        name,
        count,
        if count == 1 { "" } else { "s" }
    );
    if let Err(e) = ingest_events::create(
        pool,
        "kobo.sync",
        None,
        "ok",
        Some(&message),
        Some(device.device_id),
    )
    .await
    {
        error!(error = %e, device_id = %device.device_id, "failed to record sync event");
    }
    let subtitle = format!("{} item{} synced", count, if count == 1 { "" } else { "s" });
    crate::routes::library::helpers::record_activity(
        pool,
        "synced",
        Some("device"),
        Some(device.device_id),
        name,
        Some(&subtitle),
        None,
        None,
        Some(device.user_id),
    )
    .await;

    // Return the max timestamp so the caller can advance sync state
    // after the response has been successfully delivered to the device.
    let deferred_timestamp = if max_processed_timestamp > last_sync_at {
        Some(max_processed_timestamp)
    } else {
        None
    };

    SyncResponse {
        items: sync_results,
        continue_sync,
        error: None,
        max_processed_timestamp: deferred_timestamp,
        device_id: device.device_id,
    }
}
