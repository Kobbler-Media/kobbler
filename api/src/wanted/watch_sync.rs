//! Watch sync service for syncing wanted books to Kobload
//!
//! This module handles:
//! - Syncing wanted books (is_in_library=FALSE, want_status='wanted') to Kobload as watch patterns
//! - Receiving callbacks from Kobload when watches match

use crate::AppState;
use crate::kobload::{KobloadClient, WatchSyncItem, WatchSyncRequest};
use crate::repositories::books;
use crate::runtime_config::RuntimeConfig;
use tracing::{error, info, warn};
use uuid::Uuid;

/// Source system identifier for Kobbler watch patterns in Kobload
pub const SOURCE_SYSTEM: &str = "kobbler";

/// Wanted book data for watch syncing (from consolidated books table)
#[derive(Debug)]
pub struct WantedItemForSync {
    pub id: Uuid,
    pub title: String,
    pub author: Option<String>,
    pub series: Option<String>,
    pub series_index: Option<f64>,
    pub isbn10: Option<String>,
    pub isbn13: Option<String>,
    pub hardcover_id: Option<String>,
    pub goodreads_id: Option<String>,
    pub openlibrary_id: Option<String>,
    pub want_status: String,
    pub want_format: String,
    pub narrator: Option<String>,
    pub pubdate: Option<time::OffsetDateTime>,
}

/// Fetch all wanted books that should be synced to Kobload.
/// Books are wanted when: is_in_library=FALSE AND want_status IN ('pending', 'wanted')
pub async fn fetch_syncable_items(
    pool: &sqlx::PgPool,
) -> Result<Vec<WantedItemForSync>, sqlx::Error> {
    let rows = books::list_syncable_wanted(pool)
        .await
        .map_err(|e| match e {
            crate::repositories::RepositoryError::Database(e) => e,
            crate::repositories::RepositoryError::Validation(msg) => sqlx::Error::Protocol(msg),
        })?;

    Ok(rows
        .into_iter()
        .map(|row| WantedItemForSync {
            id: row.id,
            title: row.title,
            author: row.author,
            series: row.series_name,
            series_index: row.series_index,
            isbn10: row
                .identifiers
                .get("isbn10")
                .and_then(|v| v.as_str())
                .map(String::from),
            isbn13: row
                .identifiers
                .get("isbn13")
                .and_then(|v| v.as_str())
                .map(String::from),
            hardcover_id: row
                .identifiers
                .get("hardcover")
                .and_then(|v| v.as_str())
                .map(String::from),
            goodreads_id: row
                .identifiers
                .get("goodreads")
                .and_then(|v| v.as_str())
                .map(String::from),
            openlibrary_id: row
                .identifiers
                .get("openlibrary")
                .and_then(|v| v.as_str())
                .map(String::from),
            want_status: row.want_status,
            want_format: row.want_format,
            narrator: row.narrator,
            pubdate: row.pubdate,
        })
        .collect())
}

