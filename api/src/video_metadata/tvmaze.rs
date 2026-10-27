use async_trait::async_trait;
use serde::Deserialize;

use super::types::{
    EpisodeDetails, MovieResult, SeasonSummary, TvShowResult, VideoCapability, VideoCapabilitySet,
    VideoConfigField, VideoIdentifiers, VideoProviderInfo,
};
use super::{
    MovieDetails, MovieQuery, SeasonDetails, TvQuery, TvShowDetails, VideoMetadataProvider,
    VideoProviderError, VideoProviderFactory,
};

// ============================================================================
// Provider Info
// ============================================================================

static CONFIG_FIELDS: &[VideoConfigField] = &[];

static CAPABILITIES: &[VideoCapability] = &[
    VideoCapability::TvSearch,
    VideoCapability::TvDetails,
    VideoCapability::SeasonDetails,
    VideoCapability::ImageFetch,
];

pub static INFO: VideoProviderInfo = VideoProviderInfo {
    id: crate::http::provider_ids::TVMAZE,
    display_name: "TVMaze",
    description: "Free TV show metadata. No API key required. TV shows only.",
    website: "https://www.tvmaze.com",
    capabilities: VideoCapabilitySet::new(CAPABILITIES),
    config_fields: CONFIG_FIELDS,
    requests_per_minute: 120, // TVMaze documents 20/10sec = 2/sec
};

const BASE_URL: &str = "https://api.tvmaze.com";

// ============================================================================
// Factory
// ============================================================================

pub struct TvMazeFactory;

impl VideoProviderFactory for TvMazeFactory {
    fn info(&self) -> &'static VideoProviderInfo {
        &INFO
    }

    fn create(
        &self,
        client: reqwest::Client,
        _config: &serde_json::Value,
    ) -> Box<dyn VideoMetadataProvider> {
        Box::new(TvMazeProvider { client })
    }
}

// ============================================================================
// Provider
// ============================================================================

pub struct TvMazeClient {
    client: reqwest::Client,
}

/// Result from a TVMaze IMDB lookup.
pub struct TvMazeLookupResult {
    pub tvmaze_id: String,
    pub title: String,
    pub year: Option<u32>,
    pub imdb_id: Option<String>,
    pub tvdb_id: Option<String>,
}

impl TvMazeClient {
    pub fn new(client: reqwest::Client) -> Self {
        Self { client }
    }

    /// Look up a TV show by its IMDB ID.
    pub async fn lookup_by_imdb(
        &self,
        imdb_id: &str,
    ) -> Result<TvMazeLookupResult, super::VideoProviderError> {
        let url = format!(
            "{BASE_URL}/lookup/shows?imdb={}",
            urlencoding::encode(imdb_id)
        );
        let resp: TvMazeShowDetail = self
            .client
            .get(&url)
            .send()
            .await?
            .error_for_status()
            .map_err(super::VideoProviderError::Http)?
            .json()
            .await?;

        let year = resp
            .premiered
            .as_deref()
            .and_then(|d| d.get(..4))
            .and_then(|y| y.parse().ok());

        let (imdb, tvdb) = resp
            .externals
            .map(|e| (e.imdb, e.thetvdb.map(|id| id.to_string())))
            .unwrap_or_default();

        Ok(TvMazeLookupResult {
            tvmaze_id: resp.id.to_string(),
            title: resp.name.unwrap_or_default(),
            year,
            imdb_id: imdb,
            tvdb_id: tvdb,
        })
    }
}

struct TvMazeProvider {
    client: reqwest::Client,
}

