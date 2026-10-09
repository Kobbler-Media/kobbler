//! Cinemeta client — Stremio's free metadata service.
//!
//! Used as a fallback when TMDB/TVMaze resolution fails for IMDB IDs.
//! No auth required, no rate limits.

use serde::Deserialize;

const BASE_URL: &str = "https://v3-cinemeta.strem.io";

pub struct CinemetaClient {
    http_client: reqwest::Client,
}

pub struct CinemetaResult {
    pub title: String,
    pub year: Option<u32>,
    pub imdb_id: String,
}

impl CinemetaClient {
    pub fn new(http_client: reqwest::Client) -> Self {
        Self { http_client }
    }

    pub async fn lookup_movie(&self, imdb_id: &str) -> Result<CinemetaResult, CinemetaError> {
        let url = format!("{BASE_URL}/meta/movie/{imdb_id}.json");
        let resp: CinemetaResponse = self
            .http_client
            .get(&url)
            .send()
            .await?
            .error_for_status()?
            .json()
            .await?;

        Ok(CinemetaResult {
            title: resp.meta.name,
            year: resp.meta.year.as_deref().and_then(|y| y.parse().ok()),
            imdb_id: resp.meta.imdb_id.unwrap_or_else(|| imdb_id.to_string()),
        })
    }

    pub async fn lookup_series(&self, imdb_id: &str) -> Result<CinemetaResult, CinemetaError> {
        let url = format!("{BASE_URL}/meta/series/{imdb_id}.json");
        let resp: CinemetaResponse = self
            .http_client
            .get(&url)
            .send()
            .await?
            .error_for_status()?
            .json()
            .await?;

        Ok(CinemetaResult {
            title: resp.meta.name,
            year: resp.meta.year.as_deref().and_then(|y| y.parse().ok()),
            imdb_id: resp.meta.imdb_id.unwrap_or_else(|| imdb_id.to_string()),
        })
    }
}

#[derive(Debug, thiserror::Error)]
pub enum CinemetaError {
    #[error("HTTP request failed: {0}")]
    Http(#[from] reqwest::Error),
}

// Response types

#[derive(Deserialize)]
struct CinemetaResponse {
    meta: CinemetaMeta,
}

#[derive(Deserialize)]
struct CinemetaMeta {
    name: String,
    #[serde(default)]
    year: Option<String>,
    #[serde(default)]
    imdb_id: Option<String>,
}
