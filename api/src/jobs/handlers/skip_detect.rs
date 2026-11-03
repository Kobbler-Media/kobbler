//! Skip detection — cache-driven end-to-end.
//!
//! Reads the cached sidecars produced by `media_analyze` (which also embeds
//! frames inline via the vision service) and runs the full skip_detect
//! pipeline (intro / credits / post-credits / chapter-sourced segments) over
//! them. The only skip_detect handler — V1's split pipeline was retired.
//!
//! Writes results to the canonical tables (`skip_detect_candidates` +
//! `episode_skip_segments` / `movie_skip_segments`) tagged with
//! `enrichment_config::skip_detect_config_tag(mode, vision_url)`.
//!
//! Signal gating respects server config:
//! - `PipelineMode::Off` → no-op, returns early.
//! - `PipelineMode::Audio` → chromaprint + silence + black_frames + audio
//!   post-credits. No vision.
//! - `PipelineMode::Full` + `vision_url=None` → + vhash, still no vision
//!   classification.
//! - `PipelineMode::Full` + `vision_url=Some` → everything.
//!
//! For episode files the handler also enforces a sibling-readiness gate —
//! cross-episode detection needs every sibling's cache at `pipeline_version=2`
//! with all branches present. If any sibling isn't ready yet the handler
//! returns `deferred` without writing anything; the last sibling's
//! `media_analyze` re-enqueues us.

use crate::RuntimeConfigManager;
use crate::enrichment_config::skip_detect_config_tag;
use crate::ffmpeg::persist::detect_skip_segments_from_chapter_titles;
use crate::ffmpeg::probe::ChapterInfo;
use crate::jobs::worker::JobHandler;
use crate::jobs::{JobError, JobResult};
use crate::media_cache;
use crate::repositories::{
    episode_skip_segments, media_cache_runs, movie_files, movie_skip_segments,
    skip_detect_candidates::{self, CandidateRow},
    tv_episode_files, video_chapters,
};
use crate::skip_detect::cache_detect::{
    EpisodeDiagnostics, MovieHandle, detect_movie_from_cache, detect_season_from_cache,
};
use crate::skip_detect::tv::EpisodeFile;
use crate::skip_detect::{AudioProfile, Candidate, DetectedSegment, PipelineMode};
use async_trait::async_trait;
use nats_jobs::Publisher;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sqlx::PgPool;
use tokio_util::sync::CancellationToken;
use tracing::{info, warn};
use uuid::Uuid;

#[derive(Debug, Deserialize, Serialize)]
pub struct SkipDetectPayload {
    pub file_id: Uuid,
    /// `"episode"` or `"movie"`.
    pub media_type: String,
    /// Set by `media_analyze` so we can mark `media_cache_runs` complete.
    pub run_id: Uuid,
    /// Season this job covers, for episodes. Carried in the payload purely so
    /// `has_active_job("skip_detect", "season_id", ..)` can collapse the
    /// per-file fan-out: detection is season-scoped, so queueing one job per
    /// episode means every season is recomputed once per episode it contains.
    /// `None` for movies and for payloads enqueued before this field existed.
    #[serde(default)]
    pub season_id: Option<Uuid>,
    /// Bypass the staleness gate and re-detect regardless. For when the
    /// detection logic itself changes -- the gate compares cache time to
    /// detection time and cannot see that the code in between is different.
    #[serde(default)]
    pub force: bool,
}

pub struct SkipDetectHandler {
    pool: PgPool,
    publisher: Publisher,
    runtime_config: RuntimeConfigManager,
    storage_root: String,
    library_events: tokio::sync::broadcast::Sender<crate::LibraryEvent>,
}

impl SkipDetectHandler {
    pub fn new(
        pool: PgPool,
        publisher: Publisher,
        runtime_config: RuntimeConfigManager,
        storage_root: String,
        library_events: tokio::sync::broadcast::Sender<crate::LibraryEvent>,
    ) -> Self {
        Self {
            pool,
            publisher,
            runtime_config,
            storage_root,
            library_events,
        }
    }

    /// Tell any open player that this file's segments just landed.
    ///
    /// Detection finishes minutes after a file is ingested, and people press
    /// play as soon as the episode appears — so the common case is that a
    /// session already exists and fetched an empty segment list at creation.
    /// Without this the player shows no intro or credits for the whole of that
    /// first watch, however long the file has been sitting detected.
    async fn announce_segments(&self, file_id: Uuid, media_type: &str) {
        let kind = match media_type {
            "episode" => crate::streaming::session::MediaType::Episode,
            "movie" => crate::streaming::session::MediaType::Movie,
            _ => return,
        };
        let segments =
            crate::routes::admin::streaming::load_skip_segments(&self.pool, kind, file_id).await;
        if segments.is_empty() {
            return;
        }
        match serde_json::to_value(&segments) {
            Ok(value) => {
                let _ = self
                    .library_events
                    .send(crate::LibraryEvent::skip_segments_ready(
                        file_id,
                        media_type.to_string(),
                        value,
                    ));
            }
            Err(e) => warn!(error = %e, file_id = %file_id, "skip_segments_ready serialize failed"),
        }
    }
}

