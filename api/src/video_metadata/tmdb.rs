use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use super::types::{
    CastMember, CrewMember, EpisodeCredits, EpisodeDetails, MediaImage, MovieResult,
    PersonCombinedCredits, PersonCreditCast, PersonCreditCrew, PersonProviderDetails,
    SeasonSummary, TvShowResult, VideoCapability, VideoCapabilitySet, VideoConfigField,
    VideoConfigFieldType, VideoIdentifiers, VideoProviderInfo,
};
use super::{
    MovieDetails, MovieQuery, SeasonDetails, TvQuery, TvShowDetails, VideoMetadataProvider,
    VideoProviderError, VideoProviderFactory,
};

// ============================================================================
// Provider Info
// ============================================================================

static CONFIG_FIELDS: &[VideoConfigField] = &[
    VideoConfigField {
        key: "api_key",
        label: "API Key (Bearer Token)",
        field_type: VideoConfigFieldType::Secret,
        required: true,
        description: Some("TMDB API Read Access Token (v4 auth)"),
        help_url: Some("https://www.themoviedb.org/settings/api"),
        default: None,
    },
    VideoConfigField {
        key: "base_url",
        label: "API Base URL",
        field_type: VideoConfigFieldType::Url,
        required: false,
        description: Some("Override the TMDB API base URL (for testing)"),
        help_url: None,
        default: Some("https://api.themoviedb.org/3"),
    },
];

static CAPABILITIES: &[VideoCapability] = &[
    VideoCapability::MovieSearch,
    VideoCapability::TvSearch,
    VideoCapability::MovieDetails,
    VideoCapability::TvDetails,
    VideoCapability::SeasonDetails,
    VideoCapability::ImageFetch,
    VideoCapability::PersonDetails,
    VideoCapability::PersonCombinedCredits,
    VideoCapability::EpisodeCredits,
];

pub static INFO: VideoProviderInfo = VideoProviderInfo {
    id: crate::http::provider_ids::TMDB,
    display_name: "TMDB",
    description: "The Movie Database — comprehensive movie and TV metadata with images and credits.",
    website: "https://www.themoviedb.org",
    capabilities: VideoCapabilitySet::new(CAPABILITIES),
    config_fields: CONFIG_FIELDS,
    requests_per_minute: 240, // TMDB allows ~50/sec; 4/sec is conservative and polite
};

const BASE_URL: &str = "https://api.themoviedb.org/3";
const IMAGE_BASE: &str = "https://image.tmdb.org/t/p/";

// ============================================================================
// Factory
// ============================================================================

pub struct TmdbFactory;

impl VideoProviderFactory for TmdbFactory {
    fn info(&self) -> &'static VideoProviderInfo {
        &INFO
    }

    fn create(
        &self,
        client: reqwest::Client,
        config: &serde_json::Value,
    ) -> Box<dyn VideoMetadataProvider> {
        let api_key = config
            .get("api_key")
            .and_then(|v| v.as_str())
            .map(String::from)
            .unwrap_or_default();

        let base_url = config
            .get("base_url")
            .and_then(|v| v.as_str())
            .map(|s| s.trim_end_matches('/').to_string())
            .unwrap_or_else(|| BASE_URL.to_string());

        Box::new(TmdbProvider {
            client,
            api_key,
            base_url,
        })
    }
}

// ============================================================================
// Provider
// ============================================================================

pub struct TmdbProvider {
    client: reqwest::Client,
    api_key: String,
    base_url: String,
}

impl TmdbProvider {
    /// Create a provider directly from a config JSON value.
    pub fn from_config(client: reqwest::Client, config: &serde_json::Value) -> Self {
        let api_key = config
            .get("api_key")
            .and_then(|v| v.as_str())
            .map(String::from)
            .unwrap_or_default();
        let base_url = config
            .get("base_url")
            .and_then(|v| v.as_str())
            .map(|s| s.trim_end_matches('/').to_string())
            .unwrap_or_else(|| BASE_URL.to_string());
        Self {
            client,
            api_key,
            base_url,
        }
    }
}

impl TmdbProvider {
    pub fn new(client: reqwest::Client, config: &serde_json::Value) -> Self {
        let api_key = config
            .get("api_key")
            .and_then(|v| v.as_str())
            .map(String::from)
            .unwrap_or_default();
        let base_url = config
            .get("base_url")
            .and_then(|v| v.as_str())
            .map(|s| s.trim_end_matches('/').to_string())
            .unwrap_or_else(|| BASE_URL.to_string());
        Self {
            client,
            api_key,
            base_url,
        }
    }

    fn require_key(&self) -> Result<&str, VideoProviderError> {
        if self.api_key.is_empty() {
            Err(VideoProviderError::Other(
                "TMDB API key not configured".into(),
            ))
        } else {
            Ok(&self.api_key)
        }
    }

    /// Build a GET request with auth.
    fn get(&self, url: &str, key: &str) -> reqwest::RequestBuilder {
        self.client.get(url).bearer_auth(key)
    }

    /// Find a movie or TV show by an external ID (e.g., IMDB ID).
    ///
    /// Returns `(movie_tmdb_ids, tv_tmdb_ids)`.
    pub async fn find_by_external_id(
        &self,
        external_id: &str,
        source: &str,
    ) -> Result<(Vec<String>, Vec<String>), VideoProviderError> {
        let key = self.require_key()?;
        let base_url = &self.base_url;
        let url = format!("{base_url}/find/{external_id}?external_source={source}&language=en-US");
        let resp: TmdbFindResponse = self
            .get(&url, key)
            .send()
            .await?
            .error_for_status()
            .map_err(VideoProviderError::Http)?
            .json()
            .await?;

        let movie_ids = resp
            .movie_results
            .iter()
            .map(|m| m.id.to_string())
            .collect();
        let tv_ids = resp.tv_results.iter().map(|t| t.id.to_string()).collect();

        Ok((movie_ids, tv_ids))
    }

    fn poster_url(path: Option<&str>) -> Option<String> {
        path.filter(|p| !p.is_empty())
            .map(|p| format!("{IMAGE_BASE}w500{p}"))
    }

    fn backdrop_url(path: Option<&str>) -> Option<String> {
        path.filter(|p| !p.is_empty())
            .map(|p| format!("{IMAGE_BASE}w1280{p}"))
    }

    fn profile_url(path: Option<&str>) -> Option<String> {
        path.filter(|p| !p.is_empty())
            .map(|p| format!("{IMAGE_BASE}w185{p}"))
    }

    fn still_url(path: Option<&str>) -> Option<String> {
        path.filter(|p| !p.is_empty())
            .map(|p| format!("{IMAGE_BASE}w300{p}"))
    }

