//! Debrid stream processing handler.
//!
//! Triggered after a progressive download completes. Runs the processing
//! pipeline on the cached file: probe → single-pass media analysis → chapter
//! skip-detect. Each stage updates the session row and emits SSE events.
//!
//! The single-pass produces the full sidecar cache (trickplay, vhash,
//! chromaprint, silence, loudness, black_frames) into `<cache_path>/media_cache/`
//! — the same shape kobbler-native content gets.

use crate::RuntimeConfigManager;
use crate::ffmpeg::probe;
use crate::jobs::worker::JobHandler;
use crate::jobs::{JobError, JobResult};
use crate::media_cache::analyze::{AnalyzeInputs, run_analysis};
use crate::media_cache::episode_cache::MediaKind;
use crate::media_cache::writers::write_manifest_at;
use crate::skip_detect::cache_detect::{
    EpisodeDiagnostics, MovieHandle, detect_movie_from_cache_with_chapters,
    detect_season_from_cache, season_episode_files,
};
use async_trait::async_trait;
use nats_jobs::Publisher;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sqlx::PgPool;
use std::path::PathBuf;
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
    storage_root: String,
}

impl DebridProcessHandler {
    pub fn new(
        pool: PgPool,
        _publisher: Publisher,
        runtime_config: RuntimeConfigManager,
        storage_root: String,
    ) -> Self {
        Self {
            pool,
            runtime_config,
            storage_root,
        }
    }
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

        // Verify the file exists (and is not a directory)
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

        // Step 1: Probe the cached file using kob-ffmpeg (async wrapper around spawn_blocking)
        let probe_result = probe::probe(&cache_path)
            .await
            .map_err(|e| JobError::ExecutionFailed(format!("probe failed: {e}")))?;

        // Store probe_data on the session
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

        // Step 1.5: Cheap chapter-derived segments persist immediately so the
        // SSE skip_segments_ready event fires before analyze finishes. The
        // post-analyze block re-writes the merged (chapter + cross-episode)
        // result, and the SSE loop re-emits on value change.
        let chapter_segments_initial =
            crate::ffmpeg::persist::detect_skip_segments_from_chapter_titles(
                &probe_result.chapters,
            );
        if !chapter_segments_initial.is_empty() {
            persist_skip_segments(&self.pool, session_id, &chapter_segments_initial).await?;
        }

        let target = MediaKind::for_debrid(&session_row.media_type, session_row.media_id);
        let episode_cache_target_dir: Option<PathBuf> = target
            .as_ref()
            .map(|t| crate::media_cache::episode_cache::episode_cache_dir(&self.storage_root, t));

