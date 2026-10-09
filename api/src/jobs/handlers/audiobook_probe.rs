//! Audiobook probe job handler.
//!
//! Probes all audio files for a book to extract duration, codec info, and
//! chapters. If all files lack chapters and the book has multiple files,
//! auto-generates one chapter per file.

use crate::RuntimeConfigManager;
use crate::audio::{auto_chapters, chapters, probe, repair};
use crate::jobs::worker::JobHandler;
use crate::jobs::{JobError, JobResult};
use crate::repositories::{audiobook_chapters, audiobook_files, books};
use async_trait::async_trait;
use nats_jobs::Publisher;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sqlx::PgPool;
use std::sync::Arc;
use tokio::sync::Semaphore;
use tokio_util::sync::CancellationToken;
use tracing::{info, warn};
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AudiobookProbePayload {
    pub book_id: Uuid,
}

pub struct AudiobookProbeHandler {
    pool: PgPool,
    storage_root: String,
    fs_semaphore: Arc<Semaphore>,
    runtime_config: RuntimeConfigManager,
    nats_publisher: Publisher,
}

impl AudiobookProbeHandler {
    pub fn new(
        pool: PgPool,
        storage_root: String,
        fs_semaphore: Arc<Semaphore>,
        runtime_config: RuntimeConfigManager,
        nats_publisher: Publisher,
    ) -> Self {
        Self {
            pool,
            storage_root,
            fs_semaphore,
            runtime_config,
            nats_publisher,
        }
    }
}

#[async_trait]
impl JobHandler for AudiobookProbeHandler {
    fn job_type(&self) -> &'static str {
        "audiobook_probe"
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
        nats_jobs::check_cancelled(&cancel)?;
        let _fs_permit = self
            .fs_semaphore
            .acquire()
            .await
            .map_err(|_| JobError::ExecutionFailed("FS semaphore closed".into()))?;
        let payload: AudiobookProbePayload = serde_json::from_value(payload)?;
        let book_id = payload.book_id;

        let files = audiobook_files::list_by_book(&self.pool, book_id).await?;
        if files.is_empty() {
            return Ok(json!({"status": "no_files"}));
        }

        let mut probed = 0u64;
        let mut chapters_found = 0u64;
        let mut total_duration: f64 = 0.0;
        let mut all_files_lack_chapters = true;

        // Collect file info for auto-chapter generation
        let mut file_infos: Vec<(Uuid, String, Option<f64>)> = Vec::new();

        for file in &files {
            let file_path =
                crate::audio::path::resolve_file_path(&self.storage_root, &file.file_path);

            if !file_path.is_file() {
                warn!(
                    file_id = %file.id,
                    path = %file_path.display(),
                    "Audio file not found on disk, skipping probe"
                );
                file_infos.push((file.id, file.file_path.clone(), file.duration_seconds));
                continue;
            }

            // MP3s without a Xing/Info header force browsers and ffprobe to
            // estimate duration from bitrate, which is wrong for many old
            // rips and breaks browser seek (read-along desync). Repair before
            // probing so the duration we store is accurate.
            match repair::ensure_xing_header(&file_path).await {
                Ok(true) => info!(file_id = %file.id, "repaired MP3 header before probe"),
                Ok(false) => {}
                Err(e) => warn!(error = %e, file_id = %file.id, "failed to repair MP3 header"),
            }

            match probe::probe_audio(&file_path).await {
                Ok(result) => {
                    let duration = result.duration_seconds;
                    let file_size = tokio::fs::metadata(&file_path)
                        .await
                        .ok()
                        .map(|m| m.len() as i64);

                    // Update audiobook_files with probe data. AudioProbeResult only
                    // exposes duration today — bitrate/codec/channels/sample_rate are
                    // left untouched on existing rows.
                    let update = audiobook_files::ProbeUpdate {
                        duration_seconds: duration,
                        file_size,
                        ..Default::default()
                    };
                    if let Err(e) =
                        audiobook_files::update_probe_data(&self.pool, file.id, &update).await
                    {
                        warn!(error = %e, file_id = %file.id, "Failed to update probe data");
                    }

                    if let Some(dur) = duration {
                        total_duration += dur;
                    }

                    // Resolve and store chapters
                    let chapters = chapters::resolve_chapters(&file_path, None, None).await;
                    if !chapters.is_empty() {
                        all_files_lack_chapters = false;
                        chapters_found += chapters.len() as u64;
                        if let Err(e) =
                            audiobook_chapters::insert_chapters(&self.pool, file.id, &chapters)
                                .await
                        {
                            warn!(error = %e, file_id = %file.id, "Failed to store chapters");
                        }
                    }

                    // Detect intro/outro from chapter names
                    if !chapters.is_empty() {
                        let intro_patterns = [
                            "intro",
                            "opening",
                            "foreword",
                            "produced by",
                            "introduction",
                        ];
                        let credits_patterns =
                            ["credits", "about the author", "copyright", "afterword"];

                        let mut intro_end: Option<f64> = None;
                        let mut outro_start: Option<f64> = None;

                        for ch in &chapters {
                            if let Some(title) = &ch.title {
                                let lower = title.to_lowercase();
                                if intro_patterns.iter().any(|p| lower.contains(p)) {
                                    intro_end = Some(ch.end_seconds);
                                }
                                if credits_patterns.iter().any(|p| lower.contains(p))
                                    && (outro_start.is_none()
                                        || ch.start_seconds > outro_start.unwrap())
                                {
                                    outro_start = Some(ch.start_seconds);
                                }
                            }
                        }

                        if intro_end.is_some() || outro_start.is_some() {
                            let _ = sqlx::query(
                                "UPDATE audiobook_files SET intro_end_seconds = COALESCE($2, intro_end_seconds), outro_start_seconds = COALESCE($3, outro_start_seconds), intro_outro_source = COALESCE($4, intro_outro_source) WHERE id = $1 AND (intro_outro_source IS NULL OR intro_outro_source != 'manual')"
                            )
                            .bind(file.id)
                            .bind(intro_end)
                            .bind(outro_start)
                            .bind(Some("chapter"))
                            .execute(&self.pool)
                            .await;
                        }
                    }

                    file_infos.push((file.id, file.file_path.clone(), duration));
                    probed += 1;
                }
                Err(e) => {
                    warn!(
                        error = %e,
                        file_id = %file.id,
                        path = %file_path.display(),
                        "Failed to probe audio file"
                    );
                    file_infos.push((file.id, file.file_path.clone(), file.duration_seconds));
                }
            }
        }

