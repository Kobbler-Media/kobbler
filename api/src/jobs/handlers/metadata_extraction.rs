//! Handler for bulk metadata extraction jobs.
//!
//! Extracts metadata + covers from ebook files in the ingest queue.
//! Provides progress tracking for the UI to show "30/282 complete, 5 errors".

use crate::convert::EbookMeta;
use crate::jobs::worker::JobHandler;
use crate::jobs::{JobError, JobEvent, update_progress};
use crate::repositories::{authors, ingest_queue};
use crate::runtime_config::RuntimeConfigManager;
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sqlx::PgPool;
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use time::OffsetDateTime;
use tokio::sync::{Semaphore, broadcast};
use tokio_util::sync::CancellationToken;
use tracing::{debug, error, info, warn};
use uuid::Uuid;

/// Payload for metadata_extraction jobs.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct MetadataExtractionPayload {}

/// Maximum concurrent extractions.
const MAX_CONCURRENT_EXTRACTIONS: usize = 4;

/// Handler for metadata_extraction jobs.
pub struct MetadataExtractionHandler {
    pool: PgPool,
    job_events: broadcast::Sender<JobEvent>,
    ingest_queue_events: broadcast::Sender<crate::IngestQueueEvent>,
    runtime_config: RuntimeConfigManager,
}

impl MetadataExtractionHandler {
    pub fn new(
        pool: PgPool,
        job_events: broadcast::Sender<JobEvent>,
        ingest_queue_events: broadcast::Sender<crate::IngestQueueEvent>,
        runtime_config: RuntimeConfigManager,
    ) -> Self {
        Self {
            pool,
            job_events,
            ingest_queue_events,
            runtime_config,
        }
    }
}

#[async_trait]
impl JobHandler for MetadataExtractionHandler {
    fn job_type(&self) -> &'static str {
        "metadata_extraction"
    }

    /// CPU-heavy task, scale with available cores (min 1, max 8).
    fn max_concurrent(&self) -> usize {
        std::thread::available_parallelism()
            .map(|p| p.get().min(8))
            .unwrap_or(2)
    }

    async fn execute(
        &self,
        job_id: Uuid,
        payload: Value,
        cancel: CancellationToken,
    ) -> Result<Value, JobError> {
        let _payload: MetadataExtractionPayload = serde_json::from_value(payload)?;
        nats_jobs::check_cancelled(&cancel)?;

        info!(
            job_id = %job_id,
            "Starting metadata extraction job"
        );

        // Get ebook-meta path from config
        let config = self.runtime_config.get().await;
        let ebook_meta_path = config.ebook_meta_path.clone();

        // Find all items needing extraction
        let items = ingest_queue::list_extracting(&self.pool).await?;

        let total = items.len();
        if total == 0 {
            info!(job_id = %job_id, "No items to extract");
            return Ok(json!({
                "status": "completed",
                "total": 0,
                "processed": 0,
                "errors": 0
            }));
        }

        info!(job_id = %job_id, total = total, "Found items to extract");

        // Track progress
        let processed = Arc::new(AtomicUsize::new(0));
        let errors = Arc::new(AtomicUsize::new(0));
        let semaphore = Arc::new(Semaphore::new(MAX_CONCURRENT_EXTRACTIONS));

        // Initial progress update
        let _ = update_progress(
            &self.pool,
            &self.job_events,
            job_id,
            "metadata_extraction",
            0,
            Some(json!({
                "total": total,
                "processed": 0,
                "errors": 0
            })),
        )
        .await;

        // Throttle SSE progress: ~20 events max (same pattern as bulk_resolve)
        let progress_interval = (total / 20).clamp(1, 10);

        // Process items concurrently with progress updates
        let mut handles = Vec::with_capacity(total);

        for item in items {
            let queue_id = item.id;
            let file_path = item.file_path;
            let file_name = item.file_name;
            let pool = self.pool.clone();
            let job_events = self.job_events.clone();
            let ingest_queue_events = self.ingest_queue_events.clone();
            let ebook_meta_path = ebook_meta_path.clone();
            let semaphore = semaphore.clone();
            let processed = processed.clone();
            let errors = errors.clone();

            let handle = tokio::spawn(async move {
                let _permit = semaphore.acquire().await.expect("semaphore closed");

                let success =
                    extract_single_item(&pool, queue_id, &file_path, &file_name, ebook_meta_path)
                        .await;

                // Update counters
                let new_processed = processed.fetch_add(1, Ordering::Relaxed) + 1;
                if !success {
                    errors.fetch_add(1, Ordering::Relaxed);
                }

                // Only broadcast + persist at intervals or completion
                if new_processed.is_multiple_of(progress_interval) || new_processed == total {
                    let progress = ((new_processed as f64 / total as f64) * 100.0) as i32;
                    let current_errors = errors.load(Ordering::Relaxed);
                    let result = json!({
                        "total": total,
                        "processed": new_processed,
                        "errors": current_errors
                    });
                    let _ = update_progress(
                        &pool,
                        &job_events,
                        job_id,
                        "metadata_extraction",
                        progress,
                        Some(result),
                    )
                    .await;
                }

                // Notify ingest queue listeners that this item changed status
                let _ = ingest_queue_events.send(crate::IngestQueueEvent {
                    item_id: queue_id,
                    status: "pending".to_string(),
                });

                success
            });

            handles.push(handle);
        }

        nats_jobs::check_cancelled(&cancel)?;

        // Wait for all extractions to complete
        for handle in handles {
            let _ = handle.await;
        }

        let final_processed = processed.load(Ordering::Relaxed);
        let final_errors = errors.load(Ordering::Relaxed);

        info!(
            job_id = %job_id,
            total = total,
            processed = final_processed,
            errors = final_errors,
            "Metadata extraction job completed"
        );

        Ok(json!({
            "status": "completed",
            "total": total,
            "processed": final_processed,
            "errors": final_errors
        }))
    }
}

