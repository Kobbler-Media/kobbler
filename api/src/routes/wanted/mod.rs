//! Wanted books routes - for managing books not in library that should be acquired.
//!
//! After migration 0019, "wanted items" are books where:
//! - is_in_library = FALSE
//! - want_status = 'wanted' or 'pending'
//!
//! Download attempts are tracked in wanted_attempts with book_id.

mod actions;
mod crud;
mod files;
mod kobload;
mod search;

// Re-export all handlers used in the router

// CRUD operations
pub use crud::{create_wanted, delete_wanted, get_wanted, list_wanted};

// Book search/queue operations (registered at /admin/books/{book_id}/releases and /queue)
pub use search::{queue_book_release, search_releases};

// File operations
pub use files::{get_wanted_files, ingest_selected_files, skip_wanted_review};

// Kobload operations
pub use kobload::{list_quality_profiles, test_kobload_connection, watch_callback};

// Action operations
pub use actions::{get_wanted_status, grab_wanted_held, reject_wanted_download, search_wanted_now};

// =============================================================================
// Shared Types
// =============================================================================

use serde::{Deserialize, Serialize};
use time::OffsetDateTime;
use uuid::Uuid;

/// Response for a wanted book (not in library, want_status != 'none')
#[derive(Debug, Serialize)]
pub struct WantedItemResponse {
    pub id: Uuid,
    pub title: String,
    pub author: Option<String>,
    pub series: Option<String>,
    pub series_index: Option<f64>,
    pub identifiers: serde_json::Value,
    pub cover_url: Option<String>,
    pub want_status: String,
    pub attempt_status: Option<String>,
    #[serde(with = "time::serde::rfc3339::option")]
    pub last_attempt_at: Option<OffsetDateTime>,
    #[serde(with = "time::serde::rfc3339")]
    pub created_at: OffsetDateTime,
    #[serde(with = "time::serde::rfc3339")]
    pub updated_at: OffsetDateTime,
}

#[derive(Debug, Serialize)]
pub struct WantedAttemptResponse {
    pub id: Uuid,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub book_id: Option<Uuid>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub movie_id: Option<Uuid>,
    pub source: String,
    pub external_id: Option<String>,
    pub download_client_id: Option<i32>,
    pub download_url: Option<String>,
    pub status: String,
    pub message: Option<String>,
    #[serde(with = "time::serde::rfc3339")]
    pub created_at: OffsetDateTime,
}

#[derive(Debug, Serialize)]
pub struct WantedDetailResponse {
    pub item: WantedItemResponse,
    pub attempts: Vec<WantedAttemptResponse>,
}

#[derive(Debug, Deserialize)]
pub struct CreateWantedRequest {
    pub title: String,
    pub author: Option<String>,
    pub series: Option<String>,
    pub series_index: Option<f64>,
    pub identifiers: Option<serde_json::Value>,
    pub cover_url: Option<String>,
    /// Format preference: "ebook", "audiobook", or "both". Defaults to "ebook".
    pub want_format: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct QueueWantedRequest {
    pub source: String,
    pub release: serde_json::Value,
}

#[derive(Debug, Serialize)]
pub struct SearchReleasesResponse {
    pub results: Vec<serde_json::Value>,
}

#[derive(Debug, Deserialize)]
pub struct WantedReleasesQuery {
    pub q: Option<String>,
    /// Category scope for the search. `None`/"default" keeps the book category
    /// filter; "any" drops it so releases filed under Other/Misc are visible.
    pub scope: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct ListWantedQuery {
    pub search: Option<String>,
    pub status: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct WantedStatusCounts {
    pub pending: i64,
    pub wanted: i64,
    pub searching: i64,
    pub downloading: i64,
    pub ingesting: i64,
    pub watching: i64,
    pub search_failed: i64,
    pub queued: i64,
    pub kobload_error: i64,
    pub needs_review: i64,
    pub completed: i64,
    pub failed: i64,
}

#[derive(Debug, Serialize)]
pub struct ListWantedResponse {
    pub items: Vec<WantedItemResponse>,
    pub status_counts: WantedStatusCounts,
}
