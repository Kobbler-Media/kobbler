//! Per-connection handling logic.

use std::io::{Read, Write};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Instant;

use bytes::BytesMut;
use flate2::Compression;
use flate2::read::GzDecoder;
use flate2::write::GzEncoder;
use tokio::fs::File;
use tokio::io::{AsyncReadExt, AsyncWriteExt, BufWriter};
use tokio::net::TcpStream;
use tracing::{debug, warn};
use uuid::Uuid;

use crate::error::ProxyError;
use crate::handler::{ProxyHandler, RequestLogger};
use crate::http::{
    build_response_header_bytes, build_upstream_request, extract_upstream_host, is_kobo_domain,
    parse_request_headers, parse_response_headers, parse_response_status,
    process_cookies_in_headers, strip_upstream_host_param,
};
use crate::tls::{TlsClient, parse_upstream_url};
use crate::types::{ProxyDecision, ProxyResponse, RequestLogEntry};

/// Maximum size of response body to buffer for logging (1MB)
const MAX_LOGGABLE_BODY_SIZE: usize = 1024 * 1024;

/// Maximum request body size (100MB) - protects against DoS
const MAX_REQUEST_BODY_SIZE: usize = 100 * 1024 * 1024;

/// Maximum decompressed size (100MB) - protects against gzip bombs
const MAX_DECOMPRESSED_SIZE: usize = 100 * 1024 * 1024;

/// Configuration for upstream requests.
///
/// Groups together the parameters needed to make a request to the upstream Kobo server.
pub struct UpstreamConfig<'a> {
    pub url: &'a str,
    pub cleaned_query: Option<&'a str>,
    pub path_override: Option<&'a str>,
}

/// Handle a single client connection.
pub async fn handle_connection<H: ProxyHandler>(
    mut client: TcpStream,
    handler: Arc<H>,
    logger: Arc<dyn RequestLogger>,
    tls_client: Arc<TlsClient>,
) {
    let start = Instant::now();

    // Get blob temp dir from logger (for blob capture)
    let blob_temp_dir = logger.blob_temp_dir();

    // Parse request headers
    let parsed = match parse_request_headers(&mut client).await {
        Ok(p) => p,
        Err(e) => {
            warn!("Failed to parse request: {}", e);
            let _ = send_error_response(&mut client, 400, "Bad Request").await;
            // Log failed request
            let mut log_entry =
                RequestLogEntry::new("UNKNOWN", "/parse-error", ProxyDecision::Intercept);
            log_entry.status = 400;
            log_entry.severity = "error".to_string();
            log_entry.notes = Some(format!("Failed to parse request: {}", e));
            log_entry.total_duration_ms = start.elapsed().as_millis() as u32;
            let logger = logger.clone();
            tokio::spawn(async move {
                logger.log(log_entry).await;
            });
            return;
        }
    };

    let req = &parsed.request;
    debug!(
        method = %req.method,
        path = %req.path,
        device_token = ?req.device_token,
        "Received request"
    );

    // Check device token
    let device_token = match &req.device_token {
        Some(t) => t.clone(),
        None => {
            warn!(path = %req.original_path, "No device token in path");
            let _ = send_error_response(&mut client, 400, "Missing device token").await;
            // Log request with missing token
            let mut log_entry =
                RequestLogEntry::new(&req.method, &req.original_path, ProxyDecision::Intercept);
            log_entry.request_headers = req.headers.clone();
            log_entry.status = 400;
            log_entry.severity = "error".to_string();
            log_entry.notes = Some("Missing device token in path".to_string());
            log_entry.total_duration_ms = start.elapsed().as_millis() as u32;
            let logger = logger.clone();
            tokio::spawn(async move {
                logger.log(log_entry).await;
            });
            return;
        }
    };

    // Get upstream config
    let upstream_config = match handler.upstream_for(&device_token).await {
        Some(c) => c,
        None => {
            warn!(device_token = %device_token, "Unknown device token");
            let _ = send_error_response(&mut client, 401, "Unknown device").await;
            // Log unknown device request
            let mut log_entry =
                RequestLogEntry::new(&req.method, &req.path, ProxyDecision::Intercept);
            log_entry.device_token = Some(device_token);
            log_entry.query = req.query.clone();
            log_entry.request_headers = req.headers.clone();
            log_entry.status = 401;
            log_entry.severity = "error".to_string();
            log_entry.notes = Some("Unknown device token".to_string());
            log_entry.total_duration_ms = start.elapsed().as_millis() as u32;
            let logger = logger.clone();
            tokio::spawn(async move {
                logger.log(log_entry).await;
            });
            return;
        }
    };

    // Check for upstream host override in query params
    let upstream_override = extract_upstream_host(req.query.as_deref());
    let (effective_upstream_url, cleaned_query, path_override) = if let Some(ref override_data) =
        upstream_override
    {
        // Validate the override host is a kobo domain
        if !is_kobo_domain(&override_data.host) {
            warn!(host = %override_data.host, "Invalid upstream host override - not a Kobo domain");
            let _ = send_error_response(&mut client, 400, "Invalid upstream host").await;
            let mut log_entry =
                RequestLogEntry::new(&req.method, &req.path, ProxyDecision::Intercept);
            log_entry.device_token = Some(device_token);
            log_entry.query = req.query.clone();
            log_entry.request_headers = req.headers.clone();
            log_entry.status = 400;
            log_entry.severity = "error".to_string();
            log_entry.notes = Some(format!(
                "Invalid upstream host override: {}",
                override_data.host
            ));
            log_entry.total_duration_ms = start.elapsed().as_millis() as u32;
            let logger = logger.clone();
            tokio::spawn(async move {
                logger.log(log_entry).await;
            });
            return;
        }
        // Use override host (just the host part), strip the param from query
        (
            format!("https://{}", override_data.host),
            strip_upstream_host_param(req.query.as_deref()),
            override_data.path.clone(),
        )
    } else {
        // Use default upstream from device config
        (upstream_config.base_url.clone(), req.query.clone(), None)
    };

    debug!(
        upstream = %effective_upstream_url,
        override_data = ?upstream_override,
        "Upstream URL determined"
    );

    // Classify the request
    let classify_result = handler.classify(req).await;
    let decision = classify_result.decision;
    debug!(decision = ?decision, "Request classified");

    // Create log entry - use original_path to show full incoming request path
    let mut log_entry = RequestLogEntry::new(&req.method, &req.original_path, decision);
    log_entry.device_token = Some(device_token.clone());
    log_entry.query = req.query.clone();
    log_entry.request_headers = req.headers.clone();

    // Set notes from classification result, or from path override
    if let Some(notes) = classify_result.notes {
        log_entry.notes = Some(notes);
    } else if let Some(ref path) = path_override {
        log_entry.notes = Some(format!("Path extracted from __upstream_host: {}", path));
    }
    log_entry.alert_message = classify_result.alert_message;

    // Log request start (inflight)
    logger.log_start(&log_entry).await;

    // Transform headers for upstream (e.g., unwrap JWT sync tokens)
    let upstream_headers = handler
        .transform_upstream_headers(req, req.headers.clone())
        .await;

    // Handle based on decision and proxy enabled state
    let proxy_enabled = handler.proxy_enabled().await;
    let upstream = UpstreamConfig {
        url: &effective_upstream_url,
        cleaned_query: cleaned_query.as_deref(),
        path_override: path_override.as_deref(),
    };
    let result = match decision {
        ProxyDecision::Intercept => {
            handle_intercept(&mut client, &parsed, &handler, &mut log_entry).await
        }
        ProxyDecision::Passthrough => {
            if proxy_enabled {
                handle_passthrough(
                    &mut client,
                    &parsed,
                    &upstream,
                    &tls_client,
                    &mut log_entry,
                    blob_temp_dir.as_ref(),
                    &upstream_headers,
                )
                .await
            } else {
                handle_stub(&mut client, req, &handler, &mut log_entry).await
            }
        }
        ProxyDecision::Modify => {
            if proxy_enabled {
                handle_modify(
                    &mut client,
                    &parsed,
                    &handler,
                    &upstream,
                    &tls_client,
                    &mut log_entry,
                    blob_temp_dir.as_ref(),
                    &upstream_headers,
                )
                .await
            } else {
                // When proxy disabled, Modify becomes Intercept
                handle_intercept(&mut client, &parsed, &handler, &mut log_entry).await
            }
        }
    };

    // Log any errors
    if let Err(e) = result {
        warn!(error = %e, "Request handling failed");
        log_entry.severity = "error".to_string();
        log_entry.notes = Some(e.to_string());
        log_entry.log_status = "error".to_string();
    } else {
        log_entry.log_status = "complete".to_string();
    }

    // Set severity based on HTTP status if not already set to error
    if log_entry.severity != "error" {
        log_entry.severity = match log_entry.status {
            s if s >= 500 => "error".to_string(),
            s if s >= 400 => "warning".to_string(),
            _ => "info".to_string(),
        };
    }

    // Finalize and spawn log write
    log_entry.total_duration_ms = start.elapsed().as_millis() as u32;

    // Async logging - don't block response
    let logger = logger.clone();
    tokio::spawn(async move {
        logger.log(log_entry).await;
    });
}

