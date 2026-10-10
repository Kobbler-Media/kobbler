use async_trait::async_trait;
use serde::Deserialize;
use std::sync::Arc;
use tokio::sync::RwLock;

use super::types::{
    CastMember, EpisodeDetails, MediaImage, MovieResult, SeasonSummary, TvShowResult,
    VideoCapability, VideoCapabilitySet, VideoConfigField, VideoConfigFieldType, VideoIdentifiers,
    VideoProviderInfo,
};
use super::{
    MovieDetails, MovieQuery, SeasonDetails, TvQuery, TvShowDetails, VideoMetadataProvider,
    VideoProviderError, VideoProviderFactory,
};

// ============================================================================
// Provider Info
// ============================================================================

static CONFIG_FIELDS: &[VideoConfigField] = &[VideoConfigField {
    key: "api_key",
    label: "API Key",
    field_type: VideoConfigFieldType::Secret,
    required: true,
    description: Some("TheTVDB project API key"),
    help_url: Some("https://thetvdb.com/api-information"),
    default: None,
}];

static CAPABILITIES: &[VideoCapability] = &[
    VideoCapability::TvSearch,
    VideoCapability::TvDetails,
    VideoCapability::SeasonDetails,
    VideoCapability::ImageFetch,
];

pub static INFO: VideoProviderInfo = VideoProviderInfo {
    id: crate::http::provider_ids::TVDB,
    display_name: "TheTVDB",
    description: "Community-driven TV database. Used by Sonarr/Plex. Requires API key.",
    website: "https://thetvdb.com",
    capabilities: VideoCapabilitySet::new(CAPABILITIES),
    config_fields: CONFIG_FIELDS,
    requests_per_minute: 120, // ~2 req/sec
};

const BASE_URL: &str = "https://api4.thetvdb.com/v4";
const IMAGE_BASE: &str = "https://artworks.thetvdb.com";

// ============================================================================
// Factory
// ============================================================================

pub struct TvdbFactory;

impl VideoProviderFactory for TvdbFactory {
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

        Box::new(TvdbProvider {
            client,
            api_key,
            token: Arc::new(RwLock::new(None)),
        })
    }
}

// ============================================================================
// Provider
// ============================================================================

struct TvdbProvider {
    client: reqwest::Client,
    api_key: String,
    /// Cached bearer token from /login. Refreshed on 401.
    token: Arc<RwLock<Option<String>>>,
}

impl TvdbProvider {
    /// Get a valid bearer token, logging in if necessary.
    async fn get_token(&self) -> Result<String, VideoProviderError> {
        // Check cached token
        {
            let guard = self.token.read().await;
            if let Some(ref tok) = *guard {
                return Ok(tok.clone());
            }
        }
        self.login().await
    }

    /// Authenticate with TVDB and cache the token.
    async fn login(&self) -> Result<String, VideoProviderError> {
        let resp = self
            .client
            .post(format!("{BASE_URL}/login"))
            .json(&serde_json::json!({ "apikey": self.api_key }))
            .send()
            .await?;

        if resp.status() == reqwest::StatusCode::UNAUTHORIZED {
            return Err(VideoProviderError::Other(
                "TVDB authentication failed — check API key".into(),
            ));
        }

        let body: LoginResponse = resp
            .error_for_status()
            .map_err(VideoProviderError::Http)?
            .json()
            .await?;

        let token = body
            .data
            .and_then(|d| d.token)
            .ok_or_else(|| VideoProviderError::Other("TVDB login returned no token".into()))?;

        let mut guard = self.token.write().await;
        *guard = Some(token.clone());
        Ok(token)
    }

    /// Make an authenticated GET request. Retries once on 401 (token expired).
    async fn authed_get(&self, url: &str) -> Result<reqwest::Response, VideoProviderError> {
        let token = self.get_token().await?;

        let resp = self.client.get(url).bearer_auth(&token).send().await?;

        if resp.status() == reqwest::StatusCode::UNAUTHORIZED {
            // Token expired — re-login and retry once
            let new_token = self.login().await?;
            let resp = self.client.get(url).bearer_auth(&new_token).send().await?;
            return resp.error_for_status().map_err(check_rate_limit);
        }

        if resp.status() == reqwest::StatusCode::TOO_MANY_REQUESTS {
            return Err(VideoProviderError::RateLimited);
        }

        resp.error_for_status().map_err(VideoProviderError::Http)
    }
}