#[async_trait]
impl JobHandler for SkipDetectHandler {
    fn job_type(&self) -> &'static str {
        "skip_detect"
    }

    fn max_concurrent(&self) -> usize {
        self.runtime_config.job_concurrency("skip_detect", 2)
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

        let payload: SkipDetectPayload = serde_json::from_value(payload)
            .map_err(|e| JobError::ExecutionFailed(format!("Invalid payload: {e}")))?;

        let config = self.runtime_config.get().await;
        let mode: PipelineMode = config.skip_detect_mode.parse().unwrap_or(PipelineMode::Off);
        if mode == PipelineMode::Off {
            return Ok(json!({"status": "skipped", "reason": "pipeline_mode_off"}));
        }
        let vision_url = config.effective_vision_url();
        let config_tag = skip_detect_config_tag(&mode, &vision_url);
        // Frames were embedded through the vision URL; classifying them only
        // needs prompt vectors, which come from the text service.
        let embed = DetectEmbedding {
            vision_on: vision_url.as_deref().is_some_and(|s| !s.is_empty()),
            text_url: config.effective_text_embedding_url(),
        };

        info!(
            job_id = %job_id,
            file_id = %payload.file_id,
            media_type = %payload.media_type,
            run_id = %payload.run_id,
            mode = ?mode,
            vision = vision_url.is_some(),
            config_tag = %config_tag,
            "Starting skip_detect"
        );

        // Dispatch.
        let outcome = match payload.media_type.as_str() {
            "movie" => {
                run_movie(
                    &self.pool,
                    &self.publisher,
                    &self.storage_root,
                    payload.file_id,
                    &embed,
                    &config_tag,
                    payload.force,
                )
                .await
            }
            "episode" => {
                run_episode(
                    &self.pool,
                    &self.publisher,
                    &self.storage_root,
                    payload.file_id,
                    &embed,
                    &config_tag,
                    payload.force,
                )
                .await
            }
            other => {
                warn!(media_type = other, "skip_detect: unknown media type");
                Ok(json!({"status": "skipped", "reason": "unknown_media_type"}))
            }
        };

        // Announce to any open player before finalizing. Only on success —
        // `detected_file_ids` is absent on the skip/defer paths, which persist
        // nothing worth announcing.
        if let Ok(ref value) = outcome
            && let Some(ids) = value.get("detected_file_ids").and_then(|v| v.as_array())
        {
            for id in ids.iter().filter_map(|v| v.as_str()) {
                if let Ok(fid) = Uuid::parse_str(id) {
                    self.announce_segments(fid, &payload.media_type).await;
                }
            }
        }

        // Regardless of outcome, close the run row with timing info so the row
        // doesn't stay "in flight" forever. `finalize` is tolerant of missing
        // branch_results. We don't write the scaffolding comparison columns —
        // those are being dropped in a follow-up migration.
        if let Err(e) =
            media_cache_runs::finalize(&self.pool, payload.run_id, 0, &json!({"v2": true})).await
        {
            warn!(error = %e, run_id = %payload.run_id, "media_cache_runs::finalize failed");
        }

        outcome
    }
}

/// Whether any episode in this season needs (re-)detection.
///
/// Stale means the file has a media cache but detection has either never run
/// for it or last ran before that cache was rebuilt. Files without a cache are
/// excluded: they cannot be detected at all, so counting them would pin the
/// season to permanently-stale.
///
/// This is deliberately compared against `skip_detect_ran_at` rather than the
/// presence of segment rows. Detection legitimately produces zero segments for
/// plenty of files (no intro, no credits), and a season containing one such
/// file would otherwise re-run on every pass forever.
async fn season_needs_detect(pool: &PgPool, season_id: Uuid) -> JobResult<bool> {
    sqlx::query_scalar!(
        r#"
        SELECT EXISTS (
            SELECT 1
              FROM tv_episode_files tef
              JOIN tv_episode_file_episodes tefe ON tefe.episode_file_id = tef.id
              JOIN tv_episodes te ON te.id = tefe.episode_id
             WHERE te.season_id = $1
               AND tef.media_cache_generated_at IS NOT NULL
               AND (
                    tef.skip_detect_ran_at IS NULL
                 OR tef.skip_detect_ran_at < tef.media_cache_generated_at
               )
        ) AS "stale!"
        "#,
        season_id,
    )
    .fetch_one(pool)
    .await
    .map_err(|e| JobError::ExecutionFailed(format!("DB error: {e}")))
}

/// Movie counterpart of [`season_needs_detect`], scoped to the single file.
async fn movie_needs_detect(pool: &PgPool, file_id: Uuid) -> JobResult<bool> {
    let stale = sqlx::query_scalar!(
        r#"
        SELECT (
                media_cache_generated_at IS NOT NULL
            AND (
                    skip_detect_ran_at IS NULL
                 OR skip_detect_ran_at < media_cache_generated_at
            )
        ) AS "stale!"
          FROM movie_files
         WHERE id = $1
        "#,
        file_id,
    )
    .fetch_optional(pool)
    .await
    .map_err(|e| JobError::ExecutionFailed(format!("DB error: {e}")))?;
    // A missing row is handled downstream with a clearer error than "up to date".
    Ok(stale.unwrap_or(true))
}