/// Handle an intercepted request (local handling only).
async fn handle_intercept<H: ProxyHandler>(
    client: &mut TcpStream,
    parsed: &crate::http::ParsedRequest,
    handler: &Arc<H>,
    log_entry: &mut RequestLogEntry,
) -> Result<(), ProxyError> {
    // Read body if present (Content-Length or chunked)
    let mut req = parsed.request.clone();
    let body_start = parsed.headers_end;
    let already_read_body = &parsed.header_bytes[body_start..];

    // Check body size limit upfront for Content-Length
    if let Some(len) = req.content_length
        && len > MAX_REQUEST_BODY_SIZE
    {
        return Err(ProxyError::HttpParse(format!(
            "Request body too large: {} bytes exceeds {} byte limit",
            len, MAX_REQUEST_BODY_SIZE
        )));
    }

    let body = if req.is_chunked {
        // Read chunked request body (with size limit enforced in read_chunked_body)
        Some(read_chunked_body_limited(client, already_read_body, MAX_REQUEST_BODY_SIZE).await?)
    } else if let Some(len) = req.content_length {
        // Read Content-Length body
        let mut body = Vec::with_capacity(len);
        body.extend_from_slice(already_read_body);

        let already_read = body.len();
        if already_read < len {
            let remaining = len - already_read;
            let mut rest = vec![0u8; remaining];
            client.read_exact(&mut rest).await?;
            body.extend_from_slice(&rest);
        }
        Some(body)
    } else {
        None
    };

    if let Some(ref body) = body {
        log_entry.request_body_size = body.len();
        if is_loggable_content_vec(&req.headers) {
            let is_gzipped = is_gzip_encoded_vec(&req.headers);
            log_entry.request_body = decompress_for_logging(body, is_gzipped);
        }
        req.body = Some(body.clone());
    }

    // Check if client accepts gzip
    let client_accepts_gzip = accepts_gzip_vec(&req.headers);

    // Call handler
    let mut response = handler.handle(req).await;

    // Copy source file path to log entry (for intercept file downloads)
    if let Some(ref path) = response.source_file_path {
        log_entry.intercept_source_path = Some(path.clone());
    }

    // Log response (uncompressed version) - only for buffered bodies
    log_entry.status = response.status;
    if is_loggable_content_vec(&response.headers)
        && let Some(body_bytes) = response.body.as_bytes()
    {
        log_entry.response_body = String::from_utf8(body_bytes.to_vec()).ok();
    }

    // Gzip response if client accepts it, content is compressible, and body is buffered
    if client_accepts_gzip
        && is_loggable_content_vec(&response.headers)
        && let crate::types::ResponseBody::Buffered(ref data) = response.body
        && let Some(compressed) = compress_body(data)
    {
        response.set_header("Content-Encoding", "gzip");
        response.set_header("Content-Length", compressed.len().to_string());
        response.body = crate::types::ResponseBody::Buffered(compressed);
    }

    // Log final response info
    log_entry.response_headers = response.headers.clone();
    log_entry.response_body_size = response.body.size() as usize;

    // Send response, then run on_sent callback if delivery succeeded
    let on_sent = response.on_sent.take();
    let result = send_response(client, response).await;
    if result.is_ok()
        && let Some(cb) = on_sent
    {
        cb().await;
    }
    result
}

