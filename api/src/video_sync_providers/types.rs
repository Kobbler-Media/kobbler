//! Types for video sync providers (Trakt, Simkl, etc.)

use serde::{Deserialize, Serialize};

// ============================================================================
// Watch History
// ============================================================================

/// A single watch history item imported from a provider.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WatchHistoryItem {
    /// Provider-specific media ID (e.g., Trakt slug or TMDB ID)
    pub provider_id: String,
    /// Title of the movie/show
    pub title: String,
    /// Year of release
    pub year: Option<i32>,
    /// Media type: "movie" or "episode"
    pub media_type: String,
    /// TMDB ID if available
    pub tmdb_id: Option<String>,
    /// IMDB ID if available
    pub imdb_id: Option<String>,
    /// When the user watched this item
    pub watched_at: Option<String>,
    /// For episodes: show title
    pub show_title: Option<String>,
    /// For episodes: season number
    pub season_number: Option<u32>,
    /// For episodes: episode number
    pub episode_number: Option<u32>,
}

// ============================================================================
// Provider Info
// ============================================================================

/// Static information about a video sync provider.
#[derive(Debug, Clone, Serialize)]
pub struct VideoSyncProviderInfo {
    /// Unique provider identifier (e.g., "trakt")
    pub id: &'static str,
    /// Human-readable name
    pub display_name: &'static str,
    /// Description
    pub description: &'static str,
    /// Website URL
    pub website: &'static str,
    /// Whether this provider uses OAuth (vs. API key)
    pub uses_oauth: bool,
}

// ============================================================================
// Sync Results
// ============================================================================

/// Result from a video sync operation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VideoSyncResult {
    pub success: bool,
    pub message: Option<String>,
    pub items_synced: usize,
}

impl VideoSyncResult {
    pub fn success() -> Self {
        Self {
            success: true,
            message: None,
            items_synced: 0,
        }
    }

    pub fn with_message(mut self, msg: impl Into<String>) -> Self {
        self.message = Some(msg.into());
        self
    }

    pub fn with_count(mut self, count: usize) -> Self {
        self.items_synced = count;
        self
    }
}

// ============================================================================
// Errors
// ============================================================================

/// Errors from video sync providers.
#[derive(Debug, thiserror::Error)]
pub enum VideoSyncError {
    #[error("HTTP request failed: {0}")]
    Http(#[from] reqwest::Error),

    #[error("Failed to parse response: {0}")]
    Parse(String),

    #[error("Provider rate limited")]
    RateLimited,

    #[error("Not authenticated — OAuth required")]
    NotAuthenticated,

    #[error("Token expired and refresh failed")]
    TokenExpired,

    #[error("Unknown provider: {0}")]
    UnknownProvider(String),

    #[error("Provider error: {0}")]
    Other(String),
}

// ============================================================================
// OAuth Device Code Flow
// ============================================================================

/// Device code response from an OAuth provider.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeviceCodeResponse {
    /// The device code for backend polling
    pub device_code: String,
    /// The code the user enters on the verification page
    pub user_code: String,
    /// The URL the user visits to enter the code
    pub verification_url: String,
    /// Seconds until the codes expire
    pub expires_in: u64,
    /// Recommended polling interval in seconds
    pub interval: u64,
}

/// Result of polling for OAuth token.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum PollResult {
    /// Still waiting for user authorization
    Pending,
    /// User authorized — tokens received
    Authorized {
        access_token: String,
        refresh_token: Option<String>,
    },
    /// The device code expired
    Expired,
    /// User denied the request
    Denied,
}