        // Auto-generate file-per-chapter if all files lack chapters and book has >1 file
        let mut auto_generated = false;
        if all_files_lack_chapters && file_infos.len() > 1 {
            let generated = auto_chapters::generate_file_per_chapter(&file_infos);
            for (file_id, chapters) in &generated {
                if let Err(e) =
                    audiobook_chapters::insert_chapters(&self.pool, *file_id, chapters).await
                {
                    warn!(error = %e, file_id = %file_id, "Failed to store auto-generated chapters");
                }
            }
            auto_generated = true;
            chapters_found = generated.len() as u64;
            info!(
                book_id = %book_id,
                file_count = file_infos.len(),
                "Auto-generated file-per-chapter for multi-file audiobook"
            );
        }

        // Reindex file order now that track_number/disc_number are populated
        if let Err(e) = audiobook_files::reindex_file_order(&self.pool, book_id).await {
            warn!(error = %e, book_id = %book_id, "Failed to reindex audiobook file order after probe");
        }

        // Update book duration to sum of all file durations
        if total_duration > 0.0 {
            let update = books::BookUpdate {
                duration_seconds: Some(total_duration as i32),
                ..Default::default()
            };
            let _ = books::update(&self.pool, book_id, update).await;
        }

        // Enqueue audio enrichment (peaks + LUFS) as a separate low-priority job.
        // This avoids adding 1-2 minutes of processing to every probe, and respects
        // the enrichment_enabled config toggle.
        let config = self.runtime_config.get().await;
        if config.enrichment_enabled {
            for file in &files {
                let enrich_payload = serde_json::json!({
                    "file_id": file.id,
                    "book_id": book_id,
                });
                let context = crate::jobs::JobContext::new("Audio enrichment — peaks & LUFS");
                if let Err(e) = crate::jobs::enqueue_routine(
                    &self.pool,
                    &self.nats_publisher,
                    "enrich_audiobook",
                    enrich_payload,
                    context,
                )
                .await
                {
                    warn!(error = %e, file_id = %file.id, "Failed to enqueue audio enrichment");
                }
            }
        }

        info!(
            book_id = %book_id,
            probed,
            chapters_found,
            total_duration,
            auto_generated,
            "Audiobook probe complete"
        );

        // Auto-transcribe if transcript_mode=auto
        if config.transcript_mode == "auto" {
            let title = books::get_title(&self.pool, book_id)
                .await
                .ok()
                .flatten()
                .unwrap_or_else(|| book_id.to_string());
            crate::jobs::enqueue_audiobook_transcribe(
                &self.pool,
                &self.nats_publisher,
                book_id,
                &title,
            )
            .await;
        }

        Ok(json!({
            "probed": probed,
            "chapters_found": chapters_found,
            "total_duration_seconds": total_duration,
            "auto_generated": auto_generated,
        }))
    }
}
