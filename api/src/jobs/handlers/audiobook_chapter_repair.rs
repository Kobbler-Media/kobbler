//! Audiobook chapter repair.
//!
//! Each book's other editions are filed first (`audio::book_editions`), so
//! the chapters are settled over the files the book keeps.
//!
//! Older chapter writers merged several sources into one file's chapters,
//! kept a transcript run that found two chapters in a forty-hour book, split
//! chapter-sized files at their silences, and read "Chapter twenty four" as
//! "Chapter twenty". This re-settles every audiobook's chapters the way the
//! probe now would (`audio::book_chapters`) and rewrites the files that come
//! out different, so running it again changes nothing.
//!
//! - **sweep** (no `book_id`) — plan every audiobook without running silence
//!   detection, report what would change, and enqueue a repair per book.
//! - **repair** (`book_id` set) — plan that book in full and write it.

use crate::audio::book_chapters::{self, Silence};
use crate::audio::book_editions;
use crate::jobs::worker::JobHandler;
use crate::jobs::{JobContext, JobResult};
use crate::repositories::audiobook_files;
use crate::search::SearchBackend;
use async_trait::async_trait;
use nats_jobs::Publisher;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sqlx::PgPool;
use std::sync::Arc;
use tokio_util::sync::CancellationToken;
use tracing::{info, warn};
use uuid::Uuid;

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct AudiobookChapterRepairPayload {
    #[serde(default)]
    pub book_id: Option<Uuid>,
    #[serde(default)]
    pub dry_run: bool,
}

pub struct AudiobookChapterRepairHandler {
    pool: PgPool,
    storage_root: String,
    nats_publisher: Publisher,
    search: Arc<dyn SearchBackend>,
}

impl AudiobookChapterRepairHandler {
    pub fn new(
        pool: PgPool,
        storage_root: String,
        nats_publisher: Publisher,
        search: Arc<dyn SearchBackend>,
    ) -> Self {
        Self {
            pool,
            storage_root,
            nats_publisher,
            search,
        }
    }

    async fn sweep(&self, dry_run: bool, cancel: &CancellationToken) -> JobResult<Value> {
        let books = audiobook_files::list_audiobooks(&self.pool).await?;
        let mut findings = Vec::new();
        for book in &books {
            nats_jobs::check_cancelled(cancel)?;
            let editions =
                book_editions::inspect(&self.pool, &self.storage_root, book.book_id).await?;
            let plan =
                book_chapters::plan(&self.pool, &self.storage_root, book.book_id, Silence::Skip)
                    .await?;
            if plan.changes.is_empty() && editions.is_empty() {
                continue;
            }
            let reasons: Vec<String> = plan.changes.iter().map(|c| c.reason.clone()).collect();
            info!(book = %book.title, reasons = ?reasons, editions = ?editions, "audiobook needs repair");
            findings.push(json!({
                "book_id": book.book_id,
                "title": book.title,
                "files": reasons.len(),
                "reasons": reasons,
                "editions": editions,
            }));
            if dry_run {
                continue;
            }
            let context = JobContext::for_entity(
                "book",
                book.book_id,
                format!("Repairing chapters — {}", book.title),
            )
            .with_display_opt(
                crate::jobs::fetch_job_display(&self.pool, "book", book.book_id).await,
            );
            if let Err(e) = crate::jobs::enqueue(
                &self.pool,
                &self.nats_publisher,
                "audiobook_chapter_repair",
                json!({ "book_id": book.book_id }),
                context,
            )
            .await
            {
                warn!(error = %e, book_id = %book.book_id, "failed to enqueue chapter repair");
            }
        }
        Ok(json!({
            "dry_run": dry_run,
            "books_checked": books.len(),
            "books_to_repair": findings.len(),
            "findings": findings,
        }))
    }

    async fn repair(&self, book_id: Uuid) -> JobResult<Value> {
        let editions = book_editions::settle(&self.pool, &self.storage_root, book_id, None).await?;
        if editions.changed() {
            book_editions::follow_up(
                &self.pool,
                &self.nats_publisher,
                book_id,
                &editions,
                "audiobook_chapter_repair",
            )
            .await;
            for book in editions.other_books() {
                self.reindex_transcript(book).await;
            }
        }
        let plan =
            book_chapters::plan(&self.pool, &self.storage_root, book_id, Silence::Detect).await?;
        let written = book_chapters::apply(&self.pool, &plan).await?;
        if written > 0 || editions.changed() {
            self.reindex_transcript(book_id).await;
        }
        let reasons: Vec<&str> = plan.changes.iter().map(|c| c.reason.as_str()).collect();
        Ok(json!({
            "files_rewritten": written,
            "reasons": reasons,
            "editions": editions.report,
        }))
    }

    async fn reindex_transcript(&self, book_id: Uuid) {
        if let Err(e) = crate::search::index_transcript_segments_for_book(
            &self.pool,
            self.search.as_ref(),
            book_id,
        )
        .await
        {
            warn!(error = %e, book_id = %book_id, "failed to re-index transcript after audiobook repair");
        }
    }
}

#[async_trait]
impl JobHandler for AudiobookChapterRepairHandler {
    fn job_type(&self) -> &'static str {
        "audiobook_chapter_repair"
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
        let payload: AudiobookChapterRepairPayload = serde_json::from_value(payload)?;
        nats_jobs::check_cancelled(&cancel)?;
        match payload.book_id {
            Some(book_id) => self.repair(book_id).await,
            None => self.sweep(payload.dry_run, &cancel).await,
        }
    }
}
