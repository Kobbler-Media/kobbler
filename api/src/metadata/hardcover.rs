use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::json;

use super::types::{
    Capability, CapabilitySet, ConfigField, ConfigFieldType, ProviderInfo, SeriesBook,
    SeriesSearchResult,
};
use super::{
    MetadataProvider, MetadataQuery, MetadataResult, ProviderError, ProviderFactory, SeriesQuery,
};

// ============================================================================
// Provider Info
// ============================================================================

/// Configuration fields for Hardcover
pub static CONFIG_FIELDS: &[ConfigField] = &[ConfigField {
    key: "api_key",
    label: "API Key",
    field_type: ConfigFieldType::Secret,
    required: false,
    description: Some("Paste from your Hardcover account settings"),
    help_url: Some("https://hardcover.app/account/api"),
    default: None,
}];

/// Capabilities supported by Hardcover
/// Note: Publisher is on editions table, not books - search API doesn't return it
static CAPABILITIES: &[Capability] = &[
    Capability::IsbnLookup,
    Capability::AsinLookup,
    Capability::TitleSearch,
    Capability::AuthorSearch,
    Capability::CoverFetch,
    Capability::SeriesInfo,
    Capability::SeriesSearch, // Can search for series by name
    Capability::SubjectInfo,
    Capability::Descriptions,
    Capability::PageCount,
    Capability::PublishDate,
];

/// Static provider information
pub static INFO: ProviderInfo = ProviderInfo {
    id: crate::http::provider_ids::HARDCOVER,
    display_name: "Hardcover",
    description: "Community-driven book data with series support. API key required.",
    website: "https://hardcover.app",
    capabilities: CapabilitySet::new(CAPABILITIES),
    config_fields: CONFIG_FIELDS,
    requests_per_minute: 60, // Hardcover documents 60 req/min
};

// ============================================================================
// Factory
// ============================================================================

/// Factory for creating Hardcover provider instances
const DEFAULT_BASE_URL: &str = "https://api.hardcover.app";

fn config_base_url(config: &serde_json::Value) -> Option<String> {
    config
        .get("base_url")
        .and_then(|v| v.as_str())
        .filter(|v| !v.is_empty())
        .map(String::from)
}

pub struct HardcoverFactory {
    base_url_override: Option<String>,
}

impl HardcoverFactory {
    pub fn new(base_url_override: Option<String>) -> Self {
        Self { base_url_override }
    }
}

impl ProviderFactory for HardcoverFactory {
    fn info(&self) -> &'static ProviderInfo {
        &INFO
    }

    fn create(
        &self,
        client: reqwest::Client,
        config: &serde_json::Value,
    ) -> Box<dyn MetadataProvider> {
        let base_url = self
            .base_url_override
            .clone()
            .or_else(|| config_base_url(config))
            .unwrap_or_else(|| DEFAULT_BASE_URL.to_string());

        let api_key = config
            .get("api_key")
            .and_then(|v| v.as_str())
            .map(String::from);

        Box::new(HardcoverProvider {
            client,
            base_url,
            api_key,
        })
    }
}

/// Known noise subtitle patterns (after the colon, lowercased + trimmed).
const NOISE_SUBTITLES_EXACT: &[&str] = &[
    "a novel",
    "a memoir",
    "a thriller",
    "a mystery",
    "a romance",
    "a novella",
    "a story",
    "stories",
    "poems",
    "essays",
    "the novel",
    "the complete edition",
    "the graphic novel",
];

/// Known noise subtitle prefixes (after the colon, lowercased + trimmed).
const NOISE_SUBTITLE_PREFIXES: &[&str] = &["volume ", "book ", "part ", "edition "];

/// Strip a noise subtitle after a colon if it matches known filler patterns.
///
/// Returns the part before the colon if the subtitle is noise, otherwise the
/// original title unchanged. Only strips if the part before the colon has at
/// least one word with 4+ characters.
fn strip_noise_subtitle(title: &str) -> &str {
    let Some(colon_pos) = title.find(':') else {
        return title;
    };

    let before = title[..colon_pos].trim();
    let after = title[colon_pos + 1..].trim();
    let after_lower = after.to_lowercase();

    // Safety: don't strip if the main title part is too short
    if !before.split_whitespace().any(|w| w.len() >= 4) {
        return title;
    }

    if NOISE_SUBTITLES_EXACT.contains(&after_lower.as_str())
        || NOISE_SUBTITLE_PREFIXES
            .iter()
            .any(|p| after_lower.starts_with(p))
    {
        return before;
    }

    title
}

/// Build a search term from a title and optional author for Hardcover's API.
///
/// Strips noise subtitles (e.g. "A Novel", "Volume 3") but preserves the full
/// title including stop words — Hardcover's full-text search handles them fine,
/// and stripping them breaks titles like "A Clash of Kings".
pub(crate) fn build_search_term(title: &str, author: Option<&str>) -> String {
    let title = strip_noise_subtitle(title);
    match author {
        Some(a) if !a.is_empty() => format!("{title} {a}"),
        _ => title.to_string(),
    }
}

// ============================================================================
// Provider Implementation
// ============================================================================

/// Hardcover GraphQL API provider
/// Docs: https://hardcover.app/api
pub struct HardcoverProvider {
    client: reqwest::Client,
    base_url: String,
    api_key: Option<String>,
}

