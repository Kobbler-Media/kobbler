//! Turning a chosen debrid release into a playback session: from the local cache when the
//! release is already on disk, otherwise resolved through kobload and downloaded while it
//! plays. A warm session is the same thing held back from the viewer until it is claimed.

mod local;
mod media;
pub(crate) mod picture;
mod prepare;
mod remote;
mod response;
mod reuse;
mod subtitles;

pub(crate) use media::commit_media;
pub(crate) use prepare::probe_json;
pub(crate) use remote::WARM_HEAD_BYTES;
pub(crate) use response::live_response;
pub(crate) use reuse::suits;

use std::path::Path;

use axum::Json;
use axum::http::StatusCode;
use axum::response::IntoResponse;
use serde_json::json;
use time::OffsetDateTime;
use tokio_util::sync::CancellationToken;
use tracing::{error, warn};
use uuid::Uuid;

use crate::AppState;
use crate::ffmpeg::persist::ChapterSkipSegment;
use crate::jobs::handlers::debrid_process;
use crate::media_cache::picture_area::PictureArea;
use crate::repositories::debrid_cache_files as cache_files;
use crate::repositories::debrid_files;
use crate::repositories::debrid_stream_sessions;
use crate::routes::admin::debrid_stream::StreamResolveBody;
use crate::routes::admin::streaming::{ChapterResponse, SessionResponse, SkipSegmentResponse};
use crate::streaming::loudness::TrackLoudness;
use crate::streaming::progressive::DownloadTarget;
use crate::video_metadata::original_language::Title;

pub(crate) const VIDEO_CONTAINERS: &[&str] = &[
    "mkv", "mp4", "avi", "wmv", "webm", "m4v", "flv", "mov", "mpg", "mpeg", "ts", "m2ts", "vob",
];

/// What a warm session waits on before doing anything that costs more than its head.
#[derive(Clone, Default)]
pub(crate) struct WarmGate {
    pub claimed: CancellationToken,
    pub released: CancellationToken,
}

pub(crate) enum OpenMode {
    Play,
    Warm(WarmGate),
}

pub(crate) struct Opened {
    pub session_id: Uuid,
    pub response: SessionResponse,
    pub row: SessionRow,
    /// The rest of the file, for a warm session that stopped downloading at its head.
    pub remainder: Option<DownloadTarget>,
}

/// A `debrid_stream_sessions` row, written when the session starts to play.
#[derive(Clone)]
pub(crate) struct SessionRow {
    pub session_id: Uuid,
    pub user_id: Uuid,
    pub release_title: String,
    pub debrid_provider: String,
    pub debrid_provider_id: Uuid,
    pub info_hash: Option<String>,
    pub file_id: Option<i32>,
    pub quality: Option<serde_json::Value>,
    pub filename: String,
    pub filesize: i64,
    pub cache_path: Option<String>,
    pub debrid_url: Option<String>,
    pub debrid_url_expires_at: Option<OffsetDateTime>,
    pub chapters: Vec<ChapterResponse>,
    /// The file at `cache_path` is complete, so nothing else will analyse it.
    pub downloaded: bool,
}

pub(crate) struct Committed {
    pub media_id: Uuid,
    pub media_type: String,
    pub skip_segments: Vec<SkipSegmentResponse>,
    pub audio_loudness: Vec<TrackLoudness>,
    pub picture: Option<PictureArea>,
}

