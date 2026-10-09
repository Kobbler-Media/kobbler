use async_trait::async_trait;
use serde::Deserialize;

use super::types::{Capability, CapabilitySet, ConfigField, ConfigFieldType, ProviderInfo};
use super::{MetadataProvider, MetadataQuery, MetadataResult, ProviderError, ProviderFactory};

// ============================================================================
// Provider Info
// ============================================================================

/// Configuration fields for Open Library
pub static CONFIG_FIELDS: &[ConfigField] = &[ConfigField {
    key: "base_url",
    label: "Base URL",
    field_type: ConfigFieldType::Url,
    required: false,
    description: Some("Custom API endpoint (for mirrors or self-hosted instances)"),
    help_url: None,
    default: Some("https://openlibrary.org"),
}];

/// Capabilities supported by Open Library
static CAPABILITIES: &[Capability] = &[
    Capability::IsbnLookup,
    Capability::TitleSearch,
    Capability::AuthorSearch,
    Capability::CoverFetch,
    Capability::SubjectInfo,
    Capability::PageCount,
    Capability::PublisherInfo,
    Capability::PublishDate,
    Capability::LanguageInfo,
];

/// Static provider information
pub static INFO: ProviderInfo = ProviderInfo {
    id: crate::http::provider_ids::OPENLIBRARY,
    display_name: "Open Library",
    description: "Free, open-source book database from the Internet Archive. No API key required.",
    website: "https://openlibrary.org",
    capabilities: CapabilitySet::new(CAPABILITIES),
    config_fields: CONFIG_FIELDS,
    requests_per_minute: 100, // OL asks for politeness; ~1.7 req/sec
};

// ============================================================================
// Factory
// ============================================================================

/// Factory for creating Open Library provider instances
pub struct OpenLibraryFactory {
    base_url_override: Option<String>,
}

impl OpenLibraryFactory {
    pub fn new(base_url_override: Option<String>) -> Self {
        Self { base_url_override }
    }
}

impl ProviderFactory for OpenLibraryFactory {
    fn info(&self) -> &'static ProviderInfo {
        &INFO
    }

    fn create(
        &self,
        client: reqwest::Client,
        config: &serde_json::Value,
    ) -> Box<dyn MetadataProvider> {
        // Priority: factory override > config > default
        let base_url = self
            .base_url_override
            .clone()
            .or_else(|| {
                config
                    .get("base_url")
                    .and_then(|v| v.as_str())
                    .map(String::from)
            })
            .unwrap_or_else(|| "https://openlibrary.org".to_string());

        Box::new(OpenLibraryProvider { client, base_url })
    }
}

// ============================================================================
// Provider Implementation
// ============================================================================

/// Open Library API provider
/// Docs: https://openlibrary.org/developers/api
pub struct OpenLibraryProvider {
    client: reqwest::Client,
    base_url: String,
}

impl OpenLibraryProvider {
    fn parse_search_response(&self, response: SearchResponse) -> Vec<MetadataResult> {
        response
            .docs
            .into_iter()
            .take(10) // Limit to top 10 results
            .map(|doc| {
                let mut result = MetadataResult::new("openlibrary", doc.title);

                // Calculate confidence based on available data first (before consuming)
                let mut confidence = 0.3;
                if doc.author_name.is_some() {
                    confidence += 0.2;
                }
                if doc.isbn.is_some() {
                    confidence += 0.3;
                }
                if doc.cover_i.is_some() {
                    confidence += 0.2;
                }
                result.confidence = confidence;

                // Now consume the values
                result.authors = doc.author_name.unwrap_or_default();
                result.isbn = doc.isbn.and_then(|isbns| isbns.into_iter().next());
                result.publisher = doc.publisher.and_then(|pubs| pubs.into_iter().next());
                result.publish_date = doc.first_publish_year.map(|y| y.to_string());
                result.language = doc.language.and_then(|langs| langs.into_iter().next());
                result.subjects = doc.subject.unwrap_or_default();
                result.page_count = doc.number_of_pages_median.map(|n| n as u32);

                // Cover URL from cover_i (cover edition key)
                if let Some(cover_id) = doc.cover_i {
                    result.cover_url = Some(format!(
                        "https://covers.openlibrary.org/b/id/{}-L.jpg",
                        cover_id
                    ));
                }

                // External ID is the work key
                result.external_id = doc.key;

                result
            })
            .collect()
    }
}

#[async_trait]
impl MetadataProvider for OpenLibraryProvider {
    fn info(&self) -> &'static ProviderInfo {
        &INFO
    }

    async fn search(&self, query: &MetadataQuery) -> Result<Vec<MetadataResult>, ProviderError> {
        // Fields to request from the API (publisher and others aren't returned by default)
        // Note: description is not available from search API - would need separate work fetch
        const FIELDS: &str = "key,title,author_name,isbn,publisher,first_publish_year,language,subject,cover_i,number_of_pages_median";

        // Build search URL
        let url = if let Some(isbn) = &query.isbn {
            // ISBN search is more direct
            let clean_isbn = isbn.replace("-", "");
            format!(
                "{}/search.json?isbn={}&limit=5&fields={}",
                self.base_url, clean_isbn, FIELDS
            )
        } else if let Some(title) = &query.title {
            let mut url = format!(
                "{}/search.json?title={}&limit=10&fields={}",
                self.base_url,
                urlencoding::encode(title),
                FIELDS
            );
            if let Some(author) = query.authors.first() {
                url.push_str(&format!("&author={}", urlencoding::encode(author)));
            }
            url
        } else {
            return Ok(Vec::new());
        };

        tracing::debug!("Open Library search: {}", url);

        let response = self
            .client
            .get(&url)
            .send()
            .await?
            .json::<SearchResponse>()
            .await?;

        let results = self.parse_search_response(response);

        // Fallback: ISBN returned empty but we have a title → retry with title search
        if results.is_empty()
            && query.isbn.is_some()
            && let Some(title) = &query.title
        {
            let mut fallback_url = format!(
                "{}/search.json?title={}&limit=10&fields={}",
                self.base_url,
                urlencoding::encode(title),
                FIELDS
            );
            if let Some(author) = query.authors.first() {
                fallback_url.push_str(&format!("&author={}", urlencoding::encode(author)));
            }

            tracing::debug!(
                "Open Library ISBN empty, falling back to title: {}",
                fallback_url
            );

            let fallback_response = self
                .client
                .get(&fallback_url)
                .send()
                .await?
                .json::<SearchResponse>()
                .await?;

            return Ok(self.parse_search_response(fallback_response));
        }

        Ok(results)
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
}

// ============================================================================
// Response Types
// ============================================================================

#[derive(Debug, Deserialize)]
struct SearchResponse {
    #[allow(dead_code)]
    num_found: Option<i32>,
    docs: Vec<SearchDoc>,
}

#[derive(Debug, Deserialize)]
struct SearchDoc {
    key: Option<String>,
    title: String,
    author_name: Option<Vec<String>>,
    isbn: Option<Vec<String>>,
    publisher: Option<Vec<String>>,
    first_publish_year: Option<i32>,
    language: Option<Vec<String>>,
    subject: Option<Vec<String>>,
    cover_i: Option<i64>,
    number_of_pages_median: Option<i32>,
}
