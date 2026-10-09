//! Audiobook conversion job handler.
//!
//! Converts audiobook files to MP3Z format for Kobo device sync.
//! Splits M4B/audio into per-chapter MP3 files, then bundles into a ZIP.

use crate::audio::convert::{create_mp3z, split_to_mp3_chapters};
use crate::jobs::worker::JobHandler;
use crate::jobs::{JobError, JobResult};
use crate::repositories::{audiobook_chapters, book_files};
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sqlx::PgPool;
use std::path::PathBuf;
use tokio_util::sync::CancellationToken;
use tracing::info;
use uuid::Uuid;

/// Payload for audiobook_convert jobs.
#[derive(Debug, Deserialize, Serialize)]
pub struct AudiobookConvertPayload {
    pub book_id: Uuid,
    pub source_file_id: Uuid,
}

pub struct AudiobookConvertHandler {
    pool: PgPool,
    storage_root: String,
}

impl AudiobookConvertHandler {
    pub fn new(pool: PgPool, storage_root: String) -> Self {
        Self { pool, storage_root }
    }
}

#[async_trait]
impl JobHandler for AudiobookConvertHandler {
    fn job_type(&self) -> &'static str {
        "audiobook_convert"
    }

    async fn execute(
        &self,
        job_id: Uuid,
        payload: Value,
        cancel: CancellationToken,
    ) -> JobResult<Value> {
        let payload: AudiobookConvertPayload = serde_json::from_value(payload)
            .map_err(|e| JobError::ExecutionFailed(format!("Invalid payload: {e}")))?;

        nats_jobs::check_cancelled(&cancel)?;

        info!(
            job_id = %job_id,
            book_id = %payload.book_id,
            source_file_id = %payload.source_file_id,
            "Starting audiobook conversion to MP3Z"
        );

        // Check if MP3Z already exists for this book
        let existing = book_files::get_path_by_format(&self.pool, payload.book_id, "mp3z")
            .await
            .map_err(|e| JobError::ExecutionFailed(format!("DB error: {e}")))?;

        if existing.is_some() {
            info!(book_id = %payload.book_id, "MP3Z already exists, skipping conversion");
            return Ok(json!({ "skipped": true, "reason": "mp3z already exists" }));
        }

        // Get source audio file
        let source_file =
            book_files::get_by_id_for_book(&self.pool, payload.source_file_id, payload.book_id)
                .await
                .map_err(|e| JobError::ExecutionFailed(format!("DB error: {e}")))?
                .ok_or_else(|| JobError::ExecutionFailed("Source file not found".to_string()))?;

        let source_path = source_file
            .file_path
            .as_ref()
            .map(PathBuf::from)
            .ok_or_else(|| JobError::ExecutionFailed("Source file has no path".to_string()))?;
        if !source_path.is_file() {
            return Err(JobError::ExecutionFailed(format!(
                "Source file does not exist: {}",
                source_path.display()
            )));
        }

        // Get chapters for the source file
        let chapters = audiobook_chapters::get_chapters(&self.pool, payload.source_file_id)
            .await
            .map_err(|e| JobError::ExecutionFailed(format!("DB error: {e}")))?;

        if chapters.is_empty() {
            return Err(JobError::ExecutionFailed(
                "No chapters found for source file".to_string(),
            ));
        }

        // Convert chapter rows to ChapterInput for the split function
        let chapter_inputs: Vec<audiobook_chapters::ChapterInput> = chapters
            .iter()
            .map(|c| audiobook_chapters::ChapterInput {
                chapter_index: c.chapter_index,
                title: c.title.clone(),
                start_seconds: c.start_seconds,
                end_seconds: c.end_seconds,
                file_path: c.file_path.clone(),
                source: c.source.clone(),
            })
            .collect();

        // Create temp directory for MP3 files
        let temp_dir = PathBuf::from(&self.storage_root)
            .join("tmp")
            .join(format!("audiobook_convert_{}", job_id));

        nats_jobs::check_cancelled(&cancel)?;

        // Split into per-chapter MP3 files
        split_to_mp3_chapters(&source_path, &chapter_inputs, &temp_dir)
            .await
            .map_err(|e| JobError::ExecutionFailed(format!("Split failed: {e}")))?;

        // Create MP3Z output path
        let mp3z_dir = PathBuf::from(&self.storage_root)
            .join("books")
            .join(payload.book_id.to_string());
        tokio::fs::create_dir_all(&mp3z_dir)
            .await
            .map_err(|e| JobError::ExecutionFailed(format!("Failed to create dir: {e}")))?;
        let mp3z_path = mp3z_dir.join(format!("{}.mp3z", payload.book_id));

        nats_jobs::check_cancelled(&cancel)?;

        // Bundle into MP3Z
        create_mp3z(&temp_dir, &mp3z_path)
            .await
            .map_err(|e| JobError::ExecutionFailed(format!("MP3Z creation failed: {e}")))?;

        // Register the MP3Z as a book_file
        let mp3z_size = tokio::fs::metadata(&mp3z_path)
            .await
            .map(|m| m.len() as i64)
            .unwrap_or(0);

        book_files::create(
            &self.pool,
            book_files::BookFileInput {
                book_id: payload.book_id,
                format: "mp3z".to_string(),
                file_path: mp3z_path.to_string_lossy().to_string(),
                size_bytes: mp3z_size,
                source_path: None,
                source_mtime: None,
                file_hash: None,
                source_type: Some("conversion".to_string()),
                source_metadata: None,
                ingest_queue_id: None,
            },
        )
        .await
        .map_err(|e| JobError::ExecutionFailed(format!("Failed to create book_file: {e}")))?;

        // Clean up temp directory
        let _ = tokio::fs::remove_dir_all(&temp_dir).await;

        info!(
            book_id = %payload.book_id,
            mp3z_path = %mp3z_path.display(),
            "Audiobook converted to MP3Z"
        );

        Ok(json!({
            "book_id": payload.book_id,
            "mp3z_path": mp3z_path.to_string_lossy(),
            "mp3z_size": mp3z_size,
        }))
    }
}
