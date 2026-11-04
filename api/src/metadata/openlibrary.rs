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

fn work_key(external_id: &str) -> Option<String> {
    let id = external_id
        .trim()
        .trim_start_matches('/')
        .trim_start_matches("works/");
    (!id.is_empty() && id.chars().all(|c| c.is_ascii_alphanumeric()))
        .then(|| format!("/works/{id}"))
}

/// The search index has the work's authors and ISBNs; only the work record has its description.
fn work_result(work: WorkResponse, indexed: Option<MetadataResult>) -> MetadataResult {
    let mut result = indexed.unwrap_or_else(|| {
        let mut r = MetadataResult::new("openlibrary", work.title);
        r.subjects = work.subjects;
        r.publish_date = work.first_publish_date;
        r.cover_url = work
            .covers
            .iter()
            .find(|id| **id > 0)
            .map(|id| format!("https://covers.openlibrary.org/b/id/{id}-L.jpg"));
        r
    });
    result.external_id = Some(work.key);
    result.description = work.description.map(WorkDescription::into_text);
    result.confidence = 1.0;
    result
}

#[async_trait]
impl MetadataProvider for OpenLibraryProvider {
    fn info(&self) -> &'static ProviderInfo {
        &INFO
    }

    async fn search(&self, query: &MetadataQuery) -> Result<Vec<MetadataResult>, ProviderError> {
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

    async fn fetch_by_id(
        &self,
        external_id: &str,
    ) -> Result<Option<MetadataResult>, ProviderError> {
        let Some(key) = work_key(external_id) else {
            return Ok(None);
        };
        let response = self
            .client
            .get(format!("{}{key}.json", self.base_url))
            .send()
            .await?;
        if response.status() == reqwest::StatusCode::NOT_FOUND {
            return Ok(None);
        }
        if !response.status().is_success() {
            return Err(ProviderError::from_http_status(
                response.status(),
                "Open Library works API",
            ));
        }
        let work: WorkResponse = response.json().await?;

        let indexed = self
            .client
            .get(format!(
                "{}/search.json?q={}&fields={FIELDS}",
                self.base_url,
                urlencoding::encode(&format!("key:{key}"))
            ))
            .send()
            .await?
            .json::<SearchResponse>()
            .await?;
        let indexed = self.parse_search_response(indexed).into_iter().next();
        Ok(Some(work_result(work, indexed)))
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

// Description is not in the search index; it comes from the work record.
const FIELDS: &str = "key,title,author_name,isbn,publisher,first_publish_year,language,subject,cover_i,number_of_pages_median";

#[derive(Debug, Deserialize)]
struct WorkResponse {
    key: String,
    title: String,
    description: Option<WorkDescription>,
    #[serde(default)]
    covers: Vec<i64>,
    #[serde(default)]
    subjects: Vec<String>,
    first_publish_date: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum WorkDescription {
    Text(String),
    Typed { value: String },
}

impl WorkDescription {
    fn into_text(self) -> String {
        match self {
            Self::Text(text) | Self::Typed { value: text } => text,
        }
    }
}

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

#[cfg(test)]
mod tests {
    use super::*;

    fn provider() -> OpenLibraryProvider {
        OpenLibraryProvider {
            client: reqwest::Client::new(),
            base_url: String::new(),
        }
    }

    fn work() -> WorkResponse {
        serde_json::from_value(serde_json::json!({
            "key": "/works/OL45883W",
            "title": "Fantastic Mr Fox",
            "description": { "type": "/type/text", "value": "Three farmers." },
            "covers": [-1, 6498519],
            "subjects": ["Foxes"],
            "first_publish_date": "1970",
            "authors": [{ "author": { "key": "/authors/OL34184A" } }]
        }))
        .unwrap()
    }

    #[test]
    fn work_key_accepts_bare_and_prefixed_ids() {
        assert_eq!(work_key("OL45883W").as_deref(), Some("/works/OL45883W"));
        assert_eq!(
            work_key("/works/OL45883W").as_deref(),
            Some("/works/OL45883W")
        );
        assert_eq!(work_key("../authors/x"), None);
        assert_eq!(work_key(""), None);
    }

    #[test]
    fn work_sample_merges_indexed_fields_with_description() {
        let search: SearchResponse = serde_json::from_value(serde_json::json!({
            "num_found": 1,
            "docs": [{
                "key": "/works/OL45883W",
                "title": "Fantastic Mr Fox",
                "author_name": ["Roald Dahl"],
                "isbn": ["9780142410349"],
                "first_publish_year": 1970,
                "subject": ["Foxes", "Farmers"],
                "cover_i": 6498519
            }]
        }))
        .unwrap();
        let indexed = provider().parse_search_response(search).into_iter().next();

        let r = work_result(work(), indexed);
        assert_eq!(r.authors, ["Roald Dahl"]);
        assert_eq!(r.isbn.as_deref(), Some("9780142410349"));
        assert_eq!(r.description.as_deref(), Some("Three farmers."));
        assert_eq!(r.external_id.as_deref(), Some("/works/OL45883W"));
        assert_eq!(r.publish_date.as_deref(), Some("1970"));
        assert_eq!(r.subjects, ["Foxes", "Farmers"]);
    }

    #[test]
    fn work_sample_alone_still_maps() {
        let r = work_result(work(), None);
        assert_eq!(r.title, "Fantastic Mr Fox");
        assert_eq!(r.description.as_deref(), Some("Three farmers."));
        assert_eq!(
            r.cover_url.as_deref(),
            Some("https://covers.openlibrary.org/b/id/6498519-L.jpg")
        );
        assert_eq!(r.subjects, ["Foxes"]);
    }
}
