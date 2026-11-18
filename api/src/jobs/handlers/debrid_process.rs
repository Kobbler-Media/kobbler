//! Debrid stream processing handler.
//!
//! Triggered after a progressive download completes. Runs the processing
//! pipeline on the cached file: probe → single-pass media analysis → skip
//! detection, the same steps and the same detector a library file gets.
//!
//! Everything found is the download's own (`debrid_files`): its chapters,
//! intro/credits, corrupt ranges and detection diagnostics, with the analysis
//! sidecars in `episode_cache/debrid_file/{id}/`. Each release of an episode
//! keeps its own, and siblings in the season are matched against whichever
//! file of theirs was analysed.

use crate::RuntimeConfigManager;
use crate::debrid_outcomes::StreamRelease;
use crate::enrichment_config::skip_detect_config_tag;
use crate::ffmpeg::persist::{ChapterSkipSegment, chapter_fallback_segments};
use crate::ffmpeg::probe;
use crate::gpu::GpuLock;
use crate::jobs::worker::JobHandler;
use crate::jobs::{JobError, JobResult};
use crate::media_cache::analyze::{AnalyzeInputs, run_analysis};
use crate::media_cache::embeddings::{embed_frames, ready_vision_gpu};
use crate::media_cache::episode_cache::{MediaKind, episode_cache_dir};
use crate::media_cache::writers::write_manifest_at_async;
use crate::repositories::debrid_files::{self, DebridFile};
use crate::repositories::debrid_stream_sessions;
use crate::repositories::skip_detect_candidates::{self, Scope};
use crate::routes::admin::debrid_open;
use crate::routes::admin::streaming::ChapterResponse;
use crate::skip_detect::cache_detect::{
    EpisodeDiagnostics, MovieHandle, detect_movie_from_cache, detect_season_from_cache,
    season_episode_files,
};
use crate::skip_detect::{DetectedSegment, PipelineMode, diagnostics};
use async_trait::async_trait;
use nats_jobs::Publisher;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sqlx::PgPool;
use std::path::{Path, PathBuf};
use tokio_util::sync::CancellationToken;
use tracing::{info, warn};
use uuid::Uuid;

/// Payload for debrid_process jobs.
#[derive(Debug, Deserialize, Serialize)]
pub struct DebridProcessPayload {
    pub session_id: Uuid,
    pub cache_path: String,
}

pub struct DebridProcessHandler {
    pool: PgPool,
    runtime_config: RuntimeConfigManager,
    gpu_lock: GpuLock,
    storage_root: String,
}

impl DebridProcessHandler {
    pub fn new(
        pool: PgPool,
        _publisher: Publisher,
        runtime_config: RuntimeConfigManager,
        gpu_lock: GpuLock,
        storage_root: String,
    ) -> Self {
        Self {
            pool,
            runtime_config,
            gpu_lock,
            storage_root,
        }
    }

    async fn measure_picture(&self, session_id: Uuid, cache_path: &Path, duration_secs: f64) {
        let release: StreamRelease =
            match debrid_stream_sessions::release(&self.pool, session_id, None).await {
                Ok(Some(release)) => release.into(),
                _ => return,
            };
        let source = cache_path.to_string_lossy();
        let Some(picture) =
            debrid_open::picture::measured(&self.pool, &release, &source, duration_secs).await
        else {
            return;
        };
        if let Err(e) = debrid_stream_sessions::set_picture(&self.pool, session_id, &picture).await
        {
            warn!(error = %e, %session_id, "failed to store debrid picture area");
        }
    }
}

