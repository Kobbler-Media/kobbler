pub mod cinemeta;
pub mod merge;
mod omdb;
pub mod release_dates;
pub mod thesportsdb;
pub mod tmdb;
mod tvdb;
pub mod tvmaze;
pub mod types;

use async_trait::async_trait;
use sqlx::PgPool;
use std::collections::HashMap;

pub use types::{
    EpisodeCredits, EpisodeDetails, MovieDetails, MovieQuery, MovieResult, MovieSearchOutcome,
    PersonCombinedCredits, PersonProviderDetails, SeasonDetails, StoredVideoProvider, TvQuery,
    TvSearchOutcome, TvShowDetails, TvShowResult, UpdateVideoProviderRequest, VideoProviderInfo,
};

// ============================================================================
// Cache freshness
// ============================================================================

/// Most metadata is effectively immutable — an ended show's back catalogue does
/// not change — so it caches for a week.
const CACHE_TTL_STABLE: time::Duration = time::Duration::days(7);

/// Data that can gain entries between polls: a continuing show's season list
/// and any season still airing. `tv_monitor` runs hourly to pick up new
/// episodes, so anything longer than a day makes it silently useless — it was
/// reading week-old season data and reporting success.
const CACHE_TTL_ACTIVE: time::Duration = time::Duration::days(1);

/// A season counts as active until it has been off the air for this long.
/// Covers late provider edits (episode titles, stills, air-date corrections)
/// landing after the finale.
const SEASON_ACTIVE_AFTER_LAST_AIRING: time::Duration = time::Duration::days(30);

fn tv_show_detail_ttl(details: &TvShowDetails) -> time::Duration {
    match details.status.as_deref() {
        Some("ended") | Some("canceled") => CACHE_TTL_STABLE,
        _ => CACHE_TTL_ACTIVE,
    }
}

/// A season is active when it has no episodes yet, when we cannot date any of
/// its episodes, or when its most recent airing is recent (or still ahead).
fn season_ttl(details: &SeasonDetails) -> time::Duration {
    if details.episodes.is_empty() {
        return CACHE_TTL_ACTIVE;
    }
    let Some(last_air_date) = details
        .episodes
        .iter()
        .filter_map(|e| e.air_date.as_deref())
        .max()
        .and_then(parse_iso_date)
    else {
        return CACHE_TTL_ACTIVE;
    };
    let cutoff = time::OffsetDateTime::now_utc().date() - SEASON_ACTIVE_AFTER_LAST_AIRING;
    if last_air_date >= cutoff {
        CACHE_TTL_ACTIVE
    } else {
        CACHE_TTL_STABLE
    }
}

/// Parse a provider air *time* (RFC 3339) into an instant. Returns `None` for
/// anything unparseable — a bad string must never become a confident time.
pub fn parse_air_time(s: &str) -> Option<time::OffsetDateTime> {
    time::OffsetDateTime::parse(s, &time::format_description::well_known::Rfc3339).ok()
}

fn parse_iso_date(s: &str) -> Option<time::Date> {
    time::Date::parse(s, &time::format_description::well_known::Iso8601::DEFAULT).ok()
}

// ============================================================================
// Provider Traits
// ============================================================================

/// Core trait for video metadata providers.
#[async_trait]
pub trait VideoMetadataProvider: Send + Sync {
    /// Get static information about this provider.
    fn info(&self) -> &'static VideoProviderInfo;

    /// Convenience: get provider ID.
    fn id(&self) -> &'static str {
        self.info().id
    }

    /// Search for movies matching the query.
    async fn search_movies(
        &self,
        query: &MovieQuery,
    ) -> Result<Vec<MovieResult>, VideoProviderError>;

    /// Search for TV shows matching the query.
    async fn search_tv(&self, query: &TvQuery) -> Result<Vec<TvShowResult>, VideoProviderError>;

    /// Get full movie details by provider-specific ID.
    async fn get_movie(&self, provider_id: &str) -> Result<MovieDetails, VideoProviderError>;

    /// Get full TV show details by provider-specific ID.
    async fn get_tv_show(&self, provider_id: &str) -> Result<TvShowDetails, VideoProviderError>;

    /// Get season details (episodes) for a TV show.
    async fn get_season(
        &self,
        show_id: &str,
        season: u32,
    ) -> Result<SeasonDetails, VideoProviderError>;

    /// Fetch image bytes from a URL.
    async fn fetch_image(&self, url: &str) -> Result<Vec<u8>, VideoProviderError>;

    /// Get person details (bio, dates, identifiers).
    async fn get_person(
        &self,
        _provider_id: &str,
    ) -> Result<PersonProviderDetails, VideoProviderError> {
        Err(VideoProviderError::NotSupported)
    }

    /// Get person combined credits (filmography).
    async fn get_person_credits(
        &self,
        _provider_id: &str,
    ) -> Result<PersonCombinedCredits, VideoProviderError> {
        Err(VideoProviderError::NotSupported)
    }

    /// Get the credits specific to one episode — guest stars and episode crew.
    async fn get_episode_credits(
        &self,
        _show_id: &str,
        _season: u32,
        _episode: u32,
    ) -> Result<EpisodeCredits, VideoProviderError> {
        Err(VideoProviderError::NotSupported)
    }
}

