use async_trait::async_trait;
use serde::Deserialize;

use super::types::{Capability, CapabilitySet, ConfigField, ConfigFieldType, ProviderInfo};
use super::{MetadataProvider, MetadataQuery, MetadataResult, ProviderError, ProviderFactory};

// ============================================================================
// Provider Info
// ============================================================================

/// Configuration fields for Google Books
pub static CONFIG_FIELDS: &[ConfigField] = &[ConfigField {
    key: "api_key",
    label: "API Key",
    field_type: ConfigFieldType::Secret,
    required: false,
    description: Some(
        "Create a Google Cloud project, enable the Books API, then create an API key. Without a key, requests share a global public quota and may be rate-limited.",
    ),
    help_url: Some("https://console.cloud.google.com/apis/library/books.googleapis.com"),
    default: None,
}];

/// Capabilities supported by Google Books
static CAPABILITIES: &[Capability] = &[
    Capability::IsbnLookup,
    Capability::TitleSearch,
    Capability::AuthorSearch,
    Capability::CoverFetch,
    Capability::SubjectInfo,
    Capability::Descriptions,
    Capability::PageCount,
    Capability::PublisherInfo,
    Capability::LanguageInfo,
    Capability::FreeTextSearch,
];

/// Static provider information
pub static INFO: ProviderInfo = ProviderInfo {
    id: crate::http::provider_ids::GOOGLEBOOKS,
    display_name: "Google Books",
    description: "Good descriptions and covers. Add an API key to avoid shared public quota limits.",
    website: "https://books.google.com",
    capabilities: CapabilitySet::new(CAPABILITIES),
    config_fields: CONFIG_FIELDS,
    requests_per_minute: 60, // Conservative for shared public quota without API key
};

// ============================================================================
// Factory
// ============================================================================

/// Factory for creating Google Books provider instances
pub struct GoogleBooksFactory {
    base_url_override: Option<String>,
}

impl GoogleBooksFactory {
    pub fn new(base_url_override: Option<String>) -> Self {
        Self { base_url_override }
    }
}

impl ProviderFactory for GoogleBooksFactory {
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
            .unwrap_or_else(|| "https://www.googleapis.com".to_string());

        let api_key = config
            .get("api_key")
            .and_then(|v| v.as_str())
            .map(String::from);

        Box::new(GoogleBooksProvider {
            client,
            base_url,
            api_key,
        })
    }
}

// ============================================================================
// Provider Implementation
// ============================================================================

/// Google Books API provider
/// Docs: https://developers.google.com/books/docs/v1/using
pub struct GoogleBooksProvider {
    client: reqwest::Client,
    base_url: String,
    api_key: Option<String>,
}

impl GoogleBooksProvider {
    fn parse_response(&self, response: SearchResponse) -> Vec<MetadataResult> {
        let items = response.items.unwrap_or_default();

        items
            .into_iter()
            .take(10)
            .filter_map(|item| {
                let info = item.volume_info?;

                let mut result = MetadataResult::new("googlebooks", info.title);

                result.authors = info.authors.unwrap_or_default();
                result.description = info.description;
                result.publisher = info.publisher;
                result.publish_date = info.published_date;
                result.language = info.language;
                result.page_count = info.page_count;
                result.subjects = info.categories.unwrap_or_default();
                result.external_id = Some(item.id);
                result.rating = info.average_rating;
                result.rating_count = info.ratings_count;

                // Extract identifiers (ISBN-10, ISBN-13, ASIN) from industry_identifiers
                if let Some(identifiers) = info.industry_identifiers {
                    let mut extracted_ids = serde_json::Map::new();
                    for id in identifiers {
                        match id.identifier_type.as_str() {
                            "ISBN_13" => {
                                // Set as primary ISBN for compatibility
                                if result.isbn.is_none() {
                                    result.isbn = Some(id.identifier.clone());
                                }
                                extracted_ids.insert(
                                    "isbn13".to_string(),
                                    serde_json::Value::String(id.identifier),
                                );
                            }
                            "ISBN_10" => {
                                // Use ISBN-10 as fallback if no ISBN-13
                                if result.isbn.is_none() {
                                    result.isbn = Some(id.identifier.clone());
                                }
                                extracted_ids.insert(
                                    "isbn10".to_string(),
                                    serde_json::Value::String(id.identifier),
                                );
                            }
                            "ASIN" => {
                                extracted_ids.insert(
                                    "asin".to_string(),
                                    serde_json::Value::String(id.identifier),
                                );
                            }
                            _ => {} // Ignore OTHER and unknown types
                        }
                    }
                    if !extracted_ids.is_empty() {
                        result.identifiers = serde_json::Value::Object(extracted_ids);
                    }
                }

                // Get cover image (prefer larger images)
                if let Some(links) = info.image_links {
                    result.cover_url = links
                        .extra_large
                        .or(links.large)
                        .or(links.medium)
                        .or(links.small)
                        .or(links.thumbnail)
                        .or(links.small_thumbnail);

                    // Google sometimes returns HTTP URLs, ensure HTTPS
                    if let Some(ref url) = result.cover_url
                        && url.starts_with("http://")
                    {
                        result.cover_url = Some(url.replacen("http://", "https://", 1));
                    }
                }

                // Calculate confidence
                let mut confidence = 0.4;
                if !result.authors.is_empty() {
                    confidence += 0.2;
                }
                if result.isbn.is_some() {
                    confidence += 0.2;
                }
                if result.cover_url.is_some() {
                    confidence += 0.1;
                }
                if result.description.is_some() {
                    confidence += 0.1;
                }
                result.confidence = confidence;

                Some(result)
            })
            .collect()
    }
}

