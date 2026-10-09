//! Video sync providers for external watch trackers (Trakt, Simkl, etc.)
//!
//! Follows the same trait-based pattern as sync_providers/ but targets
//! video media (movies, TV episodes) instead of books.

pub mod trakt;
pub mod types;

use async_trait::async_trait;
use std::collections::HashMap;

pub use types::{
    DeviceCodeResponse, PollResult, VideoSyncError, VideoSyncProviderInfo, VideoSyncResult,
    WatchHistoryItem,
};

// ============================================================================
// Provider Trait
// ============================================================================

/// Core trait for video sync providers.
#[async_trait]
pub trait VideoSyncProvider: Send + Sync {
    /// Get static information about this provider.
    fn info(&self) -> &'static VideoSyncProviderInfo;

    /// Mark a movie as watched.
    async fn mark_movie_watched(
        &self,
        tmdb_id: &str,
        watched_at: Option<&str>,
    ) -> Result<VideoSyncResult, VideoSyncError>;

    /// Mark a movie as unwatched.
    async fn mark_movie_unwatched(&self, tmdb_id: &str) -> Result<VideoSyncResult, VideoSyncError>;

    /// Mark a TV episode as watched.
    async fn mark_episode_watched(
        &self,
        show_tmdb_id: &str,
        season: u32,
        episode: u32,
        watched_at: Option<&str>,
    ) -> Result<VideoSyncResult, VideoSyncError>;

    /// Mark a TV episode as unwatched.
    async fn mark_episode_unwatched(
        &self,
        show_tmdb_id: &str,
        season: u32,
        episode: u32,
    ) -> Result<VideoSyncResult, VideoSyncError>;

    /// Mark multiple episodes as watched in a single API call.
    /// Each tuple is (season_number, episode_number, optional watched_at ISO 8601).
    async fn mark_episodes_watched(
        &self,
        show_tmdb_id: &str,
        episodes: &[(u32, u32, Option<String>)],
    ) -> Result<VideoSyncResult, VideoSyncError> {
        for (season, episode, watched_at) in episodes {
            self.mark_episode_watched(show_tmdb_id, *season, *episode, watched_at.as_deref())
                .await?;
        }
        Ok(VideoSyncResult::success()
            .with_message("Episodes marked as watched")
            .with_count(episodes.len()))
    }

    /// Mark multiple episodes as unwatched in a single API call.
    /// Each tuple is (season_number, episode_number).
    async fn mark_episodes_unwatched(
        &self,
        show_tmdb_id: &str,
        episodes: &[(u32, u32)],
    ) -> Result<VideoSyncResult, VideoSyncError> {
        for (season, episode) in episodes {
            self.mark_episode_unwatched(show_tmdb_id, *season, *episode)
                .await?;
        }
        Ok(VideoSyncResult::success()
            .with_message("Episodes removed from watch history")
            .with_count(episodes.len()))
    }

    /// Import watch history for movies.
    async fn import_movie_history(&self) -> Result<Vec<WatchHistoryItem>, VideoSyncError>;

    /// Import watch history for TV shows.
    async fn import_show_history(&self) -> Result<Vec<WatchHistoryItem>, VideoSyncError>;
}

// ============================================================================
// Provider Factory
// ============================================================================

/// Factory for creating video sync provider instances.
pub trait VideoSyncProviderFactory: Send + Sync {
    fn info(&self) -> &'static VideoSyncProviderInfo;

    /// Create a provider instance from stored config (access_token, etc.).
    fn create(
        &self,
        client: reqwest::Client,
        config: &serde_json::Value,
    ) -> Option<Box<dyn VideoSyncProvider>>;
}

// ============================================================================
// Registry
// ============================================================================

/// Registry of video sync providers.
pub struct VideoSyncProviderRegistry {
    factories: HashMap<&'static str, Box<dyn VideoSyncProviderFactory>>,
    http_client: reqwest::Client,
}

/// Configuration for provider base URLs (for testing with mock servers).
#[derive(Default)]
pub struct VideoSyncProviderUrls {
    pub trakt: Option<String>,
}

impl VideoSyncProviderRegistry {
    pub fn new() -> Self {
        Self::with_urls(VideoSyncProviderUrls::default())
    }

    /// Create a registry with custom base URLs (for testing).
    pub fn with_urls(urls: VideoSyncProviderUrls) -> Self {
        let http_client = crate::http::new_client();

        // Register Trakt rate limit: 1000 calls per 5 min = 200/min, conservative 180
        crate::http::provider_cooldown().register_rate_limit(crate::http::provider_ids::TRAKT, 180);

        let mut factories: HashMap<&'static str, Box<dyn VideoSyncProviderFactory>> =
            HashMap::new();

        factories.insert(
            trakt::INFO.id,
            Box::new(trakt::TraktSyncFactory::new(urls.trakt)),
        );

        Self {
            factories,
            http_client,
        }
    }

    /// Get information about all registered providers.
    pub fn available_providers(&self) -> Vec<&'static VideoSyncProviderInfo> {
        self.factories.values().map(|f| f.info()).collect()
    }

    /// Create a provider instance from stored config.
    pub fn create_provider(
        &self,
        provider_id: &str,
        config: &serde_json::Value,
    ) -> Option<Box<dyn VideoSyncProvider>> {
        self.factories
            .get(provider_id)
            .and_then(|f| f.create(self.http_client.clone(), config))
    }

    /// Get the HTTP client (for OAuth flows that happen outside the provider instance).
    pub fn http_client(&self) -> &reqwest::Client {
        &self.http_client
    }
}

impl Default for VideoSyncProviderRegistry {
    fn default() -> Self {
        Self::new()
    }
}
