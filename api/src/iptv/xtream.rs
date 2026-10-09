//! Xtream Codes API client.

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use tracing::warn;

/// Deserialize any JSON value (string, number, null) into Option<String>.
/// Xtream providers are wildly inconsistent with types — same field can be
/// int, string, or null across different providers or even different records.
fn flexi_string<'de, D>(deserializer: D) -> Result<Option<String>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let value = serde_json::Value::deserialize(deserializer)?;
    match value {
        serde_json::Value::Null => Ok(None),
        serde_json::Value::String(s) if s.is_empty() => Ok(None),
        serde_json::Value::String(s) => Ok(Some(s)),
        serde_json::Value::Number(n) => Ok(Some(n.to_string())),
        serde_json::Value::Bool(b) => Ok(Some(b.to_string())),
        _ => Ok(None),
    }
}

#[derive(Debug, Deserialize)]
pub struct XtreamStream {
    #[serde(default, deserialize_with = "flexi_string")]
    pub stream_id: Option<String>,
    #[serde(default, deserialize_with = "flexi_string")]
    pub name: Option<String>,
    #[serde(default, deserialize_with = "flexi_string")]
    pub stream_icon: Option<String>,
    #[serde(default, deserialize_with = "flexi_string")]
    pub epg_channel_id: Option<String>,
    #[serde(default, deserialize_with = "flexi_string")]
    pub category_id: Option<String>,
    #[serde(default, deserialize_with = "flexi_string")]
    pub stream_type: Option<String>,
    #[serde(default, deserialize_with = "flexi_string")]
    pub tv_archive: Option<String>,
    #[serde(default, deserialize_with = "flexi_string")]
    pub tv_archive_duration: Option<String>,
}

impl XtreamStream {
    /// Parse stream_id as i64 (required for URL construction and DB storage).
    pub fn stream_id_i64(&self) -> Option<i64> {
        self.stream_id.as_deref()?.parse().ok()
    }

    /// Whether this stream has TV archive (catch-up) support.
    pub fn has_archive(&self) -> bool {
        self.tv_archive.as_deref().is_some_and(|v| v != "0")
    }
}

#[derive(Debug, Deserialize)]
pub struct XtreamCategory {
    pub category_id: String,
    pub category_name: String,
    pub parent_id: Option<i64>,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct XtreamAccountInfo {
    pub username: String,
    pub status: String,
    pub exp_date: Option<String>,
    pub max_connections: Option<String>,
    pub allowed_output_formats: Option<Vec<String>>,
}

/// Wrapper for the Xtream account info response which nests user_info.
#[derive(Debug, Deserialize)]
struct AccountInfoResponse {
    user_info: XtreamAccountInfo,
}

pub struct XtreamClient {
    http: reqwest::Client,
    base_url: String,
    username: String,
    password: String,
}

impl XtreamClient {
    /// Construct a new Xtream client. Strips trailing slash from base_url.
    pub fn new(
        http: reqwest::Client,
        base_url: impl Into<String>,
        username: impl Into<String>,
        password: impl Into<String>,
    ) -> Self {
        let mut base = base_url.into();
        while base.ends_with('/') {
            base.pop();
        }
        Self {
            http,
            base_url: base,
            username: username.into(),
            password: password.into(),
        }
    }

    /// Build the base query params (username + password, URL-encoded).
    fn api_url(&self, action: Option<&str>) -> String {
        let u = urlencoding::encode(&self.username);
        let p = urlencoding::encode(&self.password);
        match action {
            Some(a) => format!(
                "{}/player_api.php?username={u}&password={p}&action={a}",
                self.base_url
            ),
            None => format!("{}/player_api.php?username={u}&password={p}", self.base_url),
        }
    }

    /// GET `player_api.php` without action → account info.
    pub async fn get_account_info(&self) -> Result<XtreamAccountInfo> {
        let url = self.api_url(None);
        let resp = self
            .http
            .get(&url)
            .timeout(std::time::Duration::from_secs(30))
            .send()
            .await
            .context("fetching account info")?;
        let body = resp
            .error_for_status()
            .context("account info response status")?
            .text()
            .await
            .context("reading account info body")?;

        // Some providers return { user_info: {...}, server_info: {...} }
        if let Ok(wrapper) = serde_json::from_str::<AccountInfoResponse>(&body) {
            return Ok(wrapper.user_info);
        }
        // Others return the user_info directly
        serde_json::from_str::<XtreamAccountInfo>(&body).context("parsing account info JSON")
    }