#[async_trait]
impl MetadataProvider for GoogleBooksProvider {
    fn info(&self) -> &'static ProviderInfo {
        &INFO
    }

    async fn search(&self, query: &MetadataQuery) -> Result<Vec<MetadataResult>, ProviderError> {
        // Build query string
        let q = if let Some(isbn) = &query.isbn {
            format!("isbn:{}", urlencoding::encode(&isbn.replace("-", "")))
        } else if let Some(title) = &query.title {
            if let Some(author) = query.authors.first() {
                format!(
                    "intitle:{}+inauthor:{}",
                    urlencoding::encode(title),
                    urlencoding::encode(author)
                )
            } else {
                format!("intitle:{}", urlencoding::encode(title))
            }
        } else if let Some(text) = &query.free_text {
            // Content-based fallback: plain free-text query (no intitle/isbn prefix)
            let truncated = if text.len() > 200 {
                text[..text[..200].rfind(' ').unwrap_or(200)].to_string()
            } else {
                text.clone()
            };
            urlencoding::encode(&truncated).into_owned()
        } else {
            return Ok(Vec::new());
        };

        // Build URL with optional API key for higher rate limits
        let mut url = format!(
            "{}/books/v1/volumes?q={}&maxResults=10&langRestrict=en",
            self.base_url, q
        );

        if let Some(ref key) = self.api_key {
            url.push_str(&format!("&key={}", urlencoding::encode(key)));
        }

        tracing::debug!("Google Books search: {}", url);

        let response = self
            .client
            .get(&url)
            .send()
            .await?
            .json::<SearchResponse>()
            .await?;

        let results = self.parse_response(response);

        // Fallback: ISBN returned empty but we have a title → retry with title search
        if results.is_empty()
            && query.isbn.is_some()
            && let Some(title) = &query.title
        {
            let fallback_q = if let Some(author) = query.authors.first() {
                format!(
                    "intitle:{}+inauthor:{}",
                    urlencoding::encode(title),
                    urlencoding::encode(author)
                )
            } else {
                format!("intitle:{}", urlencoding::encode(title))
            };

            let mut fallback_url = format!(
                "{}/books/v1/volumes?q={}&maxResults=10&langRestrict=en",
                self.base_url, fallback_q
            );

            if let Some(ref key) = self.api_key {
                fallback_url.push_str(&format!("&key={}", urlencoding::encode(key)));
            }

            tracing::debug!(
                "Google Books ISBN empty, falling back to title: {}",
                fallback_url
            );

            let fallback_response = self
                .client
                .get(&fallback_url)
                .send()
                .await?
                .json::<SearchResponse>()
                .await?;

            return Ok(self.parse_response(fallback_response));
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
    total_items: Option<i32>,
    items: Option<Vec<VolumeItem>>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct VolumeItem {
    id: String,
    volume_info: Option<VolumeInfo>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct VolumeInfo {
    title: String,
    authors: Option<Vec<String>>,
    publisher: Option<String>,
    published_date: Option<String>,
    description: Option<String>,
    industry_identifiers: Option<Vec<IndustryIdentifier>>,
    page_count: Option<u32>,
    categories: Option<Vec<String>>,
    image_links: Option<ImageLinks>,
    language: Option<String>,
    average_rating: Option<f64>,
    ratings_count: Option<i32>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct IndustryIdentifier {
    #[serde(rename = "type")]
    identifier_type: String,
    identifier: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ImageLinks {
    small_thumbnail: Option<String>,
    thumbnail: Option<String>,
    small: Option<String>,
    medium: Option<String>,
    large: Option<String>,
    extra_large: Option<String>,
}
