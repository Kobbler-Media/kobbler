use async_trait::async_trait;
use serde::Deserialize;

use super::types::{
    SubtitleCapability, SubtitleCapabilitySet, SubtitleConfigField, SubtitleConfigFieldType,
    SubtitleProviderInfo,
};
use super::{
    EpisodeSubtitleQuery, MovieSubtitleQuery, SubtitleDownload, SubtitleProvider,
    SubtitleProviderError, SubtitleProviderFactory, SubtitleSearchResult,
};

// ============================================================================
// Provider Info
// ============================================================================

static CONFIG_FIELDS: &[SubtitleConfigField] = &[
    SubtitleConfigField {
        key: "api_key",
        label: "API Key",
        field_type: SubtitleConfigFieldType::Secret,
        required: true,
        description: Some("OpenSubtitles REST API key"),
        help_url: Some("https://www.opensubtitles.com/en/consumers"),
        default: None,
    },
    SubtitleConfigField {
        key: "username",
        label: "Username",
        field_type: SubtitleConfigFieldType::String,
        required: false,
        description: Some("Account username for higher download quotas"),
        help_url: None,
        default: None,
    },
    SubtitleConfigField {
        key: "password",
        label: "Password",
        field_type: SubtitleConfigFieldType::Secret,
        required: false,
        description: Some("Account password (required if username is set)"),
        help_url: None,
        default: None,
    },
];

static CAPABILITIES: &[SubtitleCapability] = &[
    SubtitleCapability::MovieSearch,
    SubtitleCapability::EpisodeSearch,
    SubtitleCapability::HashSearch,
    SubtitleCapability::ForcedFilter,
    SubtitleCapability::HearingImpaired,
];

pub static INFO: SubtitleProviderInfo = SubtitleProviderInfo {
    id: "opensubtitles",
    display_name: "OpenSubtitles",
    description: "OpenSubtitles.com — large community-driven subtitle database with REST API.",
    website: "https://www.opensubtitles.com",
    capabilities: SubtitleCapabilitySet::new(CAPABILITIES),
    config_fields: CONFIG_FIELDS,
};

const BASE_URL: &str = "https://api.opensubtitles.com/api/v1";

// ============================================================================
// Factory
// ============================================================================

pub struct OpenSubtitlesFactory;

impl SubtitleProviderFactory for OpenSubtitlesFactory {
    fn info(&self) -> &'static SubtitleProviderInfo {
        &INFO
    }

    fn create(
        &self,
        client: reqwest::Client,
        config: &serde_json::Value,
    ) -> Box<dyn SubtitleProvider> {
        let api_key = config
            .get("api_key")
            .and_then(|v| v.as_str())
            .map(String::from)
            .unwrap_or_default();

        let username = config
            .get("username")
            .and_then(|v| v.as_str())
            .map(String::from);

        let password = config
            .get("password")
            .and_then(|v| v.as_str())
            .map(String::from);

        Box::new(OpenSubtitlesProvider {
            client,
            api_key,
            username,
            password,
        })
    }
}

// ============================================================================
// Provider
// ============================================================================

struct OpenSubtitlesProvider {
    client: reqwest::Client,
    api_key: String,
    username: Option<String>,
    password: Option<String>,
}

impl OpenSubtitlesProvider {
    fn require_key(&self) -> Result<&str, SubtitleProviderError> {
        if self.api_key.is_empty() {
            Err(SubtitleProviderError::ApiKeyRequired)
        } else {
            Ok(&self.api_key)
        }
    }

    fn get(&self, url: &str, key: &str) -> reqwest::RequestBuilder {
        self.client.get(url).header("Api-Key", key)
    }

    fn post(&self, url: &str, key: &str) -> reqwest::RequestBuilder {
        self.client.post(url).header("Api-Key", key)
    }

    /// Login to get a bearer token for higher download quotas.
    async fn login(&self) -> Result<Option<String>, SubtitleProviderError> {
        let (Some(username), Some(password)) = (&self.username, &self.password) else {
            return Ok(None);
        };

        if username.is_empty() || password.is_empty() {
            return Ok(None);
        }

        let key = self.require_key()?;
        let resp = self
            .post(&format!("{BASE_URL}/login"), key)
            .json(&serde_json::json!({
                "username": username,
                "password": password,
            }))
            .send()
            .await?;

        if resp.status() == reqwest::StatusCode::UNAUTHORIZED {
            tracing::warn!("OpenSubtitles login failed: invalid credentials");
            return Ok(None);
        }

        let body: OsLoginResponse = resp
            .error_for_status()
            .map_err(SubtitleProviderError::Http)?
            .json()
            .await?;

        Ok(Some(body.token))
    }

    async fn search(
        &self,
        params: &[(&str, &str)],
    ) -> Result<Vec<SubtitleSearchResult>, SubtitleProviderError> {
        let key = self.require_key()?;

        let url = reqwest::Url::parse_with_params(&format!("{BASE_URL}/subtitles"), params)
            .map_err(|e| SubtitleProviderError::Other(format!("URL parse error: {e}")))?;

        let resp = self.get(url.as_str(), key).send().await?;

        if resp.status() == reqwest::StatusCode::TOO_MANY_REQUESTS {
            return Err(SubtitleProviderError::RateLimited);
        }

        let body: OsSearchResponse = resp
            .error_for_status()
            .map_err(SubtitleProviderError::Http)?
            .json()
            .await?;

        Ok(body
            .data
            .into_iter()
            .flat_map(|item| {
                let attrs = item.attributes;
                let language = attrs.language;
                let is_forced = attrs.foreign_parts_only;
                let is_hi = attrs.hearing_impaired;
                let rating = attrs.ratings.filter(|&r| r > 0.0);
                let dl_count = attrs.download_count;

                attrs
                    .files
                    .into_iter()
                    .map(move |file| SubtitleSearchResult {
                        provider: "opensubtitles".into(),
                        id: file.file_id.to_string(),
                        language: language.clone(),
                        format: file
                            .file_name
                            .rsplit('.')
                            .next()
                            .unwrap_or("srt")
                            .to_lowercase(),
                        is_forced,
                        is_hearing_impaired: is_hi,
                        rating,
                        download_count: Some(dl_count),
                    })
            })
            .collect())
    }
}

