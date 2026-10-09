//! Cross-format sync job handler.
//!
//! Aligns audiobook transcripts with ebook text to produce sync points
//! that map audio timestamps to ebook CFI positions. Requires both
//! transcript and ebook segments to exist for the book.

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sqlx::PgPool;
use tokio_util::sync::CancellationToken;
use tracing::{info, warn};
use uuid::Uuid;

use crate::jobs::JobResult;
use crate::jobs::worker::JobHandler;
use crate::repositories::{audiobook_transcripts, cross_format_sync, ebook_segments};
use crate::sync::align::{self, AudioSegment, EbookTextSegment};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CrossFormatSyncPayload {
    pub book_id: Uuid,
}

pub struct CrossFormatSyncHandler {
    pool: PgPool,
}

impl CrossFormatSyncHandler {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}

#[async_trait]
impl JobHandler for CrossFormatSyncHandler {
    fn job_type(&self) -> &'static str {
        "cross_format_sync"
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
        let payload: CrossFormatSyncPayload = serde_json::from_value(payload)?;
        nats_jobs::check_cancelled(&cancel)?;
        let book_id = payload.book_id;

        // Check both sources exist
        let has_transcript = audiobook_transcripts::has_transcript(&self.pool, book_id).await?;
        let has_ebook = ebook_segments::has_segments(&self.pool, book_id).await?;

        if !has_transcript {
            warn!(
                "No audiobook transcript found for book {}, skipping sync",
                book_id
            );
            return Ok(json!({"status": "no_transcript"}));
        }
        if !has_ebook {
            warn!(
                "No ebook segments found for book {}, skipping sync",
                book_id
            );
            return Ok(json!({"status": "no_ebook_segments"}));
        }

        // Load both segment sets
        let transcript_segments = audiobook_transcripts::get_for_book(&self.pool, book_id).await?;
        let ebook_segs = ebook_segments::get_for_book(&self.pool, book_id).await?;

        info!(
            "Aligning {} audio segments with {} ebook segments for book {}",
            transcript_segments.len(),
            ebook_segs.len(),
            book_id
        );

        // Convert to alignment input types
        let audio_input: Vec<AudioSegment> = transcript_segments
            .iter()
            .map(|s| AudioSegment {
                file_id: s.audiobook_file_id,
                start_seconds: s.start_seconds,
                text: s.text.clone(),
            })
            .collect();

        let ebook_input: Vec<EbookTextSegment> = ebook_segs
            .iter()
            .map(|s| EbookTextSegment {
                file_id: s.book_file_id,
                cfi: s.cfi.clone(),
                text: s.text.clone(),
            })
            .collect();

        // Run alignment in blocking thread (CPU-bound)
        let sync_points =
            tokio::task::spawn_blocking(move || align::align_texts(&audio_input, &ebook_input, 3))
                .await
                .map_err(|e| crate::jobs::JobError::ExecutionFailed(format!("Join error: {e}")))?;

        info!(
            "Found {} sync points for book {}",
            sync_points.len(),
            book_id
        );

        // Delete old sync points and insert new ones
        cross_format_sync::delete_for_book(&self.pool, book_id).await?;

        let count = sync_points.len();
        if !sync_points.is_empty() {
            let inputs: Vec<cross_format_sync::SyncPointInput> = sync_points
                .into_iter()
                .map(|sp| cross_format_sync::SyncPointInput {
                    audio_file_id: sp.audio_file_id,
                    audio_seconds: sp.audio_seconds,
                    ebook_file_id: sp.ebook_file_id,
                    ebook_cfi: sp.ebook_cfi,
                    confidence: sp.confidence,
                })
                .collect();

            cross_format_sync::insert_sync_points(&self.pool, book_id, &inputs).await?;
        }

        Ok(json!({
            "sync_points": count,
        }))
    }
}