/// Factory for creating video provider instances.
pub trait VideoProviderFactory: Send + Sync {
    fn info(&self) -> &'static VideoProviderInfo;
    fn create(
        &self,
        client: reqwest::Client,
        config: &serde_json::Value,
    ) -> Box<dyn VideoMetadataProvider>;
}

/// Errors from video metadata providers.
#[derive(Debug, thiserror::Error)]
pub enum VideoProviderError {
    #[error("HTTP request failed: {0}")]
    Http(reqwest::Error),

    #[error("Failed to parse response: {0}")]
    Parse(String),

    #[error("Provider rate limited")]
    RateLimited,

    #[error("Not found")]
    NotFound,

    #[error("Capability not supported")]
    NotSupported,

    #[error("Provider error: {0}")]
    Other(String),
}

impl From<reqwest::Error> for VideoProviderError {
    fn from(e: reqwest::Error) -> Self {
        // reqwest errors from .error_for_status() carry the HTTP status code.
        // Detect 429 so all providers (TMDB, TVMaze, etc.) surface rate limits
        // as RateLimited without needing per-provider status checks.
        if e.status() == Some(reqwest::StatusCode::TOO_MANY_REQUESTS) {
            tracing::warn!("Provider rate limited (429): {e}");
            return VideoProviderError::RateLimited;
        }
        VideoProviderError::Http(e)
    }
}

// ============================================================================
// Provider Registry
// ============================================================================

/// Registry of video metadata providers.
pub struct VideoProviderRegistry {
    factories: HashMap<&'static str, Box<dyn VideoProviderFactory>>,
    http_client: reqwest::Client,
    pool: PgPool,
    cooldown: &'static crate::http::ProviderCooldown,
}

impl VideoProviderRegistry {
    pub fn new(pool: PgPool) -> Self {
        let http_client = crate::http::new_client();

        let mut factories: HashMap<&'static str, Box<dyn VideoProviderFactory>> = HashMap::new();

        factories.insert(tmdb::INFO.id, Box::new(tmdb::TmdbFactory));
        factories.insert(tvdb::INFO.id, Box::new(tvdb::TvdbFactory));
        factories.insert(tvmaze::INFO.id, Box::new(tvmaze::TvMazeFactory));
        factories.insert(omdb::INFO.id, Box::new(omdb::OmdbFactory));
        factories.insert(
            thesportsdb::INFO.id,
            Box::new(thesportsdb::TheSportsDbFactory),
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
            pool,
            cooldown,
        }
    }

