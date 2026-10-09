//! Backfill walker for the single-pass media-cache pipeline.
//!
//! Stateless scan: lists movie / episode / audiobook files whose
//! `media_cache_version` is below the current `PIPELINE_VERSION`, and enqueues
//! a `media_analyze` job per file. Skips files with an already-active job to
//! stay idempotent. Per-file staleness lives in the job itself (the manifest
//! short-circuit), so this walker can re-run safely.
//!
//! Reached from the "Run Enrichment Backfill Now" button, and fired by the
//! scheduler every 30 minutes; both gate on `enrichment_enabled`. Note the
//! decode this queues produces far more than the thumbnails and LUFS that name
//! suggests — trickplay and loudness are two of eight branches, the rest being
//! what skip detection reads, and each `media_analyze` chains a `skip_detect`.

use crate::RuntimeConfigManager;
use crate::jobs::worker::JobHandler;
use crate::jobs::{JobError, JobResult};
use crate::media_cache::PIPELINE_VERSION;
use crate::repositories::{audiobook_files, movie_files, tv_episode_files};
use crate::types::MediaScope;
use async_trait::async_trait;
use nats_jobs::Publisher;
use serde::Deserialize;
use serde_json::{Value, json};
use sqlx::PgPool;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use tokio_util::sync::CancellationToken;
use tracing::info;
use uuid::Uuid;

use super::media_analyze::MediaAnalyzePayload;

#[derive(Debug, Default, Deserialize)]
pub struct MediaAnalyzeBackfillPayload {
    /// Re-enqueue every available video file instead of only those below
    /// `PIPELINE_VERSION`. Warm caches short-circuit the decode, supplement
    /// `embeddings` when vision was configured after the first pass, and
    /// always re-run `skip_detect` — which makes this the "detect intros /
    /// credits on everything I already have" button. Audiobooks are excluded:
    /// they carry no skip segments.
    #[serde(default)]
    pub all_video: bool,
    /// Restrict the walk to one kind of media; absent covers every kind this
    /// backfill handles.
    ///
    /// This is the expensive pass — a full decode per file, and each
    /// `media_analyze` chains a `skip_detect` behind it. The kinds are not
    /// interchangeable in cost or in need: `credits_window_secs` was widened
    /// for movies only, so re-analysing to pick up a wider embed window is a
    /// ~600-file movie job, not a ~10,000-file library job.
    #[serde(default)]
    pub media_type: Option<MediaScope>,
}

pub struct MediaAnalyzeBackfillHandler {
    pool: PgPool,
    publisher: Publisher,
    runtime_config: RuntimeConfigManager,
    initial_scan_complete: Arc<AtomicBool>,
}

impl MediaAnalyzeBackfillHandler {
    pub fn new(
        pool: PgPool,
        publisher: Publisher,
        runtime_config: RuntimeConfigManager,
        initial_scan_complete: Arc<AtomicBool>,
    ) -> Self {
        Self {
            pool,
            publisher,
            runtime_config,
            initial_scan_complete,
        }
    }
}

