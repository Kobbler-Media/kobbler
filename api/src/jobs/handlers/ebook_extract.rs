//! Ebook text extraction job handler.
//!
//! Extracts plain text segments from EPUB files for search indexing
//! and text-based features. Segments are stored in the `ebook_segments`
//! table keyed by `book_file_id`.

use crate::ebook::extract;
use crate::jobs::JobResult;
use crate::jobs::worker::JobHandler;
use crate::repositories::{audiobook_transcripts, book_files, ebook_segments};
use crate::search::SearchBackend;
use async_trait::async_trait;
use nats_jobs::Publisher;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sqlx::PgPool;
use std::sync::Arc;
use tokio::sync::Semaphore;
use tokio_util::sync::CancellationToken;
use tracing::{info, warn};
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EbookExtractPayload {
    pub book_id: Uuid,
}

pub struct EbookExtractHandler {
    pool: PgPool,
    storage_root: String,
    nats_publisher: Publisher,
    search: Arc<dyn SearchBackend>,
    fs_semaphore: Arc<Semaphore>,
}

impl EbookExtractHandler {
    pub fn new(
        pool: PgPool,
        storage_root: String,
        nats_publisher: Publisher,
        search: Arc<dyn SearchBackend>,
        fs_semaphore: Arc<Semaphore>,
    ) -> Self {
        Self {
            pool,
            storage_root,
            nats_publisher,
            search,
            fs_semaphore,
        }
    }
}

#[async_trait]
impl JobHandler for EbookExtractHandler {
    fn job_type(&self) -> &'static str {
        "ebook_extract"
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
        nats_jobs::check_cancelled(&cancel)?;
        let _fs_permit =
            self.fs_semaphore.acquire().await.map_err(|_| {
                crate::jobs::JobError::ExecutionFailed("FS semaphore closed".into())
            })?;
        let payload: EbookExtractPayload = serde_json::from_value(payload)?;
        let book_id = payload.book_id;

        // Find the EPUB file for this book
        let epub_path = book_files::get_path_by_format(&self.pool, book_id, "EPUB").await?;
        let epub_path = match epub_path {
            Some(p) => p,
            None => {
                info!(book_id = %book_id, "No EPUB file found, skipping extraction");
                return Ok(json!({"status": "no_epub"}));
            }
        };

        // We need the file ID for ebook_segments — get preferred EPUB file
        let (file_id, _) = match book_files::get_preferred_file(&self.pool, book_id).await? {
            Some(f) => f,
            None => {
                info!(book_id = %book_id, "No available book file found");
                return Ok(json!({"status": "no_file"}));
            }
        };

        // Resolve file path
        let file_path = crate::audio::path::resolve_file_path(&self.storage_root, &epub_path);
        if !file_path.is_file() {
            warn!(
                book_id = %book_id,
                path = %file_path.display(),
                "EPUB file not found on disk"
            );
            return Ok(json!({"status": "file_not_found"}));
        }

        // Extract text from EPUB (CPU-bound, run in blocking thread)
        let segments = tokio::task::spawn_blocking(move || extract::extract_epub_text(&file_path))
            .await
            .map_err(|e| crate::jobs::JobError::ExecutionFailed(format!("Join error: {e}")))?
            .map_err(|e| {
                crate::jobs::JobError::ExecutionFailed(format!("EPUB extraction failed: {e}"))
            })?;

        let segment_count = segments.len();

        // Delete existing segments for this file
        ebook_segments::delete_for_file(&self.pool, file_id).await?;

        // Convert to repository input type and insert
        let inputs: Vec<ebook_segments::EbookSegmentInput> = segments
            .into_iter()
            .map(|s| ebook_segments::EbookSegmentInput {
                segment_index: s.segment_index,
                cfi: s.cfi,
                text: s.text,
                chapter_title: s.chapter_title,
            })
            .collect();

        if !inputs.is_empty() {
            ebook_segments::insert_segments(&self.pool, file_id, &inputs).await?;
        }

        info!(
            book_id = %book_id,
            file_id = %file_id,
            segment_count,
            "Ebook text extraction complete"
        );

        // Index segments in MeiliSearch for hybrid semantic search
        if segment_count > 0
            && let Err(e) = crate::search::index_transcript_segments_for_book(
                &self.pool,
                self.search.as_ref(),
                book_id,
            )
            .await
        {
            warn!(error = %e, book_id = %book_id, "Failed to index ebook segments");
        }

        // If audio transcript exists, trigger cross-format sync
        if segment_count > 0
            && audiobook_transcripts::has_transcript(&self.pool, book_id)
                .await
                .unwrap_or(false)
        {
            crate::jobs::enqueue_cross_format_sync(
                &self.pool,
                &self.nats_publisher,
                book_id,
                "auto",
            )
            .await;
        }

        Ok(json!({
            "segment_count": segment_count,
            "file_id": file_id.to_string(),
        }))
    }
}
