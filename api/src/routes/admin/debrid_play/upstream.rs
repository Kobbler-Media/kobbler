use std::sync::Arc;
use std::time::Duration;

use axum::body::Body;
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::{IntoResponse, Response};
use bytes::Bytes;
use futures::StreamExt;
use futures::stream::BoxStream;
use time::OffsetDateTime;
use tracing::{error, info, warn};
use uuid::Uuid;

use super::PlaySession;
use super::local::reply;
use crate::AppState;
use crate::ffmpeg::cue_gate::{CueGate, gated};
use crate::routes::admin::debrid_errors::StreamFailure;
use crate::routes::helpers::build_kobload_client;
use crate::streaming::byte_range::{ByteRange, content_range_start, parse_byte_range};
use crate::streaming::session::SessionError;

/// 5-minute buffer before considering a cached URL expired.
const URL_EXPIRY_BUFFER_SECS: i64 = 300;

/// The session's debrid URL if it is still good, without asking anyone.
///
/// URLs without an expiry are treated as non-expiring — Real-Debrid often omits it.
pub(super) fn cached_url(session: &PlaySession) -> Option<String> {
    let now = OffsetDateTime::now_utc();
    session
        .debrid_url
        .as_deref()
        .filter(|url| !url.is_empty())
        .filter(|_| {
            session
                .debrid_url_expires_at
                .is_none_or(|exp| exp > now + time::Duration::seconds(URL_EXPIRY_BUFFER_SECS))
        })
        .map(String::from)
}

/// A URL the debrid host will serve this session's file from, re-resolving through
/// Kobload when the cached one has lapsed. The error is the reply to send.
pub(crate) async fn playable_url(
    state: &AppState,
    session: &PlaySession,
) -> Result<String, Response> {
    if let Some(url) = cached_url(session) {
        return Ok(url);
    }

    let runtime = state.runtime_config.get().await;
    let client = build_kobload_client(&runtime).map_err(IntoResponse::into_response)?;
    let Some(debrid_provider_id) = session.provider_id() else {
        return Err((
            StatusCode::INTERNAL_SERVER_ERROR,
            "session has invalid debrid_provider (not a UUID)",
        )
            .into_response());
    };
    let resolve_req = nats_jobs::StreamResolveRequest {
        debrid_provider_id,
        info_hash: session.info_hash.clone(),
        magnet_url: None,
        nzb_url: None,
        file_id: session.file_id,
        priority: None,
    };

    let resolved = match client.stream_resolve(&resolve_req).await {
        Ok(r) => r,
        Err(e) => {
            error!(session_id = %session.id, error = %e, "failed to re-resolve stream URL");
            let failure = StreamFailure::from_kobload(&e);
            if matches!(failure, StreamFailure::Blocked | StreamFailure::NotCached) {
                fail_session(state, session.id, failure.code(), failure.message());
            }
            return Err(failure.response(StatusCode::BAD_GATEWAY, &e.to_string()));
        }
    };

    // Kobload answers cached=false with an empty URL while the provider is still
    // fetching the torrent; there is nothing to proxy yet.
    if resolved.stream_url.is_empty() {
        warn!(session_id = %session.id, "re-resolve returned empty URL (torrent not yet cached on provider)");
        return Err((
            StatusCode::SERVICE_UNAVAILABLE,
            "stream not ready — torrent is being fetched by debrid provider",
        )
            .into_response());
    }

    let expires_at = resolved.expires_at.as_deref().and_then(|s| {
        OffsetDateTime::parse(s, &time::format_description::well_known::Rfc3339).ok()
    });
    let _ = sqlx::query!(
        r#"
            UPDATE debrid_stream_sessions
            SET debrid_url = $1, debrid_url_expires_at = $2, updated_at = NOW()
            WHERE id = $3
            "#,
        resolved.stream_url,
        expires_at,
        session.id,
    )
    .execute(&state.pool)
    .await;
    if expires_at.is_some() {
        info!(session_id = %session.id, "Cached fresh debrid URL");
    } else {
        warn!(session_id = %session.id, "Resolved debrid URL has no expiry — caching without TTL");
    }
    Ok(resolved.stream_url)
}

pub(super) fn fail_session(state: &AppState, session_id: Uuid, code: &'static str, message: &str) {
    if let Some(mut s) = state.playback_sessions.get_mut(&session_id) {
        s.error = Some(SessionError::new(code, message));
    }
}

