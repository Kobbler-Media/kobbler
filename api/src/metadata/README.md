# Metadata Provider System

This module provides a plugin-like architecture for book metadata providers. Providers are used to fetch book information (titles, authors, covers, series info, etc.) from external APIs.

## Architecture Overview

The system has three main components:

1. **MetadataProvider trait** - Defines the interface for fetching metadata
2. **ProviderFactory trait** - Creates provider instances with configuration
3. **ProviderRegistry** - Manages factories and coordinates searches

## Built-in Providers

| Provider | Capabilities | Config |
|----------|-------------|--------|
| Open Library | ISBN, title, author, covers, subjects, pages, publisher, language | `base_url` (optional) |
| Google Books | ISBN, title, author, covers, descriptions, subjects, pages, publisher, language | `api_key` (optional, increases rate limits) |
| Hardcover | ISBN, title, author, covers, **series**, descriptions, subjects, pages | `api_key` (optional) |

## Adding a New Provider

### Step 1: Create the provider module

Create a new file `api/src/metadata/myprovider.rs`:

```rust
use async_trait::async_trait;
use serde::Deserialize;

use super::types::{Capability, CapabilitySet, ConfigField, ConfigFieldType, ProviderInfo};
use super::{MetadataProvider, MetadataQuery, MetadataResult, ProviderError, ProviderFactory};

// ============================================================================
// Provider Info (static metadata about this provider)
// ============================================================================

pub static CONFIG_FIELDS: &[ConfigField] = &[
    ConfigField {
        key: "api_key",
        label: "API Key",
        field_type: ConfigFieldType::Secret,
        required: false,
        description: Some("Your API key from the provider"),
        help_url: Some("https://provider.example.com/api-keys"),
        default: None,
    },
];

static CAPABILITIES: &[Capability] = &[
    Capability::IsbnLookup,
    Capability::TitleSearch,
    Capability::AuthorSearch,
    Capability::CoverFetch,
    // Add other capabilities your provider supports
];

pub static INFO: ProviderInfo = ProviderInfo {
    id: "myprovider",
    display_name: "My Provider",
    description: "Description shown in the UI settings.",
    website: "https://provider.example.com",
    capabilities: CapabilitySet::new(CAPABILITIES),
    config_fields: CONFIG_FIELDS,
    requests_per_minute: 60,  // Adjust based on provider's documented limits
};

// ============================================================================
// Factory
// ============================================================================

pub struct MyProviderFactory {
    base_url_override: Option<String>,
}

impl MyProviderFactory {
    pub fn new(base_url_override: Option<String>) -> Self {
        Self { base_url_override }
    }
}

impl ProviderFactory for MyProviderFactory {
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
            .unwrap_or_else(|| "https://api.provider.example.com".to_string());

        let api_key = config
            .get("api_key")
            .and_then(|v| v.as_str())
            .map(String::from);

        Box::new(MyProvider {
            client,
            base_url,
            api_key,
        })
    }
}

// ============================================================================
// Provider Implementation
// ============================================================================

pub struct MyProvider {
    client: reqwest::Client,
    base_url: String,
    api_key: Option<String>,
}

#[async_trait]
impl MetadataProvider for MyProvider {
    fn info(&self) -> &'static ProviderInfo {
        &INFO
    }

    async fn search(&self, query: &MetadataQuery) -> Result<Vec<MetadataResult>, ProviderError> {
        // Build your API request based on the query
        let search_term = if let Some(isbn) = &query.isbn {
            isbn.replace("-", "")
        } else if let Some(title) = &query.title {
            title.clone()
        } else {
            return Ok(Vec::new());
        };

        // Make the API request
        let response = self.client
            .get(&format!("{}/search?q={}", self.base_url, search_term))
            .send()
            .await?;

        // Parse response and convert to MetadataResult
        // ...

        Ok(vec![])
    }

    async fn fetch_cover(&self, url: &str) -> Result<Vec<u8>, ProviderError> {
        let response = self.client.get(url).send().await?;

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
```

### Step 2: Register the provider in mod.rs

Add to `api/src/metadata/mod.rs`:

```rust
mod myprovider;

// In ProviderUrls struct:
pub struct ProviderUrls {
    pub openlibrary: Option<String>,
    pub googlebooks: Option<String>,
    pub hardcover: Option<String>,
    pub myprovider: Option<String>,  // Add this
}

// In ProviderRegistry::with_urls():
factories.insert(
    myprovider::INFO.id,
    Box::new(myprovider::MyProviderFactory::new(urls.myprovider)),
);
```

### Step 3: Add a migration to seed the provider

Create a migration in `api/migrations/`:

```sql
-- Add new provider if not exists
INSERT INTO metadata_providers (id, provider_type, enabled, priority, config)
VALUES (gen_random_uuid(), 'myprovider', false, 40, '{}')
ON CONFLICT (provider_type) DO NOTHING;
```

## Capabilities

Providers declare their capabilities so the system knows what data they can provide:

| Capability | Description |
|------------|-------------|
| `IsbnLookup` | Can search by ISBN-10 or ISBN-13 |
| `TitleSearch` | Can search by book title |
| `AuthorSearch` | Can search by author name |
| `CoverFetch` | Can provide cover images |
| `SeriesInfo` | Returns series name and position |
| `SubjectInfo` | Returns genre/subject information |
| `Descriptions` | Returns book descriptions/summaries |
| `PageCount` | Returns page count |
| `PublisherInfo` | Returns publisher information |
| `PublishDate` | Returns publication date |
| `LanguageInfo` | Returns language code |

## Configuration Fields

Provider configuration is stored as JSONB in the database. Define fields that users can configure:

| Field Type | Description | UI Input |
|------------|-------------|----------|
| `String` | Generic text | Text input |
| `Secret` | API keys, passwords | Password input |
| `Integer` | Numbers | Number input |
| `Boolean` | Toggle values | Checkbox |
| `Url` | URL with validation | Text input |

## MetadataResult

When returning results, populate as many fields as possible:

```rust
let mut result = MetadataResult::new("myprovider", book.title);

result.authors = vec!["Author Name".to_string()];
result.isbn = Some("9781234567890".to_string());
result.description = Some("Book description...".to_string());
result.publisher = Some("Publisher Name".to_string());
result.publish_date = Some("2024-01-15".to_string());
result.language = Some("en".to_string());
result.page_count = Some(350);
result.cover_url = Some("https://covers.example.com/book.jpg".to_string());
result.series = Some("Series Name".to_string());
result.series_index = Some(1.0);
result.subjects = vec!["Fiction".to_string(), "Fantasy".to_string()];
result.external_id = Some("provider_internal_id".to_string());

// Calculate confidence (0.0 - 1.0) based on match quality
result.confidence = 0.8;
```

## Testing

Add tests for your provider in `api/tests/metadata_test.rs`. Use the mock server pattern from existing tests to avoid hitting real APIs during tests.

```rust
#[tokio::test]
async fn test_myprovider_search() {
    let mock_server = MockServer::start().await;

    Mock::given(method("GET"))
        .and(path("/search"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "results": [{ "title": "Test Book" }]
        })))
        .mount(&mock_server)
        .await;

    let pool = setup_test_db().await;
    let urls = ProviderUrls {
        myprovider: Some(mock_server.uri()),
        ..Default::default()
    };

    let registry = ProviderRegistry::with_urls(pool, urls);
    let results = registry.search(&MetadataQuery::by_title("Test")).await;

    assert_eq!(results.len(), 1);
    assert_eq!(results[0].title, "Test Book");
}
```
