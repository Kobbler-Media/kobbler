//! Turning a chosen debrid release into a playback session: from the local cache when the
//! release is already on disk, otherwise resolved through kobload and downloaded while it
//! plays. A warm session is the same thing held back from the viewer until it is claimed.

mod local;
mod media;
mod prepare;
mod remote;
mod response;
mod subtitles;

pub(crate) use media::commit_media;
pub(crate) use prepare::{chapters_json, probe_json};
pub(crate) use remote::WARM_HEAD_BYTES;
pub(crate) use response::live_response;

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
use crate::repositories::debrid_cache_files as cache_files;
use crate::repositories::debrid_stream_sessions;
use crate::routes::admin::debrid_stream::StreamResolveBody;
use crate::routes::admin::streaming::{ChapterResponse, SessionResponse, SkipSegmentResponse};
use crate::streaming::loudness::PlaybackLoudness;
use crate::streaming::progressive::DownloadTarget;

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
}

pub(crate) struct Committed {
    pub media_id: Uuid,
    pub media_type: String,
    pub skip_segments: Vec<SkipSegmentResponse>,
    pub loudness: PlaybackLoudness,
}

impl SessionRow {
    pub(crate) async fn insert(
        &self,
        pool: &sqlx::PgPool,
        media_type: &str,
        media_id: Uuid,
        skip_segments: &[ChapterSkipSegment],
        loudness: PlaybackLoudness,
    ) -> Result<(), sqlx::Error> {
        let skip_segments = (!skip_segments.is_empty())
            .then(|| serde_json::to_value(skip_segments).ok())
            .flatten();
        let chapters = chapters_json(&self.chapters);
        sqlx::query!(
            r#"INSERT INTO debrid_stream_sessions
                (id, user_id, media_type, media_id, release_title, info_hash, debrid_provider,
                 debrid_provider_id, quality, filename, filesize, file_id, cache_path,
                 debrid_url, debrid_url_expires_at, skip_segments, lufs, downmix_peak_db, chapters)
            VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15, $16,
                    $17, $18, $19)"#,
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
            skip_segments,
            loudness.lufs,
            loudness.downmix_peak_db,
            chapters,
        )
        .execute(pool)
        .await?;
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
        let skip_segments: Vec<ChapterSkipSegment> = analysis
            .skip_segments
            .and_then(|v| serde_json::from_value(v).ok())
            .unwrap_or_default();
        let loudness = PlaybackLoudness {
            lufs: analysis.lufs,
            downmix_peak_db: analysis.downmix_peak_db,
        };
        if let Err(e) = self
            .insert(&state.pool, &media_type, media_id, &skip_segments, loudness)
            .await
        {
            error!(error = %e, "failed to create stream session");
            return Err((
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({"error": "failed to create session"})),
            )
                .into_response());
        }
        Ok(Committed {
            media_id,
            media_type,
            loudness,
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
    (cached.file_size > 0 && on_disk >= cached.file_size as u64).then_some(cached)
}

fn register(state: &AppState, body: &StreamResolveBody, session_id: Uuid) {
    if let Some(key) = release_key(body.info_hash.as_deref(), body.file_id) {
        state.debrid_session_index.insert(key, session_id);
    }
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
