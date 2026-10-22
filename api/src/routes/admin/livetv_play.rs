use axum::body::Body;
use axum::{
    Json,
    extract::{Path, Query, State},
    http::{StatusCode, header},
    response::IntoResponse,
};
use futures_util::StreamExt;
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::path::PathBuf;
use time::OffsetDateTime;
use tokio_util::io::ReaderStream;
use uuid::Uuid;

use crate::AppState;
use crate::events::LibraryEvent;
use crate::livetv::live_session::{self, SessionError};
use crate::livetv::tuner_lease::LeaseInfo;
use crate::repositories::{
    livetv_channels, livetv_guide, livetv_recordings, livetv_sessions, livetv_tuners,
};
use crate::routes::{AuthUser, PlaybackAuth};
use crate::streaming::decision::ClientCapabilities;

use super::livetv::{AppError, GuideEntryResponse, guide_entry_response, require_contributor};
use super::livetv_recordings::{context_for, recording_response};

impl From<SessionError> for AppError {
    fn from(e: SessionError) -> Self {
        match e {
            SessionError::ChannelNotFound => AppError::NotFound("Channel not found".to_string()),
            SessionError::TunerNotFound => AppError::NotFound("Tuner not found".to_string()),
            SessionError::SessionNotFound => AppError::NotFound("Session not found".to_string()),
            SessionError::NotTunable => AppError::BadRequest(e.to_string()),
            // Tuner exhaustion is a hard fail, never a degrade: there is no
            // second source to fall back to.
            SessionError::Exhausted(msg) => AppError::BadRequest(msg),
            SessionError::TuneFailed(msg) => AppError::BadGateway(msg),
            SessionError::Repository(e) => AppError::Internal(e.to_string()),
            SessionError::Other(e) => AppError::Internal(e.to_string()),
        }
    }
}

// =============================================================================
// Response types
// =============================================================================

#[derive(Debug, Serialize)]
pub struct LeaseResponse {
    id: Uuid,
    tuner_id: Uuid,
    channel_id: Uuid,
    channel_name: String,
    tuner_index: Option<i32>,
    holder_kind: String,
    session_id: Option<String>,
    recording_id: Option<Uuid>,
    user_id: Option<Uuid>,
    #[serde(with = "time::serde::rfc3339")]
    acquired_at: OffsetDateTime,
    #[serde(with = "time::serde::rfc3339::option")]
    expires_at: Option<OffsetDateTime>,
}

#[derive(Debug, Serialize)]
pub struct LiveSessionResponse {
    session_id: String,
    channel_id: Uuid,
    method: String,
    playlist_url: Option<String>,
    direct_url: Option<String>,
    video_codec: String,
    audio_codec: String,
    hd: bool,
    interlaced: bool,
    thumbnails_url: Option<String>,
    width: u32,
    height: u32,
    #[serde(with = "time::serde::rfc3339")]
    buffer_start_time: OffsetDateTime,
    is_live: bool,
    duration_seconds: f64,
    lease: LeaseResponse,
    tuners_in_use: u32,
    tuner_count: u32,
    now: Option<GuideEntryResponse>,
    next: Option<GuideEntryResponse>,
}

// =============================================================================
// Mapping
// =============================================================================

fn lease_response(info: &LeaseInfo, channel_name: String) -> LeaseResponse {
    LeaseResponse {
        id: info.id,
        tuner_id: info.tuner_id,
        channel_id: info.channel_id,
        channel_name,
        tuner_index: info.tuner_index,
        holder_kind: info.holder_kind.as_str().to_string(),
        session_id: info.session_id.clone(),
        recording_id: info.recording_id,
        user_id: info.user_id,
        acquired_at: info.acquired_at,
        expires_at: info.expires_at,
    }
}

async fn channel_name(state: &AppState, channel_id: Uuid) -> String {
    match livetv_channels::get_by_id(&state.pool, channel_id).await {
        Ok(Some(row)) => crate::livetv::classify::display_name(
            row.display_name.as_deref(),
            row.scan_name.as_deref(),
            row.callsign.as_deref(),
        ),
        _ => "Unknown".to_string(),
    }
}

