use axum::body::Body;
use axum::{
    Json,
    extract::{Path, Query, State},
    http::{StatusCode, header},
    response::IntoResponse,
};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::collections::HashMap;
use time::OffsetDateTime;
use tokio_util::io::ReaderStream;
use uuid::Uuid;

use crate::AppState;
use crate::events::LibraryEvent;
use crate::livetv::recorder;
use crate::repositories::{livetv_ad_breaks, livetv_channels, livetv_guide, livetv_recordings};
use crate::routes::AuthUser;

use super::livetv::{AppError, require_contributor};
use super::livetv_watch::AdBreakResponse;

#[derive(Debug, Serialize)]
pub struct RecordingResponse {
    pub id: Uuid,
    pub channel_id: Uuid,
    pub channel_name: String,
    pub guide_id: Option<Uuid>,
    pub series_rule_id: Option<Uuid>,
    pub title: String,
    pub episode_number: Option<String>,
    pub episode_title: Option<String>,
    pub episode_key: Option<String>,
    pub series_id: Option<String>,
    pub synopsis: Option<String>,
    #[serde(with = "time::serde::rfc3339")]
    pub scheduled_start: OffsetDateTime,
    #[serde(with = "time::serde::rfc3339")]
    pub scheduled_end: OffsetDateTime,
    #[serde(with = "time::serde::rfc3339::option")]
    pub actual_start: Option<OffsetDateTime>,
    #[serde(with = "time::serde::rfc3339::option")]
    pub actual_end: Option<OffsetDateTime>,
    pub padding_before_secs: i32,
    pub padding_after_secs: i32,
    pub status: String,
    pub tuner_index: Option<i32>,
    pub output_dir: Option<String>,
    pub duration_seconds: Option<f64>,
    pub video_codec: Option<String>,
    pub audio_codec: Option<String>,
    pub container: Option<String>,
    pub size_bytes: Option<i64>,
    pub episode_id: Option<Uuid>,
    pub movie_id: Option<Uuid>,
    pub moved_to_guide_id: Option<Uuid>,
    pub image_url: Option<String>,
    pub error: Option<String>,
    #[serde(with = "time::serde::rfc3339")]
    pub created_at: OffsetDateTime,
    #[serde(with = "time::serde::rfc3339")]
    pub updated_at: OffsetDateTime,
    pub is_in_progress: bool,
    pub seconds_on_disk: f64,
    pub playable_from_start: bool,
    pub ad_breaks: Vec<AdBreakResponse>,
    pub provenance: RecordingProvenance,
    /// The effective posture, not the raw setting: the settings endpoint is
    /// admin-only, so a contributor's player can only learn its own posture from
    /// the payload it is already fetching.
    pub ad_posture: String,
    pub ad_skip_disabled: bool,
}

/// Where the file came from, kept as a struct rather than a `json!` literal: an
/// untyped payload is how `lcn` came to be read as a string by two clients.
#[derive(Debug, Serialize)]
pub struct RecordingProvenance {
    pub channel_name: String,
    pub lcn: Option<i32>,
    pub aired: String,
}

/// Everything the response needs that is not on the recording row itself.
pub struct RecordingContext {
    pub channel_name: String,
    pub lcn: Option<i32>,
    pub seconds_on_disk: f64,
    pub ad_posture: String,
    pub ad_skip_disabled: bool,
    pub ad_breaks: Vec<AdBreakResponse>,
}

