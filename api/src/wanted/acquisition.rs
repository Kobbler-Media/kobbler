//! Centralized acquisition logic.
//!
//! When an entity enters the library from any path (Kobload callback, watch directory,
//! Calibre plugin, manual upload, ingest queue approval), call `on_book_acquired()`
//! (or `on_entity_acquired()` for movies/episodes) to ensure consistent cleanup:
//! - Confirm the want to Kobload via HTTP (deletes or starts upgrade window)
//! - Broadcast status update event for real-time UI
//!
//! Call this AFTER the entity's library state has been updated.

use std::time::Duration;

use tokio::sync::broadcast;
use tracing::{info, warn};
use uuid::Uuid;

use crate::events::LibraryEvent;
use crate::kobload::{ConfirmWantStatus, KobloadClient};
use crate::runtime_config::RuntimeConfigManager;
use crate::wanted::SOURCE_SYSTEM;

/// Try to build a `KobloadClient` from the current runtime config.
/// Returns `None` if Kobload is not configured (base URL or API key missing).
fn try_build_kobload_client(
    config: &crate::runtime_config::RuntimeConfig,
) -> Option<KobloadClient> {
    let base_url = config
        .kobload_base_url
        .as_deref()
        .filter(|s| !s.is_empty())?;
    let api_key = config
        .kobload_api_key
        .as_deref()
        .filter(|s| !s.is_empty())?;
    Some(KobloadClient::new(
        base_url.to_string(),
        api_key.to_string(),
    ))
}

/// Confirm a want to Kobload with retries.
///
/// Retries up to 3 times with exponential backoff (1s, 2s, 4s) to handle
/// transient network failures that would otherwise orphan watch patterns.
async fn confirm_want_with_retry(
    client: &KobloadClient,
    source_system: &str,
    source_id: &str,
) -> Result<ConfirmWantStatus, crate::kobload::KobloadError> {
    const MAX_RETRIES: u32 = 3;

    let mut last_err = None;
    for attempt in 0..=MAX_RETRIES {
        if attempt > 0 {
            let delay = Duration::from_secs(1 << (attempt - 1));
            tokio::time::sleep(delay).await;
        }

        match client.confirm_want(source_system, source_id).await {
            Ok(status) => return Ok(status),
            Err(e) => {
                warn!(
                    source_system,
                    source_id,
                    attempt = attempt + 1,
                    max_retries = MAX_RETRIES,
                    error = %e,
                    "confirm_want attempt failed"
                );
                last_err = Some(e);
            }
        }
    }

    Err(last_err.unwrap())
}

/// Confirm a want to Kobload for any entity type.
///
/// Calls `confirm_want` via HTTP to either delete the watch pattern (no upgrades)
/// or start an upgrade window (upgrades allowed). Safe to call when Kobload is not
/// configured or the pattern doesn't exist — both cases are silently ignored.
/// Retries on transient failures to prevent orphaned watch patterns.
///
/// Returns whether the acquisition was announced. `false` means Kobload still has
/// episodes outstanding on this pattern, so nothing was broadcast at all — the
/// caller is the one that knows what to say instead.
pub async fn on_entity_acquired(
    runtime_config: &RuntimeConfigManager,
    library_events: &broadcast::Sender<LibraryEvent>,
    source_system: &str,
    source_id: &str,
    entity_id: Uuid,
    upgraded: bool,
) -> bool {
    on_entity_acquired_inner(
        runtime_config,
        library_events,
        source_system,
        source_id,
        entity_id,
        true,
        upgraded,
    )
    .await
}

/// [`on_entity_acquired`], with the announcement made conditional.
///
/// `announce = false` still broadcasts — the UI draws episode rows one at a time
/// and needs every one of them — but keeps the update out of the notifications,
/// the activity feed and the chat threads, because something larger is speaking
/// for it. That is a season pack: one download, one piece of news, however many
/// episodes it carried.
///
/// Returns whether anything was announced, which is `false` whenever `announce`
/// is, and also when Kobload says the pattern still has episodes outstanding.
pub async fn on_entity_acquired_announcing(
    runtime_config: &RuntimeConfigManager,
    library_events: &broadcast::Sender<LibraryEvent>,
    source_system: &str,
    source_id: &str,
    entity_id: Uuid,
    announce: bool,
    upgraded: bool,
) -> bool {
    on_entity_acquired_inner(
        runtime_config,
        library_events,
        source_system,
        source_id,
        entity_id,
        announce,
        upgraded,
    )
    .await
}

async fn on_entity_acquired_inner(
    runtime_config: &RuntimeConfigManager,
    library_events: &broadcast::Sender<LibraryEvent>,
    source_system: &str,
    source_id: &str,
    entity_id: Uuid,
    announce: bool,
    upgraded: bool,
) -> bool {
    let config = runtime_config.get().await;
    let mut is_partial = false;

    if let Some(client) = try_build_kobload_client(&config) {
        match confirm_want_with_retry(&client, source_system, source_id).await {
            Ok(status) => {
                is_partial = status == ConfirmWantStatus::Partial;
                info!(
                    source_system,
                    source_id,
                    partial = is_partial,
                    "Want confirmed to Kobload (entity acquired)"
                );
            }
            Err(e) => {
                warn!(
                    source_system,
                    source_id,
                    error = %e,
                    "Failed to confirm want to Kobload after retries \
                     (orphan cleanup on Kobload side will handle this)"
                );
            }
        }
    }

    // Only broadcast "acquired" when all episodes are covered.
    // For partial seasons, the pattern stays active and we don't want
    // to tell the UI the entity is fully acquired.
    if is_partial {
        return false;
    }
    let _ = library_events.send(LibraryEvent::WantedStatusUpdated {
        wanted_id: entity_id,
        status: "acquired".to_string(),
        announce,
        upgraded,
    });
    announce
}

/// Perform post-acquisition cleanup for a book that just entered the library.
///
/// A book in the library is no longer wanted in any format, so every key it
/// could be wanted under is confirmed. Safe to call for books that were never
/// wanted — Kobload ignores confirm requests for nonexistent watch patterns.
pub async fn on_book_acquired(
    runtime_config: &RuntimeConfigManager,
    library_events: &broadcast::Sender<LibraryEvent>,
    book_id: Uuid,
) {
    let config = runtime_config.get().await;
    if let Some(client) = try_build_kobload_client(&config) {
        for source_id in super::book_source::all_source_ids(book_id) {
            if let Err(e) = confirm_want_with_retry(&client, SOURCE_SYSTEM, &source_id).await {
                warn!(
                    source_id,
                    error = %e,
                    "Failed to confirm want to Kobload after retries \
                     (orphan cleanup on Kobload side will handle this)"
                );
            }
        }
    }
    let _ = library_events.send(LibraryEvent::WantedStatusUpdated {
        wanted_id: book_id,
        status: "acquired".to_string(),
        announce: true,
        upgraded: false,
    });
}
