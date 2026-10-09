//! Error types for the kobo-proxy crate.

use thiserror::Error;

/// Errors that can occur during proxy operation.
#[derive(Error, Debug)]
pub enum ProxyError {
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    #[error("TLS error: {0}")]
    Tls(String),

    #[error("HTTP parse error: {0}")]
    HttpParse(String),

    #[error("Invalid request: {0}")]
    InvalidRequest(String),

    #[error("Upstream connection failed: {0}")]
    UpstreamConnection(String),

    #[error("Upstream timeout")]
    UpstreamTimeout,

    #[error("Device not found: {0}")]
    DeviceNotFound(String),

    #[error("Configuration error: {0}")]
    Config(String),
}

impl From<httparse::Error> for ProxyError {
    fn from(e: httparse::Error) -> Self {
        ProxyError::HttpParse(e.to_string())
    }
}

impl From<rustls::Error> for ProxyError {
    fn from(e: rustls::Error) -> Self {
        ProxyError::Tls(e.to_string())
    }
}
