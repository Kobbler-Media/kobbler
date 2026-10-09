//! Minimal HTTP parsing - just enough for routing decisions.

use bytes::BytesMut;
use tokio::io::{AsyncRead, AsyncReadExt};

use crate::error::ProxyError;
use crate::types::ProxyRequest;

/// Maximum header size (16KB to handle large sync tokens)
const MAX_HEADERS_SIZE: usize = 16 * 1024;

/// Maximum number of headers to parse
const MAX_HEADERS: usize = 128;

/// Result of parsing HTTP headers from a stream.
pub struct ParsedRequest {
    /// The parsed request info
    pub request: ProxyRequest,

    /// Raw header bytes (for forwarding)
    pub header_bytes: BytesMut,

    /// Position where headers end (after \r\n\r\n)
    pub headers_end: usize,
}

/// Parse HTTP request headers from a stream.
///
/// Reads until we find the \r\n\r\n that terminates headers.
/// Does NOT read the body - that's handled separately for streaming.
pub async fn parse_request_headers<R: AsyncRead + Unpin>(
    stream: &mut R,
) -> Result<ParsedRequest, ProxyError> {
    let mut buf = BytesMut::with_capacity(4096);

    // Read until we find \r\n\r\n
    loop {
        if buf.len() >= MAX_HEADERS_SIZE {
            return Err(ProxyError::HttpParse("Headers too large".to_string()));
        }

        // Read more data
        let n = stream.read_buf(&mut buf).await?;
        if n == 0 {
            return Err(ProxyError::HttpParse(
                "Connection closed before headers complete".to_string(),
            ));
        }

        // Check for end of headers
        if let Some(pos) = find_header_end(&buf) {
            // Include the \r\n\r\n for httparse
            let request = parse_request_line_and_headers(&buf[..pos + 4])?;
            return Ok(ParsedRequest {
                request,
                header_bytes: buf,
                headers_end: pos + 4, // Include the \r\n\r\n
            });
        }
    }
}

/// Find the position of \r\n\r\n in a buffer.
fn find_header_end(buf: &[u8]) -> Option<usize> {
    buf.windows(4).position(|window| window == b"\r\n\r\n")
}

/// Parse request line and headers from a byte slice.
fn parse_request_line_and_headers(data: &[u8]) -> Result<ProxyRequest, ProxyError> {
    let mut headers_buf = [httparse::EMPTY_HEADER; MAX_HEADERS];
    let mut req = httparse::Request::new(&mut headers_buf);

    match req.parse(data)? {
        httparse::Status::Complete(_) => {}
        httparse::Status::Partial => {
            return Err(ProxyError::HttpParse("Incomplete headers".to_string()));
        }
    }

    let method = req
        .method
        .ok_or_else(|| ProxyError::HttpParse("Missing method".to_string()))?
        .to_string();

    let full_path = req
        .path
        .ok_or_else(|| ProxyError::HttpParse("Missing path".to_string()))?;

    // Split path and query
    let (path_part, query) = match full_path.find('?') {
        Some(pos) => (&full_path[..pos], Some(full_path[pos + 1..].to_string())),
        None => (full_path, None),
    };

    // Extract device token and strip /kobo/{token} prefix
    let (device_token, stripped_path) = extract_device_token(path_part);

    // Parse headers into Vec (preserves order)
    let mut headers = Vec::new();
    let mut content_length = None;
    let mut incoming_host = None;
    let mut is_chunked = false;

    for header in req.headers.iter() {
        let name = header.name.to_string();
        let value = String::from_utf8_lossy(header.value).to_string();

        if name.eq_ignore_ascii_case("content-length") {
            content_length = value.parse().ok();
        }

        if name.eq_ignore_ascii_case("host") {
            incoming_host = Some(value.clone());
        }

        if name.eq_ignore_ascii_case("transfer-encoding")
            && value.to_lowercase().contains("chunked")
        {
            is_chunked = true;
        }

        headers.push((name, value));
    }

    Ok(ProxyRequest {
        method,
        original_path: path_part.to_string(),
        path: stripped_path,
        device_token,
        query,
        headers,
        incoming_host,
        body: None,
        content_length,
        is_chunked,
    })
}

/// Extract device token from path and return stripped path.
///
/// Input: "/kobo/TOKEN123/v1/library/sync"
/// Output: (Some("TOKEN123"), "/v1/library/sync")
fn extract_device_token(path: &str) -> (Option<String>, String) {
    // Check if path starts with /kobo/
    if !path.starts_with("/kobo/") {
        return (None, path.to_string());
    }

    // Find the token (between /kobo/ and the next /)
    let after_kobo = &path[6..]; // Skip "/kobo/"

    match after_kobo.find('/') {
        Some(pos) => {
            let token = &after_kobo[..pos];
            let rest = &after_kobo[pos..];
            (Some(token.to_string()), rest.to_string())
        }
        None => {
            // Path is just /kobo/TOKEN with no trailing path
            (Some(after_kobo.to_string()), "/".to_string())
        }
    }
}