/// Convert a wanted book to Kobload watch sync item(s).
/// Returns one item for ebook or audiobook, two items for "both" (suffixed with `:ebook`/`:audiobook`).
pub fn wanted_to_sync_items(
    item: &WantedItemForSync,
    callback_url: &str,
    min_score: f64,
) -> Vec<WatchSyncItem> {
    let identifiers = nats_jobs::BookIdentifiers {
        isbn10: item.isbn10.clone(),
        isbn13: item.isbn13.clone(),
        hardcover_id: item.hardcover_id.clone(),
        goodreads_id: item.goodreads_id.clone(),
        openlibrary_id: item.openlibrary_id.clone(),
    };

    let isbn = identifiers.isbn13.clone().or(identifiers.isbn10.clone());

    let metadata = serde_json::json!({
        "book_id": item.id,
        "title": item.title,
        "author": item.author,
        "series": item.series,
        "series_index": item.series_index,
    });

    // Books with want_status='wanted' are enabled for watch sync
    let enabled = item.want_status == "wanted";

    let mut media_wants: Vec<(String, nats_jobs::MediaWant)> = Vec::new();

    if item.want_format == "ebook" || item.want_format == "both" {
        let suffix = if item.want_format == "both" {
            ":ebook"
        } else {
            ""
        };
        media_wants.push((
            format!("{}{}", item.id, suffix),
            nats_jobs::MediaWant::Book {
                title: item.title.clone(),
                author: item.author.clone(),
                isbn: isbn.clone(),
                format_preferences: crate::routes::ingest::EBOOK_EXTENSIONS
                    .iter()
                    .map(|s| (*s).to_string())
                    .collect(),
                identifiers: Some(identifiers.clone()),
                series: item.series.clone(),
                series_position: item.series_index.map(|idx| idx.to_string()),
                release_date: item.pubdate.map(|dt| dt.date().to_string()),
                alternate_titles: None,
            },
        ));
    }

    if item.want_format == "audiobook" || item.want_format == "both" {
        let suffix = if item.want_format == "both" {
            ":audiobook"
        } else {
            ""
        };
        media_wants.push((
            format!("{}{}", item.id, suffix),
            nats_jobs::MediaWant::Audiobook {
                title: item.title.clone(),
                author: item.author.clone(),
                narrator: item.narrator.clone(),
                isbn: isbn.clone(),
                format_preferences: crate::routes::ingest::AUDIOBOOK_EXTENSIONS
                    .iter()
                    .map(|s| (*s).to_string())
                    .collect(),
                identifiers: Some(identifiers.clone()),
                series: item.series.clone(),
                series_position: item.series_index.map(|idx| idx.to_string()),
                release_date: item.pubdate.map(|dt| dt.date().to_string()),
                alternate_titles: None,
            },
        ));
    }

    media_wants
        .into_iter()
        .map(|(source_id, media_want)| WatchSyncItem {
            source_id,
            callback_url: callback_url.to_string(),
            patterns: Vec::new(),
            media_want: Some(media_want),
            match_targets: Vec::new(),
            match_mode: Some("fuzzy".to_string()),
            min_score: Some(min_score),
            auto_download: true,
            metadata: Some(metadata.clone()),
            enabled,
            upgrade_allowed: true,
            search_not_before: None,
        })
        .collect()
}

/// Sync all wanted books to Kobload as watch patterns
pub async fn sync_wanted_to_kobload(state: &AppState) -> Result<(i32, i32, i32), String> {
    let runtime = state.runtime_config.get().await;

    // Check if Kobload is configured
    let (base_url, api_key) = match (&runtime.kobload_base_url, &runtime.kobload_api_key) {
        (Some(url), Some(key)) if !url.is_empty() && !key.is_empty() => (url.clone(), key.clone()),
        _ => {
            return Err("Kobload not configured".to_string());
        }
    };

    // Get callback URL from config - use ingest URL directly so files are processed
    let callback_url = match &runtime.kobload_ingest_url {
        Some(url) if !url.trim().is_empty() => url.clone(),
        _ => {
            return Err("Kobload ingest URL not configured".to_string());
        }
    };

    // Fetch wanted books
    let items = fetch_syncable_items(&state.pool)
        .await
        .map_err(|e| format!("Failed to fetch wanted books: {}", e))?;

    if items.is_empty() {
        info!("No wanted books to sync");
        return Ok((0, 0, 0));
    }

    // Convert to sync items
    let min_score = runtime.wanted_min_score.unwrap_or(0.65);

    let sync_items: Vec<WatchSyncItem> = items
        .iter()
        .flat_map(|item| wanted_to_sync_items(item, &callback_url, min_score))
        .collect();

    let count = sync_items.len();
    info!(count = count, "Syncing wanted books to Kobload");

    // Create Kobload client and sync
    let client = KobloadClient::new(base_url, api_key);
    let request = WatchSyncRequest {
        source_system: SOURCE_SYSTEM.to_string(),
        items: sync_items,
    };

    match client.sync_watch_patterns(&request).await {
        Ok(response) => {
            info!(
                created = response.created,
                updated = response.updated,
                deleted = response.deleted,
                "Watch patterns synced to Kobload"
            );
            Ok((response.created, response.updated, response.deleted))
        }
        Err(e) => {
            error!(error = ?e, "Failed to sync watch patterns to Kobload");
            Err(format!("Kobload sync failed: {}", e))
        }
    }
}

