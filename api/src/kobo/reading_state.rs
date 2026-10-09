//! Reading state operations for Kobo sync.

use chrono::Utc;
use serde_json::{Value, json};
use sqlx::PgPool;
use tracing::{debug, error};
use uuid::Uuid;

use crate::jobs::handlers::queue_sync_job;
use crate::kobo::progress::is_meaningful_delta;
use crate::sync_providers::SyncAction;
use nats_jobs::Publisher;

use crate::kobo::types::{
    BookmarkResponse, BookmarkUpdate, LocationResponse, ReadingStateResponse,
    ReadingStateUpdatePayload, StatisticsResponse, StatisticsUpdate, StatusInfoResponse,
    StatusUpdate, format_rfc3339,
};
use crate::repositories::{books, reading_states, user_sync_providers};

// Re-export ReadingStateRow from repository
pub use reading_states::ReadingStateRow;

/// Wrapper for reading state update input.
pub struct ReadingStateUpdate {
    pub current_bookmark: Option<BookmarkUpdate>,
    pub statistics: Option<StatisticsUpdate>,
    pub status_info: Option<StatusUpdate>,
}

impl From<ReadingStateUpdatePayload> for ReadingStateUpdate {
    fn from(payload: ReadingStateUpdatePayload) -> Self {
        Self {
            current_bookmark: payload.current_bookmark,
            statistics: payload.statistics,
            status_info: payload.status_info,
        }
    }
}

/// What a Kobo GET should return for a specific device.
pub enum ResolvedState {
    /// Echo back raw bytes previously stored for some device (the requester itself,
    /// or another Kobo device whose state is meaningfully ahead). Kobo's "resume
    /// from another device?" prompt triggers iff these bytes differ from what Nickel
    /// already has locally — which is correct in both cases.
    RawPayload(Value),
    /// Build a fresh Kobo-shape response from structured columns. Used when the
    /// winning row has no stored raw body (web reader, Hardcover, first GET before
    /// any PUT). Nickel sees a synthesized diff and prompts, which is the intended
    /// feature for legitimate cross-source progress.
    Synthesized(Box<ReadingStateResponse>),
    /// No state exists at all (empty array response).
    None,
}

/// Decide what bytes the requesting device should see on a GET.
///
/// Rules:
/// 1. Load every row for (user, book) across devices.
/// 2. Find the "winner" under the existing priority sort
///    (Reading > has-progress > Finished, updated_at DESC).
/// 3. If the winner is the requesting device, or no other device is meaningfully
///    ahead of it (|delta| >= 0.3%), echo the requesting device's own stored
///    client_payload verbatim so Nickel sees zero diff.
/// 4. Otherwise serve the winner's client_payload if present, else synthesize.
pub async fn resolve_state_for_device(
    pool: &PgPool,
    user_id: Uuid,
    device_id: Uuid,
    book_id: Uuid,
) -> ResolvedState {
    let rows = match reading_states::list_for_user_book(pool, user_id, book_id).await {
        Ok(rows) if !rows.is_empty() => rows,
        _ => return ResolvedState::None,
    };

    // Rows come back already ordered by the priority sort; rows[0] is the winner.
    let winner_idx = 0usize;
    let own_idx = rows.iter().position(|r| r.device_id == device_id);

    // If the requesting device has its own row and it IS the winner → echo it.
    if let Some(own) = own_idx
        && own == winner_idx
    {
        return resolved_from_row(rows.into_iter().nth(own).expect("own idx valid"));
    }

    // If the requesting device has its own row and the winner is not meaningfully
    // ahead → echo the requester's own bytes (rounding jitter on another device).
    if let Some(own) = own_idx {
        let own_progress = rows[own].progress_percent;
        let winner_progress = rows[winner_idx].progress_percent;
        if !is_meaningful_delta(own_progress, winner_progress) {
            return resolved_from_row(rows.into_iter().nth(own).expect("own idx valid"));
        }
    }

    // Otherwise serve the winner (either because the requester has no row yet,
    // or the winner is meaningfully ahead of the requester).
    resolved_from_row(rows.into_iter().next().expect("winner exists"))
}

