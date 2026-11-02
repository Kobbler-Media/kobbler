//! Single-pass media analysis job handler.
//!
//! Decodes the media file once via `kob_ffmpeg::single_pass::run_single_pass`
//! and fans the output into the per-file artefact cache (manifest + sidecars
//! under `{storage_root}/media_cache/<file_id>/`). Each successful branch is
//! recorded in the manifest with its parameters; failures are isolated.
//!
//! `vhash` and `embed_src` are narrowed to the same time windows V1's
//! `skip_detect` used to decode directly (intro window + credits window for
//! vhash, credits tail for embed_src). The decoder still runs over the whole
//! file — other branches (chromaprint, silence, loudness, audio_peaks,
//! black_frames) need the full audio/video stream — but the per-frame
//! scale + JPEG/descriptor work only happens inside the windows.
//!
//! `embed_src` JPEGs are kept in memory and sent to the vision service
//! (batches of 16) inline inside this handler; the f16-packed result is
//! written to `embeddings.bin` before the job returns. Previously a separate
//! `generate_embeddings` job read a disk spool back into memory — that hop
//! is gone.
//!
//! Records a `media_cache_runs` row for the run so the comparison panel can
//! show before/after deltas, then chains `skip_detect` once the cache
//! (including `embeddings.bin` when vision is configured) is durable.

use crate::RuntimeConfigManager;
use crate::gpu::GpuLock;
use crate::jobs::worker::JobHandler;
use crate::jobs::{JobError, JobResult, MediaRootIoGate, PhaseTimings};
use crate::media_cache::analyze::{AnalyzeInputs, run_analysis};
use crate::media_cache::embeddings::{l2_normalise, pack_fp16};
use crate::media_cache::picture_area::{self, PictureArea};
use crate::media_cache::writers::{ArtefactInfo, write_atomic, write_manifest};
use crate::media_cache::{self, Branch, PIPELINE_VERSION};
use crate::repositories::media_cache_runs::{self, TriggeredBy};
use crate::repositories::{audiobook_files, media_roots, movie_files, tv_episode_files};
use crate::skip_detect::vision::VisionClient;
use crate::streaming::session::MediaType;
use async_trait::async_trait;
use kob_ffmpeg::single_pass::BranchId;
use nats_jobs::Publisher;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sqlx::PgPool;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Instant;
use tokio::sync::Semaphore;
use tokio_util::sync::CancellationToken;
use tracing::{debug, info, warn};
use uuid::Uuid;

/// SigLIP2 SO400M output dimensionality.
const EMBED_DIM: usize = 1152;
/// Frames-per-second at which embed_src is grabbed.
const EMBED_FPS: f64 = 1.0;
/// embed_src JPEG width — must match the vision server's expected input.
const EMBED_WIDTH: i32 = 768;
/// Vision model identifier recorded in the manifest.
const EMBED_MODEL: &str = "google/siglip2-so400m-patch16-naflex";

#[derive(Debug, Deserialize, Serialize)]
pub struct MediaAnalyzePayload {
    pub file_id: Uuid,
    /// "episode" | "movie" | "audiobook"
    pub media_type: String,
    #[serde(default)]
    pub force_regen: bool,
    /// Set by the manual-button flow so the UI can correlate the run row.
    #[serde(default)]
    pub run_id: Option<Uuid>,
    /// "manual" | "auto_ingest" | "backfill". Required — every caller names
    /// its source so we can segment run stats by trigger.
    pub triggered_by: String,
    /// Fill only the cheap checks that read what an earlier analyze left on disk — the
    /// picture area from trickplay — for files analysed before those checks existed.
    /// No decode, and no skip_detect behind it.
    #[serde(default)]
    pub gaps_only: bool,
}

pub struct MediaAnalyzeHandler {
    pool: PgPool,
    publisher: Publisher,
    runtime_config: RuntimeConfigManager,
    enrichment_semaphore: Arc<Semaphore>,
    gpu_lock: GpuLock,
    storage_root: String,
    io_gate: MediaRootIoGate,
}

impl MediaAnalyzeHandler {
    pub fn new(
        pool: PgPool,
        publisher: Publisher,
        runtime_config: RuntimeConfigManager,
        enrichment_semaphore: Arc<Semaphore>,
        gpu_lock: GpuLock,
        storage_root: String,
        io_gate: MediaRootIoGate,
    ) -> Self {
        Self {
            pool,
            publisher,
            runtime_config,
            enrichment_semaphore,
            gpu_lock,
            storage_root,
            io_gate,
        }
    }

    /// Resolve where this file's content artefacts live in episode_cache.
    /// `None` for media types that don't get a cross-episode/movie cache
    /// (audiobook, recording, …) — those write only to per-file media_cache.
    async fn resolve_episode_cache_target(
        &self,
        media_type: &str,
        file_id: Uuid,
    ) -> JobResult<Option<crate::media_cache::episode_cache::MediaKind>> {
        match media_type {
            "movie" => {
                let row = movie_files::get(&self.pool, file_id)
                    .await
                    .map_err(|e| JobError::ExecutionFailed(format!("DB error: {e}")))?
                    .ok_or_else(|| {
                        JobError::ExecutionFailed(format!("Movie file not found: {file_id}"))
                    })?;
                Ok(Some(crate::media_cache::episode_cache::MediaKind::Movie {
                    movie_id: row.movie_id,
                }))
            }
            "episode" => Ok(Some(crate::media_cache::episode_cache::MediaKind::TvFile {
                file_id,
            })),
            _ => Ok(None),
        }
    }