async fn run_movie(
    pool: &PgPool,
    publisher: &Publisher,
    storage_root: &str,
    file_id: Uuid,
    embed: &DetectEmbedding,
    config_tag: &str,
    force: bool,
) -> JobResult<Value> {
    if !force && !movie_needs_detect(pool, file_id).await? {
        return Ok(json!({"status": "skipped", "reason": "already_current"}));
    }
    let file = movie_files::get(pool, file_id)
        .await
        .map_err(|e| JobError::ExecutionFailed(format!("DB error: {e}")))?
        .ok_or_else(|| JobError::ExecutionFailed(format!("Movie file not found: {file_id}")))?;

    if embed.vision_on {
        let kind = crate::media_cache::episode_cache::MediaKind::Movie {
            movie_id: file.movie_id,
        };
        let stale = {
            let root = storage_root.to_string();
            tokio::task::spawn_blocking(move || {
                crate::media_cache::episode_cache::read_manifest(&root, &kind)
                    .is_some_and(|m| embeddings_stale(&m))
            })
            .await
            .expect("skip-detect embeddings staleness task panicked")
        };
        if stale && request_embeddings(pool, publisher, &[file_id], "movie").await > 0 {
            info!(file_id = %file_id, "skip_detect: embeddings from another model — re-embedding first");
            return Ok(json!({
                "status": "deferred",
                "reason": "awaiting_embeddings",
                "file_id": file_id,
            }));
        }
    }

    let handle = MovieHandle {
        file_id: file.id,
        duration_seconds: file.duration_seconds.unwrap_or(0.0),
        movie_id: Some(file.movie_id),
    };
    let (segments, diag) =
        detect_movie_from_cache(pool, embed.text_url.as_deref(), &handle, storage_root).await;

    // Chapter-sourced segments (reads already-persisted video_chapters).
    let chapter_segments = load_chapter_segments_movie(pool, file_id).await;

    let rows = candidate_rows_from_diag(file_id, &diag);
    let candidate_count = rows.len();
    if let Err(e) =
        skip_detect_candidates::replace_movie_candidates(pool, file_id, &rows, config_tag).await
    {
        warn!(error = %e, file_id = %file_id, "replace_movie_candidates failed");
    }
    emit_diagnostics_log(file_id, "movie", &[diag]);

    let (saved_fusion, saved_chapters) =
        persist_movie_segments(pool, file_id, &segments, &chapter_segments, config_tag).await;

    // Stamp last: only a run that actually reached persistence counts as
    // "detected", or a mid-job failure would mark the file current.
    if let Err(e) = movie_files::mark_skip_detect_ran(pool, file_id).await {
        warn!(error = %e, file_id = %file_id, "mark_skip_detect_ran failed");
    }

    info!(
        file_id = %file_id,
        segments = saved_fusion,
        chapter_segments = saved_chapters,
        candidates = candidate_count,
        config_tag = %config_tag,
        "skip_detect movie complete"
    );

    Ok(json!({
        "file_id": file_id,
        "media_type": "movie",
        "segments_saved": saved_fusion,
        "chapter_segments": saved_chapters,
        "candidates_saved": candidate_count,
        "config_tag": config_tag,
        // Read back by `execute` to announce segments to any open player.
        "detected_file_ids": [file_id],
    }))
}

