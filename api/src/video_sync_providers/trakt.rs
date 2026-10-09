//! Trakt sync provider implementation.
//!
//! Supports OAuth device code flow, watch history sync, and marking
//! movies/episodes as watched/unwatched via the Trakt API v2.

use super::types::{
    DeviceCodeResponse, PollResult, VideoSyncError, VideoSyncProviderInfo, VideoSyncResult,
    WatchHistoryItem,
};
use super::{VideoSyncProvider, VideoSyncProviderFactory};
use async_trait::async_trait;
use serde::Deserialize;
use serde_json::{Value, json};

// ============================================================================
// Constants
// ============================================================================

const TRAKT_API_URL: &str = "https://api.trakt.tv";

pub static INFO: VideoSyncProviderInfo = VideoSyncProviderInfo {
    id: "trakt",
    display_name: "Trakt",
    description: "Track movies and TV shows you watch. Automatically scrobble what you're watching.",
    website: "https://trakt.tv",
    uses_oauth: true,
};

// ============================================================================
// Factory
// ============================================================================

pub struct TraktSyncFactory {
    base_url: String,
}

impl TraktSyncFactory {
    pub fn new(base_url: Option<String>) -> Self {
        Self {
            base_url: base_url.unwrap_or_else(|| TRAKT_API_URL.to_string()),
        }
    }
}

impl VideoSyncProviderFactory for TraktSyncFactory {
    fn info(&self) -> &'static VideoSyncProviderInfo {
        &INFO
    }

    fn create(
        &self,
        client: reqwest::Client,
        config: &Value,
    ) -> Option<Box<dyn VideoSyncProvider>> {
        let access_token = config.get("access_token")?.as_str()?.to_string();
        let client_id = config.get("client_id")?.as_str()?.to_string();

        Some(Box::new(TraktProvider {
            client,
            base_url: self.base_url.clone(),
            access_token,
            client_id,
        }))
    }
}

// ============================================================================
// Provider Implementation
// ============================================================================

struct TraktProvider {
    client: reqwest::Client,
    base_url: String,
    access_token: String,
    client_id: String,
}

impl TraktProvider {
    fn url(&self, path: &str) -> String {
        format!("{}{}", self.base_url, path)
    }

    fn auth_headers(&self) -> Vec<(&'static str, String)> {
        vec![
            ("Authorization", format!("Bearer {}", self.access_token)),
            ("trakt-api-key", self.client_id.clone()),
            ("trakt-api-version", "2".to_string()),
            ("Content-Type", "application/json".to_string()),
        ]
    }

    async fn request(
        &self,
        method: reqwest::Method,
        path: &str,
        body: Option<Value>,
    ) -> Result<Value, VideoSyncError> {
        let cooldown = crate::http::provider_cooldown();
        cooldown
            .wait_if_needed(crate::http::provider_ids::TRAKT)
            .await;

        let mut req = self.client.request(method, self.url(path));

        for (key, val) in self.auth_headers() {
            req = req.header(key, val);
        }

        if let Some(body) = body {
            req = req.json(&body);
        }

        let resp = req.send().await?;
        let status = resp.status();

        if status == reqwest::StatusCode::UNAUTHORIZED {
            return Err(VideoSyncError::TokenExpired);
        }
        if status == reqwest::StatusCode::TOO_MANY_REQUESTS {
            cooldown.set_cooldown(
                crate::http::provider_ids::TRAKT,
                crate::http::parse_retry_after(resp.headers()),
            );
            return Err(VideoSyncError::RateLimited);
        }
        if status.is_success() {
            cooldown.reset(crate::http::provider_ids::TRAKT);
        }
        if !status.is_success() {
            let body = resp.text().await.unwrap_or_default();
            return Err(VideoSyncError::Other(format!(
                "Trakt API error {}: {}",
                status, body
            )));
        }

        // 204 No Content returns empty body
        if status == reqwest::StatusCode::NO_CONTENT {
            return Ok(json!({}));
        }

        resp.json()
            .await
            .map_err(|e| VideoSyncError::Parse(e.to_string()))
    }
}