/// Queue processing of the complete download at `cache_path`, once.
pub async fn enqueue(pool: &PgPool, publisher: &Publisher, session_id: Uuid, cache_path: &Path) {
    let cache_path_str = cache_path.to_string_lossy().to_string();
    let already_queued: bool = sqlx::query_scalar(
        r#"
        SELECT EXISTS(
            SELECT 1 FROM job_status
            WHERE job_type = 'debrid_process'
              AND status IN ('pending', 'running')
              AND payload->>'cache_path' = $1
        )
        "#,
    )
    .bind(&cache_path_str)
    .fetch_one(pool)
    .await
    .unwrap_or(false);
    if already_queued {
        info!(
            session_id = %session_id,
            path = %cache_path_str,
            "Skipping debrid_process — job already queued for this file"
        );
        return;
    }

    info!(
        session_id = %session_id,
        path = %cache_path_str,
        "Triggering processing pipeline for cached debrid file"
    );
    let payload = json!({
        "session_id": session_id,
        "cache_path": cache_path_str,
    });
    let context = crate::jobs::JobContext::for_entity(
        "debrid_session",
        session_id,
        "Processing cached debrid stream",
    );
    if let Err(e) = crate::jobs::enqueue(pool, publisher, "debrid_process", payload, context).await
    {
        tracing::error!(session_id = %session_id, error = %e, "Failed to enqueue debrid_process job");
    }
}

/// Whether the download at `cache_path` lacks any of what processing leaves now: its
/// own seek thumbnails, a current analysis, and (with vision on) an embeddings attempt by
/// today's model. `None` is a download never processed. Blocking: reads the disk.
pub fn analysis_outdated(
    storage_root: &str,
    file: Option<Uuid>,
    cache_path: &str,
    vision: bool,
) -> bool {
    let Some(file_id) = file else {
        return true;
    };
    let thumbnails = crate::media_cache::debrid_file_cache_dir(Path::new(cache_path))
        .join("trickplay")
        .join("thumbnails.vtt");
    if !thumbnails.exists() {
        return true;
    }
    let dir = episode_cache_dir(storage_root, &MediaKind::DebridFile { file_id });
    let Some(manifest) = crate::media_cache::readers::read_manifest_at(&dir) else {
        return true;
    };
    manifest.pipeline_version < crate::media_cache::PIPELINE_VERSION
        || (vision
            && manifest.artefacts.embeddings.as_ref().is_none_or(|info| {
                crate::media_cache::readers::embeddings_model(info)
                    != Some(crate::embedding::EMBED_MODEL)
            }))
}