async fn run_episode(
    pool: &PgPool,
    publisher: &Publisher,
    storage_root: &str,
    file_id: Uuid,
    embed: &DetectEmbedding,
    config_tag: &str,
    force: bool,
) -> JobResult<Value> {
    // 1) Discover season + all sibling file_ids.
    let Some(season_id) = lookup_season_for_file(pool, file_id).await else {
        return Ok(json!({"status": "skipped", "reason": "no_season_for_file"}));
    };

    // 1b) Staleness gate. Detection is season-scoped and expensive (O(n^2) in
    //     episode count), while the queue is per-file, so without this a season
    //     is recomputed once per episode it contains. Bail before touching the
    //     filesystem — the readiness check below stats every sibling's cache.
    if !force && !season_needs_detect(pool, season_id).await? {
        return Ok(json!({
            "status": "skipped",
            "reason": "season_already_current",
            "season_id": season_id,
        }));
    }

    let sibling_ids: Vec<Uuid> = tv_episode_files::list_file_ids_by_season(pool, season_id)
        .await
        .map_err(|e| JobError::ExecutionFailed(format!("DB error: {e}")))?;
    if sibling_ids.is_empty() {
        return Ok(json!({"status": "skipped", "reason": "season_has_no_files"}));
    }

    // 2) Sibling-readiness gate. Every sibling must be at the current
    //    pipeline_version with the branches media_analyze actually writes
    //    for video files. Embeddings are required only when vision is
    //    configured (otherwise media_analyze doesn't populate that slot at
    //    all). The last sibling's `media_analyze` will re-enqueue us when
    //    ready.
    let require_embeddings = embed.vision_on;

    // Drop siblings the probe already gave up on before requiring readiness.
    //
    // Waiting is the right response to a sibling that has no cache *yet* -- a
    // NAS that is unmounted or slow, a file queued behind others -- because the
    // last sibling's `media_analyze` re-enqueues us and the season completes.
    // It is the wrong response to a file that has been read and found corrupt:
    // that one never produces a cache, so the season defers on every pass
    // forever and silently loses intro/credits detection for every other
    // episode in it. Excluding it lets the readable siblings be detected.
    //
    // This does not weaken the offline-root case. The `'unreadable'` sentinel
    // is only written when `is_file()` succeeded and ffprobe still failed; an
    // unreachable root fails that guard and errors the probe instead, leaving
    // `video_codec` NULL and the sibling blocking exactly as before. And the
    // mark is not permanent: replacing the file changes its size, which clears
    // the sentinel on the next scan, so a repaired episode rejoins the season
    // and the staleness gate re-detects it.
    let unreadable: std::collections::HashSet<Uuid> =
        tv_episode_files::list_unreadable(pool, &sibling_ids)
            .await
            .map_err(|e| JobError::ExecutionFailed(format!("DB error: {e}")))?
            .into_iter()
            .collect();
    if !unreadable.is_empty() {
        warn!(
            season_id = %season_id,
            unreadable = unreadable.len(),
            siblings = sibling_ids.len(),
            "skip_detect: excluding unreadable siblings from the season"
        );
    }
    let sibling_ids: Vec<Uuid> = sibling_ids
        .into_iter()
        .filter(|id| !unreadable.contains(id))
        .collect();
    if sibling_ids.is_empty() {
        return Ok(json!({"status": "skipped", "reason": "all_siblings_unreadable"}));
    }

    // `sibling_ready` stats a directory and parses a manifest per sibling, so
    // this is a whole season of synchronous reads. Batch it onto the blocking
    // pool rather than walking the filesystem from a runtime worker.
    let not_ready = {
        let ids = sibling_ids.clone();
        let root = storage_root.to_string();
        tokio::task::spawn_blocking(move || {
            ids.into_iter()
                .find(|sid| !sibling_ready(&root, *sid, require_embeddings))
        })
        .await
        .expect("skip-detect sibling readiness task panicked")
    };
    if let Some(sid) = not_ready {
        info!(
            file_id = %file_id,
            season_id = %season_id,
            waiting_on = %sid,
            "skip_detect: sibling cache not ready — deferring"
        );
        return Ok(json!({
            "status": "deferred",
            "reason": "awaiting_siblings",
            "season_id": season_id,
            "waiting_on": sid,
        }));
    }
    let ready_ids = sibling_ids;

    // 2b) Embeddings from another model are ignored by the reader, so detecting
    //     now would run without vision and stamp the season current. Re-embed
    //     first; each supplement re-enqueues us, and the last one finds the
    //     season ready. If nothing could be queued, detect without vision for
    //     those files rather than not at all.
    if require_embeddings {
        let stale: Vec<Uuid> = {
            let ids = ready_ids.clone();
            let root = storage_root.to_string();
            tokio::task::spawn_blocking(move || {
                ids.into_iter()
                    .filter(|sid| {
                        let kind =
                            crate::media_cache::episode_cache::MediaKind::TvFile { file_id: *sid };
                        crate::media_cache::episode_cache::read_manifest(&root, &kind)
                            .is_some_and(|m| embeddings_stale(&m))
                    })
                    .collect()
            })
            .await
            .expect("skip-detect embeddings staleness task panicked")
        };
        if !stale.is_empty() {
            let queued = request_embeddings(pool, publisher, &stale, "episode").await;
            if queued > 0 {
                info!(
                    file_id = %file_id,
                    season_id = %season_id,
                    stale = stale.len(),
                    queued,
                    "skip_detect: embeddings from another model — re-embedding first"
                );
                return Ok(json!({
                    "status": "deferred",
                    "reason": "awaiting_embeddings",
                    "season_id": season_id,
                    "stale": stale.len(),
                }));
            }
            warn!(
                season_id = %season_id,
                stale = stale.len(),
                "skip_detect: could not queue re-embedding; detecting without vision for those files"
            );
        }
    }

    // 3) Build EpisodeFile list by reading each sibling's manifest for duration.
    let mut episodes: Vec<EpisodeFile> = Vec::with_capacity(ready_ids.len());
    for sid in &ready_ids {
        let row = match tv_episode_files::get(pool, *sid).await {
            Ok(Some(r)) => r,
            _ => continue,
        };
        let manifest = media_cache::readers::read_manifest(storage_root, *sid);
        let duration = row
            .duration_seconds
            .or_else(|| manifest.as_ref().map(|m| m.input_duration_seconds))
            .unwrap_or(0.0);
        if duration <= 0.0 {
            continue;
        }
        episodes.push(EpisodeFile {
            file_id: row.id,
            file_path: row.file_path,
            duration_seconds: duration,
            // Native ingest reads from per-file media_cache/{file_id}/ — the
            // junction `tv_episode_file_episodes` is many-to-many for multi-ep
            // files, so there's no single canonical episode_id to pick here.
            // Setting `None` keeps the existing media_cache reader path.
            episode_id: None,
        });
    }
    if episodes.is_empty() {
        return Ok(json!({"status": "skipped", "reason": "no_episode_rows"}));
    }

    // 4) Run the cache-driven season detector.
    let per_ep_segments =
        detect_season_from_cache(pool, embed.text_url.as_deref(), &episodes, storage_root).await;

    // 5) Per-episode chapter-sourced segments.
    let mut chapter_by_file: std::collections::HashMap<
        Uuid,
        Vec<crate::ffmpeg::persist::ChapterSkipSegment>,
    > = std::collections::HashMap::new();
    for ep in &episodes {
        chapter_by_file.insert(
            ep.file_id,
            load_chapter_segments_episode(pool, ep.file_id).await,
        );
    }

    // 6) Persist skip_detect_candidates (debug audit) in one batched INSERT
    //    per season, then segments per file. Candidates include both fusion
    //    winners (selected=true) and every rejected candidate with the
    //    reason it was dropped — this is the queryable equivalent of the
    //    diag stream the detector produces internally.
    let mut candidate_rows: Vec<CandidateRow> = Vec::new();
    let mut diags: Vec<EpisodeDiagnostics> = Vec::with_capacity(per_ep_segments.len());
    for (fid, _segs, diag) in &per_ep_segments {
        candidate_rows.extend(candidate_rows_from_diag(*fid, diag));
        diags.push(diag.clone());
    }
    let candidate_count = candidate_rows.len();
    if let Err(e) = skip_detect_candidates::replace_season_candidates(
        pool,
        season_id,
        &candidate_rows,
        config_tag,
    )
    .await
    {
        warn!(error = %e, season_id = %season_id, "replace_season_candidates failed");
    }
    emit_diagnostics_log(file_id, "season", &diags);

    let mut fusion_saved = 0usize;
    let mut chapter_saved = 0usize;
    for (fid, segments, _diag) in &per_ep_segments {
        let chapter_segs = chapter_by_file.remove(fid).unwrap_or_default();
        let (fs, cs) =
            persist_episode_segments(pool, *fid, segments, &chapter_segs, config_tag).await;
        fusion_saved += fs;
        chapter_saved += cs;
    }
    // Catch any files with chapter segs but no fusion output.
    for (fid, chapter_segs) in chapter_by_file.drain() {
        let (_fs, cs) = persist_episode_segments(pool, fid, &[], &chapter_segs, config_tag).await;
        chapter_saved += cs;
    }

    // Stamp every episode this job actually looked at, not just the one whose
    // file_id triggered it — the detector ran across the whole season, so its
    // siblings are detected as of now too. Stamping only the trigger would
    // leave the rest stale and re-queue the same season on the next pass.
    let detected_ids: Vec<Uuid> = episodes.iter().map(|e| e.file_id).collect();
    if let Err(e) = tv_episode_files::mark_skip_detect_ran(pool, &detected_ids).await {
        warn!(error = %e, season_id = %season_id, "mark_skip_detect_ran failed");
    }

    info!(
        file_id = %file_id,
        season_id = %season_id,
        episodes = episodes.len(),
        segments = fusion_saved,
        chapter_segments = chapter_saved,
        candidates = candidate_count,
        config_tag = %config_tag,
        "skip_detect episode/season complete"
    );

    Ok(json!({
        "season_id": season_id,
        "media_type": "episode",
        "episodes_processed": episodes.len(),
        "segments_saved": fusion_saved,
        "chapter_segments": chapter_saved,
        "candidates_saved": candidate_count,
        "config_tag": config_tag,
        // Read back by `execute` to announce segments to any open player. The
        // whole season is listed, not just the triggering file: detection is
        // season-scoped, so a sibling episode someone is watching right now may
        // have just gained segments too.
        "detected_file_ids": detected_ids,
    }))
}

