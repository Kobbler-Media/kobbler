pub mod download;
mod opensubtitles;
pub mod types;

use async_trait::async_trait;
use sqlx::PgPool;
use std::collections::HashMap;

pub use types::{
    EpisodeSubtitleQuery, MovieSubtitleQuery, StoredSubtitleProvider, SubtitleDownload,
    SubtitleProviderInfo, SubtitleSearchResult, UpdateSubtitleProviderRequest,
};

// ============================================================================
// Provider Traits
// ============================================================================

/// Core trait for subtitle providers.
#[async_trait]
pub trait SubtitleProvider: Send + Sync {
    /// Get static information about this provider.
    fn info(&self) -> &'static SubtitleProviderInfo;

    /// Convenience: get provider ID.
    fn id(&self) -> &'static str {
        self.info().id
    }

    /// Search for movie subtitles.
    async fn search_movie(
        &self,
        query: &MovieSubtitleQuery,
    ) -> Result<Vec<SubtitleSearchResult>, SubtitleProviderError>;

    /// Search for TV episode subtitles.
    async fn search_episode(
        &self,
        query: &EpisodeSubtitleQuery,
    ) -> Result<Vec<SubtitleSearchResult>, SubtitleProviderError>;

    /// Download a subtitle by provider-specific file ID.
    async fn download(&self, file_id: &str) -> Result<SubtitleDownload, SubtitleProviderError>;
}

/// Factory for creating subtitle provider instances.
pub trait SubtitleProviderFactory: Send + Sync {
    fn info(&self) -> &'static SubtitleProviderInfo;
    fn create(
        &self,
        client: reqwest::Client,
        config: &serde_json::Value,
    ) -> Box<dyn SubtitleProvider>;
}

/// Errors from subtitle providers.
#[derive(Debug, thiserror::Error)]
pub enum SubtitleProviderError {
    #[error("HTTP request failed: {0}")]
    Http(#[from] reqwest::Error),

    #[error("Failed to parse response: {0}")]
    Parse(String),

    #[error("Provider rate limited")]
    RateLimited,

    #[error("API key required but not configured")]
    ApiKeyRequired,

    #[error("Download limit reached")]
    DownloadLimitReached,

    #[error("Not found")]
    NotFound,

    #[error("Provider error: {0}")]
    Other(String),
}

// ============================================================================
// Provider Registry
// ============================================================================

/// Registry of subtitle providers.
pub struct SubtitleProviderRegistry {
    factories: HashMap<&'static str, Box<dyn SubtitleProviderFactory>>,
    http_client: reqwest::Client,
    pool: PgPool,
}

impl SubtitleProviderRegistry {
    pub fn new(pool: PgPool) -> Self {
        let http_client = crate::http::new_client();

        let mut factories: HashMap<&'static str, Box<dyn SubtitleProviderFactory>> = HashMap::new();

        factories.insert(
            opensubtitles::INFO.id,
            Box::new(opensubtitles::OpenSubtitlesFactory),
        );

        Self {
            factories,
            http_client,
            pool,
        }
    }

    /// Get information about all registered providers.
    pub fn available_providers(&self) -> Vec<&'static SubtitleProviderInfo> {
        self.factories.values().map(|f| f.info()).collect()
    }