pub fn recording_response(
    row: livetv_recordings::RecordingRow,
    ctx: RecordingContext,
) -> RecordingResponse {
    let is_in_progress = row.status == "recording";
    let aired = row
        .actual_start
        .unwrap_or(row.scheduled_start)
        .format(&time::format_description::well_known::Rfc3339)
        .unwrap_or_default();
    RecordingResponse {
        provenance: RecordingProvenance {
            channel_name: ctx.channel_name.clone(),
            lcn: ctx.lcn,
            aired,
        },
        is_in_progress,
        seconds_on_disk: ctx.seconds_on_disk,
        // HLS from segment one, so it is watchable from the start while the back
        // half is still being written.
        playable_from_start: true,
        ad_breaks: ctx.ad_breaks,
        ad_posture: ctx.ad_posture,
        ad_skip_disabled: ctx.ad_skip_disabled,
        channel_name: ctx.channel_name,
        id: row.id,
        channel_id: row.channel_id,
        guide_id: row.guide_id,
        series_rule_id: row.series_rule_id,
        title: row.title,
        episode_number: row.episode_number,
        episode_title: row.episode_title,
        episode_key: row.episode_key,
        series_id: row.series_id,
        synopsis: row.synopsis,
        scheduled_start: row.scheduled_start,
        scheduled_end: row.scheduled_end,
        actual_start: row.actual_start,
        actual_end: row.actual_end,
        padding_before_secs: row.padding_before_secs,
        padding_after_secs: row.padding_after_secs,
        status: row.status,
        tuner_index: row.tuner_index,
        output_dir: row.output_dir,
        duration_seconds: row.duration_seconds,
        video_codec: row.video_codec,
        audio_codec: row.audio_codec,
        container: row.container,
        size_bytes: row.size_bytes,
        episode_id: row.episode_id,
        movie_id: row.movie_id,
        moved_to_guide_id: row.moved_to_guide_id,
        image_url: row.image_url,
        error: row.error,
        created_at: row.created_at,
        updated_at: row.updated_at,
    }
}

/// Ad skip collapses on news and live sport, so the posture is served already
/// resolved against the disabled-genre list rather than left to the client.
pub fn ad_skip_disabled_for(genres: &[String], disabled: &[String]) -> bool {
    genres
        .iter()
        .any(|g| disabled.iter().any(|d| d.eq_ignore_ascii_case(g)))
}

async fn seconds_on_disk(row: &livetv_recordings::RecordingRow) -> f64 {
    if let Some(d) = row.duration_seconds {
        return d;
    }
    let Some(dir) = row.output_dir.as_deref() else {
        return 0.0;
    };
    recorder::read_playlist(std::path::Path::new(dir), false, "segments/")
        .await
        .map(|(_, s)| s.duration_seconds)
        .unwrap_or(0.0)
}

async fn build_contexts(
    state: &AppState,
    rows: &[livetv_recordings::RecordingRow],
) -> Result<Vec<RecordingContext>, AppError> {
    let runtime = state.runtime_config.get().await;
    let channel_ids: Vec<Uuid> = rows.iter().map(|r| r.channel_id).collect();
    let labels: HashMap<Uuid, (String, Option<i32>)> =
        livetv_channels::labels_for(&state.pool, &channel_ids)
            .await?
            .into_iter()
            .map(|(id, name, lcn)| (id, (name, lcn)))
            .collect();
    let guide_ids: Vec<Uuid> = rows.iter().filter_map(|r| r.guide_id).collect();
    let genres: HashMap<Uuid, Vec<String>> = livetv_guide::genres_for(&state.pool, &guide_ids)
        .await?
        .into_iter()
        .collect();

    let recording_ids: Vec<Uuid> = rows.iter().map(|r| r.id).collect();
    let mut breaks: HashMap<Uuid, Vec<AdBreakResponse>> = HashMap::new();
    for b in livetv_ad_breaks::list_for_recordings(&state.pool, &recording_ids).await? {
        breaks
            .entry(b.recording_id)
            .or_default()
            .push(super::livetv_watch::ad_break_response(b));
    }

    let mut out = Vec::with_capacity(rows.len());
    for row in rows {
        let (name, lcn) = labels
            .get(&row.channel_id)
            .cloned()
            .unwrap_or_else(|| ("Unknown".to_string(), None));
        let entry_genres = row
            .guide_id
            .and_then(|id| genres.get(&id))
            .cloned()
            .unwrap_or_default();
        out.push(RecordingContext {
            channel_name: name,
            lcn,
            seconds_on_disk: seconds_on_disk(row).await,
            ad_posture: runtime.livetv_ad_posture.clone(),
            ad_skip_disabled: ad_skip_disabled_for(
                &entry_genres,
                &runtime.livetv_ad_disable_genres,
            ),
            ad_breaks: breaks.remove(&row.id).unwrap_or_default(),
        });
    }
    Ok(out)
}