    fn map_episode_credits(guest_stars: &[TmdbCast], crew: &[TmdbCrew]) -> EpisodeCredits {
        EpisodeCredits {
            guest_stars: guest_stars
                .iter()
                .take(30)
                .map(|a| CastMember {
                    name: a.name.clone().unwrap_or_default(),
                    character: a.character.clone(),
                    profile_url: Self::profile_url(a.profile_path.as_deref()),
                    order: a.order,
                    external_id: Some(a.id.to_string()),
                })
                .collect(),
            crew: crew
                .iter()
                .filter(|c| {
                    matches!(
                        c.job.as_deref(),
                        Some("Director") | Some("Writer") | Some("Screenplay") | Some("Teleplay")
                    )
                })
                .map(|c| CrewMember {
                    name: c.name.clone().unwrap_or_default(),
                    department: c.department.clone().unwrap_or_default(),
                    job: c.job.clone().unwrap_or_default(),
                    profile_url: Self::profile_url(c.profile_path.as_deref()),
                    external_id: Some(c.id.to_string()),
                })
                .collect(),
        }
    }

    // ========================================================================
    // Discover: trending / popular (not on the trait — TMDB-specific)
    // ========================================================================

    /// Fetch trending movies for the past week.
    pub async fn trending_movies(&self) -> Result<Vec<MovieResult>, VideoProviderError> {
        let key = self.require_key()?;
        let url = format!("{}/trending/movie/week?language=en-US", self.base_url);
        let resp: TmdbSearchResponse = self
            .get(&url, key)
            .send()
            .await?
            .error_for_status()
            .map_err(VideoProviderError::Http)?
            .json()
            .await?;
        Ok(Self::movies_from_search(resp))
    }

    /// Fetch trending TV shows for the past week.
    pub async fn trending_tv(&self) -> Result<Vec<TvShowResult>, VideoProviderError> {
        let key = self.require_key()?;
        let url = format!("{}/trending/tv/week?language=en-US", self.base_url);
        let resp: TmdbTvSearchResponse = self
            .get(&url, key)
            .send()
            .await?
            .error_for_status()
            .map_err(VideoProviderError::Http)?
            .json()
            .await?;
        Ok(Self::tv_from_search(resp))
    }

    /// Fetch popular movies.
    pub async fn popular_movies(&self) -> Result<Vec<MovieResult>, VideoProviderError> {
        let key = self.require_key()?;
        let url = format!("{}/movie/popular?language=en-US", self.base_url);
        let resp: TmdbSearchResponse = self
            .get(&url, key)
            .send()
            .await?
            .error_for_status()
            .map_err(VideoProviderError::Http)?
            .json()
            .await?;
        Ok(Self::movies_from_search(resp))
    }

    /// Fetch popular TV shows.
    pub async fn popular_tv(&self) -> Result<Vec<TvShowResult>, VideoProviderError> {
        let key = self.require_key()?;
        let url = format!("{}/tv/popular?language=en-US", self.base_url);
        let resp: TmdbTvSearchResponse = self
            .get(&url, key)
            .send()
            .await?
            .error_for_status()
            .map_err(VideoProviderError::Http)?
            .json()
            .await?;
        Ok(Self::tv_from_search(resp))
    }

    // ========================================================================
    // Discover by streaming service (not on the trait — TMDB-specific)
    // ========================================================================

    /// Discover popular movies available on a specific streaming service.
    pub async fn discover_movies_by_provider(
        &self,
        provider_id: i32,
        region: &str,
    ) -> Result<Vec<MovieResult>, VideoProviderError> {
        let key = self.require_key()?;
        let url = format!(
            "{}/discover/movie?with_watch_providers={}&watch_region={}&sort_by=popularity.desc&with_watch_monetization_types=flatrate&language=en-US",
            self.base_url, provider_id, region,
        );
        let resp: TmdbSearchResponse = self
            .get(&url, key)
            .send()
            .await?
            .error_for_status()
            .map_err(VideoProviderError::Http)?
            .json()
            .await?;
        Ok(Self::movies_from_search(resp))
    }

    /// Discover popular TV shows available on a specific streaming service.
    pub async fn discover_tv_by_provider(
        &self,
        provider_id: i32,
        region: &str,
    ) -> Result<Vec<TvShowResult>, VideoProviderError> {
        let key = self.require_key()?;
        let url = format!(
            "{}/discover/tv?with_watch_providers={}&watch_region={}&sort_by=popularity.desc&with_watch_monetization_types=flatrate&language=en-US",
            self.base_url, provider_id, region,
        );
        let resp: TmdbTvSearchResponse = self
            .get(&url, key)
            .send()
            .await?
            .error_for_status()
            .map_err(VideoProviderError::Http)?
            .json()
            .await?;
        Ok(Self::tv_from_search(resp))
    }

    /// List available streaming/watch providers for a region.
    pub async fn watch_providers_for_region(
        &self,
        region: &str,
    ) -> Result<Vec<WatchProviderInfo>, VideoProviderError> {
        let key = self.require_key()?;
        let url = format!(
            "{}/watch/providers/movie?watch_region={}&language=en-US",
            self.base_url, region,
        );
        let resp: TmdbWatchProviderListResponse = self
            .get(&url, key)
            .send()
            .await?
            .error_for_status()
            .map_err(VideoProviderError::Http)?
            .json()
            .await?;
        Ok(resp
            .results
            .into_iter()
            .map(|p| WatchProviderInfo {
                provider_id: p.provider_id,
                provider_name: p.provider_name,
                logo_url: p.logo_path.map(|path| format!("{IMAGE_BASE}w92{path}")),
            })
            .collect())
    }

    // ========================================================================
    // Alternate titles (not on the trait — TMDB-specific)
    // ========================================================================

    /// Fetch alternate/alternative titles for a movie from TMDB.
    /// Filters to English-relevant countries (US, GB, empty) and deduplicates.
    pub async fn get_movie_alternate_titles(
        &self,
        tmdb_id: &str,
        primary_title: &str,
    ) -> Result<Vec<String>, VideoProviderError> {
        let key = self.require_key()?;
        let url = format!("{}/movie/{}/alternative_titles", self.base_url, tmdb_id);
        let resp: TmdbMovieAlternativeTitles = self
            .get(&url, key)
            .send()
            .await?
            .error_for_status()
            .map_err(VideoProviderError::Http)?
            .json()
            .await?;
        Ok(filter_alternate_titles(resp.titles, primary_title))
    }

    /// Fetch alternate/alternative titles for a TV show from TMDB.
    /// Filters to English-relevant countries (US, GB, empty) and deduplicates.
    pub async fn get_tv_alternate_titles(
        &self,
        tmdb_id: &str,
        primary_title: &str,
    ) -> Result<Vec<String>, VideoProviderError> {
        let key = self.require_key()?;
        let url = format!("{}/tv/{}/alternative_titles", self.base_url, tmdb_id);
        let resp: TmdbTvAlternativeTitles = self
            .get(&url, key)
            .send()
            .await?
            .error_for_status()
            .map_err(VideoProviderError::Http)?
            .json()
            .await?;
        Ok(filter_alternate_titles(resp.results, primary_title))
    }

