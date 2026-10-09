//! TLS and plain TCP connection handling for upstream connections.

use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;

use rustls::ClientConfig;
use rustls_pki_types::ServerName;
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};
use tokio::net::TcpStream;
use tokio::time::timeout;
use tokio_rustls::TlsConnector;
use tokio_rustls::client::TlsStream;

use crate::error::ProxyError;

/// Default connection timeout in milliseconds
const DEFAULT_TIMEOUT_MS: u64 = 30_000;

/// An upstream connection that can be either TLS or plain TCP.
pub enum UpstreamConnection {
    Tls(Box<TlsStream<TcpStream>>),
    Plain(TcpStream),
}

impl AsyncRead for UpstreamConnection {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        match self.get_mut() {
            UpstreamConnection::Tls(s) => Pin::new(s.as_mut()).poll_read(cx, buf),
            UpstreamConnection::Plain(s) => Pin::new(s).poll_read(cx, buf),
        }
    }
}

impl AsyncWrite for UpstreamConnection {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        buf: &[u8],
    ) -> std::task::Poll<std::io::Result<usize>> {
        match self.get_mut() {
            UpstreamConnection::Tls(s) => Pin::new(s.as_mut()).poll_write(cx, buf),
            UpstreamConnection::Plain(s) => Pin::new(s).poll_write(cx, buf),
        }
    }

    fn poll_flush(
        self: Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        match self.get_mut() {
            UpstreamConnection::Tls(s) => Pin::new(s.as_mut()).poll_flush(cx),
            UpstreamConnection::Plain(s) => Pin::new(s).poll_flush(cx),
        }
    }

    fn poll_shutdown(
        self: Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        match self.get_mut() {
            UpstreamConnection::Tls(s) => Pin::new(s.as_mut()).poll_shutdown(cx),
            UpstreamConnection::Plain(s) => Pin::new(s).poll_shutdown(cx),
        }
    }
}

/// A client for connecting to upstream servers (supports both HTTP and HTTPS).
pub struct TlsClient {
    connector: TlsConnector,
}

impl TlsClient {
    /// Create a new client with default root certificates for TLS.
    pub fn new() -> Result<Self, ProxyError> {
        let root_store =
            rustls::RootCertStore::from_iter(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());

        let config = ClientConfig::builder()
            .with_root_certificates(root_store)
            .with_no_client_auth();

        Ok(Self {
            connector: TlsConnector::from(Arc::new(config)),
        })
    }

    /// Connect to an upstream server (TLS or plain TCP based on use_tls flag).
    pub async fn connect(
        &self,
        host: &str,
        port: u16,
        use_tls: bool,
    ) -> Result<UpstreamConnection, ProxyError> {
        self.connect_with_timeout(host, port, use_tls, DEFAULT_TIMEOUT_MS)
            .await
    }

    /// Connect to an upstream server with configurable timeout.
    pub async fn connect_with_timeout(
        &self,
        host: &str,
        port: u16,
        use_tls: bool,
        timeout_ms: u64,
    ) -> Result<UpstreamConnection, ProxyError> {
        let addr = format!("{}:{}", host, port);
        let duration = Duration::from_millis(timeout_ms);

        // TCP connection with timeout
        let stream = timeout(duration, TcpStream::connect(&addr))
            .await
            .map_err(|_| {
                ProxyError::UpstreamConnection(format!(
                    "Connection to {} timed out after {}ms",
                    addr, timeout_ms
                ))
            })?
            .map_err(|e| {
                ProxyError::UpstreamConnection(format!("Failed to connect to {}: {}", addr, e))
            })?;

        if use_tls {
            let server_name = ServerName::try_from(host.to_string()).map_err(|_| {
                ProxyError::UpstreamConnection(format!("Invalid server name: {}", host))
            })?;

            // TLS handshake with timeout
            let tls_stream = timeout(duration, self.connector.connect(server_name, stream))
                .await
                .map_err(|_| {
                    ProxyError::Tls(format!(
                        "TLS handshake with {} timed out after {}ms",
                        host, timeout_ms
                    ))
                })?
                .map_err(|e| ProxyError::Tls(format!("TLS handshake failed: {}", e)))?;

            Ok(UpstreamConnection::Tls(Box::new(tls_stream)))
        } else {
            Ok(UpstreamConnection::Plain(stream))
        }
    }
}

impl Default for TlsClient {
    fn default() -> Self {
        Self::new().expect("Failed to create TLS client")
    }
}

/// Parse host and port from a URL, also indicating if it's HTTP or HTTPS.
///
/// Input: "https://storeapi.kobo.com" or "http://localhost:8080"
/// Output: ("storeapi.kobo.com", 443, true) or ("localhost", 8080, false)
pub fn parse_upstream_url(url: &str) -> Result<(String, u16, bool), ProxyError> {
    let is_https = url.starts_with("https://");

    // Strip protocol
    let without_protocol = url
        .strip_prefix("https://")
        .or_else(|| url.strip_prefix("http://"))
        .unwrap_or(url);

    // Strip path
    let host_port = without_protocol
        .split('/')
        .next()
        .unwrap_or(without_protocol);

    // Split host and port
    match host_port.rsplit_once(':') {
        Some((host, port_str)) => {
            let port: u16 = port_str
                .parse()
                .map_err(|_| ProxyError::Config(format!("Invalid port in URL: {}", url)))?;
            Ok((host.to_string(), port, is_https))
        }
        None => {
            // Default to 443 for HTTPS, 80 for HTTP
            let default_port = if is_https { 443 } else { 80 };
            Ok((host_port.to_string(), default_port, is_https))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_upstream_url() {
        let (host, port, is_https) = parse_upstream_url("https://storeapi.kobo.com").unwrap();
        assert_eq!(host, "storeapi.kobo.com");
        assert_eq!(port, 443);
        assert!(is_https);

        let (host, port, is_https) = parse_upstream_url("https://example.com:8443").unwrap();
        assert_eq!(host, "example.com");
        assert_eq!(port, 8443);
        assert!(is_https);

        let (host, port, is_https) =
            parse_upstream_url("https://example.com/path/ignored").unwrap();
        assert_eq!(host, "example.com");
        assert_eq!(port, 443);
        assert!(is_https);

        let (host, port, is_https) = parse_upstream_url("http://localhost:8080").unwrap();
        assert_eq!(host, "localhost");
        assert_eq!(port, 8080);
        assert!(!is_https);

        let (host, port, is_https) = parse_upstream_url("http://127.0.0.1:12345").unwrap();
        assert_eq!(host, "127.0.0.1");
        assert_eq!(port, 12345);
        assert!(!is_https);
    }
}
