//! Media extract job handler.
//!
//! Runs heavy extraction on a video file: keyframe scanning and subtitle
//! pre-extraction. Separated from media_probe (which does fast metadata only)
//! to avoid blocking the probe queue with I/O-heavy work.

use crate::RuntimeConfigManager;
use crate::ffmpeg::{mkv_cues, probe, transcode};
use crate::jobs::worker::JobHandler;
use crate::jobs::{JobError, JobResult, MediaRootIoGate};
use crate::repositories::{media_roots, movie_files, subtitle_tracks, tv_episode_files};
use crate::streaming::subtitle;
use crate::streaming::subtitle_insight::{WINDOW_SECS, analyse_extracted};
use async_trait::async_trait;
use kob_ffmpeg::subtitles::TEXT_CODECS;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sqlx::PgPool;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::Semaphore;
use tokio_util::sync::CancellationToken;
use tracing::{info, warn};
use uuid::Uuid;

/// Payload for media_extract jobs.
#[derive(Debug, Deserialize, Serialize)]
pub struct MediaExtractPayload {
    pub file_id: Uuid,
    /// "movie" or "episode"
    pub media_type: String,
}

pub struct MediaExtractHandler {
    pool: PgPool,
    storage_root: String,
    runtime_config: RuntimeConfigManager,
    fs_semaphore: Arc<Semaphore>,
    io_gate: MediaRootIoGate,
}

impl MediaExtractHandler {
    pub fn new(
        pool: PgPool,
        storage_root: String,
        runtime_config: RuntimeConfigManager,
        fs_semaphore: Arc<Semaphore>,
        io_gate: MediaRootIoGate,
    ) -> Self {
        Self {
            pool,
            storage_root,
            runtime_config,
            fs_semaphore,
            io_gate,
        }
    }
}

impl MediaExtractHandler {
    /// Reads the extracted text tracks for what their tags left out — see
    /// `streaming::subtitle_insight`. Once per file: every track is marked checked after.
    async fn check_subtitle_text(
        &self,
        movie_file_id: Option<Uuid>,
        episode_file_id: Option<Uuid>,
        tracks: &[subtitle_tracks::SubtitleTrackRow],
        file_id: Uuid,
        duration: Option<f64>,
    ) -> usize {
        let codecs: Vec<String> = TEXT_CODECS.iter().map(|c| c.to_string()).collect();
        match subtitle_tracks::has_unchecked_text_tracks(
            &self.pool,
            movie_file_id,
            episode_file_id,
            &codecs,
        )
        .await
        {
            Ok(true) => {}
            Ok(false) => return 0,
            Err(e) => {
                warn!(error = %e, "Failed to check subtitle text state (non-fatal)");
                return 0;
            }
        }
        let extracted: Vec<(i32, PathBuf)> = tracks
            .iter()
            .filter(|t| TEXT_CODECS.contains(&t.codec.as_str()))
            .map(|t| {
                (
                    t.track_index,
                    subtitle::cache_path(&self.storage_root, file_id, t.track_index, &t.codec),
                )
            })
            .collect();
        let window = duration.map_or(WINDOW_SECS, |d| d.min(WINDOW_SECS));
        let insights = tokio::task::spawn_blocking(move || analyse_extracted(&extracted, window))
            .await
            .unwrap_or_default();
        for insight in &insights {
            if let Err(e) = subtitle_tracks::record_text_insight(
                &self.pool,
                movie_file_id,
                episode_file_id,
                insight.stream_index,
                insight.language.as_deref(),
                insight.forced,
                insight.sdh,
            )
            .await
            {
                warn!(error = %e, "Failed to record subtitle insight (non-fatal)");
            }
        }
        if let Err(e) =
            subtitle_tracks::mark_text_checked(&self.pool, movie_file_id, episode_file_id).await
        {
            warn!(error = %e, "Failed to mark subtitle text checked (non-fatal)");
        }
        insights.len()
    }
}

