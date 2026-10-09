use serde::{Deserialize, Serialize};

// ============================================================================
// Capability System
// ============================================================================

/// Capabilities that a metadata provider can support
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Capability {
    /// Can search by ISBN (ISBN-10 or ISBN-13)
    IsbnLookup,
    /// Can search by title
    TitleSearch,
    /// Can search by author
    AuthorSearch,
    /// Can fetch cover images
    CoverFetch,
    /// Returns series information
    SeriesInfo,
    /// Returns genre/subject information
    SubjectInfo,
    /// Returns detailed descriptions
    Descriptions,
    /// Returns page count
    PageCount,
    /// Returns publisher information
    PublisherInfo,
    /// Returns publication date
    PublishDate,
    /// Returns language information
    LanguageInfo,
    /// Can search for series by name
    SeriesSearch,
    /// Can look up by ASIN
    AsinLookup,
    /// Returns narrator information (audiobooks)
    NarratorInfo,
    /// Returns chapter information (audiobooks)
    ChapterInfo,
    /// Returns duration information (audiobooks)
    DurationInfo,
    /// Can search by free-text content excerpt
    FreeTextSearch,
}

/// Set of capabilities for a provider (const-compatible)
#[derive(Debug, Clone)]
pub struct CapabilitySet {
    // Using a slice for const compatibility in static declarations
    capabilities: &'static [Capability],
}

impl CapabilitySet {
    /// Create a new capability set with the given capabilities
    pub const fn new(capabilities: &'static [Capability]) -> Self {
        Self { capabilities }
    }

    /// Create an empty capability set
    pub const fn empty() -> Self {
        Self { capabilities: &[] }
    }

    /// Check if this set has a specific capability
    pub fn has(&self, cap: Capability) -> bool {
        self.capabilities.contains(&cap)
    }

    /// Iterate over capabilities
    pub fn iter(&self) -> impl Iterator<Item = &Capability> {
        self.capabilities.iter()
    }

    /// Convert to a Vec for serialization
    pub fn to_vec(&self) -> Vec<Capability> {
        self.capabilities.to_vec()
    }
}

impl Default for CapabilitySet {
    fn default() -> Self {
        Self::empty()
    }
}

impl Serialize for CapabilitySet {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        self.to_vec().serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for CapabilitySet {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let caps: Vec<Capability> = Vec::deserialize(deserializer)?;
        // For deserialized data, we leak it to get a static slice
        // This is acceptable since these are typically used for static data
        let leaked: &'static [Capability] = Box::leak(caps.into_boxed_slice());
        Ok(CapabilitySet {
            capabilities: leaked,
        })
    }
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
    /// Numbers
    Integer,
    /// Toggle
    Boolean,
    /// URL with validation
    Url,
}

/// A configuration field that a provider accepts
#[derive(Debug, Clone, Serialize)]
pub struct ConfigField {
    /// Field key (e.g., "api_key", "base_url")
    pub key: &'static str,
    /// Human-readable label (e.g., "API Key", "Base URL")
    pub label: &'static str,
    /// Field type for UI rendering
    pub field_type: ConfigFieldType,
    /// Whether the field is required
    pub required: bool,
    /// Help text describing the field
    pub description: Option<&'static str>,
    /// URL where user can get/generate the value
    pub help_url: Option<&'static str>,
    /// Default value if not configured
    pub default: Option<&'static str>,
}

/// Static information about a metadata provider
#[derive(Debug, Clone, Serialize)]
pub struct ProviderInfo {
    /// Unique provider identifier (e.g., "openlibrary", "googlebooks")
    pub id: &'static str,
    /// Human-readable name (e.g., "Open Library", "Google Books")
    pub display_name: &'static str,
    /// Description of the provider
    pub description: &'static str,
    /// URL to the provider's website
    pub website: &'static str,
    /// Capabilities this provider supports
    pub capabilities: CapabilitySet,
    /// Configuration fields this provider accepts
    pub config_fields: &'static [ConfigField],
    /// Default rate limit in requests per minute. Used for proactive
    /// throttling to stay under provider limits without hitting 429s.
    pub requests_per_minute: u32,
}

