pub mod audnexus;
mod cache;
mod googlebooks;
pub mod hardcover;
pub mod merge;
mod openlibrary;
pub(crate) mod similarity;
pub mod types;

use async_trait::async_trait;
use sqlx::PgPool;
use std::collections::HashMap;

pub use cache::MetadataCache;
pub use types::{
    AuthorWithId, Capability, CapabilitySet, Chapter, ConfigField, ConfigFieldType, MetadataQuery,
    MetadataResult, ProviderInfo, SearchOutcome, SeriesBook, SeriesQuery, SeriesSearchOutcome,
    SeriesSearchResult, StoredProvider, UpdateProviderRequest,
};

// ============================================================================
// Provider Traits
// ============================================================================

/// Core trait for metadata providers
///
/// Implement this trait to create a new metadata provider.
#[async_trait]
pub trait MetadataProvider: Send + Sync {
    /// Get static information about this provider
    fn info(&self) -> &'static ProviderInfo;

    /// Convenience: get provider ID
    fn id(&self) -> &'static str {
        self.info().id
    }

    /// Convenience: check if provider supports a capability
    fn supports(&self, cap: Capability) -> bool {
        self.info().capabilities.has(cap)
    }

    /// Search for books matching the query
    async fn search(&self, query: &MetadataQuery) -> Result<Vec<MetadataResult>, ProviderError>;

    /// Search for series by name
    /// Default implementation returns empty results (provider doesn't support series search)
    async fn search_series(
        &self,
        query: &SeriesQuery,
    ) -> Result<Vec<SeriesSearchResult>, ProviderError> {
        let _ = query;
        Ok(Vec::new())
    }

    /// Fetch cover image bytes from a URL
    async fn fetch_cover(&self, url: &str) -> Result<Vec<u8>, ProviderError>;
}

/// Factory for creating provider instances
///
/// Each provider type has a factory that creates instances with the given config.
pub trait ProviderFactory: Send + Sync {
    /// Get static information about the provider this factory creates
    fn info(&self) -> &'static ProviderInfo;

    /// Create a new provider instance with the given HTTP client and config
    fn create(
        &self,
        client: reqwest::Client,
        config: &serde_json::Value,
    ) -> Box<dyn MetadataProvider>;
}

/// Errors from metadata providers
#[derive(Debug, thiserror::Error)]
pub enum ProviderError {
    #[error("HTTP request failed: {0}")]
    Http(#[from] reqwest::Error),

    #[error("Failed to parse response: {0}")]
    Parse(String),

    #[error("Provider rate limited")]
    RateLimited,

    #[error("Provider error: {0}")]
    Other(String),
}

impl ProviderError {
    /// Convert an HTTP error response into the appropriate `ProviderError`.
    ///
    /// Returns `RateLimited` for 429 status codes so the cooldown system
    /// can engage, and `Other` for everything else.
    pub fn from_http_status(status: reqwest::StatusCode, context: &str) -> Self {
        if status == reqwest::StatusCode::TOO_MANY_REQUESTS {
            Self::RateLimited
        } else {
            Self::Other(format!("{context}: {status}"))
        }
    }
}

// ============================================================================
// Provider Registry
// ============================================================================

/// Registry of metadata providers
///
/// The registry manages provider factories and creates provider instances on demand.
pub struct ProviderRegistry {
    factories: HashMap<&'static str, Box<dyn ProviderFactory>>,
    http_client: reqwest::Client,
    cache: MetadataCache,
    pool: PgPool,
    cooldown: &'static crate::http::ProviderCooldown,
}

/// Configuration for provider base URLs (used for testing with mock servers)
#[derive(Default)]
pub struct ProviderUrls {
    pub openlibrary: Option<String>,
    pub googlebooks: Option<String>,
    pub hardcover: Option<String>,
    pub audnexus: Option<String>,
}

impl ProviderRegistry {
    pub fn new(pool: PgPool) -> Self {
        Self::with_urls(pool, ProviderUrls::default())
    }