    /// Normalize a title or query for relevance comparison:
    /// lowercase, `&` → "and", collapse to a single alphanumeric-space stream.
    /// Used only for scoring — not displayed to the user.
    fn normalize_for_match(s: &str) -> String {
        let lowered = s.to_lowercase().replace('&', " and ");
        lowered
            .chars()
            .map(|c| if c.is_alphanumeric() { c } else { ' ' })
            .collect::<String>()
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ")
    }

    /// Split a trailing 4-digit year from a search query. Returns the stripped
    /// title and the parsed year, if any. Only fires when the year is reasonable
    /// (1888–2100) so something like "iPhone 1984" still works as a fallback.
    fn extract_year(query: &str) -> (String, Option<u16>) {
        // Strip trailing punctuation that commonly wraps a year: `()`, `.`, `,`.
        // Handles "Rooster (2026)", "Rooster 2026.", "Rooster, 2026".
        let trimmed = query
            .trim()
            .trim_end_matches(|c: char| c == ')' || c == '.' || c == ',' || c.is_whitespace());
        if let Some((head, tail)) = trimmed.rsplit_once(|c: char| c.is_whitespace() || c == '(')
            && tail.len() == 4
            && tail.chars().all(|c| c.is_ascii_digit())
            && let Ok(year) = tail.parse::<u16>()
            && (1888..=2100).contains(&year)
        {
            let head = head
                .trim_end_matches(|c: char| c == ',' || c == '(' || c.is_whitespace())
                .trim();
            return (head.to_string(), Some(year));
        }
        (trimmed.to_string(), None)
    }

    /// Relevance tier for a result title against a normalized query:
    /// 0 = exact match, 1 = startsWith, 2 = contains, 3 = other.
    fn match_tier(title: &str, normalized_query: &str) -> u8 {
        let t = Self::normalize_for_match(title);
        if t == normalized_query {
            0
        } else if t.starts_with(normalized_query) {
            1
        } else if t.contains(normalized_query) {
            2
        } else {
            3
        }
    }

    /// Convert a TMDB movie search response into `MovieResult`s.
    fn movies_from_search(resp: TmdbSearchResponse) -> Vec<MovieResult> {
        resp.results
            .into_iter()
            .take(20)
            .map(|m| MovieResult {
                provider: "tmdb".into(),
                provider_id: m.id.to_string(),
                title: m.title.unwrap_or_default(),
                year: m
                    .release_date
                    .as_deref()
                    .and_then(|d| d.get(..4))
                    .and_then(|y| y.parse().ok()),
                description: m.overview,
                poster_url: Self::poster_url(m.poster_path.as_deref()),
                backdrop_url: Self::backdrop_url(m.backdrop_path.as_deref()),
                rating: m.vote_average.map(|r| r / 2.0),
                rating_count: m.vote_count,
                identifiers: VideoIdentifiers {
                    tmdb_id: Some(m.id.to_string()),
                    ..Default::default()
                },
                genres: vec![],
                network: None,
            })
            .collect()
    }

    /// Convert a TMDB movie search response, applying relevance ranking
    /// against a user-supplied query. Used for lexical searches where
    /// "user typed this exact title" should beat TMDB's generic relevance.
    fn movies_from_search_ranked(mut resp: TmdbSearchResponse, query: &str) -> Vec<MovieResult> {
        let nq = Self::normalize_for_match(query);
        if !nq.is_empty() {
            resp.results.sort_by(|a, b| {
                let at = Self::match_tier(a.title.as_deref().unwrap_or(""), &nq);
                let bt = Self::match_tier(b.title.as_deref().unwrap_or(""), &nq);
                at.cmp(&bt).then_with(|| {
                    b.popularity
                        .unwrap_or(0.0)
                        .partial_cmp(&a.popularity.unwrap_or(0.0))
                        .unwrap_or(std::cmp::Ordering::Equal)
                })
            });
        }
        Self::movies_from_search(resp)
    }

    /// Convert a TMDB TV search response, applying relevance ranking
    /// against a user-supplied query (see `movies_from_search_ranked`).
    fn tv_from_search_ranked(mut resp: TmdbTvSearchResponse, query: &str) -> Vec<TvShowResult> {
        let nq = Self::normalize_for_match(query);
        if !nq.is_empty() {
            resp.results.sort_by(|a, b| {
                let at = Self::match_tier(a.name.as_deref().unwrap_or(""), &nq);
                let bt = Self::match_tier(b.name.as_deref().unwrap_or(""), &nq);
                at.cmp(&bt).then_with(|| {
                    b.popularity
                        .unwrap_or(0.0)
                        .partial_cmp(&a.popularity.unwrap_or(0.0))
                        .unwrap_or(std::cmp::Ordering::Equal)
                })
            });
        }
        Self::tv_from_search(resp)
    }

    /// Convert a TMDB TV search response into `TvShowResult`s.
    fn tv_from_search(resp: TmdbTvSearchResponse) -> Vec<TvShowResult> {
        resp.results
            .into_iter()
            .take(20)
            .map(|s| TvShowResult {
                provider: "tmdb".into(),
                provider_id: s.id.to_string(),
                title: s.name.unwrap_or_default(),
                year: s
                    .first_air_date
                    .as_deref()
                    .and_then(|d| d.get(..4))
                    .and_then(|y| y.parse().ok()),
                status: None,
                description: s.overview,
                poster_url: Self::poster_url(s.poster_path.as_deref()),
                backdrop_url: Self::backdrop_url(s.backdrop_path.as_deref()),
                network: None,
                rating: s.vote_average.map(|r| r / 2.0),
                rating_count: s.vote_count,
                identifiers: VideoIdentifiers {
                    tmdb_id: Some(s.id.to_string()),
                    ..Default::default()
                },
                genres: vec![],
            })
            .collect()
    }
}

