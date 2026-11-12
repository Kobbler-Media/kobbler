//! Audiobook enrichment job handler.
//!
//! Generates waveform peaks and measures LUFS loudness for an audiobook file.
//! Separated from audiobook_probe to keep probe fast and respect the
//! enrichment_enabled config toggle.

use crate::RuntimeConfigManager;
use crate::audio::peaks;
use crate::ffmpeg::transcode;
use crate::jobs::worker::JobHandler;
use crate::jobs::{JobError, JobResult};
use crate::repositories::file_loudness;
use crate::streaming::loudness::PlaybackLoudness;
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

#[derive(Debug, Deserialize, Serialize)]
pub struct EnrichAudiobookPayload {
    pub file_id: Uuid,
    pub book_id: Uuid,
}

pub struct EnrichAudiobookHandler {
    pool: PgPool,
    storage_root: String,
    runtime_config: RuntimeConfigManager,
    enrichment_semaphore: Arc<Semaphore>,
}

impl EnrichAudiobookHandler {
    pub fn new(
        pool: PgPool,
        storage_root: String,
        runtime_config: RuntimeConfigManager,
        enrichment_semaphore: Arc<Semaphore>,
    ) -> Self {
        Self {
            pool,
            storage_root,
            runtime_config,
            enrichment_semaphore,
        }
    }
}

#[async_trait]
impl JobHandler for EnrichAudiobookHandler {
    fn job_type(&self) -> &'static str {
        "enrich_audiobook"
    }

    fn max_concurrent(&self) -> usize {
        2
    }

    fn max_deliver(&self) -> Option<i64> {
        Some(20)
    }

    async fn execute(
        &self,
        job_id: Uuid,
        payload: Value,
        cancel: CancellationToken,
    ) -> JobResult<Value> {
        let config = self.runtime_config.get().await;
        if !config.enrichment_enabled {
            return Ok(json!({"status": "skipped", "reason": "enrichment_disabled"}));
        }

        let _permit = self
            .enrichment_semaphore
            .acquire()
            .await
            .map_err(|_| JobError::ExecutionFailed("Enrichment semaphore closed".into()))?;

        let payload: EnrichAudiobookPayload = serde_json::from_value(payload)
            .map_err(|e| JobError::ExecutionFailed(format!("Invalid payload: {e}")))?;

        nats_jobs::check_cancelled(&cancel)?;

        let file_path_str: String =
            sqlx::query_scalar("SELECT file_path FROM audiobook_files WHERE id = $1")
                .bind(payload.file_id)
                .fetch_optional(&self.pool)
                .await
                .map_err(|e| JobError::ExecutionFailed(format!("DB error: {e}")))?
                .ok_or_else(|| {
                    JobError::ExecutionFailed(format!(
                        "Audiobook file not found: {}",
                        payload.file_id
                    ))
                })?;

        let file_path = crate::audio::path::resolve_file_path(&self.storage_root, &file_path_str);
        if !file_path.is_file() {
            return Err(JobError::ExecutionFailed(format!(
                "File not found: {}",
                file_path.display()
            )));
        }

        info!(
            job_id = %job_id,
            file_id = %payload.file_id,
            "Starting audiobook enrichment (peaks + LUFS)"
        );

        nats_jobs::check_cancelled(&cancel)?;

        let file_path_clone = file_path.clone();

        let (peaks_result, loudness_result) = tokio::join!(
            peaks::generate_peaks(&file_path, 4),
            transcode::measure_loudness(&file_path_clone),
        );

        // Save peaks to disk
        if let Ok(peaks_data) = &peaks_result {
            let peaks_dir = PathBuf::from(&self.storage_root).join("audio_peaks");
            let _ = tokio::fs::create_dir_all(&peaks_dir).await;
            let peaks_path = peaks_dir.join(format!("{}.peaks", payload.file_id));
            if let Err(e) = tokio::fs::write(&peaks_path, peaks_data).await {
                warn!(error = %e, "Failed to write peaks file");
            }
        } else if let Err(e) = &peaks_result {
            warn!(error = %e, "Peaks generation failed (non-fatal)");
        }

        let loudness = match loudness_result {
            Ok(l) => PlaybackLoudness::from(l),
            Err(e) => {
                warn!(error = %e, "Loudness measurement failed (non-fatal)");
                PlaybackLoudness::default()
            }
        };
        if loudness.lufs.is_some()
            && let Err(e) =
                file_loudness::record(&self.pool, "audiobook", payload.file_id, loudness).await
        {
            warn!(error = %e, "Failed to save audiobook loudness");
        }
        let lufs_value = loudness.lufs;

        info!(
            file_id = %payload.file_id,
            lufs = ?lufs_value,
            peaks = peaks_result.is_ok(),
            "Audiobook enrichment complete"
        );

        Ok(json!({
            "file_id": payload.file_id,
            "lufs": lufs_value,
            "peaks": peaks_result.is_ok(),
        }))
    }
}