/// Handle a stub response (when proxy disabled).
async fn handle_stub<H: ProxyHandler>(
    client: &mut TcpStream,
    req: &crate::types::ProxyRequest,
    handler: &Arc<H>,
    log_entry: &mut RequestLogEntry,
) -> Result<(), ProxyError> {
    // Check if client accepts gzip
    let client_accepts_gzip = accepts_gzip_vec(&req.headers);

    let mut response = handler.stub(req).await;

    // Log response (uncompressed version) - only for buffered bodies
    log_entry.status = response.status;
    if is_loggable_content_vec(&response.headers)
        && let Some(body_bytes) = response.body.as_bytes()
    {
        log_entry.response_body = String::from_utf8(body_bytes.to_vec()).ok();
    }

    // Gzip response if client accepts it, content is compressible, and body is buffered
    if client_accepts_gzip
        && is_loggable_content_vec(&response.headers)
        && let crate::types::ResponseBody::Buffered(ref data) = response.body
        && let Some(compressed) = compress_body(data)
    {
        response.set_header("Content-Encoding", "gzip");
        response.set_header("Content-Length", compressed.len().to_string());
        response.body = crate::types::ResponseBody::Buffered(compressed);
    }

    // Log final response info
    log_entry.response_headers = response.headers.clone();
    log_entry.response_body_size = response.body.size() as usize;

    let on_sent = response.on_sent.take();
    let result = send_response(client, response).await;
    if result.is_ok()
        && let Some(cb) = on_sent
    {
        cb().await;
    }
    result
}

