//! Audiobook transcription job handler.
//!
//! Transcribes all audio files for a book via the configured STT service, storing
//! timestamped text segments in the `audiobook_transcripts` table.

use crate::RuntimeConfigManager;
use crate::audio::stt;
use crate::enrichment_config::stt_config_tag;
use crate::gpu::GpuLock;
use crate::jobs::JobResult;
use crate::jobs::worker::JobHandler;
use crate::repositories::audiobook_transcripts::{TranscriptSegmentInput, TranscriptState};
use crate::repositories::{audiobook_files, audiobook_transcripts, ebook_segments};
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

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AudiobookTranscribePayload {
    pub book_id: Uuid,
    /// User-triggered: re-transcribe even files already carrying the current
    /// config tag. Background runs leave it false and skip those files.
    #[serde(default)]
    pub manual: bool,
    /// Also redo files whose transcript is from an older STT config. Routine
    /// runs only fill in files with no transcript: regenerating the library is
    /// the user's call (see `transcribe_refresh`).
    #[serde(default)]
    pub outdated: bool,
}

pub struct AudiobookTranscribeHandler {
    pool: PgPool,
    storage_root: String,
    runtime_config: RuntimeConfigManager,
    nats_publisher: Publisher,
    search: Arc<dyn SearchBackend>,
    gpu_lock: GpuLock,
}

impl AudiobookTranscribeHandler {
    pub fn new(
        pool: PgPool,
        storage_root: String,
        runtime_config: RuntimeConfigManager,
        nats_publisher: Publisher,
        search: Arc<dyn SearchBackend>,
        gpu_lock: GpuLock,
    ) -> Self {
        Self {
            pool,
            storage_root,
            runtime_config,
            nats_publisher,
            search,
            gpu_lock,
        }
    }
}

