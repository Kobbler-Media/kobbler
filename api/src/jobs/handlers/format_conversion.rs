//! Format conversion job handler.
//!
//! Converts book files between formats (e.g., EPUB → KEPUB) based on device type needs.

use crate::convert::ConversionPipeline;
use crate::jobs::worker::JobHandler;
use crate::jobs::{JobError, JobResult};
use crate::repositories::book_files;
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sqlx::PgPool;
use std::path::PathBuf;
use tokio_util::sync::CancellationToken;
use tracing::{error, info, warn};
use uuid::Uuid;

/// Payload for format_conversion jobs.
#[derive(Debug, Deserialize, Serialize)]
pub struct FormatConversionPayload {
    pub book_id: Uuid,
    /// Target format (e.g., "KEPUB", "EPUB").
    pub target_format: String,
    /// Optional follow-up job to enqueue after conversion completes.
    pub follow_up_job: Option<FollowUpJob>,
}

/// A job to enqueue after conversion completes.
#[derive(Debug, Deserialize, Serialize)]
pub struct FollowUpJob {
    pub job_type: String,
    pub payload: Value,
}

pub struct FormatConversionHandler {
    pool: PgPool,
    storage_root: String,
}

impl FormatConversionHandler {
    pub fn new(pool: PgPool, storage_root: String) -> Self {
        Self { pool, storage_root }
    }
}

#[async_trait]
impl JobHandler for FormatConversionHandler {
    fn job_type(&self) -> &'static str {
        "format_conversion"
    }

    async fn execute(
        &self,
        job_id: Uuid,
        payload: Value,
        cancel: CancellationToken,
    ) -> JobResult<Value> {
        let payload: FormatConversionPayload = serde_json::from_value(payload)
            .map_err(|e| JobError::ExecutionFailed(format!("Invalid payload: {e}")))?;

        nats_jobs::check_cancelled(&cancel)?;

        info!(
            job_id = %job_id,
            book_id = %payload.book_id,
            target_format = %payload.target_format,
            "Starting format conversion"
        );

        // Check if target format already exists. A row can outlive its file:
        // if the file is gone, the row is a ghost and the conversion is the
        // way back, not a duplicate.
        let existing =
            book_files::get_path_by_format(&self.pool, payload.book_id, &payload.target_format)
                .await
                .map_err(|e| JobError::ExecutionFailed(format!("DB error: {e}")))?;
        if let Some(p) = &existing
            && std::path::Path::new(p).exists()
        {
            info!(
                book_id = %payload.book_id,
                format = %payload.target_format,
                "Target format already exists, skipping conversion"
            );
            return Ok(json!({ "skipped": true, "reason": "format already exists" }));
        }

        // Find source file — prefer EPUB, then any available format
        let source_path = book_files::get_path_by_format(&self.pool, payload.book_id, "EPUB")
            .await
            .map_err(|e| JobError::ExecutionFailed(format!("DB error: {e}")))?;

        let source_path = match source_path {
            Some(p) => p,
            None => {
                // Try to find any available file
                let any_path = book_files::get_any_available_path(&self.pool, payload.book_id)
                    .await
                    .map_err(|e| JobError::ExecutionFailed(format!("DB error: {e}")))?;
                match any_path {
                    Some(p) => p,
                    None => {
                        return Err(JobError::ExecutionFailed(format!(
                            "No source file found for book {}",
                            payload.book_id
                        )));
                    }
                }
            }
        };

        let source = std::path::Path::new(&source_path);
        if !source.exists() {
            return Err(JobError::ExecutionFailed(format!(
                "Source file not found: {source_path}"
            )));
        }

        let output_dir = PathBuf::from(&self.storage_root)
            .join("books")
            .join(payload.book_id.to_string());

        tokio::fs::create_dir_all(&output_dir)
            .await
            .map_err(|e| JobError::ExecutionFailed(format!("Failed to create output dir: {e}")))?;

        nats_jobs::check_cancelled(&cancel)?;

        let pipeline = ConversionPipeline::new();
        let converted_path = match payload.target_format.to_uppercase().as_str() {
            "KEPUB" => pipeline
                .convert_to_kepub(source, &output_dir)
                .await
                .map_err(|e| JobError::ExecutionFailed(format!("Conversion failed: {e}")))?,
            "MOBI" => pipeline
                .convert_to_mobi(source, &output_dir)
                .await
                .map_err(|e| JobError::ExecutionFailed(format!("Conversion failed: {e}")))?,
            _ => {
                return Err(JobError::ExecutionFailed(format!(
                    "Unsupported target format: {}",
                    payload.target_format
                )));
            }
        };

        // Get file size
        let file_size = tokio::fs::metadata(&converted_path)
            .await
            .map(|m| m.len() as i64)
            .unwrap_or(0);

        // A book gaining a KEPUB is a book the device will fetch again, and it
        // resumes from a document plus a koboSpan id. Check the new file still
        // answers to the position each device saved, so a shift is visible here
        // rather than discovered by whoever was reading it.
        let positions_kept = if payload.target_format.eq_ignore_ascii_case("KEPUB") {
            self.check_saved_positions(payload.book_id, &converted_path)
                .await
        } else {
            Value::Null
        };

        // The staleness baseline for the repair sweep: what the source looked
        // like when this copy was made.
        let source_mtime = tokio::fs::metadata(source)
            .await
            .ok()
            .and_then(|m| m.modified().ok())
            .map(crate::repositories::media_root_files::mtime_from_system_time);
        // Create book file record
        let file_input = book_files::BookFileFullInput {
            book_id: payload.book_id,
            format: payload.target_format.clone(),
            size_bytes: file_size,
            file_path: converted_path.to_string_lossy().to_string(),
            source_path: Some(source_path),
            source_mtime,
            file_hash: None,
            source_type: Some("format_conversion".to_string()),
            source_metadata: Some(json!({ "source_job_id": job_id.to_string() })),
        };

        book_files::upsert_full(&self.pool, file_input)
            .await
            .map_err(|e| {
                error!(book_id = %payload.book_id, error = %e, "Failed to create book file record");
                JobError::ExecutionFailed(format!("Failed to save file record: {e}"))
            })?;

        info!(
            book_id = %payload.book_id,
            format = %payload.target_format,
            "Format conversion complete"
        );

        Ok(json!({
            "book_id": payload.book_id,
            "format": payload.target_format,
            "path": converted_path.to_string_lossy(),
            "size_bytes": file_size,
            "reading_positions": positions_kept,
        }))
    }
}

impl FormatConversionHandler {
    /// Report, per device, whether its saved place still resolves in the file
    /// just built. Never fails the conversion: the book is better off converted
    /// either way, and a device that has lost its anchor lands at the start of
    /// the chapter rather than nowhere.
    async fn check_saved_positions(
        &self,
        book_id: Uuid,
        converted_path: &std::path::Path,
    ) -> Value {
        let positions = crate::kobo::position::saved_positions(&self.pool, book_id).await;
        if positions.is_empty() {
            return Value::Null;
        }

        let Ok(bytes) = tokio::fs::read(converted_path).await else {
            return Value::Null;
        };

        let checked: Vec<Value> = positions
            .into_iter()
            .map(|p| {
                let kept = crate::kobo::position::position_survives(&bytes, &p);
                if !kept {
                    warn!(
                        book_id = %book_id,
                        device = %p.device_name,
                        source = %p.source,
                        span = %p.span,
                        "saved reading position does not resolve in the new KEPUB"
                    );
                }
                json!({ "device": p.device_name, "source": p.source, "kept": kept })
            })
            .collect();

        json!(checked)
    }
}