#[async_trait]
impl VideoMetadataProvider for TmdbProvider {
    fn info(&self) -> &'static VideoProviderInfo {
        &INFO
    }

    async fn search_movies(
        &self,
        query: &MovieQuery,
    ) -> Result<Vec<MovieResult>, VideoProviderError> {
        let key = self.require_key()?;
        let base_url = &self.base_url;

        // If the caller didn't supply a year, try to peel a trailing 4-digit
        // year off the query ("Rooster 2026" → title="Rooster", year=2026).
        let (effective_title, effective_year) = if query.year.is_some() {
            (query.title.clone(), query.year)
        } else {
            Self::extract_year(&query.title)
        };

        let encoded_title = urlencoding::encode(&effective_title);
        let title_url = format!("{base_url}/search/movie?query={encoded_title}&language=en-US");

        let mut url = title_url.clone();
        if let Some(year) = effective_year {
            url.push_str(&format!("&year={year}"));
        }

        let resp: TmdbSearchResponse = self
            .get(&url, key)
            .send()
            .await?
            .error_for_status()
            .map_err(VideoProviderError::Http)?
            .json()
            .await?;

        // TMDB's year filter is strict — if the parsed year is slightly off or
        // TMDB catalogues the movie under a different release year, we get 0
        // results. Fall back to title-only search with the stripped title.
        let mut resp = if resp.results.is_empty() && effective_year.is_some() {
            self.get(&title_url, key)
                .send()
                .await?
                .error_for_status()
                .map_err(VideoProviderError::Http)?
                .json::<TmdbSearchResponse>()
                .await?
        } else {
            resp
        };

        // Final fallback: if stripping the year produced no results and the
        // year came from the query (not the caller), retry with the full
        // original query — handles titles like "Iron Sky 2019" where the
        // digits belong to the title itself.
        if resp.results.is_empty() && effective_year.is_some() && query.year.is_none() {
            let encoded_full = urlencoding::encode(&query.title);
            let full_url = format!("{base_url}/search/movie?query={encoded_full}&language=en-US");
            resp = self
                .get(&full_url, key)
                .send()
                .await?
                .error_for_status()
                .map_err(VideoProviderError::Http)?
                .json::<TmdbSearchResponse>()
                .await?;
        }

        Ok(Self::movies_from_search_ranked(resp, &query.title))
    }

    async fn search_tv(&self, query: &TvQuery) -> Result<Vec<TvShowResult>, VideoProviderError> {
        let key = self.require_key()?;
        let base_url = &self.base_url;

        // Same rule as `search_movies`: the caller's year wins, and only when it
        // has none do we go looking for one on the end of the title.
        let (effective_title, effective_year) = if query.year.is_some() {
            (query.title.clone(), query.year)
        } else {
            let (title, year) = Self::extract_year(&query.title);
            (title, year.map(i32::from))
        };
        let encoded_title = urlencoding::encode(&effective_title);
        let title_url = format!("{base_url}/search/tv?query={encoded_title}&language=en-US");

        let mut url = title_url.clone();
        if let Some(year) = effective_year {
            url.push_str(&format!("&first_air_date_year={year}"));
        }

        let resp: TmdbTvSearchResponse = self
            .get(&url, key)
            .send()
            .await?
            .error_for_status()
            .map_err(VideoProviderError::Http)?
            .json()
            .await?;

        // Fallback 1: year-filtered came back empty → drop year filter.
        let mut resp = if resp.results.is_empty() && effective_year.is_some() {
            self.get(&title_url, key)
                .send()
                .await?
                .error_for_status()
                .map_err(VideoProviderError::Http)?
                .json::<TmdbTvSearchResponse>()
                .await?
        } else {
            resp
        };

        // Fallback 2: stripped title still empty → retry with the full
        // original query (the "year" was part of the title). Only when the year
        // came off the title; a caller-supplied year never went into it.
        if resp.results.is_empty() && effective_year.is_some() && query.year.is_none() {
            let encoded_full = urlencoding::encode(&query.title);
            let full_url = format!("{base_url}/search/tv?query={encoded_full}&language=en-US");
            resp = self
                .get(&full_url, key)
                .send()
                .await?
                .error_for_status()
                .map_err(VideoProviderError::Http)?
                .json::<TmdbTvSearchResponse>()
                .await?;
        }

        Ok(Self::tv_from_search_ranked(resp, &query.title))
    }

    async fn get_movie(&self, provider_id: &str) -> Result<MovieDetails, VideoProviderError> {
        let key = self.require_key()?;
        let base_url = &self.base_url;

        let url = format!(
            "{base_url}/movie/{provider_id}?append_to_response=credits,images,release_dates&language=en-US&include_image_language=en,null"
        );

        let m: TmdbMovieDetail = self
            .get(&url, key)
            .send()
            .await?
            .error_for_status()
            .map_err(VideoProviderError::Http)?
            .json()
            .await?;

        let content_rating = m.release_dates.as_ref().and_then(|rd| {
            rd.results
                .iter()
                .find(|r| r.iso_3166_1 == "US")
                .and_then(|r| {
                    r.release_dates
                        .iter()
                        .find_map(|d| d.certification.clone().filter(|c| !c.is_empty()))
                })
        });

        let cast = m
            .credits
            .as_ref()
            .map(|c| {
                c.cast
                    .iter()
                    .take(30)
                    .map(|a| CastMember {
                        name: a.name.clone().unwrap_or_default(),
                        character: a.character.clone(),
                        profile_url: Self::profile_url(a.profile_path.as_deref()),
                        order: a.order,
                        external_id: Some(a.id.to_string()),
                    })
                    .collect()
            })
            .unwrap_or_default();

        let crew = m
            .credits
            .as_ref()
            .map(|c| {
                c.crew
                    .iter()
                    .filter(|c| {
                        matches!(
                            c.job.as_deref(),
                            Some("Director")
                                | Some("Writer")
                                | Some("Screenplay")
                                | Some("Producer")
                        )
                    })
                    .map(|c| CrewMember {
                        name: c.name.clone().unwrap_or_default(),
                        department: c.department.clone().unwrap_or_default(),
                        job: c.job.clone().unwrap_or_default(),
                        profile_url: Self::profile_url(c.profile_path.as_deref()),
                        external_id: Some(c.id.to_string()),
                    })
                    .collect()
            })
            .unwrap_or_default();

        let images = m
            .images
            .as_ref()
            .map(|imgs| {
                let mut out = Vec::new();
                for p in filter_en_images(&imgs.posters).take(5) {
                    if let Some(path) = &p.file_path {
                        out.push(MediaImage {
                            url: format!("{IMAGE_BASE}w500{path}"),
                            image_type: "poster".into(),
                            width: p.width,
                            height: p.height,
                        });
                    }
                }
                for b in filter_en_images(&imgs.backdrops).take(5) {
                    if let Some(path) = &b.file_path {
                        out.push(MediaImage {
                            url: format!("{IMAGE_BASE}w1280{path}"),
                            image_type: "backdrop".into(),
                            width: b.width,
                            height: b.height,
                        });
                    }
                }
                for l in filter_en_images(&imgs.logos).take(3) {
                    if let Some(path) = &l.file_path {
                        out.push(MediaImage {
                            url: format!("{IMAGE_BASE}w500{path}"),
                            image_type: "logo".into(),
                            width: l.width,
                            height: l.height,
                        });
                    }
                }
                out
            })
            .unwrap_or_default();

        // Prefer English images from the images append over the main response paths,
        // which TMDB may return in the original language even with language=en-US.
        let en_poster_url = m
            .images
            .as_ref()
            .and_then(|imgs| filter_en_images(&imgs.posters).next())
            .and_then(|p| p.file_path.as_ref())
            .map(|path| format!("{IMAGE_BASE}w500{path}"))
            .or_else(|| Self::poster_url(m.poster_path.as_deref()));

        let en_backdrop_url = m
            .images
            .as_ref()
            .and_then(|imgs| filter_en_images(&imgs.backdrops).next())
            .and_then(|b| b.file_path.as_ref())
            .map(|path| format!("{IMAGE_BASE}w1280{path}"))
            .or_else(|| Self::backdrop_url(m.backdrop_path.as_deref()));

        let logo_url = m
            .images
            .as_ref()
            .and_then(|imgs| filter_en_images(&imgs.logos).next())
            .and_then(|l| l.file_path.as_ref())
            .map(|path| format!("{IMAGE_BASE}w500{path}"));

        Ok(MovieDetails {
            provider: "tmdb".into(),
            provider_id: m.id.to_string(),
            title: m.title.unwrap_or_default(),
            original_title: m.original_title,
            year: m
                .release_date
                .as_deref()
                .and_then(|d| d.get(..4))
                .and_then(|y| y.parse().ok()),
            runtime: m.runtime.map(|r| r as u32),
            description: m.overview,
            tagline: m.tagline.filter(|t| !t.is_empty()),
            poster_url: en_poster_url,
            backdrop_url: en_backdrop_url,
            logo_url,
            identifiers: VideoIdentifiers {
                tmdb_id: Some(m.id.to_string()),
                imdb_id: m.imdb_id.clone(),
                ..Default::default()
            },
            genres: m.genres.iter().map(|g| g.name.clone()).collect(),
            rating: m.vote_average.map(|r| r / 2.0),
            rating_count: m.vote_count,
            content_rating,
            release_date: m.release_date.clone(),
            cast,
            crew,
            images,
        })
    }

    async fn get_tv_show(&self, provider_id: &str) -> Result<TvShowDetails, VideoProviderError> {
        let key = self.require_key()?;
        let base_url = &self.base_url;

        let url = format!(
            "{base_url}/tv/{provider_id}?append_to_response=aggregate_credits,images,content_ratings,external_ids&language=en-US&include_image_language=en,null"
        );

        let s: TmdbTvDetail = self
            .get(&url, key)
            .send()
            .await?
            .error_for_status()
            .map_err(VideoProviderError::Http)?
            .json()
            .await?;

        let content_rating = s.content_ratings.as_ref().and_then(|cr| {
            cr.results
                .iter()
                .find(|r| r.iso_3166_1 == "US")
                .and_then(|r| r.rating.clone().filter(|c| !c.is_empty()))
        });

        let cast = s
            .aggregate_credits
            .as_ref()
            .map(|c| {
                c.cast
                    .iter()
                    .take(30)
                    .map(|a| CastMember {
                        name: a.name.clone().unwrap_or_default(),
                        character: a.roles.first().and_then(|r| r.character.clone()),
                        profile_url: Self::profile_url(a.profile_path.as_deref()),
                        order: a.order,
                        external_id: Some(a.id.to_string()),
                    })
                    .collect()
            })
            .unwrap_or_default();

        let crew = s
            .aggregate_credits
            .as_ref()
            .map(|c| {
                c.crew
                    .iter()
                    .filter(|c| {
                        c.jobs.iter().any(|j| {
                            matches!(
                                j.job.as_deref(),
                                Some("Executive Producer") | Some("Creator")
                            )
                        })
                    })
                    .map(|c| {
                        let job = c
                            .jobs
                            .iter()
                            .find_map(|j| match j.job.as_deref() {
                                Some("Executive Producer") | Some("Creator") => j.job.clone(),
                                _ => None,
                            })
                            .unwrap_or_default();
                        CrewMember {
                            name: c.name.clone().unwrap_or_default(),
                            department: c.department.clone().unwrap_or_default(),
                            job,
                            profile_url: Self::profile_url(c.profile_path.as_deref()),
                            external_id: Some(c.id.to_string()),
                        }
                    })
                    .collect()
            })
            .unwrap_or_default();

        let images = s
            .images
            .as_ref()
            .map(|imgs| {
                let mut out = Vec::new();
                for p in filter_en_images(&imgs.posters).take(5) {
                    if let Some(path) = &p.file_path {
                        out.push(MediaImage {
                            url: format!("{IMAGE_BASE}w500{path}"),
                            image_type: "poster".into(),
                            width: p.width,
                            height: p.height,
                        });
                    }
                }
                for b in filter_en_images(&imgs.backdrops).take(5) {
                    if let Some(path) = &b.file_path {
                        out.push(MediaImage {
                            url: format!("{IMAGE_BASE}w1280{path}"),
                            image_type: "backdrop".into(),
                            width: b.width,
                            height: b.height,
                        });
                    }
                }
                for l in filter_en_images(&imgs.logos).take(3) {
                    if let Some(path) = &l.file_path {
                        out.push(MediaImage {
                            url: format!("{IMAGE_BASE}w500{path}"),
                            image_type: "logo".into(),
                            width: l.width,
                            height: l.height,
                        });
                    }
                }
                out
            })
            .unwrap_or_default();

        // Prefer English images from the images append over the main response paths,
        // which TMDB may return in the original language even with language=en-US.
        let en_poster_url = s
            .images
            .as_ref()
            .and_then(|imgs| filter_en_images(&imgs.posters).next())
            .and_then(|p| p.file_path.as_ref())
            .map(|path| format!("{IMAGE_BASE}w500{path}"))
            .or_else(|| Self::poster_url(s.poster_path.as_deref()));

        let en_backdrop_url = s
            .images
            .as_ref()
            .and_then(|imgs| filter_en_images(&imgs.backdrops).next())
            .and_then(|b| b.file_path.as_ref())
            .map(|path| format!("{IMAGE_BASE}w1280{path}"))
            .or_else(|| Self::backdrop_url(s.backdrop_path.as_deref()));

        let logo_url = s
            .images
            .as_ref()
            .and_then(|imgs| filter_en_images(&imgs.logos).next())
            .and_then(|l| l.file_path.as_ref())
            .map(|path| format!("{IMAGE_BASE}w500{path}"));

        let seasons: Vec<SeasonSummary> = s
            .seasons
            .iter()
            .map(|sn| SeasonSummary {
                season_number: sn.season_number as u32,
                episode_count: sn.episode_count.map(|c| c as u32),
                air_date: sn.air_date.clone(),
                poster_url: Self::poster_url(sn.poster_path.as_deref()),
                title: sn.name.clone(),
            })
            .collect();

        let network = s.networks.first().map(|n| n.name.clone());

        Ok(TvShowDetails {
            provider: "tmdb".into(),
            provider_id: s.id.to_string(),
            title: s.name.unwrap_or_default(),
            year: s
                .first_air_date
                .as_deref()
                .and_then(|d| d.get(..4))
                .and_then(|y| y.parse().ok()),
            status: crate::repositories::normalize_show_status(s.status),
            description: s.overview,
            poster_url: en_poster_url,
            backdrop_url: en_backdrop_url,
            logo_url,
            identifiers: VideoIdentifiers {
                tmdb_id: Some(s.id.to_string()),
                imdb_id: s
                    .external_ids
                    .as_ref()
                    .and_then(|e| e.imdb_id.clone())
                    .filter(|v| !v.is_empty()),
                tvdb_id: s
                    .external_ids
                    .as_ref()
                    .and_then(|e| e.tvdb_id)
                    .map(|id| id.to_string()),
                ..Default::default()
            },
            genres: s.genres.iter().map(|g| g.name.clone()).collect(),
            rating: s.vote_average.map(|r| r / 2.0),
            rating_count: s.vote_count,
            network,
            content_rating,
            first_aired: s.first_air_date.clone(),
            runtime: s.episode_run_time.first().map(|r| *r as u32),
            seasons,
            cast,
            crew,
            images,
        })
    }

    async fn get_season(
        &self,
        show_id: &str,
        season: u32,
    ) -> Result<SeasonDetails, VideoProviderError> {
        let key = self.require_key()?;
        let base_url = &self.base_url;

        let url = format!("{base_url}/tv/{show_id}/season/{season}?language=en-US");

        let s: TmdbSeasonDetail = self
            .get(&url, key)
            .send()
            .await?
            .error_for_status()
            .map_err(VideoProviderError::Http)?
            .json()
            .await?;

        let episodes = s
            .episodes
            .into_iter()
            .map(|ep| EpisodeDetails {
                episode_number: ep.episode_number as u32,
                title: ep.name,
                air_date: ep.air_date,
                air_time: None,
                runtime: ep.runtime.map(|r| r as u32),
                still_url: Self::still_url(ep.still_path.as_deref()),
                description: ep.overview,
                provider_id: ep.id.map(|id| id.to_string()),
                country: None,
                credits: Some(Self::map_episode_credits(&ep.guest_stars, &ep.crew)),
            })
            .collect();

        Ok(SeasonDetails {
            provider: "tmdb".into(),
            show_id: show_id.into(),
            season_number: s.season_number as u32,
            title: s.name,
            air_date: s.air_date,
            description: s.overview,
            poster_url: Self::poster_url(s.poster_path.as_deref()),
            episodes,
        })
    }

    async fn get_episode_credits(
        &self,
        show_id: &str,
        season: u32,
        episode: u32,
    ) -> Result<EpisodeCredits, VideoProviderError> {
        let key = self.require_key()?;
        let base_url = &self.base_url;
        let url = format!(
            "{base_url}/tv/{show_id}/season/{season}/episode/{episode}/credits?language=en-US"
        );

        let c: TmdbEpisodeCredits = self
            .get(&url, key)
            .send()
            .await?
            .error_for_status()
            .map_err(VideoProviderError::Http)?
            .json()
            .await?;

        Ok(Self::map_episode_credits(&c.guest_stars, &c.crew))
    }

    async fn fetch_image(&self, url: &str) -> Result<Vec<u8>, VideoProviderError> {
        let resp = self.client.get(url).send().await?;
        if !resp.status().is_success() {
            return Err(VideoProviderError::Other(format!(
                "Image fetch failed: {}",
                resp.status()
            )));
        }
        Ok(resp.bytes().await?.to_vec())
    }

    async fn get_person(
        &self,
        provider_id: &str,
    ) -> Result<PersonProviderDetails, VideoProviderError> {
        let key = self.require_key()?;
        let base_url = &self.base_url;

        let url = format!("{base_url}/person/{provider_id}?language=en-US");

        let p: TmdbPersonDetail = self
            .get(&url, key)
            .send()
            .await?
            .error_for_status()
            .map_err(VideoProviderError::Http)?
            .json()
            .await?;

        let mut identifiers = std::collections::HashMap::new();
        identifiers.insert("tmdb_id".to_string(), p.id.to_string());
        if let Some(ref imdb_id) = p.imdb_id
            && !imdb_id.is_empty()
        {
            identifiers.insert("imdb_id".to_string(), imdb_id.clone());
        }
        if let Some(ref place) = p.place_of_birth
            && !place.is_empty()
        {
            identifiers.insert("place_of_birth".to_string(), place.clone());
        }
        if let Some(ref dept) = p.known_for_department
            && !dept.is_empty()
        {
            identifiers.insert("known_for_department".to_string(), dept.clone());
        }

        Ok(PersonProviderDetails {
            name: p.name.unwrap_or_default(),
            biography: p.biography.filter(|b| !b.is_empty()),
            birthday: p.birthday.filter(|d| !d.is_empty()),
            deathday: p.deathday.filter(|d| !d.is_empty()),
            place_of_birth: p.place_of_birth.filter(|p| !p.is_empty()),
            known_for_department: p.known_for_department.filter(|d| !d.is_empty()),
            profile_url: Self::profile_url(p.profile_path.as_deref()),
            identifiers,
        })
    }

    async fn get_person_credits(
        &self,
        provider_id: &str,
    ) -> Result<PersonCombinedCredits, VideoProviderError> {
        let key = self.require_key()?;
        let base_url = &self.base_url;

        let url = format!("{base_url}/person/{provider_id}/combined_credits?language=en-US");

        let resp: TmdbCombinedCredits = self
            .get(&url, key)
            .send()
            .await?
            .error_for_status()
            .map_err(VideoProviderError::Http)?
            .json()
            .await?;

        let cast = resp
            .cast
            .into_iter()
            .map(|c| PersonCreditCast {
                provider_id: c.id.to_string(),
                media_type: c.media_type.unwrap_or_default(),
                title: c.title.or(c.name).unwrap_or_default(),
                character: c.character.filter(|s| !s.is_empty()),
                poster_url: Self::poster_url(c.poster_path.as_deref()),
                release_date: c.release_date.or(c.first_air_date),
                vote_average: c.vote_average,
                vote_count: c.vote_count,
            })
            .collect();

        let crew = resp
            .crew
            .into_iter()
            .map(|c| PersonCreditCrew {
                provider_id: c.id.to_string(),
                media_type: c.media_type.unwrap_or_default(),
                title: c.title.or(c.name).unwrap_or_default(),
                department: c.department.filter(|s| !s.is_empty()),
                job: c.job.filter(|s| !s.is_empty()),
                poster_url: Self::poster_url(c.poster_path.as_deref()),
                release_date: c.release_date.or(c.first_air_date),
            })
            .collect();

        Ok(PersonCombinedCredits { cast, crew })
    }
}