    /// Get provider info by type.
    pub fn get_provider_info(&self, provider_type: &str) -> Option<&'static SubtitleProviderInfo> {
        self.factories.get(provider_type).map(|f| f.info())
    }

    /// Search enabled providers for movie subtitles.
    pub async fn search_movie(
        &self,
        query: &MovieSubtitleQuery,
        provider_filter: Option<&str>,
    ) -> Vec<SubtitleSearchResult> {
        let enabled = self.get_enabled_providers().await;
        let mut all_results = Vec::new();

        for stored in enabled {
            if let Some(filter) = provider_filter
                && stored.provider_type != filter
            {
                continue;
            }

            let Some(factory) = self.factories.get(stored.provider_type.as_str()) else {
                tracing::warn!("Unknown subtitle provider type: {}", stored.provider_type);
                continue;
            };

            if !factory
                .info()
                .capabilities
                .has(types::SubtitleCapability::MovieSearch)
            {
                continue;
            }

            let provider = factory.create(self.http_client.clone(), &stored.config);
            match provider.search_movie(query).await {
                Ok(results) => {
                    tracing::debug!(
                        "Subtitle provider {} returned {} movie results",
                        stored.provider_type,
                        results.len()
                    );
                    all_results.extend(results);
                }
                Err(e) => {
                    tracing::warn!("Subtitle provider {} error: {}", stored.provider_type, e);
                }
            }
        }

        all_results
    }

    /// Search enabled providers for TV episode subtitles.
    pub async fn search_episode(
        &self,
        query: &EpisodeSubtitleQuery,
        provider_filter: Option<&str>,
    ) -> Vec<SubtitleSearchResult> {
        let enabled = self.get_enabled_providers().await;
        let mut all_results = Vec::new();

        for stored in enabled {
            if let Some(filter) = provider_filter
                && stored.provider_type != filter
            {
                continue;
            }

            let Some(factory) = self.factories.get(stored.provider_type.as_str()) else {
                tracing::warn!("Unknown subtitle provider type: {}", stored.provider_type);
                continue;
            };

            if !factory
                .info()
                .capabilities
                .has(types::SubtitleCapability::EpisodeSearch)
            {
                continue;
            }

            let provider = factory.create(self.http_client.clone(), &stored.config);
            match provider.search_episode(query).await {
                Ok(results) => {
                    tracing::debug!(
                        "Subtitle provider {} returned {} episode results",
                        stored.provider_type,
                        results.len()
                    );
                    all_results.extend(results);
                }
                Err(e) => {
                    tracing::warn!("Subtitle provider {} error: {}", stored.provider_type, e);
                }
            }
        }

        all_results
    }

    /// Download a subtitle from a specific provider.
    pub async fn download(
        &self,
        provider_type: &str,
        file_id: &str,
    ) -> Result<SubtitleDownload, SubtitleProviderError> {
        let stored = self.get_provider_config(provider_type).await?;
        let factory = self.factories.get(provider_type).ok_or_else(|| {
            SubtitleProviderError::Other(format!("Unknown provider: {provider_type}"))
        })?;

        let provider = factory.create(self.http_client.clone(), &stored.config);
        provider.download(file_id).await
    }

    /// Get all providers (for admin UI).
    pub async fn get_all_providers(&self) -> Vec<StoredSubtitleProvider> {
        use crate::repositories::subtitle_providers;
        match subtitle_providers::list_all(&self.pool).await {
            Ok(providers) => providers
                .into_iter()
                .map(|p| StoredSubtitleProvider {
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
                tracing::warn!("Failed to get all subtitle providers: {}", e);
                Vec::new()
            }
        }
    }

    /// Update provider configuration.
    pub async fn update_provider(
        &self,
        provider_id: uuid::Uuid,
        update: &UpdateSubtitleProviderRequest,
    ) -> Result<(), sqlx::Error> {
        use crate::repositories::subtitle_providers::{self, ProviderUpdate};

        let repo_update = ProviderUpdate {
            enabled: update.enabled,
            priority: update.priority,
            config: update
                .config
                .as_ref()
                .map(|m| m.iter().map(|(k, v)| (k.clone(), v.clone())).collect()),
        };

        subtitle_providers::update_with_merge(&self.pool, provider_id, &repo_update)
            .await
            .map_err(|e| match e {
                crate::repositories::RepositoryError::Database(e) => e,
                crate::repositories::RepositoryError::Validation(msg) => sqlx::Error::Protocol(msg),
            })?;

        Ok(())
    }

    // ========================================================================
    // Internal helpers
    // ========================================================================

    async fn get_enabled_providers(&self) -> Vec<StoredSubtitleProvider> {
        use crate::repositories::subtitle_providers;
        match subtitle_providers::list_enabled(&self.pool).await {
            Ok(providers) => providers
                .into_iter()
                .map(|p| StoredSubtitleProvider {
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
                tracing::warn!("Failed to get enabled subtitle providers: {}", e);
                Vec::new()
            }
        }
    }

    async fn get_provider_config(
        &self,
        provider_type: &str,
    ) -> Result<StoredSubtitleProvider, SubtitleProviderError> {
        use crate::repositories::subtitle_providers;
        let providers = subtitle_providers::list_enabled(&self.pool)
            .await
            .map_err(|e| SubtitleProviderError::Other(format!("DB error: {e}")))?;

        providers
            .into_iter()
            .find(|p| p.provider_type == provider_type)
            .map(|p| StoredSubtitleProvider {
                id: p.id,
                provider_type: p.provider_type,
                enabled: p.enabled,
                priority: p.priority,
                config: p.config,
                created_at: p.created_at,
                updated_at: p.updated_at,
            })
            .ok_or_else(|| {
                SubtitleProviderError::Other(format!("Provider {provider_type} not enabled"))
            })
    }
}