#[async_trait]
impl JobHandler for MediaExtractHandler {
    fn job_type(&self) -> &'static str {
        "media_extract"
    }

    fn max_concurrent(&self) -> usize {
        self.runtime_config.job_concurrency("media_extract", 1)
    }

    async fn execute(
        &self,
        job_id: Uuid,
        payload: Value,
        cancel: CancellationToken,
    ) -> JobResult<Value> {
        let _fs_permit = self
            .fs_semaphore
            .acquire()
            .await
            .map_err(|_| JobError::ExecutionFailed("FS semaphore closed".into()))?;
        let payload: MediaExtractPayload = serde_json::from_value(payload)
            .map_err(|e| JobError::ExecutionFailed(format!("Invalid payload: {e}")))?;

        nats_jobs::check_cancelled(&cancel)?;

        info!(
            job_id = %job_id,
            file_id = %payload.file_id,
            media_type = %payload.media_type,
            "Starting media extraction (keyframes + subtitles)"
        );

        // Look up the file record: where it is, and which of this job's parts it still needs.
        let (file_path, has_keyframes, duration) = match payload.media_type.as_str() {
            "movie" => {
                let file = movie_files::get(&self.pool, payload.file_id)
                    .await
                    .map_err(|e| JobError::ExecutionFailed(format!("DB error: {e}")))?
                    .ok_or_else(|| {
                        JobError::ExecutionFailed(format!(
                            "Movie file not found: {}",
                            payload.file_id
                        ))
                    })?;
                (
                    file.file_path,
                    file.keyframes.is_some(),
                    file.duration_seconds,
                )
            }
            "episode" => {
                let file = tv_episode_files::get(&self.pool, payload.file_id)
                    .await
                    .map_err(|e| JobError::ExecutionFailed(format!("DB error: {e}")))?
                    .ok_or_else(|| {
                        JobError::ExecutionFailed(format!(
                            "Episode file not found: {}",
                            payload.file_id
                        ))
                    })?;
                (
                    file.file_path,
                    file.keyframes.is_some(),
                    file.duration_seconds,
                )
            }
            other => {
                return Err(JobError::ExecutionFailed(format!(
                    "Unknown media type: {other}"
                )));
            }
        };
        let (movie_file_id, episode_file_id) = match payload.media_type.as_str() {
            "movie" => (Some(payload.file_id), None),
            _ => (None, Some(payload.file_id)),
        };

        let full_path = PathBuf::from(&file_path);
        let is_file = {
            let fp = full_path.clone();
            tokio::task::spawn_blocking(move || fp.is_file())
                .await
                .unwrap_or(false)
        };
        if !is_file {
            return Err(JobError::ExecutionFailed(format!(
                "File not found on disk: {}",
                full_path.display()
            )));
        }

        // Gate heavy IO per media root so backfill can't collapse the NAS.
        let media_root_ids = media_roots::resolve_media_root_ids_for_file(
            &self.pool,
            &payload.media_type,
            payload.file_id,
        )
        .await
        .unwrap_or_default();
        let _io_permits = self.io_gate.acquire_many(media_root_ids).await;

        // Only what is missing: a file re-queued for its subtitles keeps its keyframes.
        let keyframe_count = if has_keyframes {
            0
        } else {
            // Probe keyframes for instant HLS playlist generation.
            // For MKV files, read the Cues element (a few hundred KB) instead of scanning
            // every packet header in the entire file (~14GB). Falls back to ffprobe if
            // Cues are unavailable.
            let keyframe_result = if is_matroska_path(&full_path) {
                match mkv_cues::read_mkv_keyframes(&full_path).await {
                    Ok(kf) if !kf.is_empty() => {
                        info!(
                            file_id = %payload.file_id,
                            count = kf.len(),
                            "Fast MKV Cues keyframe extraction succeeded"
                        );
                        Ok(kf)
                    }
                    Ok(_) => {
                        info!(file_id = %payload.file_id, "MKV Cues empty, falling back to ffprobe");
                        probe::probe_keyframes(&full_path).await
                    }
                    Err(e) => {
                        info!(
                            file_id = %payload.file_id,
                            error = %e,
                            "MKV Cues unavailable, falling back to ffprobe"
                        );
                        probe::probe_keyframes(&full_path).await
                    }
                }
            } else {
                // MP4 and other containers: ffprobe reads from container index (already fast)
                probe::probe_keyframes(&full_path).await
            };

            match keyframe_result {
                Ok(keyframes) if !keyframes.is_empty() => {
                    let kf_value = serde_json::to_value(&keyframes).unwrap_or_default();
                    let count = keyframes.len();
                    let save_result = match payload.media_type.as_str() {
                        "movie" => {
                            movie_files::update_keyframes(&self.pool, payload.file_id, &kf_value)
                                .await
                        }
                        "episode" => {
                            tv_episode_files::update_keyframes(
                                &self.pool,
                                payload.file_id,
                                &kf_value,
                            )
                            .await
                        }
                        _ => Ok(false),
                    };
                    if let Err(e) = save_result {
                        warn!(error = %e, "Failed to save keyframes to DB");
                    }
                    count
                }
                Ok(_) => {
                    // Save empty array so files with genuinely no keyframes aren't re-queued
                    let empty = serde_json::Value::Array(vec![]);
                    let save_result = match payload.media_type.as_str() {
                        "movie" => {
                            movie_files::update_keyframes(&self.pool, payload.file_id, &empty).await
                        }
                        "episode" => {
                            tv_episode_files::update_keyframes(&self.pool, payload.file_id, &empty)
                                .await
                        }
                        _ => Ok(false),
                    };
                    if let Err(e) = save_result {
                        warn!(error = %e, "Failed to save empty keyframes to DB");
                    }
                    0
                }
                Err(e) => {
                    warn!(error = %e, "Keyframe probe failed (non-fatal)");
                    0
                }
            }
        };

        nats_jobs::check_cancelled(&cancel)?;

        // Pre-extract text subtitles so they're instantly available at playback.
        // Query subtitle tracks from DB (already populated by media_probe).
        let sub_tracks = match payload.media_type.as_str() {
            "movie" => subtitle_tracks::list_by_movie_file(&self.pool, payload.file_id).await,
            "episode" => subtitle_tracks::list_by_episode_file(&self.pool, payload.file_id).await,
            _ => Ok(vec![]),
        };

        let sub_tracks = sub_tracks.unwrap_or_else(|e| {
            warn!(error = %e, "Failed to query subtitle tracks (non-fatal)");
            Vec::new()
        });
        let subs_extracted = {
            let tracks = &sub_tracks;
            let all_specs: Vec<transcode::SubtitleExtractSpec> = tracks
                .iter()
                .filter(|s| {
                    !subtitle::is_bitmap_subtitle(&s.codec)
                        && subtitle::subtitle_extension(&s.codec).is_some()
                })
                .map(|s| transcode::SubtitleExtractSpec {
                    stream_index: s.track_index,
                    output_path: subtitle::cache_path(
                        &self.storage_root,
                        payload.file_id,
                        s.track_index,
                        &s.codec,
                    ),
                })
                .collect();

            // Filter out already-extracted subtitles on the blocking pool
            let text_subs = tokio::task::spawn_blocking(move || {
                all_specs
                    .into_iter()
                    .filter(|spec| !spec.output_path.exists())
                    .collect::<Vec<_>>()
            })
            .await
            .unwrap_or_default();

            if !text_subs.is_empty() {
                match transcode::extract_all_subtitles(&full_path, &text_subs).await {
                    Ok(()) => {
                        info!(
                            file_id = %payload.file_id,
                            count = text_subs.len(),
                            "Pre-extracted subtitle tracks"
                        );
                        text_subs.len()
                    }
                    Err(e) => {
                        warn!(error = %e, "Subtitle pre-extraction failed (non-fatal)");
                        0
                    }
                }
            } else {
                0
            }
        };

        let subs_checked = self
            .check_subtitle_text(
                movie_file_id,
                episode_file_id,
                &sub_tracks,
                payload.file_id,
                duration,
            )
            .await;

        info!(
            file_id = %payload.file_id,
            keyframes = keyframe_count,
            subs_extracted = subs_extracted,
            subs_checked = subs_checked,
            "Media extraction complete"
        );

        Ok(json!({
            "file_id": payload.file_id,
            "media_type": payload.media_type,
            "keyframes": keyframe_count,
            "subs_extracted": subs_extracted,
            "subs_checked": subs_checked,
        }))
    }
}

/// Check if a file path has a Matroska container extension.
fn is_matroska_path(path: &std::path::Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .is_some_and(|ext| matches!(ext.to_ascii_lowercase().as_str(), "mkv" | "webm"))
}