#[async_trait]
impl JobHandler for MediaAnalyzeBackfillHandler {
    fn job_type(&self) -> &'static str {
        "media_analyze_backfill"
    }

    fn max_concurrent(&self) -> usize {
        1
    }

    async fn execute(
        &self,
        _job_id: Uuid,
        payload: Value,
        cancel: CancellationToken,
    ) -> JobResult<Value> {
        let payload: MediaAnalyzeBackfillPayload =
            serde_json::from_value(payload).unwrap_or_default();
        if !self.initial_scan_complete.load(Ordering::Acquire) {
            info!("Skipping media_analyze backfill — initial media root scan not yet complete");
            return Ok(json!({"status": "skipped", "reason": "initial_scan_pending"}));
        }

        let config = self.runtime_config.get().await;
        if !config.enrichment_enabled {
            return Ok(json!({"status": "skipped", "reason": "enrichment_disabled"}));
        }

        let mut movie_enqueued = 0u32;
        let mut movie_skipped = 0u32;
        let mut episode_enqueued = 0u32;
        let mut episode_skipped = 0u32;
        let mut audiobook_enqueued = 0u32;
        let mut audiobook_skipped = 0u32;

        // ── Movies ──
        let movie_ids = if !MediaScope::Movie.covered_by(payload.media_type) {
            Vec::new()
        } else if payload.all_video {
            movie_files::list_all_with_media(&self.pool)
                .await
                .map_err(|e| JobError::ExecutionFailed(format!("DB error (movies): {e}")))?
        } else {
            movie_files::list_needing_media_cache(&self.pool, PIPELINE_VERSION)
                .await
                .map_err(|e| JobError::ExecutionFailed(format!("DB error (movies): {e}")))?
        };
        for file_id in &movie_ids {
            nats_jobs::check_cancelled(&cancel)?;
            match enqueue_media_analyze(&self.pool, &self.publisher, *file_id, "movie").await {
                BackfillEnqueue::Enqueued => movie_enqueued += 1,
                BackfillEnqueue::Skipped => movie_skipped += 1,
            }
        }

        // ── TV episodes ──
        let episode_ids = if !MediaScope::Episode.covered_by(payload.media_type) {
            Vec::new()
        } else if payload.all_video {
            tv_episode_files::list_all_with_media(&self.pool)
                .await
                .map_err(|e| JobError::ExecutionFailed(format!("DB error (episodes): {e}")))?
        } else {
            tv_episode_files::list_needing_media_cache(&self.pool, PIPELINE_VERSION)
                .await
                .map_err(|e| JobError::ExecutionFailed(format!("DB error (episodes): {e}")))?
        };
        for file_id in &episode_ids {
            nats_jobs::check_cancelled(&cancel)?;
            match enqueue_media_analyze(&self.pool, &self.publisher, *file_id, "episode").await {
                BackfillEnqueue::Enqueued => episode_enqueued += 1,
                BackfillEnqueue::Skipped => episode_skipped += 1,
            }
        }

        // ── Audiobooks ──
        let audiobook_ids =
            if payload.all_video || !MediaScope::Audiobook.covered_by(payload.media_type) {
                Vec::new()
            } else {
                audiobook_files::list_needing_media_cache(&self.pool, PIPELINE_VERSION)
                    .await
                    .map_err(|e| JobError::ExecutionFailed(format!("DB error (audiobooks): {e}")))?
            };
        for file_id in &audiobook_ids {
            nats_jobs::check_cancelled(&cancel)?;
            match enqueue_media_analyze(&self.pool, &self.publisher, *file_id, "audiobook").await {
                BackfillEnqueue::Enqueued => audiobook_enqueued += 1,
                BackfillEnqueue::Skipped => audiobook_skipped += 1,
            }
        }

        let movie_found = movie_ids.len();
        let episode_found = episode_ids.len();
        let audiobook_found = audiobook_ids.len();

        info!(
            movie_found,
            movie_enqueued,
            movie_skipped,
            episode_found,
            episode_enqueued,
            episode_skipped,
            audiobook_found,
            audiobook_enqueued,
            audiobook_skipped,
            pipeline_version = PIPELINE_VERSION,
            all_video = payload.all_video,
            media_type = payload.media_type.map(|m| m.as_str()).unwrap_or("all"),
            "media_analyze backfill complete"
        );

        Ok(json!({
            "movie_found": movie_found,
            "movie_enqueued": movie_enqueued,
            "movie_skipped": movie_skipped,
            "episode_found": episode_found,
            "episode_enqueued": episode_enqueued,
            "episode_skipped": episode_skipped,
            "audiobook_found": audiobook_found,
            "audiobook_enqueued": audiobook_enqueued,
            "audiobook_skipped": audiobook_skipped,
            "pipeline_version": PIPELINE_VERSION,
            "all_video": payload.all_video,
            "media_type": payload.media_type.map(|m| m.as_str()),
        }))
    }
}

enum BackfillEnqueue {
    Enqueued,
    Skipped,
}

/// Enqueue a `media_analyze` job for one file, skipping if one is already
/// active. The job's own short-circuit handles "already up-to-date" (manifest
/// covers all branches at PIPELINE_VERSION).
async fn enqueue_media_analyze(
    pool: &PgPool,
    publisher: &Publisher,
    file_id: Uuid,
    media_type: &str,
) -> BackfillEnqueue {
    let fid = file_id.to_string();
    if let Ok(true) = crate::jobs::has_active_job(pool, "media_analyze", "file_id", &fid).await {
        return BackfillEnqueue::Skipped;
    }
    let payload = MediaAnalyzePayload {
        file_id,
        media_type: media_type.to_string(),
        force_regen: false,
        run_id: None,
        triggered_by: "backfill".to_string(),
    };
    let context = crate::jobs::JobContext::new("Backfill — media analyze");
    if crate::jobs::enqueue_routine(
        pool,
        publisher,
        "media_analyze",
        serde_json::to_value(&payload).unwrap_or_default(),
        context,
    )
    .await
    .is_ok()
    {
        BackfillEnqueue::Enqueued
    } else {
        BackfillEnqueue::Skipped
    }
}