#[async_trait]
impl VideoSyncProvider for TraktProvider {
    fn info(&self) -> &'static VideoSyncProviderInfo {
        &INFO
    }

    async fn mark_movie_watched(
        &self,
        tmdb_id: &str,
        watched_at: Option<&str>,
    ) -> Result<VideoSyncResult, VideoSyncError> {
        let movie = json!({
            "ids": { "tmdb": tmdb_id.parse::<i64>().unwrap_or(0) }
        });

        let mut movie_entry = movie;
        if let Some(at) = watched_at {
            movie_entry["watched_at"] = json!(at);
        }

        self.request(
            reqwest::Method::POST,
            "/sync/history",
            Some(json!({ "movies": [movie_entry] })),
        )
        .await?;

        Ok(VideoSyncResult::success().with_message("Marked as watched on Trakt"))
    }

    async fn mark_movie_unwatched(&self, tmdb_id: &str) -> Result<VideoSyncResult, VideoSyncError> {
        self.request(
            reqwest::Method::POST,
            "/sync/history/remove",
            Some(json!({
                "movies": [{
                    "ids": { "tmdb": tmdb_id.parse::<i64>().unwrap_or(0) }
                }]
            })),
        )
        .await?;

        Ok(VideoSyncResult::success().with_message("Removed from Trakt watch history"))
    }

    async fn mark_episode_watched(
        &self,
        show_tmdb_id: &str,
        season: u32,
        episode: u32,
        watched_at: Option<&str>,
    ) -> Result<VideoSyncResult, VideoSyncError> {
        let mut ep = json!({
            "ids": { "tmdb": show_tmdb_id.parse::<i64>().unwrap_or(0) },
            "seasons": [{
                "number": season,
                "episodes": [{ "number": episode }]
            }]
        });

        if let Some(at) = watched_at {
            ep["seasons"][0]["episodes"][0]["watched_at"] = json!(at);
        }

        self.request(
            reqwest::Method::POST,
            "/sync/history",
            Some(json!({ "shows": [ep] })),
        )
        .await?;

        Ok(VideoSyncResult::success().with_message("Episode marked as watched on Trakt"))
    }

    async fn mark_episode_unwatched(
        &self,
        show_tmdb_id: &str,
        season: u32,
        episode: u32,
    ) -> Result<VideoSyncResult, VideoSyncError> {
        self.request(
            reqwest::Method::POST,
            "/sync/history/remove",
            Some(json!({
                "shows": [{
                    "ids": { "tmdb": show_tmdb_id.parse::<i64>().unwrap_or(0) },
                    "seasons": [{
                        "number": season,
                        "episodes": [{ "number": episode }]
                    }]
                }]
            })),
        )
        .await?;

        Ok(VideoSyncResult::success().with_message("Episode removed from Trakt history"))
    }

    async fn mark_episodes_watched(
        &self,
        show_tmdb_id: &str,
        episodes: &[(u32, u32, Option<String>)],
    ) -> Result<VideoSyncResult, VideoSyncError> {
        if episodes.is_empty() {
            return Ok(VideoSyncResult::success().with_message("No episodes to sync"));
        }

        // Group episodes by season for compact Trakt payload
        let mut seasons: std::collections::BTreeMap<u32, Vec<Value>> =
            std::collections::BTreeMap::new();
        for (season, episode, watched_at) in episodes {
            let mut ep = json!({ "number": episode });
            if let Some(at) = watched_at {
                ep["watched_at"] = json!(at);
            }
            seasons.entry(*season).or_default().push(ep);
        }

        let seasons_arr: Vec<Value> = seasons
            .into_iter()
            .map(|(num, eps)| json!({ "number": num, "episodes": eps }))
            .collect();

        self.request(
            reqwest::Method::POST,
            "/sync/history",
            Some(json!({
                "shows": [{
                    "ids": { "tmdb": show_tmdb_id.parse::<i64>().unwrap_or(0) },
                    "seasons": seasons_arr
                }]
            })),
        )
        .await?;

        Ok(VideoSyncResult::success()
            .with_message("Episodes marked as watched on Trakt")
            .with_count(episodes.len()))
    }

    async fn mark_episodes_unwatched(
        &self,
        show_tmdb_id: &str,
        episodes: &[(u32, u32)],
    ) -> Result<VideoSyncResult, VideoSyncError> {
        if episodes.is_empty() {
            return Ok(VideoSyncResult::success().with_message("No episodes to sync"));
        }

        let mut seasons: std::collections::BTreeMap<u32, Vec<Value>> =
            std::collections::BTreeMap::new();
        for (season, episode) in episodes {
            seasons
                .entry(*season)
                .or_default()
                .push(json!({ "number": episode }));
        }

        let seasons_arr: Vec<Value> = seasons
            .into_iter()
            .map(|(num, eps)| json!({ "number": num, "episodes": eps }))
            .collect();

        self.request(
            reqwest::Method::POST,
            "/sync/history/remove",
            Some(json!({
                "shows": [{
                    "ids": { "tmdb": show_tmdb_id.parse::<i64>().unwrap_or(0) },
                    "seasons": seasons_arr
                }]
            })),
        )
        .await?;

        Ok(VideoSyncResult::success()
            .with_message("Episodes removed from Trakt history")
            .with_count(episodes.len()))
    }

    async fn import_movie_history(&self) -> Result<Vec<WatchHistoryItem>, VideoSyncError> {
        let data = self
            .request(reqwest::Method::GET, "/sync/watched/movies", None)
            .await?;

        let items = data
            .as_array()
            .unwrap_or(&Vec::new())
            .iter()
            .filter_map(|entry| {
                let movie = entry.get("movie")?;
                let ids = movie.get("ids")?;
                Some(WatchHistoryItem {
                    provider_id: ids
                        .get("slug")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string(),
                    title: movie
                        .get("title")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string(),
                    year: movie.get("year").and_then(|v| v.as_i64()).map(|y| y as i32),
                    media_type: "movie".to_string(),
                    tmdb_id: ids
                        .get("tmdb")
                        .and_then(|v| v.as_i64())
                        .map(|id| id.to_string()),
                    imdb_id: ids.get("imdb").and_then(|v| v.as_str()).map(String::from),
                    watched_at: entry
                        .get("last_watched_at")
                        .and_then(|v| v.as_str())
                        .map(String::from),
                    show_title: None,
                    season_number: None,
                    episode_number: None,
                })
            })
            .collect();

        Ok(items)
    }

    async fn import_show_history(&self) -> Result<Vec<WatchHistoryItem>, VideoSyncError> {
        let data = self
            .request(reqwest::Method::GET, "/sync/watched/shows", None)
            .await?;

        let mut items = Vec::new();

        for entry in data.as_array().unwrap_or(&Vec::new()) {
            let show = match entry.get("show") {
                Some(s) => s,
                None => continue,
            };
            let show_ids = match show.get("ids") {
                Some(ids) => ids,
                None => continue,
            };
            let show_title = show
                .get("title")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            let show_tmdb_id = show_ids
                .get("tmdb")
                .and_then(|v| v.as_i64())
                .map(|id| id.to_string());

            // Iterate seasons and episodes
            if let Some(seasons) = entry.get("seasons").and_then(|v| v.as_array()) {
                for season in seasons {
                    let season_num =
                        season.get("number").and_then(|v| v.as_u64()).unwrap_or(0) as u32;

                    if let Some(episodes) = season.get("episodes").and_then(|v| v.as_array()) {
                        for ep in episodes {
                            let ep_num =
                                ep.get("number").and_then(|v| v.as_u64()).unwrap_or(0) as u32;
                            let watched_at = ep
                                .get("last_watched_at")
                                .and_then(|v| v.as_str())
                                .map(String::from);

                            items.push(WatchHistoryItem {
                                provider_id: show_ids
                                    .get("slug")
                                    .and_then(|v| v.as_str())
                                    .unwrap_or("")
                                    .to_string(),
                                title: format!("{} S{:02}E{:02}", show_title, season_num, ep_num),
                                year: show.get("year").and_then(|v| v.as_i64()).map(|y| y as i32),
                                media_type: "episode".to_string(),
                                tmdb_id: show_tmdb_id.clone(),
                                imdb_id: show_ids
                                    .get("imdb")
                                    .and_then(|v| v.as_str())
                                    .map(String::from),
                                watched_at,
                                show_title: Some(show_title.clone()),
                                season_number: Some(season_num),
                                episode_number: Some(ep_num),
                            });
                        }
                    }
                }
            }
        }

        Ok(items)
    }
}