/// A chapter in an audiobook (with timing information)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Chapter {
    /// Chapter title
    pub title: String,
    /// Start offset in milliseconds
    pub start_ms: u64,
    /// End offset in milliseconds
    pub end_ms: u64,
}

/// Author with provider ID for enrichment
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuthorWithId {
    pub name: String,
    /// Provider ID for this author (e.g., Hardcover author ID)
    pub provider_id: Option<String>,
}

/// A series membership for a book (internal metadata passing, not serialized)
#[derive(Debug, Clone, Default)]
pub struct SeriesInfo {
    pub name: String,
    pub provider_id: Option<String>,
    pub index: Option<f64>,
}

/// Result from a metadata provider search
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct MetadataResult {
    /// Provider that returned this result
    pub provider: String,
    /// Book title
    pub title: String,
    /// Authors
    pub authors: Vec<String>,
    /// Authors with provider IDs for enrichment
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub authors_with_ids: Vec<AuthorWithId>,
    /// ISBN-10 or ISBN-13
    pub isbn: Option<String>,
    /// Book description/summary
    pub description: Option<String>,
    /// Publisher name
    pub publisher: Option<String>,
    /// Publication date (various formats accepted)
    pub publish_date: Option<String>,
    /// Language code (e.g., "en", "de")
    pub language: Option<String>,
    /// Number of pages
    pub page_count: Option<u32>,
    /// Cover image URL
    pub cover_url: Option<String>,
    /// Series name if part of a series
    pub series: Option<String>,
    /// Provider ID for the series (for enrichment)
    pub series_provider_id: Option<String>,
    /// Position in series
    pub series_index: Option<f64>,
    /// All series this book belongs to (superset of the single series/series_index fields)
    #[serde(skip)]
    pub series_list: Vec<SeriesInfo>,
    /// Subjects/genres/categories
    pub subjects: Vec<String>,
    /// External ID from the provider (for fetching covers later)
    pub external_id: Option<String>,
    /// Confidence score (0.0 - 1.0) for ranking results
    pub confidence: f64,
    /// Average rating (0.0 - 5.0 scale)
    pub rating: Option<f64>,
    /// Number of ratings
    pub rating_count: Option<i32>,
    /// Narrator(s) for audiobooks (joined into a single string)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub narrator: Option<String>,
    /// Duration in seconds for audiobooks
    #[serde(skip_serializing_if = "Option::is_none")]
    pub duration_seconds: Option<u64>,
    /// Chapter list with timing information for audiobooks
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub chapters: Vec<Chapter>,
    /// ASIN (Amazon Standard Identification Number) for audiobooks
    #[serde(skip_serializing_if = "Option::is_none")]
    pub asin: Option<String>,
    /// Aggregated identifiers from all providers (for merged results)
    #[serde(default, skip_serializing_if = "is_null_or_empty_value")]
    pub identifiers: serde_json::Value,
}

/// Helper function to check if a serde_json::Value is null or empty
fn is_null_or_empty_value(v: &serde_json::Value) -> bool {
    v.is_null() || (v.is_object() && v.as_object().map(|o| o.is_empty()).unwrap_or(false))
}

impl MetadataResult {
    pub fn new(provider: &str, title: String) -> Self {
        Self {
            provider: provider.to_string(),
            title,
            authors: Vec::new(),
            authors_with_ids: Vec::new(),
            isbn: None,
            description: None,
            publisher: None,
            publish_date: None,
            language: None,
            page_count: None,
            cover_url: None,
            series: None,
            series_provider_id: None,
            series_index: None,
            series_list: Vec::new(),
            subjects: Vec::new(),
            external_id: None,
            confidence: 0.5,
            rating: None,
            rating_count: None,
            narrator: None,
            duration_seconds: None,
            chapters: Vec::new(),
            asin: None,
            identifiers: serde_json::Value::Null,
        }
    }
}