/// Handle passthrough (pipe bytes to/from upstream).
async fn handle_passthrough(
    client: &mut TcpStream,
    parsed: &crate::http::ParsedRequest,
    upstream: &UpstreamConfig<'_>,
    tls_client: &TlsClient,
    log_entry: &mut RequestLogEntry,
    blob_temp_dir: Option<&PathBuf>,
    upstream_headers: &[(String, String)],
) -> Result<(), ProxyError> {
    let (host, port, is_https) = parse_upstream_url(upstream.url)?;
    // Log full upstream URL including path (or path_override) and cleaned query (without __upstream_host)
    let scheme = if is_https { "https" } else { "http" };
    let effective_path = upstream.path_override.unwrap_or(&parsed.request.path);
    let full_upstream_url = if let Some(q) = upstream.cleaned_query {
        format!("{}://{}{}?{}", scheme, host, effective_path, q)
    } else {
        format!("{}://{}{}", scheme, host, effective_path)
    };
    log_entry.upstream_url = Some(full_upstream_url);

    // Update logged request headers to show what we actually send to upstream (Host changed)
    for (k, v) in log_entry.request_headers.iter_mut() {
        if k.eq_ignore_ascii_case("host") {
            *v = host.clone();
        }
    }

    let upstream_start = Instant::now();

    // Connect to upstream (TLS for HTTPS, plain TCP for HTTP)
    let mut upstream_conn = tls_client.connect(&host, port, is_https).await?;

    // Build and send transformed request (with cleaned query, without __upstream_host param)
    let upstream_request = build_upstream_request(
        parsed,
        &host,
        upstream.cleaned_query,
        upstream.path_override,
        upstream_headers,
    );
    upstream_conn.write_all(&upstream_request).await?;

    // Forward request body if present (Content-Length or chunked)
    let body_start = parsed.headers_end;
    let already_read_body = &parsed.header_bytes[body_start..];

    // Check body size limit upfront for Content-Length
    if let Some(len) = parsed.request.content_length
        && len > MAX_REQUEST_BODY_SIZE
    {
        return Err(ProxyError::HttpParse(format!(
            "Request body too large: {} bytes exceeds {} byte limit",
            len, MAX_REQUEST_BODY_SIZE
        )));
    }

    let request_body = if parsed.request.is_chunked {
        // Read chunked request body with size limit
        Some(read_chunked_body_limited(client, already_read_body, MAX_REQUEST_BODY_SIZE).await?)
    } else if let Some(len) = parsed.request.content_length {
        let mut request_body = Vec::with_capacity(len);
        request_body.extend_from_slice(already_read_body);

        let already_read = request_body.len();
        if already_read < len {
            let remaining = len - already_read;
            let mut rest = vec![0u8; remaining];
            client.read_exact(&mut rest).await?;
            request_body.extend_from_slice(&rest);
        }
        Some(request_body)
    } else {
        None
    };

    if let Some(ref body) = request_body {
        upstream_conn.write_all(body).await?;
        log_entry.request_body_size = body.len();

        // Log request body if loggable
        if is_loggable_content_vec(&parsed.request.headers) && body.len() <= MAX_LOGGABLE_BODY_SIZE
        {
            let is_gzipped = is_gzip_encoded_vec(&parsed.request.headers);
            log_entry.request_body = decompress_for_logging(body, is_gzipped);
        }
    }

    upstream_conn.flush().await?;

    // Read response headers
    let mut response_buf = BytesMut::with_capacity(4096);
    loop {
        let n = upstream_conn.read_buf(&mut response_buf).await?;
        if n == 0 {
            return Err(ProxyError::HttpParse(
                "Connection closed before response".to_string(),
            ));
        }

        // Check for header end
        if response_buf.windows(4).any(|w| w == b"\r\n\r\n") {
            break;
        }
    }

    log_entry.upstream_duration_ms = Some(upstream_start.elapsed().as_millis() as u32);

    // Parse response status for logging
    let response_status = if let Ok((status, _, _)) = parse_response_status(&response_buf) {
        log_entry.status = status;
        log_entry.upstream_status = Some(status); // In passthrough, response = upstream
        Some(status)
    } else {
        None
    };

    // Parse response headers for logging
    if let Ok((headers, header_end)) = parse_response_headers(&response_buf) {
        log_entry.response_headers = headers.clone();

        // Process Set-Cookie headers for proxy compatibility:
        // - Check if any Set-Cookie headers exist
        // - If yes, duplicate them with domain stripped for same-origin access
        // - Kobo proxy runs over HTTP, so strip Secure flag too
        let has_cookies = headers
            .iter()
            .any(|(k, _)| k.eq_ignore_ascii_case("set-cookie"));
        let header_bytes: Vec<u8> = if has_cookies {
            // Get status info for rebuilding headers
            let (status, reason, _) = parse_response_status(&response_buf)?;
            let processed_headers = process_cookies_in_headers(&headers, false); // proxy is HTTP
            // Update log with processed headers
            log_entry.response_headers = processed_headers.clone();
            build_response_header_bytes(status, &reason, &processed_headers)
        } else {
            // No cookies, use original bytes (fast path)
            response_buf[..header_end].to_vec()
        };

        // Status codes that MUST NOT have a body per HTTP spec:
        // 1xx (informational), 204 (No Content), 304 (Not Modified)
        let is_no_body_status = response_status.is_some_and(|s| s < 200 || s == 204 || s == 304);

        if is_no_body_status {
            // Just forward headers, no body to read
            client.write_all(&header_bytes).await?;
            client.flush().await?;
            return Ok(());
        }

        // Check transfer encoding and content length
        let is_chunked = headers.iter().any(|(k, v)| {
            k.eq_ignore_ascii_case("transfer-encoding") && v.to_lowercase().contains("chunked")
        });

        let content_length: Option<usize> =
            get_header(&headers, "Content-Length").and_then(|v| v.parse().ok());

        // Check if this is a loggable response we should collect
        let is_loggable = is_loggable_content_vec(&headers);
        let is_gzipped = is_gzip_encoded_vec(&headers);

        // For chunked responses, stream to client while collecting for logging
        if is_chunked {
            // Forward headers (with processed cookies if any)
            client.write_all(&header_bytes).await?;

            // Tee: stream chunks to client while collecting for logging
            let body_already_read = &response_buf[header_end..];
            let collected = tee_chunked_body(
                &mut upstream_conn,
                client,
                body_already_read,
                is_loggable,
                &mut log_entry.response_body_size,
            )
            .await?;

            if is_loggable {
                log_entry.response_body =
                    collected.and_then(|b| decompress_for_logging(&b, is_gzipped));
            }
        } else if is_loggable && content_length.is_some_and(|len| len <= MAX_LOGGABLE_BODY_SIZE) {
            // Non-chunked loggable content: buffer for logging, then send
            let len = content_length.unwrap();
            let body_already_read = response_buf.len() - header_end;
            let mut body = Vec::with_capacity(len);
            body.extend_from_slice(&response_buf[header_end..]);

            if body_already_read < len {
                let remaining = len - body_already_read;
                let mut rest = vec![0u8; remaining];
                upstream_conn.read_exact(&mut rest).await?;
                body.extend_from_slice(&rest);
            }

            log_entry.response_body_size = body.len();
            log_entry.response_body = decompress_for_logging(&body, is_gzipped);

            // Send headers (with processed cookies if any) and body
            client.write_all(&header_bytes).await?;
            client.write_all(&body).await?;
        } else {
            // Stream without buffering for large/binary responses
            client.write_all(&header_bytes).await?;

            let body_already_read_slice = &response_buf[header_end..];
            let body_already_read = body_already_read_slice.len();
            log_entry.response_body_size = body_already_read;

            // If blob capture enabled, capture to a temp file while streaming
            if let Some(temp_dir) = blob_temp_dir {
                if let Ok((path, mut capture_file)) = create_capture_file(temp_dir).await {
                    // Write already-read portion to both client and file
                    client.write_all(body_already_read_slice).await?;
                    capture_file.write_all(body_already_read_slice).await?;

                    // Stream remaining content
                    if let Some(len) = content_length {
                        if body_already_read < len {
                            let remaining = len - body_already_read;
                            stream_bytes_with_capture(
                                &mut upstream_conn,
                                client,
                                remaining,
                                &mut log_entry.response_body_size,
                                &mut capture_file,
                            )
                            .await?;
                        }
                    } else {
                        stream_until_eof_with_capture(
                            &mut upstream_conn,
                            client,
                            &mut log_entry.response_body_size,
                            &mut capture_file,
                        )
                        .await?;
                    }

                    capture_file.flush().await?;
                    log_entry.response_blob_temp_path = Some(path.to_string_lossy().to_string());
                    log_entry.original_response_headers = Some(headers.clone());
                } else {
                    // Failed to create capture file, fall back to regular streaming
                    client.write_all(body_already_read_slice).await?;
                    if let Some(len) = content_length {
                        if body_already_read < len {
                            let remaining = len - body_already_read;
                            stream_bytes(
                                &mut upstream_conn,
                                client,
                                remaining,
                                &mut log_entry.response_body_size,
                            )
                            .await?;
                        }
                    } else {
                        stream_until_eof(
                            &mut upstream_conn,
                            client,
                            &mut log_entry.response_body_size,
                        )
                        .await?;
                    }
                }
            } else {
                // No blob capture, stream normally
                client.write_all(body_already_read_slice).await?;
                if let Some(len) = content_length {
                    if body_already_read < len {
                        let remaining = len - body_already_read;
                        stream_bytes(
                            &mut upstream_conn,
                            client,
                            remaining,
                            &mut log_entry.response_body_size,
                        )
                        .await?;
                    }
                } else {
                    stream_until_eof(
                        &mut upstream_conn,
                        client,
                        &mut log_entry.response_body_size,
                    )
                    .await?;
                }
            }
        }
    } else {
        // Couldn't parse headers, just forward everything
        client.write_all(&response_buf).await?;
        stream_until_eof(
            &mut upstream_conn,
            client,
            &mut log_entry.response_body_size,
        )
        .await?;
    }

    client.flush().await?;
    Ok(())
}