#[async_trait]
impl JobHandler for DebridProcessHandler {
    fn job_type(&self) -> &'static str {
        "debrid_process"
    }

    fn max_concurrent(&self) -> usize {
        2
    }

    async fn execute(
        &self,
        job_id: Uuid,
        payload: Value,
        cancel: CancellationToken,
    ) -> JobResult<Value> {
        nats_jobs::check_cancelled(&cancel)?;

        let payload: DebridProcessPayload = serde_json::from_value(payload)
            .map_err(|e| JobError::ExecutionFailed(format!("Invalid payload: {e}")))?;

        let session_id = payload.session_id;
        let cache_path = PathBuf::from(&payload.cache_path);

        info!(
            job_id = %job_id,
            session_id = %session_id,
            path = %payload.cache_path,
            "Starting debrid stream processing"
        );

        // The player can end the session before its download finishes; what is left
        // to process then belongs to nobody.
        let Some(session_row) = sqlx::query!(
            r#"
            SELECT media_type, media_id
            FROM debrid_stream_sessions
            WHERE id = $1
            "#,
            session_id,
        )
        .fetch_optional(&self.pool)
        .await
        .map_err(|e| JobError::ExecutionFailed(format!("session lookup: {e}")))?
        else {
            info!(session_id = %session_id, "Debrid session is gone; nothing to process");
            return Ok(json!({ "session_id": session_id, "skipped": "session_gone" }));
        };

        let is_file = {
            let fp = cache_path.clone();
            tokio::task::spawn_blocking(move || fp.is_file())
                .await
                .unwrap_or(false)
        };
        if !is_file {
            return Err(JobError::ExecutionFailed(format!(
                "Cached file not found: {}",
                payload.cache_path
            )));
        }

        let file = debrid_files::upsert(
            &self.pool,
            &payload.cache_path,
            &session_row.media_type,
            session_row.media_id,
        )
        .await
        .map_err(|e| JobError::ExecutionFailed(format!("debrid file row: {e}")))?;

        let probe_result = probe::probe(&cache_path)
            .await
            .map_err(|e| JobError::ExecutionFailed(format!("probe failed: {e}")))?;

        let probe_json = json!({
            "duration": probe_result.format.duration_seconds,
            "bitrate": probe_result.format.bit_rate_kbps,
            "format_name": probe_result.format.format_name,
            "video_streams": probe_result.video_streams.iter().map(|v| json!({
                "codec": v.codec,
                "width": v.width,
                "height": v.height,
                "framerate": v.framerate,
                "bitrate": v.bitrate_kbps,
            })).collect::<Vec<_>>(),
            "audio_streams": probe_result.audio_streams.iter().map(|a| json!({
                "codec": a.codec,
                "channels": a.channels,
                "sample_rate": a.sample_rate,
                "language": a.language,
            })).collect::<Vec<_>>(),
            "subtitle_streams": probe_result.subtitle_streams.iter().map(|s| json!({
                "codec": s.codec,
                "language": s.language,
            })).collect::<Vec<_>>(),
        });

        sqlx::query!(
            r#"
            UPDATE debrid_stream_sessions
            SET probe_data = $1, updated_at = NOW()
            WHERE id = $2
            "#,
            probe_json,
            session_id,
        )
        .execute(&self.pool)
        .await
        .map_err(|e| JobError::ExecutionFailed(format!("Failed to store probe data: {e}")))?;

        let video = probe_result.video_streams.first();
        info!(
            session_id = %session_id,
            codec = ?video.map(|v| &v.codec),
            resolution = ?video.map(|v| format!("{}x{}", v.width, v.height)),
            audio_tracks = probe_result.audio_streams.len(),
            "Debrid probe complete"
        );

        nats_jobs::check_cancelled(&cancel)?;

        let chapters: Vec<ChapterResponse> = probe_result
            .chapters
            .iter()
            .map(|ch| ChapterResponse {
                index: ch.index,
                title: ch.title.clone(),
                start_seconds: ch.start_seconds,
                end_seconds: ch.end_seconds,
            })
            .collect();
        debrid_files::set_chapters(&self.pool, file.id, &chapters)
            .await
            .map_err(|e| JobError::ExecutionFailed(format!("Failed to store chapters: {e}")))?;

        // What the chapter titles say plays straight away; detection below refines it.
        let chapter_segments = chapter_fallback_segments(&probe_result.chapters, &file.media_type);
        if !chapter_segments.is_empty() {
            debrid_files::set_skip_segments(&self.pool, file.id, &chapter_segments)
                .await
                .map_err(|e| {
                    JobError::ExecutionFailed(format!("Failed to store skip segments: {e}"))
                })?;
        }

        // A release downloaded ahead was never opened, so nothing has measured its bars;
        // done here, the episode starts with them already trimmed.
        self.measure_picture(
            session_id,
            &cache_path,
            probe_result.format.duration_seconds.unwrap_or_default(),
        )
        .await;

        let kind = MediaKind::DebridFile { file_id: file.id };
        let content_dir = episode_cache_dir(&self.storage_root, &kind);

        // Single-pass media analysis, as a library file gets it: trickplay beside the
        // download, the detection inputs in the file's episode_cache dir.
        let config = self.runtime_config.get().await;
        if config.enrichment_enabled {
            let media_cache_dir = crate::media_cache::debrid_file_cache_dir(&cache_path);
            let vision_url = config.effective_vision_url().filter(|u| !u.is_empty());
            let inputs = AnalyzeInputs {
                input_path: cache_path.clone(),
                cache_dir: media_cache_dir.clone(),
                trickplay_dir: media_cache_dir.join("trickplay"),
                is_video: true,
                known_duration: probe_result.format.duration_seconds,
                want_embed_src: vision_url.is_some(),
                media_type: file.media_type.clone(),
                // Written here once the embeddings are in it.
                write_manifest_on_success: false,
                only_embed_src: false,
                episode_cache_dir: Some(content_dir.clone()),
                analyzed_file_id: Some(file.id),
            };
            match run_analysis(inputs, &cancel).await {
                Ok(mut outcome) => {
                    if outcome.embed_src_ok() {
                        let frames = std::mem::take(&mut outcome.embed_src_frames);
                        ready_vision_gpu(&self.gpu_lock, &config);
                        let gpu_guard = self.gpu_lock.acquire_for_vision().await;
                        if gpu_guard.is_none() {
                            nats_jobs::check_cancelled(&cancel)?;
                        }
                        if let Err(e) = embed_frames(
                            &content_dir,
                            vision_url.as_deref(),
                            frames,
                            &mut outcome.manifest,
                            &cancel,
                        )
                        .await
                        {
                            warn!(error = %e, %session_id, "debrid embeddings failed — skip-detect runs without vision");
                        }
                    }
                    if let Err(e) =
                        write_manifest_at_async(content_dir.clone(), outcome.manifest.clone()).await
                    {
                        warn!(error = %e, "debrid manifest write failed");
                    }
                    let corrupt_ranges = std::mem::take(&mut outcome.corrupt_ranges);
                    if let Err(e) =
                        debrid_files::set_corrupt_ranges(&self.pool, file.id, corrupt_ranges).await
                    {
                        warn!(error = %e, file_id = %file.id, "storing debrid corrupt ranges failed");
                    }
                    info!(
                        session_id = %session_id,
                        file_id = %file.id,
                        branches = outcome.branch_results.len(),
                        errors = outcome.per_branch_errors.len(),
                        duration_secs = outcome.input_duration_secs,
                        "debrid media analysis complete"
                    );
                }
                Err(e) => {
                    warn!(
                        error = %e,
                        session_id = %session_id,
                        "Failed to run media analysis for debrid stream"
                    );
                }
            }
        }

        let loudness = crate::media_cache::episode_cache::loudness(&self.storage_root, &kind);
        persist_loudness(&self.pool, &file, loudness).await?;

        if let Err(e) = self.detect(&file).await {
            warn!(error = %e, file_id = %file.id, "debrid skip detect failed");
        }

        Ok(json!({
            "session_id": session_id,
            "debrid_file_id": file.id,
            "probe_complete": true,
            "duration": probe_result.format.duration_seconds,
        }))
    }
}