    /// Supplementation path: the warm-cache short-circuit detected a manifest
    /// at the current pipeline version with everything present EXCEPT the
    /// embeddings (e.g. vision was off during the first analyze and has since
    /// been turned on). Run a video-only single-pass just for `embed_src`,
    /// send the JPEGs to vision, write `embeddings.bin`, update the manifest,
    /// and chain `skip_detect`. The audio decoder is skipped because
    /// `single_pass` discards unused streams.
    ///
    /// No auto-retry — if vision is still down, the attempt is recorded in
    /// the manifest (`present: false`, `attempted_at`, `error`) and we move
    /// on. The backfill walker respects `media_cache_version` so this path
    /// is only ever hit by a user Re-analyze Media click.
    /// The picture area is a check over trickplay already on disk; failing it never
    /// fails the analyze.
    async fn check_picture(&self, payload: &MediaAnalyzePayload) -> Option<PictureArea> {
        let media_type = match payload.media_type.as_str() {
            "movie" => MediaType::Movie,
            "episode" => MediaType::Episode,
            _ => return None,
        };
        match picture_area::check_file(&self.pool, &self.storage_root, media_type, payload.file_id)
            .await
        {
            Ok(area) => area.filter(|a| *a != PictureArea::FULL),
            Err(e) => {
                warn!(file_id = %payload.file_id, error = %e, "Picture area check failed");
                None
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    async fn run_embeddings_supplement(
        &self,
        payload: MediaAnalyzePayload,
        full_path: PathBuf,
        used_spool: bool,
        known_duration: Option<f64>,
        mut manifest: crate::media_cache::writers::Manifest,
        triggered_by: TriggeredBy,
        config: &crate::runtime_config::RuntimeConfig,
        vision_url: Option<String>,
        cancel: CancellationToken,
    ) -> JobResult<Value> {
        let job_started = Instant::now();
        let mut timings = PhaseTimings::new();

        let run_id = match payload.run_id {
            Some(id) => {
                if let Err(e) = media_cache_runs::start_run_with_id(
                    &self.pool,
                    id,
                    payload.file_id,
                    &payload.media_type,
                    triggered_by,
                    PIPELINE_VERSION,
                )
                .await
                {
                    warn!(error = %e, run_id = %id, "supplement: start_run_with_id failed");
                }
                id
            }
            None => media_cache_runs::start_run(
                &self.pool,
                payload.file_id,
                &payload.media_type,
                triggered_by,
                PIPELINE_VERSION,
            )
            .await
            .map_err(|e| JobError::ExecutionFailed(format!("start_run failed: {e}")))?,
        };

        let started = Instant::now();
        let trickplay_dir = media_cache::trickplay_dir(&self.storage_root, payload.file_id);
        let cache_dir = media_cache::cache_dir(&self.storage_root, payload.file_id);

        // Gate the NAS read the same way the cold path does. Without this, a
        // Re-analyze Media click on a warm-cache file would stream bytes off
        // the NAS with zero coordination with other media_analyze /
        // media_extract jobs — exactly the kind of unmanaged concurrent reader
        // the 2026-04-13 SMB credit-window fix exists to prevent.
        let media_root_ids = if used_spool {
            Vec::new()
        } else {
            media_roots::resolve_media_root_ids_for_file(
                &self.pool,
                &payload.media_type,
                payload.file_id,
            )
            .await
            .unwrap_or_default()
        };
        let io_permits = timings
            .time_async("wait_io_gate", self.io_gate.acquire_many(media_root_ids))
            .await;

        // Resolve where this file's content artefacts go (same logic as the
        // cold path). Supplement-only writes embeddings.bin + the content
        // manifest; analyze.rs's content_dir branch is unused here because
        // only_embed_src=true skips vhash / chromaprint / etc.
        let supplement_target = self
            .resolve_episode_cache_target(&payload.media_type, payload.file_id)
            .await?;
        let supplement_target_dir: Option<PathBuf> = supplement_target
            .as_ref()
            .map(|t| crate::media_cache::episode_cache::episode_cache_dir(&self.storage_root, t));

        let mut outcome = run_analysis(
            AnalyzeInputs {
                input_path: full_path,
                cache_dir,
                trickplay_dir,
                is_video: true,
                known_duration,
                want_embed_src: true,
                media_type: payload.media_type.clone(),
                // We update the warm manifest (not outcome.manifest) with
                // embedding info and write it ourselves at the end.
                write_manifest_on_success: false,
                only_embed_src: true,
                episode_cache_dir: supplement_target_dir.clone(),
                analyzed_file_id: Some(payload.file_id),
            },
            &cancel,
        )
        .await?;
        timings.record("single_pass", outcome.single_pass_duration);

        // Same handoff as the cold path — everything below is GPU + local
        // writes, so release the NAS gate now.
        drop(io_permits);

        let frames = std::mem::take(&mut outcome.embed_src_frames);
        let frame_count = frames.len();
        let mut branch_results = serde_json::Map::new();
        branch_results.insert(
            "embed_src".into(),
            json!({"ok": !frames.is_empty(), "count": frame_count}),
        );

        let embeddings_outcome = if frames.is_empty() {
            // Record a failed attempt so the supplement path doesn't spin
            // forever on a file that simply has no video in the credits region.
            manifest.artefacts.set(
                Branch::Embeddings,
                ArtefactInfo {
                    present: false,
                    params: embeddings_params_failure("embed_src produced no frames"),
                },
            );
            Err("embed_src produced no frames".to_string())
        } else {
            self.gpu_lock.set_enabled(config.gpu_preempt_tts);
            self.gpu_lock.set_service_urls(
                config.stt_service_url.clone(),
                config.tts_service_url.clone(),
                config.vision_service_url.clone(),
            );
            let gpu_guard = timings
                .time_async("wait_gpu_lock", self.gpu_lock.acquire_for_vision())
                .await;
            if gpu_guard.is_none() {
                nats_jobs::check_cancelled(&cancel)?;
            }
            let _gpu_guard = gpu_guard;
            // Embeddings + manifest go to the content target dir. When the
            // file isn't episode/movie (audiobooks etc.) the supplement
            // path can't run usefully — vision is video-only.
            match supplement_target_dir.as_deref() {
                Some(dir) => {
                    timings
                        .time_async(
                            "vision_call",
                            embed_frames_inline(
                                dir,
                                vision_url.as_deref(),
                                frames,
                                &mut manifest,
                                &cancel,
                            ),
                        )
                        .await
                }
                None => Err("no episode_cache target — supplement skipped".to_string()),
            }
        };

        match &embeddings_outcome {
            Ok(count) => {
                branch_results.insert("embeddings".into(), json!({"ok": true, "count": count}));
            }
            Err(e) => {
                branch_results.insert("embeddings".into(), json!({"ok": false, "error": e}));
            }
        }

        let manifest_started = Instant::now();
        if let Some(dir) = supplement_target_dir.as_deref() {
            if let Err(e) = crate::media_cache::writers::write_manifest_at(dir, &manifest) {
                warn!(error = %e, "supplement: episode_cache manifest write failed");
            }
        } else if let Err(e) = write_manifest(&self.storage_root, payload.file_id, &manifest) {
            warn!(error = %e, "supplement: per-file manifest write failed");
        }
        timings.record("post_decode_writes", manifest_started.elapsed());

        // Chain skip_detect regardless — audio + visual + (optionally) credit-text.
        let context = crate::jobs::JobContext::new("skip detect (post-supplement)");
        if let Err(e) = crate::jobs::enqueue_routine(
            &self.pool,
            &self.publisher,
            "skip_detect",
            json!({
                "file_id": payload.file_id,
                "media_type": payload.media_type,
                "run_id": run_id,
            }),
            context,
        )
        .await
        {
            warn!(error = %e, "Failed to enqueue skip_detect after supplement");
        }

        let duration_ms = started.elapsed().as_millis().min(i64::MAX as u128) as i64;
        let branch_results_json = Value::Object(branch_results.clone());
        if let Err(e) =
            media_cache_runs::finalize(&self.pool, run_id, duration_ms, &branch_results_json).await
        {
            warn!(error = %e, "supplement: media_cache_runs::finalize failed");
        }

        let total_ms = job_started.elapsed().as_millis().min(i64::MAX as u128) as i64;
        let mut timings_json = timings.to_json();
        if let Value::Object(map) = &mut timings_json {
            map.insert(
                "branches".to_string(),
                branch_durations_to_json(&outcome.branch_durations),
            );
        }
        info!(
            file_id = %payload.file_id,
            media_type = %payload.media_type,
            total_ms,
            timings = %timings_json,
            path = "supplement",
            "media_analyze timing"
        );

        Ok(json!({
            "status": "supplemented_embeddings",
            "file_id": payload.file_id,
            "run_id": run_id,
            "duration_ms": duration_ms,
            "embed_src_count": frame_count,
            "embeddings_ok": embeddings_outcome.is_ok(),
            "branch_results": branch_results_json,
        }))
    }
}

#[async_trait]
impl JobHandler for MediaAnalyzeHandler {
    fn job_type(&self) -> &'static str {
        "media_analyze"
    }

    fn max_concurrent(&self) -> usize {
        // Default 2: NAS reads are still serialised by `MediaRootIoGate` (cap
        // usually 1 on the QNAP TS-469 after the 2026-04-13 SMB credit-window
        // fix), but the handler drops its io permit as soon as `single_pass`
        // returns. A second slot lets the next job start its NAS decode while
        // the previous job is in `vision_call` + `post_decode_writes` (~14s of
        // pure GPU + local-disk work, measured disjoint from the NAS read
        // phase). No concurrent SMB load is introduced; only pipelining.
        // Raising past 2 just re-queues at the io_gate — pointless without
        // raising the per-root io cap too.
        self.runtime_config.job_concurrency("media_analyze", 2)
    }

    fn max_deliver(&self) -> Option<i64> {
        Some(2)
    }

    async fn execute(
        &self,
        job_id: Uuid,
        payload: Value,
        cancel: CancellationToken,
    ) -> JobResult<Value> {
        nats_jobs::check_cancelled(&cancel)?;

        let job_started = Instant::now();
        let mut timings = PhaseTimings::new();

        let payload: MediaAnalyzePayload = serde_json::from_value(payload)
            .map_err(|e| JobError::ExecutionFailed(format!("Invalid payload: {e}")))?;

        // See media_probe: one pass over a given file at a time, queued rather
        // than dropped.
        let _file_lock = crate::jobs::file_lock::lock_file(payload.file_id).await;

        if payload.gaps_only {
            let area = self.check_picture(&payload).await;
            return Ok(
                json!({ "status": "gaps_filled", "file_id": payload.file_id, "barred": area.is_some() }),
            );
        }

        let _permit = self
            .enrichment_semaphore
            .acquire()
            .await
            .map_err(|_| JobError::ExecutionFailed("Enrichment semaphore closed".into()))?;

        let triggered_by = match payload.triggered_by.as_str() {
            "auto_ingest" => TriggeredBy::AutoIngest,
            "backfill" => TriggeredBy::Backfill,
            "manual" => TriggeredBy::Manual,
            other => {
                warn!(triggered_by = %other, "Unknown triggered_by — defaulting to Manual");
                TriggeredBy::Manual
            }
        };

        let is_video = matches!(payload.media_type.as_str(), "episode" | "movie");

        info!(
            job_id = %job_id,
            file_id = %payload.file_id,
            media_type = %payload.media_type,
            triggered_by = triggered_by.as_str(),
            "Starting media_analyze"
        );

        // ── 1. Resolve source file path + known duration ─────────────────────
        // `known_duration` is used to narrow vhash/embed_src to the V1-equivalent
        // windows. When the DB row has no duration (fresh ingest), fall back to
        // whole-file extraction — safer than guessing.
        let (media_root_path, known_duration, episode_cache_target) =
            match payload.media_type.as_str() {
                "movie" => {
                    let row = movie_files::get(&self.pool, payload.file_id)
                        .await
                        .map_err(|e| JobError::ExecutionFailed(format!("DB error: {e}")))?
                        .ok_or_else(|| {
                            JobError::ExecutionFailed(format!(
                                "Movie file not found: {}",
                                payload.file_id
                            ))
                        })?;
                    let target = crate::media_cache::episode_cache::MediaKind::Movie {
                        movie_id: row.movie_id,
                    };
                    (row.file_path, row.duration_seconds, Some(target))
                }
                "episode" => {
                    let row = tv_episode_files::get(&self.pool, payload.file_id)
                        .await
                        .map_err(|e| JobError::ExecutionFailed(format!("DB error: {e}")))?
                        .ok_or_else(|| {
                            JobError::ExecutionFailed(format!(
                                "Episode file not found: {}",
                                payload.file_id
                            ))
                        })?;
                    let target =
                        crate::media_cache::episode_cache::MediaKind::TvFile { file_id: row.id };
                    (row.file_path, row.duration_seconds, Some(target))
                }
                "audiobook" => (
                    sqlx::query_scalar!(
                        "SELECT file_path FROM audiobook_files WHERE id = $1",
                        payload.file_id,
                    )
                    .fetch_optional(&self.pool)
                    .await
                    .map_err(|e| JobError::ExecutionFailed(format!("DB error: {e}")))?
                    .ok_or_else(|| {
                        JobError::ExecutionFailed(format!(
                            "Audiobook file not found: {}",
                            payload.file_id
                        ))
                    })?,
                    None,
                    // Audiobooks don't get cross-episode skip-detect — only
                    // the per-file audio_peaks in media_cache. No
                    // episode_cache target.
                    None,
                ),
                other => {
                    return Err(JobError::ExecutionFailed(format!(
                        "Unknown media type: {other}"
                    )));
                }
            };

        // Prefer local spool when present (Phase 5+ dual-write tee).
        let spool_path = media_cache::source_spool_path(&self.storage_root, payload.file_id);
        let (full_path, used_spool) = {
            let sp = spool_path.clone();
            let exists = tokio::task::spawn_blocking(move || sp.is_file())
                .await
                .unwrap_or(false);
            if exists {
                (spool_path, true)
            } else {
                (PathBuf::from(&media_root_path), false)
            }
        };

        let is_file = {
            let fp = full_path.clone();
            tokio::task::spawn_blocking(move || fp.is_file())
                .await
                .unwrap_or(false)
        };
        if !is_file {
            return Err(JobError::ExecutionFailed(format!(
                "File not found on disk: {}",
                full_path.display()
            )));
        }

        // ── 2. Short-circuit on existing manifest if no force_regen ───────────
        // Per-media-type branch matrix. Audio_peaks only matters for the
        // audiobook waveform UI (`serve_audio_peaks`, `enrich_audiobook.rs`)
        // — no video consumer reads it, so don't pay the disk + decode cost
        // on video files. Vhash / trickplay / embed_src / black_frames have
        // no audiobook consumer.
        let needed_branches: Vec<Branch> = if is_video {
            vec![
                Branch::Trickplay,
                Branch::Vhash,
                Branch::Chromaprint,
                Branch::Silence,
                Branch::Loudness,
                Branch::BlackFrames,
            ]
        } else {
            vec![
                Branch::Chromaprint,
                Branch::Silence,
                Branch::Loudness,
                Branch::AudioPeaks,
            ]
        };

        // Post-509cc95a: trickplay + audio_peaks live in the per-file manifest
        // (`media_cache/{file_id}/`); content branches (vhash, chromaprint,
        // silence, loudness, black_frames, embeddings) live in the
        // episode_cache content manifest for video. Audiobooks have no
        // episode_cache target — `analyze.rs::merge_content_into_per_file`
        // folds everything back into the per-file manifest.
        let per_file_manifest =
            media_cache::readers::read_manifest(&self.storage_root, payload.file_id);
        let content_manifest = episode_cache_target
            .as_ref()
            .and_then(|t| crate::media_cache::episode_cache::read_manifest(&self.storage_root, t));

        // Pick whichever manifest the supplement path will mutate + write back
        // to: episode_cache for video (target Some), per-file for audiobooks
        // (target None).
        let warm_manifest_for_supplement = if episode_cache_target.is_some() {
            content_manifest.clone()
        } else {
            per_file_manifest.clone()
        };

        let branch_info = |b: Branch| -> Option<&ArtefactInfo> {
            let m = match b {
                // File-bound branches always live in the per-file manifest.
                Branch::Trickplay | Branch::AudioPeaks => per_file_manifest.as_ref(),
                // Content branches live in episode_cache for video; for
                // audiobooks they're merged into the per-file manifest.
                _ => {
                    if episode_cache_target.is_some() {
                        content_manifest.as_ref()
                    } else {
                        per_file_manifest.as_ref()
                    }
                }
            };
            m.and_then(|m| match b {
                Branch::Trickplay => m.artefacts.trickplay.as_ref(),
                Branch::Vhash => m.artefacts.vhash.as_ref(),
                Branch::Chromaprint => m.artefacts.chromaprint.as_ref(),
                Branch::Silence => m.artefacts.silence.as_ref(),
                Branch::Loudness => m.artefacts.loudness.as_ref(),
                Branch::AudioPeaks => m.artefacts.audio_peaks.as_ref(),
                Branch::Embeddings => m.artefacts.embeddings.as_ref(),
                Branch::BlackFrames => m.artefacts.black_frames.as_ref(),
            })
        };
        let branch_present =
            |b: Branch| -> bool { branch_info(b).map(|i| i.present).unwrap_or(false) };

        // Both manifests we'll consult must be at the current pipeline_version.
        // For video: per-file (trickplay) + content (everything else). For
        // audiobook: just per-file.
        let per_file_pv_ok = per_file_manifest
            .as_ref()
            .map(|m| m.pipeline_version == PIPELINE_VERSION)
            .unwrap_or(false);
        let content_pv_ok = if episode_cache_target.is_some() {
            content_manifest
                .as_ref()
                .map(|m| m.pipeline_version == PIPELINE_VERSION)
                .unwrap_or(false)
        } else {
            true
        };

        // A negative DB media_cache_version means a same-path file replace
        // invalidated the caches — regenerate even if the on-disk manifest's
        // pipeline_version still matches, otherwise we'd short-circuit on the
        // previous copy's thumbnails / LUFS / skip data.
        let cache_invalidated =
            db_media_cache_invalidated(&self.pool, &payload.media_type, payload.file_id).await;

        if !payload.force_regen
            && !cache_invalidated
            && per_file_pv_ok
            && content_pv_ok
            && needed_branches.iter().all(|&b| branch_present(b))
        {
            // Vision may have been configured AFTER the first analyze — in that
            // case the cache covers everything except `embeddings`. Run a
            // light-weight supplementation pass (video-only decode, embed_src
            // branch only) to fill it in, without re-running chromaprint /
            // vhash / trickplay / etc.
            //
            // Guard: `needed_branches` doesn't include Embeddings, so we're
            // here even when it's missing. Only attempt when vision is
            // configured NOW — otherwise there's nothing to supplement, and
            // repeated clicks would no-op (no loop).
            let embeddings_missing = !branch_present(Branch::Embeddings);
            let config_for_supplement = self.runtime_config.get().await;
            let supplement_vision_url = config_for_supplement.effective_vision_url();
            let can_supplement_vision = supplement_vision_url
                .as_deref()
                .is_some_and(|s| !s.is_empty());

            if is_video && embeddings_missing && can_supplement_vision {
                info!(
                    file_id = %payload.file_id,
                    "Warm cache missing embeddings; running supplementation pass"
                );
                // Safe to unwrap: needed_branches all passed branch_present,
                // which requires the appropriate manifest to be Some — for
                // video files that's the content manifest.
                let supplement_manifest = warm_manifest_for_supplement
                    .expect("content manifest must be Some when video branches are present");
                return self
                    .run_embeddings_supplement(
                        payload,
                        full_path,
                        used_spool,
                        known_duration,
                        supplement_manifest,
                        triggered_by,
                        &config_for_supplement,
                        supplement_vision_url,
                        cancel,
                    )
                    .await;
            }

            info!(
                file_id = %payload.file_id,
                "All requested branches present and pipeline_version matches — skipping decode"
            );
            // Create/finalize a run row so the UI sees the click reflected.
            let short_run_id = match payload.run_id {
                Some(id) => {
                    if let Err(e) = media_cache_runs::start_run_with_id(
                        &self.pool,
                        id,
                        payload.file_id,
                        &payload.media_type,
                        triggered_by,
                        PIPELINE_VERSION,
                    )
                    .await
                    {
                        warn!(error = %e, run_id = %id, "warm cache: start_run_with_id failed");
                    }
                    Some(id)
                }
                None => media_cache_runs::start_run(
                    &self.pool,
                    payload.file_id,
                    &payload.media_type,
                    triggered_by,
                    PIPELINE_VERSION,
                )
                .await
                .ok(),
            };
            if let Some(id) = short_run_id {
                let _ =
                    media_cache_runs::finalize(&self.pool, id, 0, &json!({"skipped_decode": true}))
                        .await;
            }

            // Warm cache ≠ no detection. Re-run V2 against the existing cache.
            // If embeddings.bin is already present, skip straight to skip_detect;
            // otherwise fall back to the audio/visual path (no credit-text).
            if let Some(run_id) = short_run_id {
                let context = crate::jobs::JobContext::new("skip detect (warm cache rerun)");
                if let Err(e) = crate::jobs::enqueue_routine(
                    &self.pool,
                    &self.publisher,
                    "skip_detect",
                    json!({
                        "file_id": payload.file_id,
                        "media_type": payload.media_type,
                        "run_id": run_id,
                    }),
                    context,
                )
                .await
                {
                    warn!(error = %e, "Failed to enqueue skip_detect (warm cache)");
                }
            }

            // Emit one timing line per file even on the warm-cache path so
            // the jq pipeline never has gaps. Single bucket because there's
            // no real work to break down.
            timings.record("warm_cache_skip", job_started.elapsed());
            info!(
                file_id = %payload.file_id,
                media_type = %payload.media_type,
                total_ms = job_started.elapsed().as_millis().min(i64::MAX as u128) as i64,
                timings = %timings.to_json(),
                path = "warm_cache",
                "media_analyze timing"
            );

            if is_video {
                self.check_picture(&payload).await;
            }

            return Ok(json!({
                "status": "skipped_decode",
                "reason": "manifest_up_to_date",
                "file_id": payload.file_id,
                "skip_detect_enqueued": short_run_id.is_some(),
            }));
        }

        // ── 3. Ensure cache dirs exist ───────────────────────────────────────
        media_cache::ensure_dirs(&self.storage_root, payload.file_id)
            .map_err(|e| JobError::ExecutionFailed(format!("ensure_dirs failed: {e}")))?;

        // ── 4. NAS IO gate (only when reading from media root, not the spool) ─
        let media_root_ids = if used_spool {
            Vec::new()
        } else {
            media_roots::resolve_media_root_ids_for_file(
                &self.pool,
                &payload.media_type,
                payload.file_id,
            )
            .await
            .unwrap_or_default()
        };
        let io_permits = timings
            .time_async("wait_io_gate", self.io_gate.acquire_many(media_root_ids))
            .await;

        // ── 5. Insert run row ────────────────────────────────────────────────
        // Caller-provided id (UI-generated for manual clicks) goes through
        // start_run_with_id with ON CONFLICT DO NOTHING so finalize finds a
        // row to UPDATE later. Without this, the manual flow leaves
        // media_cache_runs empty and the UI run-history panel stays blank.
        let run_id = match payload.run_id {
            Some(id) => {
                if let Err(e) = media_cache_runs::start_run_with_id(
                    &self.pool,
                    id,
                    payload.file_id,
                    &payload.media_type,
                    triggered_by,
                    PIPELINE_VERSION,
                )
                .await
                {
                    warn!(error = %e, run_id = %id, "cold path: start_run_with_id failed");
                }
                id
            }
            None => media_cache_runs::start_run(
                &self.pool,
                payload.file_id,
                &payload.media_type,
                triggered_by,
                PIPELINE_VERSION,
            )
            .await
            .map_err(|e| JobError::ExecutionFailed(format!("start_run failed: {e}")))?,
        };

        nats_jobs::check_cancelled(&cancel)?;

        // ── 6. Run the single-pass primitive ─────────────────────────────────
        let config = self.runtime_config.get().await;
        let vision_url = config.effective_vision_url();
        let want_embed_src = vision_url.as_deref().is_some_and(|s| !s.is_empty());
        let trickplay_dir = media_cache::trickplay_dir(&self.storage_root, payload.file_id);
        let cache_dir = media_cache::cache_dir(&self.storage_root, payload.file_id);

        // Resolve where this file's content artefacts go.
        let content_target_dir: Option<PathBuf> = episode_cache_target
            .as_ref()
            .map(|t| crate::media_cache::episode_cache::episode_cache_dir(&self.storage_root, t));

        // Snapshot the source before decoding so the result can be checked
        // against it afterwards.
        //
        // A file still being written -- a download reported complete early, or
        // a retry that re-creates it -- otherwise yields a cache and a metadata
        // row describing bytes that no longer exist, and nothing ever looks
        // again. Three files sat that way for two months: analyzed while
        // incomplete, re-created minutes later by a retry that never finished,
        // and left as full-size 95%-zero shells behind good-looking rows.
        let stamp_before = source_stamp_async(&full_path).await;

        let started = Instant::now();
        let mut outcome = run_analysis(
            AnalyzeInputs {
                input_path: full_path.clone(),
                cache_dir: cache_dir.clone(),
                trickplay_dir: trickplay_dir.clone(),
                is_video,
                known_duration,
                want_embed_src,
                media_type: payload.media_type.clone(),
                // Handler writes the (content) manifest itself after the
                // embeddings step. analyze.rs still writes the per-file
                // manifest at cache_dir for trickplay+audio_peaks coverage.
                write_manifest_on_success: false,
                only_embed_src: false,
                episode_cache_dir: content_target_dir.clone(),
                analyzed_file_id: Some(payload.file_id),
            },
            &cancel,
        )
        .await?;

        // Refuse to publish a cache for a file that changed under us. Retryable
        // rather than fatal: once the writer settles, the next attempt sees a
        // stable file and succeeds.
        //
        // A moved mtime is a hint, not the answer. Nothing writes to the file:
        // the media sits on a CIFS mount that stamps `LastWriteTime` when the
        // writing handle closes, and ingest used to leave the download handle
        // open for seconds after declaring the file complete, so on an
        // auto-ingested file the mtime moved a second or so after this handler
        // snapshotted it, with the bytes never having changed. Discarding on
        // that alone threw away a completed multi-minute decode and left the
        // file sitting until the half-hourly backfill sweep noticed it.
        //
        // `finalize_download` in the kobload ingest closes the handle before the
        // rename now, which should settle the mtime before this job ever stats
        // the file. This stays regardless: any writer that closes late, or any
        // mount with the same close semantics, reproduces it.
        //
        // What this guard is really defending against is the re-created shell:
        // same size, rewritten as mostly zeros by a retry that never finished.
        // `count_zero_blocks` names that case directly and is already measured
        // against this library -- 150 healthy files scored 0, the three known-bad
        // ones scored 9, 12 and 5 of 12 -- so a moved mtime escalates to a
        // content sample instead of deciding by itself. Note a head/tail hash is
        // *not* the check to reach for here: it cost six false positives on this
        // same library, which is why the sampler skips the container's edges.
        let mut verified_stamp = stamp_before;
        if let Some(before) = stamp_before {
            let after = source_stamp_async(&full_path).await;
            // Sample only when the cheap signals are ambiguous -- same size,
            // moved mtime. Identical stamps need no read at all, and a changed
            // size is already decided.
            //
            // Below MIN_SAMPLE_SIZE `count_zero_blocks` reports a clean Some(0)
            // without reading anything, because there is not enough middle to
            // sample. Taking that as evidence of health would wave through every
            // small file on a signal nobody checked, so leave such a file to the
            // strict path: the whole reason to be lenient is to protect an
            // expensive decode, and there is no expensive decode to protect
            // under eight megabytes.
            let zero_blocks = match after {
                Some((size, mtime))
                    if size == before.0
                        && mtime != before.1
                        && size as u64 >= crate::media_cache::integrity::MIN_SAMPLE_SIZE =>
                {
                    crate::media_cache::integrity::count_zero_blocks_async(&full_path, size as u64)
                        .await
                }
                _ => None,
            };
            match source_verdict(before, after, zero_blocks) {
                SourceVerdict::Unchanged => {}
                SourceVerdict::TouchedButIntact => {
                    info!(
                        file_id = %payload.file_id,
                        path = %full_path.display(),
                        before = ?before,
                        after = ?after,
                        "media_analyze: source mtime moved during analysis but the content \
                         sampled clean — keeping this pass"
                    );
                    // Record what the file is *now*, not what it was at snapshot
                    // time. The manifest stamp is what `media_integrity_check`
                    // tier 1 compares against, so carrying the pre-touch mtime
                    // forward would trade this bug for a permanent StampMismatch
                    // on every file this branch rescues.
                    verified_stamp = after;
                }
                SourceVerdict::Changed => {
                    warn!(
                        file_id = %payload.file_id,
                        path = %full_path.display(),
                        before = ?before,
                        after = ?after,
                        zero_blocks = ?zero_blocks,
                        "media_analyze: source changed while being analyzed — discarding this pass"
                    );
                    return Err(JobError::Retryable(
                        "source file changed during analysis".to_string(),
                    ));
                }
            }
        }

        timings.record("single_pass", outcome.single_pass_duration);

        // Release the NAS io permit now that decoding is done. Everything
        // below this point is GPU (vision service) + local cache writes —
        // lets the next media_analyze job acquire the gate and start its
        // decode while we run ~14s of non-NAS work.
        drop(io_permits);

        // Write the DB-side lufs column from the loudness branch's result so
        // the player's audio normalization reads the current value without a
        // manifest roundtrip. Replaces the legacy `measure_lufs` handler's
        // write path.
        if let Some(loud) = outcome.manifest.artefacts.loudness.as_ref()
            && let Some(lufs) = loud.params.get("integrated_lufs").and_then(|v| v.as_f64())
            && let Err(e) =
                update_lufs_column(&self.pool, &payload.media_type, payload.file_id, lufs).await
        {
            warn!(error = %e, "lufs DB update failed");
        }

        let mut branch_results = outcome.branch_results;
        let input_duration = outcome.input_duration_secs;

        // ── 7. Embed_src → vision embeddings, in memory, no disk round-trip.
        // Embeddings are content-bound: lands in episode_cache_dir alongside
        // vhash et al. When no target is set (audiobooks), embed_src is also
        // off — `embed_src_ok` will be false and this block is skipped.
        let embed_src_ok = !outcome.embed_src_frames.is_empty()
            && !outcome
                .per_branch_errors
                .iter()
                .any(|(id, _)| matches!(id, BranchId::EmbedSrc));
        let embeddings_ok = if embed_src_ok && let Some(dir) = content_target_dir.as_deref() {
            let frames = std::mem::take(&mut outcome.embed_src_frames);
            self.gpu_lock.set_enabled(config.gpu_preempt_tts);
            self.gpu_lock.set_service_urls(
                config.stt_service_url.clone(),
                config.tts_service_url.clone(),
                config.vision_service_url.clone(),
            );
            let gpu_guard = timings
                .time_async("wait_gpu_lock", self.gpu_lock.acquire_for_vision())
                .await;
            if gpu_guard.is_none() {
                nats_jobs::check_cancelled(&cancel)?;
            }
            let _gpu_guard = gpu_guard;
            match timings
                .time_async(
                    "vision_call",
                    embed_frames_inline(
                        dir,
                        vision_url.as_deref(),
                        frames,
                        &mut outcome.manifest,
                        &cancel,
                    ),
                )
                .await
            {
                Ok(count) => {
                    branch_results.insert("embeddings".into(), json!({"ok": true, "count": count}));
                    true
                }
                Err(e) => {
                    warn!(error = %e, "inline embeddings failed — skip_detect will run audio-only");
                    branch_results.insert("embeddings".into(), json!({"ok": false, "error": e}));
                    false
                }
            }
        } else {
            false
        };

        // ── 8. Write content manifest to episode_cache_dir (if any).
        // The per-file manifest (trickplay + audio_peaks) was already written
        // by analyze.rs into cache_dir; nothing for us to do there.
        // Record what was actually read, so a later integrity pass can tell
        // whether this cache still describes the file on disk.
        if let Some((size, mtime)) = verified_stamp {
            outcome.manifest.input_size_bytes = Some(size);
            outcome.manifest.input_mtime = Some(mtime);
        }

        let manifest_started = Instant::now();
        if let Some(dir) = content_target_dir.as_deref() {
            if let Err(e) = crate::media_cache::writers::write_manifest_at(dir, &outcome.manifest) {
                warn!(error = %e, "episode_cache manifest write failed");
            }
        } else {
            // No target — outcome.manifest IS the per-file manifest.
            // analyze.rs didn't write it (write_manifest_on_success = false),
            // so do it now.
            if let Err(e) = write_manifest(&self.storage_root, payload.file_id, &outcome.manifest) {
                warn!(error = %e, "per-file manifest write failed");
            }
        }
        timings.record(
            "post_decode_writes",
            outcome.post_decode_duration + manifest_started.elapsed(),
        );

        // ── 9. Bump the file row's media_cache_version so the backfill
        // walker stops re-queuing this file on every tick. We bump even if
        // some branches errored — the manifest still records what landed,
        // and a partial cache is better than perpetually re-running.
        //
        // Must land before the skip_detect enqueue below: that handler's
        // season staleness gate reads `media_cache_generated_at`, which this
        // stamps. Enqueue first and the job can win the race, see a cache-less
        // season, and skip it — and the per-season dedupe means nothing
        // re-enqueues it.
        if let Err(e) =
            bump_media_cache_version(&self.pool, &payload.media_type, payload.file_id).await
        {
            warn!(error = %e, "media_cache_version bump failed");
        }

        // ── 10. Chain skip_detect ────────────────────────────────────────
        // Embeddings (if wanted) are already durable on disk at this point, so
        // skip_detect can read everything it needs from the cache.
        // Before skip_detect, which is what finds the intro and credits the check
        // would leave out: a re-analyze picks those up.
        if is_video {
            self.check_picture(&payload).await;
        }

        {
            let label = if embeddings_ok {
                "skip detect (audio + visual + credit-text)"
            } else if want_embed_src {
                "skip detect (embeddings failed, audio + visual only)"
            } else {
                "skip detect (no-vision mode)"
            };
            // Detection is season-scoped: one job matches every episode in the
            // season against every other and persists the lot. Queueing one per
            // file therefore recomputes the whole season once per episode it
            // contains — measured at 15x on a full-library re-run. Carry the
            // season in the payload and skip the enqueue when a job for it is
            // already in flight. The staleness gate in the handler catches the
            // rest, including siblings that finish after this check.
            let season_id = if payload.media_type == "episode" {
                crate::repositories::tv_episode_files::season_id_for_file(
                    &self.pool,
                    payload.file_id,
                )
                .await
                .unwrap_or(None)
            } else {
                None
            };

            let already_queued = match season_id {
                Some(sid) => crate::jobs::has_active_job(
                    &self.pool,
                    "skip_detect",
                    "season_id",
                    &sid.to_string(),
                )
                .await
                .unwrap_or(false),
                None => false,
            };

            if already_queued {
                debug!(
                    file_id = %payload.file_id,
                    season_id = ?season_id,
                    "skip_detect already queued for this season — not enqueueing again"
                );
            } else {
                let context = crate::jobs::JobContext::new(label);
                if let Err(e) = crate::jobs::enqueue_routine(
                    &self.pool,
                    &self.publisher,
                    "skip_detect",
                    json!({
                        "file_id": payload.file_id,
                        "media_type": payload.media_type,
                        "run_id": run_id,
                        "season_id": season_id,
                    }),
                    context,
                )
                .await
                {
                    warn!(error = %e, "Failed to enqueue skip_detect");
                }
            }
        }

        // ── 10b. Persist corrupt_ranges so the streaming layer can spawn
        // overlay transcode generations for the affected windows on next play.
        let corrupt_ranges = std::mem::take(&mut outcome.corrupt_ranges);
        match payload.media_type.as_str() {
            "episode" => {
                if let Err(e) = tv_episode_files::set_corrupt_ranges(
                    &self.pool,
                    payload.file_id,
                    corrupt_ranges,
                )
                .await
                {
                    warn!(error = %e, file_id = %payload.file_id, "set_corrupt_ranges (episode) failed");
                }
            }
            "movie" => {
                if let Err(e) =
                    movie_files::set_corrupt_ranges(&self.pool, payload.file_id, corrupt_ranges)
                        .await
                {
                    warn!(error = %e, file_id = %payload.file_id, "set_corrupt_ranges (movie) failed");
                }
            }
            _ => {
                // audiobook / recording / debrid — corrupt_ranges either not
                // tracked on these tables or persistence handled elsewhere.
            }
        }

        // ── 11. Finalize the run row ─────────────────────────────────────────
        let duration_ms = started.elapsed().as_millis().min(i64::MAX as u128) as i64;
        let branch_results_json = Value::Object(branch_results.clone());
        if let Err(e) =
            media_cache_runs::finalize(&self.pool, run_id, duration_ms, &branch_results_json).await
        {
            warn!(error = %e, "media_cache_runs::finalize failed");
        }

        let total_ms = job_started.elapsed().as_millis().min(i64::MAX as u128) as i64;
        let mut timings_json = timings.to_json();
        if let Value::Object(map) = &mut timings_json {
            map.insert(
                "branches".to_string(),
                branch_durations_to_json(&outcome.branch_durations),
            );
        }
        info!(
            file_id = %payload.file_id,
            media_type = %payload.media_type,
            total_ms,
            timings = %timings_json,
            path = "cold",
            "media_analyze timing"
        );

        info!(
            file_id = %payload.file_id,
            duration_ms,
            duration_secs = input_duration,
            errors = outcome.per_branch_errors.len(),
            "media_analyze complete"
        );

        Ok(json!({
            "file_id": payload.file_id,
            "media_type": payload.media_type,
            "run_id": run_id,
            "duration_ms": duration_ms,
            "input_duration_secs": input_duration,
            "branch_results": branch_results_json,
        }))
    }
}

/// Mirror the loudness branch's LUFS value into the file row so the streaming
/// session response can serve it without a manifest roundtrip. Replaces what
/// the legacy `measure_lufs` handler did. Audiobooks are written by
/// `enrich_audiobook` separately.
async fn update_lufs_column(
    pool: &PgPool,
    media_type: &str,
    file_id: uuid::Uuid,
    lufs: f64,
) -> Result<(), sqlx::Error> {
    match media_type {
        "movie" => {
            sqlx::query!(
                "UPDATE movie_files SET lufs = $2 WHERE id = $1",
                file_id,
                lufs,
            )
            .execute(pool)
            .await?;
        }
        "episode" => {
            sqlx::query!(
                "UPDATE tv_episode_files SET lufs = $2 WHERE id = $1",
                file_id,
                lufs,
            )
            .execute(pool)
            .await?;
        }
        _ => {}
    }
    Ok(())
}

/// Send in-memory JPEG frames to the vision service, pack to fp16, and write
/// `embeddings.bin` + patch the manifest. Replaces the previous disk-spool +
/// `generate_embeddings` job hop.
///
/// On success: returns `Ok(count)` and the manifest carries `present: true`
/// with `start_time_secs` so `EmbeddingsView::window` can index correctly.
///
/// On failure (vision down, zero frames returned, etc.): returns `Err(reason)`
/// AND records `present: false` in the manifest with `attempted_at` +
/// `error`. The `attempted_at` field is what distinguishes "never ran"
/// (no manifest entry at all) from "ran and produced nothing" in the
/// supplementation gate — critical for avoiding endless reruns when vision
/// is off but skip_detect still enqueues normally.
async fn embed_frames_inline(
    target_dir: &Path,
    vision_url: Option<&str>,
    frames: Vec<kob_ffmpeg::thumbnails::JpegFrame>,
    manifest: &mut crate::media_cache::writers::Manifest,
    cancel: &CancellationToken,
) -> Result<usize, String> {
    let start_time_secs = frames.first().map(|f| f.timestamp_secs).unwrap_or(0.0);
    let result = do_embed_frames(target_dir, vision_url, frames, cancel).await;
    match &result {
        Ok(count) => {
            manifest.artefacts.set(
                Branch::Embeddings,
                ArtefactInfo {
                    present: true,
                    params: embeddings_params_success(*count, start_time_secs),
                },
            );
        }
        Err(reason) => {
            manifest.artefacts.set(
                Branch::Embeddings,
                ArtefactInfo {
                    present: false,
                    params: embeddings_params_failure(reason),
                },
            );
        }
    }
    result
}

async fn do_embed_frames(
    target_dir: &Path,
    vision_url: Option<&str>,
    frames: Vec<kob_ffmpeg::thumbnails::JpegFrame>,
    cancel: &CancellationToken,
) -> Result<usize, String> {
    let vision_url = vision_url
        .filter(|u| !u.is_empty())
        .ok_or_else(|| "vision_service_url not configured".to_string())?;

    let client = VisionClient::new(vision_url);
    if !client.health().await {
        return Err("vision service not healthy".to_string());
    }

    // VisionClient::embed_images batches internally at VISION_BATCH_SIZE (=16).
    // We still chunk here so a cancel mid-file aborts promptly between batches
    // rather than only between files.
    let mut jpegs: Vec<Vec<u8>> = frames.into_iter().map(|f| f.jpeg_data).collect();
    let total = jpegs.len();
    let mut all_embeddings: Vec<Vec<f32>> = Vec::with_capacity(total);
    let batch = 16;
    let mut start = 0;
    while start < jpegs.len() {
        nats_jobs::check_cancelled(cancel).map_err(|e| format!("cancelled: {e}"))?;
        let end = (start + batch).min(jpegs.len());
        let chunk: Vec<Vec<u8>> = jpegs[start..end].to_vec();
        let chunk_embeddings = client
            .embed_images(&chunk)
            .await
            .map_err(|e| format!("vision embed failed at batch {start}..{end}: {e}"))?;
        all_embeddings.extend(chunk_embeddings);
        start = end;
    }
    jpegs.clear();

    if all_embeddings.is_empty() {
        return Err("vision service returned no embeddings".to_string());
    }
    let dim = all_embeddings[0].len();
    if dim != EMBED_DIM {
        warn!(
            got = dim,
            expected = EMBED_DIM,
            "embedding dim mismatch — recording actual value in manifest"
        );
    }

    l2_normalise(&mut all_embeddings);
    let packed = pack_fp16(&all_embeddings);

    let target_dir = target_dir.to_path_buf();
    tokio::task::spawn_blocking(move || {
        let sidecar = target_dir.join(
            crate::media_cache::sidecar_filename(Branch::Embeddings)
                .expect("embeddings is a file branch"),
        );
        write_atomic(&sidecar, &packed)
    })
    .await
    .map_err(|e| format!("write task panicked: {e}"))?
    .map_err(|e| format!("embeddings write failed: {e}"))?;

    Ok(all_embeddings.len())
}

/// Seconds-since-UNIX-epoch as f64; stored in the manifest so readers can
/// distinguish "never ran" (no attempted_at) from "ran but empty".
fn now_unix_secs() -> f64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs_f64())
        .unwrap_or(0.0)
}