/// Fetch a single wanted book by ID
pub async fn fetch_wanted_item(
    pool: &sqlx::PgPool,
    book_id: Uuid,
) -> Result<Option<WantedItemForSync>, sqlx::Error> {
    let row = books::get_wanted_for_sync(pool, book_id)
        .await
        .map_err(|e| match e {
            crate::repositories::RepositoryError::Database(e) => e,
            crate::repositories::RepositoryError::Validation(msg) => sqlx::Error::Protocol(msg),
        })?;

    Ok(row.map(|row| WantedItemForSync {
        id: row.id,
        title: row.title,
        author: row.author,
        series: row.series_name,
        series_index: row.series_index,
        isbn10: row
            .identifiers
            .get("isbn10")
            .and_then(|v| v.as_str())
            .map(String::from),
        isbn13: row
            .identifiers
            .get("isbn13")
            .and_then(|v| v.as_str())
            .map(String::from),
        hardcover_id: row
            .identifiers
            .get("hardcover")
            .and_then(|v| v.as_str())
            .map(String::from),
        goodreads_id: row
            .identifiers
            .get("goodreads")
            .and_then(|v| v.as_str())
            .map(String::from),
        openlibrary_id: row
            .identifiers
            .get("openlibrary")
            .and_then(|v| v.as_str())
            .map(String::from),
        want_status: row.want_status,
        want_format: row.want_format,
        narrator: row.narrator,
        pubdate: row.pubdate,
    }))
}

/// Update a single watch pattern in Kobload for a wanted book
pub async fn update_watch_pattern_for_wanted(
    state: &AppState,
    book_id: Uuid,
) -> Result<(), String> {
    let runtime = state.runtime_config.get().await;

    // Check if Kobload is configured
    let (base_url, api_key) = match (&runtime.kobload_base_url, &runtime.kobload_api_key) {
        (Some(url), Some(key)) if !url.is_empty() && !key.is_empty() => (url.clone(), key.clone()),
        _ => {
            return Ok(());
        }
    };

    // Get callback URL from config
    // Get callback URL from config - use ingest URL directly so files are processed
    let callback_url = match &runtime.kobload_ingest_url {
        Some(url) if !url.trim().is_empty() => url.clone(),
        _ => {
            return Err("Kobload ingest URL not configured".to_string());
        }
    };

    // Fetch the wanted book
    let item = fetch_wanted_item(&state.pool, book_id)
        .await
        .map_err(|e| format!("Failed to fetch wanted book: {}", e))?
        .ok_or_else(|| "Wanted book not found".to_string())?;

    // Convert to sync item(s) — may be multiple for "both" format
    let min_score = runtime.wanted_min_score.unwrap_or(0.65);

    let sync_items = wanted_to_sync_items(&item, &callback_url, min_score);

    // Update the pattern(s)
    let client = KobloadClient::new(base_url, api_key);
    for sync_item in &sync_items {
        client
            .update_watch_pattern(SOURCE_SYSTEM, &sync_item.source_id, sync_item)
            .await
            .map_err(|e| format!("Failed to update watch pattern: {}", e))?;
    }

    Ok(())
}

/// Delete a single watch pattern from Kobload
pub async fn delete_watch_pattern(runtime: &RuntimeConfig, book_id: Uuid) -> Result<(), String> {
    let (base_url, api_key) = match (&runtime.kobload_base_url, &runtime.kobload_api_key) {
        (Some(url), Some(key)) if !url.is_empty() && !key.is_empty() => (url.clone(), key.clone()),
        _ => {
            return Ok(()); // Silently skip if Kobload not configured
        }
    };

    let client = KobloadClient::new(base_url, api_key);

    match client
        .delete_watch_pattern(SOURCE_SYSTEM, &book_id.to_string())
        .await
    {
        Ok(()) => {
            info!(book_id = %book_id, "Deleted watch pattern from Kobload");
            Ok(())
        }
        Err(e) => {
            warn!(book_id = %book_id, error = ?e, "Failed to delete watch pattern (may not exist)");
            Ok(()) // Don't fail - pattern might not exist
        }
    }
}
