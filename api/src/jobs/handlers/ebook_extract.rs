//! Ebook text extraction job handler.
//!
//! Extracts the book's text segments for search indexing and read-along.
//! Segments are stored in the `ebook_segments` table keyed by the reader
//! file's `book_file_id`.

use crate::jobs::JobResult;
use crate::jobs::worker::JobHandler;
use crate::reader_pack;
use crate::repositories::{audiobook_transcripts, ebook_segments};
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

        let Some(file) = reader_pack::file::find(
            &self.pool,
            &self.storage_root,
            book_id,
            reader_pack::file::PACK_FORMATS,
        )
        .await
        else {
            info!(book_id = %book_id, "No EPUB or KEPUB file found, skipping extraction");
            return Ok(json!({"status": "no_epub"}));
        };
        let file_id = file.id;

        // Built from the reader pack so each segment carries the epubcfi the
        // readers resume from, in the same document they open.
        let pack = reader_pack::ensure(&self.storage_root, &file)
            .await
            .map_err(|e| {
                crate::jobs::JobError::ExecutionFailed(format!("EPUB extraction failed: {e}"))
            })?;
        let segments =
            tokio::task::spawn_blocking(move || reader_pack::segments::text_segments(&pack))
                .await
                .map_err(|e| crate::jobs::JobError::ExecutionFailed(format!("Join error: {e}")))?
                .map_err(|e| {
                    crate::jobs::JobError::ExecutionFailed(format!("EPUB extraction failed: {e}"))
                })?;

        let segment_count = segments.len();

        ebook_segments::delete_for_book(&self.pool, book_id).await?;

        let inputs: Vec<ebook_segments::EbookSegmentInput> = segments
            .into_iter()
            .map(|s| ebook_segments::EbookSegmentInput {
                segment_index: s.index,
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
