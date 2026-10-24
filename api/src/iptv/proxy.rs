//! Stream proxy with failover.
//!
//! Proxies IPTV streams through the Kobbler server, providing:
//! - Source failover (tries multiple providers in priority order)
//! - Health score updates on failure
//! - HLS playlist rewriting to route segments through our proxy

use anyhow::{Context, Result, bail};
use axum::body::Body;
use axum::http::Response;
use dashmap::DashMap;
use futures_util::StreamExt;
use sqlx::PgPool;
use std::sync::LazyLock;
use std::time::{Duration, Instant};
use tracing::{debug, warn};
use uuid::Uuid;

use crate::repositories;

/// Cache of segment URL mappings for HLS rewriting.
/// Key: "{channel_id}/{segment_idx}", Value: (original_url, inserted_at).
static SEGMENT_CACHE: LazyLock<DashMap<String, (String, Instant)>> = LazyLock::new(DashMap::new);

/// TTL for segment cache entries (5 minutes).
const SEGMENT_TTL: Duration = Duration::from_secs(300);

/// Proxy a channel stream, trying sources in failover order.
pub async fn proxy_channel(
    pool: &PgPool,
    http: &reqwest::Client,
    channel_id: Uuid,
    format: &str,
) -> Result<Response<Body>> {
    let sources = repositories::iptv_sources::list_by_channel(pool, channel_id)
        .await
        .context("listing sources for channel")?;

    if sources.is_empty() {
        bail!("no sources available for channel {channel_id}");
    }

    // Get provider info for URL construction
    for source in &sources {
        if source.health_score <= 0.0 {
            continue;
        }

        // Check provider connection capacity before attempting
        if !super::provider_tracker::has_capacity(source.provider_id) {
            warn!(
                provider_id = %source.provider_id,
                "provider at connection limit, skipping"
            );
            continue;
        }

        let provider =
            match repositories::iptv_providers::get_by_id(pool, source.provider_id).await? {
                Some(p) => p,
                None => continue,
            };

        let client = super::xtream::XtreamClient::new(
            http.clone(),
            &provider.url,
            &provider.username,
            &provider.password,
        );
        let stream_url = client.stream_url(source.remote_stream_id, format);

        match try_proxy_stream(http, &stream_url, format, channel_id).await {
            Ok(response) => {
                super::provider_tracker::acquire(source.provider_id);
                return Ok(response);
            }
            Err(e) => {
                warn!(
                    source_id = %source.id,
                    stream_url = %stream_url,
                    error = %e,
                    "source failed, trying next"
                );
                // Decrement health score
                let new_score = (source.health_score - 0.2).max(0.0);
                let _ = repositories::iptv_sources::update_health(pool, source.id, new_score).await;
            }
        }
    }

    bail!("all sources exhausted for channel {channel_id}")
}

/// Proxy a single HLS segment by looking up its URL from the cache.
pub async fn proxy_segment(http: &reqwest::Client, segment_url: &str) -> Result<Response<Body>> {
    // Clean up expired entries periodically
    evict_stale_segments();

    let resp = http
        .get(segment_url)
        .timeout(Duration::from_secs(15))
        .send()
        .await
        .context("fetching segment")?
        .error_for_status()
        .context("segment response status")?;

    let content_type = resp
        .headers()
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("video/mp2t")
        .to_string();

    let stream = resp
        .bytes_stream()
        .map(|chunk| chunk.map_err(std::io::Error::other));
    let body = Body::from_stream(stream);

    Ok(Response::builder()
        .header("content-type", content_type)
        .body(body)
        .expect("valid response"))
}

/// Look up a cached segment URL by channel + index.
pub fn get_cached_segment_url(channel_id: Uuid, segment_idx: u32) -> Option<String> {
    let key = format!("{channel_id}/{segment_idx}");
    let entry = SEGMENT_CACHE.get(&key)?;
    let (url, inserted_at) = entry.value();
    if inserted_at.elapsed() > SEGMENT_TTL {
        drop(entry);
        SEGMENT_CACHE.remove(&key);
        return None;
    }
    Some(url.clone())
}

/// Try to proxy a stream from a specific URL.
async fn try_proxy_stream(
    http: &reqwest::Client,
    stream_url: &str,
    format: &str,
    channel_id: Uuid,
) -> Result<Response<Body>> {
    let resp = http
        .get(stream_url)
        .timeout(Duration::from_secs(86400))
        .send()
        .await
        .context("connecting to stream")?
        .error_for_status()
        .context("stream response status")?;

    match format {
        "m3u8" => rewrite_hls_playlist(resp, channel_id).await,
        _ => proxy_raw_stream(resp),
    }
}

/// Rewrite an HLS playlist, replacing segment URLs with our proxy endpoints.
async fn rewrite_hls_playlist(resp: reqwest::Response, channel_id: Uuid) -> Result<Response<Body>> {
    let base_url = resp.url().clone();
    let body = resp.text().await.context("reading HLS playlist")?;

    let mut rewritten = String::with_capacity(body.len());
    let mut segment_idx: u32 = 0;
    let mut has_endlist = false;
    let mut first_segment_url: Option<String> = None;

    for line in body.lines() {
        if line.starts_with("#EXT-X-ENDLIST") {
            has_endlist = true;
        }
        if line.starts_with('#') || line.trim().is_empty() {
            rewritten.push_str(line);
            rewritten.push('\n');
            continue;
        }

        // This is a segment URL line — resolve and cache it
        let absolute_url = if line.starts_with("http://") || line.starts_with("https://") {
            line.to_string()
        } else {
            base_url
                .join(line)
                .map(|u| u.to_string())
                .unwrap_or_else(|_| line.to_string())
        };

        if segment_idx == 0 {
            first_segment_url = Some(absolute_url.clone());
        }

        let key = format!("{channel_id}/{segment_idx}");
        SEGMENT_CACHE.insert(key, (absolute_url, Instant::now()));

        rewritten.push_str(&format!(
            "/api/v1/admin/iptv/channels/{channel_id}/play/{segment_idx}\n"
        ));
        segment_idx += 1;
    }

    // Reject placeholder playlists: a single segment plus #EXT-X-ENDLIST is
    // what some providers serve for streams they don't actually carry (e.g. a
    // black-screen .ts file). A real live playlist has multiple rolling
    // segments and never sets ENDLIST. Failing here lets the failover loop in
    // proxy_channel try the next source instead of streaming black to the player.
    if has_endlist && segment_idx <= 1 {
        bail!(
            "placeholder playlist detected (segments={segment_idx}, endlist=true, first_segment={:?})",
            first_segment_url
        );
    }

    debug!(
        channel_id = %channel_id,
        segments = segment_idx,
        "rewrote HLS playlist"
    );

    Ok(Response::builder()
        .header("content-type", "application/vnd.apple.mpegurl")
        .body(Body::from(rewritten))
        .expect("valid response"))
}

/// Proxy a raw stream (TS, etc.) through as-is.
fn proxy_raw_stream(resp: reqwest::Response) -> Result<Response<Body>> {
    let content_type = resp
        .headers()
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("video/mp2t")
        .to_string();

    let stream = resp
        .bytes_stream()
        .map(|chunk| chunk.map_err(std::io::Error::other));
    let body = Body::from_stream(stream);

    Ok(Response::builder()
        .header("content-type", content_type)
        .body(body)
        .expect("valid response"))
}

/// Evict expired segment cache entries. Called lazily during segment lookups.
fn evict_stale_segments() {
    SEGMENT_CACHE.retain(|_, (_, inserted_at)| inserted_at.elapsed() < SEGMENT_TTL);
}