/// Build a flat list of `CandidateRow`s for a single file from its
/// diagnostics — both fusion winners (selected=true) and rejected
/// candidates (selected=false with `rejection_reason` set).
///
/// Match winners back to their pre-fusion `Candidate` by (segment_type,
/// start_seconds) within a 0.5s epsilon so the stored row carries the full
/// signal breakdown, not just the post-score summary.
fn candidate_rows_from_diag(file_id: Uuid, diag: &EpisodeDiagnostics) -> Vec<CandidateRow> {
    const MATCH_EPSILON: f64 = 0.5;
    let mut rows: Vec<CandidateRow> = Vec::new();

    for seg in &diag.final_segments {
        let matched = diag.candidates_pre_fusion.iter().find(|c| {
            c.segment_type == seg.segment_type
                && (c.start_seconds - seg.start_seconds).abs() < MATCH_EPSILON
        });
        match matched {
            Some(c) => rows.push(candidate_to_row(file_id, c, true, None)),
            None => {
                // Fallback: fusion produced a segment we can't tie back to a
                // pre-fusion candidate (e.g. synthesized downstream). Store a
                // minimal row so the audit still shows the winner.
                rows.push(CandidateRow {
                    file_id,
                    segment_type: seg.segment_type.as_str().to_string(),
                    start_seconds: seg.start_seconds,
                    end_seconds: seg.end_seconds,
                    chromaprint_score: None,
                    dhash_score: None,
                    boundary_has_black_frame: None,
                    boundary_has_silence: None,
                    boundary_refined_start: None,
                    boundary_refined_end: None,
                    audio_profile: None,
                    credit_text_score: None,
                    chapter_agreement: None,
                    file_duration: Some(diag.duration),
                    selected: true,
                    rejection_reason: None,
                });
            }
        }
    }

    for rej in &diag.rejected_candidates {
        rows.push(candidate_to_row(
            file_id,
            &rej.candidate,
            false,
            Some(rej.reason.clone()),
        ));
    }

    rows
}

fn candidate_to_row(
    file_id: Uuid,
    c: &Candidate,
    selected: bool,
    rejection_reason: Option<String>,
) -> CandidateRow {
    let (boundary_black, boundary_silence, boundary_start, boundary_end) =
        match &c.signals.boundary_match {
            Some(b) => (
                Some(b.has_black_frame),
                Some(b.has_silence),
                Some(b.refined_start),
                Some(b.refined_end),
            ),
            None => (None, None, None, None),
        };
    CandidateRow {
        file_id,
        segment_type: c.segment_type.as_str().to_string(),
        start_seconds: c.start_seconds,
        end_seconds: c.end_seconds,
        chromaprint_score: c.signals.chromaprint_score,
        dhash_score: c.signals.dhash_score,
        boundary_has_black_frame: boundary_black,
        boundary_has_silence: boundary_silence,
        boundary_refined_start: boundary_start,
        boundary_refined_end: boundary_end,
        audio_profile: c
            .signals
            .audio_profile
            .map(audio_profile_to_str)
            .map(String::from),
        credit_text_score: c.signals.credit_text_score,
        chapter_agreement: c.signals.chapter_agreement,
        file_duration: c.file_duration,
        selected,
        rejection_reason,
    }
}

fn audio_profile_to_str(p: AudioProfile) -> &'static str {
    match p {
        AudioProfile::MusicOnly => "music_only",
        AudioProfile::Dialogue => "dialogue",
        AudioProfile::NarrationLoop => "narration_loop",
        AudioProfile::Mixed => "mixed",
        AudioProfile::Unknown => "unknown",
    }
}

/// Emit one compact structured log line per file so operators can answer
/// "why didn't this episode get a segment?" from `docker logs` without
/// querying the candidates table. Mirrors the `"media_analyze timing"`
/// shape — one line, JSON in a tracing field.
fn emit_diagnostics_log(anchor_file_id: Uuid, scope: &str, diags: &[EpisodeDiagnostics]) {
    for diag in diags {
        let rejected_summary: Vec<serde_json::Value> = diag
            .rejected_candidates
            .iter()
            .map(|r| {
                json!({
                    "segment_type": r.candidate.segment_type.as_str(),
                    "start_seconds": r.candidate.start_seconds,
                    "end_seconds": r.candidate.end_seconds,
                    "reason": r.reason,
                })
            })
            .collect();
        info!(
            anchor_file_id = %anchor_file_id,
            file_id = %diag.file_id,
            scope = scope,
            duration = diag.duration,
            chromaprint_samples = diag.chromaprint_samples,
            vhash_frames = diag.vhash_frames,
            chapter_count = diag.chapter_count,
            pre_fusion = diag.candidates_pre_fusion.len(),
            persisted = diag.final_segments.len(),
            rejected = diag.rejected_candidates.len(),
            notes = %serde_json::to_string(&diag.notes).unwrap_or_default(),
            rejected_detail = %serde_json::to_string(&rejected_summary).unwrap_or_default(),
            "skip_detect diag"
        );
    }
}

/// What detection needs to know about the embedding services.
struct DetectEmbedding {
    /// Frames are being embedded (a vision URL is configured), so caches are
    /// expected to carry current-model embeddings.
    vision_on: bool,
    /// Where prompt vectors come from for zero-shot classification.
    text_url: Option<String>,
}