// ============================================================================
// Helpers
// ============================================================================

/// Filter images to English or language-neutral (null) entries, skipping SVGs
/// (which can't be rasterized by our image pipeline).
fn filter_en_images(images: &[TmdbImage]) -> impl Iterator<Item = &TmdbImage> {
    images.iter().filter(|img| {
        let lang_ok = match &img.iso_639_1 {
            None => true,
            Some(lang) => lang == "en",
        };
        let not_svg = img
            .file_path
            .as_deref()
            .is_none_or(|p| !p.ends_with(".svg"));
        lang_ok && not_svg
    })
}

// ============================================================================
// TMDB API Response Types
// ============================================================================

#[derive(Deserialize)]
struct TmdbSearchResponse {
    results: Vec<TmdbMovieSearchItem>,
}

#[derive(Deserialize)]
struct TmdbMovieSearchItem {
    id: i64,
    title: Option<String>,
    overview: Option<String>,
    release_date: Option<String>,
    poster_path: Option<String>,
    backdrop_path: Option<String>,
    vote_average: Option<f64>,
    vote_count: Option<i32>,
    #[serde(default)]
    popularity: Option<f64>,
}

#[derive(Deserialize)]
struct TmdbTvSearchResponse {
    results: Vec<TmdbTvSearchItem>,
}