impl HardcoverProvider {
    /// Create a provider directly from a config JSON value.
    pub fn from_config(client: reqwest::Client, config: &serde_json::Value) -> Self {
        let api_key = config
            .get("api_key")
            .and_then(|v| v.as_str())
            .map(String::from);
        Self {
            client,
            base_url: config_base_url(config).unwrap_or_else(|| DEFAULT_BASE_URL.to_string()),
            api_key,
        }
    }
}

impl HardcoverProvider {
    pub fn has_api_key(&self) -> bool {
        self.api_key.as_deref().is_some_and(|k| !k.is_empty())
    }

    /// Require a non-empty API key, returning an error if not configured.
    fn require_api_key(&self) -> Result<&str, ProviderError> {
        match &self.api_key {
            Some(key) if !key.is_empty() => Ok(key.as_str()),
            _ => {
                tracing::debug!("Hardcover: No API key configured, skipping");
                Err(ProviderError::Other(
                    "Hardcover API key not configured".to_string(),
                ))
            }
        }
    }

    /// Look up a book by ISBN using the editions table (exact match).
    async fn isbn_lookup(
        &self,
        isbn: &str,
        api_key: &str,
    ) -> Result<Vec<MetadataResult>, ProviderError> {
        let clean = isbn.replace("-", "");
        tracing::debug!("Hardcover ISBN lookup: {}", clean);

        let graphql_query = IsbnLookupQuery {
            query: ISBN_LOOKUP_QUERY.to_string(),
            variables: IsbnLookupVariables { isbn: clean },
        };

        let response = self
            .client
            .post(format!("{}/v1/graphql", self.base_url))
            .header("Authorization", format!("Bearer {}", api_key))
            .json(&graphql_query)
            .send()
            .await?;

        if !response.status().is_success() {
            let status = response.status();
            let text = response.text().await.unwrap_or_default();
            tracing::warn!("Hardcover editions API error: {} - {}", status, text);
            return Err(ProviderError::from_http_status(
                status,
                "Hardcover editions API",
            ));
        }

        let result = response.json::<EditionsResponse>().await?;

        if let Some(ref errors) = result.errors
            && !errors.is_empty()
        {
            tracing::warn!("Hardcover editions GraphQL errors: {:?}", errors);
        }

        Ok(self.parse_editions_response(result))
    }

    /// Look up a book by ASIN using the editions table (exact match).
    async fn asin_lookup(
        &self,
        asin: &str,
        api_key: &str,
    ) -> Result<Vec<MetadataResult>, ProviderError> {
        let clean = asin.trim().to_uppercase();
        tracing::debug!("Hardcover ASIN lookup: {}", clean);

        let graphql_query = IsbnLookupQuery {
            query: ASIN_LOOKUP_QUERY.to_string(),
            variables: IsbnLookupVariables { isbn: clean },
        };

        let response = self
            .client
            .post(format!("{}/v1/graphql", self.base_url))
            .header("Authorization", format!("Bearer {}", api_key))
            .json(&graphql_query)
            .send()
            .await?;

        if !response.status().is_success() {
            let status = response.status();
            let text = response.text().await.unwrap_or_default();
            tracing::warn!("Hardcover editions ASIN API error: {} - {}", status, text);
            return Err(ProviderError::from_http_status(
                status,
                "Hardcover ASIN API",
            ));
        }

        let result = response.json::<EditionsResponse>().await?;

        if let Some(ref errors) = result.errors
            && !errors.is_empty()
        {
            tracing::warn!("Hardcover ASIN GraphQL errors: {:?}", errors);
        }

        Ok(self.parse_editions_response(result))
    }

    /// Full-text search using Hardcover's search() endpoint.
    async fn text_search(
        &self,
        search_term: &str,
        api_key: &str,
    ) -> Result<SearchResponse, ProviderError> {
        let graphql_query = GraphQLQuery {
            query: SEARCH_QUERY.to_string(),
            variables: SearchVariables {
                query: search_term.to_string(),
                per_page: 10,
            },
        };

        tracing::debug!("Hardcover text search: {}", search_term);

        let response = self
            .client
            .post(format!("{}/v1/graphql", self.base_url))
            .header("Authorization", format!("Bearer {}", api_key))
            .json(&graphql_query)
            .send()
            .await?;

        if !response.status().is_success() {
            let status = response.status();
            let text = response.text().await.unwrap_or_default();
            tracing::warn!("Hardcover API error: {} - {}", status, text);
            return Err(ProviderError::from_http_status(
                status,
                "Hardcover search API",
            ));
        }

        let result = response.json::<SearchResponse>().await?;

        if let Some(ref errors) = result.errors
            && !errors.is_empty()
        {
            tracing::warn!("Hardcover GraphQL errors: {:?}", errors);
        }

        Ok(result)
    }

