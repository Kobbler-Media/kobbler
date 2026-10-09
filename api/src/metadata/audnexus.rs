use async_trait::async_trait;
use serde::Deserialize;

use super::types::{
    Capability, CapabilitySet, Chapter, ConfigField, ConfigFieldType, ProviderInfo,
};
use super::{MetadataProvider, MetadataQuery, MetadataResult, ProviderError, ProviderFactory};

/// Fetch chapters from Audnexus by ASIN using the default base URL.
/// Returns (title, start_seconds, end_seconds) tuples.
pub async fn fetch_chapters_by_asin(asin: &str) -> Result<Vec<(String, f64, f64)>, ProviderError> {
    let client = crate::http::ProviderClient::new(
        crate::http::new_client(),
        crate::http::provider_ids::AUDNEXUS,
    );
    let base_url = "https://api.audnex.us";
    let url = format!("{base_url}/books/{asin}/chapters");
    tracing::debug!("Audnexus direct chapter fetch: {url}");

    let response = client.execute(client.get(&url)).await?;

    if response.status() == reqwest::StatusCode::TOO_MANY_REQUESTS {
        return Err(ProviderError::RateLimited);
    }
    if response.status() == reqwest::StatusCode::NOT_FOUND {
        return Err(ProviderError::Other(format!(
            "No chapters found for ASIN: {asin}"
        )));
    }
    if !response.status().is_success() {
        return Err(ProviderError::Other(format!(
            "Audnexus chapters API error: {}",
            response.status()
        )));
    }

    let ch_resp: ChaptersResponse = response
        .json()
        .await
        .map_err(|e| ProviderError::Parse(e.to_string()))?;

    let chapter_count = ch_resp.chapters.len();
    let runtime_ms = ch_resp.runtime_length_ms;

    Ok(ch_resp
        .chapters
        .into_iter()
        .enumerate()
        .map(|(i, ch)| {
            let end_ms = if i + 1 < chapter_count {
                ch.start_offset_ms + ch.length_ms
            } else {
                runtime_ms.unwrap_or(ch.start_offset_ms + ch.length_ms)
            };
            (
                ch.title,
                ch.start_offset_ms as f64 / 1000.0,
                end_ms as f64 / 1000.0,
            )
        })
        .collect())
}

// ============================================================================
// Provider Info
// ============================================================================

/// Configuration fields for Audnexus
pub static CONFIG_FIELDS: &[ConfigField] = &[ConfigField {
    key: "base_url",
    label: "Base URL",
    field_type: ConfigFieldType::Url,
    required: false,
    description: Some("Custom API endpoint (default: https://api.audnex.us)"),
    help_url: None,
    default: Some("https://api.audnex.us"),
}];

/// Capabilities supported by Audnexus
static CAPABILITIES: &[Capability] = &[
    Capability::AsinLookup,
    Capability::NarratorInfo,
    Capability::ChapterInfo,
    Capability::DurationInfo,
    Capability::SubjectInfo,
    Capability::PublisherInfo,
    Capability::CoverFetch,
    Capability::Descriptions,
];

/// Static provider information
pub static INFO: ProviderInfo = ProviderInfo {
    id: crate::http::provider_ids::AUDNEXUS,
    display_name: "Audnexus",
    description: "Free audiobook metadata from Audible via Audnexus. Provides narrators, chapters, duration, and genres. No API key required.",
    website: "https://audnex.us",
    capabilities: CapabilitySet::new(CAPABILITIES),
    config_fields: CONFIG_FIELDS,
    requests_per_minute: 30, // Small community project, be gentle
};

// ============================================================================
// Factory
// ============================================================================

/// Factory for creating Audnexus provider instances
pub struct AudnexusFactory {
    base_url_override: Option<String>,
}

impl AudnexusFactory {
    pub fn new(base_url_override: Option<String>) -> Self {
        Self { base_url_override }
    }
}

impl ProviderFactory for AudnexusFactory {
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
            .or_else(|| {
                config
                    .get("base_url")
                    .and_then(|v| v.as_str())
                    .map(String::from)
            })
            .unwrap_or_else(|| "https://api.audnex.us".to_string());

        let provider_client =
            crate::http::ProviderClient::new(client, crate::http::provider_ids::AUDNEXUS);
        Box::new(AudnexusProvider {
            client: provider_client,
            base_url,
        })
    }
}

// ============================================================================
// Provider Implementation
// ============================================================================

/// Audnexus API provider for audiobook metadata
/// Docs: https://audnex.us
pub struct AudnexusProvider {
    client: crate::http::ProviderClient,
    base_url: String,
}

impl AudnexusProvider {
    /// Fetch book metadata by ASIN
    async fn fetch_book(&self, asin: &str) -> Result<BookResponse, ProviderError> {
        let url = format!("{}/books/{}", self.base_url, asin);
        tracing::debug!("Audnexus book lookup: {}", url);

        let response = self.client.execute(self.client.get(&url)).await?;

        if response.status() == reqwest::StatusCode::TOO_MANY_REQUESTS {
            return Err(ProviderError::RateLimited);
        }

        if response.status() == reqwest::StatusCode::NOT_FOUND {
            return Err(ProviderError::Other(format!("ASIN not found: {}", asin)));
        }

        if !response.status().is_success() {
            return Err(ProviderError::Other(format!(
                "Audnexus API error: {}",
                response.status()
            )));
        }

        response
            .json::<BookResponse>()
            .await
            .map_err(|e| ProviderError::Parse(e.to_string()))
    }