#[derive(Deserialize)]
struct TmdbTvSearchItem {
    id: i64,
    name: Option<String>,
    overview: Option<String>,
    first_air_date: Option<String>,
    poster_path: Option<String>,
    backdrop_path: Option<String>,
    vote_average: Option<f64>,
    vote_count: Option<i32>,
    #[serde(default)]
    popularity: Option<f64>,
}

#[derive(Deserialize)]
struct TmdbWatchProviderListResponse {
    results: Vec<TmdbWatchProviderEntry>,
}

#[derive(Deserialize)]
struct TmdbWatchProviderEntry {
    provider_id: i32,
    provider_name: String,
    logo_path: Option<String>,
}

/// Public info about a streaming/watch provider from TMDB.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WatchProviderInfo {
    pub provider_id: i32,
    pub provider_name: String,
    pub logo_url: Option<String>,
}

#[derive(Deserialize)]
struct TmdbMovieDetail {
    id: i64,
    title: Option<String>,
    original_title: Option<String>,
    overview: Option<String>,
    tagline: Option<String>,
    runtime: Option<i32>,
    release_date: Option<String>,
    poster_path: Option<String>,
    backdrop_path: Option<String>,
    imdb_id: Option<String>,
    vote_average: Option<f64>,
    vote_count: Option<i32>,
    genres: Vec<TmdbGenre>,
    credits: Option<TmdbCredits>,
    images: Option<TmdbImages>,
    release_dates: Option<TmdbReleaseDates>,
}