async fn session_response(
    state: &AppState,
    result: live_session::LiveSessionResult,
) -> Result<LiveSessionResponse, AppError> {
    let now_ts = OffsetDateTime::now_utc();
    let (current, next) =
        livetv_guide::now_and_next(&state.pool, result.channel_id, now_ts).await?;
    let thumbnails_url = Some(format!(
        "/api/v1/admin/livetv/sessions/{}/thumbnails.vtt",
        result.session_id
    ));
    Ok(LiveSessionResponse {
        session_id: result.session_id,
        channel_id: result.channel_id,
        method: result.mode.as_str().to_string(),
        playlist_url: result.playlist_url,
        direct_url: result.direct_url,
        video_codec: result.facts.video_codec,
        audio_codec: result.facts.audio_codec,
        hd: result.facts.hd,
        interlaced: result.facts.interlaced,
        thumbnails_url,
        width: result.facts.width,
        height: result.facts.height,
        buffer_start_time: result.buffer_start_time,
        is_live: true,
        duration_seconds: 0.0,
        lease: lease_response(&result.lease, channel_name(state, result.channel_id).await),
        tuners_in_use: result.tuners_in_use,
        tuner_count: result.tuner_count,
        now: current
            .as_ref()
            .map(|g| guide_entry_response(g, now_ts, 0, None)),
        next: next
            .as_ref()
            .map(|g| guide_entry_response(g, now_ts, 0, None)),
    })
}

fn emit_lease_changed(state: &AppState, channel_id: Uuid, tuner_id: Uuid, capacity: u32) {
    let _ = state.library_events.send(LibraryEvent::LivetvLeaseChanged {
        channel_id,
        tuner_id,
        in_use: state.livetv.leases.in_use(tuner_id),
        capacity,
    });
}

// =============================================================================
// Leases
// =============================================================================

pub async fn list_leases(
    State(state): State<AppState>,
    user: AuthUser,
) -> Result<impl IntoResponse, AppError> {
    require_contributor(&user)?;

    let tuners = livetv_tuners::list_all(&state.pool).await?;
    let capacity: i64 = tuners.iter().map(|t| t.tuner_count.max(1) as i64).sum();

    let mut items = Vec::new();
    for info in state.livetv.leases.snapshot() {
        let name = channel_name(&state, info.channel_id).await;
        items.push(lease_response(&info, name));
    }

    Ok(Json(json!({
        "total": capacity,
        "in_use": items.len(),
        "items": items,
    })))
}

// =============================================================================
// Sessions
// =============================================================================

#[derive(Debug, Deserialize)]
pub struct CreateSessionInput {
    pub channel_id: Uuid,
    #[serde(default)]
    pub capabilities: Option<ClientCapabilities>,
}

pub async fn create_session(
    State(state): State<AppState>,
    user: AuthUser,
    Json(input): Json<CreateSessionInput>,
) -> Result<impl IntoResponse, AppError> {
    let caps = input.capabilities.unwrap_or_default();
    let result =
        live_session::create_session(&state, input.channel_id, user.user_id, &caps).await?;
    let tuner_id = result.lease.tuner_id;
    let capacity = result.tuner_count;
    let body = session_response(&state, result).await?;
    emit_lease_changed(&state, input.channel_id, tuner_id, capacity);
    Ok(Json(body))
}

#[derive(Debug, Deserialize)]
pub struct SwitchChannelInput {
    pub channel_id: Uuid,
    #[serde(default)]
    pub capabilities: Option<ClientCapabilities>,
}

pub async fn switch_channel(
    State(state): State<AppState>,
    user: AuthUser,
    Path(sid): Path<String>,
    Json(input): Json<SwitchChannelInput>,
) -> Result<impl IntoResponse, AppError> {
    let caps = input.capabilities.unwrap_or_default();
    let result =
        live_session::switch_channel(&state, &sid, input.channel_id, user.user_id, &caps).await?;
    let tuner_id = result.lease.tuner_id;
    let capacity = result.tuner_count;
    let body = session_response(&state, result).await?;
    emit_lease_changed(&state, input.channel_id, tuner_id, capacity);
    Ok(Json(body))
}

pub async fn destroy_session(
    State(state): State<AppState>,
    _user: AuthUser,
    Path(sid): Path<String>,
) -> Result<impl IntoResponse, AppError> {
    if !live_session::destroy_session(&state, &sid).await {
        return Err(AppError::NotFound("Session not found".to_string()));
    }
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Debug, Deserialize)]
pub struct HeartbeatInput {
    #[serde(default)]
    pub position_seconds: Option<f64>,
    #[serde(default)]
    pub source: Option<String>,
}

pub async fn heartbeat(
    State(state): State<AppState>,
    _user: AuthUser,
    Path(sid): Path<String>,
    Json(input): Json<HeartbeatInput>,
) -> Result<impl IntoResponse, AppError> {
    tune_still_alive(&state, &sid)?;
    if !live_session::heartbeat(&state, &sid, input.source.as_deref()).await {
        return Err(AppError::NotFound("Session not found".to_string()));
    }
    Ok(Json(json!({ "ok": true })))
}