#[async_trait]
impl VideoMetadataProvider for TvMazeProvider {
    fn info(&self) -> &'static VideoProviderInfo {
        &INFO
    }

    async fn search_movies(
        &self,
        _query: &MovieQuery,
    ) -> Result<Vec<MovieResult>, VideoProviderError> {
        // TVMaze is TV-only
        Ok(Vec::new())
    }

    async fn search_tv(&self, query: &TvQuery) -> Result<Vec<TvShowResult>, VideoProviderError> {
        let url = format!(
            "{BASE_URL}/search/shows?q={}",
            urlencoding::encode(&query.title)
        );

        let resp: Vec<TvMazeSearchWrapper> = self
            .client
            .get(&url)
            .send()
            .await?
            .error_for_status()
            .map_err(VideoProviderError::Http)?
            .json()
            .await?;

        Ok(resp
            .into_iter()
            .take(20)
            .map(|w| {
                let s = w.show;
                TvShowResult {
                    provider: "tvmaze".into(),
                    provider_id: s.id.to_string(),
                    title: s.name.unwrap_or_default(),
                    year: s
                        .premiered
                        .as_deref()
                        .and_then(|d| d.get(..4))
                        .and_then(|y| y.parse().ok()),
                    status: crate::repositories::normalize_show_status(s.status),
                    description: s.summary.map(|s| strip_html_tags(&s)),
                    poster_url: s.image.and_then(|img| img.medium.or(img.original)),
                    backdrop_url: None,
                    network: s.network.map(|n| n.name),
                    rating: score(s.rating),
                    rating_count: None,
                    identifiers: VideoIdentifiers {
                        tvmaze_id: Some(s.id.to_string()),
                        ..Default::default()
                    },
                    genres: s.genres,
                }
            })
            .collect())
    }

    async fn get_movie(&self, _provider_id: &str) -> Result<MovieDetails, VideoProviderError> {
        Err(VideoProviderError::NotSupported)
    }

    async fn get_tv_show(&self, provider_id: &str) -> Result<TvShowDetails, VideoProviderError> {
        let url = format!("{BASE_URL}/shows/{provider_id}?embed=seasons");

        let s: TvMazeShowDetail = self
            .client
            .get(&url)
            .send()
            .await?
            .error_for_status()
            .map_err(VideoProviderError::Http)?
            .json()
            .await?;

        let seasons = s
            .embedded
            .as_ref()
            .and_then(|e| e.seasons.as_ref())
            .map(|seasons| {
                seasons
                    .iter()
                    .map(|sn| SeasonSummary {
                        season_number: sn.number as u32,
                        episode_count: sn.episode_order.map(|c| c as u32),
                        air_date: sn.premiere_date.clone(),
                        poster_url: sn
                            .image
                            .as_ref()
                            .and_then(|img| img.medium.clone().or(img.original.clone())),
                        title: sn.name.clone(),
                    })
                    .collect()
            })
            .unwrap_or_default();

        let mut identifiers = VideoIdentifiers {
            tvmaze_id: Some(s.id.to_string()),
            ..Default::default()
        };
        if let Some(ext) = &s.externals {
            identifiers.imdb_id = ext.imdb.clone();
            identifiers.tvdb_id = ext.thetvdb.map(|id| id.to_string());
        }

        let network = s.network.map(|n| n.name);

        Ok(TvShowDetails {
            provider: "tvmaze".into(),
            provider_id: s.id.to_string(),
            title: s.name.unwrap_or_default(),
            year: s
                .premiered
                .as_deref()
                .and_then(|d| d.get(..4))
                .and_then(|y| y.parse().ok()),
            status: crate::repositories::normalize_show_status(s.status),
            description: s.summary.map(|s| strip_html_tags(&s)),
            poster_url: s
                .image
                .as_ref()
                .and_then(|img| img.medium.clone().or(img.original.clone())),
            backdrop_url: None,
            logo_url: None,
            identifiers,
            genres: s.genres,
            rating: score(s.rating),
            rating_count: None,
            network,
            content_rating: None,
            first_aired: s.premiered.clone(),
            runtime: s.runtime.map(|r| r as u32),
            seasons,
            cast: Vec::new(),
            crew: Vec::new(),
            images: Vec::new(),
        })
    }

    async fn get_season(
        &self,
        show_id: &str,
        season: u32,
    ) -> Result<SeasonDetails, VideoProviderError> {
        // TVMaze episodes endpoint returns all episodes for a show
        let url = format!("{BASE_URL}/shows/{show_id}/episodes");

        let all_eps: Vec<TvMazeEpisode> = self
            .client
            .get(&url)
            .send()
            .await?
            .error_for_status()
            .map_err(VideoProviderError::Http)?
            .json()
            .await?;

        let episodes: Vec<EpisodeDetails> = all_eps
            .into_iter()
            .filter(|ep| ep.season == season as i32)
            .map(|ep| {
                let air_time = ep.air_time();
                EpisodeDetails {
                    episode_number: ep.number as u32,
                    title: ep.name,
                    air_date: ep.airdate,
                    air_time,
                    runtime: ep.runtime.map(|r| r as u32),
                    still_url: ep.image.and_then(|img| img.medium.or(img.original)),
                    description: ep.summary.map(|s| strip_html_tags(&s)),
                    provider_id: ep.id.map(|id| id.to_string()),
                    country: None,
                    credits: None,
                    sport: None,
                }
            })
            .collect();

        Ok(SeasonDetails {
            provider: "tvmaze".into(),
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
        let resp = self.client.get(url).send().await?;
        if !resp.status().is_success() {
            return Err(VideoProviderError::Other(format!(
                "Image fetch failed: {}",
                resp.status()
            )));
        }
        Ok(resp.bytes().await?.to_vec())
    }
}

/// Strip basic HTML tags from TVMaze summaries.
fn strip_html_tags(html: &str) -> String {
    let mut result = String::with_capacity(html.len());
    let mut in_tag = false;
    for ch in html.chars() {
        match ch {
            '<' => in_tag = true,
            '>' => in_tag = false,
            _ if !in_tag => result.push(ch),
            _ => {}
        }
    }
    result.trim().to_string()
}

// ============================================================================
// TVMaze API Response Types
// ============================================================================

#[derive(Deserialize)]
struct TvMazeSearchWrapper {
    show: TvMazeShow,
}

#[derive(Deserialize)]
struct TvMazeShow {
    id: i64,
    name: Option<String>,
    status: Option<String>,
    premiered: Option<String>,
    summary: Option<String>,
    image: Option<TvMazeImage>,
    network: Option<TvMazeNetwork>,
    rating: Option<TvMazeRating>,
    #[serde(default)]
    genres: Vec<String>,
}

#[derive(Deserialize)]
struct TvMazeShowDetail {
    id: i64,
    name: Option<String>,
    status: Option<String>,
    premiered: Option<String>,
    summary: Option<String>,
    runtime: Option<i32>,
    image: Option<TvMazeImage>,
    network: Option<TvMazeNetwork>,
    rating: Option<TvMazeRating>,
    externals: Option<TvMazeExternals>,
    #[serde(default)]
    genres: Vec<String>,
    #[serde(rename = "_embedded")]
    embedded: Option<TvMazeEmbedded>,
}

#[derive(Deserialize)]
struct TvMazeEmbedded {
    seasons: Option<Vec<TvMazeSeason>>,
}

#[derive(Deserialize)]
struct TvMazeSeason {
    number: i32,
    name: Option<String>,
    #[serde(rename = "episodeOrder")]
    episode_order: Option<i32>,
    #[serde(rename = "premiereDate")]
    premiere_date: Option<String>,
    image: Option<TvMazeImage>,
}

#[derive(Deserialize)]
struct TvMazeImage {
    medium: Option<String>,
    original: Option<String>,
}

#[derive(Deserialize)]
struct TvMazeNetwork {
    name: String,
}

#[derive(Deserialize)]
struct TvMazeRating {
    average: Option<f64>,
}

fn score(rating: Option<TvMazeRating>) -> Option<f64> {
    rating.and_then(|r| r.average)
}

#[derive(Deserialize)]
struct TvMazeExternals {
    imdb: Option<String>,
    thetvdb: Option<i64>,
}

#[derive(Deserialize)]
struct TvMazeEpisode {
    id: Option<i64>,
    season: i32,
    number: i32,
    name: Option<String>,
    airdate: Option<String>,
    /// Broadcast clock time in the show's own timezone, e.g. "21:00". TVMaze
    /// sends an empty string when it does not know one.
    airtime: Option<String>,
    /// Full ISO-8601 instant. TVMaze emits it even when `airtime` is unknown,
    /// defaulted to midnight — which is why `airtime` gates it below.
    airstamp: Option<String>,
    runtime: Option<i32>,
    summary: Option<String>,
    image: Option<TvMazeImage>,
}

impl TvMazeEpisode {
    fn air_time(&self) -> Option<String> {
        self.airtime.as_deref().filter(|t| !t.trim().is_empty())?;
        self.airstamp
            .as_deref()
            .filter(|s| !s.trim().is_empty())
            .map(str::to_string)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn episode(json: serde_json::Value) -> TvMazeEpisode {
        serde_json::from_value(json).expect("TvMazeEpisode should deserialize")
    }

    #[test]
    fn rating_keeps_tvmaze_ten_point_scale() {
        let show: TvMazeShowDetail = serde_json::from_value(serde_json::json!({
            "id": 1, "rating": {"average": 7.9}
        }))
        .unwrap();
        assert_eq!(score(show.rating), Some(7.9));
        assert_eq!(score(None), None);
    }

    #[test]
    fn airstamp_survives_when_tvmaze_knows_the_time() {
        let ep = episode(serde_json::json!({
            "id": 1,
            "season": 24,
            "number": 7,
            "name": "The Draft",
            "airdate": "2026-09-04",
            "airtime": "21:00",
            "airstamp": "2026-09-04T21:00:00-04:00",
        }));
        assert_eq!(
            ep.air_time().as_deref(),
            Some("2026-09-04T21:00:00-04:00"),
            "an episode with a real airtime must keep the full airstamp"
        );
    }

    #[test]
    fn day_only_stays_day_only() {
        // TVMaze still sends an airstamp here, defaulted to midnight. Passing it
        // through would invent "midnight Thursday" out of nothing.
        for airtime in [serde_json::json!(""), serde_json::json!(null)] {
            let ep = episode(serde_json::json!({
                "id": 1,
                "season": 24,
                "number": 7,
                "airdate": "2026-09-04",
                "airtime": airtime,
                "airstamp": "2026-09-04T00:00:00+00:00",
            }));
            assert_eq!(ep.air_time(), None);
            assert_eq!(ep.airdate.as_deref(), Some("2026-09-04"));
        }
    }

    #[test]
    fn missing_airstamp_is_not_synthesised_from_airtime() {
        let ep = episode(serde_json::json!({
            "id": 1, "season": 1, "number": 1,
            "airdate": "2026-09-04", "airtime": "21:00",
        }));
        assert_eq!(ep.air_time(), None);
    }
}