fn streaming_client() -> reqwest::Client {
    // No total timeout (transfers are large), but read_timeout catches debrid servers
    // that hang mid-stream.
    crate::http::builder()
        .timeout(Duration::MAX)
        .read_timeout(Duration::from_secs(10))
        .build()
        .expect("proxy stream client")
}

pub(super) enum Proxied {
    Served(Response),
    /// The host answered and said no — the link is dead or this file is barred.
    Refused(reqwest::StatusCode),
    Unreachable,
}

pub(super) async fn proxy_stream(
    url: &str,
    filesize: u64,
    content_type: &'static str,
    incoming_headers: &HeaderMap,
    gate: Arc<CueGate>,
) -> Proxied {
    let range = incoming_headers
        .get(header::RANGE)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| parse_byte_range(v, filesize));
    if let Some(range) = range
        && let Some(bytes) = gate.whole_range(range.start, range.length()).await
    {
        return Proxied::Served(reply(
            true,
            content_type,
            range,
            filesize,
            Body::from(bytes),
        ));
    }

    let mut req = streaming_client().get(url);
    if let Some(range) = incoming_headers.get(header::RANGE) {
        req = req.header(header::RANGE, range);
    }
    let upstream = match req.send().await {
        Ok(r) => r,
        Err(e) => {
            error!(error = %e, "failed to connect to debrid stream");
            return Proxied::Unreachable;
        }
    };

    let upstream_status = upstream.status();
    let status = if upstream_status == reqwest::StatusCode::PARTIAL_CONTENT {
        StatusCode::PARTIAL_CONTENT
    } else if upstream_status.is_success() {
        StatusCode::OK
    } else {
        error!(status = %upstream_status, "debrid upstream returned error");
        return Proxied::Refused(upstream_status);
    };
    let upstream_headers = upstream.headers().clone();

    let mut builder = Response::builder().status(status);
    for key in [
        header::CONTENT_TYPE,
        header::CONTENT_LENGTH,
        header::CONTENT_RANGE,
    ] {
        if let Some(val) = upstream_headers.get(key.as_str()) {
            builder = builder.header(key, val);
        }
    }
    if !upstream_headers.contains_key(header::CONTENT_TYPE.as_str()) {
        builder = builder.header(header::CONTENT_TYPE, content_type);
    }
    builder = builder.header(header::ACCEPT_RANGES, "bytes");
    if status == StatusCode::OK && !upstream_headers.contains_key(header::CONTENT_LENGTH.as_str()) {
        builder = builder.header(header::CONTENT_LENGTH, filesize.to_string());
    }

    // Where the response actually starts, which for a 206 is whatever upstream decided
    // to give us rather than whatever we asked for.
    let start = upstream_headers
        .get(header::CONTENT_RANGE)
        .and_then(|v| v.to_str().ok())
        .and_then(content_range_start)
        .unwrap_or(0);
    let body = Body::from_stream(gated(upstream.bytes_stream(), start, gate));
    Proxied::Served(builder.body(body).unwrap())
}

/// The rest of a response the local file could not finish, fetched from the debrid host.
pub(super) struct Remainder {
    pub state: AppState,
    pub session: Arc<PlaySession>,
}

impl Remainder {
    pub async fn open(
        &self,
        range: ByteRange,
    ) -> std::io::Result<BoxStream<'static, std::io::Result<Bytes>>> {
        let url = playable_url(&self.state, &self.session)
            .await
            .map_err(|r| std::io::Error::other(format!("no upstream URL ({})", r.status())))?;
        let resp = streaming_client()
            .get(&url)
            .header(
                reqwest::header::RANGE,
                format!("bytes={}-{}", range.start, range.end),
            )
            .send()
            .await
            .map_err(std::io::Error::other)?;
        let starts_at = resp
            .headers()
            .get(reqwest::header::CONTENT_RANGE)
            .and_then(|v| v.to_str().ok())
            .and_then(content_range_start);
        if resp.status() != reqwest::StatusCode::PARTIAL_CONTENT || starts_at != Some(range.start) {
            return Err(std::io::Error::other(format!(
                "upstream answered the remainder with {}",
                resp.status()
            )));
        }
        info!(session_id = %self.session.id, from = range.start, "local file stopped short, finishing from upstream");
        Ok(resp
            .bytes_stream()
            .map(|r| r.map_err(std::io::Error::other))
            .boxed())
    }
}
