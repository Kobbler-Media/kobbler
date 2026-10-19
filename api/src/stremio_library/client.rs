use serde::Deserialize;
use serde::de::DeserializeOwned;
use serde_json::{Value, json};
use time::OffsetDateTime;

#[derive(Debug, thiserror::Error)]
pub enum StremioError {
    #[error("Stremio rejected the request: {0}")]
    Rejected(String),
    #[error("Stremio request failed: {0}")]
    Http(#[from] reqwest::Error),
}

#[derive(Debug, Clone, Deserialize)]
pub struct LibraryItem {
    #[serde(rename = "_id")]
    pub id: String,
    #[serde(rename = "type")]
    pub item_type: String,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub removed: bool,
    #[serde(default)]
    pub temp: bool,
    #[serde(rename = "_mtime", default, deserialize_with = "lenient_timestamp")]
    pub mtime: Option<OffsetDateTime>,
    #[serde(default)]
    pub state: LibraryItemState,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LibraryItemState {
    #[serde(default, deserialize_with = "lenient_timestamp")]
    pub last_watched: Option<OffsetDateTime>,
    #[serde(default)]
    pub time_offset: u64,
    #[serde(default)]
    pub duration: u64,
    #[serde(rename = "video_id", default)]
    pub video_id: Option<String>,
    #[serde(default)]
    pub times_watched: u64,
    #[serde(default)]
    pub flagged_watched: u64,
}

// Stremio writes an empty string where it has no timestamp.
fn lenient_timestamp<'de, D>(d: D) -> Result<Option<OffsetDateTime>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let raw: Option<String> = Option::deserialize(d)?;
    Ok(raw.and_then(|s| {
        OffsetDateTime::parse(&s, &time::format_description::well_known::Rfc3339).ok()
    }))
}

#[derive(Deserialize)]
struct Envelope<T> {
    result: Option<T>,
    error: Option<Value>,
}

#[derive(Deserialize)]
struct LoginResult {
    #[serde(rename = "authKey")]
    auth_key: String,
}

pub struct StremioClient {
    http: reqwest::Client,
    base_url: String,
}

impl StremioClient {
    pub fn new(base_url: &str) -> Self {
        Self {
            http: crate::http::new_client(),
            base_url: base_url.trim_end_matches('/').to_string(),
        }
    }

    pub async fn login(&self, email: &str, password: &str) -> Result<String, StremioError> {
        let result: LoginResult = self
            .call(
                "login",
                json!({ "type": "Login", "email": email, "password": password, "facebook": false }),
            )
            .await?;
        Ok(result.auth_key)
    }

    pub async fn library(&self, auth_key: &str) -> Result<Vec<LibraryItem>, StremioError> {
        let items: Vec<Value> = self
            .call(
                "datastoreGet",
                json!({ "authKey": auth_key, "collection": "libraryItem", "ids": [], "all": true }),
            )
            .await?;
        // One malformed entry (a third-party addon's id scheme, a legacy field shape) must not
        // cost the user the whole row.
        Ok(items
            .into_iter()
            .filter_map(|v| serde_json::from_value(v).ok())
            .collect())
    }

    async fn call<T: DeserializeOwned>(
        &self,
        method: &str,
        body: Value,
    ) -> Result<T, StremioError> {
        let envelope: Envelope<T> = self
            .http
            .post(format!("{}/api/{method}", self.base_url))
            .json(&body)
            .send()
            .await?
            .json()
            .await?;
        match (envelope.result, envelope.error) {
            (Some(result), None) => Ok(result),
            (_, Some(error)) => Err(StremioError::Rejected(
                error
                    .get("message")
                    .and_then(Value::as_str)
                    .map(String::from)
                    .unwrap_or_else(|| error.to_string()),
            )),
            (None, None) => Err(StremioError::Rejected(format!("empty {method} response"))),
        }
    }
}
