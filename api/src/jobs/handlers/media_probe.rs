//! Media probe job handler.
//!
//! Runs ffprobe on a movie or episode file, then updates the DB with video
//! metadata and inserts audio/subtitle track records. After the fast probe,
//! enqueues a media_extract job for heavy I/O work (keyframes + subtitles).

use crate::RuntimeConfigManager;
use crate::ffmpeg::{persist, probe};
use crate::jobs::worker::JobHandler;
use crate::jobs::{JobError, JobResult};
use crate::repositories::{movie_files, tv_episode_files};
use async_trait::async_trait;
use nats_jobs::Publisher;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sqlx::PgPool;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::Semaphore;
use tokio_util::sync::CancellationToken;
use tracing::{info, warn};
use uuid::Uuid;

use super::media_analyze::MediaAnalyzePayload;
use super::media_extract::MediaExtractPayload;

/// Payload for media_probe jobs.
#[derive(Debug, Deserialize, Serialize)]
pub struct MediaProbePayload {
    pub file_id: Uuid,
    /// "movie" or "episode"
    pub media_type: String,
}

pub struct MediaProbeHandler {
    pool: PgPool,
    publisher: Publisher,
    runtime_config: RuntimeConfigManager,
    fs_semaphore: Arc<Semaphore>,
}

impl MediaProbeHandler {
    pub fn new(
        pool: PgPool,
        publisher: Publisher,
        runtime_config: RuntimeConfigManager,
        fs_semaphore: Arc<Semaphore>,
    ) -> Self {
        Self {
            pool,
            publisher,
            runtime_config,
            fs_semaphore,
        }
    }
}