/// Handle modify (fetch from upstream, modify response, return).
#[allow(clippy::too_many_arguments)]
async fn handle_modify<H: ProxyHandler>(
    client: &mut TcpStream,
    parsed: &crate::http::ParsedRequest,
    handler: &Arc<H>,
    upstream: &UpstreamConfig<'_>,
    tls_client: &TlsClient,
    log_entry: &mut RequestLogEntry,
    blob_temp_dir: Option<&PathBuf>,
    upstream_headers: &[(String, String)],
) -> Result<(), ProxyError> {
    let (host, port, is_https) = parse_upstream_url(upstream.url)?;
    // Log full upstream URL including path (or path_override) and cleaned query (without __upstream_host)
    let scheme = if is_https { "https" } else { "http" };
    let effective_path = upstream.path_override.unwrap_or(&parsed.request.path);
    let full_upstream_url = if let Some(q) = upstream.cleaned_query {
        format!("{}://{}{}?{}", scheme, host, effective_path, q)
    } else {
        format!("{}://{}{}", scheme, host, effective_path)
    };
    log_entry.upstream_url = Some(full_upstream_url);

    // Update logged request headers to show what we actually send to upstream (Host changed)
    for (k, v) in log_entry.request_headers.iter_mut() {
        if k.eq_ignore_ascii_case("host") {
            *v = host.clone();
        }
    }

    let upstream_start = Instant::now();

    // Connect to upstream (TLS for HTTPS, plain TCP for HTTP)
    let mut upstream_conn = tls_client.connect(&host, port, is_https).await?;

    // Build and send transformed request (with cleaned query, without __upstream_host param)
    let upstream_request = build_upstream_request(
        parsed,
        &host,
        upstream.cleaned_query,
        upstream.path_override,
        upstream_headers,
    );
    upstream_conn.write_all(&upstream_request).await?;

    // Forward request body if present (Content-Length or chunked)
    let body_start = parsed.headers_end;
    let already_read_body = &parsed.header_bytes[body_start..];

    // Check body size limit upfront for Content-Length
    if let Some(len) = parsed.request.content_length
        && len > MAX_REQUEST_BODY_SIZE
    {
        return Err(ProxyError::HttpParse(format!(
            "Request body too large: {} bytes exceeds {} byte limit",
            len, MAX_REQUEST_BODY_SIZE
        )));
    }

    let request_body = if parsed.request.is_chunked {
        // Read chunked request body with size limit
        Some(read_chunked_body_limited(client, already_read_body, MAX_REQUEST_BODY_SIZE).await?)
    } else if let Some(len) = parsed.request.content_length {
        let mut request_body = Vec::with_capacity(len);
        request_body.extend_from_slice(already_read_body);

        let already_read = request_body.len();
        if already_read < len {
            let remaining = len - already_read;
            let mut rest = vec![0u8; remaining];
            client.read_exact(&mut rest).await?;
            request_body.extend_from_slice(&rest);
        }
        Some(request_body)
    } else {
        None
    };

    if let Some(ref body) = request_body {
        upstream_conn.write_all(body).await?;
        log_entry.request_body_size = body.len();

        // Log request body if loggable
        if is_loggable_content_vec(&parsed.request.headers) && body.len() <= MAX_LOGGABLE_BODY_SIZE
        {
            let is_gzipped = is_gzip_encoded_vec(&parsed.request.headers);
            log_entry.request_body = decompress_for_logging(body, is_gzipped);
        }
    }

    upstream_conn.flush().await?;

    // Read response headers first
    let mut header_buf = BytesMut::with_capacity(4096);
    loop {
        let n = upstream_conn.read_buf(&mut header_buf).await?;
        if n == 0 {
            return Err(ProxyError::HttpParse(
                "Connection closed before response".to_string(),
            ));
        }
        if header_buf.windows(4).any(|w| w == b"\r\n\r\n") {
            break;
        }
    }

    log_entry.upstream_duration_ms = Some(upstream_start.elapsed().as_millis() as u32);

    // Parse response
    let (status, reason, _status_end) = parse_response_status(&header_buf)?;
    let (headers, body_start) = parse_response_headers(&header_buf)?;

    // Check transfer encoding and content length
    let is_chunked = headers.iter().any(|(k, v)| {
        k.eq_ignore_ascii_case("transfer-encoding") && v.to_lowercase().contains("chunked")
    });

    let content_length: Option<usize> =
        get_header(&headers, "Content-Length").and_then(|v| v.parse().ok());

    // Per HTTP spec, certain status codes MUST NOT have a body:
    // - 1xx (informational)
    // - 204 (No Content)
    // - 304 (Not Modified)
    let has_no_body = status < 200 || status == 204 || status == 304;

    let body = if has_no_body {
        // No body allowed for these status codes
        Vec::new()
    } else if is_chunked {
        // Read chunked body
        read_chunked_body(&mut upstream_conn, &header_buf[body_start..]).await?
    } else if let Some(len) = content_length {
        // Read exactly content_length bytes
        let body_already_read = header_buf.len() - body_start;
        let mut body = Vec::with_capacity(len);
        body.extend_from_slice(&header_buf[body_start..]);

        if body_already_read < len {
            let remaining = len - body_already_read;
            let mut rest = vec![0u8; remaining];
            upstream_conn.read_exact(&mut rest).await?;
            body.extend_from_slice(&rest);
        }
        body
    } else {
        // No Content-Length and not chunked - read until EOF
        let mut body = header_buf[body_start..].to_vec();
        upstream_conn.read_to_end(&mut body).await?;
        body
    };

    // Check if upstream response was gzipped
    let upstream_is_gzipped = is_gzip_encoded_vec(&headers);

    // Decompress body for handler if gzipped
    let decompressed_body = if upstream_is_gzipped {
        decompress_body(&body).unwrap_or_else(|| body.clone())
    } else {
        body.clone()
    };

    // Build ProxyResponse for handler with DECOMPRESSED body
    // (handler works with plain JSON, we handle compression transparently)
    let upstream_response = ProxyResponse {
        status,
        reason,
        headers: headers.clone(),
        body: crate::types::ResponseBody::Buffered(decompressed_body.clone()),
        source_file_path: None,
        on_sent: None,
    };

    // Log upstream response BEFORE modification (log the decompressed version)
    log_entry.upstream_status = Some(upstream_response.status);
    log_entry.upstream_headers = Some(upstream_response.headers.clone());
    log_entry.upstream_body_size = body.len(); // Log original compressed size
    if is_loggable_content_vec(&upstream_response.headers) {
        // Body is already decompressed, just convert to string
        log_entry.upstream_body = String::from_utf8(decompressed_body).ok();
    }

    // Call handler to modify (receives decompressed body)
    let mut modified = handler.modify(&parsed.request, upstream_response).await;

    // Log the modified response (before re-compression) - only for buffered bodies
    log_entry.status = modified.status;
    if is_loggable_content_vec(&modified.headers)
        && let Some(body_bytes) = modified.body.as_bytes()
    {
        log_entry.response_body = String::from_utf8(body_bytes.to_vec()).ok();
    }

    // Re-gzip if upstream was gzipped and body is buffered (maintain same encoding)
    if upstream_is_gzipped && let crate::types::ResponseBody::Buffered(ref data) = modified.body {
        let data_len = data.len();
        if let Some(compressed) = compress_body(data) {
            // Update Content-Length for compressed size
            modified.set_header("Content-Length", compressed.len().to_string());
            // Remove Transfer-Encoding if present (we're using Content-Length)
            modified.remove_header("transfer-encoding");
            modified.body = crate::types::ResponseBody::Buffered(compressed);
        } else {
            // Compression failed - remove Content-Encoding and send uncompressed
            modified.remove_header("content-encoding");
            modified.set_header("Content-Length", data_len.to_string());
        }
    }

    // Process Set-Cookie headers for proxy compatibility
    // - Duplicate cookies with domain stripped for same-origin access
    // - Kobo proxy runs over HTTP, so strip Secure flag too
    let has_cookies = modified
        .headers
        .iter()
        .any(|(k, _)| k.eq_ignore_ascii_case("set-cookie"));
    if has_cookies {
        modified.headers = process_cookies_in_headers(&modified.headers, false); // proxy is HTTP
    }

    // Update log with final response info
    log_entry.response_headers = modified.headers.clone();
    log_entry.response_body_size = modified.body.size() as usize;

    // Blob capture for binary responses (only if blob capture enabled and body is binary)
    if let Some(temp_dir) = blob_temp_dir
        && !is_loggable_content_vec(&modified.headers)
        && let Some(body_bytes) = modified.body.as_bytes()
        && !body_bytes.is_empty()
        && let Ok((path, mut capture_file)) = create_capture_file(temp_dir).await
        && capture_file.write_all(body_bytes).await.is_ok()
    {
        let _ = capture_file.flush().await;
        log_entry.response_blob_temp_path = Some(path.to_string_lossy().to_string());
        log_entry.original_response_headers = Some(modified.headers.clone());
    }

    // Send modified response, then run on_sent callback if delivery succeeded
    let on_sent = modified.on_sent.take();
    let result = send_response(client, modified).await;
    if result.is_ok()
        && let Some(cb) = on_sent
    {
        cb().await;
    }
    result
}

