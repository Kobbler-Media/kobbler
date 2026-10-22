use serde::{Deserialize, Serialize};

// ============================================================================
// Capability System
// ============================================================================

/// Capabilities that a video metadata provider can support.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum VideoCapability {
    /// Can search for movies by title
    MovieSearch,
    /// Can search for TV shows by title
    TvSearch,
    /// Can fetch full movie details (credits, images)
    MovieDetails,
    /// Can fetch full TV show details
    TvDetails,
    /// Can fetch season/episode details
    SeasonDetails,
    /// Can fetch images (posters, backdrops, stills)
    ImageFetch,
    /// Provides ratings/scores from external sources
    Ratings,
    /// Can fetch person bio, dates, identifiers
    PersonDetails,
    /// Can fetch person's full filmography (combined credits)
    PersonCombinedCredits,
    /// Can fetch per-episode credits (guest stars, episode crew)
    EpisodeCredits,
}

/// Set of capabilities for a video provider (const-compatible).
#[derive(Debug, Clone)]
pub struct VideoCapabilitySet {
    capabilities: &'static [VideoCapability],
}

impl VideoCapabilitySet {
    pub const fn new(capabilities: &'static [VideoCapability]) -> Self {
        Self { capabilities }
    }

    pub fn has(&self, cap: VideoCapability) -> bool {
        self.capabilities.contains(&cap)
    }

    pub fn to_vec(&self) -> Vec<VideoCapability> {
        self.capabilities.to_vec()
    }
}

impl Serialize for VideoCapabilitySet {
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

/// Configuration field types for the UI (reuses same concept as metadata).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum VideoConfigFieldType {
    String,
    Secret,
    Url,
}

/// A configuration field that a video provider accepts.
#[derive(Debug, Clone, Serialize)]
pub struct VideoConfigField {
    pub key: &'static str,
    pub label: &'static str,
    pub field_type: VideoConfigFieldType,
    pub required: bool,
    pub description: Option<&'static str>,
    pub help_url: Option<&'static str>,
    pub default: Option<&'static str>,
}

/// Static information about a video metadata provider.
#[derive(Debug, Clone, Serialize)]
pub struct VideoProviderInfo {
    pub id: &'static str,
    pub display_name: &'static str,
    pub description: &'static str,
    pub website: &'static str,
    pub capabilities: VideoCapabilitySet,
    pub config_fields: &'static [VideoConfigField],
    /// Default rate limit in requests per minute. Used for proactive
    /// throttling to stay under provider limits without hitting 429s.
    pub requests_per_minute: u32,
}

// ============================================================================
// Query Types
// ============================================================================

/// Query parameters for movie search.
#[derive(Debug, Clone, Default)]
pub struct MovieQuery {
    pub title: String,
    pub year: Option<u16>,
}

impl MovieQuery {
    pub fn cache_key(&self) -> String {
        let mut key = format!("video_movie:{}", self.title.to_lowercase());
        if let Some(year) = self.year {
            key.push_str(&format!(":y{}", year));
        }
        key
    }
}

/// Query parameters for TV show search.
#[derive(Debug, Clone, Default)]
pub struct TvQuery {
    pub title: String,
    /// First-aired year, when the caller knows it. A provider that has a year
    /// field takes it there; a title with "(2026)" still in it matches nothing.
    pub year: Option<i32>,
}

impl TvQuery {
    pub fn cache_key(&self) -> String {
        let mut key = format!("video_tv:{}", self.title.to_lowercase());
        if let Some(year) = self.year {
            key.push_str(&format!(":y{}", year));
        }
        key
    }
}

// ============================================================================
// Search Result Types
// ============================================================================

