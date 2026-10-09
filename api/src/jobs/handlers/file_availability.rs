//! Handler for file availability check jobs.
//!
//! Checks if book files exist on disk and updates their availability status.

use crate::jobs::JobError;
use crate::jobs::worker::JobHandler;
use crate::repositories::{audiobook_files, book_files};
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sqlx::PgPool;
use tokio_util::sync::CancellationToken;
use tracing::{info, warn};
use uuid::Uuid;

/// Payload for file_availability_check jobs.
/// If `book_id` is provided, checks only files for that book.
/// If not provided, checks all files.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct FileAvailabilityPayload {
    /// Check files for a specific book.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub book_id: Option<Uuid>,
}

/// Handler for file_availability_check jobs.
pub struct FileAvailabilityHandler {
    pool: PgPool,
}

impl FileAvailabilityHandler {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}

#[async_trait]
impl JobHandler for FileAvailabilityHandler {
    fn job_type(&self) -> &'static str {
        "file_availability_check"
    }

    async fn execute(
        &self,
        job_id: Uuid,
        payload: Value,
        cancel: CancellationToken,
    ) -> Result<Value, JobError> {
        nats_jobs::check_cancelled(&cancel)?;
        let payload: FileAvailabilityPayload = serde_json::from_value(payload)?;

        info!(
            job_id = %job_id,
            book_id = ?payload.book_id,
            "Starting file availability check"
        );

        let result = check_file_availability(&self.pool, payload.book_id).await?;

        let checked = result.get("checked").and_then(|v| v.as_i64()).unwrap_or(0);
        let unavailable = result
            .get("unavailable")
            .and_then(|v| v.as_i64())
            .unwrap_or(0);

        if unavailable > 0 {
            warn!(
                job_id = %job_id,
                checked = checked,
                unavailable = unavailable,
                "File availability check found unavailable files"
            );
        } else {
            info!(
                job_id = %job_id,
                checked = checked,
                "File availability check completed, all files available"
            );
        }

        Ok(result)
    }
}

/// Check file availability for book files.
///
/// Verifies that files referenced in the database actually exist on disk,
/// and updates their availability status accordingly.
pub async fn check_file_availability(
    pool: &PgPool,
    book_id: Option<Uuid>,
) -> Result<Value, JobError> {
    let files = book_files::list_for_availability_check(pool, book_id).await?;

    let total = files.len();
    let mut checked = 0;
    let mut unavailable = 0;
    let mut restored = 0;

    for file in files {
        checked += 1;
        let exists = tokio::fs::try_exists(&file.file_path)
            .await
            .unwrap_or(false);

        if exists && !file.available {
            // File was unavailable but now exists again
            let _ = book_files::set_available(pool, file.id).await;
            restored += 1;
        } else if !exists && file.available {
            // File was available but no longer exists
            let _ = book_files::set_unavailable(pool, file.id).await;
            unavailable += 1;
        } else if exists {
            // File exists and is already marked available — just touch it
            let _ = book_files::touch(pool, file.id).await;
        }
    }

    // Second pass: check audiobook_files
    let audio_files = audiobook_files::list_for_availability_check(pool, book_id).await?;
    let audio_total = audio_files.len();

    for file in audio_files {
        checked += 1;
        let exists = tokio::fs::try_exists(&file.file_path)
            .await
            .unwrap_or(false);

        if exists && !file.available {
            let _ = audiobook_files::set_available(pool, file.id).await;
            restored += 1;
        } else if !exists && file.available {
            let _ = audiobook_files::set_unavailable(pool, file.id).await;
            unavailable += 1;
        }
    }

    let available = checked - unavailable;
    Ok(serde_json::json!({
        "checked": checked,
        "total": total + audio_total,
        "available": available,
        "unavailable": unavailable,
        "restored": restored,
    }))
}