fn resolved_from_row(row: ReadingStateRow) -> ResolvedState {
    match row.client_payload.clone() {
        Some(payload) => ResolvedState::RawPayload(payload),
        None => ResolvedState::Synthesized(Box::new(build_reading_state_response(row))),
    }
}

/// Serialize a row to the JSON value Kobo expects in sync-stream responses.
/// Echoes `client_payload` verbatim when present and owned by the requesting
/// device, otherwise falls back to synthesizing from structured columns. Used
/// by `kobo::sync` for entitlement-attached and standalone reading state.
pub fn reading_state_sync_json(row: ReadingStateRow, requesting_device_id: Uuid) -> Value {
    if row.device_id == requesting_device_id
        && let Some(payload) = row.client_payload.clone()
    {
        return payload;
    }
    serde_json::to_value(build_reading_state_response(row)).unwrap_or_else(|_| json!({}))
}

/// Ensure a reading state exists for a book, creating it if necessary.
/// Uses INSERT ... ON CONFLICT to handle race conditions atomically.
pub async fn ensure_reading_state(
    pool: &PgPool,
    user_id: Uuid,
    device_id: Uuid,
    book_id: Uuid,
) -> Option<ReadingStateRow> {
    // Check if book exists
    if !books::exists(pool, book_id).await.ok()? {
        return None;
    }

    // Upsert reading state with all child records
    let state_id = match reading_states::upsert_with_children(pool, user_id, device_id, book_id)
        .await
    {
        Ok(Some(id)) => id,
        Ok(None) => return None,
        Err(e) => {
            error!(error = %e, user_id = %user_id, book_id = %book_id, "failed to ensure reading state");
            return None;
        }
    };

    reading_states::get_full_by_id(pool, state_id, book_id)
        .await
        .ok()
        .flatten()
}

/// Build reading state response from a row.
pub fn build_reading_state_response(row: ReadingStateRow) -> ReadingStateResponse {
    ReadingStateResponse {
        entitlement_id: row.book_id,
        created: format_rfc3339(row.created_at),
        last_modified: format_rfc3339(row.updated_at),
        priority_timestamp: format_rfc3339(row.priority_timestamp),
        status_info: StatusInfoResponse {
            last_modified: format_rfc3339(row.status_last_modified),
            status: row.status,
            times_started_reading: row.times_started,
            last_time_started_reading: row.last_time_started.map(format_rfc3339),
        },
        statistics: StatisticsResponse {
            last_modified: format_rfc3339(row.stats_last_modified),
            spent_reading_minutes: row.spent_reading_minutes,
            remaining_time_minutes: row.remaining_time_minutes,
        },
        current_bookmark: BookmarkResponse {
            last_modified: format_rfc3339(row.bookmark_last_modified),
            progress_percent: row.progress_percent,
            content_source_progress_percent: row.content_source_progress_percent,
            location: row
                .location_value
                .filter(|_| row.location_source.as_deref() != Some("web-reader"))
                .map(|value| LocationResponse {
                    value,
                    location_type: row.location_type.unwrap_or_else(|| "Location".to_string()),
                    source: row.location_source.unwrap_or_else(|| "Kobo".to_string()),
                }),
        },
    }
}