    /// GET live streams.
    pub async fn get_live_streams(&self) -> Result<Vec<XtreamStream>> {
        self.fetch_streams("get_live_streams").await
    }

    /// GET live categories.
    pub async fn get_live_categories(&self) -> Result<Vec<XtreamCategory>> {
        let url = self.api_url(Some("get_live_categories"));
        let resp = self
            .http
            .get(&url)
            .timeout(std::time::Duration::from_secs(30))
            .send()
            .await
            .context("fetching live categories")?;
        let body = resp
            .error_for_status()
            .context("live categories response status")?
            .text()
            .await
            .context("reading live categories body")?;

        parse_json_array::<XtreamCategory>(&body).context("parsing live categories")
    }

    /// GET VOD streams.
    pub async fn get_vod_streams(&self) -> Result<Vec<XtreamStream>> {
        self.fetch_streams("get_vod_streams").await
    }

    /// Returns the XMLTV EPG URL for this provider.
    pub fn epg_url(&self) -> String {
        let u = urlencoding::encode(&self.username);
        let p = urlencoding::encode(&self.password);
        format!("{}/xmltv.php?username={u}&password={p}", self.base_url)
    }

    /// Returns the stream URL for a live stream.
    pub fn stream_url(&self, stream_id: i64, format: &str) -> String {
        let u = urlencoding::encode(&self.username);
        let p = urlencoding::encode(&self.password);
        format!("{}/live/{u}/{p}/{stream_id}.{format}", self.base_url)
    }

    /// Returns the stream URL for a VOD item.
    pub fn vod_url(&self, stream_id: i64, ext: &str) -> String {
        let u = urlencoding::encode(&self.username);
        let p = urlencoding::encode(&self.password);
        format!("{}/movie/{u}/{p}/{stream_id}.{ext}", self.base_url)
    }