#[derive(Deserialize)]
struct TmdbTvDetail {
    id: i64,
    name: Option<String>,
    overview: Option<String>,
    status: Option<String>,
    first_air_date: Option<String>,
    poster_path: Option<String>,
    backdrop_path: Option<String>,
    vote_average: Option<f64>,
    vote_count: Option<i32>,
    #[serde(default)]
    episode_run_time: Vec<i32>,
    genres: Vec<TmdbGenre>,
    #[serde(default)]
    networks: Vec<TmdbNetwork>,
    #[serde(default)]
    seasons: Vec<TmdbSeasonSummary>,
    aggregate_credits: Option<TmdbAggregateCredits>,
    images: Option<TmdbImages>,
    content_ratings: Option<TmdbContentRatings>,
    external_ids: Option<TmdbExternalIds>,
}

#[derive(Deserialize)]
struct TmdbExternalIds {
    imdb_id: Option<String>,
    tvdb_id: Option<i64>,
}

#[derive(Deserialize)]
struct TmdbGenre {
    name: String,
}

#[derive(Deserialize)]
struct TmdbNetwork {
    name: String,
}

#[derive(Deserialize)]
struct TmdbSeasonSummary {
    season_number: i32,
    episode_count: Option<i32>,
    air_date: Option<String>,
    poster_path: Option<String>,
    name: Option<String>,
}

#[derive(Deserialize)]
struct TmdbEpisodeCredits {
    #[serde(default)]
    guest_stars: Vec<TmdbCast>,
    #[serde(default)]
    crew: Vec<TmdbCrew>,
}

#[derive(Deserialize)]
struct TmdbCredits {
    #[serde(default)]
    cast: Vec<TmdbCast>,
    #[serde(default)]
    crew: Vec<TmdbCrew>,
}

#[derive(Deserialize)]
struct TmdbCast {
    id: i64,
    name: Option<String>,
    character: Option<String>,
    profile_path: Option<String>,
    order: Option<i32>,
}

#[derive(Deserialize)]
struct TmdbCrew {
    id: i64,
    name: Option<String>,
    department: Option<String>,
    job: Option<String>,
    profile_path: Option<String>,
}

#[derive(Deserialize)]
struct TmdbAggregateCredits {
    #[serde(default)]
    cast: Vec<TmdbAggregateCast>,
    #[serde(default)]
    crew: Vec<TmdbAggregateCrew>,
}

#[derive(Deserialize)]
struct TmdbAggregateCast {
    id: i64,
    name: Option<String>,
    profile_path: Option<String>,
    order: Option<i32>,
    #[serde(default)]
    roles: Vec<TmdbRole>,
}

#[derive(Deserialize)]
struct TmdbRole {
    character: Option<String>,
}

#[derive(Deserialize)]
struct TmdbAggregateCrew {
    id: i64,
    name: Option<String>,
    profile_path: Option<String>,
    department: Option<String>,
    #[serde(default)]
    jobs: Vec<TmdbJob>,
}

#[derive(Deserialize)]
struct TmdbJob {
    job: Option<String>,
}

#[derive(Deserialize)]
struct TmdbImages {
    #[serde(default)]
    posters: Vec<TmdbImage>,
    #[serde(default)]
    backdrops: Vec<TmdbImage>,
    #[serde(default)]
    logos: Vec<TmdbImage>,
}

#[derive(Deserialize)]
struct TmdbImage {
    file_path: Option<String>,
    width: Option<u32>,
    height: Option<u32>,
    iso_639_1: Option<String>,
}

#[derive(Deserialize)]
struct TmdbReleaseDates {
    #[serde(default)]
    results: Vec<TmdbReleaseDateCountry>,
}

#[derive(Deserialize)]
struct TmdbReleaseDateCountry {
    iso_3166_1: String,
    #[serde(default)]
    release_dates: Vec<TmdbReleaseDateEntry>,
}

#[derive(Deserialize)]
struct TmdbReleaseDateEntry {
    certification: Option<String>,
}

#[derive(Deserialize)]
struct TmdbContentRatings {
    #[serde(default)]
    results: Vec<TmdbContentRating>,
}

#[derive(Deserialize)]
struct TmdbContentRating {
    iso_3166_1: String,
    rating: Option<String>,
}

#[derive(Deserialize)]
struct TmdbSeasonDetail {
    season_number: i32,
    name: Option<String>,
    overview: Option<String>,
    air_date: Option<String>,
    poster_path: Option<String>,
    #[serde(default)]
    episodes: Vec<TmdbEpisode>,
}