/// Read a chunked transfer-encoded body and decode it.
async fn read_chunked_body<R: tokio::io::AsyncRead + Unpin>(
    reader: &mut R,
    already_read: &[u8],
) -> Result<Vec<u8>, ProxyError> {
    read_chunked_body_limited(reader, already_read, usize::MAX).await
}

/// Read a chunked transfer-encoded body with size limit.
async fn read_chunked_body_limited<R: tokio::io::AsyncRead + Unpin>(
    reader: &mut R,
    already_read: &[u8],
    max_size: usize,
) -> Result<Vec<u8>, ProxyError> {
    let mut buf = BytesMut::from(already_read);
    let mut body = Vec::new();

    loop {
        // Find chunk size line
        while !buf.windows(2).any(|w| w == b"\r\n") {
            let n = reader.read_buf(&mut buf).await?;
            if n == 0 {
                return Err(ProxyError::HttpParse(
                    "Connection closed during chunked read".to_string(),
                ));
            }
        }

        // Parse chunk size (strip chunk extensions after semicolon)
        let line_end = buf.windows(2).position(|w| w == b"\r\n").unwrap();
        let size_str = String::from_utf8_lossy(&buf[..line_end]);
        let size_part = size_str.split(';').next().unwrap_or(&size_str);
        let chunk_size = usize::from_str_radix(size_part.trim(), 16)
            .map_err(|_| ProxyError::HttpParse(format!("Invalid chunk size: {}", size_str)))?;

        // Check size limit
        if body.len() + chunk_size > max_size {
            return Err(ProxyError::HttpParse(format!(
                "Chunked body exceeds size limit of {} bytes",
                max_size
            )));
        }

        // Remove the size line from buffer
        let _ = buf.split_to(line_end + 2);

        // Zero chunk means end
        if chunk_size == 0 {
            // Read trailing \r\n
            while buf.len() < 2 {
                let n = reader.read_buf(&mut buf).await?;
                if n == 0 {
                    break;
                }
            }
            break;
        }

        // Read chunk data + trailing \r\n
        let needed = chunk_size + 2;
        while buf.len() < needed {
            let n = reader.read_buf(&mut buf).await?;
            if n == 0 {
                return Err(ProxyError::HttpParse(
                    "Connection closed during chunk data".to_string(),
                ));
            }
        }

        // Append chunk data (without trailing \r\n)
        body.extend_from_slice(&buf[..chunk_size]);
        let _ = buf.split_to(needed);
    }

    Ok(body)
}

