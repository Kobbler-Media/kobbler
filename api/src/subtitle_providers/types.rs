use serde::{Deserialize, Serialize};

// ============================================================================
// Capability System
// ============================================================================

/// Capabilities that a subtitle provider can support.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SubtitleCapability {
    /// Can search for movie subtitles by title/IDs
    MovieSearch,
    /// Can search for TV episode subtitles
    EpisodeSearch,
    /// Can search by file hash (e.g., OpenSubtitles moviehash)
    HashSearch,
    /// Supports filtering forced/foreign-parts subtitles
    ForcedFilter,
    /// Supports filtering hearing-impaired subtitles
    HearingImpaired,
}

/// Set of capabilities for a subtitle provider (const-compatible).
#[derive(Debug, Clone)]
pub struct SubtitleCapabilitySet {
    capabilities: &'static [SubtitleCapability],
}

impl SubtitleCapabilitySet {
    pub const fn new(capabilities: &'static [SubtitleCapability]) -> Self {
        Self { capabilities }
    }

    pub fn has(&self, cap: SubtitleCapability) -> bool {
        self.capabilities.contains(&cap)
    }

    pub fn to_vec(&self) -> Vec<SubtitleCapability> {
        self.capabilities.to_vec()
    }
}

impl Serialize for SubtitleCapabilitySet {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        self.to_vec().serialize(serializer)
    }
}

// ============================================================================
// Configuration Schema
// ============================================================================

/// Configuration field types for the UI.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SubtitleConfigFieldType {
    String,
    Secret,
    Url,
}

/// A configuration field that a subtitle provider accepts.
#[derive(Debug, Clone, Serialize)]
pub struct SubtitleConfigField {
    pub key: &'static str,
    pub label: &'static str,
    pub field_type: SubtitleConfigFieldType,
    pub required: bool,
    pub description: Option<&'static str>,
    pub help_url: Option<&'static str>,
    pub default: Option<&'static str>,
}

/// Static information about a subtitle provider.
#[derive(Debug, Clone, Serialize)]
pub struct SubtitleProviderInfo {
    pub id: &'static str,
    pub display_name: &'static str,
    pub description: &'static str,
    pub website: &'static str,
    pub capabilities: SubtitleCapabilitySet,
    pub config_fields: &'static [SubtitleConfigField],
}

// ============================================================================
// Query Types
// ============================================================================

/// Query parameters for movie subtitle search.
#[derive(Debug, Clone, Default)]
pub struct MovieSubtitleQuery {
    pub title: String,
    pub year: Option<u16>,
    pub imdb_id: Option<String>,
    pub tmdb_id: Option<String>,
    pub languages: Vec<String>,
    pub forced_only: bool,
}

/// Query parameters for TV episode subtitle search.
#[derive(Debug, Clone, Default)]
pub struct EpisodeSubtitleQuery {
    pub show_title: String,
    pub show_imdb_id: Option<String>,
    pub show_tmdb_id: Option<String>,
    pub season: u32,
    pub episode: u32,
    pub languages: Vec<String>,
    pub forced_only: bool,
}

// ============================================================================
// Result Types
// ============================================================================

/// A subtitle search result from a provider.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SubtitleSearchResult {
    /// Which provider returned this result
    pub provider: String,
    /// Provider-specific ID (e.g., file_id for OpenSubtitles)
    pub id: String,
    /// ISO 639-1/2 language code
    pub language: String,
    /// Subtitle format (srt, ass, sub, etc.)
    pub format: String,
    /// Whether this is a forced/foreign-parts subtitle
    pub is_forced: bool,
    /// Whether this is a hearing-impaired subtitle
    pub is_hearing_impaired: bool,
    /// User rating (0.0 to 10.0)
    pub rating: Option<f64>,
    /// Number of downloads
    pub download_count: Option<i64>,
}

/// Downloaded subtitle content.
#[derive(Debug)]
pub struct SubtitleDownload {
    /// Raw subtitle file content
    pub content: Vec<u8>,
    /// Subtitle format (srt, ass, etc.)
    pub format: String,
    /// Suggested filename
    pub filename: String,
}

// ============================================================================
// Stored Provider (DB row)
// ============================================================================

/// Stored provider configuration from the database.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StoredSubtitleProvider {
    pub id: uuid::Uuid,
    pub provider_type: String,
    pub enabled: bool,
    pub priority: i32,
    #[serde(default)]
    pub config: serde_json::Value,
    #[serde(with = "time::serde::rfc3339")]
    pub created_at: time::OffsetDateTime,
    #[serde(with = "time::serde::rfc3339")]
    pub updated_at: time::OffsetDateTime,
}

/// Update request for subtitle provider configuration.
#[derive(Debug, Clone, Deserialize)]
pub struct UpdateSubtitleProviderRequest {
    pub enabled: Option<bool>,
    pub priority: Option<i32>,
    pub config: Option<serde_json::Map<String, serde_json::Value>>,
}