fn embeddings_params_success(count: usize, start_time_secs: f64) -> serde_json::Map<String, Value> {
    let mut params = serde_json::Map::new();
    params.insert("dim".into(), json!(EMBED_DIM));
    params.insert("count".into(), json!(count));
    params.insert("fps".into(), json!(EMBED_FPS));
    params.insert("dtype".into(), json!("f16"));
    params.insert("model".into(), json!(EMBED_MODEL));
    params.insert("width".into(), json!(EMBED_WIDTH));
    params.insert("start_time_secs".into(), json!(start_time_secs));
    params.insert("attempted_at".into(), json!(now_unix_secs()));
    params
}

fn embeddings_params_failure(reason: &str) -> serde_json::Map<String, Value> {
    let mut params = serde_json::Map::new();
    params.insert("attempted_at".into(), json!(now_unix_secs()));
    params.insert("error".into(), json!(reason));
    params
}

/// A negative `media_cache_version` is the sentinel `create_or_update` writes
/// when a same-path content replace invalidates the on-disk caches. It is kept
/// distinct from the default `0` (legacy / never-analyzed files) so this gate
/// forces a fresh decode only for genuinely-replaced files — not a mass
/// re-decode of every pre-versioning file that already has a valid manifest.
/// Defaults to "not invalidated" on a lookup miss / unknown type.
async fn db_media_cache_invalidated(pool: &PgPool, media_type: &str, file_id: uuid::Uuid) -> bool {
    let v = match media_type {
        "movie" => movie_files::get_media_cache_version(pool, file_id).await,
        "episode" => tv_episode_files::get_media_cache_version(pool, file_id).await,
        "audiobook" => audiobook_files::get_media_cache_version(pool, file_id).await,
        _ => return false,
    };
    matches!(v, Ok(Some(version)) if version < 0)
}