/// A session whose tune died answers every request with why, until the viewer tunes
/// again — a 404 would read as "your session expired" and a client would retry into it.
fn tune_still_alive(state: &AppState, sid: &str) -> Result<(), AppError> {
    match live_session::session_failure(state, sid) {
        Some(reason) => Err(AppError::BadGateway(reason)),
        None => Ok(()),
    }
}

pub async fn buffer_status(
    State(state): State<AppState>,
    _user: AuthUser,
    Path(sid): Path<String>,
) -> Result<impl IntoResponse, AppError> {
    tune_still_alive(&state, &sid)?;
    let key = live_session::tune_for_session(&state, &sid)
        .ok_or_else(|| AppError::NotFound("Session not found".to_string()))?;
    let (dir, start, evicted, preempted_by, recording_id, tune_error) = {
        let tune = state
            .livetv
            .tunes
            .get(&key)
            .ok_or_else(|| AppError::NotFound("Session not found".to_string()))?;
        (
            tune.buffer_dir.clone(),
            tune.started_at,
            tune.lease.is_evicted(),
            tune.lease.flags.preempted_by(),
            tune.recording_id,
            tune.health.failure(),
        )
    };

    let runtime = state.runtime_config.get().await;
    let cap_seconds = runtime.livetv_buffer_cap_hours.clamp(1, 24) * 3600;

    let status = live_session::read_buffer(&dir)
        .await
        .map(|(_, s)| s)
        .unwrap_or_default();
    let end = start + time::Duration::seconds_f64(status.duration_seconds);

    Ok(Json(json!({
        "start_time": start.format(&time::format_description::well_known::Rfc3339).unwrap_or_default(),
        "end_time": end.format(&time::format_description::well_known::Rfc3339).unwrap_or_default(),
        "live_edge_seconds": status.duration_seconds,
        "duration_seconds": status.duration_seconds,
        "retained_seconds": status.retained_seconds,
        "cap_seconds": cap_seconds,
        "segments": status.segments,
        "recording_id": recording_id,
        // A scheduled recording on another channel takes the tuner: keep serving
        // the buffer, but jump-to-live is gone until a slot frees up.
        "preempted": evicted,
        "preempted_by_recording_id": preempted_by,
        "can_jump_to_live": !evicted,
        // Set only while the tune has never produced a byte: the reason, for the viewer.
        "tune_error": tune_error,
    })))
}

// =============================================================================
// Buffer playback — the HLS segment directory IS the buffer
// =============================================================================

fn buffer_dir_for(state: &AppState, sid: &str) -> Result<PathBuf, AppError> {
    live_session::buffer_dir_of(state, sid)
        .ok_or_else(|| AppError::NotFound("Session not found".to_string()))
}

// The session id is the credential on this and the three routes below. It only
// resolves through the live tune map, so an id that was never minted is a miss
// rather than a lookup, and it dies with the tune. Players cannot send an
// Authorization header, and requiring one only forces every segment through the
// client's localhost proxy.
pub async fn stream_playlist(
    State(state): State<AppState>,
    Path(sid): Path<String>,
) -> Result<axum::response::Response, AppError> {
    tune_still_alive(&state, &sid)?;
    let dir = buffer_dir_for(&state, &sid)?;
    let health = live_session::tune_health_of(&state, &sid);
    let waited = live_session::wait_for_first_segment(
        &dir,
        std::time::Duration::from_secs(8),
        health.as_deref(),
    )
    .await;
    let ready = waited.as_ref().is_some_and(|(_, s)| s.segments > 0);
    if !ready && let Some(reason) = health.as_ref().and_then(|h| h.failure()) {
        return Err(AppError::BadGateway(reason));
    }
    let Some((playlist, status)) = waited else {
        return Err(AppError::Internal("Buffer not ready".to_string()));
    };
    if status.segments == 0 {
        return Ok((StatusCode::SERVICE_UNAVAILABLE, "buffer not ready").into_response());
    }
    Ok(axum::response::Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, "application/vnd.apple.mpegurl")
        .header(header::CACHE_CONTROL, "no-store")
        .body(Body::from(playlist))
        .expect("valid playlist response"))
}