/// Stream chunked body to writer while collecting for logging.
///
/// This is a "tee" operation: chunks are forwarded to the client immediately
/// (preserving chunked encoding) while also being collected for logging.
/// Returns the collected body if `collect` is true and size doesn't exceed limit.
async fn tee_chunked_body<R, W>(
    reader: &mut R,
    writer: &mut W,
    already_read: &[u8],
    collect: bool,
    total_size: &mut usize,
) -> Result<Option<Vec<u8>>, ProxyError>
where
    R: tokio::io::AsyncRead + Unpin,
    W: tokio::io::AsyncWrite + Unpin,
{
    let mut buf = BytesMut::from(already_read);
    let mut collected = if collect { Some(Vec::new()) } else { None };

    loop {
        // Find chunk size line
        while !buf.windows(2).any(|w| w == b"\r\n") {
            let n = reader.read_buf(&mut buf).await?;
            if n == 0 {
                return Err(ProxyError::HttpParse(
                    "Connection closed during chunked read".to_string(),
                ));
            }
        }

        // Parse chunk size (strip chunk extensions after semicolon)
        let line_end = buf.windows(2).position(|w| w == b"\r\n").unwrap();
        let size_str = String::from_utf8_lossy(&buf[..line_end]);
        let size_part = size_str.split(';').next().unwrap_or(&size_str);
        let chunk_size = usize::from_str_radix(size_part.trim(), 16)
            .map_err(|_| ProxyError::HttpParse(format!("Invalid chunk size: {}", size_str)))?;

        // Forward chunk header to client
        writer.write_all(&buf[..line_end + 2]).await?;
        let _ = buf.split_to(line_end + 2);

        // Zero chunk means end
        if chunk_size == 0 {
            // Read and forward trailing \r\n
            while buf.len() < 2 {
                let n = reader.read_buf(&mut buf).await?;
                if n == 0 {
                    break;
                }
            }
            if buf.len() >= 2 {
                writer.write_all(&buf[..2]).await?;
            }
            break;
        }

        // Read chunk data + trailing \r\n
        let needed = chunk_size + 2;
        while buf.len() < needed {
            let n = reader.read_buf(&mut buf).await?;
            if n == 0 {
                return Err(ProxyError::HttpParse(
                    "Connection closed during chunk data".to_string(),
                ));
            }
        }

        // Forward chunk data + trailing \r\n to client
        writer.write_all(&buf[..needed]).await?;
        *total_size += chunk_size;

        // Collect for logging if enabled and under size limit
        if let Some(ref mut body) = collected {
            if body.len() + chunk_size <= MAX_LOGGABLE_BODY_SIZE {
                body.extend_from_slice(&buf[..chunk_size]);
            } else {
                // Exceeded limit, stop collecting but continue streaming
                collected = None;
            }
        }

        let _ = buf.split_to(needed);
    }

    writer.flush().await?;
    Ok(collected)
}

/// Stream bytes from reader to writer.
async fn stream_bytes<R, W>(
    reader: &mut R,
    writer: &mut W,
    count: usize,
    total: &mut usize,
) -> Result<(), ProxyError>
where
    R: tokio::io::AsyncRead + Unpin,
    W: tokio::io::AsyncWrite + Unpin,
{
    let mut remaining = count;
    let mut buf = [0u8; 8192];

    while remaining > 0 {
        let to_read = remaining.min(buf.len());
        let n = reader.read(&mut buf[..to_read]).await?;
        if n == 0 {
            break;
        }
        writer.write_all(&buf[..n]).await?;
        remaining -= n;
        *total += n;
    }

    Ok(())
}

/// Stream bytes until EOF.
async fn stream_until_eof<R, W>(
    reader: &mut R,
    writer: &mut W,
    total: &mut usize,
) -> Result<(), ProxyError>
where
    R: tokio::io::AsyncRead + Unpin,
    W: tokio::io::AsyncWrite + Unpin,
{
    let mut buf = [0u8; 8192];

    loop {
        let n = reader.read(&mut buf).await?;
        if n == 0 {
            break;
        }
        writer.write_all(&buf[..n]).await?;
        *total += n;
    }

    Ok(())
}