pub async fn context_for(
    state: &AppState,
    row: &livetv_recordings::RecordingRow,
) -> Result<RecordingContext, AppError> {
    Ok(build_contexts(state, std::slice::from_ref(row))
        .await?
        .pop()
        .expect("one context per row"))
}

// =============================================================================
// Recordings
// =============================================================================

#[derive(Debug, Deserialize)]
pub struct RecordingQuery {
    pub status: Option<String>,
    pub from: Option<String>,
    pub to: Option<String>,
    pub limit: Option<i64>,
    pub offset: Option<i64>,
}

fn parse_time(raw: Option<&String>) -> Result<Option<OffsetDateTime>, AppError> {
    match raw {
        None => Ok(None),
        Some(s) => OffsetDateTime::parse(s, &time::format_description::well_known::Rfc3339)
            .map(Some)
            .map_err(|_| AppError::BadRequest(format!("'{s}' is not an RFC3339 timestamp"))),
    }
}

pub async fn list_recordings(
    State(state): State<AppState>,
    user: AuthUser,
    Query(q): Query<RecordingQuery>,
) -> Result<impl IntoResponse, AppError> {
    require_contributor(&user)?;
    let filters = livetv_recordings::RecordingFilters {
        status: q.status.clone(),
        from: parse_time(q.from.as_ref())?,
        to: parse_time(q.to.as_ref())?,
        series_rule_id: None,
    };
    let limit = q.limit.unwrap_or(50).clamp(1, 500);
    let offset = q.offset.unwrap_or(0).max(0);
    let (rows, total) =
        livetv_recordings::list_filtered(&state.pool, &filters, limit, offset).await?;
    let contexts = build_contexts(&state, &rows).await?;
    let items: Vec<RecordingResponse> = rows
        .into_iter()
        .zip(contexts)
        .map(|(row, ctx)| recording_response(row, ctx))
        .collect();
    Ok(Json(json!({ "items": items, "total": total })))
}

#[derive(Debug, Deserialize)]
pub struct CreateRecordingInput {
    #[serde(default)]
    pub guide_id: Option<Uuid>,
    #[serde(default)]
    pub channel_id: Option<Uuid>,
    #[serde(default)]
    pub start_time: Option<String>,
    #[serde(default)]
    pub end_time: Option<String>,
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub padding_before_secs: Option<i32>,
    #[serde(default)]
    pub padding_after_secs: Option<i32>,
}