impl DebridProcessHandler {
    /// Detect intro/credits for `file` as the library detects them for its own files:
    /// the same detector over the season's analyses (or the movie's), gated by the same
    /// mode, leaving segments and diagnostics on every debrid download it looked at.
    async fn detect(&self, file: &DebridFile) -> Result<(), String> {
        let config = self.runtime_config.get().await;
        let mode: PipelineMode = config.skip_detect_mode.parse().unwrap_or(PipelineMode::Off);
        if mode == PipelineMode::Off {
            return Ok(());
        }
        let config_tag = skip_detect_config_tag(&mode, &config.effective_vision_url());
        // Detection classifies cached frame vectors, which needs only the text
        // embedder (prompt vectors).
        let text_url = config
            .effective_text_embedding_url()
            .filter(|s| !s.is_empty());

        let results = match file.media_type.as_str() {
            "episode" => {
                let episodes = season_episode_files(&self.pool, &self.storage_root, file)
                    .await
                    .map_err(|e| format!("season_episode_files: {e}"))?;
                let debrid: Vec<Uuid> = episodes
                    .iter()
                    .filter(|e| matches!(e.cache, MediaKind::DebridFile { .. }))
                    .map(|e| e.file_id)
                    .collect();
                detect_season_from_cache(
                    &self.pool,
                    text_url.as_deref(),
                    &episodes,
                    &self.storage_root,
                )
                .await
                .into_iter()
                .filter(|(id, _, _)| debrid.contains(id))
                .collect()
            }
            "movie" => {
                let handle = MovieHandle {
                    file_id: file.id,
                    duration_seconds: 0.0,
                    cache: MediaKind::DebridFile { file_id: file.id },
                };
                let (segments, diag) = detect_movie_from_cache(
                    &self.pool,
                    text_url.as_deref(),
                    &handle,
                    &self.storage_root,
                )
                .await;
                vec![(file.id, segments, diag)]
            }
            _ => Vec::new(),
        };
        self.persist(file, results, &config_tag).await
    }

