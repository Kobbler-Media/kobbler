//! Book repository operations.
//!
//! This module is split into submodules for maintainability:
//! - `crud`: Create, update, delete operations
//! - `queries`: Complex queries, recommendations, Kobo sync data
//! - `linking`: Author linking operations

mod crud;
mod linking;
mod queries;

// Re-export all types
pub use crud::{
    BookIdentifierMatch, IngestBookInput, add_rejected_release, clear_auto_wanted_for_author,
    clear_auto_wanted_for_series, clear_rejected_releases, create_discovered_book,
    create_ingest_book, delete, delete_by_media_root, delete_orphaned_books,
    delete_stale_discovered, exists, find_by_identifier, get_cover_url, get_identifiers,
    get_identifiers_and_metadata, get_rejected_releases, get_title, mark_user_edited, rearm_want,
    set_identifiers, set_ingest_status, set_media_root_id, set_want_status, set_want_status_if,
    set_want_status_if_not_in_library, touch, touch_many, unwant_all_for_author,
    unwant_all_for_series, update, update_identifiers, upsert_library_book,
    want_all_pending_for_author, want_all_pending_for_series,
};
pub use linking::{clear_authors, link_author, set_authors};
pub use queries::{
    BookCursor, BookEditData, BookListFilter, BookSortField, BookSummary, DiscoveredBookMatchInfo,
    ExistingBookInfo, IN_LIBRARY, KoboMetadataInfo, LibraryBookRow, MetadataWriteBookInfo,
    NOT_IN_LIBRARY, RecommendationRow, SearchTerm, UnavailableBookRow, WantedBookMetadata,
    WantedItemRow, WantedSyncRow, count_with_filters, find_by_hardcover_or_series_title,
    find_library_book_by_title_and_author_id, find_library_book_by_title_and_author_names,
    find_library_book_by_title_in_media_root, find_library_match_by_title, first_by_letter,
    format_condition, get_by_id, get_by_id_for_user, get_for_edit, get_for_kobo_sync,
    get_for_metadata_write, get_ids_by_series, get_page_count, get_recommendations, get_summary,
    get_wanted_for_sync, get_wanted_item, get_wanted_metadata, is_wanted_book, list_by_author,
    list_by_genre, list_by_series, list_by_shelf, list_discovered_for_matching,
    list_fully_unavailable_ids, list_syncable_wanted, list_unavailable, list_wanted,
    list_with_filters, match_discovered_to_library, match_series_discovered_to_library,
};

// Re-export series-related operations from crud
pub use crud::{clear_series_from_books, move_books_to_series, set_series, update_series_index};

use super::{RepositoryError, Result};
use crate::images::ImageMap;
use serde_json::Value;
use time::OffsetDateTime;
use uuid::Uuid;

// =============================================================================
// Input Types
// =============================================================================

/// Input for creating or upserting a library book.
#[derive(Debug, Default)]
pub struct BookInput {
    pub id: Option<Uuid>,
    pub title: String,
    pub description: Option<String>,
    pub language: Option<String>,
    pub publisher: Option<String>,
    pub pubdate: Option<OffsetDateTime>,
    pub identifiers: Option<Value>,
    pub page_count: Option<i32>,
    pub rating: Option<f64>,
    pub rating_count: Option<i32>,
    pub images: Option<ImageMap>,
    pub narrator: Option<String>,
    pub want_format: Option<String>,
    pub is_abridged: Option<bool>,
}

impl BookInput {
    pub fn new(title: impl Into<String>) -> Self {
        Self {
            title: title.into(),
            ..Default::default()
        }
    }

    pub fn with_id(mut self, id: Uuid) -> Self {
        self.id = Some(id);
        self
    }

    pub fn with_identifiers(mut self, identifiers: Value) -> Self {
        self.identifiers = Some(identifiers);
        self
    }
}

/// Input for creating a discovered (non-library) book.
#[derive(Debug, Default)]
pub struct DiscoveredBookInput {
    pub title: String,
    pub description: Option<String>,
    pub identifiers: Option<Value>,
    pub images: Option<ImageMap>,
    pub release_date: Option<time::Date>,
    pub is_released: Option<bool>,
    pub want_status: Option<String>,
    pub discovered_from_author_id: Option<Uuid>,
    pub page_count: Option<i32>,
    pub rating: Option<f64>,
    pub rating_count: Option<i32>,
    pub provider_metadata: Option<serde_json::Value>,
    pub narrator: Option<String>,
    pub want_format: Option<String>,
}

impl DiscoveredBookInput {
    pub fn new(title: impl Into<String>) -> Self {
        Self {
            title: title.into(),
            ..Default::default()
        }
    }

    pub fn with_identifiers(mut self, identifiers: Value) -> Self {
        self.identifiers = Some(identifiers);
        self
    }
}

/// Input for updating an existing book. All fields are optional - only provided fields are updated.
#[derive(Debug, Default)]
pub struct BookUpdate {
    pub title: Option<String>,
    pub description: Option<String>,
    pub language: Option<String>,
    pub publisher: Option<String>,
    pub pubdate: Option<OffsetDateTime>,
    pub identifiers: Option<Value>,
    pub page_count: Option<i32>,
    pub rating: Option<f64>,
    pub rating_count: Option<i32>,
    pub images: Option<ImageMap>,
    pub is_released: Option<bool>,
    pub want_status: Option<String>,
    pub provider_metadata: Option<serde_json::Value>,
    pub narrator: Option<String>,
    pub duration_seconds: Option<i32>,
    pub is_abridged: Option<bool>,
    pub want_format: Option<String>,
}
