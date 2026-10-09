//! Directory watching for automatic book ingestion.
//!
//! Watches configured directories for new ebook files and adds them to the ingest queue
//! for processing. Supports both real-time filesystem events (via notify crate) and
//! fallback polling for network filesystems.

mod service;

pub use service::{WatcherConfig, WatcherService, process_queue_item};

use serde::{Deserialize, Serialize};
use sqlx::prelude::FromRow;
use time::OffsetDateTime;
use uuid::Uuid;

/// Ingestion mode for a watch directory.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum IngestMode {
    /// Automatically ingest new books into the library.
    Auto,
    /// Add new books to the ingest queue for manual approval.
    #[default]
    Manual,
}

impl From<&str> for IngestMode {
    fn from(s: &str) -> Self {
        match s.to_lowercase().as_str() {
            "auto" => IngestMode::Auto,
            _ => IngestMode::Manual,
        }
    }
}

impl IngestMode {
    pub fn as_str(&self) -> &'static str {
        match self {
            IngestMode::Auto => "auto",
            IngestMode::Manual => "manual",
        }
    }
}

/// File handling mode for ingested books.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum FileMode {
    /// Copy files to storage (default behavior).
    #[default]
    Copy,
    /// Serve files from their original location (symlink-like).
    Link,
}

impl From<&str> for FileMode {
    fn from(s: &str) -> Self {
        match s.to_lowercase().as_str() {
            "link" => FileMode::Link,
            _ => FileMode::Copy,
        }
    }
}

impl FileMode {
    pub fn as_str(&self) -> &'static str {
        match self {
            FileMode::Copy => "copy",
            FileMode::Link => "link",
        }
    }
}

/// Status of an item in the ingest queue.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum IngestQueueStatus {
    /// Metadata is being extracted from the file.
    Extracting,
    /// Ready for user review/approval.
    Pending,
    /// Being converted to KEPUB format.
    Converting,
    /// Approved and ingested into library.
    Approved,
    /// Rejected by user.
    Rejected,
    /// Error during processing.
    Error,
}

impl From<&str> for IngestQueueStatus {
    fn from(s: &str) -> Self {
        match s.to_lowercase().as_str() {
            "extracting" => IngestQueueStatus::Extracting,
            "converting" => IngestQueueStatus::Converting,
            "approved" => IngestQueueStatus::Approved,
            "rejected" => IngestQueueStatus::Rejected,
            "error" => IngestQueueStatus::Error,
            _ => IngestQueueStatus::Pending,
        }
    }
}

impl IngestQueueStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            IngestQueueStatus::Extracting => "extracting",
            IngestQueueStatus::Pending => "pending",
            IngestQueueStatus::Converting => "converting",
            IngestQueueStatus::Approved => "approved",
            IngestQueueStatus::Rejected => "rejected",
            IngestQueueStatus::Error => "error",
        }
    }
}

/// An item in the ingest queue (book row with ingest_status set).
///
/// Field names match Book/LibraryBook for consistency. SQL aliases in SELECT_COLS
/// map DB columns to these names.
#[derive(Debug, Clone, FromRow, Serialize)]
pub struct IngestQueueItem {
    pub id: Uuid,
    pub file_path: String,
    pub file_name: String,
    pub file_size: i64,
    #[sqlx(default)]
    pub file_hash: Option<String>,
    #[sqlx(default)]
    pub source_mtime: Option<OffsetDateTime>,
    pub source_format: String,
    // -- Book-shaped fields --
    #[sqlx(default)]
    pub title: Option<String>,
    /// Linked authors as JSONB array of {id, name}
    #[sqlx(default)]
    pub linked_authors: Option<serde_json::Value>,
    /// Flat merged author names (linked + pending) for backwards compat in approve flow
    #[sqlx(default)]
    pub merged_author_names: Option<Vec<String>>,
    #[sqlx(default)]
    pub description: Option<String>,
    #[sqlx(default)]
    pub series_id: Option<Uuid>,
    #[sqlx(default)]
    pub series_name: Option<String>,
    #[sqlx(default)]
    pub series_index: Option<f64>,
    #[sqlx(default)]
    pub language: Option<String>,
    #[sqlx(default)]
    pub publisher: Option<String>,
    #[sqlx(default)]
    pub pubdate: Option<OffsetDateTime>,
    #[sqlx(default)]
    pub identifiers: serde_json::Value,
    #[sqlx(default)]
    pub rating: Option<f64>,
    #[sqlx(default)]
    pub page_count: Option<i32>,
    #[sqlx(default)]
    pub genres: Vec<String>,
    // Note: extracted_cover is BYTEA, we don't load it by default but we track if it exists
    #[sqlx(default)]
    pub has_cover: bool,
    /// Pending authors not yet linked: [{name, hardcover_id?, image_url?}]
    #[sqlx(default)]
    pub pending_authors: Option<serde_json::Value>,
    /// Pending series not yet linked: {name, hardcover_id?}
    #[sqlx(default)]
    pub pending_series: Option<serde_json::Value>,
    #[sqlx(default)]
    pub pending_series_index: Option<f64>,
    /// Source metadata: provenance info (source_type, provider, external_id, subjects)
    #[sqlx(default)]
    pub source_metadata: Option<serde_json::Value>,
    pub status: String,
    #[sqlx(default)]
    pub conversion_error: Option<String>,
    /// If set, this is an update to an existing book rather than a new ingestion.
    #[sqlx(default)]
    pub update_target_book_id: Option<Uuid>,
    pub created_at: OffsetDateTime,
    #[sqlx(default)]
    pub updated_at: Option<OffsetDateTime>,
}

impl IngestQueueItem {
    pub fn status(&self) -> IngestQueueStatus {
        IngestQueueStatus::from(self.status.as_str())
    }
}

/// Compute a SHA-256 content hash for a file.
pub async fn compute_content_hash(
    path: impl AsRef<std::path::Path>,
) -> Result<String, Box<dyn std::error::Error + Send + Sync>> {
    use sha2::{Digest, Sha256};
    let data = tokio::fs::read(path.as_ref()).await?;
    let mut hasher = Sha256::new();
    hasher.update(&data);
    Ok(hex::encode(hasher.finalize()))
}

/// Compute a fast content fingerprint for change detection and rename tracking.
///
/// Hashes the first 64KB and last 64KB of the file using xxh3-128, producing a
/// hex string. For files ≤ 64KB the entire file is hashed in one read. This is
/// "near free" I/O-wise (two small reads) while being effectively collision-proof
/// when scoped to a single media root.
pub fn compute_file_fingerprint(
    path: &std::path::Path,
) -> Result<String, Box<dyn std::error::Error + Send + Sync>> {
    use std::io::{Read, Seek, SeekFrom};

    const CHUNK: u64 = 65536;

    let mut file = std::fs::File::open(path)?;
    let len = file.metadata()?.len();

    let mut hasher = xxhash_rust::xxh3::Xxh3Default::new();
    let mut buf = [0u8; CHUNK as usize];

    // Read first 64KB (or entire file if smaller)
    let n = file.read(&mut buf)?;
    hasher.update(&buf[..n]);

    // Read last 64KB if file is larger than one chunk
    if len > CHUNK {
        file.seek(SeekFrom::End(-(CHUNK as i64)))?;
        let n = file.read(&mut buf)?;
        hasher.update(&buf[..n]);
    }

    Ok(format!("{:032x}", hasher.digest128()))
}
