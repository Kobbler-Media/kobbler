//! Builds a book's app reader pack ahead of the first open.
//!
//! The pack routes build on demand too; this job only takes that wait off
//! the first open. Books with no EPUB/KEPUB are left to the on-demand
//! conversion the routes perform.
//!
//! Without a `book_id` it sweeps the library, queueing every book whose pack
//! of the current generation is missing — books from before reader packs,
//! and every book after a generation bump — and removing the packs of book
//! files that no longer exist. Books whose read-along text was extracted
//! before it carried real epubcfi are re-extracted, which rebuilds their
//! audio sync points too. Runs on each boot; a no-op once the library is
//! built.

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sqlx::PgPool;
use std::sync::Arc;
use tokio::sync::Semaphore;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use crate::jobs::JobResult;
use crate::jobs::worker::JobHandler;
use crate::reader_pack;
use crate::repositories::{book_files, ebook_segments};
use nats_jobs::Publisher;
use tracing::info;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReaderPackPayload {
    #[serde(default)]
    pub book_id: Option<Uuid>,
}

pub struct ReaderPackHandler {
    pool: PgPool,
    storage_root: String,
    nats_publisher: Publisher,
    fs_semaphore: Arc<Semaphore>,
}

impl ReaderPackHandler {
    pub fn new(
        pool: PgPool,
        storage_root: String,
        nats_publisher: Publisher,
        fs_semaphore: Arc<Semaphore>,
    ) -> Self {
        Self {
            pool,
            storage_root,
            nats_publisher,
            fs_semaphore,
        }
    }

    async fn sweep(&self, cancel: &CancellationToken) -> JobResult<Value> {
        let books =
            book_files::list_book_ids_with_formats(&self.pool, reader_pack::file::PACK_FORMATS)
                .await?;
        let mut queued = 0;
        for &book_id in &books {
            nats_jobs::check_cancelled(cancel)?;
            let Some(file) = reader_pack::file::find(
                &self.pool,
                &self.storage_root,
                book_id,
                reader_pack::file::PACK_FORMATS,
            )
            .await
            else {
                continue;
            };
            if !reader_pack::is_built(&self.storage_root, &file).await {
                crate::jobs::enqueue_reader_pack(&self.pool, &self.nats_publisher, book_id).await;
                queued += 1;
            }
        }
        let legacy = ebook_segments::books_with_legacy_cfis(&self.pool).await?;
        for (book_id, title) in &legacy {
            crate::jobs::enqueue_ebook_extract(&self.pool, &self.nats_publisher, *book_id, title)
                .await;
        }
        let removed = reader_pack::remove_orphans(&self.pool, &self.storage_root)
            .await
            .map_err(|e| crate::jobs::JobError::ExecutionFailed(e.to_string()))?;
        info!(
            books = books.len(),
            queued,
            removed,
            re_extracted = legacy.len(),
            "Reader pack sweep complete"
        );
        Ok(json!({
            "books": books.len(),
            "queued": queued,
            "removed": removed,
            "re_extracted": legacy.len(),
        }))
    }
}

#[async_trait]
impl JobHandler for ReaderPackHandler {
    fn job_type(&self) -> &'static str {
        "reader_pack"
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
        let payload: ReaderPackPayload = serde_json::from_value(payload)?;
        let Some(book_id) = payload.book_id else {
            return self.sweep(&cancel).await;
        };
        let Some(file) = reader_pack::file::find(
            &self.pool,
            &self.storage_root,
            book_id,
            reader_pack::file::PACK_FORMATS,
        )
        .await
        else {
            return Ok(json!({"status": "no_epub"}));
        };
        if reader_pack::is_built(&self.storage_root, &file).await {
            return Ok(json!({"status": "current", "file_id": file.id}));
        }
        let _fs_permit =
            self.fs_semaphore.acquire().await.map_err(|_| {
                crate::jobs::JobError::ExecutionFailed("FS semaphore closed".into())
            })?;
        reader_pack::ensure(&self.storage_root, &file)
            .await
            .map_err(|e| crate::jobs::JobError::ExecutionFailed(e.to_string()))?;
        Ok(json!({"status": "built", "file_id": file.id, "format": file.format}))
    }
}
