//! Series repository operations.
//!
//! This module is split into submodules for maintainability:
//! - `crud`: Create, update, delete operations
//! - `queries`: Complex queries for sync and enrichment

mod crud;
mod queries;

// Re-export all CRUD operations
pub use crud::{
    SeriesRefreshInput, apply_metadata, bump_next_refresh, delete, delete_orphaned,
    get_or_create_by_name, get_or_create_with_author_overlap, merge_identifiers, refresh_metadata,
    rename, set_book_series, update_monitoring, upsert,
};

// Re-export query operations
pub use queries::{
    SeriesBookRow, SeriesEnrichmentInfo, SeriesListFilter, SeriesSummaryRow, SeriesSyncInfo,
    count_with_filters, exists, find_by_identifier, get_auto_want_mode, get_by_id,
    get_for_enrichment, get_image_url, get_name, list_books, list_due_for_refresh,
    list_with_filters,
};

use super::{RepositoryError, Result};
use crate::images::ImageMap;
use serde_json::Value;

/// Input for creating or updating a series.
#[derive(Debug, Default)]
pub struct SeriesInput {
    pub name: String,
    pub identifiers: Option<Value>,
    pub images: Option<ImageMap>,
    pub description: Option<String>,
    pub total_book_count: Option<i32>,
    pub is_monitored: Option<bool>,
    pub auto_want_mode: Option<String>,
}

impl SeriesInput {
    /// Create a minimal series input with just a name.
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
}

/// Normalizes a series name by trimming whitespace.
/// Returns `None` if the input is empty or only whitespace.
pub fn normalize_name(value: Option<String>) -> Option<String> {
    value.and_then(|v| {
        let trimmed = v.trim();
        if trimmed.is_empty() {
            None
        } else {
            Some(trimmed.to_string())
        }
    })
}