/// The cache holds an embeddings entry — vectors or a failed attempt — that was
/// not made by the current model. A cache with no entry at all is not stale: it
/// is still waiting on its first analyze, which the readiness gate handles.
fn embeddings_stale(manifest: &media_cache::writers::Manifest) -> bool {
    manifest.artefacts.embeddings.as_ref().is_some_and(|info| {
        media_cache::readers::embeddings_model(info) != Some(crate::embedding::EMBED_MODEL)
    })
}

/// Queue the embeddings-only supplement for each file (a warm `media_analyze`
/// re-embeds when its embeddings are stale). Returns how many files now have
/// one queued or already running.
///
/// A file whose analyze finished within the last hour and is still stale is
/// left out: that analyze could not re-embed it (and chained this very job), so
/// queueing another would loop. It is detected without vision instead.
async fn request_embeddings(
    pool: &PgPool,
    publisher: &Publisher,
    file_ids: &[Uuid],
    media_type: &str,
) -> usize {
    let mut queued = 0;
    for file_id in file_ids {
        let fid = file_id.to_string();
        if let Ok(true) = crate::jobs::has_active_job(pool, "media_analyze", "file_id", &fid).await
        {
            queued += 1;
            continue;
        }
        if analyzed_recently(pool, &fid).await {
            warn!(file_id = %file_id, "skip_detect: re-embedding did not take; not retrying");
            continue;
        }
        let payload = super::media_analyze::MediaAnalyzePayload {
            file_id: *file_id,
            media_type: media_type.to_string(),
            force_regen: false,
            run_id: None,
            triggered_by: "backfill".to_string(),
            gaps_only: false,
        };
        let context = crate::jobs::JobContext::new("Re-embed frames for skip detection");
        match crate::jobs::enqueue_routine(
            pool,
            publisher,
            "media_analyze",
            serde_json::to_value(&payload).unwrap_or_default(),
            context,
        )
        .await
        {
            Ok(_) => queued += 1,
            Err(e) => warn!(error = %e, file_id = %file_id, "Failed to enqueue re-embedding"),
        }
    }
    queued
}

async fn analyzed_recently(pool: &PgPool, file_id: &str) -> bool {
    sqlx::query_scalar!(
        r#"
        SELECT EXISTS (
            SELECT 1 FROM job_status
            WHERE job_type = 'media_analyze'
              AND payload->>'file_id' = $1
              AND status = 'completed'
              AND completed_at > NOW() - INTERVAL '1 hour'
        ) AS "exists!"
        "#,
        file_id,
    )
    .fetch_one(pool)
    .await
    .unwrap_or(false)
}

/// True when `file_id`'s media_analyze cache is populated enough for
/// cross-episode detection to run against it.
///
/// Mirrors what `media_analyze` actually writes for video files
/// (`media_analyze.rs` `needed_branches` for `is_video`): chromaprint, vhash,
/// silence, black_frames. `audio_peaks` is audiobook-only and is deliberately
/// NOT required here — requiring it would deadlock every episode season.
///
/// Reads the content manifest from `episode_cache/tv_file/{file_id}/`. Since
/// commit 509cc95a the per-file `media_cache/{file_id}/manifest.json` only
/// carries trickplay (+ audio_peaks for audiobooks); content branches live
/// in episode_cache.
///
/// `require_embeddings` is set by the caller when vision is configured. We
/// accept any `Some(_)` entry (including a failed attempt) because
/// `media_analyze` records failures as `present: false` with `attempted_at`
/// so the detection code can fall back to audio-only for that file — a
/// single flaky vision call must not deadlock the whole season.
fn sibling_ready(storage_root: &str, file_id: Uuid, require_embeddings: bool) -> bool {
    let kind = crate::media_cache::episode_cache::MediaKind::TvFile { file_id };
    let Some(m) = crate::media_cache::episode_cache::read_manifest(storage_root, &kind) else {
        return false;
    };
    if m.pipeline_version < media_cache::PIPELINE_VERSION {
        return false;
    }
    let present = |slot: &Option<media_cache::writers::ArtefactInfo>| -> bool {
        slot.as_ref().map(|i| i.present).unwrap_or(false)
    };
    let core_ok = present(&m.artefacts.chromaprint)
        && present(&m.artefacts.vhash)
        && present(&m.artefacts.silence)
        && present(&m.artefacts.black_frames);
    if !core_ok {
        return false;
    }
    if require_embeddings && m.artefacts.embeddings.is_none() {
        return false;
    }
    true
}

async fn lookup_season_for_file(pool: &PgPool, file_id: Uuid) -> Option<Uuid> {
    tv_episode_files::season_id_for_file(pool, file_id)
        .await
        .ok()
        .flatten()
}

async fn load_chapter_segments_episode(
    pool: &PgPool,
    file_id: Uuid,
) -> Vec<crate::ffmpeg::persist::ChapterSkipSegment> {
    let chapters = video_chapters::list_by_episode_file(pool, file_id)
        .await
        .unwrap_or_default();
    let chapter_infos: Vec<ChapterInfo> = chapters
        .into_iter()
        .map(|c| ChapterInfo {
            index: c.chapter_index,
            start_seconds: c.start_seconds,
            end_seconds: c.end_seconds,
            title: c.title,
        })
        .collect();
    detect_skip_segments_from_chapter_titles(&chapter_infos)
}

async fn load_chapter_segments_movie(
    pool: &PgPool,
    file_id: Uuid,
) -> Vec<crate::ffmpeg::persist::ChapterSkipSegment> {
    let chapters = video_chapters::list_by_movie_file(pool, file_id)
        .await
        .unwrap_or_default();
    let chapter_infos: Vec<ChapterInfo> = chapters
        .into_iter()
        .map(|c| ChapterInfo {
            index: c.chapter_index,
            start_seconds: c.start_seconds,
            end_seconds: c.end_seconds,
            title: c.title,
        })
        .collect();
    detect_skip_segments_from_chapter_titles(&chapter_infos)
        .into_iter()
        // Movies only care about credits per V1's chapter movie path.
        .filter(|s| s.segment_type == "credits")
        .collect()
}