#[async_trait]
impl SubtitleProvider for OpenSubtitlesProvider {
    fn info(&self) -> &'static SubtitleProviderInfo {
        &INFO
    }

    async fn search_movie(
        &self,
        query: &MovieSubtitleQuery,
    ) -> Result<Vec<SubtitleSearchResult>, SubtitleProviderError> {
        let mut params = Vec::new();
        params.push(("type".to_string(), "movie".to_string()));

        if let Some(ref tmdb_id) = query.tmdb_id {
            params.push(("tmdb_id".to_string(), tmdb_id.clone()));
        } else if let Some(ref imdb_id) = query.imdb_id {
            // OpenSubtitles wants numeric IMDB ID (without "tt" prefix)
            let numeric = imdb_id.strip_prefix("tt").unwrap_or(imdb_id);
            params.push(("imdb_id".to_string(), numeric.to_string()));
        } else {
            params.push(("query".to_string(), query.title.clone()));
            if let Some(year) = query.year {
                params.push(("year".to_string(), year.to_string()));
            }
        }

        if !query.languages.is_empty() {
            params.push(("languages".to_string(), query.languages.join(",")));
        }

        if query.forced_only {
            params.push(("foreign_parts_only".to_string(), "only".to_string()));
        }

        let str_params: Vec<(&str, &str)> = params
            .iter()
            .map(|(k, v)| (k.as_str(), v.as_str()))
            .collect();
        self.search(&str_params).await
    }

    async fn search_episode(
        &self,
        query: &EpisodeSubtitleQuery,
    ) -> Result<Vec<SubtitleSearchResult>, SubtitleProviderError> {
        let mut params = Vec::new();
        params.push(("type".to_string(), "episode".to_string()));

        if let Some(ref tmdb_id) = query.show_tmdb_id {
            params.push(("parent_tmdb_id".to_string(), tmdb_id.clone()));
        } else if let Some(ref imdb_id) = query.show_imdb_id {
            let numeric = imdb_id.strip_prefix("tt").unwrap_or(imdb_id);
            params.push(("parent_imdb_id".to_string(), numeric.to_string()));
        } else {
            params.push(("query".to_string(), query.show_title.clone()));
        }

        params.push(("season_number".to_string(), query.season.to_string()));
        params.push(("episode_number".to_string(), query.episode.to_string()));

        if !query.languages.is_empty() {
            params.push(("languages".to_string(), query.languages.join(",")));
        }

        if query.forced_only {
            params.push(("foreign_parts_only".to_string(), "only".to_string()));
        }

        let str_params: Vec<(&str, &str)> = params
            .iter()
            .map(|(k, v)| (k.as_str(), v.as_str()))
            .collect();
        self.search(&str_params).await
    }

    async fn download(&self, file_id: &str) -> Result<SubtitleDownload, SubtitleProviderError> {
        let key = self.require_key()?;

        let file_id_num: i64 = file_id
            .parse()
            .map_err(|_| SubtitleProviderError::Parse("Invalid file_id".into()))?;

        // Try to login for higher quota; proceed without if login fails
        let token = self.login().await.unwrap_or(None);

        let mut req = self
            .post(&format!("{BASE_URL}/download"), key)
            .json(&serde_json::json!({ "file_id": file_id_num }));

        if let Some(ref token) = token {
            req = req.bearer_auth(token);
        }

        let resp = req.send().await?;

        if resp.status() == reqwest::StatusCode::TOO_MANY_REQUESTS {
            return Err(SubtitleProviderError::DownloadLimitReached);
        }

        let body: OsDownloadResponse = resp
            .error_for_status()
            .map_err(SubtitleProviderError::Http)?
            .json()
            .await?;

        let content = self
            .client
            .get(&body.link)
            .send()
            .await?
            .error_for_status()
            .map_err(SubtitleProviderError::Http)?
            .bytes()
            .await?
            .to_vec();

        let format = body
            .file_name
            .rsplit('.')
            .next()
            .unwrap_or("srt")
            .to_lowercase();

        Ok(SubtitleDownload {
            content,
            format,
            filename: body.file_name,
        })
    }
}

// ============================================================================
// OpenSubtitles API Response Types
// ============================================================================

#[derive(Deserialize)]
struct OsLoginResponse {
    token: String,
}

#[derive(Deserialize)]
struct OsSearchResponse {
    #[serde(default)]
    data: Vec<OsSearchItem>,
}

#[derive(Deserialize)]
struct OsSearchItem {
    attributes: OsSearchAttributes,
}

#[derive(Deserialize)]
struct OsSearchAttributes {
    language: String,
    #[serde(default)]
    foreign_parts_only: bool,
    #[serde(default)]
    hearing_impaired: bool,
    ratings: Option<f64>,
    #[serde(default)]
    download_count: i64,
    #[serde(default)]
    files: Vec<OsFile>,
}

#[derive(Clone, Deserialize)]
struct OsFile {
    file_id: i64,
    file_name: String,
}

#[derive(Deserialize)]
struct OsDownloadResponse {
    link: String,
    file_name: String,
}
