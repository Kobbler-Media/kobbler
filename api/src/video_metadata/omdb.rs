use async_trait::async_trait;
use serde::Deserialize;

use super::types::{
    MovieResult, TvShowResult, VideoCapability, VideoCapabilitySet, VideoConfigField,
    VideoConfigFieldType, VideoIdentifiers, VideoProviderInfo,
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
    description: Some("OMDb API key for ratings enrichment"),
    help_url: Some("https://www.omdbapi.com/apikey.aspx"),
    default: None,
}];

static CAPABILITIES: &[VideoCapability] = &[
    VideoCapability::MovieSearch,
    VideoCapability::MovieDetails,
    VideoCapability::Ratings,
];

pub static INFO: VideoProviderInfo = VideoProviderInfo {
    id: crate::http::provider_ids::OMDB,
    display_name: "OMDb",
    description: "Open Movie Database — ratings enrichment from IMDB, Rotten Tomatoes, Metacritic.",
    website: "https://www.omdbapi.com",
    capabilities: VideoCapabilitySet::new(CAPABILITIES),
    config_fields: CONFIG_FIELDS,
    requests_per_minute: 60, // Free tier: 1000/day; 1/sec keeps us well under
};

const BASE_URL: &str = "https://www.omdbapi.com";

// ============================================================================
// Factory
// ============================================================================

pub struct OmdbFactory;

impl VideoProviderFactory for OmdbFactory {
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

        Box::new(OmdbProvider { client, api_key })
    }
}

// ============================================================================
// Provider
// ============================================================================

struct OmdbProvider {
    client: reqwest::Client,
    api_key: String,
}

impl OmdbProvider {
    fn require_key(&self) -> Result<&str, VideoProviderError> {
        if self.api_key.is_empty() {
            Err(VideoProviderError::Other(
                "OMDb API key not configured".into(),
            ))
        } else {
            Ok(&self.api_key)
        }
    }
}

#[async_trait]
impl VideoMetadataProvider for OmdbProvider {
    fn info(&self) -> &'static VideoProviderInfo {
        &INFO
    }

    async fn search_movies(
        &self,
        query: &MovieQuery,
    ) -> Result<Vec<MovieResult>, VideoProviderError> {
        let key = self.require_key()?;

        let mut url = format!(
            "{BASE_URL}/?apikey={key}&t={}&type=movie",
            urlencoding::encode(&query.title)
        );
        if let Some(year) = query.year {
            url.push_str(&format!("&y={year}"));
        }

        let resp: OmdbResponse = self.client.get(&url).send().await?.json().await?;

        if resp.response.as_deref() != Some("True") {
            return Ok(Vec::new());
        }

        let rating = resp
            .imdb_rating
            .as_deref()
            .and_then(|r| r.parse::<f64>().ok())
            .map(|r| r / 2.0);

        let rating_count = resp
            .imdb_votes
            .as_deref()
            .map(|v| v.replace(',', ""))
            .and_then(|v| v.parse::<i32>().ok());

        Ok(vec![MovieResult {
            provider: "omdb".into(),
            provider_id: resp.imdb_id.clone().unwrap_or_default(),
            title: resp.title.unwrap_or_default(),
            year: resp
                .year
                .as_deref()
                .and_then(|y| y.get(..4))
                .and_then(|y| y.parse().ok()),
            description: resp.plot,
            poster_url: resp.poster.filter(|p| p != "N/A"),
            backdrop_url: None,
            rating,
            rating_count,
            identifiers: VideoIdentifiers {
                imdb_id: resp.imdb_id.clone(),
                ..Default::default()
            },
            genres: resp
                .genre
                .as_deref()
                .map(|g| g.split(", ").map(|s| s.to_string()).collect())
                .unwrap_or_default(),
            network: None,
        }])
    }

    async fn search_tv(&self, _query: &TvQuery) -> Result<Vec<TvShowResult>, VideoProviderError> {
        // OMDb is primarily for ratings enrichment, not TV search
        Ok(Vec::new())
    }

    async fn get_movie(&self, provider_id: &str) -> Result<MovieDetails, VideoProviderError> {
        let key = self.require_key()?;

        let url = format!("{BASE_URL}/?apikey={key}&i={provider_id}&plot=full");

        let resp: OmdbResponse = self.client.get(&url).send().await?.json().await?;

        if resp.response.as_deref() != Some("True") {
            return Err(VideoProviderError::NotFound);
        }

        let rating = resp
            .imdb_rating
            .as_deref()
            .and_then(|r| r.parse::<f64>().ok())
            .map(|r| r / 2.0);

        let rating_count = resp
            .imdb_votes
            .as_deref()
            .map(|v| v.replace(',', ""))
            .and_then(|v| v.parse::<i32>().ok());

        let runtime = resp
            .runtime
            .as_deref()
            .and_then(|r| r.trim_end_matches(" min").parse::<u32>().ok());

        let genres: Vec<String> = resp
            .genre
            .as_deref()
            .map(|g| g.split(", ").map(String::from).collect())
            .unwrap_or_default();

        Ok(MovieDetails {
            provider: "omdb".into(),
            provider_id: resp.imdb_id.clone().unwrap_or_default(),
            title: resp.title.unwrap_or_default(),
            original_title: None,
            year: resp
                .year
                .as_deref()
                .and_then(|y| y.get(..4))
                .and_then(|y| y.parse().ok()),
            runtime,
            description: resp.plot,
            tagline: None,
            poster_url: resp.poster.filter(|p| p != "N/A"),
            backdrop_url: None,
            logo_url: None,
            identifiers: VideoIdentifiers {
                imdb_id: resp.imdb_id,
                ..Default::default()
            },
            genres,
            rating,
            rating_count,
            content_rating: resp.rated.filter(|r| r != "N/A"),
            release_date: resp.released.filter(|r| r != "N/A"),
            cast: Vec::new(),
            crew: Vec::new(),
            images: Vec::new(),
        })
    }

    async fn get_tv_show(&self, _provider_id: &str) -> Result<TvShowDetails, VideoProviderError> {
        Err(VideoProviderError::NotSupported)
    }

    async fn get_season(
        &self,
        _show_id: &str,
        _season: u32,
    ) -> Result<SeasonDetails, VideoProviderError> {
        Err(VideoProviderError::NotSupported)
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

// ============================================================================
// OMDb API Response Types
// ============================================================================

#[derive(Deserialize)]
struct OmdbResponse {
    #[serde(rename = "Response")]
    response: Option<String>,
    #[serde(rename = "Title")]
    title: Option<String>,
    #[serde(rename = "Year")]
    year: Option<String>,
    #[serde(rename = "Rated")]
    rated: Option<String>,
    #[serde(rename = "Released")]
    released: Option<String>,
    #[serde(rename = "Runtime")]
    runtime: Option<String>,
    #[serde(rename = "Genre")]
    genre: Option<String>,
    #[serde(rename = "Plot")]
    plot: Option<String>,
    #[serde(rename = "Poster")]
    poster: Option<String>,
    #[serde(rename = "imdbRating")]
    imdb_rating: Option<String>,
    #[serde(rename = "imdbVotes")]
    imdb_votes: Option<String>,
    #[serde(rename = "imdbID")]
    imdb_id: Option<String>,
}