#[async_trait]
impl JobHandler for AudiobookTranscribeHandler {
    fn job_type(&self) -> &'static str {
        "audiobook_transcribe"
    }

    fn max_concurrent(&self) -> usize {
        self.runtime_config
            .job_concurrency("audiobook_transcribe", 1)
    }

    /// Allow a few redeliveries so a transient STT outage (crash/restart
    /// window) self-heals instead of permanently failing the job. Only
    /// errors classified retryable (see execute) consume these attempts;
    /// genuinely undecodable files still fail on the first try.
    fn max_deliver(&self) -> Option<i64> {
        Some(5)
    }

    async fn execute(
        &self,
        _job_id: Uuid,
        payload: Value,
        cancel: CancellationToken,
    ) -> JobResult<Value> {
        let payload: AudiobookTranscribePayload = serde_json::from_value(payload)?;
        let book_id = payload.book_id;

        let files = audiobook_files::list_by_book(&self.pool, book_id).await?;
        if files.is_empty() {
            return Ok(json!({"status": "no_files"}));
        }

        let rt_config = self.runtime_config.get().await;
        // Local GpuLock is a no-op when koblem owns the authoritative lock.
        self.gpu_lock
            .set_enabled(rt_config.gpu_preempt_tts && rt_config.use_local_gpu_lock());
        self.gpu_lock.set_service_urls(
            rt_config.stt_service_url.clone(),
            rt_config.tts_service_url.clone(),
            rt_config.vision_service_url.clone(),
        );

        // effective_stt_url redirects to koblem when route_gpu_via_koblem=true.
        let effective_url = rt_config.effective_stt_url();
        let stt_service_url = match effective_url.as_deref().filter(|u| !u.is_empty()) {
            Some(url) => url.to_string(),
            None => {
                return Err(crate::jobs::JobError::ExecutionFailed(
                    "STT service not configured (stt_service_url is empty)".to_string(),
                ));
            }
        };

        let config_tag = stt_config_tag(&rt_config);

        let total = files.len();
        let mut transcribed = 0u64;
        let mut up_to_date = 0u64;
        let mut outdated_kept = 0u64;
        let mut segments_total = 0u64;
        // Set when a file fails with a transient error (STT down/restarting).
        // Used to decide retryable vs. permanent failure below.
        let mut had_transient_failure = false;

        for (i, file) in files.iter().enumerate() {
            nats_jobs::check_cancelled(&cancel)?;

            if !payload.manual {
                match audiobook_transcripts::transcript_state(&self.pool, file.id, &config_tag)
                    .await
                {
                    Ok(TranscriptState::Current) => {
                        up_to_date += 1;
                        continue;
                    }
                    Ok(TranscriptState::Outdated) if !payload.outdated => {
                        outdated_kept += 1;
                        continue;
                    }
                    _ => {}
                }
            }

            let file_path =
                crate::audio::path::resolve_file_path(&self.storage_root, &file.file_path);

            if !file_path.is_file() {
                warn!(
                    file_id = %file.id,
                    path = %file_path.display(),
                    "Audio file not found on disk, skipping transcription"
                );
                continue;
            }

            // Yield to TTS if it has requested GPU preemption
            if self.gpu_lock.is_tts_requested() {
                info!("TTS requested GPU, yielding");
                self.gpu_lock.yield_to_tts().await;
            }

            info!(
                file_id = %file.id,
                progress = format!("{}/{}", i + 1, total),
                "Transcribing audiobook file"
            );

            // Acquire GPU — lock handles TTS eviction on first acquire
            let _gpu_guard = match self.gpu_lock.acquire_for_stt().await {
                Some(guard) => guard,
                None => {
                    // TTS preempted between the check and acquire — yield and retry
                    self.gpu_lock.yield_to_tts().await;
                    match self.gpu_lock.acquire_for_stt().await {
                        Some(guard) => guard,
                        None => {
                            warn!("STT acquire failed after TTS release, skipping file");
                            continue;
                        }
                    }
                }
            };
            match stt::transcribe_audio(&stt_service_url, &file_path).await {
                Ok(result) => {
                    let segments: Vec<TranscriptSegmentInput> = result
                        .segments
                        .into_iter()
                        .map(|s| TranscriptSegmentInput {
                            segment_index: s.index,
                            start_seconds: s.start_seconds,
                            end_seconds: s.end_seconds,
                            text: s.text,
                            avg_logprob: s.avg_logprob,
                            no_speech_prob: s.no_speech_prob,
                            compression_ratio: s.compression_ratio,
                        })
                        .collect();

                    let count = segments.len() as u64;
                    let mut tx = self.pool.begin().await?;
                    if let Err(e) = audiobook_transcripts::delete_for_file(&mut *tx, file.id).await
                    {
                        warn!(error = %e, file_id = %file.id, "Failed to delete existing transcript segments");
                    } else if let Err(e) = audiobook_transcripts::insert_segments(
                        &mut *tx,
                        file.id,
                        &segments,
                        Some(&config_tag),
                    )
                    .await
                    {
                        warn!(error = %e, file_id = %file.id, "Failed to store transcript segments");
                    } else {
                        tx.commit().await?;
                        segments_total += count;
                    }

                    transcribed += 1;
                }
                Err(e) => {
                    if e.is_transient() {
                        had_transient_failure = true;
                    }
                    warn!(
                        error = %e,
                        file_id = %file.id,
                        path = %file_path.display(),
                        transient = e.is_transient(),
                        "Failed to transcribe audio file"
                    );
                }
            }

            drop(_gpu_guard);
        }

        info!(
            book_id = %book_id,
            transcribed,
            up_to_date,
            outdated_kept,
            segments_total,
            "Audiobook transcription complete"
        );

        if segments_total > 0 {
            match crate::audio::book_editions::settle(&self.pool, &self.storage_root, book_id, None)
                .await
            {
                Ok(editions) if editions.changed() => {
                    info!(book_id = %book_id, editions = ?editions.report, "transcripts showed other audiobook editions");
                    crate::audio::book_editions::follow_up(
                        &self.pool,
                        &self.nats_publisher,
                        book_id,
                        &editions,
                        "audiobook_chapter_repair",
                    )
                    .await;
                }
                Ok(_) => {}
                Err(e) => {
                    warn!(error = %e, book_id = %book_id, "failed to settle audiobook editions")
                }
            }

            // Index transcript segments in MeiliSearch for hybrid semantic search
            if let Err(e) = crate::search::index_transcript_segments_for_book(
                &self.pool,
                self.search.as_ref(),
                book_id,
            )
            .await
            {
                warn!(error = %e, book_id = %book_id, "Failed to index transcript segments");
            }

            // If ebook segments exist, trigger cross-format sync
            if ebook_segments::has_segments(&self.pool, book_id)
                .await
                .unwrap_or(false)
            {
                crate::jobs::enqueue_cross_format_sync(
                    &self.pool,
                    &self.nats_publisher,
                    book_id,
                    "auto",
                )
                .await;
            }

            // Trigger chapter detection from transcript analysis
            crate::jobs::enqueue_chapter_detect(&self.pool, &self.nats_publisher, book_id, "auto")
                .await;
        }

        if transcribed + up_to_date + outdated_kept == 0 {
            let msg = format!("All {total} file(s) failed to transcribe");
            // If at least one file failed transiently (STT crash/restart
            // window), retry the whole job — the audio is fine, the service
            // wasn't. Otherwise the files are genuinely undecodable: permanent.
            if had_transient_failure {
                return Err(crate::jobs::JobError::Retryable(msg));
            }
            return Err(crate::jobs::JobError::ExecutionFailed(msg));
        }

        Ok(json!({
            "transcribed": transcribed,
            "up_to_date": up_to_date,
            "outdated_kept": outdated_kept,
            "segments_total": segments_total,
        }))
    }
}
