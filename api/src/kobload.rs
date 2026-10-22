pub use nats_jobs::{
    ListWatchMatchesResponse, ListWatchPatternsResponse, WatchMatchResponse,
    WatchPatternDetailResponse, WatchPatternResponse,
};
use nats_jobs::{MediaWant, SearchRequest};
use reqwest::StatusCode;
use serde::{Deserialize, Serialize};
use tracing::info;

#[derive(Clone)]
pub struct KobloadClient {
    base_url: String,
    api_key: String,
    client: reqwest::Client,
}

impl KobloadClient {
    pub fn new(base_url: String, api_key: String) -> Self {
        Self {
            base_url: base_url.trim_end_matches('/').to_string(),
            api_key,
            client: crate::http::new_client(),
        }
    }

    /// Create a client with a shared HTTP client for connection pooling.
    pub fn new_with_client(base_url: String, api_key: String, client: reqwest::Client) -> Self {
        Self {
            base_url: base_url.trim_end_matches('/').to_string(),
            api_key,
            client,
        }
    }

    pub async fn search(
        &self,
        query: &str,
        protocol: Option<&str>,
        media_want: Option<MediaWant>,
    ) -> Result<Vec<serde_json::Value>, KobloadError> {
        self.search_scoped(query, protocol, media_want, None).await
    }

    /// Search with an explicit media type key (e.g. "ebooks", "audiobooks").
    ///
    /// Kobload prefers an explicit `media_type` over deriving one from
    /// `media_want`, so this is how a caller keeps (or deliberately drops)
    /// category filtering on a free-text search.
    pub async fn search_scoped(
        &self,
        query: &str,
        protocol: Option<&str>,
        media_want: Option<MediaWant>,
        media_type: Option<String>,
    ) -> Result<Vec<serde_json::Value>, KobloadError> {
        let url = format!("{}/api/v1/search", self.base_url);
        let payload = SearchRequest {
            query: query.to_string(),
            protocol: protocol.map(|value| value.to_string()),
            media_want,
            media_type,
            ..Default::default()
        };
        let resp = self
            .client
            .post(url)
            .bearer_auth(&self.api_key)
            .json(&payload)
            .send()
            .await
            .map_err(KobloadError::Request)?;

        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            return Err(KobloadError::Status(status, body));
        }