/// Update reading state and return results.
///
/// `raw_payload` is the per-state JSON object as it arrived from the Kobo device
/// (i.e. one element of the top-level `ReadingStates` array). It's stored verbatim
/// (merged over any existing stored payload for this device's row) so future GETs
/// from the same device can echo byte-identical bytes and Nickel doesn't spuriously
/// prompt "continue from another device?".
pub async fn update_reading_state(
    pool: &PgPool,
    nats: &Publisher,
    user_id: Uuid,
    device_id: Uuid,
    book_id: Uuid,
    update: ReadingStateUpdate,
    raw_payload: Value,
) -> Result<Value, String> {
    let reading_state = ensure_reading_state(pool, user_id, device_id, book_id).await;
    let reading_state = match reading_state {
        Some(state) => state,
        None => {
            return Ok(json!({
                "RequestResult": "Success",
                "UpdateResults": []
            }));
        }
    };

    let mut update_results = json!({
        "EntitlementId": book_id
    });

    if let Some(status) = &update.status_info {
        let status_val = status
            .status
            .clone()
            .unwrap_or_else(|| "ReadyToRead".to_string());
        if let Err(e) = reading_states::update_status(pool, reading_state.id, &status_val).await {
            error!(error = %e, reading_state_id = %reading_state.id, "failed to update reading status");
        }
        update_results["StatusInfoResult"] = json!({"Result": "Success"});
    }

    if let Some(stats) = &update.statistics {
        if let Err(e) = reading_states::update_statistics(
            pool,
            reading_state.id,
            stats.spent_reading_minutes,
            stats.remaining_time_minutes,
        )
        .await
        {
            error!(error = %e, reading_state_id = %reading_state.id, "failed to update reading statistics");
        }
        update_results["StatisticsResult"] = json!({"Result": "Success"});
    }

    if let Some(bookmark) = &update.current_bookmark {
        let (loc_value, loc_type, loc_source) = if let Some(loc) = &bookmark.location {
            (
                loc.value.as_deref(),
                loc.location_type.as_deref(),
                loc.source.as_deref(),
            )
        } else {
            (None, None, None)
        };

        // Only touch the structured columns when the progress move is beyond
        // rounding noise. The raw client_payload is still stored below so the
        // same device keeps seeing its own bytes on subsequent GETs — but we
        // don't poison the DB progress_percent that other devices and sync
        // providers compare against.
        let progress_meaningful =
            is_meaningful_delta(reading_state.progress_percent, bookmark.progress_percent);

        if progress_meaningful
            && let Err(e) = reading_states::update_bookmark(
                pool,
                reading_state.id,
                bookmark.progress_percent,
                bookmark.content_source_progress_percent,
                loc_value,
                loc_type,
                loc_source,
            )
            .await
        {
            error!(error = %e, reading_state_id = %reading_state.id, "failed to update reading bookmark");
        }
        update_results["CurrentBookmarkResult"] = json!({"Result": "Success"});
    }

    // Re-fetch the row post-update so scaffolded sub-objects (for any missing
    // CurrentBookmark / Statistics / StatusInfo the PUT didn't touch) reflect
    // the latest structured state, not the pre-update values.
    let post_row = reading_states::get_full_by_id(pool, reading_state.id, book_id)
        .await
        .ok()
        .flatten()
        .unwrap_or(reading_state.clone());

    // Merge and persist the raw Kobo body on every PUT (even when nothing
    // meaningful moved), so LastModified timestamps stay fresh in the echo.
    let merged_payload =
        merge_client_payload(post_row.client_payload.as_ref(), &raw_payload, &post_row);
    if let Err(e) =
        reading_states::set_client_payload(pool, reading_state.id, &merged_payload).await
    {
        error!(error = %e, reading_state_id = %reading_state.id, "failed to store client payload");
    }

    // Update the parent reading_states timestamp
    if let Err(e) = reading_states::touch(pool, reading_state.id).await {
        error!(error = %e, reading_state_id = %reading_state.id, "failed to update reading state timestamp");
    }

    // Queue sync jobs for enabled providers
    queue_sync_jobs_for_update(
        pool,
        nats,
        device_id,
        book_id,
        &reading_state.status,
        &update,
    )
    .await;

    Ok(json!({
        "RequestResult": "Success",
        "UpdateResults": [update_results]
    }))
}

