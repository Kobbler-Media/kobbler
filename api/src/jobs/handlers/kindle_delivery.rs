//! Kindle delivery job handler.
//!
//! Sends a book file to a Kindle device via native stk-client.
//! Prefers MOBI format, triggers conversion from EPUB/other formats if needed.

use crate::convert::ConversionPipeline;
use crate::jobs::worker::JobHandler;
use crate::jobs::{JobError, JobResult};
use crate::repositories::{book_files, books, kindle};
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sqlx::PgPool;
use std::path::{Path, PathBuf};
use stk_client::Client as StkClient;
use tokio_util::sync::CancellationToken;
use tracing::{error, info, warn};
use uuid::Uuid;

/// Payload for kindle_delivery jobs.
#[derive(Debug, Deserialize, Serialize)]
pub struct KindleDeliveryPayload {
    pub book_id: Uuid,
    pub target_id: Uuid,
    /// If true, re-send even if already delivered.
    #[serde(default)]
    pub force: bool,
}

pub struct KindleDeliveryHandler {
    pool: PgPool,
    storage_root: String,
}

impl KindleDeliveryHandler {
    pub fn new(pool: PgPool, storage_root: String) -> Self {
        Self { pool, storage_root }
    }
}

#[async_trait]
impl JobHandler for KindleDeliveryHandler {
    fn job_type(&self) -> &'static str {
        "kindle_delivery"
    }

    async fn execute(
        &self,
        job_id: Uuid,
        payload: Value,
        cancel: CancellationToken,
    ) -> JobResult<Value> {
        let payload: KindleDeliveryPayload = serde_json::from_value(payload)
            .map_err(|e| JobError::ExecutionFailed(format!("Invalid payload: {e}")))?;

        nats_jobs::check_cancelled(&cancel)?;

        info!(
            job_id = %job_id,
            book_id = %payload.book_id,
            target_id = %payload.target_id,
            "Starting Kindle delivery"
        );

        // Check if already delivered (skip unless forced)
        if !payload.force {
            let already = kindle::is_delivered(&self.pool, payload.target_id, payload.book_id)
                .await
                .map_err(|e| JobError::ExecutionFailed(format!("DB error: {e}")))?;
            if already {
                info!(book_id = %payload.book_id, target_id = %payload.target_id, "Already delivered, skipping");
                return Ok(json!({ "skipped": true, "reason": "already delivered" }));
            }
        }

        // Get the target device
        let target = kindle::get_target(&self.pool, payload.target_id)
            .await
            .map_err(|e| JobError::ExecutionFailed(format!("DB error: {e}")))?
            .ok_or_else(|| {
                JobError::ExecutionFailed(format!("Target {} not found", payload.target_id))
            })?;

        if !target.enabled {
            return Err(JobError::ExecutionFailed(format!(
                "Target {} is disabled",
                target
                    .device_name
                    .as_deref()
                    .unwrap_or(&target.device_serial)
            )));
        }

        // Get account credentials
        let credentials_json = kindle::get_credentials(&self.pool, target.kindle_account_id)
            .await
            .map_err(|e| JobError::ExecutionFailed(format!("DB error: {e}")))?
            .ok_or_else(|| {
                JobError::ExecutionFailed(format!("Account {} not found", target.kindle_account_id))
            })?;

        // Load the STK client from credentials
        let client = StkClient::loads(&credentials_json)
            .map_err(|e| JobError::ExecutionFailed(format!("Invalid credentials: {e}")))?;

        // Find a suitable book file (prefer MOBI > AZW3 > AZW > EPUB > KEPUB)
        let (file_id, file_path, fmt) = book_files::get_kindle_file(&self.pool, payload.book_id)
            .await
            .map_err(|e| JobError::ExecutionFailed(format!("DB error: {e}")))?
            .ok_or_else(|| {
                JobError::ExecutionFailed(format!(
                    "No compatible file found for book {}",
                    payload.book_id
                ))
            })?;

        // If file is EPUB or other non-MOBI format, convert to MOBI first
        let (final_path, final_fmt): (PathBuf, String) = if fmt != "MOBI"
            && fmt != "AZW3"
            && fmt != "AZW"
        {
            info!(book_id = %payload.book_id, source_format = %fmt, "Converting to MOBI for Kindle");

            let output_dir = PathBuf::from(&self.storage_root)
                .join("books")
                .join(payload.book_id.to_string());

            tokio::fs::create_dir_all(&output_dir).await.map_err(|e| {
                JobError::ExecutionFailed(format!("Failed to create output dir: {e}"))
            })?;

            let pipeline = ConversionPipeline::new();
            let source = Path::new(&file_path);

            match pipeline.convert_to_mobi(source, &output_dir).await {
                Ok(mobi_path) => {
                    // Get file size and save to DB
                    let file_size = tokio::fs::metadata(&mobi_path)
                        .await
                        .map(|m| m.len() as i64)
                        .unwrap_or(0);

                    let file_input = book_files::BookFileFullInput {
                        book_id: payload.book_id,
                        format: "MOBI".to_string(),
                        size_bytes: file_size,
                        file_path: mobi_path.to_string_lossy().to_string(),
                        source_path: Some(file_path.clone()),
                        source_mtime: None,
                        file_hash: None,
                        source_type: Some("kindle_conversion".to_string()),
                        source_metadata: Some(json!({ "source_format": fmt })),
                    };

                    if let Err(e) = book_files::upsert_full(&self.pool, file_input).await {
                        warn!(error = %e, "Failed to save MOBI file record (continuing with send)");
                    }

                    (mobi_path, "MOBI".to_string())
                }
                Err(e) => {
                    warn!(error = %e, "MOBI conversion failed, sending original format");
                    (PathBuf::from(&file_path), fmt)
                }
            }
        } else {
            (PathBuf::from(&file_path), fmt)
        };

        // Get book title and author
        let summary = books::get_summary(&self.pool, payload.book_id)
            .await
            .map_err(|e| JobError::ExecutionFailed(format!("DB error: {e}")))?;
        let (title, author) = match summary {
            Some(s) => {
                let authors = if s.authors.is_empty() {
                    "Unknown".to_string()
                } else {
                    s.authors
                };
                (s.title, authors)
            }
            None => ("Unknown".to_string(), "Unknown".to_string()),
        };

        info!(
            book_id = %payload.book_id,
            serial = %target.device_serial,
            format = %final_fmt,
            title = %title,
            "Sending to Kindle via stk-client"
        );

        // Send the file using the native client
        let result = client
            .send_file(
                &final_path,
                &[target.device_serial.as_str()],
                &author,
                &title,
                &final_fmt,
            )
            .await;

        match result {
            Ok(sku) => {
                info!(book_id = %payload.book_id, sku = %sku, "File sent successfully");
            }
            Err(e) => {
                error!(book_id = %payload.book_id, error = %e, "Failed to send to Kindle");
                return Err(JobError::retryable(format!(
                    "Failed to send to Kindle: {e}"
                )));
            }
        }

        // Record delivery
        if let Err(e) =
            kindle::create_delivery(&self.pool, payload.target_id, payload.book_id, file_id).await
        {
            // Log but don't fail - the send succeeded
            error!(error = %e, "Failed to record delivery (send succeeded)");
        }

        info!(
            book_id = %payload.book_id,
            target_id = %payload.target_id,
            "Kindle delivery complete"
        );

        // Record activity (no AppState, call repository directly)
        crate::routes::library::helpers::record_activity(
            &self.pool,
            "delivered",
            Some("book"),
            Some(payload.book_id),
            &title,
            Some("Sent to Kindle"),
            Some(&format!("/api/v1/library/books/{}/cover", payload.book_id)),
            Some(&format!("/library/books/{}", payload.book_id)),
            kindle::account_owner(&self.pool, target.kindle_account_id)
                .await
                .ok()
                .flatten(),
        )
        .await;

        Ok(json!({
            "book_id": payload.book_id,
            "target_id": payload.target_id,
            "device_serial": target.device_serial,
            "title": title,
        }))
    }
}
