//! Author repository operations.
//!
//! This module is split into submodules for maintainability:
//! - `crud`: Create, update, delete operations and book linking
//! - `queries`: Complex queries for sync and enrichment

mod crud;
mod queries;

// Re-export all CRUD operations
pub use crud::{
    AuthorRefreshInput, apply_metadata, bump_next_refresh, clear_book_authors, copy_book_links,
    delete, delete_orphaned, get_or_create, get_or_create_by_name, link_book, merge_identifiers,
    refresh_metadata, rename, unlink_all_books, update_metadata, update_monitoring, upsert,
};

// Re-export query operations
pub use queries::{
    AuthorEnrichInfo, AuthorListFilter, AuthorSummaryRow, AuthorSyncInfo, SeriesMonitoring,
    TrackedAuthorSyncInfo, count_with_filters, find_by_name, get_book_author_ids,
    get_book_author_names, get_book_ids, get_by_id, get_first_for_book, get_for_enrich,
    get_image_url, get_name, get_series_monitoring, list_due_for_sync,
    list_monitored_due_for_refresh, list_with_filters,
};

use super::{RepositoryError, Result};
use crate::images::ImageMap;
use serde_json::Value;

/// Input for creating or updating an author.
#[derive(Debug, Default)]
pub struct AuthorInput {
    pub name: String,
    pub identifiers: Option<Value>,
    pub images: Option<ImageMap>,
    pub bio: Option<String>,
    pub is_monitored: Option<bool>,
    pub auto_want_mode: Option<String>,
    pub known_book_count: Option<i32>,
}

impl AuthorInput {
    /// Create a minimal author input with just a name.
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            ..Default::default()
        }
    }

    /// Set identifiers.
    pub fn with_identifiers(mut self, identifiers: Value) -> Self {
        self.identifiers = Some(identifiers);
        self
    }

    /// Set images.
    pub fn with_images(mut self, images: ImageMap) -> Self {
        self.images = Some(images);
        self
    }
}