    /// Create a registry with custom base URLs (for testing with mock servers)
    pub fn with_urls(pool: PgPool, urls: ProviderUrls) -> Self {
        let cache = MetadataCache::new(pool.clone());
        let http_client = crate::http::new_client();

        let mut factories: HashMap<&'static str, Box<dyn ProviderFactory>> = HashMap::new();

        // Register built-in providers
        factories.insert(
            openlibrary::INFO.id,
            Box::new(openlibrary::OpenLibraryFactory::new(urls.openlibrary)),
        );
        factories.insert(
            googlebooks::INFO.id,
            Box::new(googlebooks::GoogleBooksFactory::new(urls.googlebooks)),
        );
        factories.insert(
            hardcover::INFO.id,
            Box::new(hardcover::HardcoverFactory::new(urls.hardcover)),
        );
        factories.insert(
            audnexus::INFO.id,
            Box::new(audnexus::AudnexusFactory::new(urls.audnexus)),
        );

        // Register rate limits for all providers with the shared cooldown
        let cooldown = crate::http::provider_cooldown();
        for factory in factories.values() {
            let info = factory.info();
            cooldown.register_rate_limit(info.id, info.requests_per_minute);
        }

        Self {
            factories,
            http_client,
            cache,
            pool,
            cooldown,
        }
    }

    /// Get information about all registered providers
    pub fn available_providers(&self) -> Vec<&'static ProviderInfo> {
        self.factories.values().map(|f| f.info()).collect()
    }

