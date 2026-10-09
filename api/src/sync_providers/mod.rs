//! Sync providers for external reading trackers (Hardcover, etc.)
//!
//! This module provides a trait-based system for syncing reading activity
//! to external services like Hardcover, StoryGraph, etc.

pub mod hardcover;
pub mod types;

use async_trait::async_trait;
use std::collections::HashMap;

pub use types::{
    ConfigField, ConfigFieldType, PullSyncBook, PullSyncStatus, SyncAction, SyncBookResult,
    SyncCapabilities, SyncError, SyncProviderInfo, SyncResult,
};

// ============================================================================
// Provider Traits
// ============================================================================

/// Core trait for sync providers
///
/// Implement this trait to create a new sync provider for external reading trackers.
#[async_trait]
pub trait SyncProvider: Send + Sync {
    /// Get static information about this provider
    fn info(&self) -> &'static SyncProviderInfo;

    /// Convenience: get provider ID
    fn id(&self) -> &'static str {
        self.info().id
    }

    /// The key used in books.identifiers JSONB (e.g., "hardcover", "goodreads")
    /// This is how the system knows if a book is linked to this provider
    fn identifier_key(&self) -> &'static str;

    /// Check if book has required identifier (default: check identifier_key exists)
    fn is_book_linked(&self, book_identifiers: &serde_json::Value) -> bool {
        book_identifiers
            .get(self.identifier_key())
            .and_then(|v| v.as_str())
            .map(|s| !s.is_empty())
            .unwrap_or(false)
    }

    /// Get the external book ID from identifiers
    fn get_book_id(&self, book_identifiers: &serde_json::Value) -> Option<String> {
        book_identifiers
            .get(self.identifier_key())
            .and_then(|v| v.as_str())
            .map(String::from)
    }

    /// Mark book as currently reading
    async fn mark_reading(
        &self,
        book_id: &str,
        provider_metadata: &serde_json::Value,
    ) -> Result<SyncResult, SyncError>;

    /// Update reading progress (0-100). page_count is used to calculate progress_pages if available.
    /// If existing_read_id is provided, updates the existing read; otherwise inserts a new one.
    async fn update_progress(
        &self,
        book_id: &str,
        existing_read_id: Option<i64>,
        percent: f64,
        page_count: Option<i32>,
        started_at: Option<&str>,
        provider_metadata: &serde_json::Value,
    ) -> Result<SyncResult, SyncError>;

    /// Mark book as finished. If existing_read_id is provided, closes that read with finished_at.
    async fn mark_finished(
        &self,
        book_id: &str,
        existing_read_id: Option<i64>,
        provider_metadata: &serde_json::Value,
    ) -> Result<SyncResult, SyncError>;

    /// Set rating (1-5 stars)
    async fn set_rating(
        &self,
        book_id: &str,
        rating: i32,
        provider_metadata: &serde_json::Value,
    ) -> Result<SyncResult, SyncError>;

    /// Search for book (for manual linking)
    async fn search_book(&self, query: &str) -> Result<Vec<SyncBookResult>, SyncError>;

    /// Pull user's books from provider (for background pull sync).
    /// Only providers with `pull_sync` capability need to implement this.
    async fn pull_user_books(&self) -> Result<Vec<PullSyncBook>, SyncError> {
        Err(SyncError::Other("pull sync not supported".into()))
    }

    /// Resolve provider-specific edition/format metadata for sync operations.
    /// Returns an edition ID or similar value used in sync mutations.
    /// Default: returns None (no edition resolution needed).
    fn resolve_edition_id(&self, provider_metadata: &serde_json::Value) -> Option<i64> {
        let _ = provider_metadata;
        None
    }
}

/// Factory for creating provider instances
///
/// Each provider type has a factory that creates instances with the given config.
pub trait SyncProviderFactory: Send + Sync {
    /// Get static information about the provider this factory creates
    fn info(&self) -> &'static SyncProviderInfo;

    /// Create a new provider instance with the given HTTP client and API key
    fn create(&self, client: reqwest::Client, api_key: &str) -> Box<dyn SyncProvider>;
}

// ============================================================================
// Provider Registry
// ============================================================================