/// Merge a Kobo PUT body into any previously stored client_payload for this
/// reading_states row.
///
/// Always produces a complete payload with all three sub-objects
/// (CurrentBookmark / Statistics / StatusInfo) and top-level scaffolding — so
/// cross-device GETs that echo this blob don't leak partial state. Base is the
/// synthesized response from the post-update row; any previously stored
/// sub-objects are preserved on top of that base (partial PUTs don't clobber
/// unrelated sub-objects) and the incoming PUT's sub-objects are overlaid last.
///
/// web-reader Locations are stripped from the incoming CurrentBookmark so Kobo
/// sync responses never advertise the web reader's cfi spans to Nickel — matches
/// the filter in `build_reading_state_response`.
fn merge_client_payload(
    existing: Option<&Value>,
    incoming: &Value,
    row: &ReadingStateRow,
) -> Value {
    let mut out = serde_json::to_value(build_reading_state_response(row.clone()))
        .unwrap_or_else(|_| json!({}));
    let out_obj = out
        .as_object_mut()
        .expect("synthesized response serializes to a JSON object");

    if let Some(Value::Object(existing_obj)) = existing {
        for key in ["CurrentBookmark", "Statistics", "StatusInfo"] {
            if let Some(v) = existing_obj.get(key) {
                out_obj.insert(key.to_string(), v.clone());
            }
        }
    }

    if let Some(incoming_obj) = incoming.as_object() {
        for key in ["CurrentBookmark", "Statistics", "StatusInfo"] {
            if let Some(v) = incoming_obj.get(key) {
                let mut sub = v.clone();
                if key == "CurrentBookmark"
                    && let Some(sub_obj) = sub.as_object_mut()
                    && let Some(loc) = sub_obj.get("Location")
                    && loc.get("Source").and_then(|s| s.as_str()) == Some("web-reader")
                {
                    sub_obj.remove("Location");
                }
                out_obj.insert(key.to_string(), sub);
            }
        }
        if let Some(v) = incoming_obj.get("LastModified") {
            out_obj.insert("LastModified".to_string(), v.clone());
        }
    }

    out
}

/// Queue sync jobs for all enabled providers based on reading state update.
async fn queue_sync_jobs_for_update(
    pool: &PgPool,
    nats: &Publisher,
    device_id: Uuid,
    book_id: Uuid,
    old_status: &str,
    update: &ReadingStateUpdate,
) {
    // Get enabled providers for this device
    let providers = get_enabled_sync_providers(pool, device_id).await;
    if providers.is_empty() {
        return;
    }

    for provider_id in providers {
        // Status transitions
        if let Some(status_update) = &update.status_info
            && let Some(new_status) = &status_update.status
        {
            // ReadyToRead → Reading = started
            if old_status == "ReadyToRead"
                && new_status == "Reading"
                && let Err(e) = queue_sync_job(
                    pool,
                    nats,
                    device_id,
                    book_id,
                    &provider_id,
                    SyncAction::Started,
                )
                .await
            {
                debug!(
                    provider = %provider_id,
                    error = %e,
                    "Failed to queue started sync job"
                );
            }

            // Any → Finished = finished
            if new_status == "Finished"
                && old_status != "Finished"
                && let Err(e) = queue_sync_job(
                    pool,
                    nats,
                    device_id,
                    book_id,
                    &provider_id,
                    SyncAction::Finished,
                )
                .await
            {
                debug!(
                    provider = %provider_id,
                    error = %e,
                    "Failed to queue finished sync job"
                );
            }
        }

        // Progress updates
        if let Some(bookmark) = &update.current_bookmark
            && let Some(progress) = bookmark.progress_percent
        {
            let today = Utc::now().format("%Y-%m-%d").to_string();
            if let Err(e) = queue_sync_job(
                pool,
                nats,
                device_id,
                book_id,
                &provider_id,
                SyncAction::Progress {
                    percent: progress,
                    page_count: books::get_page_count(pool, book_id).await.ok().flatten(),
                    started_at: Some(today),
                },
            )
            .await
            {
                debug!(
                    provider = %provider_id,
                    error = %e,
                    "Failed to queue progress sync job"
                );
            }
        }
    }
}

/// Get list of enabled sync provider IDs for a device.
///
/// Checks both user-level and device-level provider settings via the
/// user_sync_providers and device_sync_providers tables.
async fn get_enabled_sync_providers(pool: &PgPool, device_id: Uuid) -> Vec<String> {
    user_sync_providers::list_enabled_for_device(pool, device_id)
        .await
        .unwrap_or_default()
        .into_iter()
        .map(|(provider_type, _api_key)| provider_type)
        .collect()
}
