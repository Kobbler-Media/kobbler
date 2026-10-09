//! kobo-proxy - Transparent byte-level proxy for Kobo e-reader sync protocol.
//!
//! This crate provides a smart reverse proxy that can intercept and modify
//! HTTP traffic between Kobo e-readers and the Kobo sync service.
//!
//! # Features
//!
//! - **Transparent proxying**: Passes bytes through unchanged for non-intercepted requests
//! - **Minimal HTTP parsing**: Only parses what's needed for routing decisions
//! - **Request classification**: Intercept, passthrough, or modify each request
//! - **Async logging**: Request/response logging that doesn't block the response
//! - **HTTP/HTTPS upstream**: Connects to upstream over HTTP or HTTPS
//! - **Proxy disable mode**: Can operate in local-only mode without upstream
//!
//! # Example
//!
//! ```rust,no_run
//! use std::sync::Arc;
//! use kobo_proxy::{run_proxy, ProxyConfig, ProxyHandler, ClassifyResult, ProxyDecision, ProxyRequest, ProxyResponse, UpstreamConfig};
//! use async_trait::async_trait;
//!
//! struct MyHandler;
//!
//! #[async_trait]
//! impl ProxyHandler for MyHandler {
//!     async fn classify(&self, req: &ProxyRequest) -> ClassifyResult {
//!         ProxyDecision::Passthrough.into()
//!     }
//!
//!     async fn handle(&self, req: ProxyRequest) -> ProxyResponse {
//!         ProxyResponse::empty_json(200)
//!     }
//!
//!     async fn modify(&self, req: &ProxyRequest, resp: ProxyResponse) -> ProxyResponse {
//!         resp
//!     }
//!
//!     async fn stub(&self, req: &ProxyRequest) -> ProxyResponse {
//!         ProxyResponse::empty_json(200)
//!     }
//!
//!     async fn upstream_for(&self, device_token: &str) -> Option<UpstreamConfig> {
//!         Some(UpstreamConfig::default())
//!     }
//!
//!     async fn proxy_enabled(&self) -> bool {
//!         true
//!     }
//! }
//!
//! #[tokio::main]
//! async fn main() {
//!     let config = ProxyConfig::default();
//!     let handler = Arc::new(MyHandler);
//!     run_proxy(config, handler, None).await.unwrap();
//! }
//! ```

mod connection;
mod error;
mod handler;
mod http;
mod proxy;
mod tls;
mod types;

// Re-exports
pub use error::ProxyError;
pub use handler::{NoOpLogger, ProxyHandler, RequestLogger};
pub use http::{UPSTREAM_HOST_PARAM, is_kobo_domain};
pub use proxy::{ProxyConfig, run_proxy, serve_proxy};
pub use types::{
    ClassifyResult, OnSentCallback, ProxyDecision, ProxyRequest, ProxyResponse, RequestLogEntry,
    ResponseBody, UpstreamConfig, header_serde,
};

// Re-export async_trait for convenience
pub use async_trait::async_trait;
