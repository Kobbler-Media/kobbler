//! Error types for the STK client.

use thiserror::Error;

/// Result type alias for STK client operations.
pub type Result<T> = std::result::Result<T, Error>;

/// Errors that can occur during STK client operations.
#[derive(Debug, Error)]
pub enum Error {
    /// HTTP request failed.
    #[error("HTTP request failed: {0}")]
    Http(#[from] reqwest::Error),

    /// JSON serialization/deserialization failed.
    #[error("JSON error: {0}")]
    Json(#[from] serde_json::Error),

    /// XML parsing failed.
    #[error("XML parsing error: {0}")]
    Xml(#[from] quick_xml::DeError),

    /// RSA cryptography error.
    #[error("RSA error: {0}")]
    Rsa(#[from] rsa::Error),

    /// PKCS1 key parsing error.
    #[error("PKCS1 error: {0}")]
    Pkcs1(#[from] rsa::pkcs1::Error),

    /// URL parsing error.
    #[error("URL error: {0}")]
    Url(#[from] url::ParseError),

    /// I/O error (file operations).
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),

    /// API returned an error response.
    #[error("API error ({status}): {message}")]
    Api {
        /// HTTP status code.
        status: u16,
        /// Error message from API.
        message: String,
    },

    /// OAuth2 flow error (missing code, invalid redirect).
    #[error("OAuth error: {0}")]
    OAuth(String),

    /// Credential serialization error.
    #[error("Invalid credentials: {0}")]
    Credentials(String),
}