/// Query parameters for metadata search
#[derive(Debug, Clone, Default)]
pub struct MetadataQuery {
    /// Search by ISBN
    pub isbn: Option<String>,
    /// Search by title
    pub title: Option<String>,
    /// Search by author name(s) — all are checked in relevance scoring,
    /// but only the first is used for provider search queries.
    pub authors: Vec<String>,
    /// Search by ASIN (Amazon Standard Identification Number)
    pub asin: Option<String>,
    /// Search by free-text content excerpt (for content-based identification)
    pub free_text: Option<String>,
}

impl MetadataQuery {
    pub fn by_isbn(isbn: &str) -> Self {
        Self {
            isbn: Some(isbn.to_string()),
            ..Default::default()
        }
    }

    pub fn by_title(title: &str) -> Self {
        Self {
            title: Some(title.to_string()),
            ..Default::default()
        }
    }

    pub fn by_title_author(title: &str, author: &str) -> Self {
        Self {
            title: Some(title.to_string()),
            authors: vec![author.to_string()],
            ..Default::default()
        }
    }

    /// Generate a cache key for this query
    pub fn cache_key(&self) -> String {
        let mut parts = Vec::new();
        if let Some(isbn) = &self.isbn {
            parts.push(format!("isbn:{}", isbn.to_lowercase().replace("-", "")));
        }
        if let Some(title) = &self.title {
            parts.push(format!("title:{}", title.to_lowercase()));
        }
        if let Some(author) = self.authors.first() {
            parts.push(format!("author:{}", author.to_lowercase()));
        }
        if let Some(asin) = &self.asin {
            parts.push(format!("asin:{}", asin.to_uppercase()));
        }
        if let Some(text) = &self.free_text {
            use std::hash::{Hash, Hasher};
            let mut hasher = std::collections::hash_map::DefaultHasher::new();
            text.hash(&mut hasher);
            parts.push(format!("freetext:{:x}", hasher.finish()));
        }
        parts.join("|")
    }

    pub fn by_free_text(text: &str) -> Self {
        Self {
            free_text: Some(text.to_string()),
            ..Default::default()
        }
    }
}

/// Stored provider configuration (from database)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StoredProvider {
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

impl StoredProvider {
    /// Get a config value by key
    pub fn get_config<T: serde::de::DeserializeOwned>(&self, key: &str) -> Option<T> {
        self.config
            .get(key)
            .and_then(|v| serde_json::from_value(v.clone()).ok())
    }

    /// Get a string config value
    pub fn get_config_str(&self, key: &str) -> Option<String> {
        self.config
            .get(key)
            .and_then(|v| v.as_str())
            .map(String::from)
    }
}

/// Update request for provider configuration
#[derive(Debug, Clone, Deserialize)]
pub struct UpdateProviderRequest {
    pub enabled: Option<bool>,
    pub priority: Option<i32>,
    /// Generic config updates - merged with existing config
    pub config: Option<serde_json::Map<String, serde_json::Value>>,
}

// ============================================================================
// Series Search Types
// ============================================================================

/// Query parameters for series search
#[derive(Debug, Clone, Default)]
pub struct SeriesQuery {
    /// Series name to search for
    pub name: String,
}

impl SeriesQuery {
    pub fn new(name: &str) -> Self {
        Self {
            name: name.to_string(),
        }
    }

    /// Generate a cache key for this query
    pub fn cache_key(&self) -> String {
        format!("series:{}", self.name.to_lowercase())
    }
}

/// A book within a series search result
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SeriesBook {
    /// Book title
    pub title: String,
    /// Authors
    pub authors: Vec<String>,
    /// Position in series (1.0, 2.0, etc.)
    pub series_index: Option<f64>,
    /// ISBN if available
    pub isbn: Option<String>,
    /// Cover image URL if available
    pub cover_url: Option<String>,
    /// Publication date
    pub publish_date: Option<String>,
    /// External ID from the provider
    pub external_id: Option<String>,
}