impl SessionRow {
    pub(crate) async fn insert(
        &self,
        pool: &sqlx::PgPool,
        media_type: &str,
        media_id: Uuid,
        audio_loudness: Option<&[TrackLoudness]>,
        picture: Option<&serde_json::Value>,
    ) -> Result<(), sqlx::Error> {
        sqlx::query!(
            r#"INSERT INTO debrid_stream_sessions
                (id, user_id, media_type, media_id, release_title, info_hash, debrid_provider,
                 debrid_provider_id, quality, filename, filesize, file_id, cache_path,
                 debrid_url, debrid_url_expires_at, audio_loudness, picture)
            VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15, $16,
                    $17)"#,
            self.session_id,
            self.user_id,
            media_type,
            media_id,
            self.release_title,
            self.info_hash,
            self.debrid_provider,
            self.debrid_provider_id,
            self.quality,
            self.filename,
            self.filesize,
            self.file_id,
            self.cache_path,
            self.debrid_url,
            self.debrid_url_expires_at,
            audio_loudness.map(sqlx::types::Json) as _,
            picture,
        )
        .execute(pool)
        .await?;
        if let Err(e) =
            debrid_files::record_session_chapters(pool, self.session_id, &self.chapters).await
        {
            warn!(error = %e, session_id = %self.session_id, "storing the download's chapters failed");
        }
        Ok(())
    }

    /// Make the session playable: its library item exists and its row is written, starting
    /// from what earlier analysis of the release found so a replay need not wait for it.
    pub(crate) async fn commit(
        &self,
        state: &AppState,
        body: &StreamResolveBody,
    ) -> Result<Committed, axum::response::Response> {
        let (media_id, media_type) = commit_media(state, body).await?;
        let analysis = debrid_stream_sessions::release_analysis(
            &state.pool,
            &media_type,
            media_id,
            self.info_hash.as_deref(),
            self.file_id,
            self.cache_path.as_deref(),
        )
        .await
        .unwrap_or_else(|e| {
            warn!(error = %e, %media_id, "earlier analysis of the release unreadable");
            Default::default()
        });
        let skip_segments: Vec<ChapterSkipSegment> = match self.cache_path.as_deref() {
            Some(path) => debrid_files::skip_segments_at(&state.pool, path)
                .await
                .unwrap_or_else(|e| {
                    warn!(error = %e, %media_id, "the download's analysis unreadable");
                    Vec::new()
                }),
            None => Vec::new(),
        };
        let audio_loudness = analysis.audio_loudness.map(|sqlx::types::Json(t)| t);
        if let Err(e) = self
            .insert(
                &state.pool,
                &media_type,
                media_id,
                audio_loudness.as_deref(),
                analysis.picture.as_ref(),
            )
            .await
        {
            error!(error = %e, "failed to create stream session");
            return Err((
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({"error": "failed to create session"})),
            )
                .into_response());
        }
        if self.downloaded
            && let Some(path) = self.cache_path.clone()
        {
            reanalyse_if_outdated(state, self.session_id, path).await;
        }
        Ok(Committed {
            media_id,
            media_type,
            audio_loudness: audio_loudness.unwrap_or_default(),
            picture: analysis.picture.and_then(picture::shown),
            skip_segments: skip_segments
                .into_iter()
                .map(|s| SkipSegmentResponse {
                    segment_type: s.segment_type,
                    start_seconds: s.start_seconds,
                    end_seconds: s.end_seconds,
                    confidence: None,
                    source: "detected".to_string(),
                })
                .collect(),
        })
    }
}

/// The dedup key for a debrid release's live session, from (info_hash, file_id).
pub(crate) fn release_key(info_hash: Option<&str>, file_id: Option<i32>) -> Option<String> {
    let hash = info_hash?;
    Some(match file_id {
        Some(fid) => format!("{hash}_f{fid}"),
        None => hash.to_string(),
    })
}

/// A Play session is committed and listed in the dedup index before this returns; a Warm
/// one is neither until it is claimed.
pub(crate) async fn open(
    state: &AppState,
    user_id: Uuid,
    body: &StreamResolveBody,
    mode: OpenMode,
) -> Result<Opened, axum::response::Response> {
    let runtime = state.runtime_config.get().await;
    if let Some(cached) = complete_cache_entry(
        state,
        body.debrid_provider_id,
        body.info_hash.as_deref(),
        body.file_id,
    )
    .await
    {
        return local::open(state, &runtime, user_id, body, cached, mode).await;
    }
    remote::open(state, &runtime, user_id, body, mode).await
}