/// Registry of sync providers
///
/// The registry manages provider factories and creates provider instances on demand.
pub struct SyncProviderRegistry {
    factories: HashMap<&'static str, Box<dyn SyncProviderFactory>>,
    http_client: reqwest::Client,
}

/// Configuration for provider base URLs (used for testing with mock servers)
#[derive(Default)]
pub struct SyncProviderUrls {
    pub overrides: std::collections::HashMap<String, String>,
}

impl SyncProviderUrls {
    /// Set a base URL override for a specific provider.
    pub fn with_override(mut self, provider_id: &str, url: String) -> Self {
        self.overrides.insert(provider_id.to_string(), url);
        self
    }
}

impl SyncProviderRegistry {
    pub fn new() -> Self {
        Self::with_urls(SyncProviderUrls::default())
    }

    /// Create a registry with custom base URLs (for testing with mock servers)
    pub fn with_urls(urls: SyncProviderUrls) -> Self {
        let http_client = crate::http::new_client();

        let mut factories: HashMap<&'static str, Box<dyn SyncProviderFactory>> = HashMap::new();

        // Register built-in providers
        factories.insert(
            hardcover::INFO.id,
            Box::new(hardcover::HardcoverSyncFactory::new(
                urls.overrides.get("hardcover").cloned(),
            )),
        );

        Self {
            factories,
            http_client,
        }
    }

    /// Get information about all registered providers
    pub fn available_providers(&self) -> Vec<&'static SyncProviderInfo> {
        self.factories.values().map(|f| f.info()).collect()
    }

    /// Get all registered factories
    pub fn list_factories(&self) -> Vec<&dyn SyncProviderFactory> {
        self.factories.values().map(|f| f.as_ref()).collect()
    }

    /// Get a specific factory by provider ID
    pub fn get_factory(&self, provider_id: &str) -> Option<&dyn SyncProviderFactory> {
        self.factories.get(provider_id).map(|f| f.as_ref())
    }

    /// Get provider info by ID
    pub fn get_provider_info(&self, provider_id: &str) -> Option<&'static SyncProviderInfo> {
        self.factories.get(provider_id).map(|f| f.info())
    }

    /// Create a provider instance with the given API key
    pub fn create_provider(
        &self,
        provider_id: &str,
        api_key: &str,
    ) -> Option<Box<dyn SyncProvider>> {
        self.factories
            .get(provider_id)
            .map(|f| f.create(self.http_client.clone(), api_key))
    }

    /// Execute a sync action for a device
    ///
    /// Returns Ok(Some(result)) if sync was performed, Ok(None) if book not linked,
    /// or Err if sync failed.
    pub async fn execute_sync(
        &self,
        provider_id: &str,
        api_key: &str,
        book_identifiers: &serde_json::Value,
        action: &SyncAction,
        existing_read_id: Option<i64>,
        provider_metadata: &serde_json::Value,
    ) -> Result<Option<SyncResult>, SyncError> {
        let provider = self
            .create_provider(provider_id, api_key)
            .ok_or_else(|| SyncError::UnknownProvider(provider_id.to_string()))?;

        // Check if book is linked to this provider
        if !provider.is_book_linked(book_identifiers) {
            tracing::debug!(
                provider = provider_id,
                "Book not linked to provider, skipping sync"
            );
            return Ok(None);
        }

        let book_id = provider
            .get_book_id(book_identifiers)
            .ok_or_else(|| SyncError::BookNotLinked)?;

        let result = match action {
            SyncAction::Started => provider.mark_reading(&book_id, provider_metadata).await?,
            SyncAction::Finished => {
                provider
                    .mark_finished(&book_id, existing_read_id, provider_metadata)
                    .await?
            }
            SyncAction::Progress {
                percent,
                page_count,
                started_at,
            } => {
                provider
                    .update_progress(
                        &book_id,
                        existing_read_id,
                        *percent,
                        *page_count,
                        started_at.as_deref(),
                        provider_metadata,
                    )
                    .await?
            }
            SyncAction::Rating { stars } => {
                provider
                    .set_rating(&book_id, *stars, provider_metadata)
                    .await?
            }
        };

        Ok(Some(result))
    }
}

impl Default for SyncProviderRegistry {
    fn default() -> Self {
        Self::new()
    }
}