#[async_trait]
impl JobHandler for MediaProbeHandler {
    fn job_type(&self) -> &'static str {
        "media_probe"
    }

    fn max_concurrent(&self) -> usize {
        self.runtime_config.job_concurrency("media_probe", 16)
    }

    async fn execute(
        &self,
        job_id: Uuid,
        payload: Value,
        cancel: CancellationToken,
    ) -> JobResult<Value> {
        nats_jobs::check_cancelled(&cancel)?;
        let _fs_permit = self
            .fs_semaphore
            .acquire()
            .await
            .map_err(|_| JobError::ExecutionFailed("FS semaphore closed".into()))?;
        let payload: MediaProbePayload = serde_json::from_value(payload)
            .map_err(|e| JobError::ExecutionFailed(format!("Invalid payload: {e}")))?;

        // Serialize work on this file. Probe runs 16-wide and analyze 2-wide,
        // and two passes over the same file write the same cache directory.
        // Queued, not dropped: a later pass is usually the one that sees the
        // finished download.
        let _file_lock = crate::jobs::file_lock::lock_file(payload.file_id).await;

        info!(
            job_id = %job_id,
            file_id = %payload.file_id,
            media_type = %payload.media_type,
            "Starting media probe"
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

        // Resolve the full path — file_path is already absolute on disk
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

        let result = persist::probe_and_persist(
            &self.pool,
            payload.file_id,
            &full_path,
            &payload.media_type,
        )
        .await
        .map_err(JobError::ExecutionFailed)?;

        // Completeness gate.
        //
        // ffprobe answering does not mean the file is all there. A download
        // that stopped part-way keeps a valid header and a plausible duration
        // and decodes fine; the missing pieces are just zeros. Sampling for
        // those costs ~768 KB against the full decode that is about to happen
        // anyway, and it is the only check that catches this at ingest instead
        // of two months later.
        //
        // Deliberately not `mark_unreadable`: a file still downloading is not
        // broken, and quarantining it would take it out of `list_needing_probe`
        // for good. Skipping the analyze chain leaves the metadata that was
        // just persisted and lets the next scan re-probe once the file settles.
        if result.is_some() {
            let size = tokio::fs::metadata(&full_path)
                .await
                .map(|m| m.len())
                .unwrap_or(0);
            if let Some(zero_blocks) =
                crate::media_cache::integrity::count_zero_blocks_async(&full_path, size).await
                && zero_blocks > 0
            {
                warn!(
                    file_id = %payload.file_id,
                    path = %full_path.display(),
                    zero_blocks,
                    "media_probe: file looks incomplete — not chaining analysis"
                );
                // Say so out loud. The whole reason this check exists is that
                // three files rotted silently for two months; catching it and
                // staying quiet would repeat the same mistake, just faster.
                let name = full_path
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_else(|| payload.file_id.to_string());
                crate::notifications::notify_agents(
                    &self.pool,
                    &self.publisher,
                    &crate::notifications::NotificationPayload {
                        trigger: crate::notifications::NotificationTrigger::OnMediaCorrupt,
                        title: "Incomplete file detected".to_string(),
                        message: format!(
                            "{name} looks incomplete at import — {zero_blocks} of 12 sampled \
                             blocks are empty, which is what an unfinished download leaves \
                             behind. It has not been analyzed. Path: {}",
                            full_path.display()
                        ),
                        entity_type: Some(payload.media_type.clone()),
                        entity_id: Some(payload.file_id),
                        entity_name: Some(name),
                        url: None,
                        public_url: None,
                        show_id: None,
                        show_name: None,
                        season_number: None,
                        show_year: None,
                        upgraded: false,
                        details: None,
                    },
                )
                .await;
                return Ok(json!({
                    "file_id": payload.file_id,
                    "media_type": payload.media_type,
                    "probed": true,
                    "status": "deferred",
                    "reason": "incomplete_file",
                    "zero_blocks": zero_blocks,
                }));
            }
        }

        match result {
            Some(probe) => {
                // Enqueue heavy extraction (keyframes + subtitles) as a separate job,
                // unless the file is universally direct-playable (MP4 + H.264 + AAC/opus/flac).
                // Direct-play files use HTTP Range serving, never HLS, so keyframes are unused.
                if probe::is_direct_playable(&probe) {
                    info!(
                        file_id = %payload.file_id,
                        "Skipping media_extract for direct-play file"
                    );
                } else {
                    let extract_payload = MediaExtractPayload {
                        file_id: payload.file_id,
                        media_type: payload.media_type.clone(),
                    };
                    let context = crate::jobs::JobContext::new("Extracting keyframes & subtitles");
                    if let Err(e) = crate::jobs::enqueue_routine(
                        &self.pool,
                        &self.publisher,
                        "media_extract",
                        serde_json::to_value(&extract_payload).unwrap(),
                        context,
                    )
                    .await
                    {
                        warn!(error = %e, file_id = %payload.file_id, "Failed to enqueue media_extract job");
                    }
                }

                // Single-pass media analysis. Produces the full sidecar
                // cache (trickplay, vhash, chromaprint, silence, loudness,
                // black_frames) in one decode and chains `skip_detect`
                // from inside media_analyze once the cache is durable.
                {
                    let config = self.runtime_config.get().await;
                    if config.enrichment_enabled {
                        let analyze_payload = MediaAnalyzePayload {
                            file_id: payload.file_id,
                            media_type: payload.media_type.clone(),
                            force_regen: false,
                            run_id: None,
                            triggered_by: "auto_ingest".to_string(),
                            gaps_only: false,
                        };
                        let context = crate::jobs::JobContext::new("Analyzing media (single-pass)");
                        if let Err(e) = crate::jobs::enqueue_routine(
                            &self.pool,
                            &self.publisher,
                            "media_analyze",
                            serde_json::to_value(&analyze_payload).unwrap(),
                            context,
                        )
                        .await
                        {
                            warn!(error = %e, file_id = %payload.file_id, "Failed to enqueue media_analyze job");
                        }
                    }
                }

                let video = probe.video_streams.first();
                info!(
                    file_id = %payload.file_id,
                    codec = ?video.map(|v| &v.codec),
                    resolution = ?video.map(|v| format!("{}x{}", v.width, v.height)),
                    audio_tracks = probe.audio_streams.len(),
                    subtitle_tracks = probe.subtitle_streams.len(),
                    "Media probe complete"
                );
                Ok(json!({
                    "file_id": payload.file_id,
                    "media_type": payload.media_type,
                    "codec": video.map(|v| &v.codec),
                    "resolution": video.map(|v| format!("{}x{}", v.width, v.height)),
                    "audio_tracks": probe.audio_streams.len(),
                    "subtitle_tracks": probe.subtitle_streams.len(),
                }))
            }
            None => {
                warn!(
                    file_id = %payload.file_id,
                    "FFprobe returned no result — marking file as unreadable"
                );
                // Set video_codec = 'unreadable' so list_needing_probe skips this file
                // on future backfills (it checks WHERE video_codec IS NULL).
                let mark_result = match payload.media_type.as_str() {
                    "movie" => movie_files::mark_unreadable(&self.pool, payload.file_id).await,
                    "episode" => {
                        tv_episode_files::mark_unreadable(&self.pool, payload.file_id).await
                    }
                    _ => Ok(false),
                };
                if let Err(e) = mark_result {
                    warn!(error = %e, file_id = %payload.file_id, "Failed to mark file as unreadable");
                }
                Ok(json!({
                    "file_id": payload.file_id,
                    "media_type": payload.media_type,
                    "probed": false,
                }))
            }
        }
    }
}
