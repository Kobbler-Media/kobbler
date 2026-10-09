//! Media root book conversion job handler.
//!
//! Converts added book files to KEPUB format for Kobo device sync, writing the
//! KEPUB alongside its source inside the media root.
//!
//! Scoped to books a Kobo is actually entitled to. This runs off the tail of
//! the ingest chain (`media_root_scan` → … → `media_root_enrich`), which only
//! fires when a scan turns up new files — so it is the path that catches books
//! as they arrive, not a way to reach a library that has already settled.
//! `kepub_check` is the one that does that, on a schedule and regardless of
//! whether anything was ingested.

use crate::convert::{ConversionPipeline, EbookFormat};
use crate::jobs::JobResult;
use crate::jobs::worker::JobHandler;
use crate::repositories::{book_files, entitlements, media_root_files, media_roots};
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sqlx::PgPool;
use std::path::Path;
use tokio_util::sync::CancellationToken;
use tracing::{debug, info, warn};
use uuid::Uuid;

const BATCH_SIZE: i64 = 100;

/// Payload for media_root_book_convert jobs.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MediaRootBookConvertPayload {
    pub media_root_id: Uuid,
}

pub struct MediaRootBookConvertHandler {
    pool: PgPool,
}

impl MediaRootBookConvertHandler {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}

#[async_trait]
impl JobHandler for MediaRootBookConvertHandler {
    fn job_type(&self) -> &'static str {
        "media_root_book_convert"
    }

    fn max_concurrent(&self) -> usize {
        1
    }

    async fn execute(
        &self,
        _job_id: Uuid,
        payload: Value,
        cancel: CancellationToken,
    ) -> JobResult<Value> {
        let payload: MediaRootBookConvertPayload = serde_json::from_value(payload)?;

        let root = match media_roots::get_by_id(&self.pool, payload.media_root_id).await? {
            Some(r) => r,
            None => {
                tracing::warn!(
                    "Media root {} no longer exists, skipping",
                    payload.media_root_id
                );
                return Ok(json!({"skipped": true, "reason": "media_root_deleted"}));
            }
        };

        if !media_roots::check_available(&self.pool, &root).await? {
            return Ok(json!({"deferred": true, "reason": "media_root_unavailable"}));
        }

        if root.media_type != "book" {
            return Ok(json!({"skipped": true, "reason": "not a book media root"}));
        }

        let pipeline = ConversionPipeline::new();
        // Native pipeline (kob-ebook) is always available — no tool check needed.
        // Calibre is only required if the media root contains formats outside
        // EPUB/KEPUB/MOBI/AZW/AZW3 (PDF/FB2/RTF/TXT/HTML/DOCX/LIT/PDB).
        let ebook_convert_available = pipeline.calibre_available().await;

        let mut converted = 0u64;
        let mut skipped = 0u64;
        let mut errors = 0u64;
        let mut offset = 0i64;

        // Paged rather than one batch. `list_for_root` orders by file_path, so
        // a single `LIMIT 100 OFFSET 0` read the same alphabetical first
        // hundred on every run and could never reach the rest of a library —
        // and since converting a file does not change its match_status, there
        // was nothing to move the window along.
        loop {
            nats_jobs::check_cancelled(&cancel)?;
            let files = media_root_files::list_for_root(
                &self.pool,
                root.id,
                Some("added"),
                BATCH_SIZE,
                offset,
            )
            .await?;
            if files.is_empty() {
                break;
            }
            let batch_len = files.len() as i64;

            for file in &files {
                nats_jobs::check_cancelled(&cancel)?;
                let file_path = Path::new(&file.file_path);
                let format = EbookFormat::from_path(file_path);

                // Skip if already KEPUB
                if format.is_kepub() {
                    skipped += 1;
                    continue;
                }

                // Skip if this book already has a KEPUB file
                if let Some(entity_id) = file.entity_id {
                    match book_files::has_format(&self.pool, entity_id, &["KEPUB"]).await {
                        Ok(true) => {
                            debug!(book_id = %entity_id, "KEPUB already exists, skipping conversion");
                            skipped += 1;
                            continue;
                        }
                        Err(e) => {
                            warn!(error = %e, "Failed to check existing KEPUB");
                        }
                        _ => {}
                    }
                }

                // Skip books no Kobo is entitled to. A KEPUB is only ever read
                // by a Kobo, and it is written next to the source inside the
                // user's own library — so converting the whole shelf on the
                // chance that one of them is sent to a device would roughly
                // double the tree for no reader's benefit. `kepub_check` picks
                // a book up within minutes of it actually being synced.
                if let Some(entity_id) = file.entity_id {
                    match entitlements::book_reaches_kobo(&self.pool, entity_id).await {
                        Ok(false) => {
                            skipped += 1;
                            continue;
                        }
                        Err(e) => {
                            warn!(error = %e, "Failed to check Kobo entitlement");
                            skipped += 1;
                            continue;
                        }
                        _ => {}
                    }
                }

                // Skip non-convertible formats (PDF etc can't become good KEPUBs)
                if !matches!(
                    format,
                    EbookFormat::Epub | EbookFormat::Mobi | EbookFormat::Azw | EbookFormat::Azw3
                ) {
                    if !ebook_convert_available && !matches!(format, EbookFormat::Epub) {
                        skipped += 1;
                        continue;
                    }
                    // For non-EPUB formats that need ebook-convert first
                    if !ebook_convert_available {
                        debug!(file = %file.file_name, "Skipping conversion - ebook-convert not available");
                        skipped += 1;
                        continue;
                    }
                }

                // Convert to KEPUB in the same directory as the source
                let output_dir = match file_path.parent() {
                    Some(dir) => dir,
                    None => {
                        skipped += 1;
                        continue;
                    }
                };

                match pipeline.convert_to_kepub(file_path, output_dir).await {
                    Ok(kepub_path) => {
                        // Register the KEPUB as a book file
                        if let Some(entity_id) = file.entity_id {
                            let kepub_size = tokio::fs::metadata(&kepub_path)
                                .await
                                .map(|m| m.len() as i64)
                                .unwrap_or(0);

                            let input = book_files::BookFileInput {
                                book_id: entity_id,
                                format: "KEPUB".to_string(),
                                size_bytes: kepub_size,
                                file_path: kepub_path.to_string_lossy().to_string(),
                                source_type: Some("media_root".to_string()),
                                ..Default::default()
                            };

                            if let Err(e) = book_files::upsert(&self.pool, input).await {
                                warn!(error = %e, "Failed to register KEPUB file");
                                errors += 1;
                            } else {
                                converted += 1;
                            }
                        } else {
                            warn!(file_id = %file.id, "Matched file has no entity_id");
                            errors += 1;
                        }
                    }
                    Err(e) => {
                        warn!(file = %file.file_name, error = %e, "KEPUB conversion failed");
                        errors += 1;
                    }
                }
            }

            if batch_len < BATCH_SIZE {
                break;
            }
            offset += batch_len;
        }

        info!(
            root = %root.name,
            converted = converted,
            skipped = skipped,
            errors = errors,
            "Book KEPUB conversion complete"
        );

        Ok(json!({
            "converted": converted,
            "skipped": skipped,
            "errors": errors,
        }))
    }
}
