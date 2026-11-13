//! Keep a debrid stream in the library: finish its download, then import the cached file
//! as the session's movie or episode file through the same placement kobload deliveries
//! use. The file is moved, not copied, whenever the cache and the library share a
//! filesystem.

use std::path::{Path, PathBuf};
use std::time::Duration;

use async_trait::async_trait;
use serde::Deserialize;
use serde_json::{Value, json};
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;
use tracing::{info, warn};
use uuid::Uuid;

use crate::AppState;
use crate::events::LibraryEvent;
use crate::file_link_mode::FileLinkMode;
use crate::jobs::worker::JobHandler;
use crate::jobs::{JobError, JobResult};
use crate::repositories::{
    debrid_cache_files as cache_files, debrid_keeps, movie_files, movies, tv_episode_files,
    tv_episodes, tv_seasons,
};
use crate::routes::admin::debrid_play::{PlaySession, playable_url};
use crate::routes::admin::stream_keep::KEEP_JOB;
use crate::routes::ingest::placement::{self, FileKind, Placement};
use crate::streaming::cache_download;
use crate::streaming::download_meter::DownloadMeter;

const DOWNLOAD_ATTEMPTS: u32 = 3;

#[derive(Deserialize)]
struct Payload {
    session_id: Uuid,
}

pub struct DebridKeepHandler {
    state: AppState,
}

impl DebridKeepHandler {
    pub fn new(state: AppState) -> Self {
        Self { state }
    }
}

#[async_trait]
impl JobHandler for DebridKeepHandler {
    fn job_type(&self) -> &'static str {
        KEEP_JOB
    }

    fn max_concurrent(&self) -> usize {
        2
    }

    async fn execute(
        &self,
        _job_id: Uuid,
        payload: Value,
        cancel: CancellationToken,
    ) -> JobResult<Value> {
        let Payload { session_id } = serde_json::from_value(payload)?;
        let pool = &self.state.pool;
        match debrid_keeps::get(pool, session_id).await? {
            Some(k) if k.state == "done" => return Ok(json!({ "skipped": "already kept" })),
            Some(_) => {}
            None => return Ok(json!({ "skipped": "not kept" })),
        }
        match keep(&self.state, session_id, &cancel).await {
            Ok(path) => {
                debrid_keeps::set_done(pool, session_id, &path.to_string_lossy()).await?;
                info!(%session_id, path = %path.display(), "kept debrid stream in library");
                Ok(json!({ "session_id": session_id, "file_path": path }))
            }
            Err(message) => {
                debrid_keeps::set_state(pool, session_id, "failed", 0.0, Some(&message)).await?;
                Err(JobError::ExecutionFailed(message))
            }
        }
    }
}

struct Target {
    kind: FileKind,
    media_id: Uuid,
}

async fn keep(
    state: &AppState,
    session_id: Uuid,
    cancel: &CancellationToken,
) -> Result<PathBuf, String> {
    let row = sqlx::query!(
        "SELECT media_type, media_id FROM debrid_stream_sessions WHERE id = $1",
        session_id,
    )
    .fetch_optional(&state.pool)
    .await
    .map_err(|e| e.to_string())?
    .ok_or("the stream session is gone")?;
    let target = Target {
        kind: match row.media_type.as_str() {
            "movie" => FileKind::Movie,
            "episode" => FileKind::Episode,
            other => return Err(format!("cannot keep a {other} stream")),
        },
        media_id: row.media_id,
    };
    let session = PlaySession::load(&state.pool, session_id, None)
        .await
        .map_err(|e| e.to_string())?
        .ok_or("the stream session is gone")?;
    let cache_path = PathBuf::from(
        session
            .cache_path
            .as_deref()
            .ok_or("the stream has no local copy to keep")?,
    );

    finish_download(state, &session, &cache_path, cancel).await?;
    debrid_keeps::set_state(&state.pool, session_id, "importing", 1.0, None)
        .await
        .map_err(|e| e.to_string())?;
    import(state, &session, &cache_path, &target).await
}

async fn on_disk(path: &Path) -> u64 {
    tokio::fs::metadata(path).await.map_or(0, |m| m.len())
}

/// Whether the live session is downloading the file itself, in which case a second
/// writer would corrupt it.
fn live_download(state: &AppState, session_id: Uuid) -> bool {
    state.playback_sessions.get(&session_id).is_some_and(|s| {
        s.download_cancel
            .as_ref()
            .is_some_and(|c| !c.is_cancelled())
    })
}

