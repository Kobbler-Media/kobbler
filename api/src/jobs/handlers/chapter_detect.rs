//! Transcript-based chapter detection job handler.
//!
//! Runs after transcription. Only files without chapters of their own are
//! touched, and a transcript that announces too few chapters to be believed
//! leaves the book as it was.

use crate::audio::book_chapters::{self, Transcript};
use crate::jobs::JobResult;
use crate::jobs::worker::JobHandler;
use crate::repositories::{audiobook_chapters, audiobook_files};
use crate::search::SearchBackend;
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sqlx::PgPool;
use std::collections::HashMap;
use std::sync::Arc;
use tokio_util::sync::CancellationToken;
use tracing::{info, warn};
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChapterDetectPayload {
    pub book_id: Uuid,
}

pub struct ChapterDetectHandler {
    pool: PgPool,
    search: Arc<dyn SearchBackend>,
}

impl ChapterDetectHandler {
    pub fn new(pool: PgPool, search: Arc<dyn SearchBackend>) -> Self {
        Self { pool, search }
    }
}

#[async_trait]
impl JobHandler for ChapterDetectHandler {
    fn job_type(&self) -> &'static str {
        "chapter_detect"
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
        let payload: ChapterDetectPayload = serde_json::from_value(payload)?;
        nats_jobs::check_cancelled(&cancel)?;
        let book_id = payload.book_id;

        let files = audiobook_files::list_by_book(&self.pool, book_id).await?;
        if files.is_empty() {
            return Ok(json!({"status": "no_files"}));
        }

        let stored = book_chapters::stored_chapters(&self.pool, &files).await?;
        let targets = book_chapters::files_without_own_chapters(&files, &stored);
        if targets.is_empty() {
            return Ok(json!({"detected_chapters": 0, "reason": "files_have_own_chapters"}));
        }

        let mut by_file: HashMap<Uuid, Vec<_>> =
            match book_chapters::transcript_chapters(&self.pool, &files, &targets).await? {
                Transcript::Chapters(by_file) => by_file.into_iter().collect(),
                Transcript::Skipped(reason) => {
                    return Ok(json!({"detected_chapters": 0, "reason": reason}));
                }
            };

        let mut total_detected = 0u64;
        for file_id in &targets {
            let inputs = by_file.remove(file_id).unwrap_or_default();
            if inputs.is_empty() {
                for source in ["transcript", "filename", "detected"] {
                    if let Err(e) =
                        audiobook_chapters::delete_by_source(&self.pool, *file_id, source).await
                    {
                        warn!(error = %e, file_id = %file_id, source, "Failed to delete prior chapters");
                    }
                }
                continue;
            }
            let count = inputs.len() as u64;
            match audiobook_chapters::store_chapters(&self.pool, *file_id, &inputs).await {
                Ok(true) => {
                    total_detected += count;
                    info!(file_id = %file_id, chapters = count, "Stored detected chapters");
                }
                Ok(false) => {
                    info!(file_id = %file_id, "Kept the file's own chapters over detected ones");
                }
                Err(e) => {
                    warn!(error = %e, file_id = %file_id, "Failed to store detected chapters");
                }
            }
        }

        info!(
            book_id = %book_id,
            total_detected,
            files = files.len(),
            "Book-level chapter detection complete"
        );

        if total_detected > 0
            && let Err(e) = crate::search::index_transcript_segments_for_book(
                &self.pool,
                self.search.as_ref(),
                book_id,
            )
            .await
        {
            warn!(error = %e, book_id = %book_id, "Failed to re-index transcript segments after chapter detection");
        }

        Ok(json!({"detected_chapters": total_detected}))
    }
}