fn check_rate_limit(err: reqwest::Error) -> VideoProviderError {
    if err
        .status()
        .is_some_and(|s| s == reqwest::StatusCode::TOO_MANY_REQUESTS)
    {
        VideoProviderError::RateLimited
    } else {
        VideoProviderError::Http(err)
    }
}

/// Build a full image URL from a TVDB artwork path.
fn image_url(path: &str) -> String {
    if path.starts_with("http") {
        path.to_string()
    } else {
        format!("{IMAGE_BASE}{}", path.trim_start_matches('/'))
    }
}

/// Extract year from "YYYY-MM-DD" date string.
fn year_from_date(date: &str) -> Option<u16> {
    date.get(..4)?.parse().ok()
}

#[async_trait]
impl VideoMetadataProvider for TvdbProvider {
    fn info(&self) -> &'static VideoProviderInfo {
        &INFO
    }

    async fn search_movies(
        &self,
        _query: &MovieQuery,
    ) -> Result<Vec<MovieResult>, VideoProviderError> {
        // TVDB is TV-focused; movie support is limited
        Ok(Vec::new())
    }

    async fn search_tv(&self, query: &TvQuery) -> Result<Vec<TvShowResult>, VideoProviderError> {
        let url = format!(
            "{BASE_URL}/search?query={}&type=series&limit=25",
            urlencoding::encode(&query.title)
        );

        let resp = self.authed_get(&url).await?;
        let body: ApiResponse<Vec<SearchResult>> = resp.json().await?;

        let results = body.data.unwrap_or_default();

        Ok(results
            .into_iter()
            .take(20)
            .filter_map(|s| {
                let id = s.tvdb_id.or(s.id)?;
                let title = s.name.or(s.translated_name)?;
                Some(TvShowResult {
                    provider: "tvdb".into(),
                    provider_id: id.clone(),
                    title,
                    year: s
                        .year
                        .and_then(|y| y.parse().ok())
                        .or_else(|| s.first_air_time.as_deref().and_then(year_from_date)),
                    status: crate::repositories::normalize_show_status(s.status),
                    description: s.overview.or(s.translated_overview),
                    poster_url: s.image_url.map(|u| image_url(&u)),
                    backdrop_url: None,
                    network: s.network,
                    rating: None,
                    rating_count: None,
                    identifiers: VideoIdentifiers {
                        tvdb_id: Some(id),
                        ..Default::default()
                    },
                    genres: Vec::new(),
                })
            })
            .collect())
    }

    async fn get_movie(&self, _provider_id: &str) -> Result<MovieDetails, VideoProviderError> {
        Err(VideoProviderError::NotSupported)
    }

    async fn get_tv_show(&self, provider_id: &str) -> Result<TvShowDetails, VideoProviderError> {
        let url = format!("{BASE_URL}/series/{provider_id}/extended?meta=episodes");

        let resp = self.authed_get(&url).await?;
        let body: ApiResponse<SeriesExtended> = resp.json().await?;

        let s = body.data.ok_or_else(|| VideoProviderError::NotFound)?;

        let seasons: Vec<SeasonSummary> = s
            .seasons
            .unwrap_or_default()
            .into_iter()
            // Only include "official" season type (type 1) or default (type 0)
            .filter(|sn| {
                let type_id = sn.season_type.as_ref().and_then(|t| t.id);
                type_id.is_none() || matches!(type_id, Some(0 | 1))
            })
            .map(|sn| SeasonSummary {
                season_number: sn.number as u32,
                episode_count: sn.episode_count.map(|c| c as u32),
                air_date: None,
                poster_url: sn.image.as_deref().map(image_url),
                title: sn.name,
            })
            .collect();

        let cast: Vec<CastMember> = s
            .characters
            .unwrap_or_default()
            .into_iter()
            .filter(|c| c.people_type == Some("Actor".into()))
            .take(30)
            .map(|c| CastMember {
                name: c.person_name.unwrap_or_default(),
                character: c.name,
                profile_url: c.person_img_url.as_deref().map(image_url),
                order: c.sort.map(|s| s as i32),
                external_id: c.people_id.map(|id| id.to_string()),
            })
            .collect();

        let images: Vec<MediaImage> = s
            .artworks
            .unwrap_or_default()
            .into_iter()
            .filter_map(|a| {
                let url = image_url(&a.image?);
                let image_type = match a.artwork_type {
                    Some(1 | 2) => "banner",
                    Some(3) => "background",
                    Some(6 | 7) => "poster",
                    Some(14 | 15) => "backdrop",
                    _ => "other",
                };
                Some(MediaImage {
                    url,
                    image_type: image_type.into(),
                    width: a.width.map(|w| w as u32),
                    height: a.height.map(|h| h as u32),
                })
            })
            .collect();

        let poster_url = images
            .iter()
            .find(|i| i.image_type == "poster")
            .map(|i| i.url.clone())
            .or_else(|| s.image.as_deref().map(image_url));

        let backdrop_url = images
            .iter()
            .find(|i| i.image_type == "backdrop" || i.image_type == "background")
            .map(|i| i.url.clone());

        let mut identifiers = VideoIdentifiers {
            tvdb_id: Some(s.id.to_string()),
            ..Default::default()
        };
        // Extract external IDs if available
        if let Some(remotes) = s.remote_ids {
            for r in &remotes {
                if let (Some(source_name), Some(id)) = (&r.source_name, &r.id) {
                    match source_name.as_str() {
                        "IMDB" => identifiers.imdb_id = Some(id.clone()),
                        "TheMovieDB.com" => identifiers.tmdb_id = Some(id.clone()),
                        _ => {}
                    }
                }
            }
        }

        let network = s
            .original_network
            .and_then(|n| n.name)
            .or_else(|| s.latest_network.and_then(|n| n.name));

        let genres: Vec<String> = s
            .genres
            .unwrap_or_default()
            .into_iter()
            .filter_map(|g| g.name)
            .collect();

        Ok(TvShowDetails {
            provider: "tvdb".into(),
            provider_id: s.id.to_string(),
            title: s.name.unwrap_or_default(),
            year: s
                .year
                .and_then(|y| y.parse().ok())
                .or_else(|| s.first_aired.as_deref().and_then(year_from_date)),
            status: crate::repositories::normalize_show_status(s.status.and_then(|st| st.name)),
            description: s.overview,
            poster_url,
            backdrop_url,
            logo_url: None,
            identifiers,
            genres,
            rating: None,
            rating_count: None,
            network,
            content_rating: None,
            first_aired: s.first_aired,
            runtime: s.average_runtime.map(|r| r as u32),
            seasons,
            cast,
            crew: Vec::new(),
            images,
        })
    }

    async fn get_season(
        &self,
        show_id: &str,
        season: u32,
    ) -> Result<SeasonDetails, VideoProviderError> {
        // TVDB v4: paginated episodes endpoint, filter by season
        let mut all_episodes = Vec::new();
        let mut page = 0;

        loop {
            let url =
                format!("{BASE_URL}/series/{show_id}/episodes/default?season={season}&page={page}");

            let resp = self.authed_get(&url).await?;
            let body: ApiResponse<SeriesEpisodesData> = resp.json().await?;

            let data = body.data;
            let episodes = data
                .as_ref()
                .and_then(|d| d.episodes.as_ref())
                .cloned()
                .unwrap_or_default();

            if episodes.is_empty() {
                break;
            }
            all_episodes.extend(episodes);

            // Check if there are more pages
            if body.links.as_ref().is_some_and(|l| l.next.is_some()) {
                page += 1;
            } else {
                break;
            }
        }

        if all_episodes.is_empty() {
            return Err(VideoProviderError::NotFound);
        }

        let episodes: Vec<EpisodeDetails> = all_episodes
            .into_iter()
            .filter(|ep| ep.season_number == Some(season as i64))
            .map(|ep| EpisodeDetails {
                episode_number: ep.number.unwrap_or(0) as u32,
                title: ep.name,
                air_date: ep.aired,
                air_time: None,
                runtime: ep.runtime.map(|r| r as u32),
                still_url: ep.image.as_deref().map(image_url),
                description: ep.overview,
                provider_id: ep.id.map(|id| id.to_string()),
                country: None,
                credits: None,
            })
            .collect();

        Ok(SeasonDetails {
            provider: "tvdb".into(),
            show_id: show_id.into(),
            season_number: season,
            title: None,
            air_date: episodes.first().and_then(|e| e.air_date.clone()),
            description: None,
            poster_url: None,
            episodes,
        })
    }

    async fn fetch_image(&self, url: &str) -> Result<Vec<u8>, VideoProviderError> {
        let full_url = image_url(url);
        let resp = self.client.get(&full_url).send().await?;
        if !resp.status().is_success() {
            return Err(VideoProviderError::Other(format!(
                "Image fetch failed: {}",
                resp.status()
            )));
        }
        Ok(resp.bytes().await?.to_vec())
    }
}