    fn parse_editions_response(&self, response: EditionsResponse) -> Vec<MetadataResult> {
        let editions = match response.data {
            Some(data) => data.editions,
            None => return Vec::new(),
        };

        editions
            .into_iter()
            .filter_map(|edition| {
                let book = edition.book?;

                let title = book.title;
                let mut result = MetadataResult::new("hardcover", title);

                // Authors from contributions
                result.authors = book
                    .contributions
                    .unwrap_or_default()
                    .into_iter()
                    .filter_map(|c| c.author.map(|a| a.name))
                    .collect();

                result.description = book.description;
                result.page_count = book.pages.map(|p| p as u32);
                result.publish_date = book.release_year.map(|y| y.to_string());
                result.external_id = Some(book.id.to_string());

                // Build identifiers
                let mut idents = serde_json::Map::new();
                idents.insert("hardcover".to_string(), json!(book.id.to_string()));
                if let Some(slug) = &book.slug {
                    idents.insert("hardcover_slug".to_string(), json!(slug));
                }

                // Extract ASIN: prefer matched edition's ASIN, then audiobook sibling edition
                let asin = edition.asin.or_else(|| {
                    book.audiobook_editions
                        .as_ref()
                        .and_then(|eds| eds.first())
                        .and_then(|e| e.asin.clone())
                });
                if let Some(ref asin) = asin {
                    idents.insert("asin".to_string(), json!(asin));
                    result.asin = Some(asin.clone());
                }

                result.identifiers = json!(idents);

                // Cover image
                result.cover_url = book.image.and_then(|i| i.url);

                // ISBN from the edition that matched
                result.isbn = edition.isbn_13.or(edition.isbn_10);

                // Series info — collect all memberships
                if let Some(book_series) = book.book_series {
                    result.series_list = book_series
                        .iter()
                        .filter_map(|bs| {
                            let series = bs.series.as_ref()?;
                            Some(super::types::SeriesInfo {
                                name: series.name.clone(),
                                provider_id: Some(series.id.to_string()),
                                index: bs.position,
                            })
                        })
                        .collect();

                    // Single fields: pick first entry for backward compat
                    if let Some(first) = book_series.first() {
                        result.series_index = first.position;
                        if let Some(ref series) = first.series {
                            result.series = Some(series.name.clone());
                            result.series_provider_id = Some(series.id.to_string());
                        }
                    }
                }

                // Subjects from taggings
                if let Some(taggings) = book.taggings {
                    result.subjects = taggings
                        .into_iter()
                        .filter_map(|t| t.tag.map(|tag| tag.tag))
                        .collect();
                }

                // Ratings
                result.rating = book.rating;
                result.rating_count = book.ratings_count.map(|c| c as i32);

                // ISBN exact match → high confidence
                let mut confidence = 0.8;
                if !result.authors.is_empty() {
                    confidence += 0.1;
                }
                if result.cover_url.is_some() {
                    confidence += 0.1;
                }
                result.confidence = confidence;

                Some(result)
            })
            .collect()
    }

    // ========================================================================
    // Discover: trending / popular (not on the trait — Hardcover-specific)
    // ========================================================================

    /// Fetch popular books from Hardcover, sorted by user count (most popular).
    pub async fn popular_books(&self) -> Result<Vec<MetadataResult>, ProviderError> {
        let api_key = self.require_api_key()?;

        let graphql_query = serde_json::json!({
            "query": POPULAR_BOOKS_QUERY,
            "variables": { "limit": 20 }
        });

        tracing::debug!("Hardcover: fetching popular books");

        let response = self
            .client
            .post(format!("{}/v1/graphql", self.base_url))
            .header("Authorization", format!("Bearer {}", api_key))
            .json(&graphql_query)
            .send()
            .await?;

        if !response.status().is_success() {
            let status = response.status();
            let text = response.text().await.unwrap_or_default();
            tracing::warn!("Hardcover popular books API error: {} - {}", status, text);
            return Err(ProviderError::from_http_status(
                status,
                "Hardcover popular books API",
            ));
        }

        let result: PopularBooksResponse = response.json().await?;

        if let Some(ref errors) = result.errors
            && !errors.is_empty()
        {
            tracing::warn!("Hardcover popular books GraphQL errors: {:?}", errors);
        }

        Ok(self.parse_popular_books_response(result))
    }

    fn parse_popular_books_response(&self, response: PopularBooksResponse) -> Vec<MetadataResult> {
        let books = match response.data {
            Some(data) => data.books,
            None => return Vec::new(),
        };

        books
            .into_iter()
            .map(|book| {
                let title = book.title;
                let mut result = MetadataResult::new("hardcover", title);

                result.authors = book
                    .contributions
                    .unwrap_or_default()
                    .into_iter()
                    .filter_map(|c| c.author.map(|a| a.name))
                    .collect();

                result.description = book.description;
                result.page_count = book.pages.map(|p| p as u32);
                result.publish_date = book.release_year.map(|y| y.to_string());
                result.external_id = Some(book.id.to_string());

                let mut idents = serde_json::Map::new();
                idents.insert("hardcover".to_string(), json!(book.id.to_string()));
                if let Some(slug) = &book.slug {
                    idents.insert("hardcover_slug".to_string(), json!(slug));
                }
                result.identifiers = json!(idents);

                result.cover_url = book.image.and_then(|i| i.url);
                result.rating = book.rating;
                result.rating_count = book.ratings_count.map(|c| c as i32);

                // Series info — collect all memberships
                if let Some(book_series) = book.book_series {
                    result.series_list = book_series
                        .iter()
                        .filter_map(|bs| {
                            let series = bs.series.as_ref()?;
                            Some(super::types::SeriesInfo {
                                name: series.name.clone(),
                                provider_id: Some(series.id.to_string()),
                                index: bs.position,
                            })
                        })
                        .collect();

                    // Single fields: pick first entry for backward compat
                    if let Some(first) = book_series.first() {
                        result.series_index = first.position;
                        if let Some(ref series) = first.series {
                            result.series = Some(series.name.clone());
                            result.series_provider_id = Some(series.id.to_string());
                        }
                    }
                }

                // Subjects from taggings
                if let Some(taggings) = book.taggings {
                    result.subjects = taggings
                        .into_iter()
                        .filter_map(|t| t.tag.map(|tag| tag.tag))
                        .collect();
                }

                result.confidence = 1.0;
                result
            })
            .collect()
    }

