//! Media file interpretation - extract metadata from file paths.
//!
//! Abstracts file identification behind a trait so different backends
//! (Kobload HTTP API, local regex) can be swapped.

pub mod kobload;
pub mod local;

pub use kobload::KobloadInterpreter;
pub use local::LocalInterpreter;

use crate::RuntimeConfigManager;
use async_trait::async_trait;
use regex::Regex;
use serde::{Deserialize, Serialize};
use std::sync::LazyLock;

static SHOW_YEAR: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^(.+?)[\s._]*\(?((?:19|20)\d{2})\)?$").unwrap());

/// Extract year from end of a show/movie name like "Show Name (2020)".
pub(crate) fn extract_year(name: &str) -> Option<u32> {
    SHOW_YEAR
        .captures(name)
        .and_then(|caps| caps[2].parse().ok())
}

/// Strip trailing year from a name.
pub(crate) fn strip_year(name: &str) -> String {
    if let Some(caps) = SHOW_YEAR.captures(name) {
        caps[1].trim().to_string()
    } else {
        name.to_string()
    }
}

/// An item to be interpreted.
#[derive(Debug, Clone)]
pub struct InterpretItem {
    /// Relative path within the media root (folder structure matters).
    pub relative_path: String,
    /// File size in bytes.
    pub file_size: Option<i64>,
    /// Media kind hint: "tv" or "movies".
    pub media_kind: String,
}

/// Result of interpreting a single file.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InterpretResult {
    /// The original path that was interpreted.
    pub path: String,
    /// Whether interpretation succeeded.
    pub success: bool,
    /// Error message if interpretation failed.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    /// Parsed media type: "tv" or "movie".
    #[serde(skip_serializing_if = "Option::is_none")]
    pub media_type: Option<String>,

    // -- TV fields --
    /// Show name (for TV).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub show_name: Option<String>,
    /// Season number (for TV).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub season: Option<u32>,
    /// Episode numbers (for TV).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub episodes: Vec<u32>,
    /// Episode title (for TV).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub episode_title: Option<String>,
    /// Show year for disambiguation.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub show_year: Option<u32>,

    // -- Movie fields --
    /// Movie title.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub movie_title: Option<String>,
    /// Movie year.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub movie_year: Option<u32>,

    // -- Book fields --
    /// Book title (from filename parsing).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub book_title: Option<String>,
    /// Book authors (from filename parsing).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub book_authors: Vec<String>,

    /// Full parsed release from the interpreter (stored in DB as JSONB).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub raw: Option<nats_jobs::ParsedRelease>,

    /// Provider IDs extracted from sidecar files (NFO, metadata.json, etc.).
    /// Populated by the interpret phase when sidecar metadata is available.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sidecar_ids: Option<serde_json::Value>,
}

/// Trait for interpreting media file paths into structured metadata.
#[async_trait]
pub trait MediaInterpreter: Send + Sync {
    /// Interpret a batch of file paths, returning parsed metadata for each.
    async fn interpret_batch(
        &self,
        items: Vec<InterpretItem>,
    ) -> Result<Vec<InterpretResult>, InterpreterError>;
}

/// Get the appropriate interpreter based on runtime config.
///
/// Returns `KobloadInterpreter` when Kobload is configured, otherwise `LocalInterpreter`.
pub async fn get_interpreter(runtime_config: &RuntimeConfigManager) -> Box<dyn MediaInterpreter> {
    let config = runtime_config.get().await;
    if let (Some(base_url), Some(api_key)) = (&config.kobload_base_url, &config.kobload_api_key)
        && !base_url.is_empty()
        && !api_key.is_empty()
    {
        return Box::new(KobloadInterpreter::new(base_url.clone(), api_key.clone()));
    }
    Box::new(LocalInterpreter::new())
}

/// Errors from media interpretation.
#[derive(Debug, thiserror::Error)]
pub enum InterpreterError {
    #[error("HTTP error: {0}")]
    Http(#[from] reqwest::Error),

    #[error("Parse error: {0}")]
    Parse(String),

    #[error("Not configured: {0}")]
    NotConfigured(String),

    #[error("Other: {0}")]
    Other(String),
}