/// Extract metadata and cover for a single queue item.
/// Returns true if extraction succeeded (or partially succeeded), false on complete failure.
async fn extract_single_item(
    pool: &PgPool,
    queue_id: Uuid,
    file_path: &str,
    file_name: &str,
    ebook_meta_path: Option<String>,
) -> bool {
    let ebook_meta = EbookMeta::with_config_path(ebook_meta_path);
    let path = Path::new(file_path);

    // Native pipeline (kob-ebook) handles EPUB/MOBI/AZW/AZW3/KFX without
    // Calibre. Only gate on Calibre when the format actually needs it.
    if !kob_ebook::native_supports(path) && !ebook_meta.is_available().await {
        warn!(
            queue_id = %queue_id,
            "[{}] ebook-meta not available and native pipeline can't read this format",
            file_name
        );
        let _ = ingest_queue::update_status(pool, queue_id, "pending").await;
        return false;
    }

    // Extract metadata
    debug!(queue_id = %queue_id, "[{}] Extracting metadata", file_name);
    let metadata_result = ebook_meta.extract(path).await;

    // Extract cover (try even if metadata failed)
    let temp_dir = std::env::temp_dir().join(format!("kobbler-cover-{}", queue_id));
    let cover_bytes: Option<Vec<u8>> = match ebook_meta.extract_cover(path, &temp_dir).await {
        Ok(cover_path) => {
            let bytes = tokio::fs::read(&cover_path).await.ok();
            let _ = tokio::fs::remove_file(&cover_path).await;
            let _ = tokio::fs::remove_dir_all(&temp_dir).await;
            if bytes.is_some() {
                debug!(queue_id = %queue_id, "[{}] Cover extracted", file_name);
            }
            bytes
        }
        Err(_) => {
            let _ = tokio::fs::remove_dir_all(&temp_dir).await;
            None
        }
    };

    // Build update query with all extracted data
    match metadata_result {
        Ok(metadata) => {
            let authors_extracted: Option<Vec<String>> = if metadata.authors.is_empty() {
                None
            } else {
                Some(metadata.authors)
            };

            // Parse pubdate string to OffsetDateTime
            let pubdate: Option<OffsetDateTime> = metadata.pubdate.as_ref().and_then(|s| {
                time::OffsetDateTime::parse(s, &time::format_description::well_known::Rfc3339)
                    .ok()
                    .or_else(|| {
                        // Try parsing just a date (YYYY-MM-DD)
                        time::Date::parse(
                            s,
                            &time::format_description::parse_borrowed::<2>("[year]-[month]-[day]")
                                .ok()?,
                        )
                        .ok()
                        .map(|d| d.with_hms(0, 0, 0).unwrap().assume_utc())
                    })
            });

            debug!(
                queue_id = %queue_id,
                "[{}] Extracted: title={:?}, authors={:?}",
                file_name,
                metadata.title.as_deref().unwrap_or("(none)"),
                authors_extracted
                    .as_ref()
                    .map(|a| a.join(", "))
                    .unwrap_or_else(|| "(none)".to_string())
            );

            // Only set pending_authors from extraction if the book has no linked authors.
            // Provider-matched authors (from Hardcover/OpenLibrary) are authoritative;
            // calibre extraction is a fallback for books without provider metadata.
            let has_linked_authors = !authors::get_book_author_ids(pool, queue_id)
                .await
                .unwrap_or_default()
                .is_empty();

            let pending_authors_json = if has_linked_authors {
                debug!(
                    queue_id = %queue_id,
                    "[{}] Skipping extracted authors — book already has linked authors",
                    file_name
                );
                None
            } else {
                authors_extracted.as_ref().map(|names| {
                    serde_json::json!(
                        names
                            .iter()
                            .map(|name| serde_json::json!({"name": name}))
                            .collect::<Vec<_>>()
                    )
                })
            };

            // Build pending_series JSONB from extracted series name
            let pending_series_json = metadata
                .series
                .as_ref()
                .map(|name| serde_json::json!({"name": name}));

            // Update with all metadata fields + cover, then transition to pending
            let update = ingest_queue::IngestQueueUpdate {
                title: metadata.title,
                description: metadata.description,
                language: metadata.language,
                publisher: metadata.publisher,
                pubdate,
                cover: cover_bytes,
                pending_authors: pending_authors_json,
                pending_series: pending_series_json,
                pending_series_index: metadata.series_index,
                status: Some("pending".to_string()),
                ..Default::default()
            };

            // Also set ISBN via book identifiers if extracted
            if let Some(ref isbn) = metadata.isbn {
                let _ = crate::repositories::books::update_identifiers(
                    pool,
                    queue_id,
                    serde_json::json!({"isbn": isbn}),
                )
                .await;
            }

            if let Err(e) = ingest_queue::update(pool, queue_id, update).await {
                error!(queue_id = %queue_id, "[{}] Failed to update queue item: {}", file_name, e);
                return false;
            }
            true
        }
        Err(e) => {
            warn!(
                queue_id = %queue_id,
                "[{}] Metadata extraction failed: {}", file_name, e
            );
            // Extraction failed, but still transition to pending so user can see it
            if let Err(e) =
                ingest_queue::update_cover_and_set_pending(pool, queue_id, cover_bytes.as_deref())
                    .await
            {
                error!(queue_id = %queue_id, "[{}] Failed to update queue item: {}", file_name, e);
                return false;
            }
            // We got the cover but not metadata - partial success still counts as error for UI
            false
        }
    }
}