/// Result from a series search
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SeriesSearchResult {
    /// Provider that returned this result
    pub provider: String,
    /// Series name
    pub name: String,
    /// Primary author(s) of the series
    pub authors: Vec<String>,
    /// Series description if available
    pub description: Option<String>,
    /// Total number of books in the series
    pub book_count: Option<u32>,
    /// Series image URL if available
    pub image_url: Option<String>,
    /// Books in this series (ordered by series_index)
    pub books: Vec<SeriesBook>,
    /// External ID from the provider
    pub external_id: Option<String>,
    /// Slug for URL generation (e.g., Hardcover website URLs)
    pub slug: Option<String>,
    /// Confidence score (0.0 - 1.0) for ranking results
    pub confidence: f64,
}

impl SeriesSearchResult {
    pub fn new(provider: &str, name: String) -> Self {
        Self {
            provider: provider.to_string(),
            name,
            authors: Vec::new(),
            description: None,
            book_count: None,
            image_url: None,
            books: Vec::new(),
            external_id: None,
            slug: None,
            confidence: 0.5,
        }
    }
}

// ============================================================================
// Search Outcome Types (cache-aware return wrappers)
// ============================================================================

/// Result of a metadata search, including cache-hit information.
pub struct SearchOutcome {
    pub results: Vec<MetadataResult>,
    /// True when every provider returned results from cache (no external API calls made).
    pub all_cached: bool,
}