/// Build transformed request bytes for upstream.
///
/// Takes the original parsed request and pre-transformed headers, then:
/// 1. Replaces the path (strips /kobo/{token}), or uses path_override if provided
/// 2. Changes the Host header to the upstream host
/// 3. Uses the provided query string (with __upstream_host stripped)
/// 4. Filters out proxy-added headers
pub fn build_upstream_request(
    original: &ParsedRequest,
    upstream_host: &str,
    cleaned_query: Option<&str>,
    path_override: Option<&str>,
    headers: &[(String, String)],
) -> BytesMut {
    // Find request line end
    let data = &original.header_bytes[..original.headers_end];
    let line_end = data.windows(2).position(|w| w == b"\r\n").unwrap_or(0);

    // Parse original request line to get HTTP version
    let request_line = String::from_utf8_lossy(&data[..line_end]);
    let parts: Vec<&str> = request_line.split_whitespace().collect();
    let http_version = parts.get(2).unwrap_or(&"HTTP/1.1");

    // Build new request line with stripped path (or path_override) and cleaned query
    let base_path = path_override.unwrap_or(&original.request.path);
    let new_path = match cleaned_query {
        Some(q) => format!("{}?{}", base_path, q),
        None => base_path.to_string(),
    };
    let new_request_line = format!("{} {} {}", original.request.method, new_path, http_version);

    // Build new headers
    let mut result = BytesMut::with_capacity(original.header_bytes.len() + 256);
    result.extend_from_slice(new_request_line.as_bytes());
    result.extend_from_slice(b"\r\n");

    // Copy headers, replacing Host and filtering out proxy-added headers
    for (name, value) in headers {
        if is_proxy_header(name) {
            continue;
        }

        if name.eq_ignore_ascii_case("host") {
            result.extend_from_slice(format!("Host: {}\r\n", upstream_host).as_bytes());
        } else {
            result.extend_from_slice(format!("{}: {}\r\n", name, value).as_bytes());
        }
    }

    result.extend_from_slice(b"\r\n");
    result
}

/// Parse HTTP response status line.
///
/// Returns (status_code, reason_phrase, position after status line).
pub fn parse_response_status(data: &[u8]) -> Result<(u16, String, usize), ProxyError> {
    // Find end of status line
    let line_end = data
        .windows(2)
        .position(|w| w == b"\r\n")
        .ok_or_else(|| ProxyError::HttpParse("No status line found".to_string()))?;

    let status_line = String::from_utf8_lossy(&data[..line_end]);
    let parts: Vec<&str> = status_line.splitn(3, ' ').collect();

    if parts.len() < 2 {
        return Err(ProxyError::HttpParse("Invalid status line".to_string()));
    }

    let status: u16 = parts[1]
        .parse()
        .map_err(|_| ProxyError::HttpParse("Invalid status code".to_string()))?;

    let reason = parts.get(2).unwrap_or(&"").to_string();

    Ok((status, reason, line_end + 2))
}

/// Parse response headers from bytes.
///
/// Returns headers (as Vec to preserve duplicates) and position after \r\n\r\n.
pub fn parse_response_headers(data: &[u8]) -> Result<(Vec<(String, String)>, usize), ProxyError> {
    let header_end = find_header_end(data)
        .ok_or_else(|| ProxyError::HttpParse("No header end found".to_string()))?;

    let mut headers_buf = [httparse::EMPTY_HEADER; MAX_HEADERS];
    let mut resp = httparse::Response::new(&mut headers_buf);

    match resp.parse(data)? {
        httparse::Status::Complete(_) => {}
        httparse::Status::Partial => {
            return Err(ProxyError::HttpParse("Incomplete response".to_string()));
        }
    }

    let headers: Vec<(String, String)> = resp
        .headers
        .iter()
        .map(|h| {
            (
                h.name.to_string(),
                String::from_utf8_lossy(h.value).to_string(),
            )
        })
        .collect();

    Ok((headers, header_end + 4))
}

/// The query parameter name used to override the upstream host.
pub const UPSTREAM_HOST_PARAM: &str = "__upstream_host";

/// Parsed upstream host override with optional path.
///
/// When `__upstream_host` contains a path (e.g., "oauth.kobo.com/.well-known/openid-configuration"),
/// the host and path are separated so the path can be used for the upstream request.
#[derive(Debug, Clone, PartialEq)]
pub struct UpstreamOverride {
    pub host: String,
    pub path: Option<String>,
}

/// Extract the upstream host override from a query string.
///
/// Returns `Some(UpstreamOverride)` if `__upstream_host=value` is present in the query.
/// If the value contains a path (e.g., "oauth.kobo.com/.well-known/openid-configuration"),
/// separates host and path so both can be used correctly.
pub fn extract_upstream_host(query: Option<&str>) -> Option<UpstreamOverride> {
    let query = query?;
    let prefix = format!("{}=", UPSTREAM_HOST_PARAM);
    for part in query.split('&') {
        if let Some(value) = part.strip_prefix(&prefix) {
            // URL decode the value
            let decoded = urlencoding::decode(value).ok()?;
            if !decoded.is_empty() {
                // Split on first '/' to separate host from path
                if let Some(slash_pos) = decoded.find('/') {
                    let host = decoded[..slash_pos].to_string();
                    let path = decoded[slash_pos..].to_string();
                    return Some(UpstreamOverride {
                        host,
                        path: Some(path),
                    });
                } else {
                    return Some(UpstreamOverride {
                        host: decoded.into_owned(),
                        path: None,
                    });
                }
            }
        }
    }
    None
}