pub(crate) async fn complete_cache_entry(
    state: &AppState,
    debrid_provider_id: Uuid,
    info_hash: Option<&str>,
    file_id: Option<i32>,
) -> Option<cache_files::CachedFile> {
    let info_hash = info_hash?;
    let cached = cache_files::find_release(&state.pool, debrid_provider_id, info_hash, file_id)
        .await
        .ok()
        .flatten()?;
    let on_disk = tokio::fs::metadata(Path::new(&cached.file_path))
        .await
        .ok()?
        .len();
    if cached.file_size <= 0 || on_disk < cached.file_size as u64 {
        return None;
    }
    if let Err(e) = cache_files::mark_complete(&state.pool, &cached.file_path).await {
        tracing::warn!(error = %e, path = %cached.file_path, "Failed to mark cache file complete");
    }
    Some(cached)
}

fn register(state: &AppState, body: &StreamResolveBody, session_id: Uuid) {
    if let Some(key) = release_key(body.info_hash.as_deref(), body.file_id) {
        state.debrid_session_index.insert(key, session_id);
    }
}

fn title_of(body: &StreamResolveBody) -> Option<Title> {
    if let Some(id) = body.media_id.filter(|id| !id.is_nil()) {
        return match body.media_type.as_str() {
            "movie" => Some(Title::Movie(id)),
            "episode" => Some(Title::Episode(id)),
            _ => None,
        };
    }
    let tmdb_id = body
        .auto_create
        .as_ref()?
        .identifiers
        .as_ref()?
        .get("tmdb_id")
        .and_then(|v| {
            v.as_str()
                .map(String::from)
                .or_else(|| v.as_i64().map(|n| n.to_string()))
        })?;
    match body.media_type.as_str() {
        "movie" => Some(Title::TmdbMovie(tmdb_id)),
        "tv" | "episode" => Some(Title::TmdbShow(tmdb_id)),
        _ => None,
    }
}

/// What a dub would be marked in: the release's name and the name of the file in it.
fn release_name(release_title: &str, filename: &str) -> String {
    format!("{release_title} {filename}")
}

fn audio_language<'a>(
    body: &'a StreamResolveBody,
    runtime: &'a crate::runtime_config::RuntimeConfig,
) -> &'a str {
    body.preferred_audio_language
        .as_deref()
        .filter(|l| !l.trim().is_empty())
        .unwrap_or(&runtime.preferred_audio_language)
}

fn segment_dir(
    state: &AppState,
    runtime: &crate::runtime_config::RuntimeConfig,
    session_id: Uuid,
) -> std::path::PathBuf {
    runtime
        .transcode_cache_dir
        .as_deref()
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| {
            std::path::PathBuf::from(&state.config.storage_root)
                .join("cache")
                .join("transcode")
        })
        .join(session_id.to_string())
}

/// A download analysed by an older kobbler, or never analysed at all, is analysed again
/// while it plays.
async fn reanalyse_if_outdated(state: &AppState, session_id: Uuid, cache_path: String) {
    let config = state.runtime_config.get().await;
    if !config.enrichment_enabled {
        return;
    }
    let vision = config.effective_vision_url().is_some_and(|u| !u.is_empty());
    let file = match debrid_files::at(&state.pool, &cache_path).await {
        Ok(file) => file.map(|f| f.id),
        Err(e) => {
            warn!(error = %e, %session_id, "debrid file lookup failed");
            return;
        }
    };
    let storage_root = state.config.storage_root.clone();
    let path = cache_path.clone();
    let outdated = tokio::task::spawn_blocking(move || {
        debrid_process::analysis_outdated(&storage_root, file, &path, vision)
    })
    .await
    .unwrap_or(false);
    if outdated {
        debrid_process::enqueue(
            &state.pool,
            state.nats.publisher(),
            session_id,
            std::path::Path::new(&cache_path),
        )
        .await;
    }
}
