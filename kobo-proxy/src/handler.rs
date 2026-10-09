//! ProxyHandler trait for implementing custom proxy behavior.

use async_trait::async_trait;

use crate::types::{ClassifyResult, ProxyRequest, ProxyResponse, RequestLogEntry, UpstreamConfig};

/// Implement this trait to define proxy behavior.
///
/// The handler is called for each incoming request to:
/// 1. Classify the request (intercept, passthrough, or modify)
/// 2. Handle intercepted requests
/// 3. Modify responses from upstream
/// 4. Provide stubs when proxy is disabled
/// 5. Configure upstream URLs per device
#[async_trait]
pub trait ProxyHandler: Send + Sync + 'static {
    /// Decide what to do with this request.
    ///
    /// Called early in request processing with parsed request info.
    /// The decision determines the flow:
    /// - Intercept: call handle(), never contact upstream
    /// - Passthrough: pipe bytes to/from upstream unchanged
    /// - Modify: fetch from upstream, call modify(), return modified response
    ///
    /// Returns a ClassifyResult containing the decision and optional notes
    /// for logging (e.g., book title for local book requests).
    async fn classify(&self, req: &ProxyRequest) -> ClassifyResult;

    /// Handle an intercepted request (return response directly).
    ///
    /// Called when classify() returns Intercept, or when proxy is disabled
    /// and classify() returns Modify.
    async fn handle(&self, req: ProxyRequest) -> ProxyResponse;

    /// Modify an upstream response before returning to client.
    ///
    /// Called when classify() returns Modify and proxy is enabled.
    /// Receives the original request and the upstream response.
    /// Return the (possibly modified) response to send to the client.
    async fn modify(&self, req: &ProxyRequest, resp: ProxyResponse) -> ProxyResponse;

    /// Return stub response when proxy is disabled (for Passthrough decisions).
    ///
    /// Called when classify() returns Passthrough but proxy_enabled() is false.
    async fn stub(&self, req: &ProxyRequest) -> ProxyResponse;

    /// Get upstream configuration for a device token.
    ///
    /// Returns None if the device is unknown (will return 401).
    /// The upstream URL can be per-device for different Kobo regions.
    async fn upstream_for(&self, device_token: &str) -> Option<UpstreamConfig>;

    /// Is proxying to upstream enabled?
    ///
    /// When false:
    /// - Passthrough requests return stub()
    /// - Modify requests call handle() instead
    /// - No TLS connections to upstream are opened
    async fn proxy_enabled(&self) -> bool;

    /// Transform request headers before sending to upstream.
    /// Default: no-op (returns headers unchanged).
    async fn transform_upstream_headers(
        &self,
        _req: &ProxyRequest,
        headers: Vec<(String, String)>,
    ) -> Vec<(String, String)> {
        headers
    }
}

/// Implement this trait to log requests/responses.
///
/// Logging is asynchronous - the request/response is not blocked waiting
/// for the log to be written.
#[async_trait]
pub trait RequestLogger: Send + Sync + 'static {
    /// Log a request at the start (in_progress status).
    ///
    /// Called early in request processing to show inflight requests.
    /// Returns the log entry ID for later update.
    async fn log_start(&self, entry: &RequestLogEntry);

    /// Log a completed request/response.
    ///
    /// Called after the response has been sent to the client.
    /// Updates the existing log entry with final status and response details.
    async fn log(&self, entry: RequestLogEntry);

    /// Get temp directory for blob capture, if enabled.
    ///
    /// Returns Some(path) if blob capture is enabled, None otherwise.
    /// Binary request/response bodies will be streamed to temp files in this directory.
    fn blob_temp_dir(&self) -> Option<std::path::PathBuf> {
        None
    }
}

/// No-op logger that discards all log entries.
pub struct NoOpLogger;

#[async_trait]
impl RequestLogger for NoOpLogger {
    async fn log_start(&self, _entry: &RequestLogEntry) {
        // Discard
    }

    async fn log(&self, _entry: RequestLogEntry) {
        // Discard
    }
}