    async fn persist(
        &self,
        file: &DebridFile,
        results: Vec<(Uuid, Vec<DetectedSegment>, EpisodeDiagnostics)>,
        config_tag: &str,
    ) -> Result<(), String> {
        let ids: Vec<Uuid> = results.iter().map(|(id, _, _)| *id).collect();
        let mut rows = Vec::new();
        for (id, segments, diag) in &results {
            rows.extend(diagnostics::candidate_rows(*id, diag));
            let chapters: Vec<probe::ChapterInfo> = debrid_files::chapters(&self.pool, *id)
                .await
                .unwrap_or_default()
                .into_iter()
                .map(|c| probe::ChapterInfo {
                    index: c.index,
                    start_seconds: c.start_seconds,
                    end_seconds: c.end_seconds,
                    title: c.title,
                })
                .collect();
            let merged = merge_segments(
                chapter_fallback_segments(&chapters, &file.media_type),
                segments.iter().map(detected_to_skip_segment).collect(),
            );
            debrid_files::set_skip_segments(&self.pool, *id, &merged)
                .await
                .map_err(|e| format!("storing skip segments: {e}"))?;
        }
        if let Err(e) =
            skip_detect_candidates::replace(&self.pool, Scope::DebridFiles(&ids), &rows, config_tag)
                .await
        {
            warn!(error = %e, file_id = %file.id, "storing debrid candidates failed");
        }
        let diags: Vec<EpisodeDiagnostics> = results.into_iter().map(|(_, _, d)| d).collect();
        diagnostics::log(file.id, "debrid", &diags);
        debrid_files::mark_skip_detect_ran(&self.pool, &ids)
            .await
            .map_err(|e| format!("marking detected: {e}"))?;
        info!(
            file_id = %file.id,
            files = ids.len(),
            candidates = rows.len(),
            config_tag,
            "debrid skip detect complete"
        );
        Ok(())
    }
}

fn detected_to_skip_segment(s: &DetectedSegment) -> ChapterSkipSegment {
    ChapterSkipSegment {
        segment_type: s.segment_type.as_str().to_string(),
        start_seconds: s.start_seconds,
        end_seconds: s.end_seconds,
    }
}

/// What a library file's row would carry, onto every session on the download.
async fn persist_loudness(
    pool: &PgPool,
    file: &DebridFile,
    loudness: crate::streaming::loudness::PlaybackLoudness,
) -> JobResult<()> {
    sqlx::query!(
        r#"
        UPDATE debrid_stream_sessions
        SET lufs = COALESCE($2, lufs), downmix_peak_db = COALESCE($3, downmix_peak_db),
            updated_at = NOW()
        WHERE cache_path = $1
        "#,
        file.cache_path,
        loudness.lufs,
        loudness.downmix_peak_db,
    )
    .execute(pool)
    .await
    .map_err(|e| JobError::ExecutionFailed(format!("Failed to store loudness: {e}")))?;
    Ok(())
}

/// Detection wins over chapter titles of the same type: titles can lie.
fn merge_segments(
    chapter_segments: Vec<ChapterSkipSegment>,
    detected: Vec<ChapterSkipSegment>,
) -> Vec<ChapterSkipSegment> {
    let mut by_type: std::collections::BTreeMap<String, ChapterSkipSegment> =
        std::collections::BTreeMap::new();
    for s in chapter_segments.into_iter().chain(detected) {
        by_type.insert(s.segment_type.clone(), s);
    }
    by_type.into_values().collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::skip_detect::SegmentType;

    #[test]
    fn segment_types_are_named_as_the_players_and_library_name_them() {
        for kind in [
            SegmentType::Intro,
            SegmentType::Credits,
            SegmentType::Recap,
            SegmentType::PostCreditsScene,
        ] {
            let segment = DetectedSegment {
                segment_type: kind,
                start_seconds: 1.0,
                end_seconds: 2.0,
                confidence: 1.0,
                veto_score: 0.0,
                source: "test".to_string(),
            };
            assert_eq!(
                detected_to_skip_segment(&segment).segment_type,
                kind.as_str()
            );
        }
    }
}