// ============================================================================
// TVDB v4 API Response Types
// ============================================================================

#[derive(Deserialize)]
struct ApiResponse<T> {
    #[allow(dead_code)]
    status: Option<String>,
    data: Option<T>,
    links: Option<ApiLinks>,
}

#[derive(Deserialize)]
struct ApiLinks {
    next: Option<String>,
}

#[derive(Deserialize)]
struct LoginResponse {
    data: Option<LoginData>,
}

#[derive(Deserialize)]
struct LoginData {
    token: Option<String>,
}

// -- Search --

#[derive(Deserialize)]
struct SearchResult {
    tvdb_id: Option<String>,
    id: Option<String>,
    name: Option<String>,
    #[serde(rename = "translatedName")]
    translated_name: Option<String>,
    overview: Option<String>,
    #[serde(rename = "translatedOverview")]
    translated_overview: Option<String>,
    #[serde(rename = "image_url")]
    image_url: Option<String>,
    status: Option<String>,
    year: Option<String>,
    #[serde(rename = "first_air_time")]
    first_air_time: Option<String>,
    network: Option<String>,
}

// -- Series Extended --

#[derive(Deserialize)]
struct SeriesExtended {
    id: i64,
    name: Option<String>,
    overview: Option<String>,
    image: Option<String>,
    #[serde(rename = "firstAired")]
    first_aired: Option<String>,
    year: Option<String>,
    #[serde(rename = "averageRuntime")]
    average_runtime: Option<i64>,
    status: Option<SeriesStatus>,
    #[serde(rename = "originalNetwork")]
    original_network: Option<Network>,
    #[serde(rename = "latestNetwork")]
    latest_network: Option<Network>,
    seasons: Option<Vec<TvdbSeason>>,
    characters: Option<Vec<TvdbCharacter>>,
    artworks: Option<Vec<TvdbArtwork>>,
    genres: Option<Vec<TvdbGenre>>,
    #[serde(rename = "remoteIds")]
    remote_ids: Option<Vec<TvdbRemoteId>>,
}