/// Strip the `__upstream_host` parameter from a query string.
///
/// Returns the query string without the parameter, or None if query becomes empty.
pub fn strip_upstream_host_param(query: Option<&str>) -> Option<String> {
    let query = query?;
    let prefix = format!("{}=", UPSTREAM_HOST_PARAM);
    let parts: Vec<&str> = query
        .split('&')
        .filter(|part| !part.starts_with(&prefix))
        .collect();

    if parts.is_empty() {
        None
    } else {
        Some(parts.join("&"))
    }
}

/// Check if a host is a valid Kobo domain.
///
/// Exact apex or a real subdomain only — a plain substring match (the former
/// implementation) lets `x.kobo.attacker.tld` through, which turns the proxy's
/// upstream-host override into an open SSRF redirector.
pub fn is_kobo_domain(host: &str) -> bool {
    let host = host.to_ascii_lowercase();
    host == "kobo.com"
        || host.ends_with(".kobo.com")
        || host == "kobobooks.com"
        || host.ends_with(".kobobooks.com")
}

/// Headers added by reverse proxies/CDNs that should be stripped when forwarding to upstream.
/// A real Kobo device would never send these.
const PROXY_HEADER_PREFIXES: &[&str] = &[
    "x-forwarded-", // nginx, apache, haproxy, traefik, caddy, etc.
    "cf-",          // Cloudflare (cf-connecting-ip, cf-ray, cf-ipcountry, etc.)
    "x-amzn-",      // AWS ALB/ELB (x-amzn-trace-id, etc.)
    "x-azure-",     // Azure (x-azure-clientip, x-azure-fdid, etc.)
    "x-envoy-",     // Envoy proxy
    "x-cloud-",     // Google Cloud (x-cloud-trace-context)
    "x-arr-",       // Azure ARR (x-arr-ssl, x-arr-log-id)
];

const PROXY_HEADER_EXACT: &[&str] = &[
    "x-real-ip",        // nginx
    "x-request-id",     // Various proxies
    "x-correlation-id", // Various proxies
    "x-client-ip",      // Various proxies
    "x-original-url",   // IIS/Azure
    "forwarded",        // RFC 7239 standard proxy header
    "via",              // HTTP standard proxy header
    "cdn-loop",         // CDN loop detection
    "true-client-ip",   // Akamai/Cloudflare
];

/// Check if a header is a proxy-added header that should be stripped.
fn is_proxy_header(name: &str) -> bool {
    let name_lower = name.to_lowercase();
    PROXY_HEADER_PREFIXES
        .iter()
        .any(|prefix| name_lower.starts_with(prefix))
        || PROXY_HEADER_EXACT.contains(&name_lower.as_str())
}

// =============================================================================
// Set-Cookie processing for proxy compatibility
// =============================================================================

/// Strip the domain attribute from a Set-Cookie header value.
///
/// Handles both `domain=.example.com` and `Domain=example.com` (case-insensitive).
/// Returns the cookie with the domain attribute removed.
pub fn strip_cookie_domain(cookie: &str) -> String {
    // Split cookie by semicolons, filter out domain attribute
    let parts: Vec<&str> = cookie
        .split(';')
        .map(|s| s.trim())
        .filter(|part| {
            !part.to_lowercase().starts_with("domain=")
                && !part.to_lowercase().starts_with("domain =")
        })
        .collect();

    parts.join("; ")
}

/// Strip the Secure flag from a Set-Cookie header value.
///
/// Removes the `Secure` attribute (case-insensitive) from the cookie.
pub fn strip_secure_flag(cookie: &str) -> String {
    let parts: Vec<&str> = cookie
        .split(';')
        .map(|s| s.trim())
        .filter(|part| !part.eq_ignore_ascii_case("secure"))
        .collect();

    parts.join("; ")
}

/// Process a Set-Cookie header for proxy compatibility.
///
/// Returns a Vec with potentially multiple cookies:
/// - Original cookie (unchanged, for upstream domain if device ever contacts it directly)
/// - Same-origin cookie (domain stripped, Secure adjusted for proxy protocol)
///
/// This ensures cookies work both through the proxy and with direct upstream access.
pub fn process_set_cookie_for_proxy(cookie: &str, proxy_is_https: bool) -> Vec<String> {
    let mut result = vec![cookie.to_string()]; // Keep original

    // Create same-origin version: strip domain, adjust Secure if needed
    let mut same_origin = strip_cookie_domain(cookie);
    if !proxy_is_https {
        same_origin = strip_secure_flag(&same_origin);
    }

    // Only add if different from original (avoid duplicate if cookie had no domain)
    if same_origin != cookie {
        result.push(same_origin);
    }

    result
}