    /// Shared fetch logic for stream endpoints.
    async fn fetch_streams(&self, action: &str) -> Result<Vec<XtreamStream>> {
        let url = self.api_url(Some(action));
        let resp = self
            .http
            .get(&url)
            .timeout(std::time::Duration::from_secs(120))
            .send()
            .await
            .with_context(|| format!("fetching {action}"))?;
        let body = resp
            .error_for_status()
            .with_context(|| format!("{action} response status"))?
            .text()
            .await
            .with_context(|| format!("reading {action} body"))?;

        parse_json_array::<XtreamStream>(&body).with_context(|| format!("parsing {action}"))
    }
}

/// Parse a JSON body that should be an array, handling providers that wrap
/// arrays in an extra object (e.g. `{"items": [...]}`).
fn parse_json_array<T: serde::de::DeserializeOwned>(body: &str) -> Result<Vec<T>> {
    // Try direct array first
    match serde_json::from_str::<Vec<T>>(body) {
        Ok(items) => return Ok(items),
        Err(e) => {
            warn!(
                error = %e,
                body_len = body.len(),
                body_start = &body[..body.len().min(200)],
                "direct array parse failed, trying wrapped object"
            );
        }
    }

    // Some providers wrap arrays in an object — try extracting first array value
    if let Ok(obj) = serde_json::from_str::<serde_json::Value>(body)
        && let Some(obj) = obj.as_object()
    {
        for value in obj.values() {
            if let Some(arr) = value.as_array() {
                let items: Vec<T> = arr
                    .iter()
                    .filter_map(|v| serde_json::from_value(v.clone()).ok())
                    .collect();
                if !items.is_empty() {
                    return Ok(items);
                }
            }
        }
    }

    warn!("could not parse response as array, returning empty");
    Ok(vec![])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strips_trailing_slash() {
        let client = XtreamClient::new(
            reqwest::Client::new(),
            "http://example.com/",
            "user",
            "pass",
        );
        assert_eq!(client.base_url, "http://example.com");
    }

    #[test]
    fn stream_url_format() {
        let client = XtreamClient::new(
            reqwest::Client::new(),
            "http://iptv.example.com",
            "myuser",
            "mypass",
        );
        assert_eq!(
            client.stream_url(12345, "m3u8"),
            "http://iptv.example.com/live/myuser/mypass/12345.m3u8"
        );
    }

    #[test]
    fn vod_url_format() {
        let client = XtreamClient::new(
            reqwest::Client::new(),
            "http://iptv.example.com",
            "myuser",
            "mypass",
        );
        assert_eq!(
            client.vod_url(999, "mp4"),
            "http://iptv.example.com/movie/myuser/mypass/999.mp4"
        );
    }

    #[test]
    fn epg_url_format() {
        let client = XtreamClient::new(
            reqwest::Client::new(),
            "http://iptv.example.com",
            "myuser",
            "mypass",
        );
        assert_eq!(
            client.epg_url(),
            "http://iptv.example.com/xmltv.php?username=myuser&password=mypass"
        );
    }

    #[test]
    fn url_encodes_credentials() {
        let client = XtreamClient::new(
            reqwest::Client::new(),
            "http://iptv.example.com",
            "user@name",
            "p@ss&word",
        );
        let url = client.stream_url(1, "ts");
        assert!(url.contains("user%40name"));
        assert!(url.contains("p%40ss%26word"));
    }

    #[test]
    fn parse_json_array_direct() {
        let body = r#"[{"stream_id":1,"name":"Test"}]"#;
        let result: Vec<XtreamStream> = parse_json_array(body).unwrap();
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].stream_id_i64(), Some(1));
    }

    #[test]
    fn parse_json_array_wrapped() {
        let body = r#"{"items":[{"stream_id":2,"name":"Wrapped"}]}"#;
        let result: Vec<XtreamStream> = parse_json_array(body).unwrap();
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].stream_id_i64(), Some(2));
    }

    #[test]
    fn parse_json_array_empty_on_garbage() {
        let body = "not json at all";
        let result: Vec<XtreamStream> = parse_json_array(body).unwrap();
        assert!(result.is_empty());
    }

    #[test]
    fn stream_id_as_string() {
        let body = r#"[{"stream_id":"1867495","name":"Test Channel"}]"#;
        let result: Vec<XtreamStream> = parse_json_array(body).unwrap();
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].stream_id_i64(), Some(1867495));
    }

    #[test]
    fn stream_id_as_int() {
        let body = r#"[{"stream_id":1867495,"name":"Test Channel"}]"#;
        let result: Vec<XtreamStream> = parse_json_array(body).unwrap();
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].stream_id_i64(), Some(1867495));
    }

    #[test]
    fn empty_string_epg_channel_id_becomes_none() {
        let body = r#"[{"stream_id":1,"name":"Test","epg_channel_id":""}]"#;
        let result: Vec<XtreamStream> = parse_json_array(body).unwrap();
        assert!(result[0].epg_channel_id.is_none());
    }

    #[test]
    fn nonempty_epg_channel_id_preserved() {
        let body = r#"[{"stream_id":1,"name":"Test","epg_channel_id":"bbc1.uk"}]"#;
        let result: Vec<XtreamStream> = parse_json_array(body).unwrap();
        assert_eq!(result[0].epg_channel_id.as_deref(), Some("bbc1.uk"));
    }

    #[test]
    fn empty_string_stream_icon_becomes_none() {
        let body = r#"[{"stream_id":1,"name":"Test","stream_icon":""}]"#;
        let result: Vec<XtreamStream> = parse_json_array(body).unwrap();
        assert!(result[0].stream_icon.is_none());
    }

    #[test]
    fn tv_archive_fields_parsed() {
        let body = r#"[{"stream_id":1,"name":"Test","tv_archive":1,"tv_archive_duration":3}]"#;
        let result: Vec<XtreamStream> = parse_json_array(body).unwrap();
        assert_eq!(result[0].tv_archive.as_deref(), Some("1"));
        assert_eq!(result[0].tv_archive_duration.as_deref(), Some("3"));
    }

    #[test]
    fn tv_archive_string_values_parsed() {
        let body = r#"[{"stream_id":1,"name":"Test","tv_archive":"1","tv_archive_duration":"2"}]"#;
        let result: Vec<XtreamStream> = parse_json_array(body).unwrap();
        assert_eq!(result[0].tv_archive.as_deref(), Some("1"));
        assert_eq!(result[0].tv_archive_duration.as_deref(), Some("2"));
    }

    #[test]
    fn tv_archive_fields_default_when_missing() {
        let body = r#"[{"stream_id":1,"name":"Test"}]"#;
        let result: Vec<XtreamStream> = parse_json_array(body).unwrap();
        assert!(result[0].tv_archive.is_none());
        assert!(result[0].tv_archive_duration.is_none());
    }
}