        let payload: SearchResponse = resp.json().await.map_err(KobloadError::Request)?;
        Ok(payload.results)
    }

    pub async fn create_download(
        &self,
        payload: &CreateDownloadRequest,
    ) -> Result<CreateDownloadResponse, KobloadError> {
        let url = format!("{}/api/v1/downloads", self.base_url);
        info!(
            url = %url,
            name = %payload.name,
            protocol = %payload.protocol,
            "Creating Kobload download"
        );
        let resp = self
            .client
            .post(&url)
            .bearer_auth(&self.api_key)
            .json(payload)
            .send()
            .await
            .map_err(KobloadError::Request)?;

        let status = resp.status();
        if !status.is_success() {
            let body = resp.text().await.unwrap_or_default();
            return Err(KobloadError::Status(status, body));
        }

        resp.json().await.map_err(KobloadError::Request)
    }

    /// Fetch quality profiles from Kobload.
    pub async fn list_quality_profiles(&self) -> Result<Vec<serde_json::Value>, KobloadError> {
        let url = format!("{}/api/v1/quality-profiles", self.base_url);
        let resp = self
            .client
            .get(url)
            .bearer_auth(&self.api_key)
            .send()
            .await
            .map_err(KobloadError::Request)?;

        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            return Err(KobloadError::Status(status, body));
        }

        resp.json().await.map_err(KobloadError::Request)
    }

    /// Check connectivity to Kobload
    pub async fn health_check(&self) -> Result<(), KobloadError> {
        let url = format!("{}/health", self.base_url);
        let resp = self
            .client
            .get(url)
            .bearer_auth(&self.api_key)
            .send()
            .await
            .map_err(KobloadError::Request)?;

        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            return Err(KobloadError::Status(status, body));
        }

        Ok(())
    }

    /// Download a specific file from a completed download
    pub async fn download_file(
        &self,
        download_id: uuid::Uuid,
        path: &str,
    ) -> Result<Vec<u8>, KobloadError> {
        let url = format!(
            "{}/api/v1/downloads/{}/files/{}",
            self.base_url,
            download_id,
            urlencoding::encode(path)
        );
        let resp = self
            .client
            .get(&url)
            .bearer_auth(&self.api_key)
            .send()
            .await
            .map_err(KobloadError::Request)?;

        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            return Err(KobloadError::Status(status, body));
        }

        let bytes = resp.bytes().await.map_err(KobloadError::Request)?;
        Ok(bytes.to_vec())
    }

    /// Confirm that all files have been imported from a download
    /// This signals to Kobload that it can safely delete the files from the download client
    pub async fn confirm_import(&self, download_id: uuid::Uuid) -> Result<(), KobloadError> {
        let url = format!(
            "{}/api/v1/downloads/{}/imported",
            self.base_url, download_id
        );
        let resp = self
            .client
            .post(&url)
            .bearer_auth(&self.api_key)
            .send()
            .await
            .map_err(KobloadError::Request)?;

        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            return Err(KobloadError::Status(status, body));
        }

        Ok(())
    }

    /// Sync watch patterns to Kobload
    pub async fn sync_watch_patterns(
        &self,
        request: &nats_jobs::WatchSyncRequest,
    ) -> Result<nats_jobs::WatchSyncResponse, KobloadError> {
        let url = format!("{}/api/v1/watch/sync", self.base_url);
        let resp = self
            .client
            .post(&url)
            .bearer_auth(&self.api_key)
            .json(request)
            .send()
            .await
            .map_err(KobloadError::Request)?;

        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            return Err(KobloadError::Status(status, body));
        }

        resp.json().await.map_err(KobloadError::Request)
    }

    /// Create a single watch pattern
    pub async fn create_watch_pattern(
        &self,
        source_system: &str,
        item: &nats_jobs::WatchSyncItem,
    ) -> Result<(), KobloadError> {
        let url = format!("{}/api/v1/watch", self.base_url);
        let mut payload = serde_json::json!({
            "source_system": source_system,
            "source_id": item.source_id,
            "callback_url": item.callback_url,
            "patterns": item.patterns,
            "match_mode": item.match_mode,
            "min_score": item.min_score,
            "auto_download": item.auto_download,
            "metadata": item.metadata,
            "enabled": item.enabled,
            "upgrade_allowed": item.upgrade_allowed,
        });

        // Include match_targets if present
        if !item.match_targets.is_empty() {
            payload["match_targets"] =
                serde_json::to_value(&item.match_targets).unwrap_or_default();
        }
        if let Some(ref mw) = item.media_want {
            payload["media_want"] = serde_json::to_value(mw).unwrap_or_default();
        }
        if let Some(ref snb) = item.search_not_before {
            payload["search_not_before"] = serde_json::json!(snb);
        }

        let resp = self
            .client
            .post(&url)
            .bearer_auth(&self.api_key)
            .json(&payload)
            .send()
            .await
            .map_err(KobloadError::Request)?;

        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            return Err(KobloadError::Status(status, body));
        }

        Ok(())
    }

    /// Create a "want" request - combines watch pattern creation with immediate search.
    ///
    /// This creates a watch pattern for ongoing monitoring AND does an immediate search.
    /// If a match is found, a download is started automatically.
    /// The watch pattern remains active until `confirm_want` is called.
    pub async fn want_content(
        &self,
        request: &nats_jobs::WantRequest,
    ) -> Result<nats_jobs::WantResponse, KobloadError> {
        let url = format!("{}/api/v1/want", self.base_url);
        info!(
            source_system = %request.source_system,
            source_id = %request.source_id,
            patterns = ?request.patterns,
            "Creating want request to Kobload"
        );

        let resp = self
            .client
            .post(&url)
            .bearer_auth(&self.api_key)
            .json(request)
            .send()
            .await
            .map_err(KobloadError::Request)?;

        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            return Err(KobloadError::Status(status, body));
        }

        resp.json().await.map_err(KobloadError::Request)
    }

    /// Confirm successful ingest of a want.
    ///
    /// Call this after successfully ingesting the downloaded content.
    /// When `upgrade_allowed=false`, deletes the watch pattern (one-and-done).
    /// When `upgrade_allowed=true`, opens an upgrade window and returns 200 with
    /// status info — the pattern stays active so higher-quality matches can replace
    /// the initial download.
    /// For TvSeason patterns, returns `Partial` when not all episodes are covered yet.
    pub async fn confirm_want(
        &self,
        source_system: &str,
        source_id: &str,
    ) -> Result<ConfirmWantStatus, KobloadError> {
        let url = format!(
            "{}/api/v1/want/{}/{}/confirm",
            self.base_url,
            urlencoding::encode(source_system),
            urlencoding::encode(source_id)
        );
        info!(
            source_system = %source_system,
            source_id = %source_id,
            "Confirming want to Kobload"
        );

        let resp = self
            .client
            .post(&url)
            .bearer_auth(&self.api_key)
            .send()
            .await
            .map_err(KobloadError::Request)?;

        let status_code = resp.status();

        // NOT_FOUND (404) means pattern already deleted
        if status_code == StatusCode::NOT_FOUND {
            return Ok(ConfirmWantStatus::Completed);
        }

        if !status_code.is_success() {
            let body = resp.text().await.unwrap_or_default();
            return Err(KobloadError::Status(status_code, body));
        }

        // NO_CONTENT (204) = pattern deleted (completed, no upgrades)
        if status_code == StatusCode::NO_CONTENT {
            return Ok(ConfirmWantStatus::Completed);
        }

        // 200 OK = has a JSON body with status
        let body: ConfirmWantBody = resp.json().await.unwrap_or(ConfirmWantBody {
            status: "completed".to_string(),
        });

        if body.status == "partial" {
            Ok(ConfirmWantStatus::Partial)
        } else {
            Ok(ConfirmWantStatus::Completed)
        }
    }

    /// Reject a watch match — tells kobload to cancel the associated download
    /// and re-open the pattern for new matches.
    pub async fn reject_match(
        &self,
        match_id: uuid::Uuid,
        reason: Option<&str>,
    ) -> Result<(), KobloadError> {
        let url = format!("{}/api/v1/watch/matches/{}/reject", self.base_url, match_id);
        let body = serde_json::json!({ "reason": reason });
        info!(match_id = %match_id, reason = ?reason, "Rejecting watch match on Kobload");

        let resp = self
            .client
            .post(&url)
            .bearer_auth(&self.api_key)
            .json(&body)
            .send()
            .await
            .map_err(KobloadError::Request)?;

        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            return Err(KobloadError::Status(status, body));
        }

        Ok(())
    }

    /// List all watch patterns, optionally filtered by source_system.
    /// Paginates through all pages automatically.
    pub async fn list_watch_patterns(
        &self,
        source_system: Option<&str>,
    ) -> Result<Vec<WatchPatternResponse>, KobloadError> {
        let mut all_items = Vec::new();
        let mut cursor: Option<String> = None;

        loop {
            let mut url = format!("{}/api/v1/watch?limit=200", self.base_url);
            if let Some(ss) = source_system {
                url.push_str(&format!("&source_system={}", urlencoding::encode(ss)));
            }
            if let Some(ref c) = cursor {
                url.push_str(&format!("&cursor={}", urlencoding::encode(c)));
            }

            let resp = self
                .client
                .get(&url)
                .bearer_auth(&self.api_key)
                .send()
                .await
                .map_err(KobloadError::Request)?;

            if !resp.status().is_success() {
                let status = resp.status();
                let body = resp.text().await.unwrap_or_default();
                return Err(KobloadError::Status(status, body));
            }

            let page: ListWatchPatternsResponse =
                resp.json().await.map_err(KobloadError::Request)?;
            all_items.extend(page.items);

            match page.next_cursor {
                Some(c) => cursor = Some(c),
                None => break,
            }
        }

        Ok(all_items)
    }

    /// List all watch matches, optionally filtered by pattern_id.
    /// Paginates through all pages automatically.
    pub async fn list_watch_matches(
        &self,
        pattern_id: Option<uuid::Uuid>,
    ) -> Result<Vec<WatchMatchResponse>, KobloadError> {
        let mut all_items = Vec::new();
        let mut cursor: Option<String> = None;

        loop {
            let mut url = format!("{}/api/v1/watch-matches?limit=200", self.base_url);
            if let Some(pid) = pattern_id {
                url.push_str(&format!("&watch_pattern_id={}", pid));
            }
            if let Some(ref c) = cursor {
                url.push_str(&format!("&cursor={}", urlencoding::encode(c)));
            }

            let resp = self
                .client
                .get(&url)
                .bearer_auth(&self.api_key)
                .send()
                .await
                .map_err(KobloadError::Request)?;

            if !resp.status().is_success() {
                let status = resp.status();
                let body = resp.text().await.unwrap_or_default();
                return Err(KobloadError::Status(status, body));
            }

            let page: ListWatchMatchesResponse =
                resp.json().await.map_err(KobloadError::Request)?;
            all_items.extend(page.items);

            match page.next_cursor {
                Some(c) => cursor = Some(c),
                None => break,
            }
        }

        Ok(all_items)
    }

    /// Get pattern detail with status and matches from Kobload.
    pub async fn get_pattern_detail(
        &self,
        source_system: &str,
        source_id: &str,
    ) -> Result<WatchPatternDetailResponse, KobloadError> {
        let url = format!(
            "{}/api/v1/watch/{}/{}/detail",
            self.base_url,
            urlencoding::encode(source_system),
            urlencoding::encode(source_id)
        );
        let resp = self
            .client
            .get(&url)
            .bearer_auth(&self.api_key)
            .send()
            .await
            .map_err(KobloadError::Request)?;

        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            return Err(KobloadError::Status(status, body));
        }

        resp.json().await.map_err(KobloadError::Request)
    }

    /// Trigger an immediate search for a pattern.
    pub async fn search_pattern_now(
        &self,
        source_system: &str,
        source_id: &str,
    ) -> Result<SearchNowResponse, KobloadError> {
        let url = format!(
            "{}/api/v1/watch/{}/{}/search-now",
            self.base_url,
            urlencoding::encode(source_system),
            urlencoding::encode(source_id)
        );
        let resp = self
            .client
            .post(&url)
            .bearer_auth(&self.api_key)
            .send()
            .await
            .map_err(KobloadError::Request)?;

        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            return Err(KobloadError::Status(status, body));
        }

        resp.json().await.map_err(KobloadError::Request)
    }

    /// Grab the held match for a pattern.
    pub async fn grab_held_match(
        &self,
        source_system: &str,
        source_id: &str,
    ) -> Result<GrabResponse, KobloadError> {
        let url = format!(
            "{}/api/v1/watch/{}/{}/grab-held",
            self.base_url,
            urlencoding::encode(source_system),
            urlencoding::encode(source_id)
        );
        let resp = self
            .client
            .post(&url)
            .bearer_auth(&self.api_key)
            .send()
            .await
            .map_err(KobloadError::Request)?;

        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            return Err(KobloadError::Status(status, body));
        }

        resp.json().await.map_err(KobloadError::Request)
    }

    /// Search for streams matching the given criteria.
    pub async fn stream_search(
        &self,
        request: &nats_jobs::StreamSearchRequest,
    ) -> Result<nats_jobs::StreamSearchResponse, KobloadError> {
        let value = self.stream_search_value(request).await?;
        serde_json::from_value(value).map_err(|e| {
            KobloadError::Status(
                StatusCode::BAD_GATEWAY,
                format!("undecodable search response: {e}"),
            )
        })
    }

    /// The search response as kobload sent it, for passing on to clients: option
    /// fields kobbler has no type for yet still reach them.
    pub async fn stream_search_value(
        &self,
        request: &nats_jobs::StreamSearchRequest,
    ) -> Result<serde_json::Value, KobloadError> {
        let url = format!("{}/api/v1/stream/search", self.base_url);
        let resp = self
            .client
            .post(&url)
            .bearer_auth(&self.api_key)
            .json(request)
            .send()
            .await
            .map_err(KobloadError::Request)?;

        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            return Err(KobloadError::Status(status, body));
        }

        resp.json().await.map_err(KobloadError::Request)
    }

    /// Resolve a stream to get playback URLs.
    pub async fn stream_resolve(
        &self,
        request: &nats_jobs::StreamResolveRequest,
    ) -> Result<nats_jobs::StreamResolveResponse, KobloadError> {
        let url = format!("{}/api/v1/stream/resolve", self.base_url);
        let resp = self
            .client
            .post(&url)
            .bearer_auth(&self.api_key)
            .json(request)
            .send()
            .await
            .map_err(KobloadError::Request)?;

        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            return Err(KobloadError::Status(status, body));
        }

        resp.json().await.map_err(KobloadError::Request)
    }

    pub async fn stream_outcome(
        &self,
        report: &nats_jobs::StreamOutcomeReport,
    ) -> Result<(), KobloadError> {
        let url = format!("{}/api/v1/stream/outcome", self.base_url);
        let resp = self
            .client
            .post(&url)
            .bearer_auth(&self.api_key)
            .json(report)
            .send()
            .await
            .map_err(KobloadError::Request)?;

        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            return Err(KobloadError::Status(status, body));
        }
        Ok(())
    }

    /// Poll the status of an uncached torrent being downloaded on a debrid provider.
    pub async fn stream_resolve_status(
        &self,
        provider_id: &str,
        torrent_id: &str,
    ) -> Result<StreamResolveStatusResponse, KobloadError> {
        let url = format!(
            "{}/api/v1/stream/resolve/status?provider_id={}&torrent_id={}",
            self.base_url,
            urlencoding::encode(provider_id),
            urlencoding::encode(torrent_id),
        );
        let resp = self
            .client
            .get(&url)
            .bearer_auth(&self.api_key)
            .send()
            .await
            .map_err(KobloadError::Request)?;

        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            return Err(KobloadError::Status(status, body));
        }

        resp.json().await.map_err(KobloadError::Request)
    }

    /// Reject the active download for an entity by looking up its watch pattern and matches.
    ///
    /// Finds the kobbler watch pattern for `entity_id`, gets its active (non-rejected) match
    /// with a download, and rejects it. Returns the rejected match_id, or None if no active
    /// match was found.
    pub async fn reject_active_download(
        &self,
        entity_id: &str,
        reason: Option<&str>,
    ) -> Result<Option<uuid::Uuid>, KobloadError> {
        // Find the pattern for this entity
        let patterns = self.list_watch_patterns(Some("kobbler")).await?;
        let pattern = patterns.iter().find(|p| p.source_id == entity_id);
        let pattern = match pattern {
            Some(p) => p,
            None => return Ok(None),
        };

        // Get matches for this pattern
        let matches = self.list_watch_matches(Some(pattern.id)).await?;

        // Find the active non-rejected match with a download
        let active_match = matches
            .iter()
            .find(|m| !m.rejected && m.download_id.is_some() && m.action != "rejected");

        let active_match = match active_match {
            Some(m) => m,
            None => return Ok(None),
        };

        let match_id = active_match.id;
        self.reject_match(match_id, reason).await?;
        Ok(Some(match_id))
    }
}