    fn parse_search_response(&self, response: SearchResponse) -> Vec<MetadataResult> {
        let search_data = match response.data {
            Some(data) => data.search,
            None => return Vec::new(),
        };

        let results_json = match search_data.results {
            Some(r) => r,
            None => return Vec::new(),
        };

        let hits = match results_json.get("hits").and_then(|h| h.as_array()) {
            Some(h) => h,
            None => return Vec::new(),
        };

        hits.iter()
            .take(10)
            .filter_map(|hit| {
                let doc = hit.get("document")?;

                let title = doc.get("title")?.as_str()?.to_string();
                let mut result = MetadataResult::new("hardcover", title);

                // Extract authors
                if let Some(authors) = doc.get("author_names").and_then(|a| a.as_array()) {
                    result.authors = authors
                        .iter()
                        .filter_map(|a| a.as_str().map(String::from))
                        .collect();
                }

                result.description = doc
                    .get("description")
                    .and_then(|d| d.as_str())
                    .map(String::from);
                result.page_count = doc.get("pages").and_then(|p| p.as_i64()).map(|p| p as u32);
                // API returns release_date_i as Unix timestamp, convert to ISO date
                // Fall back to release_year if release_date_i is not available
                result.publish_date = doc
                    .get("release_date_i")
                    .and_then(|d| d.as_i64())
                    .and_then(|ts| {
                        time::OffsetDateTime::from_unix_timestamp(ts)
                            .ok()
                            .map(|dt| dt.date().to_string())
                    })
                    .or_else(|| {
                        doc.get("release_year")
                            .and_then(|y| y.as_i64())
                            .map(|y| y.to_string())
                    });
                result.external_id = doc.get("id").and_then(|id| id.as_str()).map(String::from);

                // Build identifiers with hardcover ID and slug
                let mut idents = serde_json::Map::new();
                if let Some(id) = &result.external_id {
                    idents.insert("hardcover".to_string(), json!(id));
                }
                if let Some(slug) = doc.get("slug").and_then(|s| s.as_str()) {
                    idents.insert("hardcover_slug".to_string(), json!(slug));
                }
                if !idents.is_empty() {
                    result.identifiers = json!(idents);
                }

                // Get cover image
                if let Some(image) = doc.get("image") {
                    result.cover_url = image.get("url").and_then(|u| u.as_str()).map(String::from);
                }

                // Get ISBN from isbns array (prefer ISBN-13)
                if let Some(isbns) = doc.get("isbns").and_then(|i| i.as_array()) {
                    for isbn in isbns {
                        if let Some(isbn_str) = isbn.as_str() {
                            // Prefer 13-digit ISBNs
                            if isbn_str.len() == 13 {
                                result.isbn = Some(isbn_str.to_string());
                                break;
                            } else if result.isbn.is_none() {
                                result.isbn = Some(isbn_str.to_string());
                            }
                        }
                    }
                }

                // Get series info from featured_series (includes position and ID)
                // Search API only returns the featured series, not all memberships,
                // so series_list will have at most one entry from search results.
                if let Some(featured) = doc.get("featured_series") {
                    if let Some(series) = featured.get("series") {
                        let name = series
                            .get("name")
                            .and_then(|n| n.as_str())
                            .map(String::from);
                        let provider_id = series
                            .get("id")
                            .and_then(|id| id.as_i64())
                            .map(|id| id.to_string());
                        let index = featured.get("position").and_then(|p| p.as_f64());

                        result.series = name.clone();
                        result.series_provider_id = provider_id.clone();
                        result.series_index = index;

                        if let Some(name) = name {
                            result.series_list.push(super::types::SeriesInfo {
                                name,
                                provider_id,
                                index,
                            });
                        }
                    }
                } else if let Some(series_names) =
                    doc.get("series_names").and_then(|s| s.as_array())
                    && let Some(first_series) = series_names.first()
                {
                    // Fallback to series_names if no featured_series
                    if let Some(name) = first_series.as_str().map(String::from) {
                        result.series = Some(name.clone());
                        result.series_list.push(super::types::SeriesInfo {
                            name,
                            provider_id: None,
                            index: None,
                        });
                    }
                }

                // Get genres/subjects
                if let Some(genres) = doc.get("genres").and_then(|g| g.as_array()) {
                    result.subjects = genres
                        .iter()
                        .filter_map(|g| g.as_str().map(String::from))
                        .collect();
                }

                // Get ratings if available
                result.rating = doc.get("rating").and_then(|r| r.as_f64());
                result.rating_count = doc
                    .get("ratings_count")
                    .and_then(|c| c.as_i64())
                    .map(|c| c as i32);

                // Calculate confidence
                let mut confidence = 0.4;
                if !result.authors.is_empty() {
                    confidence += 0.2;
                }
                if result.cover_url.is_some() {
                    confidence += 0.2;
                }
                if result.description.is_some() {
                    confidence += 0.1;
                }
                if result.isbn.is_some() {
                    confidence += 0.1;
                }
                result.confidence = confidence;

                Some(result)
            })
            .collect()
    }
}