    /// Get information about all registered providers.
    pub fn available_providers(&self) -> Vec<&'static VideoProviderInfo> {
        self.factories.values().map(|f| f.info()).collect()
    }

    /// Get provider info by type.
    pub fn get_provider_info(&self, provider_type: &str) -> Option<&'static VideoProviderInfo> {
        self.factories.get(provider_type).map(|f| f.info())
    }

    /// Search enabled providers for movies.
    pub async fn search_movies(
        &self,
        query: &MovieQuery,
        provider_filter: Option<&str>,
    ) -> MovieSearchOutcome {
        let enabled = self.get_enabled_providers().await;
        let mut all_results = Vec::new();
        let cache_key = query.cache_key();
        let mut any_api_called = false;

        for stored in enabled {
            if let Some(filter) = provider_filter
                && stored.provider_type != filter
            {
                continue;
            }

            let Some(factory) = self.factories.get(stored.provider_type.as_str()) else {
                tracing::warn!("Unknown video provider type: {}", stored.provider_type);
                continue;
            };

            if !factory
                .info()
                .capabilities
                .has(types::VideoCapability::MovieSearch)
            {
                continue;
            }

            // Check cache
            if let Ok(Some(cached)) = self.cache_get(&stored.provider_type, &cache_key).await
                && let Ok(results) = serde_json::from_value::<Vec<MovieResult>>(cached)
            {
                tracing::debug!(
                    "Cache hit for {} movie query: {}",
                    stored.provider_type,
                    cache_key
                );
                all_results.extend(results);
                continue;
            }

            self.cooldown.wait_if_needed(&stored.provider_type).await;
            let provider = factory.create(self.http_client.clone(), &stored.config);
            any_api_called = true;
            match provider.search_movies(query).await {
                Ok(results) => {
                    self.cooldown.reset(&stored.provider_type);
                    tracing::debug!(
                        "Video provider {} returned {} movie results",
                        stored.provider_type,
                        results.len()
                    );
                    // Only cache non-empty results to avoid caching transient API failures
                    if !results.is_empty()
                        && let Ok(json) = serde_json::to_value(&results)
                    {
                        let _ = self
                            .cache_set(&stored.provider_type, &cache_key, &json)
                            .await;
                    }
                    all_results.extend(results);
                }
                Err(VideoProviderError::RateLimited) => {
                    self.cooldown.set_cooldown(&stored.provider_type, 10);
                }
                Err(e) => {
                    tracing::warn!("Video provider {} error: {}", stored.provider_type, e);
                }
            }
        }

        MovieSearchOutcome {
            results: all_results,
            all_cached: !any_api_called,
        }
    }

    /// Search enabled providers for TV shows.
    pub async fn search_tv(
        &self,
        query: &TvQuery,
        provider_filter: Option<&str>,
    ) -> TvSearchOutcome {
        let enabled = self.get_enabled_providers().await;
        let mut all_results = Vec::new();
        let cache_key = query.cache_key();
        let mut any_api_called = false;

        for stored in enabled {
            if let Some(filter) = provider_filter
                && stored.provider_type != filter
            {
                continue;
            }

            let Some(factory) = self.factories.get(stored.provider_type.as_str()) else {
                tracing::warn!("Unknown video provider type: {}", stored.provider_type);
                continue;
            };

            if !factory
                .info()
                .capabilities
                .has(types::VideoCapability::TvSearch)
            {
                continue;
            }

            // Check cache
            if let Ok(Some(cached)) = self.cache_get(&stored.provider_type, &cache_key).await
                && let Ok(results) = serde_json::from_value::<Vec<TvShowResult>>(cached)
            {
                tracing::debug!(
                    "Cache hit for {} TV query: {}",
                    stored.provider_type,
                    cache_key
                );
                all_results.extend(results);
                continue;
            }

            self.cooldown.wait_if_needed(&stored.provider_type).await;
            let provider = factory.create(self.http_client.clone(), &stored.config);
            any_api_called = true;
            match provider.search_tv(query).await {
                Ok(results) => {
                    self.cooldown.reset(&stored.provider_type);
                    tracing::debug!(
                        "Video provider {} returned {} TV results",
                        stored.provider_type,
                        results.len()
                    );
                    // Only cache non-empty results to avoid caching transient API failures
                    if !results.is_empty()
                        && let Ok(json) = serde_json::to_value(&results)
                    {
                        let _ = self
                            .cache_set(&stored.provider_type, &cache_key, &json)
                            .await;
                    }
                    all_results.extend(results);
                }
                Err(VideoProviderError::RateLimited) => {
                    self.cooldown.set_cooldown(&stored.provider_type, 10);
                }
                Err(e) => {
                    tracing::warn!("Video provider {} error: {}", stored.provider_type, e);
                }
            }
        }

        TvSearchOutcome {
            results: all_results,
            all_cached: !any_api_called,
        }
    }

    /// Get movie details from a specific provider.
    pub async fn get_movie(
        &self,
        provider_type: &str,
        provider_id: &str,
    ) -> Result<MovieDetails, VideoProviderError> {
        let stored = self.get_provider_config(provider_type).await?;
        let factory = self.factories.get(provider_type).ok_or_else(|| {
            VideoProviderError::Other(format!("Unknown provider: {provider_type}"))
        })?;

        let cache_key = format!("video_movie_detail:{}:{}", provider_type, provider_id);
        if let Ok(Some(cached)) = self.cache_get(provider_type, &cache_key).await
            && let Ok(details) = serde_json::from_value::<MovieDetails>(cached)
        {
            return Ok(details);
        }

        self.cooldown.wait_if_needed(provider_type).await;
        let provider = factory.create(self.http_client.clone(), &stored.config);
        let details = match provider.get_movie(provider_id).await {
            Ok(d) => {
                self.cooldown.reset(provider_type);
                d
            }
            Err(VideoProviderError::RateLimited) => {
                self.cooldown.set_cooldown(provider_type, 10);
                return Err(VideoProviderError::RateLimited);
            }
            Err(e) => return Err(e),
        };

        if let Ok(json) = serde_json::to_value(&details) {
            let _ = self.cache_set(provider_type, &cache_key, &json).await;
        }

        Ok(details)
    }

    /// Get TV show details from a specific provider.
    pub async fn get_tv_show(
        &self,
        provider_type: &str,
        provider_id: &str,
    ) -> Result<TvShowDetails, VideoProviderError> {
        let stored = self.get_provider_config(provider_type).await?;
        let factory = self.factories.get(provider_type).ok_or_else(|| {
            VideoProviderError::Other(format!("Unknown provider: {provider_type}"))
        })?;

        let cache_key = format!("video_tv_detail:{}:{}", provider_type, provider_id);
        if let Ok(Some(cached)) = self.cache_get(provider_type, &cache_key).await
            && let Ok(details) = serde_json::from_value::<TvShowDetails>(cached)
        {
            return Ok(details);
        }

        self.cooldown.wait_if_needed(provider_type).await;
        let provider = factory.create(self.http_client.clone(), &stored.config);
        let details = match provider.get_tv_show(provider_id).await {
            Ok(d) => {
                self.cooldown.reset(provider_type);
                d
            }
            Err(VideoProviderError::RateLimited) => {
                self.cooldown.set_cooldown(provider_type, 10);
                return Err(VideoProviderError::RateLimited);
            }
            Err(e) => return Err(e),
        };

        if let Ok(json) = serde_json::to_value(&details) {
            let _ = self
                .cache_set_for(
                    provider_type,
                    &cache_key,
                    &json,
                    tv_show_detail_ttl(&details),
                )
                .await;
        }

        Ok(details)
    }

    /// Get season details from a specific provider.
    pub async fn get_season(
        &self,
        provider_type: &str,
        show_id: &str,
        season: u32,
    ) -> Result<SeasonDetails, VideoProviderError> {
        let stored = self.get_provider_config(provider_type).await?;
        let factory = self.factories.get(provider_type).ok_or_else(|| {
            VideoProviderError::Other(format!("Unknown provider: {provider_type}"))
        })?;

        let cache_key = format!("video_season:{}:{}:s{}", provider_type, show_id, season);
        if let Ok(Some(cached)) = self.cache_get(provider_type, &cache_key).await
            && let Ok(details) = serde_json::from_value::<SeasonDetails>(cached)
        {
            return Ok(details);
        }

        self.cooldown.wait_if_needed(provider_type).await;
        let provider = factory.create(self.http_client.clone(), &stored.config);
        let details = match provider.get_season(show_id, season).await {
            Ok(d) => {
                self.cooldown.reset(provider_type);
                d
            }
            Err(VideoProviderError::RateLimited) => {
                self.cooldown.set_cooldown(provider_type, 10);
                return Err(VideoProviderError::RateLimited);
            }
            Err(e) => return Err(e),
        };

        if let Ok(json) = serde_json::to_value(&details) {
            let _ = self
                .cache_set_for(provider_type, &cache_key, &json, season_ttl(&details))
                .await;
        }

        Ok(details)
    }

    /// Get season details, supplemented with any air *times* a secondary
    /// provider can add.
    ///
    /// TMDB has no episode air time at all, so a show enriched from TMDB would
    /// never learn one even though TVMaze publishes it. When the entity carries
    /// a TVMaze id and TVMaze is enabled, its season is fetched too and merged
    /// in behind the primary — never over it, and never inventing a time.
    pub async fn get_season_merged(
        &self,
        provider_type: &str,
        show_id: &str,
        season: u32,
        identifiers: &serde_json::Value,
    ) -> Result<SeasonDetails, VideoProviderError> {
        let primary = self.get_season(provider_type, show_id, season).await?;

        if provider_type == tvmaze::INFO.id {
            return Ok(primary);
        }
        let Some(tvmaze_id) = identifiers
            .get("tvmaze_id")
            .and_then(|v| v.as_str())
            .filter(|v| !v.is_empty())
        else {
            return Ok(primary);
        };
        if !self
            .get_enabled_providers()
            .await
            .iter()
            .any(|p| p.provider_type == tvmaze::INFO.id)
        {
            return Ok(primary);
        }

        match self.get_season(tvmaze::INFO.id, tvmaze_id, season).await {
            Ok(secondary) => Ok(merge::merge_season_details(&[primary, secondary])),
            Err(e) => {
                tracing::debug!("TVMaze air-time supplement failed for {tvmaze_id}: {e}");
                Ok(primary)
            }
        }
    }

    /// Get the credits for one episode from a specific provider.
    pub async fn get_episode_credits(
        &self,
        provider_type: &str,
        show_id: &str,
        season: u32,
        episode: u32,
    ) -> Result<EpisodeCredits, VideoProviderError> {
        let stored = self.get_provider_config(provider_type).await?;
        let factory = self.factories.get(provider_type).ok_or_else(|| {
            VideoProviderError::Other(format!("Unknown provider: {provider_type}"))
        })?;

        let cache_key =
            format!("video_episode_credits:{provider_type}:{show_id}:s{season}e{episode}");
        if let Ok(Some(cached)) = self.cache_get(provider_type, &cache_key).await
            && let Ok(credits) = serde_json::from_value::<EpisodeCredits>(cached)
        {
            return Ok(credits);
        }

        self.cooldown.wait_if_needed(provider_type).await;
        let provider = factory.create(self.http_client.clone(), &stored.config);
        let credits = match provider.get_episode_credits(show_id, season, episode).await {
            Ok(c) => {
                self.cooldown.reset(provider_type);
                c
            }
            Err(VideoProviderError::RateLimited) => {
                self.cooldown.set_cooldown(provider_type, 10);
                return Err(VideoProviderError::RateLimited);
            }
            Err(e) => return Err(e),
        };

        if let Ok(json) = serde_json::to_value(&credits) {
            let _ = self
                .cache_set_for(provider_type, &cache_key, &json, CACHE_TTL_STABLE)
                .await;
        }

        Ok(credits)
    }

    /// Get person details from a specific provider.
    pub async fn get_person_details(
        &self,
        provider_type: &str,
        provider_id: &str,
    ) -> Result<PersonProviderDetails, VideoProviderError> {
        let stored = self.get_provider_config(provider_type).await?;
        let factory = self.factories.get(provider_type).ok_or_else(|| {
            VideoProviderError::Other(format!("Unknown provider: {provider_type}"))
        })?;

        let cache_key = format!("video_person_detail:{}:{}", provider_type, provider_id);
        if let Ok(Some(cached)) = self.cache_get(provider_type, &cache_key).await
            && let Ok(details) = serde_json::from_value::<PersonProviderDetails>(cached)
        {
            return Ok(details);
        }

        self.cooldown.wait_if_needed(provider_type).await;
        let provider = factory.create(self.http_client.clone(), &stored.config);
        let details = match provider.get_person(provider_id).await {
            Ok(d) => {
                self.cooldown.reset(provider_type);
                d
            }
            Err(VideoProviderError::RateLimited) => {
                self.cooldown.set_cooldown(provider_type, 10);
                return Err(VideoProviderError::RateLimited);
            }
            Err(e) => return Err(e),
        };

        if let Ok(json) = serde_json::to_value(&details) {
            let _ = self.cache_set(provider_type, &cache_key, &json).await;
        }

        Ok(details)
    }

    /// Get person combined credits from a specific provider.
    pub async fn get_person_credits(
        &self,
        provider_type: &str,
        provider_id: &str,
    ) -> Result<PersonCombinedCredits, VideoProviderError> {
        let stored = self.get_provider_config(provider_type).await?;
        let factory = self.factories.get(provider_type).ok_or_else(|| {
            VideoProviderError::Other(format!("Unknown provider: {provider_type}"))
        })?;

        let cache_key = format!("video_person_credits:{}:{}", provider_type, provider_id);
        if let Ok(Some(cached)) = self.cache_get(provider_type, &cache_key).await
            && let Ok(credits) = serde_json::from_value::<PersonCombinedCredits>(cached)
        {
            return Ok(credits);
        }

        self.cooldown.wait_if_needed(provider_type).await;
        let provider = factory.create(self.http_client.clone(), &stored.config);
        let credits = match provider.get_person_credits(provider_id).await {
            Ok(c) => {
                self.cooldown.reset(provider_type);
                c
            }
            Err(VideoProviderError::RateLimited) => {
                self.cooldown.set_cooldown(provider_type, 10);
                return Err(VideoProviderError::RateLimited);
            }
            Err(e) => return Err(e),
        };

        if let Ok(json) = serde_json::to_value(&credits) {
            let _ = self.cache_set(provider_type, &cache_key, &json).await;
        }

        Ok(credits)
    }

    /// Invalidate cached TV show detail and season entries for a specific provider/show.
    pub async fn invalidate_tv_show_cache(&self, provider_type: &str, provider_id: &str) {
        use crate::repositories::metadata_cache;
        let detail_key = format!("video_tv_detail:{provider_type}:{provider_id}");
        let season_prefix = format!("video_season:{provider_type}:{provider_id}:");
        let _ = metadata_cache::delete_by_prefix(&self.pool, provider_type, &detail_key).await;
        let _ = metadata_cache::delete_by_prefix(&self.pool, provider_type, &season_prefix).await;
    }

    /// Invalidate only the cached TV show detail (season list), preserving per-season event caches.
    /// Use this during enrichment to re-fetch the season list without re-fetching all events.
    pub async fn invalidate_tv_show_detail_cache(&self, provider_type: &str, provider_id: &str) {
        use crate::repositories::metadata_cache;
        let detail_key = format!("video_tv_detail:{provider_type}:{provider_id}");
        let _ = metadata_cache::delete_by_prefix(&self.pool, provider_type, &detail_key).await;
    }

    /// Invalidate cached movie detail for a specific provider/movie.
    pub async fn invalidate_movie_cache(&self, provider_type: &str, provider_id: &str) {
        use crate::repositories::metadata_cache;
        let key = format!("video_movie_detail:{provider_type}:{provider_id}");
        let _ = metadata_cache::delete_by_prefix(&self.pool, provider_type, &key).await;
    }

    /// Invalidate cached person detail and credits for a specific provider/person.
    pub async fn invalidate_person_cache(&self, provider_type: &str, provider_id: &str) {
        use crate::repositories::metadata_cache;
        let detail_key = format!("video_person_detail:{provider_type}:{provider_id}");
        let credits_key = format!("video_person_credits:{provider_type}:{provider_id}");
        let _ = metadata_cache::delete_by_prefix(&self.pool, provider_type, &detail_key).await;
        let _ = metadata_cache::delete_by_prefix(&self.pool, provider_type, &credits_key).await;
    }

    /// Look up TMDB IDs from an IMDB ID using the TMDB `/find` endpoint.
    ///
    /// Returns `(movie_tmdb_ids, tv_tmdb_ids)`.
    pub async fn find_by_imdb_id(
        &self,
        imdb_id: &str,
    ) -> Result<(Vec<String>, Vec<String>), VideoProviderError> {
        let stored = self.get_provider_config("tmdb").await?;
        let tmdb = tmdb::TmdbProvider::new(self.http_client.clone(), &stored.config);
        tmdb.find_by_external_id(imdb_id, "imdb_id").await
    }

    /// Fetch alternate titles for a movie from TMDB.
    pub async fn get_movie_alternate_titles(
        &self,
        tmdb_id: &str,
        primary_title: &str,
    ) -> Result<Vec<String>, VideoProviderError> {
        let stored = self.get_provider_config("tmdb").await?;
        let tmdb = tmdb::TmdbProvider::new(self.http_client.clone(), &stored.config);
        tmdb.get_movie_alternate_titles(tmdb_id, primary_title)
            .await
    }

    /// Fetch alternate titles for a TV show from TMDB.
    pub async fn get_tv_alternate_titles(
        &self,
        tmdb_id: &str,
        primary_title: &str,
    ) -> Result<Vec<String>, VideoProviderError> {
        let stored = self.get_provider_config("tmdb").await?;
        let tmdb = tmdb::TmdbProvider::new(self.http_client.clone(), &stored.config);
        tmdb.get_tv_alternate_titles(tmdb_id, primary_title).await
    }

    /// Fetch an image from a specific provider.
    pub async fn fetch_image(
        &self,
        provider_type: &str,
        url: &str,
    ) -> Result<Vec<u8>, VideoProviderError> {
        let factory = self.factories.get(provider_type).ok_or_else(|| {
            VideoProviderError::Other(format!("Unknown provider: {provider_type}"))
        })?;
        let provider = factory.create(self.http_client.clone(), &serde_json::json!({}));
        provider.fetch_image(url).await
    }

    /// Get all providers (for admin UI).
    pub async fn get_all_providers(&self) -> Vec<StoredVideoProvider> {
        use crate::repositories::video_metadata_providers;
        match video_metadata_providers::list_all(&self.pool).await {
            Ok(providers) => providers
                .into_iter()
                .map(|p| StoredVideoProvider {
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
                tracing::warn!("Failed to get all video providers: {}", e);
                Vec::new()
            }
        }
    }

    /// Update provider configuration.
    pub async fn update_provider(
        &self,
        provider_id: uuid::Uuid,
        update: &UpdateVideoProviderRequest,
    ) -> Result<(), sqlx::Error> {
        use crate::repositories::video_metadata_providers::{self, ProviderUpdate};

        let repo_update = ProviderUpdate {
            enabled: update.enabled,
            priority: update.priority,
            config: update
                .config
                .as_ref()
                .map(|m| m.iter().map(|(k, v)| (k.clone(), v.clone())).collect()),
        };

        video_metadata_providers::update_with_merge(&self.pool, provider_id, &repo_update)
            .await
            .map_err(|e| match e {
                crate::repositories::RepositoryError::Database(e) => e,
                crate::repositories::RepositoryError::Validation(msg) => sqlx::Error::Protocol(msg),
            })?;

        Ok(())
    }

    // ========================================================================
    // Provider Resolution Helpers
    // ========================================================================

    /// Find the best enabled provider for an entity based on its stored identifiers.
    ///
    /// Given an entity's `identifiers` JSONB (e.g. `{"tmdb_id": "12345", "tvdb_id": "67890"}`),
    /// returns the first enabled provider (by priority) that has a matching `{provider_type}_id`
    /// key in identifiers and supports the required capability.
    ///
    /// Returns `(provider_type, provider_id_value)`.
    pub async fn resolve_provider_for_identifiers(
        &self,
        identifiers: &serde_json::Value,
        capability: types::VideoCapability,
    ) -> Option<(String, String)> {
        let obj = identifiers.as_object()?;
        let enabled = self.get_enabled_providers().await;

        for stored in enabled {
            let Some(factory) = self.factories.get(stored.provider_type.as_str()) else {
                continue;
            };
            if !factory.info().capabilities.has(capability) {
                continue;
            }
            let key = format!("{}_id", stored.provider_type);
            if let Some(val) = obj.get(&key).and_then(|v| v.as_str())
                && !val.is_empty()
            {
                return Some((stored.provider_type, val.to_string()));
            }
        }
        None
    }

    /// Get the highest-priority enabled provider that has the given capability.
    ///
    /// Used for search/matching where no entity exists yet (so there are no
    /// stored identifiers to resolve against).
    pub async fn preferred_provider(&self, capability: types::VideoCapability) -> Option<String> {
        let enabled = self.get_enabled_providers().await;
        for stored in enabled {
            let Some(factory) = self.factories.get(stored.provider_type.as_str()) else {
                continue;
            };
            if factory.info().capabilities.has(capability) {
                return Some(stored.provider_type);
            }
        }
        None
    }

    // ========================================================================
    // Internal helpers
    // ========================================================================

    async fn get_enabled_providers(&self) -> Vec<StoredVideoProvider> {
        use crate::repositories::video_metadata_providers;
        match video_metadata_providers::list_enabled(&self.pool).await {
            Ok(providers) => providers
                .into_iter()
                .map(|p| StoredVideoProvider {
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
                tracing::warn!("Failed to get enabled video providers: {}", e);
                Vec::new()
            }
        }
    }

    async fn get_provider_config(
        &self,
        provider_type: &str,
    ) -> Result<StoredVideoProvider, VideoProviderError> {
        use crate::repositories::video_metadata_providers;
        let providers = video_metadata_providers::list_enabled(&self.pool)
            .await
            .map_err(|e| VideoProviderError::Other(format!("DB error: {e}")))?;

        providers
            .into_iter()
            .find(|p| p.provider_type == provider_type)
            .map(|p| StoredVideoProvider {
                id: p.id,
                provider_type: p.provider_type,
                enabled: p.enabled,
                priority: p.priority,
                config: p.config,
                created_at: p.created_at,
                updated_at: p.updated_at,
            })
            .ok_or_else(|| {
                VideoProviderError::Other(format!("Provider {provider_type} not enabled"))
            })
    }

    async fn cache_get(
        &self,
        provider_type: &str,
        cache_key: &str,
    ) -> Result<Option<serde_json::Value>, ()> {
        use crate::repositories::metadata_cache as cache_repo;
        cache_repo::get(&self.pool, provider_type, cache_key)
            .await
            .map_err(|_| ())
    }

    async fn cache_set(
        &self,
        provider_type: &str,
        cache_key: &str,
        value: &serde_json::Value,
    ) -> Result<(), ()> {
        self.cache_set_for(provider_type, cache_key, value, CACHE_TTL_STABLE)
            .await
    }

    async fn cache_set_for(
        &self,
        provider_type: &str,
        cache_key: &str,
        value: &serde_json::Value,
        ttl: time::Duration,
    ) -> Result<(), ()> {
        use crate::repositories::metadata_cache as cache_repo;
        let expires_at = time::OffsetDateTime::now_utc() + ttl;
        cache_repo::set(&self.pool, provider_type, cache_key, value, expires_at)
            .await
            .map_err(|_| ())
    }
}

#[cfg(test)]
mod cache_ttl_tests {
    use super::*;

    fn show(status: Option<&str>) -> TvShowDetails {
        TvShowDetails {
            provider: "tmdb".to_string(),
            provider_id: "12271".to_string(),
            title: "The Block".to_string(),
            year: Some(2003),
            status: status.map(str::to_string),
            description: None,
            poster_url: None,
            backdrop_url: None,
            logo_url: None,
            identifiers: Default::default(),
            genres: Vec::new(),
            rating: None,
            rating_count: None,
            network: None,
            content_rating: None,
            first_aired: None,
            runtime: None,
            seasons: Vec::new(),
            cast: Vec::new(),
            crew: Vec::new(),
            images: Vec::new(),
        }
    }

    fn season(air_dates: &[Option<&str>]) -> SeasonDetails {
        SeasonDetails {
            provider: "tmdb".to_string(),
            show_id: "12271".to_string(),
            season_number: 22,
            title: None,
            air_date: None,
            description: None,
            poster_url: None,
            episodes: air_dates
                .iter()
                .enumerate()
                .map(|(i, air_date)| EpisodeDetails {
                    episode_number: i as u32 + 1,
                    title: None,
                    air_date: air_date.map(str::to_string),
                    air_time: None,
                    runtime: None,
                    still_url: None,
                    description: None,
                    provider_id: None,
                    country: None,
                    credits: None,
                })
                .collect(),
        }
    }

    fn days_ago(days: i64) -> String {
        (time::OffsetDateTime::now_utc().date() - time::Duration::days(days)).to_string()
    }

    #[test]
    fn continuing_show_detail_refreshes_daily() {
        assert_eq!(
            tv_show_detail_ttl(&show(Some("continuing"))),
            CACHE_TTL_ACTIVE
        );
        assert_eq!(
            tv_show_detail_ttl(&show(Some("upcoming"))),
            CACHE_TTL_ACTIVE
        );
    }

    #[test]
    fn finished_show_detail_keeps_the_long_ttl() {
        assert_eq!(tv_show_detail_ttl(&show(Some("ended"))), CACHE_TTL_STABLE);
        assert_eq!(
            tv_show_detail_ttl(&show(Some("canceled"))),
            CACHE_TTL_STABLE
        );
    }

    /// The prod bug: tv_monitor runs hourly to find new episodes but every call
    /// was a cache hit on week-old season data, so episodes surfaced up to 7
    /// days late. A season that is still airing has to refresh daily.
    #[test]
    fn currently_airing_season_refreshes_daily() {
        let details = season(&[Some(&days_ago(9)), Some(&days_ago(2))]);
        assert_eq!(season_ttl(&details), CACHE_TTL_ACTIVE);
    }

    #[test]
    fn season_airing_in_the_future_refreshes_daily() {
        let future =
            (time::OffsetDateTime::now_utc().date() + time::Duration::days(14)).to_string();
        assert_eq!(season_ttl(&season(&[Some(&future)])), CACHE_TTL_ACTIVE);
    }

    #[test]
    fn long_finished_season_keeps_the_long_ttl() {
        let details = season(&[Some(&days_ago(400)), Some(&days_ago(365))]);
        assert_eq!(season_ttl(&details), CACHE_TTL_STABLE);
    }

    /// An empty or undated season is a placeholder the provider has yet to fill
    /// in — exactly the case where waiting a week hurts.
    #[test]
    fn undatable_seasons_refresh_daily() {
        assert_eq!(season_ttl(&season(&[])), CACHE_TTL_ACTIVE);
        assert_eq!(season_ttl(&season(&[None, None])), CACHE_TTL_ACTIVE);
    }

    /// Back-catalogue seasons of a still-running show must not all refresh
    /// daily — The Block has 23 of them and only one is airing.
    #[test]
    fn back_catalogue_season_of_continuing_show_keeps_the_long_ttl() {
        let details = season(&[Some("2019-08-04"), Some("2019-11-17")]);
        assert_eq!(season_ttl(&details), CACHE_TTL_STABLE);
    }
}