#[derive(Debug)]
pub enum KobloadError {
    Request(reqwest::Error),
    Status(StatusCode, String),
}

impl std::fmt::Display for KobloadError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            KobloadError::Request(e) => write!(f, "request failed: {e}"),
            KobloadError::Status(status, body) => {
                if body.is_empty() {
                    write!(f, "HTTP {status}")
                } else {
                    write!(f, "HTTP {status}: {body}")
                }
            }
        }
    }
}

/// Status returned from confirm_want.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConfirmWantStatus {
    /// All episodes/content acquired — pattern deleted or upgrade window started.
    Completed,
    /// TvSeason partially fulfilled — some episodes still needed, pattern stays active.
    Partial,
}

#[derive(Debug, Deserialize)]
struct ConfirmWantBody {
    status: String,
}

#[derive(Debug, Deserialize)]
struct SearchResponse {
    results: Vec<serde_json::Value>,
}

/// Response from search-now endpoint.
#[derive(Debug, Deserialize, Serialize)]
pub struct SearchNowResponse {
    pub found: bool,
}

/// Response from grab-held endpoint.
#[derive(Debug, Deserialize, Serialize)]
pub struct GrabResponse {
    pub download_id: Option<uuid::Uuid>,
}

/// Response from the stream resolve status endpoint.
#[derive(Debug, Deserialize, Serialize)]
pub struct StreamResolveStatusResponse {
    pub status: String,
    #[serde(default)]
    pub stream_url: Option<String>,
    #[serde(default)]
    pub filename: Option<String>,
    #[serde(default)]
    pub filesize: Option<i64>,
    #[serde(default)]
    pub progress: Option<f32>,
    #[serde(default)]
    pub message: Option<String>,
}

// Re-export nats_jobs types used by callers
pub use nats_jobs::{
    CreateDownloadRequest, CreateDownloadResponse, MatchTarget, ProcessedReleaseDto, WantRequest,
    WantResponse, WantStatus, WatchSyncItem, WatchSyncRequest, WatchSyncResponse,
};
