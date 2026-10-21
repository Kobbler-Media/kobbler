//! GET /api/v1/admin/stream/{id}/play — the bytes of a debrid session's file.
//!
//! Served from the session's cache file wherever the progressive download has reached
//! (or is about to), and proxied from the debrid host otherwise.

mod gate;
mod local;
mod remux;
mod upstream;

pub(crate) use gate::{GateSpec, session_cue_gate};
pub(crate) use upstream::playable_url;

use std::path::{Path as FsPath, PathBuf};
use std::sync::Arc;

use axum::body::Body;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::{IntoResponse, Response};
use time::OffsetDateTime;
use tokio_util::sync::CancellationToken;
use tracing::{error, info, warn};
use uuid::Uuid;

use crate::AppState;
use crate::debrid_outcomes::{self, StreamRelease};
use crate::repositories::debrid_cache_files::{self as cache_files, CacheRelease};
use crate::routes::PlaybackAuth;
use crate::routes::admin::debrid_stream::session_owner;
use crate::streaming::byte_range::{ByteRange, parse_byte_range};
use crate::streaming::cache_download;
use crate::streaming::decision::PlaybackMethod;
use crate::streaming::download_meter::DownloadMeter;
use crate::streaming::feed::{FeedSource, FeedTicket, StreamFeed, metered};
use crate::streaming::progressive::DownloadTarget;

pub(crate) struct PlaySession {
    pub id: Uuid,
    pub debrid_provider_id: Option<Uuid>,
    pub info_hash: Option<String>,
    pub file_id: Option<i32>,
    pub debrid_provider: String,
    pub filename: String,
    pub filesize: i64,
    pub release_title: String,
    pub cache_path: Option<String>,
    pub debrid_url: Option<String>,
    pub debrid_url_expires_at: Option<OffsetDateTime>,
}

#[derive(Default)]
struct Live {
    feed: Option<Arc<StreamFeed>>,
    meter: Option<Arc<DownloadMeter>>,
    is_direct_play: bool,
    audio_stream: Option<i32>,
    resume: Option<CancellationToken>,
}

impl Live {
    fn serving_from(&self, source: FeedSource) {
        if let Some(f) = &self.feed {
            f.serving_from(source);
        }
    }
}

pub async fn stream_play(
    State(state): State<AppState>,
    auth: PlaybackAuth,
    Path(session_id): Path<Uuid>,
    headers: HeaderMap,
) -> Response {
    let session = match PlaySession::load(
        &state.pool,
        session_id,
        session_owner(auth.user_id, &auth.role),
    )
    .await
    {
        Ok(Some(s)) => Arc::new(s),
        Ok(None) => {
            warn!(session_id = %session_id, "stream_play: session not found in DB");
            return (StatusCode::NOT_FOUND, "session not found").into_response();
        }
        Err(e) => {
            error!(session_id = %session_id, error = %e, "stream_play: database error");
            return (StatusCode::INTERNAL_SERVER_ERROR, "database error").into_response();
        }
    };

    let live = touch(&state, &session);
    if let Some(cancel) = live.resume.clone() {
        resume_download(&state, &session, live.meter.clone(), cancel).await;
    }
    let ticket = live.feed.as_ref().map(StreamFeed::open);
    let resp = respond(&state, &session, &live, &headers).await;
    metered_response(resp, ticket)
}

/// Keep the in-memory session alive, and restart its download if the idle reaper
/// stopped it short.
fn touch(state: &AppState, session: &PlaySession) -> Live {
    let Some(mut ps) = state.playback_sessions.get_mut(&session.id) else {
        return Live::default();
    };
    ps.touch();
    let stopped = ps
        .download_cancel
        .as_ref()
        .is_some_and(|c| c.is_cancelled());
    let incomplete = || {
        let on_disk = session
            .cache_path
            .as_ref()
            .and_then(|p| std::fs::metadata(p).ok())
            .map_or(0, |m| m.len());
        on_disk < session.filesize.max(0) as u64
    };
    let resume = (stopped && incomplete()).then(|| {
        let fresh = CancellationToken::new();
        ps.download_cancel = Some(fresh.clone());
        fresh
    });
    Live {
        feed: ps.feed.clone(),
        meter: ps.download.clone(),
        is_direct_play: ps.decision.method == PlaybackMethod::DirectPlay,
        audio_stream: (!ps.audio_metas.is_empty()).then_some(ps.decision.audio_stream_index),
        resume,
    }
}