#[async_trait]
impl MetadataProvider for HardcoverProvider {
    fn info(&self) -> &'static ProviderInfo {
        &INFO
    }

    async fn search(&self, query: &MetadataQuery) -> Result<Vec<MetadataResult>, ProviderError> {
        let api_key = self.require_api_key()?;

        // 1. If ISBN present → try exact editions lookup first
        if let Some(isbn) = &query.isbn {
            let results = self.isbn_lookup(isbn, api_key).await?;
            if !results.is_empty() {
                return Ok(results);
            }
            tracing::debug!(
                "Hardcover: ISBN {} not found in editions, falling through",
                isbn
            );
        }

        // 2. If ASIN present → try exact editions lookup
        if let Some(asin) = &query.asin {
            let results = self.asin_lookup(asin, api_key).await?;
            if !results.is_empty() {
                return Ok(results);
            }
            tracing::debug!(
                "Hardcover: ASIN {} not found in editions, falling through to title search",
                asin
            );
        }

        // 3. Fall through to full-text search with title
        if let Some(title) = &query.title {
            let search_term = build_search_term(title, query.authors.first().map(|s| s.as_str()));
            let result = self.text_search(&search_term, api_key).await?;
            return Ok(self.parse_search_response(result));
        }

        Ok(Vec::new())
    }

    async fn fetch_cover(&self, url: &str) -> Result<Vec<u8>, ProviderError> {
        let response = self.client.get(url).send().await?;

        if !response.status().is_success() {
            return Err(ProviderError::from_http_status(
                response.status(),
                "Cover fetch failed",
            ));
        }

        let bytes = response.bytes().await?;
        Ok(bytes.to_vec())
    }

    async fn search_series(
        &self,
        query: &SeriesQuery,
    ) -> Result<Vec<SeriesSearchResult>, ProviderError> {
        let api_key = self.require_api_key()?;

        if query.name.is_empty() {
            return Ok(Vec::new());
        }

        // Use the dedicated search query with query_type: "Series"
        let graphql_query = GraphQLQuery {
            query: SERIES_SEARCH_QUERY.to_string(),
            variables: SearchVariables {
                query: query.name.clone(),
                per_page: 10,
            },
        };

        tracing::debug!("Hardcover series search: {}", query.name);

        let response = self
            .client
            .post(format!("{}/v1/graphql", self.base_url))
            .header("Authorization", format!("Bearer {}", api_key))
            .json(&graphql_query)
            .send()
            .await?;

        if !response.status().is_success() {
            let status = response.status();
            let text = response.text().await.unwrap_or_default();
            tracing::warn!("Hardcover API error: {} - {}", status, text);
            return Err(ProviderError::from_http_status(
                status,
                "Hardcover series API",
            ));
        }

        let result = response.json::<SearchResponse>().await?;

        if let Some(ref errors) = result.errors
            && !errors.is_empty()
        {
            tracing::warn!("Hardcover GraphQL errors: {:?}", errors);
        }

        Ok(self.parse_series_search_response(result))
    }
}

impl HardcoverProvider {
    fn parse_series_search_response(&self, response: SearchResponse) -> Vec<SeriesSearchResult> {
        let search_data = match response.data {
            Some(data) => data.search,
            None => return Vec::new(),
        };

        let results_json = match search_data.results {
            Some(r) => r,
            None => return Vec::new(),
        };

        let hits = match results_json.get("hits").and_then(|h| h.as_array()) {
            Some(h) => h,
            None => return Vec::new(),
        };

        hits.iter()
            .take(10)
            .filter_map(|hit| {
                let doc = hit.get("document")?;

                // Debug: log full series search result for inspection
                tracing::info!(
                    "Hardcover series search result: {}",
                    serde_json::to_string_pretty(doc).unwrap_or_default()
                );

                let name = doc.get("name")?.as_str()?.to_string();
                let mut result = SeriesSearchResult::new("hardcover", name);

                // Extract external ID (can be string or integer)
                result.external_id = doc.get("id").and_then(|id| {
                    id.as_str()
                        .map(String::from)
                        .or_else(|| id.as_i64().map(|n| n.to_string()))
                });

                // Extract slug for URL generation
                result.slug = doc.get("slug").and_then(|s| s.as_str()).map(String::from);

                // Extract book count
                result.book_count = doc
                    .get("books_count")
                    .and_then(|c| c.as_i64())
                    .map(|c| c as u32);

                // Try to extract image from search document (may not exist)
                result.image_url = doc
                    .get("image")
                    .and_then(|i| i.get("url"))
                    .and_then(|u| u.as_str())
                    .map(String::from);

                // Extract series description
                result.description = doc
                    .get("description")
                    .and_then(|d| d.as_str())
                    .map(String::from);

                // Extract author name (Hardcover returns author_name for primary author)
                if let Some(author_name) = doc
                    .get("author_name")
                    .and_then(|a| a.as_str())
                    .map(String::from)
                {
                    result.authors.push(author_name);
                }

                // Extract books in the series
                if let Some(books) = doc.get("books").and_then(|b| b.as_array()) {
                    result.books = books
                        .iter()
                        .filter_map(|book| {
                            let title = book.get("title")?.as_str()?.to_string();

                            let authors = book
                                .get("contributions")
                                .and_then(|c| c.as_array())
                                .map(|contribs| {
                                    contribs
                                        .iter()
                                        .filter_map(|contrib| {
                                            contrib
                                                .get("author")
                                                .and_then(|a| a.get("name"))
                                                .and_then(|n| n.as_str())
                                                .map(String::from)
                                        })
                                        .collect()
                                })
                                .unwrap_or_default();

                            let series_index = book
                                .get("book_series")
                                .and_then(|bs| bs.as_array())
                                .and_then(|arr| arr.first())
                                .and_then(|first| first.get("position"))
                                .and_then(|p| p.as_f64());

                            let cover_url = book
                                .get("image")
                                .and_then(|img| img.get("url"))
                                .and_then(|u| u.as_str())
                                .map(String::from);

                            let publish_date = book
                                .get("release_date_i")
                                .and_then(|d| d.as_i64())
                                .and_then(|ts| {
                                    time::OffsetDateTime::from_unix_timestamp(ts)
                                        .ok()
                                        .map(|dt| dt.date().to_string())
                                })
                                .or_else(|| {
                                    book.get("release_year")
                                        .and_then(|y| y.as_i64())
                                        .map(|y| y.to_string())
                                });

                            let external_id = book
                                .get("id")
                                .and_then(|id| id.as_i64())
                                .map(|id| id.to_string());

                            Some(SeriesBook {
                                title,
                                authors,
                                series_index,
                                isbn: None, // ISBNs not included in series book list
                                cover_url,
                                publish_date,
                                external_id,
                            })
                        })
                        .collect();

                    // Sort books by series_index
                    result.books.sort_by(|a, b| {
                        a.series_index
                            .partial_cmp(&b.series_index)
                            .unwrap_or(std::cmp::Ordering::Equal)
                    });
                }

                // Calculate confidence based on data completeness
                let mut confidence = 0.5;
                if result.book_count.is_some() {
                    confidence += 0.2;
                }
                if !result.books.is_empty() {
                    confidence += 0.3;
                }
                result.confidence = confidence;

                Some(result)
            })
            .collect()
    }
}