async fn finish_download(
    state: &AppState,
    session: &PlaySession,
    cache_path: &Path,
    cancel: &CancellationToken,
) -> Result<(), String> {
    let total = session.filesize.max(0) as u64;
    let mut ours: Option<JoinHandle<Result<(), String>>> = None;
    let mut failures = 0;
    loop {
        let have = on_disk(cache_path).await;
        if total > 0 && have >= total {
            return Ok(());
        }
        if cancel.is_cancelled() {
            return Err("cancelled".to_string());
        }
        let _ = debrid_keeps::set_state(
            &state.pool,
            session.id,
            "downloading",
            have as f64 / total.max(1) as f64,
            None,
        )
        .await;

        // The idle reaper stops a quiet session's download; keeping it counts as use.
        if let Some(mut live) = state.playback_sessions.get_mut(&session.id) {
            live.touch();
        }
        if let Some(handle) = ours.take_if(|h| h.is_finished()) {
            if let Err(e) = handle.await.unwrap_or_else(|e| Err(e.to_string())) {
                failures += 1;
                warn!(session_id = %session.id, error = %e, "keep download attempt failed");
                if failures >= DOWNLOAD_ATTEMPTS {
                    return Err(format!("download failed: {e}"));
                }
            }
        } else if ours.is_none() && !live_download(state, session.id) {
            ours = Some(start_download(state, session, cache_path).await?);
        }
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
}

/// Download the rest of the file on the session's behalf, under the session's own
/// cancel token when it is live so playback resuming does not start a second writer.
async fn start_download(
    state: &AppState,
    session: &PlaySession,
    cache_path: &Path,
) -> Result<JoinHandle<Result<(), String>>, String> {
    let url = playable_url(state, session).await.map_err(|resp| {
        format!(
            "the debrid host would not serve the file ({})",
            resp.status()
        )
    })?;
    if let Some(key) = crate::routes::admin::debrid_open::release_key(
        session.info_hash.as_deref(),
        session.file_id,
    ) {
        state.downloads_ahead.hand_over(&key).await;
    }
    let cancel = CancellationToken::new();
    let total = session.filesize.max(0) as u64;
    let have = on_disk(cache_path).await;
    let meter = match state.playback_sessions.get_mut(&session.id) {
        Some(mut live) => {
            live.download_cancel = Some(cancel.clone());
            live.download
                .clone()
                .unwrap_or_else(|| DownloadMeter::new(total, have))
        }
        None => DownloadMeter::new(total, have),
    };
    let target = session.download_target(url, cache_path.to_path_buf());
    let state = state.clone();
    Ok(tokio::spawn(async move {
        cache_download::download(&state, target, cancel, meter).await
    }))
}

async fn import(
    state: &AppState,
    session: &PlaySession,
    cache_path: &Path,
    target: &Target,
) -> Result<PathBuf, String> {
    let runtime = state.runtime_config.get().await;
    let (placement, show_id) = match target.kind {
        FileKind::Movie => {
            let movie = movies::get(&state.pool, target.media_id)
                .await
                .map_err(|e| e.to_string())?
                .ok_or("the movie is gone")?;
            let placement =
                placement::movie(state, &runtime, movie.id, &movie.title, movie.year).await;
            (placement, None)
        }
        FileKind::Episode => {
            let episode = tv_episodes::get(&state.pool, target.media_id)
                .await
                .map_err(|e| e.to_string())?
                .ok_or("the episode is gone")?;
            let show_id = tv_seasons::get(&state.pool, episode.season_id)
                .await
                .ok()
                .flatten()
                .map(|s| s.show_id);
            let naming = placement::EpisodeNaming::load(
                state,
                episode.id,
                show_id,
                Some(episode.season_id),
                None,
            )
            .await;
            (placement::episode(state, &runtime, &naming).await, show_id)
        }
    };
    let placement =
        placement.map_err(|resp| format!("the library is unavailable ({})", resp.status()))?;
    let extension = session
        .filename
        .rsplit('.')
        .next()
        .unwrap_or("mkv")
        .to_lowercase();
    let dest = placement.file_path(&extension, &session.filename);
    tokio::fs::create_dir_all(&placement.dir)
        .await
        .map_err(|e| format!("create {}: {e}", placement.dir.display()))?;
    place(cache_path, &dest, &placement).await?;
    hand_over(state, cache_path, &dest).await;
    register(state, session, &placement, &dest, target, show_id).await?;
    Ok(dest)
}

/// Rename when the cache and the library share a filesystem; otherwise copy beside the
/// destination under a hidden name, so the scanner never sees a half-written file, and
/// rename that into place.
async fn place(src: &Path, dest: &Path, placement: &Placement) -> Result<(), String> {
    if tokio::fs::rename(src, dest).await.is_ok() {
        return Ok(());
    }
    let temp = placement
        .dir
        .join(format!(".kobbler-keep-{}", Uuid::now_v7()));
    if let Err(e) = tokio::fs::copy(src, &temp).await {
        let _ = tokio::fs::remove_file(&temp).await;
        return Err(format!("copy into the library: {e}"));
    }
    tokio::fs::rename(&temp, dest)
        .await
        .map_err(|e| format!("rename into the library: {e}"))?;
    let _ = tokio::fs::remove_file(src).await;
    Ok(())
}

/// Sessions playing the cached file carry on from its library copy, and the cache entry
/// and its probe/thumbnail leftovers go.
async fn hand_over(state: &AppState, src: &Path, dest: &Path) {
    let src_str = src.to_string_lossy().to_string();
    let dest_str = dest.to_string_lossy().to_string();
    let _ = sqlx::query!(
        "UPDATE debrid_stream_sessions SET cache_path = $2, updated_at = NOW() WHERE cache_path = $1",
        src_str,
        dest_str,
    )
    .execute(&state.pool)
    .await;
    for mut live in state.playback_sessions.iter_mut() {
        if live.file_path == src {
            live.file_path = dest.to_path_buf();
        }
    }
    crate::jobs::handlers::debrid_cache_cleanup::remove_cache_entry(&src_str).await;
    if let Err(e) = cache_files::forget(&state.pool, &src_str).await {
        warn!(path = %src_str, error = %e, "failed to forget kept cache file");
    }
}

async fn register(
    state: &AppState,
    session: &PlaySession,
    placement: &Placement,
    dest: &Path,
    target: &Target,
    show_id: Option<Uuid>,
) -> Result<(), String> {
    let file_path = dest.to_string_lossy().to_string();
    let file_size = Some(on_disk(dest).await as i64);
    let format = session
        .filename
        .rsplit('.')
        .next()
        .unwrap_or("mkv")
        .to_lowercase();
    let source_metadata = Some(json!({
        "info_hash": session.info_hash,
        "file_id": session.file_id,
        "release_title": session.release_title,
    }));
    let file_id = match target.kind {
        FileKind::Movie => {
            let file_id = movie_files::create_or_update(
                &state.pool,
                movie_files::MovieFileInput {
                    movie_id: target.media_id,
                    file_path,
                    file_size,
                    file_fingerprint: None,
                    format,
                    quality: None,
                    languages: None,
                    subtitles: None,
                    source_type: Some("debrid".to_string()),
                    source_metadata,
                    video_codec: None,
                    video_width: None,
                    video_height: None,
                    video_bitrate_kbps: None,
                    framerate: None,
                    bit_depth: None,
                    is_hdr: false,
                    hdr_format: None,
                    is_interlaced: false,
                    duration_seconds: None,
                    container: None,
                    overall_bitrate_kbps: None,
                },
            )
            .await
            .map_err(|e| format!("create movie file: {e}"))?;
            let replaced = movie_files::replace_other_files_for_movie(
                &state.pool,
                target.media_id,
                file_id,
                FileLinkMode::Replace,
            )
            .await
            .unwrap_or_default();
            crate::repositories::cleanup_replaced_files(&replaced).await;
            file_id
        }
        FileKind::Episode => {
            let file_id = tv_episode_files::create_or_update(
                &state.pool,
                tv_episode_files::EpisodeFileInput {
                    file_path,
                    file_size,
                    file_fingerprint: None,
                    format,
                    quality: None,
                    languages: None,
                    subtitles: None,
                    source_type: Some("debrid".to_string()),
                    source_metadata,
                    video_codec: None,
                    video_width: None,
                    video_height: None,
                    video_bitrate_kbps: None,
                    framerate: None,
                    bit_depth: None,
                    is_hdr: false,
                    is_interlaced: false,
                    hdr_format: None,
                    duration_seconds: None,
                    container: None,
                    overall_bitrate_kbps: None,
                },
            )
            .await
            .map_err(|e| format!("create episode file: {e}"))?;
            let replaced = tv_episode_files::link_episodes_with_mode(
                &state.pool,
                file_id,
                &[target.media_id],
                FileLinkMode::Replace,
            )
            .await
            .map_err(|e| format!("link episode file: {e}"))?;
            crate::repositories::cleanup_replaced_files(&replaced).await;
            file_id
        }
    };
    placement::announce_file(
        state,
        placement,
        dest,
        target.kind,
        file_id,
        target.media_id,
    )
    .await;
    match target.kind {
        FileKind::Movie => {
            placement::complete_movie(state, target.media_id).await;
            let _ = state
                .library_events
                .send(LibraryEvent::movie_updated(target.media_id));
        }
        FileKind::Episode => {
            placement::complete_episode(state, target.media_id, show_id).await;
            if let Some(sid) = show_id {
                let _ = state
                    .library_events
                    .send(LibraryEvent::tv_show_updated(sid));
            }
        }
    }
    Ok(())
}
