use axum::{
    Json,
    http::StatusCode,
    response::{IntoResponse, Response},
};
use serde_json::json;

use crate::kobload::KobloadError;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StreamFailure {
    Blocked,
    RateLimited,
    AuthFailed,
    NotCached,
    Timeout,
    Unavailable,
    NotVideo,
}

impl StreamFailure {
    pub fn code(self) -> &'static str {
        match self {
            Self::Blocked => "blocked",
            Self::RateLimited => "rate_limited",
            Self::AuthFailed => "auth_failed",
            Self::NotCached => "not_cached",
            Self::Timeout => "timeout",
            Self::Unavailable => "unavailable",
            Self::NotVideo => "not_video",
        }
    }

    pub fn message(self) -> &'static str {
        match self {
            Self::Blocked => "The debrid provider refused this release. Try a different source.",
            Self::RateLimited => {
                "The debrid provider is limiting requests right now. Wait a moment and try again."
            }
            Self::AuthFailed => {
                "The debrid provider or Kobload rejected its credentials. Check the API keys in Settings."
            }
            Self::NotCached => {
                "This release isn't available on the debrid provider. Try a different source."
            }
            Self::Timeout => {
                "The debrid provider took too long to respond. Try again or pick a different source."
            }
            Self::Unavailable => "The stream service is unavailable right now. Try again shortly.",
            Self::NotVideo => {
                "This release doesn't contain a playable video. Try a different source."
            }
        }
    }

    pub fn classify(status: Option<u16>, detail: &str) -> Self {
        let text = detail.to_ascii_lowercase();
        let has = |needles: &[&str]| needles.iter().any(|n| text.contains(n));
        if has(&["refused this", "infringing", "blocked"]) {
            Self::Blocked
        } else if has(&["not a video"]) {
            Self::NotVideo
        } else if status == Some(429) || has(&["rate limit", "fair usage", "too many active"]) {
            Self::RateLimited
        } else if matches!(status, Some(401 | 403))
            || has(&["authentication failed", "unauthorized"])
        {
            Self::AuthFailed
        } else if status == Some(504) || has(&["timed out", "timeout"]) {
            Self::Timeout
        } else if has(&["not found or disabled", "no debrid providers"]) {
            Self::Unavailable
        } else if has(&["not found", "no download links", "not cached"]) {
            Self::NotCached
        } else {
            Self::Unavailable
        }
    }

    pub fn from_kobload(e: &KobloadError) -> Self {
        match e {
            KobloadError::Request(err) if err.is_timeout() => Self::Timeout,
            KobloadError::Request(_) => Self::Unavailable,
            KobloadError::Status(status, body) => Self::classify(Some(status.as_u16()), body),
        }
    }

    pub fn response(self, status: StatusCode, detail: &str) -> Response {
        (
            status,
            Json(json!({
                "error": self.message(),
                "reason": self.code(),
                "detail": detail,
            })),
        )
            .into_response()
    }
}

#[cfg(test)]
mod tests {
    use super::StreamFailure::{self, *};

    #[test]
    fn classifies_kobload_errors() {
        let cases: &[(Option<u16>, &str, StreamFailure)] = &[
            (
                Some(500),
                "Debrid error: Real-Debrid refused this release. Try a different source.",
                Blocked,
            ),
            (
                Some(500),
                "Debrid error: Failed to add magnet: Provider refused this file (infringing/filtered)",
                Blocked,
            ),
            (
                Some(500),
                "Debrid error: Failed to add magnet: Rate limited (retry after 5s)",
                RateLimited,
            ),
            (
                Some(500),
                "Debrid error: Failed to add magnet: Provider fair usage limit reached",
                RateLimited,
            ),
            (
                Some(500),
                "Debrid error: Failed to add magnet: Too many active torrents on the provider",
                RateLimited,
            ),
            (Some(429), "", RateLimited),
            (
                Some(500),
                "Debrid error: Failed to add magnet: Authentication failed",
                AuthFailed,
            ),
            (Some(401), "Unauthorized", AuthFailed),
            (
                Some(500),
                "Debrid error: Timed out waiting for usenet download",
                Timeout,
            ),
            (None, "request failed: operation timed out", Timeout),
            (
                Some(500),
                "Debrid error: Debrid provider 0000 not found or disabled",
                Unavailable,
            ),
            (
                Some(500),
                "Debrid error: Failed to add magnet: Not found",
                NotCached,
            ),
            (
                Some(500),
                "Debrid error: No download links available",
                NotCached,
            ),
            (
                Some(500),
                "Debrid error: Usenet download failed: missing articles",
                Unavailable,
            ),
            (None, "request failed: connection refused", Unavailable),
            (Some(500), "Search failed: all indexers down", Unavailable),
            (
                Some(500),
                "Debrid error: Resolved file is not a video: setup.exe (application/x-msdownload)",
                NotVideo,
            ),
            (
                Some(500),
                "Debrid error: Failed to add magnet: Provider refused this torrent (too big for the account)",
                Blocked,
            ),
        ];
        for (status, detail, want) in cases {
            assert_eq!(
                StreamFailure::classify(*status, detail),
                *want,
                "{status:?} {detail}"
            );
        }
    }
}