/// Stream bytes from reader to writer, also capturing to a file.
async fn stream_bytes_with_capture<R, W>(
    reader: &mut R,
    writer: &mut W,
    count: usize,
    total: &mut usize,
    capture_file: &mut BufWriter<File>,
) -> Result<(), ProxyError>
where
    R: tokio::io::AsyncRead + Unpin,
    W: tokio::io::AsyncWrite + Unpin,
{
    let mut remaining = count;
    let mut buf = [0u8; 8192];

    while remaining > 0 {
        let to_read = remaining.min(buf.len());
        let n = reader.read(&mut buf[..to_read]).await?;
        if n == 0 {
            break;
        }
        writer.write_all(&buf[..n]).await?;
        capture_file.write_all(&buf[..n]).await?;
        remaining -= n;
        *total += n;
    }

    capture_file.flush().await?;
    Ok(())
}

/// Stream bytes until EOF, also capturing to a file.
async fn stream_until_eof_with_capture<R, W>(
    reader: &mut R,
    writer: &mut W,
    total: &mut usize,
    capture_file: &mut BufWriter<File>,
) -> Result<(), ProxyError>
where
    R: tokio::io::AsyncRead + Unpin,
    W: tokio::io::AsyncWrite + Unpin,
{
    let mut buf = [0u8; 8192];

    loop {
        let n = reader.read(&mut buf).await?;
        if n == 0 {
            break;
        }
        writer.write_all(&buf[..n]).await?;
        capture_file.write_all(&buf[..n]).await?;
        *total += n;
    }

    capture_file.flush().await?;
    Ok(())
}

/// Create a temp file for blob capture in the given directory.
async fn create_capture_file(temp_dir: &PathBuf) -> Result<(PathBuf, BufWriter<File>), ProxyError> {
    let id = Uuid::new_v4();
    let path = temp_dir.join(format!("{}.tmp", id));

    // Ensure temp dir exists
    if let Err(e) = tokio::fs::create_dir_all(temp_dir).await {
        warn!(error = %e, "Failed to create blob temp dir");
    }

    let file = File::create(&path).await.map_err(ProxyError::Io)?;
    Ok((path, BufWriter::new(file)))
}

/// Send an HTTP response to the client (takes ownership to support streaming).
async fn send_response(client: &mut TcpStream, response: ProxyResponse) -> Result<(), ProxyError> {
    // Status line
    let status_line = format!("HTTP/1.1 {} {}\r\n", response.status, response.reason);
    client.write_all(status_line.as_bytes()).await?;

    // Headers
    for (name, value) in &response.headers {
        client
            .write_all(format!("{}: {}\r\n", name, value).as_bytes())
            .await?;
    }

    // End headers
    client.write_all(b"\r\n").await?;

    // Body - handle both buffered and streaming
    match response.body {
        crate::types::ResponseBody::Buffered(data) => {
            if !data.is_empty() {
                client.write_all(&data).await?;
            }
        }
        crate::types::ResponseBody::Stream { mut reader, .. } => {
            // Stream the body in chunks
            let mut buf = [0u8; 8192];
            loop {
                let n = reader.read(&mut buf).await?;
                if n == 0 {
                    break;
                }
                client.write_all(&buf[..n]).await?;
            }
        }
    }

    client.flush().await?;
    Ok(())
}

/// Send an error response to the client.
async fn send_error_response(
    client: &mut TcpStream,
    status: u16,
    message: &str,
) -> Result<(), ProxyError> {
    let response = ProxyResponse::json(status, &serde_json::json!({ "error": message }));
    send_response(client, response).await
}

/// Get first header value by name (case-insensitive) from Vec headers
fn get_header<'a>(headers: &'a [(String, String)], name: &str) -> Option<&'a str> {
    headers
        .iter()
        .find(|(k, _)| k.eq_ignore_ascii_case(name))
        .map(|(_, v)| v.as_str())
}

/// Check if content type indicates loggable text content (JSON, text, XML, etc.)
fn is_loggable_content_vec(headers: &[(String, String)]) -> bool {
    headers.iter().any(|(k, v)| {
        k.eq_ignore_ascii_case("content-type")
            && (v.contains("json")
                || v.contains("text/")
                || v.contains("xml")
                || v.contains("javascript"))
    })
}

/// Check if content is gzip encoded
fn is_gzip_encoded_vec(headers: &[(String, String)]) -> bool {
    headers
        .iter()
        .any(|(k, v)| k.eq_ignore_ascii_case("content-encoding") && v.contains("gzip"))
}

/// Check if client accepts gzip encoding
fn accepts_gzip_vec(headers: &[(String, String)]) -> bool {
    headers
        .iter()
        .any(|(k, v)| k.eq_ignore_ascii_case("accept-encoding") && v.contains("gzip"))
}

/// Decompress gzip data if needed, returning the decompressed string for logging
fn decompress_for_logging(body: &[u8], is_gzipped: bool) -> Option<String> {
    if is_gzipped {
        // Limit decompression to prevent gzip bombs
        let mut decoder = GzDecoder::new(body).take(MAX_DECOMPRESSED_SIZE as u64);
        let mut decompressed = String::new();
        match decoder.read_to_string(&mut decompressed) {
            Ok(_) => Some(decompressed),
            Err(_) => None, // Binary, invalid, or exceeded size limit - skip logging body
        }
    } else {
        String::from_utf8(body.to_vec()).ok()
    }
}

/// Decompress gzip body to bytes with size limit
fn decompress_body(body: &[u8]) -> Option<Vec<u8>> {
    // Limit decompression to prevent gzip bombs
    let mut decoder = GzDecoder::new(body).take(MAX_DECOMPRESSED_SIZE as u64);
    let mut decompressed = Vec::new();
    match decoder.read_to_end(&mut decompressed) {
        Ok(_) => Some(decompressed),
        Err(_) => None,
    }
}

/// Compress body with gzip
fn compress_body(body: &[u8]) -> Option<Vec<u8>> {
    let mut encoder = GzEncoder::new(Vec::new(), Compression::default());
    if encoder.write_all(body).is_err() {
        return None;
    }
    encoder.finish().ok()
}