pub async fn create_recording(
    State(state): State<AppState>,
    user: AuthUser,
    Json(input): Json<CreateRecordingInput>,
) -> Result<impl IntoResponse, AppError> {
    require_contributor(&user)?;

    let padding_before = input.padding_before_secs.unwrap_or(120).clamp(0, 3600);
    let padding_after = input.padding_after_secs.unwrap_or(360).clamp(0, 7200);

    let create = if let Some(guide_id) = input.guide_id {
        if let Some(existing) = livetv_recordings::get_by_guide_id(&state.pool, guide_id).await? {
            let ctx = context_for(&state, &existing).await?;
            return Ok((StatusCode::OK, Json(recording_response(existing, ctx))));
        }
        let entry = livetv_guide::get_by_id(&state.pool, guide_id)
            .await?
            .ok_or_else(|| AppError::NotFound("Guide entry not found".to_string()))?;
        livetv_recordings::CreateRecording {
            channel_id: entry.channel_id,
            guide_id: Some(entry.id),
            series_rule_id: None,
            title: entry.title.clone(),
            episode_number: entry.episode_number.clone(),
            episode_title: entry.episode_title.clone(),
            episode_key: entry.episode_key.clone(),
            series_id: entry.series_id.clone(),
            synopsis: entry.synopsis.clone(),
            scheduled_start: entry.start_time,
            scheduled_end: entry.end_time,
            actual_start: None,
            status: "scheduled".to_string(),
            output_dir: None,
            container: Some("hls".to_string()),
            video_codec: None,
            audio_codec: None,
            padding_before_secs: padding_before,
            padding_after_secs: padding_after,
            image_url: entry.image_url.clone().or_else(|| entry.poster_url.clone()),
        }
    } else {
        let channel_id = input.channel_id.ok_or_else(|| {
            AppError::BadRequest("guide_id or channel_id is required".to_string())
        })?;
        let start = parse_time(input.start_time.as_ref())?
            .ok_or_else(|| AppError::BadRequest("start_time is required".to_string()))?;
        let end = parse_time(input.end_time.as_ref())?
            .ok_or_else(|| AppError::BadRequest("end_time is required".to_string()))?;
        if end <= start {
            return Err(AppError::BadRequest(
                "end_time must be after start_time".to_string(),
            ));
        }
        if livetv_channels::get_by_id(&state.pool, channel_id)
            .await?
            .is_none()
        {
            return Err(AppError::NotFound("Channel not found".to_string()));
        }
        livetv_recordings::CreateRecording {
            channel_id,
            guide_id: None,
            series_rule_id: None,
            title: input.title.clone().unwrap_or_else(|| "Live TV".to_string()),
            episode_number: None,
            episode_title: None,
            episode_key: None,
            series_id: None,
            synopsis: None,
            scheduled_start: start,
            scheduled_end: end,
            actual_start: None,
            status: "scheduled".to_string(),
            output_dir: None,
            container: Some("hls".to_string()),
            video_codec: None,
            audio_codec: None,
            padding_before_secs: padding_before,
            padding_after_secs: padding_after,
            image_url: None,
        }
    };

    let row = livetv_recordings::create(&state.pool, create).await?;
    let _ = state
        .library_events
        .send(LibraryEvent::LivetvRecordingUpdated {
            recording_id: row.id,
            status: row.status.clone(),
        });
    let ctx = context_for(&state, &row).await?;
    Ok((StatusCode::CREATED, Json(recording_response(row, ctx))))
}

async fn load(state: &AppState, id: Uuid) -> Result<livetv_recordings::RecordingRow, AppError> {
    livetv_recordings::get_by_id(&state.pool, id)
        .await?
        .ok_or_else(|| AppError::NotFound("Recording not found".to_string()))
}

pub async fn get_recording(
    State(state): State<AppState>,
    user: AuthUser,
    Path(id): Path<Uuid>,
) -> Result<impl IntoResponse, AppError> {
    require_contributor(&user)?;
    let row = load(&state, id).await?;
    let ctx = context_for(&state, &row).await?;
    Ok(Json(recording_response(row, ctx)))
}

pub async fn stop_recording(
    State(state): State<AppState>,
    user: AuthUser,
    Path(id): Path<Uuid>,
) -> Result<impl IntoResponse, AppError> {
    require_contributor(&user)?;
    let row = load(&state, id).await?;
    livetv_recordings::cancel(&state.pool, row.id).await?;
    let row = load(&state, id).await?;
    let _ = state
        .library_events
        .send(LibraryEvent::LivetvRecordingUpdated {
            recording_id: row.id,
            status: row.status.clone(),
        });
    let ctx = context_for(&state, &row).await?;
    Ok(Json(recording_response(row, ctx)))
}