    /// Get provider info by type
    pub fn get_provider_info(&self, provider_type: &str) -> Option<&'static ProviderInfo> {
        self.factories.get(provider_type).map(|f| f.info())
    }

    /// Search enabled providers and aggregate results
    /// If `provider_filter` is Some, only query that specific provider
    pub async fn search(
        &self,
        query: &MetadataQuery,
        provider_filter: Option<&str>,
    ) -> SearchOutcome {
        // Get enabled providers from database, ordered by priority
        let enabled = self.get_enabled_providers().await;

        let mut all_results = Vec::new();
        let cache_key = query.cache_key();
        let mut any_api_called = false;

        for stored in enabled {
            // If provider filter is set, skip providers that don't match
            if let Some(filter) = provider_filter
                && stored.provider_type != filter
            {
                continue;
            }

            // Get factory for this provider type
            let Some(factory) = self.factories.get(stored.provider_type.as_str()) else {
                tracing::warn!("Unknown provider type: {}", stored.provider_type);
                continue;
            };

            // If query is free-text only, skip providers without FreeTextSearch capability
            if query.free_text.is_some()
                && query.isbn.is_none()
                && query.title.is_none()
                && query.asin.is_none()
                && !factory.info().capabilities.has(Capability::FreeTextSearch)
            {
                tracing::debug!(
                    "Provider {} does not support free-text search, skipping",
                    stored.provider_type
                );
                continue;
            }

            // Check cache first
            if let Ok(Some(cached)) = self.cache.get(&stored.provider_type, &cache_key).await {
                tracing::debug!(
                    "Cache hit for {} query: {}",
                    stored.provider_type,
                    cache_key
                );
                all_results.extend(cached);
                continue;
            }

            // Wait for any active cooldown before calling provider.
            self.cooldown.wait_if_needed(&stored.provider_type).await;

            // Create provider instance with config
            let provider = factory.create(self.http_client.clone(), &stored.config);

            // Query provider
            any_api_called = true;
            match provider.search(query).await {
                Ok(results) => {
                    self.cooldown.reset(&stored.provider_type);
                    tracing::debug!(
                        "Provider {} returned {} results",
                        stored.provider_type,
                        results.len()
                    );

                    // Cache results (errors are not cached, so missing API key etc. won't be cached)
                    if let Err(e) = self
                        .cache
                        .set(&stored.provider_type, &cache_key, &results)
                        .await
                    {
                        tracing::warn!("Failed to cache results: {}", e);
                    }

                    all_results.extend(results);
                }
                Err(ProviderError::RateLimited) => {
                    self.cooldown.set_cooldown(&stored.provider_type, 10);
                }
                Err(e) => {
                    tracing::warn!("Provider {} error: {}", stored.provider_type, e);
                }
            }
        }

        // Sort by confidence (descending)
        all_results.sort_by(|a, b| {
            b.confidence
                .partial_cmp(&a.confidence)
                .unwrap_or(std::cmp::Ordering::Equal)
        });

        SearchOutcome {
            results: all_results,
            all_cached: !any_api_called,
        }
    }

    /// Search enabled providers for series by name and aggregate results
    /// If `provider_filter` is Some, only query that specific provider
    /// Only queries providers that have the SeriesSearch capability
    pub async fn search_series(
        &self,
        query: &SeriesQuery,
        provider_filter: Option<&str>,
    ) -> SeriesSearchOutcome {
        // Get enabled providers from database, ordered by priority
        let enabled = self.get_enabled_providers().await;

        let mut all_results = Vec::new();
        let cache_key = query.cache_key();
        let mut any_api_called = false;

        for stored in enabled {
            // If provider filter is set, skip providers that don't match
            if let Some(filter) = provider_filter
                && stored.provider_type != filter
            {
                continue;
            }

            // Get factory for this provider type
            let Some(factory) = self.factories.get(stored.provider_type.as_str()) else {
                tracing::warn!("Unknown provider type: {}", stored.provider_type);
                continue;
            };

            // Check if this provider supports series search
            if !factory.info().capabilities.has(Capability::SeriesSearch) {
                tracing::debug!(
                    "Provider {} does not support series search, skipping",
                    stored.provider_type
                );
                continue;
            }

            // Check cache first
            if let Ok(Some(cached)) = self
                .cache
                .get_series(&stored.provider_type, &cache_key)
                .await
            {
                tracing::debug!(
                    "Cache hit for {} series query: {}",
                    stored.provider_type,
                    cache_key
                );
                all_results.extend(cached);
                continue;
            }

            // Wait for any active cooldown before calling provider.
            self.cooldown.wait_if_needed(&stored.provider_type).await;

            // Create provider instance with config
            let provider = factory.create(self.http_client.clone(), &stored.config);

            // Query provider
            any_api_called = true;
            match provider.search_series(query).await {
                Ok(results) => {
                    self.cooldown.reset(&stored.provider_type);
                    tracing::debug!(
                        "Provider {} returned {} series results",
                        stored.provider_type,
                        results.len()
                    );

                    // Cache results (errors are not cached, so missing API key etc. won't be cached)
                    if let Err(e) = self
                        .cache
                        .set_series(&stored.provider_type, &cache_key, &results)
                        .await
                    {
                        tracing::warn!("Failed to cache series results: {}", e);
                    }

                    all_results.extend(results);
                }
                Err(ProviderError::RateLimited) => {
                    self.cooldown.set_cooldown(&stored.provider_type, 10);
                }
                Err(e) => {
                    tracing::warn!(
                        "Provider {} series search error: {}",
                        stored.provider_type,
                        e
                    );
                }
            }
        }

        // Sort by confidence (descending)
        all_results.sort_by(|a, b| {
            b.confidence
                .partial_cmp(&a.confidence)
                .unwrap_or(std::cmp::Ordering::Equal)
        });

        SeriesSearchOutcome {
            results: all_results,
            all_cached: !any_api_called,
        }
    }

    /// Check if any enabled provider supports series search and is properly configured
    /// Returns Ok(()) if at least one provider can handle series search
    /// Returns Err with a helpful message if no provider is available
    pub async fn check_series_search_availability(
        &self,
        provider_filter: Option<&str>,
    ) -> Result<(), String> {
        let enabled = self.get_enabled_providers().await;

        let mut has_capable_provider = false;
        let mut unconfigured_providers: Vec<&str> = Vec::new();

        for stored in &enabled {
            // If provider filter is set, skip providers that don't match
            if let Some(filter) = provider_filter
                && stored.provider_type != filter
            {
                continue;
            }

            // Get factory for this provider type
            let Some(factory) = self.factories.get(stored.provider_type.as_str()) else {
                continue;
            };

            // Check if this provider supports series search
            if !factory.info().capabilities.has(Capability::SeriesSearch) {
                continue;
            }

            // Provider supports series search - check if it's configured
            // For hardcover specifically, check for API key
            if stored.provider_type == crate::http::provider_ids::HARDCOVER {
                let has_api_key = stored
                    .config
                    .get("api_key")
                    .and_then(|v| v.as_str())
                    .map(|s| !s.is_empty())
                    .unwrap_or(false);

                if !has_api_key {
                    unconfigured_providers.push("Hardcover (requires API key)");
                    continue;
                }
            }

            has_capable_provider = true;
        }

        if has_capable_provider {
            return Ok(());
        }

        // Build helpful error message
        if !unconfigured_providers.is_empty() {
            return Err(format!(
                "No configured providers support series search. The following providers support it but need configuration: {}. Configure them in Settings > Metadata Providers.",
                unconfigured_providers.join(", ")
            ));
        }

        if provider_filter.is_some() {
            return Err(
                "The specified provider does not support series search or is not enabled."
                    .to_string(),
            );
        }

        Err(
            "No enabled providers support series search. Enable and configure Hardcover in Settings > Metadata Providers to search for series.".to_string()
        )
    }

    /// Fetch a cover image from a specific provider
    pub async fn fetch_cover(
        &self,
        provider_type: &str,
        url: &str,
    ) -> Result<Vec<u8>, ProviderError> {
        let factory = self
            .factories
            .get(provider_type)
            .ok_or_else(|| ProviderError::Other(format!("Unknown provider: {}", provider_type)))?;

        // Create a provider with empty config just for cover fetching
        let provider = factory.create(self.http_client.clone(), &serde_json::json!({}));
        provider.fetch_cover(url).await
    }

    /// Get list of enabled providers from database
    async fn get_enabled_providers(&self) -> Vec<StoredProvider> {
        use crate::repositories::metadata_providers;
        match metadata_providers::list_enabled(&self.pool).await {
            Ok(providers) => providers
                .into_iter()
                .map(|p| StoredProvider {
                    id: p.id,
                    provider_type: p.provider_type,
                    enabled: p.enabled,
                    priority: p.priority,
                    config: p.config,
                    created_at: p.created_at,
                    updated_at: p.updated_at,
                })
                .collect(),
            Err(e) => {
                tracing::warn!("Failed to get enabled providers: {}", e);
                Vec::new()
            }
        }
    }

    /// Get all providers (for admin UI)
    pub async fn get_all_providers(&self) -> Vec<StoredProvider> {
        use crate::repositories::metadata_providers;
        match metadata_providers::list_all(&self.pool).await {
            Ok(providers) => providers
                .into_iter()
                .map(|p| StoredProvider {
                    id: p.id,
                    provider_type: p.provider_type,
                    enabled: p.enabled,
                    priority: p.priority,
                    config: p.config,
                    created_at: p.created_at,
                    updated_at: p.updated_at,
                })
                .collect(),
            Err(e) => {
                tracing::warn!("Failed to get all providers: {}", e);
                Vec::new()
            }
        }
    }

    /// Update provider configuration
    pub async fn update_provider(
        &self,
        provider_id: uuid::Uuid,
        update: &UpdateProviderRequest,
    ) -> Result<(), sqlx::Error> {
        use crate::repositories::metadata_providers::{self, ProviderUpdate};

        let repo_update = ProviderUpdate {
            enabled: update.enabled,
            priority: update.priority,
            config: update
                .config
                .as_ref()
                .map(|m| m.iter().map(|(k, v)| (k.clone(), v.clone())).collect()),
        };

        metadata_providers::update_with_merge(&self.pool, provider_id, &repo_update)
            .await
            .map_err(|e| match e {
                crate::repositories::RepositoryError::Database(e) => e,
                crate::repositories::RepositoryError::Validation(msg) => sqlx::Error::Protocol(msg),
            })?;

        Ok(())
    }
}