        // Step 2: Single-pass media analysis — writes file-bound artefacts
        // (trickplay, audio_peaks) into `<cache_path>/media_cache/` and
        // content-bound artefacts (vhash, chromaprint, silence, loudness,
        // black_frames, embeddings) into the persistent
        // `episode_cache/{namespace}/{id}/` directory. Cross-episode
        // skip-detect runs after the cache write so its inputs are fresh.
        {
            let rt_config = self.runtime_config.get().await;
            if rt_config.enrichment_enabled {
                let media_cache_dir = cache_path
                    .parent()
                    .unwrap_or(&cache_path)
                    .join("media_cache");
                let trickplay_dir = media_cache_dir.join("trickplay");

                let inputs = AnalyzeInputs {
                    input_path: cache_path.clone(),
                    cache_dir: media_cache_dir.clone(),
                    trickplay_dir,
                    is_video: true,
                    known_duration: probe_result.format.duration_seconds,
                    // Vision is optional and gated by analyze.rs (no vision URL =
                    // skip silently). Generating embeddings during the debrid pass
                    // costs <1 KB/min for the credits window and unlocks visual
                    // credit-text detection on cross-episode replays.
                    want_embed_src: true,
                    media_type: "debrid".to_string(),
                    // analyze.rs writes both manifests (per-file + content)
                    // when this is true. Single-write design.
                    write_manifest_on_success: true,
                    only_embed_src: false,
                    episode_cache_dir: episode_cache_target_dir.clone(),
                    analyzed_file_id: Some(session_row.media_id),
                };
                match run_analysis(inputs, &cancel).await {
                    Ok(outcome) => {
                        if let Err(e) = write_manifest_at(&media_cache_dir, &outcome.manifest) {
                            warn!(error = %e, "debrid manifest write failed");
                        }
                        info!(
                            session_id = %session_id,
                            branches = outcome.branch_results.len(),
                            errors = outcome.per_branch_errors.len(),
                            duration_secs = outcome.input_duration_secs,
                            ?target,
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
        }

        let chapters: Vec<crate::routes::admin::streaming::ChapterResponse> = probe_result
            .chapters
            .iter()
            .map(|ch| crate::routes::admin::streaming::ChapterResponse {
                index: ch.index,
                title: ch.title.clone(),
                start_seconds: ch.start_seconds,
                end_seconds: ch.end_seconds,
            })
            .collect();
        let lufs = crate::media_cache::episode_cache::debrid_title_lufs(
            &self.storage_root,
            &session_row.media_type,
            session_row.media_id,
        );
        persist_file_facts(&self.pool, session_id, lufs, &chapters).await?;

        // Cross-episode / single-file segments from the persisted episode_cache,
        // produced after analyze completes. Empty on first-stream-of-season
        // (the cache_detect fallback path returns single-file results).
        let cross_segments = match self.detect_session_segments(session_id).await {
            Ok(segs) => segs,
            Err(e) => {
                warn!(
                    error = %e,
                    session_id = %session_id,
                    "cross-episode skip detect failed"
                );
                Vec::new()
            }
        };

        let merged = merge_segments(chapter_segments_initial, cross_segments);
        if !merged.is_empty() {
            persist_skip_segments(&self.pool, session_id, &merged).await?;
        }

        Ok(json!({
            "session_id": session_id,
            "probe_complete": true,
            "duration": probe_result.format.duration_seconds,
        }))
    }
}

impl DebridProcessHandler {
    /// Compute skip-segments for the session's media via the persisted
    /// episode_cache, returning a list ready for storage in
    /// `debrid_stream_sessions.skip_segments`. TV episodes use the cross-
    /// episode detector with all season siblings; movies use single-file
    /// detect; everything else returns an empty list.
    async fn detect_session_segments(
        &self,
        session_id: Uuid,
    ) -> Result<Vec<crate::ffmpeg::persist::ChapterSkipSegment>, String> {
        let row = sqlx::query!(
            r#"
            SELECT media_type, media_id
            FROM debrid_stream_sessions
            WHERE id = $1
            "#,
            session_id,
        )
        .fetch_optional(&self.pool)
        .await
        .map_err(|e| format!("session lookup: {e}"))?;
        let Some(row) = row else {
            return Ok(Vec::new());
        };
        let Some(kind) = MediaKind::for_debrid(&row.media_type, row.media_id) else {
            return Ok(Vec::new());
        };

        // Detection classifies cached frame vectors, which needs only the text
        // embedder (prompt vectors).
        let vision_url_owned = self
            .runtime_config
            .get()
            .await
            .effective_text_embedding_url()
            .filter(|s| !s.is_empty());
        let vision_url = vision_url_owned.as_deref();

        match kind {
            MediaKind::DebridEpisode { episode_id } => {
                // Cache hit: a previous debrid stream of this episode (or a
                // native ingest) already produced segments. Skip detect work
                // entirely and return immediately — the SSE handler can
                // emit skip_segments_ready before analyze is even running.
                if let Some(cached) =
                    crate::repositories::cached_segments::get_episode(&self.pool, episode_id)
                        .await
                        .map_err(|e| format!("cached_segments lookup: {e}"))?
                    && cached.pipeline_version >= crate::media_cache::MIN_SUPPORTED_PIPELINE_VERSION
                {
                    info!(
                        %episode_id,
                        pipeline_version = cached.pipeline_version,
                        count = cached.segments.len(),
                        "cached cross-episode segments hit — skipping detect"
                    );
                    return Ok(cached.segments);
                }

                let episodes = season_episode_files(&self.pool, &self.storage_root, episode_id)
                    .await
                    .map_err(|e| format!("season_episode_files: {e}"))?;
                if episodes.is_empty() {
                    return Ok(Vec::new());
                }
                let per_ep =
                    detect_season_from_cache(&self.pool, vision_url, &episodes, &self.storage_root)
                        .await;
                // Each new episode is another sibling to compare, so every episode's
                // segments improve on what was cached when it was itself played; the
                // first of a season, compared with nothing, has none at all.
                let by_episode = segments_by_episode(&episodes, per_ep);
                for (sibling, segments) in &by_episode {
                    if !segments.is_empty()
                        && let Err(e) = crate::repositories::cached_segments::upsert_episode(
                            &self.pool,
                            *sibling,
                            segments,
                            crate::media_cache::PIPELINE_VERSION,
                        )
                        .await
                    {
                        warn!(error = %e, episode_id = %sibling, "cached_segments upsert (episode) failed");
                    }
                }
                let segments = by_episode
                    .into_iter()
                    .find(|(id, _)| *id == episode_id)
                    .map(|(_, segs)| segs)
                    .unwrap_or_default();
                Ok(segments)
            }
            MediaKind::Movie { movie_id } => {
                if let Some(cached) =
                    crate::repositories::cached_segments::get_movie(&self.pool, movie_id)
                        .await
                        .map_err(|e| format!("cached_segments lookup: {e}"))?
                    && cached.pipeline_version >= crate::media_cache::MIN_SUPPORTED_PIPELINE_VERSION
                {
                    info!(
                        %movie_id,
                        pipeline_version = cached.pipeline_version,
                        count = cached.segments.len(),
                        "cached movie segments hit — skipping detect"
                    );
                    return Ok(cached.segments);
                }

                let handle = MovieHandle {
                    file_id: movie_id,
                    duration_seconds: 0.0,
                    movie_id: Some(movie_id),
                };
                // No chapters available without DB join; movie path tolerates
                // empty chapters and returns segments from cache alone.
                let chapters: Vec<(f64, Option<String>)> = Vec::new();
                let (segs, _diag) = detect_movie_from_cache_with_chapters(
                    vision_url,
                    &handle,
                    &self.storage_root,
                    &chapters,
                )
                .await;
                let segments: Vec<_> = segs.into_iter().map(detected_to_skip_segment).collect();
                if !segments.is_empty()
                    && let Err(e) = crate::repositories::cached_segments::upsert_movie(
                        &self.pool,
                        movie_id,
                        &segments,
                        crate::media_cache::PIPELINE_VERSION,
                    )
                    .await
                {
                    warn!(error = %e, %movie_id, "cached_segments upsert (movie) failed");
                }
                Ok(segments)
            }
            // for_debrid only returns DebridEpisode/Movie; TvFile is not
            // produced for debrid sessions.
            MediaKind::TvFile { .. } => Ok(Vec::new()),
        }
    }
}

fn segments_by_episode(
    episodes: &[crate::skip_detect::tv::EpisodeFile],
    per_ep: Vec<(
        Uuid,
        Vec<crate::skip_detect::DetectedSegment>,
        EpisodeDiagnostics,
    )>,
) -> Vec<(Uuid, Vec<crate::ffmpeg::persist::ChapterSkipSegment>)> {
    per_ep
        .into_iter()
        .filter_map(|(file_id, segs, _)| {
            let episode_id = episodes
                .iter()
                .find(|ef| ef.file_id == file_id)?
                .episode_id?;
            Some((
                episode_id,
                segs.into_iter().map(detected_to_skip_segment).collect(),
            ))
        })
        .collect()
}

fn detected_to_skip_segment(
    s: crate::skip_detect::DetectedSegment,
) -> crate::ffmpeg::persist::ChapterSkipSegment {
    crate::ffmpeg::persist::ChapterSkipSegment {
        segment_type: s.segment_type.as_str().to_string(),
        start_seconds: s.start_seconds,
        end_seconds: s.end_seconds,
    }
}

async fn persist_skip_segments(
    pool: &PgPool,
    session_id: Uuid,
    segments: &[crate::ffmpeg::persist::ChapterSkipSegment],
) -> JobResult<()> {
    let segments_json = serde_json::to_value(segments).map_err(|e| {
        JobError::ExecutionFailed(format!("Failed to serialize skip segments: {e}"))
    })?;
    // Every session on the file: a player that joined a download ahead watches its own row.
    sqlx::query!(
        r#"
        UPDATE debrid_stream_sessions
        SET skip_segments = $1, updated_at = NOW()
        WHERE id = $2
           OR cache_path = (SELECT cache_path FROM debrid_stream_sessions WHERE id = $2)
        "#,
        segments_json,
        session_id,
    )
    .execute(pool)
    .await
    .map_err(|e| JobError::ExecutionFailed(format!("Failed to store skip segments: {e}")))?;

    info!(
        session_id = %session_id,
        count = segments.len(),
        "Persisted skip segments for debrid stream"
    );
    Ok(())
}

/// What a library file's row would carry, onto every session on the file.
async fn persist_file_facts(
    pool: &PgPool,
    session_id: Uuid,
    lufs: Option<f64>,
    chapters: &[crate::routes::admin::streaming::ChapterResponse],
) -> JobResult<()> {
    sqlx::query!(
        r#"
        UPDATE debrid_stream_sessions
        SET lufs = COALESCE($2, lufs), chapters = COALESCE($3, chapters), updated_at = NOW()
        WHERE id = $1
           OR cache_path = (SELECT cache_path FROM debrid_stream_sessions WHERE id = $1)
        "#,
        session_id,
        lufs,
        crate::routes::admin::debrid_open::chapters_json(chapters),
    )
    .execute(pool)
    .await
    .map_err(|e| JobError::ExecutionFailed(format!("Failed to store file facts: {e}")))?;
    Ok(())
}

fn merge_segments(
    chapter_segments: Vec<crate::ffmpeg::persist::ChapterSkipSegment>,
    cross_segments: Vec<crate::ffmpeg::persist::ChapterSkipSegment>,
) -> Vec<crate::ffmpeg::persist::ChapterSkipSegment> {
    // Cross-episode results win on conflict — they're more accurate than
    // chapter titles, which can lie. Merge by segment_type, preferring cross.
    let mut by_type: std::collections::HashMap<String, crate::ffmpeg::persist::ChapterSkipSegment> =
        std::collections::HashMap::new();
    for s in chapter_segments {
        by_type.insert(s.segment_type.clone(), s);
    }
    for s in cross_segments {
        by_type.insert(s.segment_type.clone(), s);
    }
    by_type.into_values().collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::skip_detect::tv::EpisodeFile;
    use crate::skip_detect::{DetectedSegment, SegmentType};

    fn episode(file_id: Uuid, episode_id: Uuid) -> EpisodeFile {
        EpisodeFile {
            file_id,
            file_path: String::new(),
            duration_seconds: 3000.0,
            episode_id: Some(episode_id),
        }
    }

    fn credits(start: f64) -> DetectedSegment {
        DetectedSegment {
            segment_type: SegmentType::Credits,
            start_seconds: start,
            end_seconds: 3000.0,
            confidence: 1.0,
            veto_score: 0.0,
            source: "test".to_string(),
        }
    }

    #[test]
    fn every_sibling_detected_in_the_season_pass_gets_its_own_segments() {
        let (debrid_ep, local_ep, local_file) = (Uuid::now_v7(), Uuid::now_v7(), Uuid::now_v7());
        let episodes = [episode(debrid_ep, debrid_ep), episode(local_file, local_ep)];
        let per_ep = vec![
            (
                debrid_ep,
                vec![credits(2800.0)],
                EpisodeDiagnostics::default(),
            ),
            (
                local_file,
                vec![credits(2700.0)],
                EpisodeDiagnostics::default(),
            ),
            (
                Uuid::now_v7(),
                vec![credits(1.0)],
                EpisodeDiagnostics::default(),
            ),
        ];
        let starts: Vec<(Uuid, Vec<f64>)> = segments_by_episode(&episodes, per_ep)
            .into_iter()
            .map(|(id, segs)| (id, segs.iter().map(|s| s.start_seconds).collect()))
            .collect();
        assert_eq!(
            starts,
            vec![(debrid_ep, vec![2800.0]), (local_ep, vec![2700.0])]
        );
    }

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
                ..credits(1.0)
            };
            assert_eq!(
                detected_to_skip_segment(segment).segment_type,
                kind.as_str()
            );
        }
    }
}