    /// Fetch chapter information by ASIN
    async fn fetch_chapters(&self, asin: &str) -> Result<ChaptersResponse, ProviderError> {
        let url = format!("{}/books/{}/chapters", self.base_url, asin);
        tracing::debug!("Audnexus chapters lookup: {}", url);

        let response = self.client.execute(self.client.get(&url)).await?;

        if response.status() == reqwest::StatusCode::TOO_MANY_REQUESTS {
            return Err(ProviderError::RateLimited);
        }

        if !response.status().is_success() {
            return Err(ProviderError::Other(format!(
                "Audnexus chapters API error: {}",
                response.status()
            )));
        }

        response
            .json::<ChaptersResponse>()
            .await
            .map_err(|e| ProviderError::Parse(e.to_string()))
    }

    /// Convert an Audnexus book response (+ optional chapters) into a MetadataResult
    fn to_metadata_result(
        &self,
        book: BookResponse,
        chapters_resp: Option<ChaptersResponse>,
    ) -> MetadataResult {
        let mut result = MetadataResult::new(crate::http::provider_ids::AUDNEXUS, book.title);

        result.authors = book.authors.into_iter().map(|a| a.name).collect();
        result.description = book.description.or(book.summary);
        result.publisher = book.publisher_name;
        result.publish_date = book.release_date;
        result.language = book.language;
        result.cover_url = book.image;
        result.isbn = book.isbn;
        result.asin = Some(book.asin.clone());
        result.external_id = Some(book.asin);

        // Narrators
        if let Some(narrators) = book.narrators {
            let narrator_names: Vec<String> = narrators.into_iter().map(|n| n.name).collect();
            if !narrator_names.is_empty() {
                result.narrator = Some(narrator_names.join(", "));
            }
        }

        // Duration from runtime_length_min
        if let Some(runtime_min) = book.runtime_length_min {
            result.duration_seconds = Some(runtime_min as u64 * 60);
        }

        // Genres/subjects
        if let Some(genres) = book.genres {
            result.subjects = genres.into_iter().map(|g| g.name).collect();
        }

        // Rating
        if let Some(rating_str) = book.rating
            && let Ok(rating) = rating_str.parse::<f64>()
        {
            result.rating = Some(rating);
        }

        // Chapters
        if let Some(ch_resp) = chapters_resp {
            let chapter_count = ch_resp.chapters.len();
            let runtime_ms = ch_resp.runtime_length_ms;
            result.chapters = ch_resp
                .chapters
                .into_iter()
                .enumerate()
                .map(|(i, ch)| {
                    let end_ms = if i + 1 < chapter_count {
                        ch.start_offset_ms + ch.length_ms
                    } else {
                        // Last chapter: use total runtime or start + length
                        runtime_ms.unwrap_or(ch.start_offset_ms + ch.length_ms)
                    };
                    Chapter {
                        title: ch.title,
                        start_ms: ch.start_offset_ms,
                        end_ms,
                    }
                })
                .collect();

            // Use more precise runtime from chapters response if available
            if let Some(ms) = runtime_ms {
                result.duration_seconds = Some(ms / 1000);
            }
        }

        // High confidence since this is a direct ASIN lookup
        result.confidence = 0.95;

        result
    }
}

#[async_trait]
impl MetadataProvider for AudnexusProvider {
    fn info(&self) -> &'static ProviderInfo {
        &INFO
    }

    async fn search(&self, query: &MetadataQuery) -> Result<Vec<MetadataResult>, ProviderError> {
        // Audnexus only supports ASIN lookup, not free-text search
        let asin = match &query.asin {
            Some(asin) => asin.clone(),
            None => return Ok(Vec::new()),
        };

        let book = self.fetch_book(&asin).await?;

        // Also fetch chapters (non-fatal if this fails)
        let chapters = match self.fetch_chapters(&asin).await {
            Ok(ch) => Some(ch),
            Err(e) => {
                tracing::warn!("Failed to fetch Audnexus chapters for {}: {}", asin, e);
                None
            }
        };

        let result = self.to_metadata_result(book, chapters);
        Ok(vec![result])
    }

    async fn fetch_cover(&self, url: &str) -> Result<Vec<u8>, ProviderError> {
        let response = self.client.execute(self.client.get(url)).await?;

        if !response.status().is_success() {
            return Err(ProviderError::Other(format!(
                "Cover fetch failed: {}",
                response.status()
            )));
        }

        let bytes = response.bytes().await?;
        Ok(bytes.to_vec())
    }
}

// ============================================================================
// Response Types
// ============================================================================

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct BookResponse {
    asin: String,
    title: String,
    authors: Vec<AuthorEntry>,
    #[serde(default)]
    narrators: Option<Vec<NarratorEntry>>,
    description: Option<String>,
    summary: Option<String>,
    publisher_name: Option<String>,
    release_date: Option<String>,
    runtime_length_min: Option<i64>,
    language: Option<String>,
    image: Option<String>,
    isbn: Option<String>,
    #[serde(default)]
    genres: Option<Vec<GenreEntry>>,
    rating: Option<String>,
}

#[derive(Debug, Deserialize)]
struct AuthorEntry {
    name: String,
}

#[derive(Debug, Deserialize)]
struct NarratorEntry {
    name: String,
}

#[derive(Debug, Deserialize)]
struct GenreEntry {
    name: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ChaptersResponse {
    runtime_length_ms: Option<u64>,
    chapters: Vec<ChapterEntry>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ChapterEntry {
    title: String,
    start_offset_ms: u64,
    length_ms: u64,
}