// ============================================================================
// OAuth Device Code Flow (standalone functions)
// ============================================================================

/// Start Trakt OAuth device code flow.
pub async fn start_device_auth(
    client: &reqwest::Client,
    base_url: Option<&str>,
    client_id: &str,
) -> Result<DeviceCodeResponse, VideoSyncError> {
    let url = format!("{}/oauth/device/code", base_url.unwrap_or(TRAKT_API_URL));

    let resp = client
        .post(&url)
        .header("Content-Type", "application/json")
        .json(&json!({ "client_id": client_id }))
        .send()
        .await?;

    if !resp.status().is_success() {
        return Err(VideoSyncError::Other(format!(
            "Device auth failed: {}",
            resp.status()
        )));
    }

    let body: TraktDeviceCodeResponse = resp
        .json()
        .await
        .map_err(|e| VideoSyncError::Parse(e.to_string()))?;

    Ok(DeviceCodeResponse {
        device_code: body.device_code,
        user_code: body.user_code,
        verification_url: body.verification_url,
        expires_in: body.expires_in,
        interval: body.interval,
    })
}

/// Poll for device token completion.
pub async fn poll_device_token(
    client: &reqwest::Client,
    base_url: Option<&str>,
    client_id: &str,
    client_secret: &str,
    device_code: &str,
) -> Result<PollResult, VideoSyncError> {
    let url = format!("{}/oauth/device/token", base_url.unwrap_or(TRAKT_API_URL));

    let resp = client
        .post(&url)
        .header("Content-Type", "application/json")
        .json(&json!({
            "code": device_code,
            "client_id": client_id,
            "client_secret": client_secret,
        }))
        .send()
        .await?;

    match resp.status().as_u16() {
        200 => {
            let body: TraktTokenResponse = resp
                .json()
                .await
                .map_err(|e| VideoSyncError::Parse(e.to_string()))?;

            Ok(PollResult::Authorized {
                access_token: body.access_token,
                refresh_token: Some(body.refresh_token),
            })
        }
        400 => Ok(PollResult::Pending),
        404 => Ok(PollResult::Denied),
        410 => Ok(PollResult::Expired),
        409 => Ok(PollResult::Denied),  // Already approved or denied
        418 => Ok(PollResult::Denied),  // Denied by user
        429 => Ok(PollResult::Pending), // Slow down (treat as pending)
        status => Err(VideoSyncError::Other(format!(
            "Unexpected poll response: {}",
            status
        ))),
    }
}