// ============================================================================
// GraphQL Types
// ============================================================================

/// ISBN lookup via the editions table — exact match on isbn_10 or isbn_13.
/// Also fetches the audiobook edition (reading_format_id: 2 = "Listened") for ASIN.
const ISBN_LOOKUP_QUERY: &str = r#"
query IsbnLookup($isbn: String!) {
  editions(where: {_or: [{isbn_10: {_eq: $isbn}}, {isbn_13: {_eq: $isbn}}]}, limit: 1) {
    isbn_10
    isbn_13
    asin
    book_id
    book {
      id
      title
      slug
      description
      image { url }
      contributions { author { id name } }
      book_series { position series { id name } }
      pages
      release_year
      rating
      ratings_count
      taggings(limit: 10) { tag { tag } }
      audiobook_editions: editions(where: {reading_format_id: {_eq: 2}}, limit: 1) {
        asin
        audio_seconds
      }
    }
  }
}
"#;

/// GraphQL query for ASIN lookup via the editions table.
/// Reuses the same book fragment as ISBN lookup.
const ASIN_LOOKUP_QUERY: &str = r#"
query AsinLookup($isbn: String!) {
  editions(where: {asin: {_eq: $isbn}}, limit: 1) {
    isbn_10
    isbn_13
    asin
    book_id
    book {
      id
      title
      slug
      description
      image { url }
      contributions { author { id name } }
      book_series { position series { id name } }
      pages
      release_year
      rating
      ratings_count
      taggings(limit: 10) { tag { tag } }
      audiobook_editions: editions(where: {reading_format_id: {_eq: 2}}, limit: 1) {
        asin
        audio_seconds
      }
    }
  }
}
"#;

const BOOK_TITLES_QUERY: &str = r#"
query BookTitles($id: Int!) {
  books_by_pk(id: $id) {
    title
    subtitle
    editions(limit: 100) { title subtitle }
  }
}
"#;

/// GraphQL query using Hardcover's dedicated search endpoint.
/// The search query returns results as JSON in the `results` field.
/// We cannot use `books(where: {_ilike: ...})` as Hardcover blocks _ilike operations.
const SEARCH_QUERY: &str = r#"
query SearchBooks($query: String!, $per_page: Int!) {
  search(query: $query, query_type: "Book", per_page: $per_page) {
    ids
    results
  }
}
"#;

/// GraphQL query for popular/trending books — fetches by users_count descending.
const POPULAR_BOOKS_QUERY: &str = r#"
query PopularBooks($limit: Int!) {
  books(order_by: {users_count: desc}, limit: $limit) {
    id
    title
    slug
    description
    image { url }
    contributions { author { id name } }
    book_series { position series { id name } }
    pages
    release_year
    rating
    ratings_count
    taggings(limit: 10) { tag { tag } }
  }
}
"#;

/// GraphQL query for series search
const SERIES_SEARCH_QUERY: &str = r#"
query SearchSeries($query: String!, $per_page: Int!) {
  search(query: $query, query_type: "Series", per_page: $per_page) {
    ids
    results
  }
}
"#;

// -- ISBN editions types --

#[derive(Debug, Serialize)]
struct IsbnLookupQuery {
    query: String,
    variables: IsbnLookupVariables,
}

#[derive(Debug, Serialize)]
struct IsbnLookupVariables {
    isbn: String,
}

#[derive(Debug, Deserialize)]
struct EditionsResponse {
    data: Option<EditionsResponseData>,
    errors: Option<Vec<GraphQLError>>,
}

#[derive(Debug, Deserialize)]
struct EditionsResponseData {
    editions: Vec<Edition>,
}

#[derive(Debug, Deserialize)]
struct Edition {
    isbn_10: Option<String>,
    isbn_13: Option<String>,
    asin: Option<String>,
    #[allow(dead_code)]
    book_id: Option<i64>,
    book: Option<EditionBook>,
}

