//! Chapter re-detection job handler.
//!
//! Deletes existing "detected" chapters and re-runs pure silence-based
//! chapter detection. Does not touch "transcript" or other chapter sources.

use crate::audio::chapters;
use crate::jobs::JobResult;
use crate::jobs::worker::JobHandler;
use crate::repositories::{audiobook_chapters, audiobook_files};
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sqlx::PgPool;
use tokio_util::sync::CancellationToken;
use tracing::{info, warn};
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChapterRedetectPayload {
    pub book_id: Uuid,
}

pub struct ChapterRedetectHandler {
    pool: PgPool,
    storage_root: String,
}

impl ChapterRedetectHandler {
    pub fn new(pool: PgPool, storage_root: String) -> Self {
        Self { pool, storage_root }
    }
}

#[async_trait]
impl JobHandler for ChapterRedetectHandler {
    fn job_type(&self) -> &'static str {
        "chapter_redetect"
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
        let payload: ChapterRedetectPayload = serde_json::from_value(payload)?;
        nats_jobs::check_cancelled(&cancel)?;
        let book_id = payload.book_id;

        let files = audiobook_files::list_by_book(&self.pool, book_id).await?;
        if files.is_empty() {
            return Ok(json!({"status": "no_files"}));
        }

        let mut total_detected = 0u64;

        for file in &files {
            // Delete existing detected chapters only (preserve transcript and other sources)
            if let Err(e) =
                audiobook_chapters::delete_by_source(&self.pool, file.id, "detected").await
            {
                warn!(error = %e, file_id = %file.id, "Failed to delete old detected chapters");
            }

            let file_path =
                crate::audio::path::resolve_file_path(&self.storage_root, &file.file_path);
            if !tokio::fs::metadata(&file_path)
                .await
                .is_ok_and(|m| m.is_file())
            {
                warn!(
                    file_id = %file.id,
                    path = %file_path.display(),
                    "Audio file not found on disk, skipping chapter detection"
                );
                continue;
            }

            // Run pure silence detection
            let (gaps, duration) = chapters::detect_silence_gaps(&file_path).await;
            let detected = chapters::select_chapter_boundaries(&gaps, duration);

            if detected.is_empty() {
                continue;
            }

            let count = detected.len() as u64;
            match audiobook_chapters::store_chapters(&self.pool, file.id, &detected).await {
                Ok(true) => {
                    total_detected += count;
                    info!(
                        file_id = %file.id,
                        chapters = count,
                        "Re-detected chapters via silence analysis"
                    );
                }
                Ok(false) => {}
                Err(e) => {
                    warn!(error = %e, file_id = %file.id, "Failed to store detected chapters");
                }
            }
        }

        info!(
            book_id = %book_id,
            total_detected,
            "Chapter re-detection complete"
        );

        Ok(json!({
            "detected_chapters": total_detected,
        }))
    }
}