async fn resume_download(
    state: &AppState,
    session: &Arc<PlaySession>,
    meter: Option<Arc<DownloadMeter>>,
    cancel: CancellationToken,
) {
    let bg_state = state.clone();
    let session = session.clone();
    info!(session_id = %session.id, "Resuming stopped download");
    tokio::spawn(async move {
        let Ok(url) = upstream::playable_url(&bg_state, &session).await else {
            return;
        };
        let cache_path = PathBuf::from(session.cache_path.as_deref().unwrap_or(""));
        let on_disk = tokio::fs::metadata(&cache_path)
            .await
            .map_or(0, |m| m.len());
        let meter =
            meter.unwrap_or_else(|| DownloadMeter::new(session.filesize.max(0) as u64, on_disk));
        let target = session.download_target(url, cache_path);
        // Fire-and-forget cache warm; playback does not depend on it finishing.
        let _ = cache_download::download(&bg_state, target, cancel, meter).await;
    });
}

impl PlaySession {
    pub(crate) async fn load(
        pool: &sqlx::PgPool,
        session_id: Uuid,
        owner: Option<Uuid>,
    ) -> Result<Option<Self>, sqlx::Error> {
        let row = sqlx::query!(
            r#"
            SELECT id, debrid_provider_id, info_hash, file_id, debrid_provider, filename,
                   filesize, release_title,
                   cache_path AS "cache_path?",
                   debrid_url AS "debrid_url?",
                   debrid_url_expires_at AS "debrid_url_expires_at?"
            FROM debrid_stream_sessions
            WHERE id = $1 AND ($2::uuid IS NULL OR user_id = $2)
            "#,
            session_id,
            owner,
        )
        .fetch_optional(pool)
        .await?;
        Ok(row.map(|s| Self {
            id: s.id,
            debrid_provider_id: s.debrid_provider_id,
            info_hash: s.info_hash,
            file_id: s.file_id,
            debrid_provider: s.debrid_provider,
            filename: s.filename,
            filesize: s.filesize,
            release_title: s.release_title,
            cache_path: s.cache_path,
            debrid_url: s.debrid_url,
            debrid_url_expires_at: s.debrid_url_expires_at,
        }))
    }

    pub(crate) fn provider_id(&self) -> Option<Uuid> {
        self.debrid_provider_id
            .or_else(|| Uuid::parse_str(&self.debrid_provider).ok())
    }

    pub(crate) fn download_target(&self, url: String, cache_path: PathBuf) -> DownloadTarget {
        DownloadTarget {
            session_id: self.id,
            url,
            cache_path,
            filesize: self.filesize,
            release: CacheRelease {
                debrid_provider_id: self.provider_id(),
                info_hash: self.info_hash.clone(),
                file_id: self.file_id,
                filename: self.filename.clone(),
            },
        }
    }
}