#[derive(Debug, Deserialize)]
struct EditionBook {
    id: i64,
    title: String,
    slug: Option<String>,
    description: Option<String>,
    image: Option<EditionImage>,
    contributions: Option<Vec<EditionContribution>>,
    book_series: Option<Vec<EditionBookSeries>>,
    pages: Option<i64>,
    release_year: Option<i64>,
    rating: Option<f64>,
    ratings_count: Option<i64>,
    taggings: Option<Vec<EditionTagging>>,
    audiobook_editions: Option<Vec<AudiobookEdition>>,
}

#[derive(Debug, Deserialize)]
struct AudiobookEdition {
    asin: Option<String>,
    #[allow(dead_code)]
    audio_seconds: Option<i64>,
}

#[derive(Debug, Deserialize)]
struct EditionImage {
    url: Option<String>,
}

#[derive(Debug, Deserialize)]
struct EditionContribution {
    author: Option<EditionAuthor>,
}

#[derive(Debug, Deserialize)]
struct EditionAuthor {
    #[allow(dead_code)]
    id: Option<i64>,
    name: String,
}

#[derive(Debug, Deserialize)]
struct EditionBookSeries {
    position: Option<f64>,
    series: Option<EditionSeries>,
}

#[derive(Debug, Deserialize)]
struct EditionSeries {
    id: i64,
    name: String,
}

#[derive(Debug, Deserialize)]
struct EditionTagging {
    tag: Option<EditionTag>,
}

#[derive(Debug, Deserialize)]
struct EditionTag {
    tag: String,
}

// -- Search types --

#[derive(Debug, Serialize)]
struct GraphQLQuery {
    query: String,
    variables: SearchVariables,
}

#[derive(Debug, Serialize)]
struct SearchVariables {
    query: String,
    per_page: i32,
}

#[derive(Debug, Deserialize)]
struct SearchResponse {
    data: Option<SearchResponseData>,
    errors: Option<Vec<GraphQLError>>,
}

#[derive(Debug, Deserialize)]
struct GraphQLError {
    #[allow(dead_code)]
    message: String,
}

#[derive(Debug, Deserialize)]
struct SearchResponseData {
    search: SearchOutput,
}

#[derive(Debug, Deserialize)]
struct SearchOutput {
    #[allow(dead_code)]
    ids: Option<Vec<String>>,
    /// The results field is JSONB containing `hits[].document` with book data
    results: Option<serde_json::Value>,
}

// -- Popular books types --

#[derive(Debug, Deserialize)]
struct PopularBooksResponse {
    data: Option<PopularBooksData>,
    errors: Option<Vec<GraphQLError>>,
}