/// What the post-analysis source check concluded about the file just decoded.
#[derive(Debug, PartialEq)]
enum SourceVerdict {
    /// Byte-for-byte the file that was snapshotted. Publish as-is.
    Unchanged,
    /// The mtime moved but the size held and the content sampled clean — the
    /// benign touch ingest applies as it finishes. Publish against the new
    /// stamp.
    TouchedButIntact,
    /// A different file, one that has gone away, or one that now reads as a
    /// partial shell. Discard the pass.
    Changed,
}

/// Decide whether a completed pass may be published.
///
/// `zero_blocks` is [`count_zero_blocks`](crate::media_cache::integrity) for the
/// file as it stands now. It is only meaningful when the size held, which is the
/// only case the caller samples; otherwise it is `None` and the size decides.
/// `None` on a same-size file means no sample vouched for the content -- it
/// could not be read, or the file was too small to sample -- which is not
/// evidence of health and so does not rescue the pass.
fn source_verdict(
    before: (i64, chrono::DateTime<chrono::Utc>),
    after: Option<(i64, chrono::DateTime<chrono::Utc>)>,
    zero_blocks: Option<u64>,
) -> SourceVerdict {
    if after == Some(before) {
        return SourceVerdict::Unchanged;
    }
    match after {
        Some((size, _)) if size == before.0 && zero_blocks == Some(0) => {
            SourceVerdict::TouchedButIntact
        }
        _ => SourceVerdict::Changed,
    }
}