/// Process all headers, expanding Set-Cookie headers for proxy compatibility.
///
/// Returns a new header list with Set-Cookie headers duplicated (original + same-origin).
pub fn process_cookies_in_headers(
    headers: &[(String, String)],
    proxy_is_https: bool,
) -> Vec<(String, String)> {
    let mut result = Vec::with_capacity(headers.len() * 2);

    for (name, value) in headers {
        if name.eq_ignore_ascii_case("set-cookie") {
            for cookie in process_set_cookie_for_proxy(value, proxy_is_https) {
                result.push((name.clone(), cookie));
            }
        } else {
            result.push((name.clone(), value.clone()));
        }
    }

    result
}

/// Build HTTP response header bytes from status and headers.
///
/// Returns bytes suitable for sending to client.
pub fn build_response_header_bytes(
    status: u16,
    reason: &str,
    headers: &[(String, String)],
) -> Vec<u8> {
    let mut buf = Vec::with_capacity(1024);

    // Status line
    buf.extend_from_slice(format!("HTTP/1.1 {} {}\r\n", status, reason).as_bytes());

    // Headers
    for (name, value) in headers {
        buf.extend_from_slice(format!("{}: {}\r\n", name, value).as_bytes());
    }

    // End of headers
    buf.extend_from_slice(b"\r\n");

    buf
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_extract_upstream_host() {
        // Host only (no path)
        assert_eq!(
            extract_upstream_host(Some(
                "foo=bar&__upstream_host=storedownloads.kobo.com&baz=qux"
            )),
            Some(UpstreamOverride {
                host: "storedownloads.kobo.com".to_string(),
                path: None
            })
        );
        assert_eq!(
            extract_upstream_host(Some("__upstream_host=cdn.kobo.com")),
            Some(UpstreamOverride {
                host: "cdn.kobo.com".to_string(),
                path: None
            })
        );
        assert_eq!(extract_upstream_host(Some("foo=bar")), None);
        assert_eq!(extract_upstream_host(None), None);
        // URL encoded value
        assert_eq!(
            extract_upstream_host(Some("__upstream_host=store%2Ekobo%2Ecom")),
            Some(UpstreamOverride {
                host: "store.kobo.com".to_string(),
                path: None
            })
        );
    }

    #[test]
    fn test_extract_upstream_host_with_path() {
        // Host with path - the key scenario for oauth.kobo.com/.well-known/openid-configuration
        assert_eq!(
            extract_upstream_host(Some(
                "__upstream_host=oauth.kobo.com/.well-known/openid-configuration"
            )),
            Some(UpstreamOverride {
                host: "oauth.kobo.com".to_string(),
                path: Some("/.well-known/openid-configuration".to_string())
            })
        );

        // Host with simple path
        assert_eq!(
            extract_upstream_host(Some("__upstream_host=api.kobo.com/v1/auth")),
            Some(UpstreamOverride {
                host: "api.kobo.com".to_string(),
                path: Some("/v1/auth".to_string())
            })
        );

        // URL encoded path
        assert_eq!(
            extract_upstream_host(Some(
                "__upstream_host=oauth.kobo.com%2F.well-known%2Fopenid-configuration"
            )),
            Some(UpstreamOverride {
                host: "oauth.kobo.com".to_string(),
                path: Some("/.well-known/openid-configuration".to_string())
            })
        );
    }

    #[test]
    fn test_strip_upstream_host_param() {
        assert_eq!(
            strip_upstream_host_param(Some("foo=bar&__upstream_host=x.kobo.com&baz=qux")),
            Some("foo=bar&baz=qux".to_string())
        );
        assert_eq!(
            strip_upstream_host_param(Some("__upstream_host=x.kobo.com")),
            None
        );
        assert_eq!(
            strip_upstream_host_param(Some("foo=bar")),
            Some("foo=bar".to_string())
        );
        assert_eq!(strip_upstream_host_param(None), None);
    }

    #[test]
    fn test_is_kobo_domain() {
        assert!(is_kobo_domain("storeapi.kobo.com"));
        assert!(is_kobo_domain("storedownloads.kobo.com"));
        assert!(is_kobo_domain("cdn.kobo.com"));
        assert!(is_kobo_domain("discovery.kobobooks.com"));
        assert!(is_kobo_domain("kobo.com"));
        assert!(is_kobo_domain("kobobooks.com"));
        // Case-insensitive: DNS hostnames aren't.
        assert!(is_kobo_domain("STOREAPI.KOBO.COM"));
        assert!(!is_kobo_domain("evil.com"));
        assert!(!is_kobo_domain("kobo.evil.com"));
        // `.contains(".kobo.")` used to pass these: any host with ".kobo."
        // or ".kobobooks." as a substring anywhere, not just as the real
        // apex — letting a proxy client point the Kobo proxy's upstream
        // override at a server of their choosing (SSRF).
        assert!(!is_kobo_domain("x.kobo.attacker.tld"));
        assert!(!is_kobo_domain("evil.kobo.attacker.com"));
        assert!(!is_kobo_domain("notkobo.com"));
        assert!(!is_kobo_domain("x.kobobooks.attacker.tld"));
        assert!(!is_kobo_domain("kobo.com.attacker.tld"));
    }

    #[test]
    fn test_is_proxy_header() {
        // Prefix-based proxy headers (various providers)
        assert!(is_proxy_header("X-Forwarded-For"));
        assert!(is_proxy_header("x-forwarded-proto"));
        assert!(is_proxy_header("CF-Connecting-IP"));
        assert!(is_proxy_header("cf-ray"));
        assert!(is_proxy_header("X-Amzn-Trace-Id"));
        assert!(is_proxy_header("X-Azure-ClientIP"));
        assert!(is_proxy_header("X-Envoy-External-Address"));
        assert!(is_proxy_header("X-Cloud-Trace-Context"));
        assert!(is_proxy_header("X-ARR-SSL"));

        // Exact-match proxy headers
        assert!(is_proxy_header("X-Real-IP"));
        assert!(is_proxy_header("x-real-ip"));
        assert!(is_proxy_header("Via"));
        assert!(is_proxy_header("Forwarded"));
        assert!(is_proxy_header("CDN-Loop"));
        assert!(is_proxy_header("True-Client-IP"));

        // NOT proxy headers (should be preserved)
        assert!(!is_proxy_header("Host"));
        assert!(!is_proxy_header("Accept"));
        assert!(!is_proxy_header("Authorization"));
        assert!(!is_proxy_header("x-kobo-deviceid"));
        assert!(!is_proxy_header("x-kobo-appversion"));
        assert!(!is_proxy_header("Content-Type"));
        assert!(!is_proxy_header("User-Agent"));
    }

    #[test]
    fn test_extract_device_token() {
        let (token, path) = extract_device_token("/kobo/ABC123/v1/library/sync");
        assert_eq!(token, Some("ABC123".to_string()));
        assert_eq!(path, "/v1/library/sync");

        let (token, path) = extract_device_token("/kobo/TOKEN/v1/initialization");
        assert_eq!(token, Some("TOKEN".to_string()));
        assert_eq!(path, "/v1/initialization");

        let (token, path) = extract_device_token("/other/path");
        assert_eq!(token, None);
        assert_eq!(path, "/other/path");
    }

    #[test]
    fn test_find_header_end() {
        assert_eq!(find_header_end(b"GET / HTTP/1.1\r\n\r\n"), Some(14));
        assert_eq!(
            find_header_end(b"GET / HTTP/1.1\r\nHost: x\r\n\r\n"),
            Some(23)
        );
        assert_eq!(find_header_end(b"incomplete"), None);
    }

    #[test]
    fn test_query_string_with_weak_etag() {
        // This is the actual query string from the failing request
        // The W/ in the ETag is NOT URL-encoded (forward slash is raw)
        let query = "DiffRequests=[%7BKey:EPD-KoboPlus-Subscribed,ETag:W/NjM4MzE4NjcxNjEwMDAwMDAwLTMyNDA4%7D]";

        // strip_upstream_host_param should pass it through unchanged
        assert_eq!(
            strip_upstream_host_param(Some(query)),
            Some(query.to_string())
        );

        // With an upstream host param, it should strip only that
        let with_upstream = format!("{}&__upstream_host=storeapi.kobo.com", query);
        assert_eq!(
            strip_upstream_host_param(Some(&with_upstream)),
            Some(query.to_string())
        );
    }

    #[test]
    fn test_build_upstream_request_preserves_query() {
        // Build a minimal ParsedRequest to test query preservation
        let raw_request = b"GET /kobo/TOKEN/v1/assets?DiffRequests=[%7BKey:EPD,ETag:W/abc%7D] HTTP/1.1\r\nHost: proxy.local\r\n\r\n";
        let headers = vec![("Host".to_string(), "proxy.local".to_string())];

        let parsed = ParsedRequest {
            request: ProxyRequest {
                method: "GET".to_string(),
                original_path: "/kobo/TOKEN/v1/assets".to_string(),
                path: "/v1/assets".to_string(),
                device_token: Some("TOKEN".to_string()),
                query: Some("DiffRequests=[%7BKey:EPD,ETag:W/abc%7D]".to_string()),
                headers,
                incoming_host: Some("proxy.local".to_string()),
                body: None,
                content_length: None,
                is_chunked: false,
            },
            header_bytes: BytesMut::from(&raw_request[..]),
            headers_end: raw_request.len(),
        };

        let upstream_request = build_upstream_request(
            &parsed,
            "storeapi.kobo.com",
            Some("DiffRequests=[%7BKey:EPD,ETag:W/abc%7D]"),
            None, // no path override
            &parsed.request.headers,
        );

        let request_str = String::from_utf8_lossy(&upstream_request);

        // The request line should have the stripped path with the full query
        assert!(
            request_str.contains("GET /v1/assets?DiffRequests=[%7BKey:EPD,ETag:W/abc%7D] HTTP/1.1"),
            "Request line should preserve query with W/ ETag. Got: {}",
            request_str
        );

        // Host header should be changed to upstream
        assert!(
            request_str.contains("Host: storeapi.kobo.com"),
            "Host header should be upstream. Got: {}",
            request_str
        );
    }

    #[test]
    fn test_build_upstream_request_strips_upstream_host_param() {
        let headers = vec![("Host".to_string(), "proxy.local".to_string())];

        let raw_request = b"GET /kobo/TOKEN/v1/assets?foo=bar&__upstream_host=other.kobo.com HTTP/1.1\r\nHost: proxy.local\r\n\r\n";

        let parsed = ParsedRequest {
            request: ProxyRequest {
                method: "GET".to_string(),
                original_path: "/kobo/TOKEN/v1/assets".to_string(),
                path: "/v1/assets".to_string(),
                device_token: Some("TOKEN".to_string()),
                query: Some("foo=bar&__upstream_host=other.kobo.com".to_string()),
                headers,
                incoming_host: Some("proxy.local".to_string()),
                body: None,
                content_length: None,
                is_chunked: false,
            },
            header_bytes: BytesMut::from(&raw_request[..]),
            headers_end: raw_request.len(),
        };

        // The cleaned query should have __upstream_host stripped
        let cleaned_query = strip_upstream_host_param(parsed.request.query.as_deref());
        assert_eq!(cleaned_query, Some("foo=bar".to_string()));

        let upstream_request = build_upstream_request(
            &parsed,
            "storeapi.kobo.com",
            cleaned_query.as_deref(),
            None, // no path override
            &parsed.request.headers,
        );

        let request_str = String::from_utf8_lossy(&upstream_request);

        // Should NOT contain __upstream_host in the forwarded request
        assert!(
            !request_str.contains("__upstream_host"),
            "Request should not contain __upstream_host param. Got: {}",
            request_str
        );

        // Should contain the remaining query param
        assert!(
            request_str.contains("?foo=bar"),
            "Request should contain remaining query params. Got: {}",
            request_str
        );
    }

    #[test]
    fn test_build_upstream_request_with_path_override() {
        // Test the key scenario: path extracted from __upstream_host
        let headers = vec![("Host".to_string(), "proxy.local".to_string())];

        // Original request has no path (just /)
        let raw_request = b"GET /kobo/TOKEN/ HTTP/1.1\r\nHost: proxy.local\r\n\r\n";

        let parsed = ParsedRequest {
            request: ProxyRequest {
                method: "GET".to_string(),
                original_path: "/kobo/TOKEN/".to_string(),
                path: "/".to_string(),
                device_token: Some("TOKEN".to_string()),
                query: None,
                headers,
                incoming_host: Some("proxy.local".to_string()),
                body: None,
                content_length: None,
                is_chunked: false,
            },
            header_bytes: BytesMut::from(&raw_request[..]),
            headers_end: raw_request.len(),
        };

        // With path override (e.g., from oauth.kobo.com/.well-known/openid-configuration)
        let upstream_request = build_upstream_request(
            &parsed,
            "oauth.kobo.com",
            None,
            Some("/.well-known/openid-configuration"),
            &parsed.request.headers,
        );

        let request_str = String::from_utf8_lossy(&upstream_request);

        // Request line should use the path override, not the original path
        assert!(
            request_str.contains("GET /.well-known/openid-configuration HTTP/1.1"),
            "Request should use path override. Got: {}",
            request_str
        );

        // Host header should be the upstream host
        assert!(
            request_str.contains("Host: oauth.kobo.com"),
            "Host header should be upstream. Got: {}",
            request_str
        );
    }

    #[test]
    fn test_parse_request_with_complex_query() {
        // Simulate parsing a request with the problematic query string
        let raw = b"GET /kobo/TOKEN/v1/assets?DiffRequests=[%7BKey:EPD-KoboPlus-Subscribed,ETag:W/NjM4MzE4NjcxNjEwMDAwMDAwLTMyNDA4%7D] HTTP/1.1\r\nHost: proxy.local\r\nAccept: application/json\r\n\r\n";

        let result = parse_request_line_and_headers(raw);
        assert!(result.is_ok(), "Should parse request with complex query");

        let req = result.unwrap();
        assert_eq!(req.method, "GET");
        assert_eq!(req.path, "/v1/assets");
        assert_eq!(req.device_token, Some("TOKEN".to_string()));

        // The query should be preserved exactly as-is
        assert_eq!(
            req.query,
            Some("DiffRequests=[%7BKey:EPD-KoboPlus-Subscribed,ETag:W/NjM4MzE4NjcxNjEwMDAwMDAwLTMyNDA4%7D]".to_string())
        );
    }

    #[test]
    fn test_build_upstream_request_filters_proxy_headers() {
        // Headers that should be filtered when forwarding to upstream
        // Using Vec preserves order - this also tests that order is maintained
        let headers = vec![
            ("Host".to_string(), "proxy.local".to_string()),
            ("Accept".to_string(), "application/json".to_string()),
            // nginx/standard proxy headers
            ("X-Forwarded-For".to_string(), "10.0.0.1".to_string()),
            ("X-Forwarded-Proto".to_string(), "https".to_string()),
            ("X-Forwarded-Scheme".to_string(), "https".to_string()),
            ("X-Real-IP".to_string(), "10.0.0.1".to_string()),
            // Cloudflare headers
            ("CF-Connecting-IP".to_string(), "1.2.3.4".to_string()),
            ("CF-Ray".to_string(), "abc123".to_string()),
            ("CF-IPCountry".to_string(), "US".to_string()),
            // AWS headers
            ("X-Amzn-Trace-Id".to_string(), "Root=1-abc".to_string()),
            // Azure headers
            ("X-Azure-ClientIP".to_string(), "1.2.3.4".to_string()),
            // Standard proxy headers
            ("Via".to_string(), "1.1 proxy.local".to_string()),
            ("Forwarded".to_string(), "for=1.2.3.4".to_string()),
            // Device headers that should be preserved
            ("x-kobo-deviceid".to_string(), "abc123".to_string()),
            ("x-kobo-appversion".to_string(), "4.38.23552".to_string()),
            ("Authorization".to_string(), "Bearer token".to_string()),
        ];

        let raw_request = b"GET /kobo/TOKEN/v1/sync HTTP/1.1\r\nHost: proxy.local\r\n\r\n";

        let parsed = ParsedRequest {
            request: ProxyRequest {
                method: "GET".to_string(),
                original_path: "/kobo/TOKEN/v1/sync".to_string(),
                path: "/v1/sync".to_string(),
                device_token: Some("TOKEN".to_string()),
                query: None,
                headers,
                incoming_host: Some("proxy.local".to_string()),
                body: None,
                content_length: None,
                is_chunked: false,
            },
            header_bytes: BytesMut::from(&raw_request[..]),
            headers_end: raw_request.len(),
        };

        let upstream_request = build_upstream_request(
            &parsed,
            "storeapi.kobo.com",
            None,
            None,
            &parsed.request.headers,
        );
        let request_str = String::from_utf8_lossy(&upstream_request);

        // These proxy-added headers should be filtered out
        assert!(
            !request_str.contains("X-Forwarded-"),
            "Should filter X-Forwarded-* headers"
        );
        assert!(
            !request_str.contains("X-Real-IP"),
            "Should filter X-Real-IP header"
        );
        assert!(
            !request_str.contains("CF-"),
            "Should filter CF-* (Cloudflare) headers"
        );
        assert!(
            !request_str.contains("X-Amzn-"),
            "Should filter X-Amzn-* (AWS) headers"
        );
        assert!(
            !request_str.contains("X-Azure-"),
            "Should filter X-Azure-* headers"
        );
        assert!(!request_str.contains("Via:"), "Should filter Via header");
        assert!(
            !request_str.contains("Forwarded:"),
            "Should filter Forwarded header"
        );

        // These headers should be preserved (device/legitimate headers)
        assert!(
            request_str.contains("Accept: application/json"),
            "Should preserve Accept header"
        );
        assert!(
            request_str.contains("x-kobo-deviceid: abc123"),
            "Should preserve x-kobo-deviceid"
        );
        assert!(
            request_str.contains("x-kobo-appversion: 4.38.23552"),
            "Should preserve x-kobo-appversion"
        );
        assert!(
            request_str.contains("Authorization: Bearer token"),
            "Should preserve Authorization"
        );
        assert!(
            request_str.contains("Host: storeapi.kobo.com"),
            "Should update Host header"
        );
    }

    // =========================================================================
    // Set-Cookie processing tests
    // =========================================================================

    #[test]
    fn test_strip_cookie_domain() {
        // Standard domain attribute
        assert_eq!(
            strip_cookie_domain("__cf_bm=abc; path=/; domain=.kobo.com; HttpOnly"),
            "__cf_bm=abc; path=/; HttpOnly"
        );

        // Domain with leading dot
        assert_eq!(
            strip_cookie_domain("session=xyz; Domain=.example.com; Path=/"),
            "session=xyz; Path=/"
        );

        // Case insensitive
        assert_eq!(
            strip_cookie_domain("token=123; DOMAIN=test.com; Secure"),
            "token=123; Secure"
        );

        // No domain attribute (unchanged except whitespace normalization)
        assert_eq!(
            strip_cookie_domain("simple=value; path=/"),
            "simple=value; path=/"
        );

        // Real Kobo cookie example
        let kobo_cookie = "__cf_bm=EdTDPGmTto.SZtVZN4sVLQE.p.9MJb1KxcQbmKrCDt8-1768320014-1.0.1.1-BJ3HkeFXtFHV7vRW3auQqy1TIL2EqNDM9pQxcIfjhn587QcADgZeTMqMUpiUN7FBC4TLsXg3Hiqt9tcK4A.W7N9TqBJTHwBKJ7qsTH7OKjM; path=/; expires=Tue, 13-Jan-26 16:30:14 GMT; domain=.kobo.com; HttpOnly; Secure; SameSite=None";
        let stripped = strip_cookie_domain(kobo_cookie);
        assert!(!stripped.contains("domain="), "Should not contain domain");
        assert!(stripped.contains("path=/"), "Should preserve path");
        assert!(stripped.contains("HttpOnly"), "Should preserve HttpOnly");
        assert!(stripped.contains("Secure"), "Should preserve Secure");
    }

    #[test]
    fn test_strip_secure_flag() {
        // Standard Secure flag
        assert_eq!(
            strip_secure_flag("token=abc; path=/; Secure; HttpOnly"),
            "token=abc; path=/; HttpOnly"
        );

        // Case insensitive
        assert_eq!(
            strip_secure_flag("session=xyz; SECURE; path=/"),
            "session=xyz; path=/"
        );

        // No Secure flag (unchanged)
        assert_eq!(
            strip_secure_flag("simple=value; path=/; HttpOnly"),
            "simple=value; path=/; HttpOnly"
        );

        // SameSite=None without Secure (edge case)
        assert_eq!(
            strip_secure_flag("token=abc; SameSite=None"),
            "token=abc; SameSite=None"
        );
    }

    #[test]
    fn test_process_set_cookie_for_proxy_http() {
        // Full Kobo cookie - HTTP proxy should strip domain AND Secure
        let cookie = "__cf_bm=abc; path=/; domain=.kobo.com; HttpOnly; Secure; SameSite=None";
        let result = process_set_cookie_for_proxy(cookie, false);

        assert_eq!(result.len(), 2, "Should return original + same-origin");
        assert_eq!(result[0], cookie, "First should be original");
        assert!(
            !result[1].contains("domain="),
            "Same-origin should not have domain"
        );
        assert!(
            !result[1].contains("Secure"),
            "Same-origin should not have Secure for HTTP proxy"
        );
        assert!(result[1].contains("HttpOnly"), "Should preserve HttpOnly");
    }

    #[test]
    fn test_process_set_cookie_for_proxy_https() {
        // With HTTPS proxy - keep Secure flag
        let cookie = "session=xyz; domain=.kobo.com; Secure; HttpOnly";
        let result = process_set_cookie_for_proxy(cookie, true);

        assert_eq!(result.len(), 2);
        assert!(
            result[1].contains("Secure"),
            "Same-origin should keep Secure for HTTPS proxy"
        );
        assert!(!result[1].contains("domain="), "Should still strip domain");
    }

    #[test]
    fn test_process_set_cookie_no_domain() {
        // Cookie without domain - should NOT duplicate
        let cookie = "simple=value; path=/; HttpOnly";
        let result = process_set_cookie_for_proxy(cookie, false);

        assert_eq!(
            result.len(),
            1,
            "Should not duplicate cookie without domain"
        );
        assert_eq!(result[0], cookie);
    }

    #[test]
    fn test_process_cookies_in_headers() {
        let headers = vec![
            ("Content-Type".to_string(), "application/json".to_string()),
            (
                "Set-Cookie".to_string(),
                "__cf_bm=abc; domain=.kobo.com; Secure".to_string(),
            ),
            ("X-Request-Id".to_string(), "123".to_string()),
            (
                "Set-Cookie".to_string(),
                "session=xyz; domain=.kobo.com; HttpOnly".to_string(),
            ),
        ];

        let processed = process_cookies_in_headers(&headers, false);

        // Should have: Content-Type, Set-Cookie x2 for first, X-Request-Id, Set-Cookie x2 for second
        // = 6 headers total
        assert_eq!(processed.len(), 6);

        // Count Set-Cookie headers
        let cookie_count = processed
            .iter()
            .filter(|(n, _)| n.eq_ignore_ascii_case("set-cookie"))
            .count();
        assert_eq!(
            cookie_count, 4,
            "Should have 4 Set-Cookie headers (2 original + 2 same-origin)"
        );

        // First Set-Cookie pair should be the originals, then same-origin versions
        let cookies: Vec<&String> = processed
            .iter()
            .filter(|(n, _)| n.eq_ignore_ascii_case("set-cookie"))
            .map(|(_, v)| v)
            .collect();

        assert!(
            cookies[0].contains("domain="),
            "First should be original with domain"
        );
        assert!(
            !cookies[1].contains("domain="),
            "Second should be same-origin without domain"
        );
    }

    #[test]
    fn test_build_response_header_bytes() {
        let headers = vec![
            ("Content-Type".to_string(), "text/plain".to_string()),
            ("Content-Length".to_string(), "5".to_string()),
        ];

        let bytes = build_response_header_bytes(200, "OK", &headers);
        let result = String::from_utf8(bytes).unwrap();

        assert!(result.starts_with("HTTP/1.1 200 OK\r\n"));
        assert!(result.contains("Content-Type: text/plain\r\n"));
        assert!(result.contains("Content-Length: 5\r\n"));
        assert!(result.ends_with("\r\n\r\n"));
    }
}