#[derive(Debug, Deserialize)]
struct PopularBooksData {
    books: Vec<EditionBook>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct BookTitles {
    pub title: String,
    #[serde(default)]
    pub subtitle: Option<String>,
    #[serde(default)]
    pub editions: Vec<EditionTitle>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct EditionTitle {
    pub title: String,
    #[serde(default)]
    pub subtitle: Option<String>,
}

fn parse_book_titles(body: serde_json::Value) -> Option<BookTitles> {
    let book = body.get("data")?.get("books_by_pk")?;
    let mut titles: BookTitles = serde_json::from_value(book.clone()).ok()?;
    let mut seen = std::collections::HashSet::new();
    titles.editions.retain(|e| {
        !e.title.trim().is_empty()
            && seen.insert((
                e.title.to_lowercase(),
                e.subtitle.as_ref().map(|s| s.to_lowercase()),
            ))
    });
    Some(titles)
}

impl HardcoverProvider {
    /// `Ok(None)` when Hardcover has no such book, which is an answer.
    pub async fn book_titles(&self, book_id: &str) -> Result<Option<BookTitles>, ProviderError> {
        let api_key = self.require_api_key()?;
        let Ok(id) = book_id.trim().parse::<i64>() else {
            return Ok(None);
        };
        let response = self
            .client
            .post(format!("{}/v1/graphql", self.base_url))
            .header("Authorization", format!("Bearer {api_key}"))
            .timeout(crate::source_cache::LOOKUP_TIMEOUT)
            .json(&serde_json::json!({ "query": BOOK_TITLES_QUERY, "variables": { "id": id } }))
            .send()
            .await?;
        if !response.status().is_success() {
            return Err(ProviderError::from_http_status(
                response.status(),
                "Hardcover book titles",
            ));
        }
        let body: serde_json::Value = response.json().await?;
        if let Some(errors) = body.get("errors") {
            return Err(ProviderError::Other(format!(
                "Hardcover book titles query failed: {errors}"
            )));
        }
        Ok(parse_book_titles(body))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_build_search_term_preserves_stop_words() {
        let result = build_search_term("The Adventures of Tom Sawyer", None);
        assert_eq!(result, "The Adventures of Tom Sawyer");
    }

    #[test]
    fn test_build_search_term_preserves_stop_word_heavy_titles() {
        let result = build_search_term("A Clash of Kings", None);
        assert_eq!(result, "A Clash of Kings");
    }

    #[test]
    fn test_build_search_term_appends_author() {
        let result = build_search_term("The Adventures of Tom Sawyer", Some("Mark Twain"));
        assert_eq!(result, "The Adventures of Tom Sawyer Mark Twain");
    }

    #[test]
    fn test_build_search_term_no_author() {
        let result = build_search_term("Captain Underpants", None);
        assert_eq!(result, "Captain Underpants");
    }

    #[test]
    fn test_build_search_term_single_word_title() {
        let result = build_search_term("The Odyssey", None);
        assert_eq!(result, "The Odyssey");
    }

    // =========================================================================
    // strip_noise_subtitle
    // =========================================================================

    #[test]
    fn test_strip_noise_subtitle_novel() {
        assert_eq!(strip_noise_subtitle("Upgrade: A Novel"), "Upgrade");
    }

    #[test]
    fn test_strip_noise_subtitle_volume() {
        assert_eq!(
            strip_noise_subtitle("Fire In You: Volume Six"),
            "Fire In You"
        );
    }

    #[test]
    fn test_strip_noise_subtitle_part() {
        assert_eq!(strip_noise_subtitle("Dune: Part Two"), "Dune");
    }

    #[test]
    fn test_strip_noise_subtitle_book() {
        assert_eq!(strip_noise_subtitle("Saga: Book Three"), "Saga");
    }

    #[test]
    fn test_strip_noise_subtitle_preserves_meaningful() {
        assert_eq!(
            strip_noise_subtitle("2001: A Space Odyssey"),
            "2001: A Space Odyssey"
        );
    }

    #[test]
    fn test_strip_noise_subtitle_preserves_short_before_colon() {
        // "It" has no word >= 4 chars, don't strip
        assert_eq!(strip_noise_subtitle("It: A Novel"), "It: A Novel");
    }

    #[test]
    fn test_strip_noise_subtitle_no_colon() {
        assert_eq!(strip_noise_subtitle("The Hobbit"), "The Hobbit");
    }

    #[test]
    fn test_strip_noise_subtitle_memoir() {
        assert_eq!(strip_noise_subtitle("Educated: A Memoir"), "Educated");
    }

    #[test]
    fn test_build_search_term_with_noise_subtitle() {
        let result = build_search_term("Upgrade: A Novel", Some("Blake Crouch"));
        assert_eq!(result, "Upgrade Blake Crouch");
    }

    #[test]
    fn test_build_search_term_volume_subtitle() {
        let result = build_search_term("Fire In You: Volume Six", Some("J. Lynn"));
        assert_eq!(result, "Fire In You J. Lynn");
    }

    #[test]
    fn test_parse_editions_response_sample() {
        let provider = HardcoverProvider {
            client: reqwest::Client::new(),
            base_url: String::new(),
            api_key: None,
        };

        let response: EditionsResponse = serde_json::from_value(json!({
            "data": {
                "editions": [{
                    "isbn_10": null,
                    "isbn_13": "9780545504928",
                    "asin": null,
                    "book_id": 42,
                    "book": {
                        "id": 42,
                        "title": "Captain Underpants and the Sensational Saga of Sir Stinks-A-Lot",
                        "slug": "captain-underpants-12",
                        "description": "A silly adventure",
                        "image": { "url": "https://example.com/cover.jpg" },
                        "contributions": [
                            { "author": { "id": 1, "name": "Dav Pilkey" } }
                        ],
                        "book_series": [
                            { "position": 12.0, "series": { "id": 100, "name": "Captain Underpants" } }
                        ],
                        "pages": 176,
                        "release_year": 2015,
                        "rating": 3.8,
                        "ratings_count": 500,
                        "taggings": [
                            { "tag": { "tag": "Children's Fiction" } },
                            { "tag": { "tag": "Humor" } }
                        ],
                        "audiobook_editions": [
                            { "asin": "B00ABC1234", "audio_seconds": 18000 }
                        ]
                    }
                }]
            }
        }))
        .unwrap();

        let results = provider.parse_editions_response(response);
        assert_eq!(results.len(), 1);

        let r = &results[0];
        assert_eq!(
            r.title,
            "Captain Underpants and the Sensational Saga of Sir Stinks-A-Lot"
        );
        assert_eq!(r.authors, vec!["Dav Pilkey"]);
        assert_eq!(r.isbn, Some("9780545504928".to_string()));
        assert_eq!(r.asin, Some("B00ABC1234".to_string()));
        assert_eq!(r.series, Some("Captain Underpants".to_string()));
        assert_eq!(r.series_index, Some(12.0));
        assert_eq!(r.page_count, Some(176));
        assert!(r.cover_url.is_some());
        assert!(r.confidence >= 0.9); // ISBN match → high confidence
        assert_eq!(r.subjects, vec!["Children's Fiction", "Humor"]);
    }

    #[test]
    fn test_parse_editions_response_empty() {
        let provider = HardcoverProvider {
            client: reqwest::Client::new(),
            base_url: String::new(),
            api_key: None,
        };

        let response: EditionsResponse = serde_json::from_value(json!({
            "data": {
                "editions": []
            }
        }))
        .unwrap();

        let results = provider.parse_editions_response(response);
        assert!(results.is_empty());
    }

    #[test]
    fn book_titles_parse_with_editions_deduplicated() {
        let body = serde_json::json!({ "data": { "books_by_pk": {
            "title": "Babel",
            "subtitle": "An Arcane History",
            "editions": [
                { "title": "Babel", "subtitle": null },
                { "title": "babel", "subtitle": null },
                { "title": "Babel, or the Necessity of Violence", "subtitle": null },
                { "title": " ", "subtitle": null },
            ],
        }}});
        let titles = parse_book_titles(body).unwrap();
        assert_eq!(titles.subtitle.as_deref(), Some("An Arcane History"));
        assert_eq!(
            titles
                .editions
                .iter()
                .map(|e| e.title.as_str())
                .collect::<Vec<_>>(),
            ["Babel", "Babel, or the Necessity of Violence"]
        );
        assert!(
            parse_book_titles(serde_json::json!({ "data": { "books_by_pk": null } })).is_none()
        );
        assert!(parse_book_titles(serde_json::json!({ "errors": [] })).is_none());
    }
}