async fn respond(
    state: &AppState,
    session: &Arc<PlaySession>,
    live: &Live,
    headers: &HeaderMap,
) -> Response {
    let total = session.filesize.max(0) as u64;
    let content_type = guess_video_content_type(&session.filename);
    // DirectPlay clients accept the raw container, so partial MKV/AVI bytes are fine.
    let remux = !live.is_direct_play && needs_remux(&session.filename);
    let local_path = session.cache_path.as_deref().map(PathBuf::from);

    if let Some(path) = &local_path
        && total > 0
        && let Ok(meta) = tokio::fs::metadata(path).await
        && meta.len() > 0
    {
        let on_disk = meta.len();
        let complete = on_disk >= total;
        if complete {
            info!(session_id = %session.id, "serving from complete local cache");
            let _ = cache_files::touch(&state.pool, &path.to_string_lossy()).await;
            if remux {
                live.serving_from(FeedSource::Local);
                return remux::remux_local_file(path, live.audio_stream).await;
            }
        }
        let (range, partial) = requested_range(headers, total);
        let reachable = complete
            || range.end < on_disk
            || live
                .meter
                .as_ref()
                .is_some_and(|m| m.is_live() && range.start <= m.reach().max(on_disk));
        if !remux && reachable {
            live.serving_from(FeedSource::Local);
            let gate = session_cue_gate(
                state,
                session.id,
                gate_spec(session, Some(path), upstream::cached_url(session)),
            )
            .await;
            return local::serve(local::LocalRead {
                path: path.clone(),
                total,
                range,
                partial,
                content_type,
                gate,
                meter: live.meter.clone(),
                feed: live.feed.clone(),
                remainder: upstream::Remainder {
                    state: state.clone(),
                    session: session.clone(),
                },
            })
            .await;
        }
    }

    let url = match upstream::playable_url(state, session).await {
        Ok(url) => url,
        Err(resp) => return resp,
    };
    let _ = sqlx::query!(
        "UPDATE debrid_stream_sessions SET updated_at = NOW() WHERE id = $1",
        session.id,
    )
    .execute(&state.pool)
    .await;

    let release = StreamRelease {
        debrid_provider_id: session.provider_id(),
        info_hash: session.info_hash.clone(),
        file_id: session.file_id,
        release_title: session.release_title.clone(),
        filename: session.filename.clone(),
    };
    if remux {
        live.serving_from(FeedSource::Proxy);
        let resp = remux::remux_proxy_stream(&url, live.audio_stream).await;
        if resp.status() == StatusCode::BAD_GATEWAY {
            debrid_outcomes::report_failure(state, &release, "upstream_failed").await;
        }
        return resp;
    }

    let gate = session_cue_gate(
        state,
        session.id,
        gate_spec(session, local_path.as_deref(), Some(url.clone())),
    )
    .await;
    match upstream::proxy_stream(&url, total, content_type, headers, gate).await {
        upstream::Proxied::Served(resp) => {
            live.serving_from(FeedSource::Proxy);
            resp
        }
        upstream::Proxied::Refused(status) => {
            if status.is_client_error()
                && status != reqwest::StatusCode::RANGE_NOT_SATISFIABLE
                && status != reqwest::StatusCode::TOO_MANY_REQUESTS
            {
                upstream::fail_session(
                    state,
                    session.id,
                    "upstream_refused",
                    &format!("The debrid host refused the file ({status})"),
                );
            }
            debrid_outcomes::report_failure(state, &release, "upstream_failed").await;
            (StatusCode::BAD_GATEWAY, "upstream error").into_response()
        }
        upstream::Proxied::Unreachable => {
            debrid_outcomes::report_failure(state, &release, "upstream_failed").await;
            (StatusCode::BAD_GATEWAY, "upstream connection failed").into_response()
        }
    }
}

fn gate_spec<'a>(
    session: &'a PlaySession,
    local: Option<&'a FsPath>,
    url: Option<String>,
) -> GateSpec<'a> {
    GateSpec {
        info_hash: session.info_hash.as_deref(),
        filename: &session.filename,
        filesize: session.filesize,
        local,
        url,
    }
}

/// The range to serve and whether it is a 206. A missing or unusable Range header is
/// answered with the whole file.
fn requested_range(headers: &HeaderMap, total: u64) -> (ByteRange, bool) {
    let whole = ByteRange {
        start: 0,
        end: total - 1,
    };
    match headers
        .get(header::RANGE)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| parse_byte_range(v, total))
    {
        Some(range) => (range, true),
        None => (whole, false),
    }
}

fn metered_response(resp: Response, ticket: Option<FeedTicket>) -> Response {
    let Some(ticket) = ticket.filter(|_| resp.status().is_success()) else {
        return resp;
    };
    let (parts, body) = resp.into_parts();
    Response::from_parts(
        parts,
        Body::from_stream(metered(body.into_data_stream(), ticket)),
    )
}

fn guess_video_content_type(filename: &str) -> &'static str {
    let ext = filename.rsplit('.').next().unwrap_or("").to_lowercase();
    match ext.as_str() {
        "mp4" | "m4v" => "video/mp4",
        "mkv" => "video/x-matroska",
        "webm" => "video/webm",
        "avi" => "video/x-msvideo",
        "mov" => "video/quicktime",
        "ts" => "video/mp2t",
        "wmv" => "video/x-ms-wmv",
        _ => "video/mp4",
    }
}

/// Container formats that browsers cannot natively play and need an on-the-fly remux to
/// fragmented MP4.
fn needs_remux(filename: &str) -> bool {
    let ext = filename.rsplit('.').next().unwrap_or("").to_lowercase();
    matches!(ext.as_str(), "mkv" | "avi" | "ts" | "m2ts" | "flv" | "wmv")
}