async fn persist_episode_segments(
    pool: &PgPool,
    file_id: Uuid,
    segments: &[DetectedSegment],
    chapter_segs: &[crate::ffmpeg::persist::ChapterSkipSegment],
    config_tag: &str,
) -> (usize, usize) {
    // Wipe prior auto-detected rows for this file before inserting fresh.
    // detection re-derives start_seconds from cached sidecars, which can
    // drift by 1s between runs (boundary refinement is sensitive to small
    // changes in silence/black gap detection). Without this delete, the
    // unique key (file_id, segment_type, start_seconds) lets each run
    // accumulate a sibling row instead of replacing — observed on Leftovers
    // S3 where reruns left 28 rows for an 8-episode season that should have
    // 16. Confirmed via xmin: today's run UPDATEs the row matching its
    // current start_seconds but doesn't touch yesterday's leftovers.
    // Manual + chapter rows are preserved.
    if let Err(e) = episode_skip_segments::delete_auto_detected(pool, &[file_id]).await {
        warn!(error = %e, file_id = %file_id, "Failed to clear prior auto segments before persist");
    }

    let mut fusion_saved = 0usize;
    for segment in segments {
        if let Err(e) = episode_skip_segments::upsert(
            pool,
            file_id,
            segment.segment_type.as_str(),
            segment.start_seconds,
            segment.end_seconds,
            &segment.source,
            Some(segment.effective_confidence()),
            Some(config_tag),
        )
        .await
        {
            warn!(error = %e, file_id = %file_id, "Failed to save episode skip segment");
            continue;
        }
        fusion_saved += 1;
        if segment.source != "chapter" && segment.source != "manual" {
            let _ = episode_skip_segments::delete_chapter_for_type(
                pool,
                file_id,
                segment.segment_type.as_str(),
            )
            .await;
        }
    }
    if fusion_saved != segments.len() {
        warn!(
            file_id = %file_id,
            attempted = segments.len(),
            saved = fusion_saved,
            "Episode skip-segment persist count mismatch — some upserts failed"
        );
    }

    let mut chapter_saved = 0usize;
    for cs in chapter_segs {
        if let Err(e) = episode_skip_segments::upsert(
            pool,
            file_id,
            &cs.segment_type,
            cs.start_seconds,
            cs.end_seconds,
            "chapter",
            None,
            Some(config_tag),
        )
        .await
        {
            warn!(error = %e, file_id = %file_id, "Failed to save chapter skip segment");
            continue;
        }
        chapter_saved += 1;
    }

    (fusion_saved, chapter_saved)
}