/// Result from a movie search.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MovieResult {
    pub provider: String,
    #[serde(alias = "external_id", rename(serialize = "external_id"))]
    pub provider_id: String,
    pub title: String,
    pub year: Option<u16>,
    pub description: Option<String>,
    pub poster_url: Option<String>,
    #[serde(default)]
    pub backdrop_url: Option<String>,
    pub rating: Option<f64>,
    pub rating_count: Option<i32>,
    #[serde(default)]
    pub identifiers: VideoIdentifiers,
    #[serde(default)]
    pub genres: Vec<String>,
    pub network: Option<String>,
}

/// Result from a TV show search.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TvShowResult {
    pub provider: String,
    #[serde(alias = "external_id", rename(serialize = "external_id"))]
    pub provider_id: String,
    pub title: String,
    pub year: Option<u16>,
    pub status: Option<String>,
    pub description: Option<String>,
    pub poster_url: Option<String>,
    #[serde(default)]
    pub backdrop_url: Option<String>,
    pub network: Option<String>,
    pub rating: Option<f64>,
    pub rating_count: Option<i32>,
    #[serde(default)]
    pub identifiers: VideoIdentifiers,
    #[serde(default)]
    pub genres: Vec<String>,
}

// ============================================================================
// Detail Types
// ============================================================================

/// A cast member for a movie or TV show.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CastMember {
    pub name: String,
    pub character: Option<String>,
    pub profile_url: Option<String>,
    pub order: Option<i32>,
    /// External identifier (e.g., TMDB person ID)
    pub external_id: Option<String>,
}

/// A crew member for a movie or TV show.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CrewMember {
    pub name: String,
    pub department: String,
    pub job: String,
    pub profile_url: Option<String>,
    pub external_id: Option<String>,
}

/// An image associated with a movie or TV show.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MediaImage {
    pub url: String,
    pub image_type: String,
    pub width: Option<u32>,
    pub height: Option<u32>,
}

/// External identifiers for a movie or TV show.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct VideoIdentifiers {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tmdb_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub imdb_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tvmaze_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tvdb_id: Option<String>,
}

/// Full movie details from a provider.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MovieDetails {
    pub provider: String,
    pub provider_id: String,
    pub title: String,
    pub original_title: Option<String>,
    pub year: Option<u16>,
    pub runtime: Option<u32>,
    pub description: Option<String>,
    pub tagline: Option<String>,
    pub poster_url: Option<String>,
    pub backdrop_url: Option<String>,
    pub logo_url: Option<String>,
    pub identifiers: VideoIdentifiers,
    pub genres: Vec<String>,
    pub rating: Option<f64>,
    pub rating_count: Option<i32>,
    pub content_rating: Option<String>,
    pub release_date: Option<String>,
    #[serde(default, skip_serializing_if = "std::collections::BTreeMap::is_empty")]
    pub release_dates_by_region: super::release_dates::ReleaseDatesByRegion,
    pub cast: Vec<CastMember>,
    pub crew: Vec<CrewMember>,
    pub images: Vec<MediaImage>,
}

/// Summary of a season within TV show details.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SeasonSummary {
    pub season_number: u32,
    pub episode_count: Option<u32>,
    pub air_date: Option<String>,
    pub poster_url: Option<String>,
    pub title: Option<String>,
}

/// Full TV show details from a provider.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TvShowDetails {
    pub provider: String,
    pub provider_id: String,
    pub title: String,
    pub year: Option<u16>,
    pub status: Option<String>,
    pub description: Option<String>,
    pub poster_url: Option<String>,
    pub backdrop_url: Option<String>,
    pub logo_url: Option<String>,
    pub identifiers: VideoIdentifiers,
    pub genres: Vec<String>,
    pub rating: Option<f64>,
    pub rating_count: Option<i32>,
    pub network: Option<String>,
    pub content_rating: Option<String>,
    pub first_aired: Option<String>,
    pub runtime: Option<u32>,
    pub seasons: Vec<SeasonSummary>,
    pub cast: Vec<CastMember>,
    pub crew: Vec<CrewMember>,
    pub images: Vec<MediaImage>,
}

