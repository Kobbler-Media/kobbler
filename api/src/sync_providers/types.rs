//! Types for sync providers

use serde::{Deserialize, Serialize};

// ============================================================================
// Capability System
// ============================================================================

/// Capabilities that a sync provider can support
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct SyncCapabilities {
    /// Can mark books as "currently reading"
    pub mark_reading: bool,
    /// Can mark books as "finished"
    pub mark_finished: bool,
    /// Can update reading progress percentage
    pub progress: bool,
    /// Can set book ratings
    pub rating: bool,
    /// Can pull user's reading library from provider
    pub pull_sync: bool,
    /// Has edition-specific metadata for sync (e.g., Hardcover edition_id)
    pub edition_resolution: bool,
}

impl SyncCapabilities {
    pub const fn all() -> Self {
        Self {
            mark_reading: true,
            mark_finished: true,
            progress: true,
            rating: true,
            pull_sync: true,
            edition_resolution: true,
        }
    }

    pub const fn none() -> Self {
        Self {
            mark_reading: false,
            mark_finished: false,
            progress: false,
            rating: false,
            pull_sync: false,
            edition_resolution: false,
        }
    }
}

impl Default for SyncCapabilities {
    fn default() -> Self {
        Self::none()
    }
}

// ============================================================================
// Pull Sync Types
// ============================================================================

/// Reading status from a pull sync.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PullSyncStatus {
    WantToRead,
    Reading,
    Finished,
    DidNotFinish,
}

impl PullSyncStatus {
    /// Convert to our internal status string.
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::WantToRead => "want_to_read",
            Self::Reading => "reading",
            Self::Finished => "finished",
            Self::DidNotFinish => "did_not_finish",
        }
    }
}

/// A book from a provider's pull sync.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PullSyncBook {
    /// External ID from the provider
    pub external_id: String,
    /// Reading status
    pub status: Option<PullSyncStatus>,
    /// Progress 0.0-1.0
    pub progress: Option<f64>,
    /// Rating 1-5
    pub rating: Option<i32>,
    /// When this entry was last updated on the provider
    pub updated_at: Option<String>,
    /// The in-progress read ID (None if no active read)
    pub active_read_id: Option<i64>,
}

// ============================================================================
// Configuration Schema
// ============================================================================

/// Configuration field types for the UI
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConfigFieldType {
    /// Generic text input
    String,
    /// API key, password (rendered as password input in UI)
    Secret,
    /// Toggle
    Boolean,
    /// URL with validation
    Url,
}

/// A configuration field that a provider accepts
#[derive(Debug, Clone, Serialize)]
pub struct ConfigField {
    /// Field key (e.g., "api_key")
    pub key: &'static str,
    /// Human-readable label (e.g., "API Key")
    pub label: &'static str,
    /// Field type for UI rendering
    pub field_type: ConfigFieldType,
    /// Whether the field is required
    pub required: bool,
    /// Help text describing the field
    pub description: Option<&'static str>,
    /// URL where user can get/generate the value
    pub help_url: Option<&'static str>,
}

/// Static information about a sync provider
#[derive(Debug, Clone, Serialize)]
pub struct SyncProviderInfo {
    /// Unique provider identifier (e.g., "hardcover")
    pub id: &'static str,
    /// Human-readable name (e.g., "Hardcover")
    pub display_name: &'static str,
    /// Description of the provider
    pub description: &'static str,
    /// URL to the provider's website
    pub website: &'static str,
    /// Capabilities this provider supports
    pub capabilities: SyncCapabilities,
    /// Configuration fields this provider accepts
    pub config_fields: &'static [ConfigField],
}

// ============================================================================
// Sync Actions and Results
// ============================================================================

/// Actions that can be synced to a provider
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum SyncAction {
    /// Book started reading
    Started,
    /// Book finished
    Finished,
    /// Reading progress updated
    Progress {
        percent: f64,
        /// Total page count for the book (if known). Used to calculate progress_pages.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        page_count: Option<i32>,
        /// Date reading started (YYYY-MM-DD). Sent to Hardcover as started_at.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        started_at: Option<String>,
    },
    /// Rating set
    Rating { stars: i32 },
}

impl SyncAction {
    pub fn action_name(&self) -> &'static str {
        match self {
            SyncAction::Started => "started",
            SyncAction::Finished => "finished",
            SyncAction::Progress { .. } => "progress",
            SyncAction::Rating { .. } => "rating",
        }
    }
}

/// Result from a sync operation
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SyncResult {
    /// Whether the sync was successful
    pub success: bool,
    /// Optional user book ID from the provider (for subsequent operations)
    pub user_book_id: Option<String>,
    /// Optional read ID from the provider (Hardcover user_book_read ID)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub read_id: Option<i64>,
    /// Optional message
    pub message: Option<String>,
    /// Raw response from provider (for debugging)
    pub raw_response: Option<serde_json::Value>,
}

impl SyncResult {
    pub fn success() -> Self {
        Self {
            success: true,
            user_book_id: None,
            read_id: None,
            message: None,
            raw_response: None,
        }
    }

    pub fn with_user_book_id(mut self, id: impl Into<String>) -> Self {
        self.user_book_id = Some(id.into());
        self
    }

    pub fn with_read_id(mut self, id: i64) -> Self {
        self.read_id = Some(id);
        self
    }

    pub fn with_message(mut self, msg: impl Into<String>) -> Self {
        self.message = Some(msg.into());
        self
    }

    pub fn with_raw_response(mut self, response: serde_json::Value) -> Self {
        self.raw_response = Some(response);
        self
    }
}

/// Search result for manual book linking
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SyncBookResult {
    /// External ID from the provider
    pub external_id: String,
    /// Book title
    pub title: String,
    /// Authors
    pub authors: Vec<String>,
    /// Publication year
    pub year: Option<String>,
    /// Cover image URL
    pub cover_url: Option<String>,
    /// Confidence score
    pub confidence: f64,
}

// ============================================================================
// Errors
// ============================================================================

/// Errors from sync providers
#[derive(Debug, thiserror::Error)]
pub enum SyncError {
    #[error("HTTP request failed: {0}")]
    Http(#[from] reqwest::Error),

    #[error("Failed to parse response: {0}")]
    Parse(String),

    #[error("Provider rate limited")]
    RateLimited,

    #[error("Book not linked to provider")]
    BookNotLinked,

    #[error("Unknown provider: {0}")]
    UnknownProvider(String),

    #[error("API key required")]
    ApiKeyRequired,

    #[error("Provider error: {0}")]
    Other(String),
}

impl SyncError {
    /// Convert an HTTP error response into the appropriate `SyncError`.
    ///
    /// Returns `RateLimited` for 429 status codes so backoff can engage.
    pub fn from_http_status(status: reqwest::StatusCode, context: &str) -> Self {
        if status == reqwest::StatusCode::TOO_MANY_REQUESTS {
            Self::RateLimited
        } else {
            Self::Other(format!("{context}: {status}"))
        }
    }
}