#[derive(Deserialize)]
struct TmdbEpisode {
    id: Option<i64>,
    episode_number: i32,
    name: Option<String>,
    overview: Option<String>,
    air_date: Option<String>,
    runtime: Option<i32>,
    still_path: Option<String>,
    // TMDB returns these on every episode of the season payload; kobbler used
    // to drop them on the floor. They are the only source of guest stars.
    #[serde(default)]
    guest_stars: Vec<TmdbCast>,
    #[serde(default)]
    crew: Vec<TmdbCrew>,
}

#[derive(Deserialize)]
struct TmdbFindResponse {
    #[serde(default)]
    movie_results: Vec<TmdbFindItem>,
    #[serde(default)]
    tv_results: Vec<TmdbFindItem>,
}

#[derive(Deserialize)]
struct TmdbFindItem {
    id: i64,
}

// ============================================================================
// TMDB Person API Response Types
// ============================================================================

#[derive(Deserialize)]
struct TmdbPersonDetail {
    id: i64,
    name: Option<String>,
    biography: Option<String>,
    birthday: Option<String>,
    deathday: Option<String>,
    place_of_birth: Option<String>,
    known_for_department: Option<String>,
    profile_path: Option<String>,
    imdb_id: Option<String>,
}

#[derive(Deserialize)]
struct TmdbCombinedCredits {
    #[serde(default)]
    cast: Vec<TmdbCombinedCast>,
    #[serde(default)]
    crew: Vec<TmdbCombinedCrew>,
}

#[derive(Deserialize)]
struct TmdbCombinedCast {
    id: i64,
    media_type: Option<String>,
    title: Option<String>,
    name: Option<String>,
    character: Option<String>,
    poster_path: Option<String>,
    release_date: Option<String>,
    first_air_date: Option<String>,
    vote_average: Option<f64>,
    vote_count: Option<i64>,
}

#[derive(Deserialize)]
struct TmdbCombinedCrew {
    id: i64,
    media_type: Option<String>,
    title: Option<String>,
    name: Option<String>,
    department: Option<String>,
    job: Option<String>,
    poster_path: Option<String>,
    release_date: Option<String>,
    first_air_date: Option<String>,
}

/// Filter TMDB alternate titles to English-relevant countries and deduplicate.
fn filter_alternate_titles(titles: Vec<TmdbAlternativeTitle>, primary_title: &str) -> Vec<String> {
    let primary_lower = primary_title.to_lowercase();
    let mut seen = std::collections::HashSet::new();
    seen.insert(primary_lower.clone());
    titles
        .into_iter()
        .filter(|t| matches!(t.iso_3166_1.as_str(), "US" | "GB" | ""))
        .filter_map(|t| {
            let title = t.title.trim().to_string();
            if title.is_empty() || !seen.insert(title.to_lowercase()) {
                None
            } else {
                Some(title)
            }
        })
        .collect()
}

// Movie alternate titles: GET /movie/{id}/alternative_titles
#[derive(Deserialize)]
struct TmdbMovieAlternativeTitles {
    titles: Vec<TmdbAlternativeTitle>,
}

// TV alternate titles: GET /tv/{id}/alternative_titles
#[derive(Deserialize)]
struct TmdbTvAlternativeTitles {
    results: Vec<TmdbAlternativeTitle>,
}

#[derive(Deserialize)]
struct TmdbAlternativeTitle {
    iso_3166_1: String,
    title: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Trimmed to the shape TMDB actually returns from
    /// `GET /tv/{id}/season/{n}?language=en-US`.
    fn season_payload() -> serde_json::Value {
        serde_json::json!({
            "season_number": 2,
            "name": "Season 2",
            "air_date": "2009-03-08",
            "overview": "",
            "poster_path": "/s2.jpg",
            "episodes": [{
                "id": 62130,
                "episode_number": 6,
                "name": "Peekaboo",
                "overview": "Jesse tracks down the thieves.",
                "air_date": "2009-04-12",
                "runtime": 47,
                "still_path": "/still.jpg",
                "guest_stars": [
                    {"id": 84497, "name": "Dale Dickey", "character": "Spooge's Woman",
                     "profile_path": "/dd.jpg", "order": 0},
                    {"id": 1223786, "name": "David Ury", "character": "Spooge",
                     "profile_path": null, "order": 1}
                ],
                "crew": [
                    {"id": 66633, "name": "Vince Gilligan", "department": "Writing",
                     "job": "Writer", "profile_path": null},
                    {"id": 1, "name": "Somebody", "department": "Camera",
                     "job": "Gaffer", "profile_path": null}
                ]
            }]
        })
    }

    #[test]
    fn list_results_carry_full_size_backdrops() {
        let movies: TmdbSearchResponse = serde_json::from_value(serde_json::json!({
            "results": [{"id": 1, "title": "M", "poster_path": "/p.jpg", "backdrop_path": "/b.jpg"}]
        }))
        .unwrap();
        let shows: TmdbTvSearchResponse = serde_json::from_value(serde_json::json!({
            "results": [{"id": 2, "name": "S", "poster_path": "/p.jpg", "backdrop_path": "/b.jpg"}]
        }))
        .unwrap();

        let expected = Some(format!("{IMAGE_BASE}w1280/b.jpg"));
        assert_eq!(
            TmdbProvider::movies_from_search(movies)[0].backdrop_url,
            expected
        );
        assert_eq!(
            TmdbProvider::tv_from_search(shows)[0].backdrop_url,
            expected
        );
    }

    #[test]
    fn season_payload_keeps_guest_stars_and_crew() {
        let s: TmdbSeasonDetail =
            serde_json::from_value(season_payload()).expect("season should deserialize");
        let ep = &s.episodes[0];

        let credits = TmdbProvider::map_episode_credits(&ep.guest_stars, &ep.crew);

        let names: Vec<&str> = credits
            .guest_stars
            .iter()
            .map(|g| g.name.as_str())
            .collect();
        assert_eq!(names, vec!["Dale Dickey", "David Ury"]);
        assert_eq!(
            credits.guest_stars[0].character.as_deref(),
            Some("Spooge's Woman")
        );
        assert_eq!(credits.guest_stars[0].external_id.as_deref(), Some("84497"));
        assert!(credits.guest_stars[0].profile_url.is_some());
        assert!(credits.guest_stars[1].profile_url.is_none());

        // Gaffers are not the couch question; writers and directors are.
        assert_eq!(credits.crew.len(), 1);
        assert_eq!(credits.crew[0].name, "Vince Gilligan");
        assert_eq!(credits.crew[0].job, "Writer");
    }

    #[test]
    fn episode_without_guest_stars_maps_to_empty_not_missing() {
        let s: TmdbSeasonDetail = serde_json::from_value(serde_json::json!({
            "season_number": 1,
            "episodes": [{"id": 1, "episode_number": 1}]
        }))
        .expect("season should deserialize");
        let credits =
            TmdbProvider::map_episode_credits(&s.episodes[0].guest_stars, &s.episodes[0].crew);
        assert!(credits.guest_stars.is_empty());
        assert!(credits.crew.is_empty());
    }
}