/// Refresh an expired access token.
pub async fn refresh_token(
    client: &reqwest::Client,
    base_url: Option<&str>,
    client_id: &str,
    client_secret: &str,
    refresh_token_value: &str,
) -> Result<(String, String), VideoSyncError> {
    let url = format!("{}/oauth/token", base_url.unwrap_or(TRAKT_API_URL));

    let resp = client
        .post(&url)
        .header("Content-Type", "application/json")
        .json(&json!({
            "refresh_token": refresh_token_value,
            "client_id": client_id,
            "client_secret": client_secret,
            "redirect_uri": "urn:ietf:wg:oauth:2.0:oob",
            "grant_type": "refresh_token",
        }))
        .send()
        .await?;

    if !resp.status().is_success() {
        return Err(VideoSyncError::TokenExpired);
    }

    let body: TraktTokenResponse = resp
        .json()
        .await
        .map_err(|e| VideoSyncError::Parse(e.to_string()))?;

    Ok((body.access_token, body.refresh_token))
}

/// Try to refresh stored Trakt tokens. Returns updated config on success.
///
/// Extracts credentials from the stored config, calls the Trakt refresh endpoint,
/// and persists the new tokens via `update_config_merge`. Returns the updated config
/// so callers can create a new provider instance with fresh tokens.
pub async fn try_refresh_stored_tokens(
    pool: &sqlx::PgPool,
    user_id: uuid::Uuid,
    config: &Value,
    base_url: Option<&str>,
) -> Result<Value, VideoSyncError> {
    let client_id = config
        .get("client_id")
        .and_then(|v| v.as_str())
        .ok_or_else(|| VideoSyncError::Other("Missing client_id in config".into()))?;
    let client_secret = config
        .get("client_secret")
        .and_then(|v| v.as_str())
        .ok_or_else(|| VideoSyncError::Other("Missing client_secret in config".into()))?;
    let refresh_tok = config
        .get("refresh_token")
        .and_then(|v| v.as_str())
        .ok_or_else(|| VideoSyncError::Other("Missing refresh_token in config".into()))?;

    let http = crate::http::new_client();
    let (new_access, new_refresh) =
        refresh_token(&http, base_url, client_id, client_secret, refresh_tok).await?;

    // Persist new tokens
    let mut updates = std::collections::HashMap::new();
    updates.insert("access_token".to_string(), json!(new_access));
    updates.insert("refresh_token".to_string(), json!(new_refresh));

    crate::repositories::user_sync_providers::update_config_merge(pool, user_id, "trakt", &updates)
        .await
        .map_err(|e| VideoSyncError::Other(format!("Failed to persist refreshed tokens: {e}")))?;

    // Build updated config for caller
    let mut new_config = config.clone();
    if let Some(obj) = new_config.as_object_mut() {
        obj.insert("access_token".to_string(), json!(new_access));
        obj.insert("refresh_token".to_string(), json!(new_refresh));
    }

    Ok(new_config)
}

// ============================================================================
// Internal Trakt API types
// ============================================================================

#[derive(Deserialize)]
struct TraktDeviceCodeResponse {
    device_code: String,
    user_code: String,
    verification_url: String,
    expires_in: u64,
    interval: u64,
}

#[derive(Deserialize)]
struct TraktTokenResponse {
    access_token: String,
    refresh_token: String,
}
