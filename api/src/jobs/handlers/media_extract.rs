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
use async_trait::async_trait;
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

        // Look up the file record to get file_path
        let file_path = match payload.media_type.as_str() {
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
                file.file_path
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
                file.file_path
            }
            other => {
                return Err(JobError::ExecutionFailed(format!(
                    "Unknown media type: {other}"
                )));
            }
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

        let keyframe_count = match keyframe_result {
            Ok(keyframes) if !keyframes.is_empty() => {
                let kf_value = serde_json::to_value(&keyframes).unwrap_or_default();
                let count = keyframes.len();
                let save_result = match payload.media_type.as_str() {
                    "movie" => {
                        movie_files::update_keyframes(&self.pool, payload.file_id, &kf_value).await
                    }
                    "episode" => {
                        tv_episode_files::update_keyframes(&self.pool, payload.file_id, &kf_value)
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
        };

        nats_jobs::check_cancelled(&cancel)?;

        // Pre-extract text subtitles so they're instantly available at playback.
        // Query subtitle tracks from DB (already populated by media_probe).
        let sub_tracks = match payload.media_type.as_str() {
            "movie" => subtitle_tracks::list_by_movie_file(&self.pool, payload.file_id).await,
            "episode" => subtitle_tracks::list_by_episode_file(&self.pool, payload.file_id).await,
            _ => Ok(vec![]),
        };

        let subs_extracted = match sub_tracks {
            Ok(tracks) => {
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
            }
            Err(e) => {
                warn!(error = %e, "Failed to query subtitle tracks (non-fatal)");
                0
            }
        };

        info!(
            file_id = %payload.file_id,
            keyframes = keyframe_count,
            subs_extracted = subs_extracted,
            "Media extraction complete"
        );

        Ok(json!({
            "file_id": payload.file_id,
            "media_type": payload.media_type,
            "keyframes": keyframe_count,
            "subs_extracted": subs_extracted,
        }))
    }
}

/// Check if a file path has a Matroska container extension.
fn is_matroska_path(path: &std::path::Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .is_some_and(|ext| matches!(ext.to_ascii_lowercase().as_str(), "mkv" | "webm"))
}