/// Per-episode credits: the people specific to one episode rather than the
/// series regulars carried on `TvShowDetails`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct EpisodeCredits {
    #[serde(default)]
    pub guest_stars: Vec<CastMember>,
    #[serde(default)]
    pub crew: Vec<CrewMember>,
}

/// An episode within a season.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EpisodeDetails {
    pub episode_number: u32,
    pub title: Option<String>,
    pub air_date: Option<String>,
    /// Full broadcast instant (RFC 3339) when the provider publishes one.
    /// `None` means the day is known but the time is not — never synthesise it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub air_time: Option<String>,
    pub runtime: Option<u32>,
    pub still_url: Option<String>,
    pub description: Option<String>,
    /// Provider-specific episode/event ID (TheSportsDB id_event, TMDB episode ID, etc.)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider_id: Option<String>,
    /// Country where the event takes place (from TheSportsDB str_country).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub country: Option<String>,
    /// Credits carried on the season payload itself, where the provider
    /// includes them (TMDB does). Empty is not "none" — see `EpisodeCredits`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub credits: Option<EpisodeCredits>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sport: Option<SportEventSource>,
}

/// Venue and city are patchy per row, so readers merge them across a round.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct SportEventSource {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub venue: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub city: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub alternate_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub filename: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thesportsdb_id: Option<String>,
}

/// Full season details from a provider.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SeasonDetails {
    pub provider: String,
    pub show_id: String,
    pub season_number: u32,
    pub title: Option<String>,
    pub air_date: Option<String>,
    pub description: Option<String>,
    pub poster_url: Option<String>,
    pub episodes: Vec<EpisodeDetails>,
}

// ============================================================================
// Person Types
// ============================================================================

/// Person details from a provider (bio, dates, identifiers).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PersonProviderDetails {
    pub name: String,
    pub biography: Option<String>,
    pub birthday: Option<String>,
    pub deathday: Option<String>,
    pub place_of_birth: Option<String>,
    pub known_for_department: Option<String>,
    pub profile_url: Option<String>,
    pub identifiers: std::collections::HashMap<String, String>,
}

/// Combined credits (filmography) for a person.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PersonCombinedCredits {
    pub cast: Vec<PersonCreditCast>,
    pub crew: Vec<PersonCreditCrew>,
}

/// A cast (acting) credit for a person.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PersonCreditCast {
    pub provider_id: String,
    pub media_type: String,
    pub title: String,
    pub character: Option<String>,
    pub poster_url: Option<String>,
    pub release_date: Option<String>,
    pub vote_average: Option<f64>,
    pub vote_count: Option<i64>,
}

/// A crew credit for a person.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PersonCreditCrew {
    pub provider_id: String,
    pub media_type: String,
    pub title: String,
    pub department: Option<String>,
    pub job: Option<String>,
    pub poster_url: Option<String>,
    pub release_date: Option<String>,
}

// ============================================================================
// Stored Provider (DB row)
// ============================================================================

/// Stored provider configuration from the database.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StoredVideoProvider {
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

/// Update request for video provider configuration.
#[derive(Debug, Clone, Deserialize)]
pub struct UpdateVideoProviderRequest {
    pub enabled: Option<bool>,
    pub priority: Option<i32>,
    pub config: Option<serde_json::Map<String, serde_json::Value>>,
}

// ============================================================================
// Search Outcome Types (cache-aware return wrappers)
// ============================================================================

/// Result of a movie search, including cache-hit information.
pub struct MovieSearchOutcome {
    pub results: Vec<MovieResult>,
    /// True when every provider returned results from cache (no external API calls made).
    pub all_cached: bool,
}

/// Result of a TV show search, including cache-hit information.
pub struct TvSearchOutcome {
    pub results: Vec<TvShowResult>,
    /// True when every provider returned results from cache (no external API calls made).
    pub all_cached: bool,
}