impl SearchOutcome {
    /// Adjust result confidence based on how well each result matches the
    /// original query. Provider confidence reflects data completeness, not
    /// query relevance — this method adds relevance signals.
    ///
    /// 1. Title mismatch → 0.3x penalty (strong: wrong title = likely wrong book)
    /// 2. Author mismatch → 0.5x penalty (moderate: could be pseudonym/editor)
    /// 3. Title+author match with low confidence → boost to 0.8
    /// 4. Re-sort by adjusted confidence
    pub fn adjust_relevance(&mut self, query: &MetadataQuery) {
        use super::similarity::{fuzzy_match, fuzzy_match_title, strip_punctuation};

        if self.results.is_empty() {
            return;
        }

        let query_title_norm = query
            .title
            .as_ref()
            .map(|t| strip_punctuation(&t.to_lowercase()));
        let query_authors_norm: Vec<String> = query
            .authors
            .iter()
            .map(|a| strip_punctuation(&a.to_lowercase()))
            .collect();

        for result in &mut self.results {
            let result_title_norm = strip_punctuation(&result.title.to_lowercase());

            let title_matches = query_title_norm
                .as_ref()
                .is_some_and(|qt| fuzzy_match_title(&result_title_norm, qt));

            // Any query author matching any result author counts as a match
            let author_matches = !query_authors_norm.is_empty()
                && query_authors_norm.iter().any(|qa| {
                    result.authors.iter().any(|a| {
                        let a_norm = strip_punctuation(&a.to_lowercase());
                        fuzzy_match(&a_norm, qa)
                    })
                });

            // Penalize title mismatch (only when query has a title to compare)
            if query_title_norm.is_some() && !title_matches {
                result.confidence *= 0.3;
            }

            // Penalize author mismatch (only when query has an author to compare)
            if !query_authors_norm.is_empty() && !author_matches {
                result.confidence *= 0.5;
            }

            // Boost correct but sparse matches to the review threshold
            if title_matches && author_matches && result.confidence < 0.8 {
                result.confidence = 0.8;
            }
        }

        // Re-sort after adjustments
        self.results.sort_by(|a, b| {
            b.confidence
                .partial_cmp(&a.confidence)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
    }

    /// Boost confidence of results that have series data when they fuzzy-match
    /// another result on title+author. This lets richer results (e.g. Hardcover
    /// with series info) outrank sparser ones (e.g. OpenLibrary without series)
    /// when they're clearly the same book. Re-sorts results after boosting.
    pub fn boost_series_matches(&mut self) {
        use super::similarity::{fuzzy_match, fuzzy_match_title, strip_punctuation};

        if self.results.len() < 2 {
            return;
        }

        // Collect normalized titles+authors for comparison
        let normalized: Vec<(String, Vec<String>)> = self
            .results
            .iter()
            .map(|r| {
                let title = strip_punctuation(&r.title.to_lowercase());
                let authors: Vec<String> = r
                    .authors
                    .iter()
                    .map(|a| strip_punctuation(&a.to_lowercase()))
                    .collect();
                (title, authors)
            })
            .collect();

        // For each result with series, check if any other result matches on
        // title+author but lacks series. If so, the series-bearing result gets
        // a small boost to prefer richer metadata.
        for i in 0..self.results.len() {
            if self.results[i].series.is_none() {
                continue;
            }
            let has_peer_without_series = (0..self.results.len()).any(|j| {
                j != i
                    && self.results[j].series.is_none()
                    && fuzzy_match_title(&normalized[j].0, &normalized[i].0)
                    && (normalized[i].1.is_empty()
                        || normalized[j]
                            .1
                            .iter()
                            .any(|ja| normalized[i].1.iter().any(|ia| fuzzy_match(ja, ia))))
            });
            if has_peer_without_series {
                self.results[i].confidence = (self.results[i].confidence + 0.05).min(1.0);
            }
        }

        // Re-sort so boosted results can overtake peers
        self.results.sort_by(|a, b| {
            b.confidence
                .partial_cmp(&a.confidence)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
    }
}

/// Result of a series search, including cache-hit information.
pub struct SeriesSearchOutcome {
    pub results: Vec<SeriesSearchResult>,
    /// True when every provider returned results from cache (no external API calls made).
    pub all_cached: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_result(provider: &str, title: &str, authors: &[&str]) -> MetadataResult {
        let mut r = MetadataResult::new(provider, title.to_string());
        r.authors = authors.iter().map(|a| a.to_string()).collect();
        r.confidence = 0.9;
        r
    }

    fn make_outcome(results: Vec<MetadataResult>) -> SearchOutcome {
        SearchOutcome {
            results,
            all_cached: false,
        }
    }

    #[test]
    fn boost_series_promotes_matching_result() {
        // OL result at 0.9 without series, HC result at 0.88 with series
        let top = make_result("openlibrary", "The Final Empire", &["Brandon Sanderson"]);

        let mut peer = make_result("hardcover", "The Final Empire", &["Brandon Sanderson"]);
        peer.series = Some("Mistborn".to_string());
        peer.series_provider_id = Some("42".to_string());
        peer.series_index = Some(1.0);
        peer.confidence = 0.88;

        let mut outcome = make_outcome(vec![top, peer]);
        outcome.boost_series_matches();

        // HC result should now be on top (0.88 + 0.05 = 0.93 > 0.9)
        assert_eq!(outcome.results[0].provider, "hardcover");
        assert_eq!(outcome.results[0].series.as_deref(), Some("Mistborn"));
    }

    #[test]
    fn boost_series_no_boost_when_no_peer_without_series() {
        // Both results have series — no boost needed
        let mut a = make_result("hardcover", "Mistborn", &["Brandon Sanderson"]);
        a.series = Some("Cosmere".to_string());

        let mut b = make_result("openlibrary", "Mistborn", &["Brandon Sanderson"]);
        b.series = Some("Cosmere".to_string());
        b.confidence = 0.8;

        let mut outcome = make_outcome(vec![a, b]);
        let conf_before = outcome.results[0].confidence;
        outcome.boost_series_matches();

        assert_eq!(outcome.results[0].confidence, conf_before);
    }

    #[test]
    fn boost_series_no_boost_for_different_title() {
        let top = make_result("openlibrary", "The Final Empire", &["Brandon Sanderson"]);

        let mut peer = make_result("hardcover", "Elantris", &["Brandon Sanderson"]);
        peer.series = Some("Cosmere".to_string());
        peer.confidence = 0.88;

        let mut outcome = make_outcome(vec![top, peer]);
        outcome.boost_series_matches();

        // Different title — no boost, OL stays on top
        assert_eq!(outcome.results[0].provider, "openlibrary");
    }

    #[test]
    fn boost_series_no_boost_for_different_author() {
        let top = make_result("openlibrary", "Mistborn", &["Brandon Sanderson"]);

        let mut peer = make_result("hardcover", "Mistborn", &["Terry Pratchett"]);
        peer.series = Some("Discworld".to_string());
        peer.confidence = 0.88;

        let mut outcome = make_outcome(vec![top, peer]);
        outcome.boost_series_matches();

        // Different author — no boost, OL stays on top
        assert_eq!(outcome.results[0].provider, "openlibrary");
    }

    #[test]
    fn boost_series_empty_and_single() {
        let mut empty = make_outcome(vec![]);
        empty.boost_series_matches(); // no panic

        let top = make_result("openlibrary", "Mistborn", &["Brandon Sanderson"]);
        let mut single = make_outcome(vec![top]);
        single.boost_series_matches(); // no panic
    }

    #[test]
    fn boost_series_fuzzy_author_match() {
        // Reordered name should still trigger boost
        let top = make_result("openlibrary", "The Final Empire", &["Sanderson, Brandon"]);

        let mut peer = make_result("hardcover", "The Final Empire", &["Brandon Sanderson"]);
        peer.series = Some("Mistborn".to_string());
        peer.confidence = 0.88;

        let mut outcome = make_outcome(vec![top, peer]);
        outcome.boost_series_matches();

        assert_eq!(outcome.results[0].provider, "hardcover");
    }

    #[test]
    fn boost_series_does_not_overtake_much_higher_confidence() {
        // If the gap is large enough, the series result shouldn't win
        let mut top = make_result("openlibrary", "The Final Empire", &["Brandon Sanderson"]);
        top.confidence = 0.95;

        let mut peer = make_result("hardcover", "The Final Empire", &["Brandon Sanderson"]);
        peer.series = Some("Mistborn".to_string());
        peer.confidence = 0.7;

        let mut outcome = make_outcome(vec![top, peer]);
        outcome.boost_series_matches();

        // 0.7 + 0.05 = 0.75 < 0.95 — OL still wins
        assert_eq!(outcome.results[0].provider, "openlibrary");
    }

    // =========================================================================
    // adjust_relevance tests
    // =========================================================================

    fn make_query(title: Option<&str>, author: Option<&str>) -> MetadataQuery {
        MetadataQuery {
            title: title.map(String::from),
            authors: author.into_iter().map(String::from).collect(),
            ..Default::default()
        }
    }

    #[test]
    fn adjust_title_mismatch_penalty() {
        let mut r = make_result("hardcover", "2001: A Space Odyssey", &["Arthur C. Clarke"]);
        r.confidence = 1.0;
        let mut outcome = make_outcome(vec![r]);

        let query = make_query(Some("Tales from Planet Earth"), Some("Arthur C. Clarke"));
        outcome.adjust_relevance(&query);

        // Title mismatch → 0.3x, author matches → no penalty
        assert!((outcome.results[0].confidence - 0.3).abs() < 0.01);
    }

    #[test]
    fn adjust_author_mismatch_penalty() {
        let mut r = make_result("hardcover", "Mistborn", &["Isaac Asimov"]);
        r.confidence = 1.0;
        let mut outcome = make_outcome(vec![r]);

        let query = make_query(Some("Mistborn"), Some("Brandon Sanderson"));
        outcome.adjust_relevance(&query);

        // Title matches → no penalty, author mismatch → 0.5x
        assert!((outcome.results[0].confidence - 0.5).abs() < 0.01);
    }

    #[test]
    fn adjust_both_mismatch() {
        let mut r = make_result("hardcover", "2001: A Space Odyssey", &["Isaac Asimov"]);
        r.confidence = 1.0;
        let mut outcome = make_outcome(vec![r]);

        let query = make_query(Some("Tales from Planet Earth"), Some("Arthur C. Clarke"));
        outcome.adjust_relevance(&query);

        // 0.3 * 0.5 = 0.15
        assert!((outcome.results[0].confidence - 0.15).abs() < 0.01);
    }

    #[test]
    fn adjust_title_author_boost() {
        let mut r = make_result("openlibrary", "Mistborn", &["Brandon Sanderson"]);
        r.confidence = 0.5;
        let mut outcome = make_outcome(vec![r]);

        let query = make_query(Some("Mistborn"), Some("Brandon Sanderson"));
        outcome.adjust_relevance(&query);

        assert!((outcome.results[0].confidence - 0.8).abs() < 0.01);
    }

    #[test]
    fn adjust_no_penalty_for_correct_match() {
        let mut r = make_result("hardcover", "Ender's Game", &["Orson Scott Card"]);
        r.confidence = 1.0;
        let mut outcome = make_outcome(vec![r]);

        let query = make_query(Some("Ender's Game"), Some("Orson Scott Card"));
        outcome.adjust_relevance(&query);

        assert!((outcome.results[0].confidence - 1.0).abs() < 0.01);
    }

    #[test]
    fn adjust_fuzzy_title_not_penalized() {
        // EPUB title contains series prefix — should still match
        let mut r = make_result("hardcover", "Ender's Game", &["Orson Scott Card"]);
        r.confidence = 1.0;
        let mut outcome = make_outcome(vec![r]);

        let query = make_query(
            Some("Ender's saga #01 - Ender's Game"),
            Some("Orson Scott Card"),
        );
        outcome.adjust_relevance(&query);

        assert!((outcome.results[0].confidence - 1.0).abs() < 0.01);
    }

    #[test]
    fn adjust_empty_results() {
        let mut outcome = make_outcome(vec![]);
        let query = make_query(Some("Anything"), Some("Anyone"));
        outcome.adjust_relevance(&query); // should not panic
    }

    #[test]
    fn adjust_no_author_query() {
        let mut r = make_result("hardcover", "2001: A Space Odyssey", &["Arthur C. Clarke"]);
        r.confidence = 1.0;
        let mut outcome = make_outcome(vec![r]);

        // No author in query — only title penalty applies
        let query = make_query(Some("Tales from Planet Earth"), None);
        outcome.adjust_relevance(&query);

        // Title mismatch → 0.3x, no author penalty
        assert!((outcome.results[0].confidence - 0.3).abs() < 0.01);
    }

    #[test]
    fn adjust_reorders_results() {
        // Wrong book at 1.0, correct book at 0.6
        let mut wrong = make_result(
            "googlebooks",
            "2001: A Space Odyssey",
            &["Arthur C. Clarke"],
        );
        wrong.confidence = 1.0;
        let mut correct = make_result(
            "hardcover",
            "Tales from Planet Earth",
            &["Arthur C. Clarke"],
        );
        correct.confidence = 0.6;

        let mut outcome = make_outcome(vec![wrong, correct]);
        let query = make_query(Some("Tales from Planet Earth"), Some("Arthur C. Clarke"));
        outcome.adjust_relevance(&query);

        // Wrong: 1.0 * 0.3 = 0.3 (title mismatch)
        // Correct: 0.6 → boosted to 0.8 (title+author match)
        assert_eq!(outcome.results[0].provider, "hardcover");
        assert!((outcome.results[0].confidence - 0.8).abs() < 0.01);
    }
}