pub async fn serve_segment(
    State(state): State<AppState>,
    Path((sid, file)): Path<(String, String)>,
) -> Result<axum::response::Response, AppError> {
    let valid = !file.contains('/')
        && !file.contains('\\')
        && !file.contains("..")
        && ((file.starts_with("seg_") && file.ends_with(".m4s")) || file == "init.mp4");
    if !valid {
        return Err(AppError::BadRequest("invalid segment path".to_string()));
    }
    let path = buffer_dir_for(&state, &sid)?.join(&file);
    let f = tokio::fs::File::open(&path)
        .await
        .map_err(|_| AppError::NotFound("Segment not found".to_string()))?;
    let content_type = if file == "init.mp4" {
        "video/mp4"
    } else {
        "video/iso.segment"
    };
    Ok(axum::response::Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, content_type)
        .header(header::CACHE_CONTROL, "no-store")
        .body(Body::from_stream(ReaderStream::new(f)))
        .expect("valid segment response"))
}

pub async fn thumbnails_vtt(
    State(state): State<AppState>,
    Path(sid): Path<String>,
) -> Result<axum::response::Response, AppError> {
    let dir = buffer_dir_for(&state, &sid)?;
    let vtt = tokio::fs::read_to_string(dir.join("trickplay").join("thumbnails.vtt"))
        .await
        .map_err(|_| AppError::NotFound("No thumbnails yet".to_string()))?;
    // The buffer the client is scrubbing starts at the first segment retention left
    // behind, not at the one the trickplay pass started from.
    let dropped = live_session::read_buffer(&dir)
        .await
        .map(|(_, status)| status.dropped_seconds)
        .unwrap_or(0.0);
    let body = crate::livetv::trickplay::rebase_vtt(&vtt, dropped);
    Ok(axum::response::Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, "text/vtt")
        .header(header::CACHE_CONTROL, "no-store")
        .body(Body::from(body))
        .expect("valid vtt response"))
}

pub async fn serve_sprite(
    State(state): State<AppState>,
    Path((sid, file)): Path<(String, String)>,
) -> Result<axum::response::Response, AppError> {
    let valid = !file.contains('/')
        && !file.contains('\\')
        && !file.contains("..")
        && file.starts_with("sprite_")
        && file.ends_with(".jpg");
    if !valid {
        return Err(AppError::BadRequest("invalid sprite path".to_string()));
    }
    let path = buffer_dir_for(&state, &sid)?.join("trickplay").join(&file);
    let f = tokio::fs::File::open(&path)
        .await
        .map_err(|_| AppError::NotFound("Sprite not found".to_string()))?;
    Ok(axum::response::Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, "image/jpeg")
        .header(header::CACHE_CONTROL, "no-store")
        .body(Body::from_stream(ReaderStream::new(f)))
        .expect("valid sprite response"))
}

// =============================================================================
// DirectPlay — kobapp bypasses everything
// =============================================================================

#[derive(Debug, Deserialize)]
pub struct StreamTsQuery {
    #[serde(default)]
    pub session: Option<String>,
}

// Two very different requests share this route. With a `?session=` that resolves,
// the caller only subscribes to a tune someone already authorised and paid a
// tuner for, so the session id is credential enough and the player can fetch it
// directly instead of through its localhost proxy. Without one, the fall-through
// in `take_direct_stream` opens a *new* physical tuner — and there are four in the
// box — so that request still has to prove who it is.
pub async fn stream_ts(
    State(state): State<AppState>,
    user: Option<PlaybackAuth>,
    Path(id): Path<Uuid>,
    Query(q): Query<StreamTsQuery>,
) -> Result<axum::response::Response, AppError> {
    let rides_existing_tune = q
        .session
        .as_deref()
        .is_some_and(|sid| live_session::has_open_tune(&state, sid));
    if user.is_none() && !rides_existing_tune {
        if let Some(sid) = q.session.as_deref() {
            tune_still_alive(&state, sid)?;
        }
        return Err(AppError::Unauthorized);
    }
    if let Some(sid) = q.session.as_deref() {
        tune_still_alive(&state, sid)?;
    }
    let channel = livetv_channels::get_by_id(&state.pool, id)
        .await?
        .ok_or_else(|| AppError::NotFound("Channel not found".to_string()))?;
    let tuner = livetv_tuners::get_by_id(&state.pool, channel.tuner_id)
        .await?
        .ok_or_else(|| AppError::NotFound("Tuner not found".to_string()))?;
    if channel.lcn.is_none() {
        return Err(AppError::BadRequest(
            "Channel has no LCN to tune".to_string(),
        ));
    }
    let candidates = live_session::source_candidates(&state, &channel, &tuner).await;

    let (stream, content_type) =
        live_session::take_direct_stream(&state, q.session.as_deref(), &candidates)
            .await
            .map_err(AppError::BadGateway)?;

    Ok(axum::response::Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, content_type)
        .header(header::CACHE_CONTROL, "no-store")
        .body(Body::from_stream(
            stream.map(|r| r.map_err(|e| std::io::Error::other(e.to_string()))),
        ))
        .expect("valid ts response"))
}