async fn persist_movie_segments(
    pool: &PgPool,
    file_id: Uuid,
    segments: &[DetectedSegment],
    chapter_segs: &[crate::ffmpeg::persist::ChapterSkipSegment],
    config_tag: &str,
) -> (usize, usize) {
    if let Err(e) = movie_skip_segments::delete_auto_detected(pool, &[file_id]).await {
        warn!(error = %e, file_id = %file_id, "Failed to clear prior auto segments before persist");
    }

    let mut fusion_saved = 0usize;
    for segment in segments {
        if let Err(e) = movie_skip_segments::upsert(
            pool,
            file_id,
            segment.segment_type.as_str(),
            segment.start_seconds,
            segment.end_seconds,
            &segment.source,
            Some(segment.effective_confidence()),
            Some(config_tag),
        )
        .await
        {
            warn!(error = %e, file_id = %file_id, "Failed to save movie skip segment");
            continue;
        }
        fusion_saved += 1;
        if segment.source != "chapter" && segment.source != "manual" {
            let _ = movie_skip_segments::delete_chapter_for_type(
                pool,
                file_id,
                segment.segment_type.as_str(),
            )
            .await;
        }
    }
    if fusion_saved != segments.len() {
        warn!(
            file_id = %file_id,
            attempted = segments.len(),
            saved = fusion_saved,
            "Movie skip-segment persist count mismatch — some upserts failed"
        );
    }

    let mut chapter_saved = 0usize;
    for cs in chapter_segs {
        if let Err(e) = movie_skip_segments::upsert(
            pool,
            file_id,
            &cs.segment_type,
            cs.start_seconds,
            cs.end_seconds,
            "chapter",
            None,
            Some(config_tag),
        )
        .await
        {
            warn!(error = %e, file_id = %file_id, "Failed to save movie chapter segment");
            continue;
        }
        chapter_saved += 1;
    }

    (fusion_saved, chapter_saved)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::media_cache::episode_cache::{MediaKind, episode_cache_dir};
    use crate::media_cache::writers::{ArtefactInfo, Manifest, new_manifest_at, write_manifest_at};
    use crate::media_cache::{Branch, PIPELINE_VERSION};

    /// Build a manifest that mirrors what `media_analyze` writes for a video
    /// file's content branches: vhash/chromaprint/silence/loudness/black_frames
    /// and embeddings. `trickplay` and `audio_peaks` are deliberately absent —
    /// post-509cc95a they live in the per-file `media_cache/{file_id}/`
    /// manifest, not the episode_cache content manifest that `sibling_ready`
    /// reads.
    fn episode_manifest_with_vision(file_id: Uuid) -> Manifest {
        let mut m = new_manifest_at(
            std::path::Path::new(&format!("/tmp/trickplay/{file_id}")),
            3600.0,
        );
        for b in [
            Branch::Vhash,
            Branch::Chromaprint,
            Branch::Silence,
            Branch::Loudness,
            Branch::BlackFrames,
            Branch::Embeddings,
        ] {
            m.artefacts.set(
                b,
                ArtefactInfo {
                    present: true,
                    params: Default::default(),
                },
            );
        }
        m
    }

    /// Persist `m` to the episode_cache layout `sibling_ready` reads from:
    /// `<storage>/episode_cache/tv_file/<file_id>/manifest.json`.
    fn write_tv_file_manifest(storage_root: &str, file_id: Uuid, m: &Manifest) {
        let dir = episode_cache_dir(storage_root, &MediaKind::TvFile { file_id });
        std::fs::create_dir_all(&dir).expect("create episode_cache dir");
        write_manifest_at(&dir, m).expect("write manifest");
    }

    #[test]
    fn realistic_episode_manifest_with_vision_passes_gate() {
        // Regression guards two prior bugs:
        // 1) Pre-2026-04, sibling_ready required audio_peaks (audiobook-only)
        //    so every video manifest failed the gate.
        // 2) Post-509cc95a (2026-04-28), content branches moved out of
        //    media_cache/{file_id}/ into episode_cache/tv_file/{file_id}/ but
        //    sibling_ready was still reading the per-file path — which now
        //    only carries trickplay — so every episode deferred forever.
        let tmp = tempfile::tempdir().expect("tempdir");
        let storage = tmp.path().to_string_lossy().into_owned();
        let file_id = Uuid::now_v7();
        let m = episode_manifest_with_vision(file_id);
        write_tv_file_manifest(&storage, file_id, &m);

        assert!(
            sibling_ready(&storage, file_id, true),
            "manifest matching media_analyze output (vision on, no audio_peaks) must pass the gate"
        );
    }

    #[test]
    fn realistic_episode_manifest_without_vision_passes_gate() {
        // When vision is off, `media_analyze` skips the embeddings branch
        // entirely — the manifest won't have an embeddings entry at all.
        // The gate must accept that when the caller isn't requiring vision.
        let tmp = tempfile::tempdir().expect("tempdir");
        let storage = tmp.path().to_string_lossy().into_owned();
        let file_id = Uuid::now_v7();
        let mut m = new_manifest_at(
            std::path::Path::new(&format!("/tmp/trickplay/{file_id}")),
            3600.0,
        );
        for b in [
            Branch::Vhash,
            Branch::Chromaprint,
            Branch::Silence,
            Branch::Loudness,
            Branch::BlackFrames,
        ] {
            m.artefacts.set(
                b,
                ArtefactInfo {
                    present: true,
                    params: Default::default(),
                },
            );
        }
        write_tv_file_manifest(&storage, file_id, &m);

        assert!(
            sibling_ready(&storage, file_id, false),
            "no-vision manifest must pass the gate when embeddings aren't required"
        );
    }

    #[test]
    fn failed_vision_attempt_still_counts_as_ready() {
        // If vision was configured but the HTTP call failed, `media_analyze`
        // records `Some(ArtefactInfo { present: false, ... })` so the
        // supplement path knows it was attempted. The detection code falls
        // back to audio-only for that file, so the gate must accept it —
        // otherwise one flaky vision call would deadlock the whole season.
        let tmp = tempfile::tempdir().expect("tempdir");
        let storage = tmp.path().to_string_lossy().into_owned();
        let file_id = Uuid::now_v7();
        let mut m = episode_manifest_with_vision(file_id);
        m.artefacts.embeddings = Some(ArtefactInfo {
            present: false,
            params: Default::default(),
        });
        write_tv_file_manifest(&storage, file_id, &m);

        assert!(
            sibling_ready(&storage, file_id, true),
            "failed vision attempt must still count as ready so detection runs audio-only"
        );
    }

    #[test]
    fn missing_manifest_is_not_ready() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let storage = tmp.path().to_string_lossy().into_owned();
        assert!(!sibling_ready(&storage, Uuid::now_v7(), true));
    }

    #[test]
    fn per_file_only_manifest_is_not_ready() {
        // Regression guard for the 509cc95a bug: post-migration, the per-file
        // manifest at `media_cache/{file_id}/manifest.json` exists but holds
        // only trickplay. sibling_ready must look at episode_cache instead;
        // a per-file-only manifest must NOT count as ready.
        use crate::media_cache::writers::write_manifest;
        let tmp = tempfile::tempdir().expect("tempdir");
        let storage = tmp.path().to_string_lossy().into_owned();
        let file_id = Uuid::now_v7();
        let mut m = new_manifest_at(
            std::path::Path::new(&format!("/tmp/trickplay/{file_id}")),
            3600.0,
        );
        m.artefacts.set(
            Branch::Trickplay,
            ArtefactInfo {
                present: true,
                params: Default::default(),
            },
        );
        write_manifest(&storage, file_id, &m).expect("write per-file manifest");

        assert!(
            !sibling_ready(&storage, file_id, true),
            "trickplay-only per-file manifest must not satisfy the gate — content branches live in episode_cache"
        );
    }

    #[test]
    fn stale_pipeline_version_is_not_ready() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let storage = tmp.path().to_string_lossy().into_owned();
        let file_id = Uuid::now_v7();
        let mut m = episode_manifest_with_vision(file_id);
        m.pipeline_version = PIPELINE_VERSION - 1;
        write_tv_file_manifest(&storage, file_id, &m);
        assert!(!sibling_ready(&storage, file_id, true));
    }

    #[test]
    fn missing_core_branch_is_not_ready() {
        // Core branches (chromaprint, vhash, silence, black_frames) must be
        // present regardless of vision — these are what cross-episode
        // detection actually consumes.
        let tmp = tempfile::tempdir().expect("tempdir");
        let storage = tmp.path().to_string_lossy().into_owned();
        let file_id = Uuid::now_v7();
        let mut m = episode_manifest_with_vision(file_id);
        m.artefacts.chromaprint = None;
        write_tv_file_manifest(&storage, file_id, &m);
        assert!(!sibling_ready(&storage, file_id, true));
    }
}
