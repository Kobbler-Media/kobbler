//! Main proxy server implementation.

use std::net::SocketAddr;
use std::sync::Arc;

use tokio::net::TcpListener;
use tracing::{error, info};

use crate::connection::handle_connection;
use crate::error::ProxyError;
use crate::handler::{NoOpLogger, ProxyHandler, RequestLogger};
use crate::tls::TlsClient;

/// Configuration for the proxy server.
#[derive(Debug, Clone)]
pub struct ProxyConfig {
    /// Address to listen on
    pub listen_addr: SocketAddr,
}

impl Default for ProxyConfig {
    fn default() -> Self {
        Self {
            listen_addr: ([0, 0, 0, 0], 6767).into(),
        }
    }
}

/// Bind `config.listen_addr` and serve until an error occurs.
pub async fn run_proxy<H: ProxyHandler>(
    config: ProxyConfig,
    handler: Arc<H>,
    logger: Option<Arc<dyn RequestLogger>>,
) -> Result<(), ProxyError> {
    let listener = TcpListener::bind(config.listen_addr).await?;
    serve_proxy(listener, handler, logger).await
}

/// Serve on an already-bound listener, so the caller can know the port before
/// any connection arrives without releasing and re-binding it.
pub async fn serve_proxy<H: ProxyHandler>(
    listener: TcpListener,
    handler: Arc<H>,
    logger: Option<Arc<dyn RequestLogger>>,
) -> Result<(), ProxyError> {
    let logger = logger.unwrap_or_else(|| Arc::new(NoOpLogger));
    let tls_client = Arc::new(TlsClient::new()?);
    info!(addr = %listener.local_addr()?, "Proxy server listening");

    loop {
        match listener.accept().await {
            Ok((stream, _addr)) => {
                let handler = handler.clone();
                let logger = logger.clone();
                let tls_client = tls_client.clone();

                tokio::spawn(async move {
                    handle_connection(stream, handler, logger, tls_client).await;
                });
            }
            Err(e) => {
                error!(error = %e, "Failed to accept connection");
            }
        }
    }
}
