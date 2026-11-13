//! Handler for writing metadata to ebook files.

use crate::convert::{EbookMeta, MetadataWrite};
use crate::jobs::JobError;
use crate::jobs::worker::JobHandler;
use crate::repositories::{authors, book_files, books, series};
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sqlx::PgPool;
use std::path::PathBuf;
use tokio_util::sync::CancellationToken;
use tracing::{debug, info, warn};
use uuid::Uuid;

/// Payload for metadata_write jobs.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MetadataWritePayload {
    /// The book ID to write metadata for.
    pub book_id: Uuid,
}

/// Handler for metadata_write jobs.
pub struct MetadataWriteHandler {
    pool: PgPool,
    ebook_meta: EbookMeta,
}

impl MetadataWriteHandler {
    pub fn new(pool: PgPool) -> Self {
        Self {
            pool,
            ebook_meta: EbookMeta::new(),
        }
    }
}

#[async_trait]
impl JobHandler for MetadataWriteHandler {
    fn job_type(&self) -> &'static str {
        "metadata_write"
    }

    async fn execute(
        &self,
        job_id: Uuid,
        payload: Value,
        cancel: CancellationToken,
    ) -> Result<Value, JobError> {
        let payload: MetadataWritePayload = serde_json::from_value(payload)?;
        nats_jobs::check_cancelled(&cancel)?;
        let book_id = payload.book_id;

        info!(job_id = %job_id, book_id = %book_id, "Writing metadata to ebook files");

        // Fetch book metadata from database
        let book = books::get_for_metadata_write(&self.pool, book_id)
            .await?
            .ok_or_else(|| JobError::ExecutionFailed(format!("Book not found: {}", book_id)))?;

        // Fetch authors
        let author_names = authors::get_book_author_names(&self.pool, book_id).await?;

        // Fetch series name if series_id is set
        let series_name = match book.series_id {
            Some(sid) => series::get_name(&self.pool, sid).await?,
            None => None,
        };

        // Fetch ebook file paths (KEPUB and EPUB only)
        let files = book_files::list_for_metadata_write(&self.pool, book_id).await?;

        if files.is_empty() {
            return Ok(json!({
                "skipped": true,
                "reason": "No KEPUB or EPUB files found"
            }));
        }

        // Build metadata to write
        let metadata = MetadataWrite {
            title: Some(book.title),
            authors: if author_names.is_empty() {
                None
            } else {
                Some(author_names)
            },
            description: book.description,
            publisher: book.publisher,
            pubdate: book.pubdate.map(|d| {
                let format =
                    time::format_description::parse_borrowed::<2>("[year]-[month]-[day]").unwrap();
                d.format(&format).unwrap_or_default()
            }),
            language: book.language,
            series: series_name,
            series_index: book.series_index,
        };

        // Write metadata to each file
        let mut updated_formats = Vec::new();
        let mut errors = Vec::new();

        for file in files {
            let path = PathBuf::from(&file.file_path);

            if !tokio::fs::try_exists(&path).await.unwrap_or(false) {
                warn!(path = %file.file_path, "File not found, skipping");
                errors.push(format!("{}: file not found", file.format));
                continue;
            }

            debug!(format = %file.format, path = %file.file_path, "Writing metadata");

            match self.ebook_meta.write_metadata(&path, &metadata).await {
                Ok(()) => {
                    info!(format = %file.format, "Metadata written successfully");
                    // Update size_bytes in DB (no content_revision bump)
                    if let Ok(meta) = tokio::fs::metadata(&path).await {
                        let new_size = meta.len() as i64;
                        if let Err(e) =
                            book_files::update_size_bytes(&self.pool, file.id, new_size).await
                        {
                            warn!(file_id = %file.id, error = %e, "Failed to update file size after metadata write");
                        }
                    }
                    updated_formats.push(file.format);
                }
                Err(e) => {
                    warn!(format = %file.format, error = %e, "Failed to write metadata");
                    errors.push(format!("{}: {}", file.format, e));
                }
            }
        }

        if updated_formats.is_empty() && !errors.is_empty() {
            return Err(JobError::ExecutionFailed(errors.join("; ")));
        }

        Ok(json!({
            "updated_formats": updated_formats,
            "errors": errors
        }))
    }
}