/// `source_stamp` off the runtime: the media lives on a network mount, so even
/// a bare `stat` can block for as long as the link takes to answer.
async fn source_stamp_async(path: &Path) -> Option<(i64, chrono::DateTime<chrono::Utc>)> {
    let p = path.to_path_buf();
    tokio::task::spawn_blocking(move || crate::media_cache::writers::source_stamp(&p))
        .await
        .unwrap_or(None)
}

async fn bump_media_cache_version(
    pool: &PgPool,
    media_type: &str,
    file_id: uuid::Uuid,
) -> Result<(), sqlx::Error> {
    match media_type {
        "movie" => movie_files::set_media_cache_version(pool, file_id, PIPELINE_VERSION).await,
        "episode" => {
            tv_episode_files::set_media_cache_version(pool, file_id, PIPELINE_VERSION).await
        }
        "audiobook" => {
            audiobook_files::set_media_cache_version(pool, file_id, PIPELINE_VERSION).await
        }
        other => {
            warn!(
                media_type = other,
                "unknown media_type — skipping version bump"
            );
            Ok(())
        }
    }
    .map_err(|e| match e {
        crate::repositories::RepositoryError::Database(db) => db,
        other => sqlx::Error::Protocol(other.to_string()),
    })
}

/// Convert koblibs' per-`BranchId` wall-clock map into a JSON object keyed by
/// snake_case branch name (`{vhash_ms: 1234, chromaprint_ms: 567, ...}`).
/// Inserted under the `branches` key inside the `timings` field of the
/// `media_analyze timing` log line so it's jq-able alongside phase timings.
fn branch_durations_to_json(
    durations: &std::collections::HashMap<BranchId, std::time::Duration>,
) -> Value {
    let mut map = serde_json::Map::with_capacity(durations.len());
    for (id, dur) in durations {
        let key = match id {
            BranchId::Trickplay => "trickplay_ms",
            BranchId::Vhash => "vhash_ms",
            BranchId::EmbedSrc => "embed_src_ms",
            BranchId::Chromaprint => "chromaprint_ms",
            BranchId::Silence => "silence_ms",
            BranchId::Loudness => "loudness_ms",
            BranchId::AudioPeaks => "audio_peaks_ms",
            BranchId::BlackFrames => "black_frames_ms",
            BranchId::Corruption => "corruption_ms",
        };
        let ms = dur.as_millis().min(u64::MAX as u128) as u64;
        map.insert(key.to_string(), json!(ms));
    }
    Value::Object(map)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(secs: i64) -> chrono::DateTime<chrono::Utc> {
        chrono::DateTime::from_timestamp(secs, 0).unwrap()
    }

    #[test]
    fn an_untouched_file_is_unchanged() {
        let before = (5_005_321_293, at(1000));
        assert_eq!(
            source_verdict(before, Some(before), None),
            SourceVerdict::Unchanged
        );
    }

    #[test]
    fn the_ingest_touch_is_not_a_change() {
        // Silo S03E09: same size to the byte, mtime 2.7s later, content clean.
        let before = (5_005_321_293, at(1000));
        let after = Some((5_005_321_293, at(1003)));
        assert_eq!(
            source_verdict(before, after, Some(0)),
            SourceVerdict::TouchedButIntact
        );
    }

    #[test]
    fn a_same_size_shell_full_of_zeros_is_a_change() {
        // The case the guard exists for: re-created at full size by a retry
        // that never finished, so the sample finds holes.
        let before = (5_005_321_293, at(1000));
        let after = Some((5_005_321_293, at(1003)));
        assert_eq!(
            source_verdict(before, after, Some(9)),
            SourceVerdict::Changed
        );
    }

    #[test]
    fn a_file_too_small_to_sample_does_not_rescue_the_pass() {
        // The caller passes None below MIN_SAMPLE_SIZE rather than
        // count_zero_blocks' unread Some(0), so a small file stays strict.
        let before = (4 * 1024 * 1024, at(1000));
        let after = Some((4 * 1024 * 1024, at(1003)));
        assert_eq!(source_verdict(before, after, None), SourceVerdict::Changed);
    }

    #[test]
    fn an_unreadable_sample_does_not_rescue_the_pass() {
        let before = (5_005_321_293, at(1000));
        let after = Some((5_005_321_293, at(1003)));
        assert_eq!(source_verdict(before, after, None), SourceVerdict::Changed);
    }

    #[test]
    fn a_changed_size_is_a_change_without_sampling() {
        let before = (5_005_321_293, at(1000));
        let after = Some((1_221_949_393, at(1003)));
        assert_eq!(source_verdict(before, after, None), SourceVerdict::Changed);
    }

    #[test]
    fn a_file_that_vanished_is_a_change() {
        let before = (5_005_321_293, at(1000));
        assert_eq!(source_verdict(before, None, None), SourceVerdict::Changed);
    }
}