#[derive(Deserialize)]
struct SeriesStatus {
    name: Option<String>,
}

#[derive(Deserialize)]
struct Network {
    name: Option<String>,
}

#[derive(Deserialize)]
struct TvdbSeasonType {
    id: Option<i64>,
}

#[derive(Deserialize)]
struct TvdbSeason {
    number: i64,
    name: Option<String>,
    image: Option<String>,
    #[serde(rename = "type")]
    season_type: Option<TvdbSeasonType>,
    #[serde(rename = "episodeCount")]
    episode_count: Option<i64>,
}

#[derive(Deserialize)]
struct TvdbCharacter {
    name: Option<String>,
    #[serde(rename = "personName")]
    person_name: Option<String>,
    #[serde(rename = "personImgURL")]
    person_img_url: Option<String>,
    #[serde(rename = "peopleId")]
    people_id: Option<i64>,
    #[serde(rename = "peopleType")]
    people_type: Option<String>,
    sort: Option<i64>,
}

#[derive(Deserialize)]
struct TvdbArtwork {
    image: Option<String>,
    #[serde(rename = "type")]
    artwork_type: Option<i64>,
    width: Option<i64>,
    height: Option<i64>,
}

#[derive(Deserialize)]
struct TvdbGenre {
    name: Option<String>,
}

#[derive(Deserialize)]
struct TvdbRemoteId {
    id: Option<String>,
    #[serde(rename = "sourceName")]
    source_name: Option<String>,
}

// -- Episodes --

#[derive(Deserialize)]
struct SeriesEpisodesData {
    episodes: Option<Vec<TvdbEpisode>>,
}

#[derive(Deserialize, Clone)]
struct TvdbEpisode {
    id: Option<i64>,
    #[serde(rename = "seasonNumber")]
    season_number: Option<i64>,
    number: Option<i64>,
    name: Option<String>,
    aired: Option<String>,
    runtime: Option<i64>,
    overview: Option<String>,
    image: Option<String>,
}
