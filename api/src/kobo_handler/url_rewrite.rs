//! URL rewriting logic for proxying Kobo requests.

use kobo_proxy::{UPSTREAM_HOST_PARAM, is_kobo_domain};
use serde_json::Value;

/// Rewrite URLs in JSON to proxy external kobo domains through us.
///
/// Any URL pointing to a kobo domain will be rewritten to pass through our proxy
/// with the `__upstream_host` parameter.
///
/// When `rewrite_all` is false, URLs to the device's default upstream are skipped
/// (assuming they already go through the proxy via the configured api_endpoint).
///
/// When `rewrite_all` is true, ALL kobo domain URLs are rewritten (useful for debugging).
///
/// Example:
///   `https://storedownloads.kobo.com/download?token=xyz`
/// becomes:
///   `https://kobbler.example.com/kobo/{device_token}/download?token=xyz&__upstream_host=storedownloads.kobo.com`
pub fn rewrite_kobo_urls(
    json: &mut Value,
    proxy_base: &str,
    device_upstream: &str,
    rewrite_all: bool,
) {
    match json {
        Value::String(s) => {
            if let Some(rewritten) = try_rewrite_url(s, proxy_base, device_upstream, rewrite_all) {
                *s = rewritten;
            }
        }
        Value::Array(arr) => {
            for item in arr {
                rewrite_kobo_urls(item, proxy_base, device_upstream, rewrite_all);
            }
        }
        Value::Object(obj) => {
            for (_, v) in obj {
                rewrite_kobo_urls(v, proxy_base, device_upstream, rewrite_all);
            }
        }
        _ => {}
    }
}

/// Try to rewrite a URL string if it points to a kobo domain.
///
/// Returns `Some(rewritten_url)` if the URL should be rewritten, `None` otherwise.
/// When `rewrite_all` is true, URLs to the device's upstream are also rewritten.
pub fn try_rewrite_url(
    url: &str,
    proxy_base: &str,
    device_upstream: &str,
    rewrite_all: bool,
) -> Option<String> {
    // Only process http/https URLs
    let url_lower = url.to_lowercase();
    if !url_lower.starts_with("http://") && !url_lower.starts_with("https://") {
        return None;
    }

    // Parse the URL to extract the host
    let host_start = if url_lower.starts_with("https://") {
        8
    } else {
        7
    };

    let rest = &url[host_start..];
    let host_end = rest
        .find('/')
        .or_else(|| rest.find('?'))
        .unwrap_or(rest.len());
    let host_with_port = &rest[..host_end];

    // Strip port if present
    let host = host_with_port.split(':').next().unwrap_or(host_with_port);

    // Check if this is a kobo domain
    if !is_kobo_domain(host) {
        return None;
    }

    // Extract the device's default upstream host (from base URL like "https://storeapi.kobo.com")
    let device_upstream_host = device_upstream
        .strip_prefix("https://")
        .or_else(|| device_upstream.strip_prefix("http://"))
        .unwrap_or(device_upstream)
        .split('/')
        .next()
        .unwrap_or("")
        .split(':')
        .next()
        .unwrap_or("");

    // If the URL is to the same host as the device's upstream, skip unless rewrite_all is set
    // (normally requests to that host already go through our proxy via the configured api_endpoint)
    if !rewrite_all && host.eq_ignore_ascii_case(device_upstream_host) {
        return None;
    }

    // Calculate path start and check if URL has a meaningful path
    let path_start = host_start + host_end;

    // If URL is host-only (no path or just "/"), don't rewrite it.
    // These are "host" resources like dictionary_host, userguide_host where the device
    // constructs full URLs by appending paths. If we rewrite them with __upstream_host,
    // the device appends the path to our query string, breaking the URL.
    // Example: https://ereaderfiles.kobo.com -> device appends /ereader/guide/file.pdf
    // We can't proxy these without breaking the device's URL construction.
    if path_start >= url.len() {
        return None;
    }
    let path_and_query = &url[path_start..];
    if path_and_query == "/" {
        return None;
    }

    // Build new URL with __upstream_host param
    let encoded_host = urlencoding::encode(host);
    let new_url = if path_and_query.contains('?') {
        format!(
            "{}{}&{}={}",
            proxy_base, path_and_query, UPSTREAM_HOST_PARAM, encoded_host
        )
    } else {
        format!(
            "{}{}?{}={}",
            proxy_base, path_and_query, UPSTREAM_HOST_PARAM, encoded_host
        )
    };

    Some(new_url)
}