pub async fn delete_recording(
    State(state): State<AppState>,
    user: AuthUser,
    Path(id): Path<Uuid>,
) -> Result<impl IntoResponse, AppError> {
    require_contributor(&user)?;
    let row = load(&state, id).await?;
    livetv_recordings::cancel(&state.pool, row.id).await?;
    // A promoted live buffer is a session directory the sweeper owns; only a
    // scheduled capture's own directory is ours to remove.
    if let Some(dir) = row.output_dir.as_deref()
        && std::path::Path::new(dir)
            .file_name()
            .is_some_and(|n| n == row.id.to_string().as_str())
    {
        let _ = tokio::fs::remove_dir_all(dir).await;
    }
    livetv_recordings::delete(&state.pool, row.id).await?;
    let _ = state
        .library_events
        .send(LibraryEvent::LivetvRecordingUpdated {
            recording_id: row.id,
            status: "deleted".to_string(),
        });
    Ok(StatusCode::NO_CONTENT)
}

// =============================================================================
// Playback — the segment directory IS the recording
// =============================================================================

pub async fn recording_playback(
    State(state): State<AppState>,
    user: AuthUser,
    Path(id): Path<Uuid>,
) -> Result<impl IntoResponse, AppError> {
    require_contributor(&user)?;
    let row = load(&state, id).await?;
    let dir = row
        .output_dir
        .clone()
        .ok_or_else(|| AppError::BadRequest("Nothing has been captured yet".to_string()))?;
    let finished = row.status != "recording";
    let (_, status) = recorder::read_playlist(std::path::Path::new(&dir), finished, "segments/")
        .await
        .ok_or_else(|| AppError::BadRequest("Nothing has been captured yet".to_string()))?;
    let ctx = context_for(&state, &row).await?;

    Ok(Json(json!({
        "session_id": row.id,
        "method": "remux",
        "playlist_url": format!("/api/v1/admin/livetv/recordings/{id}/stream.m3u8"),
        "direct_url": null,
        "duration_seconds": status.duration_seconds,
        "subtitles": [],
        "audio_tracks": [],
        "media_id": row.id,
        "media_type": "recording",
        "ad_posture": ctx.ad_posture,
        "ad_skip_disabled": ctx.ad_skip_disabled,
    })))
}

pub async fn recording_playlist(
    State(state): State<AppState>,
    user: AuthUser,
    Path(id): Path<Uuid>,
) -> Result<axum::response::Response, AppError> {
    require_contributor(&user)?;
    let row = load(&state, id).await?;
    let dir = row
        .output_dir
        .clone()
        .ok_or_else(|| AppError::NotFound("Nothing has been captured yet".to_string()))?;
    let finished = row.status != "recording";
    let (playlist, _) = recorder::read_playlist(std::path::Path::new(&dir), finished, "segments/")
        .await
        .ok_or_else(|| AppError::NotFound("Nothing has been captured yet".to_string()))?;
    Ok(axum::response::Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, "application/vnd.apple.mpegurl")
        .header(header::CACHE_CONTROL, "no-store")
        .body(Body::from(playlist))
        .expect("valid playlist response"))
}

pub async fn recording_segment(
    State(state): State<AppState>,
    user: AuthUser,
    Path((id, file)): Path<(Uuid, String)>,
) -> Result<axum::response::Response, AppError> {
    require_contributor(&user)?;
    let valid = !file.contains("..")
        && !file.starts_with('/')
        && file
            .split('/')
            .all(|seg| seg == "init.mp4" || seg.starts_with("part_") || seg.starts_with("seg_"))
        && (file.ends_with(".m4s") || file.ends_with("init.mp4"));
    if !valid {
        return Err(AppError::BadRequest("invalid segment path".to_string()));
    }
    let row = load(&state, id).await?;
    let dir = row
        .output_dir
        .clone()
        .ok_or_else(|| AppError::NotFound("Nothing has been captured yet".to_string()))?;
    let path = std::path::Path::new(&dir).join(&file);
    let f = tokio::fs::File::open(&path)
        .await
        .map_err(|_| AppError::NotFound("Segment not found".to_string()))?;
    let content_type = if file.ends_with("init.mp4") {
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
