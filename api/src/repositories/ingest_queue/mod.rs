//! Ingest queue repository operations.
//!
//! This module is split into submodules for maintainability:
//! - `crud`: Create, update, delete operations
//! - `queries`: List and query operations

mod crud;
mod queries;

// Re-export all CRUD operations
pub use crud::{
    UploadInput, approve, batch_reject, create, create_for_upload, delete, exists_by_path,
    get_by_id, get_cover, hash_exists, reject, restore, set_converting, set_error, update,
    update_cover, update_cover_and_set_pending, update_status, update_status_if_pending,
};

// Re-export query operations
pub use queries::{
    ExtractionItem, ListFilters, StatusCounts, get_by_id_full, get_status_counts, list_extracting,
    list_with_filters,
};

use super::Result;
use serde_json::Value;
use time::OffsetDateTime;
use uuid::Uuid;

/// Input for creating an ingest queue item.
#[derive(Debug, Default)]
pub struct IngestQueueInput {
    pub file_path: String,
    pub file_name: String,
    pub file_size: i64,
    pub file_hash: Option<String>,
    pub source_mtime: Option<OffsetDateTime>,
    pub source_format: String,
    pub title: Option<String>,
    pub description: Option<String>,
    pub language: Option<String>,
    pub publisher: Option<String>,
    pub pubdate: Option<OffsetDateTime>,
    pub cover: Option<Vec<u8>>,
    /// Pending authors not yet linked: [{name, hardcover_id?, image_url?}]
    pub pending_authors: Option<Value>,
    /// Pending series not yet linked: {name, hardcover_id?}
    pub pending_series: Option<Value>,
    /// Source metadata: provenance info (source_type, provider, external_id, subjects)
    pub source_metadata: Option<Value>,
    pub update_target_book_id: Option<Uuid>,
    pub status: String,
}

/// Input for updating an ingest queue item.
#[derive(Debug, Default)]
pub struct IngestQueueUpdate {
    pub title: Option<String>,
    pub description: Option<String>,
    pub language: Option<String>,
    pub publisher: Option<String>,
    pub pubdate: Option<OffsetDateTime>,
    pub cover: Option<Vec<u8>>,
    /// Pending authors not yet linked: [{name, hardcover_id?, image_url?}]
    pub pending_authors: Option<Value>,
    /// Pending series not yet linked: {name, hardcover_id?}
    pub pending_series: Option<Value>,
    pub pending_series_index: Option<f64>,
    /// Source metadata: provenance info (source_type, provider, external_id, subjects)
    pub source_metadata: Option<Value>,
    pub update_target_book_id: Option<Uuid>,
    pub status: Option<String>,
    pub conversion_error: Option<String>,
}