// =============================================================================
// Record — promoting the buffer, retroactive to tune-in
// =============================================================================

#[derive(Debug, Deserialize)]
pub struct RecordInput {
    pub mode: String,
    #[serde(default)]
    pub guide_id: Option<Uuid>,
}

pub async fn record_session(
    State(state): State<AppState>,
    user: AuthUser,
    Path(sid): Path<String>,
    Json(input): Json<RecordInput>,
) -> Result<impl IntoResponse, AppError> {
    require_contributor(&user)?;
    if !matches!(input.mode.as_str(), "programme" | "open_ended") {
        return Err(AppError::BadRequest(
            "mode must be 'programme' or 'open_ended'".to_string(),
        ));
    }

    let session = livetv_sessions::get_by_session_id(&state.pool, &sid)
        .await?
        .ok_or_else(|| AppError::NotFound("Session not found".to_string()))?;
    if let Some(existing) = session.recording_id {
        let row = livetv_recordings::get_by_id(&state.pool, existing)
            .await?
            .ok_or_else(|| AppError::NotFound("Recording not found".to_string()))?;
        let mut ctx = context_for(&state, &row).await?;
        ctx.seconds_on_disk = buffer_seconds(&state, &sid).await;
        return Ok(Json(recording_response(row, ctx)));
    }

    let now = OffsetDateTime::now_utc();
    // Retroactive: the buffer already runs back to tune-in, so that is the start.
    let scheduled_start = live_session::buffer_start_of(&state, &sid).unwrap_or(session.started_at);
    // A guide entry that has already finished is a stale reference, not a request. The
    // player reads the session's `now` once at tune-in and keeps sending it, so forty
    // minutes into a watch this named a programme that was over — titling the recording
    // wrongly and, through `scheduled_end` below, ending it before it started. Resolve
    // what is actually on instead.
    let requested = match input.guide_id {
        Some(id) => livetv_guide::get_by_id(&state.pool, id).await?,
        None => None,
    };
    let guide = match requested {
        Some(g) if g.end_time > now => Some(g),
        _ => {
            livetv_guide::now_and_next(&state.pool, session.channel_id, now)
                .await?
                .0
        }
    };

    let runtime = state.runtime_config.get().await;
    let cap = time::Duration::hours(runtime.livetv_buffer_cap_hours.clamp(1, 24));
    let title = guide
        .as_ref()
        .map(|g| g.title.clone())
        .unwrap_or_else(|| "Live TV".to_string());
    let scheduled_end = match (input.mode.as_str(), guide.as_ref()) {
        ("programme", Some(g)) => g.end_time,
        _ => now + cap,
    };

    let row = livetv_recordings::create(
        &state.pool,
        livetv_recordings::CreateRecording {
            channel_id: session.channel_id,
            guide_id: guide.as_ref().map(|g| g.id),
            series_rule_id: None,
            title,
            episode_number: guide.as_ref().and_then(|g| g.episode_number.clone()),
            episode_title: guide.as_ref().and_then(|g| g.episode_title.clone()),
            episode_key: guide.as_ref().and_then(|g| g.episode_key.clone()),
            series_id: guide.as_ref().and_then(|g| g.series_id.clone()),
            synopsis: guide.as_ref().and_then(|g| g.synopsis.clone()),
            scheduled_start,
            scheduled_end,
            actual_start: Some(scheduled_start),
            status: "recording".to_string(),
            output_dir: Some(session.buffer_dir.clone()),
            container: Some("hls".to_string()),
            video_codec: Some("h264".to_string()),
            audio_codec: Some("aac".to_string()),
            padding_before_secs: 0,
            padding_after_secs: 0,
            image_url: guide
                .as_ref()
                .and_then(|g| g.image_url.clone().or_else(|| g.poster_url.clone())),
        },
    )
    .await?;

    live_session::promote_to_recording(&state, &sid, row.id).await?;

    let _ = state
        .library_events
        .send(LibraryEvent::LivetvRecordingUpdated {
            recording_id: row.id,
            status: row.status.clone(),
        });

    let mut ctx = context_for(&state, &row).await?;
    ctx.seconds_on_disk = buffer_seconds(&state, &sid).await;
    Ok(Json(recording_response(row, ctx)))
}

async fn buffer_seconds(state: &AppState, sid: &str) -> f64 {
    match live_session::buffer_dir_of(state, sid) {
        Some(dir) => live_session::read_buffer(&dir)
            .await
            .map(|(_, s)| s.duration_seconds)
            .unwrap_or(0.0),
        None => 0.0,
    }
}
