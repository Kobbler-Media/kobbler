use axum::{
    Json,
    extract::{Path, State, rejection::JsonRejection},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
};
use serde::{Deserialize, Serialize};
use tracing::{error, warn};
use uuid::Uuid;

use crate::AppState;
use crate::jobs::{self, JobContext};
use crate::repositories::offline_exports::{self, OfflineExport};
use crate::repositories::{movie_files, tv_episode_files};
use crate::routes::AuthUser;
use crate::routes::admin::streaming::{load_playable_file, preferred_file_id};
use crate::streaming::byte_range::file_response;
use crate::streaming::decision::{self, ClientCapabilities, FileMeta, ViewerPrefs};
use crate::streaming::offline_export::{ExportPlan, Quality, export_file};
use crate::streaming::session::MediaType;

#[derive(Debug, Deserialize)]
pub struct CreateExportRequest {
    pub media_type: String,
    pub media_id: String,
    pub quality: String,
    #[serde(default)]
    pub preferred_audio_language: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct ExportResponse {
    pub export_id: Uuid,
    pub state: String,
    pub progress: f64,
    pub size_bytes: Option<i64>,
    pub error: Option<String>,
    pub file_url: String,
}

impl From<OfflineExport> for ExportResponse {
    fn from(e: OfflineExport) -> Self {
        Self {
            file_url: format!("/api/v1/admin/offline/exports/{}/file", e.id),
            export_id: e.id,
            state: e.state,
            progress: e.progress,
            size_bytes: e.size_bytes,
            error: e.error,
        }
    }
}

const QUIET_JOB_MINUTES: i32 = 30;

fn bad_request(message: &str) -> Response {
    (StatusCode::BAD_REQUEST, message.to_string()).into_response()
}

fn not_found() -> Response {
    (StatusCode::NOT_FOUND, "export not found").into_response()
}

fn db_error(e: impl std::fmt::Display) -> Response {
    error!(error = %e, "offline export database error");
    (StatusCode::INTERNAL_SERVER_ERROR, "database error").into_response()
}

pub async fn create_export(
    State(state): State<AppState>,
    auth: AuthUser,
    body: Result<Json<CreateExportRequest>, JsonRejection>,
) -> Result<Json<ExportResponse>, Response> {
    let Json(req) = body.map_err(|e| bad_request(&e.body_text()))?;
    let media_type = match req.media_type.as_str() {
        "movie" => MediaType::Movie,
        "episode" => MediaType::Episode,
        _ => return Err(bad_request("media_type must be movie or episode")),
    };
    let quality = Quality::parse(&req.quality)
        .ok_or_else(|| bad_request("quality must be original, 1080p, 720p or 480p"))?;
    let media_id = Uuid::parse_str(&req.media_id).map_err(|_| bad_request("invalid media_id"))?;

    if auth.is_kids {
        let allowed = match media_type {
            MediaType::Movie => crate::kids::movie_allowed(&state.pool, media_id).await,
            _ => crate::kids::episode_allowed(&state.pool, media_id).await,
        };
        if !allowed.unwrap_or(false) {
            return Err((StatusCode::NOT_FOUND, "not available on a Kids profile").into_response());
        }
    }

    let file_id = preferred_file_id(&state, media_type, media_id).await?;
    let (input_path, meta) = load_playable_file(&state, media_type, file_id, true).await?;

    let config = state.runtime_config.get().await;
    let language = decision::audio_language_pref(
        req.preferred_audio_language.as_deref(),
        &config.preferred_audio_language,
    );
    let playable = decision::decide(
        &FileMeta {
            container: meta.container.clone(),
            video_codec: meta.video_codec.clone(),
            video_width: meta.video_width,
            video_height: meta.video_height,
            interlaced: meta.interlaced.unwrap_or(false),
        },
        &meta.audio_metas,
        &[],
        &ClientCapabilities {
            supports_interlaced: Some(false),
            ..ClientCapabilities::default()
        },
        ViewerPrefs {
            preferred_audio_index: decision::find_preferred_audio(&meta.audio_metas, language),
            preferred_subtitle_index: None,
            understood_language: language,
        },
        &["h264"],
    );
    let audio_track = meta
        .audio_metas
        .iter()
        .find(|a| a.stream_index == playable.audio_stream_index);

    let (mut export, inserted) = offline_exports::find_or_create(
        &state.pool,
        media_type.as_str(),
        file_id,
        quality.as_str(),
        playable.audio_stream_index,
    )
    .await
    .map_err(db_error)?;

    let stale = export.state == "failed"
        || (export.state == "ready"
            && !export_file(&state.config.storage_root, export.id).exists());
    if stale {
        export = offline_exports::requeue(&state.pool, export.id)
            .await
            .map_err(db_error)?
            .ok_or_else(not_found)?;
    }

    // A job lost between the queue and a worker leaves the row queued for good; one that
    // has not moved in this long is sent again, and claim() makes a duplicate harmless.
    let revived = !inserted
        && !stale
        && offline_exports::revive_if_quiet(&state.pool, export.id, QUIET_JOB_MINUTES)
            .await
            .map_err(db_error)?;

    if inserted || stale || revived {
        let plan = ExportPlan {
            export_id: export.id,
            input_path,
            duration_seconds: meta.duration,
            quality,
            source_width: meta.video_width,
            source_height: meta.video_height,
            audio_stream_index: playable.audio_stream_index,
            audio_bitrate_kbps: decision::suggest_audio_bitrate(audio_track),
            deinterlace: playable.deinterlace,
        };
        enqueue(&state, media_type, media_id, &plan).await?;
    }

    Ok(Json(export.into()))
}

async fn enqueue(
    state: &AppState,
    media_type: MediaType,
    media_id: Uuid,
    plan: &ExportPlan,
) -> Result<(), Response> {
    let entity = media_type.as_str();
    let display = jobs::fetch_job_display(&state.pool, entity, media_id).await;
    let context = JobContext::for_entity(
        entity,
        media_id,
        format!("Preparing {} offline download", plan.quality.as_str()),
    )
    .with_display_opt(display);
    let payload = serde_json::to_value(plan).expect("export plan serializes");
    if let Err(e) = jobs::enqueue(
        &state.pool,
        state.nats.publisher(),
        "offline_export",
        payload,
        context,
    )
    .await
    {
        warn!(export_id = %plan.export_id, error = %e, "failed to enqueue offline export");
        let _ = offline_exports::mark_failed(&state.pool, plan.export_id, &e.to_string()).await;
        return Err((StatusCode::INTERNAL_SERVER_ERROR, "failed to queue export").into_response());
    }
    Ok(())
}

pub async fn get_export(
    State(state): State<AppState>,
    auth: AuthUser,
    Path(export_id): Path<String>,
) -> Result<Json<ExportResponse>, Response> {
    let id = Uuid::parse_str(&export_id).map_err(|_| not_found())?;
    let export = offline_exports::get(&state.pool, id)
        .await
        .map_err(db_error)?
        .ok_or_else(not_found)?;
    if auth.is_kids && !kids_may_play(&state, &export).await {
        return Err(not_found());
    }
    Ok(Json(export.into()))
}

/// An export id is no permission of its own: a Kids profile reaches the file only when it
/// could play a title the file belongs to.
async fn kids_may_play(state: &AppState, export: &OfflineExport) -> bool {
    match export.media_type.as_str() {
        "movie" => match movie_files::get(&state.pool, export.file_id).await {
            Ok(Some(file)) => crate::kids::movie_allowed(&state.pool, file.movie_id)
                .await
                .unwrap_or(false),
            _ => false,
        },
        _ => {
            let episodes = tv_episode_files::get_episode_ids(&state.pool, export.file_id)
                .await
                .unwrap_or_default();
            for episode in episodes {
                if crate::kids::episode_allowed(&state.pool, episode)
                    .await
                    .unwrap_or(false)
                {
                    return true;
                }
            }
            false
        }
    }
}

pub async fn download_export(
    State(state): State<AppState>,
    auth: AuthUser,
    Path(export_id): Path<String>,
    headers: HeaderMap,
) -> Result<Response, Response> {
    let id = Uuid::parse_str(&export_id).map_err(|_| not_found())?;
    let export = offline_exports::touch(&state.pool, id)
        .await
        .map_err(db_error)?
        .ok_or_else(not_found)?;
    if auth.is_kids && !kids_may_play(&state, &export).await {
        return Err(not_found());
    }
    let not_ready = || (StatusCode::CONFLICT, "export not ready").into_response();
    if export.state != "ready" {
        return Err(not_ready());
    }

    let path = export_file(&state.config.storage_root, id);
    let file = match tokio::fs::File::open(&path).await {
        Ok(f) => f,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            // A failed export is what a client re-requests, which rebuilds it.
            warn!(export_id = %id, "ready offline export has no file");
            offline_exports::mark_failed(&state.pool, id, "export file missing")
                .await
                .map_err(db_error)?;
            return Err(not_ready());
        }
        Err(e) => {
            error!(error = %e, path = %path.display(), "failed to open offline export");
            return Err(
                (StatusCode::INTERNAL_SERVER_ERROR, "failed to read export").into_response()
            );
        }
    };
    let size = file
        .metadata()
        .await
        .map_err(|e| {
            error!(error = %e, "failed to stat offline export");
            (StatusCode::INTERNAL_SERVER_ERROR, "failed to read export").into_response()
        })?
        .len();
    Ok(file_response(file, size, "video/mp4", &headers).await)
}
