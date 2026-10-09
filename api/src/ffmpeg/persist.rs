//! Shared helpers for running ffprobe and persisting results to the database.
//!
//! Used by ingest handlers (media_root_resolve, media_root_match, admin routes)
//! and the media_probe background job to populate video metadata and track records.

use crate::ffmpeg::probe::{self, ChapterInfo, ProbeResult};
use crate::repositories::{
    audio_tracks, episode_skip_segments, movie_files, movie_skip_segments, subtitle_tracks,
    tv_episode_files, video_chapters,
};
use sqlx::PgPool;
use std::path::Path;
use tracing::{debug, warn};
use uuid::Uuid;

/// Run ffprobe on a file, update the file's video metadata, and insert audio/subtitle track records.
///
/// `file_id` is either a movie_file or episode_file id depending on `media_type`.
/// If ffprobe fails, logs a warning and returns Ok(None) — file creation is not blocked.
pub async fn probe_and_persist(
    pool: &PgPool,
    file_id: Uuid,
    file_path: &Path,
    media_type: &str,
) -> Result<Option<ProbeResult>, String> {
    let result = match probe::probe(file_path).await {
        Ok(r) => r,
        Err(e) => {
            warn!(
                file_id = %file_id,
                path = %file_path.display(),
                error = %e,
                "FFprobe failed, skipping metadata population"
            );
            return Ok(None);
        }
    };

    persist_probe_result(pool, file_id, media_type, &result).await?;
    Ok(Some(result))
}

/// Persist a ProbeResult to the database: update video metadata and insert track records.
///
/// Deletes existing tracks first to support re-probing.
pub async fn persist_probe_result(
    pool: &PgPool,
    file_id: Uuid,
    media_type: &str,
    result: &ProbeResult,
) -> Result<(), String> {
    let video = result.video_streams.first();

    match media_type {
        "movie" => {
            let meta = movie_files::VideoMetadataUpdate {
                video_codec: video.map(|v| v.codec.clone()),
                video_width: video.map(|v| v.width),
                video_height: video.map(|v| v.height),
                video_bitrate_kbps: video.and_then(|v| v.bitrate_kbps),
                framerate: video.and_then(|v| v.framerate),
                bit_depth: video.and_then(|v| v.bit_depth),
                is_hdr: video.is_some_and(|v| v.is_hdr),
                hdr_format: video.and_then(|v| v.hdr_format.clone()),
                is_interlaced: video.is_some_and(|v| v.is_interlaced),
                duration_seconds: result.format.duration_seconds,
                container: Some(result.format.format_name.clone()),
                overall_bitrate_kbps: result.format.bit_rate_kbps,
            };
            movie_files::update_video_metadata(pool, file_id, &meta)
                .await
                .map_err(|e| format!("Failed to update movie file metadata: {e}"))?;

            // Delete existing tracks before inserting (supports re-probe)
            let _ = audio_tracks::delete_by_movie_file(pool, file_id).await;
            let _ = subtitle_tracks::delete_by_movie_file(pool, file_id).await;
            let _ = video_chapters::delete_by_movie_file(pool, file_id).await;

            persist_audio_tracks(pool, Some(file_id), None, &result.audio_streams).await;
            persist_subtitle_tracks(pool, Some(file_id), None, &result.subtitle_streams).await;
            persist_chapters(pool, Some(file_id), None, &result.chapters).await;

            // Detect credits from chapter titles (zero-cost heuristic)
            detect_skip_segments_from_chapters_movie(pool, file_id, &result.chapters).await;
        }
        "episode" => {
            let meta = tv_episode_files::VideoMetadataUpdate {
                video_codec: video.map(|v| v.codec.clone()),
                video_width: video.map(|v| v.width),
                video_height: video.map(|v| v.height),
                video_bitrate_kbps: video.and_then(|v| v.bitrate_kbps),
                framerate: video.and_then(|v| v.framerate),
                bit_depth: video.and_then(|v| v.bit_depth),
                is_hdr: video.is_some_and(|v| v.is_hdr),
                hdr_format: video.and_then(|v| v.hdr_format.clone()),
                is_interlaced: video.is_some_and(|v| v.is_interlaced),
                duration_seconds: result.format.duration_seconds,
                container: Some(result.format.format_name.clone()),
                overall_bitrate_kbps: result.format.bit_rate_kbps,
            };
            tv_episode_files::update_video_metadata(pool, file_id, &meta)
                .await
                .map_err(|e| format!("Failed to update episode file metadata: {e}"))?;

            // Delete existing tracks before inserting (supports re-probe)
            let _ = audio_tracks::delete_by_episode_file(pool, file_id).await;
            let _ = subtitle_tracks::delete_by_episode_file(pool, file_id).await;
            let _ = video_chapters::delete_by_episode_file(pool, file_id).await;

            persist_audio_tracks(pool, None, Some(file_id), &result.audio_streams).await;
            persist_subtitle_tracks(pool, None, Some(file_id), &result.subtitle_streams).await;
            persist_chapters(pool, None, Some(file_id), &result.chapters).await;

            // Detect intro/credits from chapter titles (episode only, zero-cost)
            detect_skip_segments_from_chapters(pool, file_id, &result.chapters).await;
        }
        other => {
            return Err(format!("Unknown media type for probe: '{other}'"));
        }
    }

    Ok(())
}

async fn persist_audio_tracks(
    pool: &PgPool,
    movie_file_id: Option<Uuid>,
    episode_file_id: Option<Uuid>,
    streams: &[probe::AudioStream],
) {
    let inputs: Vec<audio_tracks::AudioTrackInput> = streams
        .iter()
        .map(|a| audio_tracks::AudioTrackInput {
            movie_file_id,
            episode_file_id,
            track_index: a.index,
            codec: a.codec.clone(),
            channels: a.channels as i16,
            bitrate_kbps: a.bitrate_kbps,
            sample_rate: a.sample_rate,
            language: a.language.clone(),
            title: a.title.clone(),
            is_default: a.is_default,
        })
        .collect();

    if !inputs.is_empty()
        && let Err(e) = audio_tracks::create_batch(pool, &inputs).await
    {
        warn!(error = %e, "Failed to insert audio tracks from ffprobe");
    }
}

async fn persist_subtitle_tracks(
    pool: &PgPool,
    movie_file_id: Option<Uuid>,
    episode_file_id: Option<Uuid>,
    streams: &[probe::SubtitleStream],
) {
    let inputs: Vec<subtitle_tracks::SubtitleTrackInput> = streams
        .iter()
        .map(|s| subtitle_tracks::SubtitleTrackInput {
            movie_file_id,
            episode_file_id,
            track_index: s.index,
            codec: s.codec.clone(),
            language: s.language.clone(),
            title: s.title.clone(),
            is_forced: s.is_forced,
            is_default: s.is_default,
            is_hearing_impaired: s.is_hearing_impaired,
        })
        .collect();

    if !inputs.is_empty()
        && let Err(e) = subtitle_tracks::create_batch(pool, &inputs).await
    {
        warn!(error = %e, "Failed to insert subtitle tracks from ffprobe");
    }
}

/// Returns true if the title looks like an auto-generated junk chapter.
fn is_junk_title(title: Option<&str>) -> bool {
    let t = match title {
        None => return true,
        Some("") => return true,
        Some(s) => s.trim(),
    };
    if t.is_empty() {
        return true;
    }
    // Timestamp: "00:00:00" or "00:00:00.000"
    if t.len() >= 8
        && t.as_bytes()[2] == b':'
        && t.as_bytes()[5] == b':'
        && t[..2].bytes().all(|b| b.is_ascii_digit())
        && t[3..5].bytes().all(|b| b.is_ascii_digit())
        && t[6..8].bytes().all(|b| b.is_ascii_digit())
    {
        return true;
    }
    // Scene detection: "Scene 1", "Scene 23", or "No Scene Description"
    if t == "No Scene Description"
        || t.strip_prefix("Scene ")
            .is_some_and(|rest| !rest.is_empty() && rest.bytes().all(|b| b.is_ascii_digit()))
    {
        return true;
    }
    // Bare numbers: "1", "10", "3." (digits with optional trailing dot)
    let stripped = t.strip_suffix('.').unwrap_or(t);
    if !stripped.is_empty() && stripped.bytes().all(|b| b.is_ascii_digit()) {
        return true;
    }
    false
}

/// Returns true if ALL chapters in the set are auto-generated junk and should be discarded.
///
/// If any chapter has a meaningful title, the entire set is kept.
pub(crate) fn is_junk_chapter_set(chapters: &[ChapterInfo]) -> bool {
    if chapters.is_empty() {
        return false;
    }
    chapters.iter().all(|ch| is_junk_title(ch.title.as_deref()))
}

async fn persist_chapters(
    pool: &PgPool,
    movie_file_id: Option<Uuid>,
    episode_file_id: Option<Uuid>,
    chapters: &[probe::ChapterInfo],
) {
    if is_junk_chapter_set(chapters) {
        debug!(
            movie_file_id = ?movie_file_id,
            episode_file_id = ?episode_file_id,
            count = chapters.len(),
            "Skipping junk auto-generated chapters"
        );
        return;
    }

    let inputs: Vec<video_chapters::ChapterInput> = chapters
        .iter()
        .map(|ch| video_chapters::ChapterInput {
            movie_file_id,
            episode_file_id,
            chapter_index: ch.index,
            title: ch.title.clone(),
            start_seconds: ch.start_seconds,
            end_seconds: ch.end_seconds,
        })
        .collect();

    if !inputs.is_empty()
        && let Err(e) = video_chapters::insert_chapters(pool, &inputs).await
    {
        warn!(error = %e, "Failed to insert video chapters from ffprobe");
    }
}

/// A skip segment detected from chapter titles.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ChapterSkipSegment {
    pub segment_type: String,
    pub start_seconds: f64,
    pub end_seconds: f64,
}

/// Detect intro/credits/recap skip segments from chapter titles (pure function).
///
/// Scans chapter titles for patterns like "Opening", "OP", "Credits", "ED".
/// Returns detected segments without any DB interaction.
pub fn detect_skip_segments_from_chapter_titles(
    chapters: &[probe::ChapterInfo],
) -> Vec<ChapterSkipSegment> {
    if chapters.is_empty() || is_junk_chapter_set(chapters) {
        return vec![];
    }

    let intro_patterns = ["intro", "opening", "cold open"];
    let credits_patterns = ["credits", "ending", "preview", "next on"];
    let recap_patterns = ["recap", "previously", "previously on", "last time"];
    let intro_abbrevs = ["op"];
    let credits_abbrevs = ["ed"];

    let mut segments = Vec::new();

    for ch in chapters {
        let title = match &ch.title {
            Some(t) => t.to_lowercase(),
            None => continue,
        };
        let title_trimmed = title.trim();

        let is_intro = intro_patterns.iter().any(|p| title_trimmed.contains(p))
            || intro_abbrevs.iter().any(|p| {
                title_trimmed == *p
                    || title_trimmed.starts_with(&format!("{p} "))
                    || title_trimmed.starts_with(&format!("{p}."))
            });

        let is_credits = credits_patterns.iter().any(|p| title_trimmed.contains(p))
            || credits_abbrevs.iter().any(|p| {
                title_trimmed == *p
                    || title_trimmed.starts_with(&format!("{p} "))
                    || title_trimmed.starts_with(&format!("{p}."))
            });

        let is_recap = recap_patterns.iter().any(|p| title_trimmed.contains(p));

        if is_intro {
            segments.push(ChapterSkipSegment {
                segment_type: "intro".to_string(),
                start_seconds: ch.start_seconds,
                end_seconds: ch.end_seconds,
            });
        }
        if is_credits && (ch.end_seconds - ch.start_seconds) >= 15.0 {
            segments.push(ChapterSkipSegment {
                segment_type: "credits".to_string(),
                start_seconds: ch.start_seconds,
                end_seconds: ch.end_seconds,
            });
        }
        if is_recap {
            segments.push(ChapterSkipSegment {
                segment_type: "recap".to_string(),
                start_seconds: ch.start_seconds,
                end_seconds: ch.end_seconds,
            });
        }
    }

    segments
}

/// Detect intro/credits skip segments from chapter titles and persist to DB.
///
/// Only runs for episode files.
async fn detect_skip_segments_from_chapters(
    pool: &PgPool,
    episode_file_id: Uuid,
    chapters: &[probe::ChapterInfo],
) {
    for seg in detect_skip_segments_from_chapter_titles(chapters) {
        if let Err(e) = episode_skip_segments::upsert(
            pool,
            episode_file_id,
            &seg.segment_type,
            seg.start_seconds,
            seg.end_seconds,
            "chapter",
            None,
            None,
        )
        .await
        {
            warn!(error = %e, "Failed to insert {} skip segment from chapter", seg.segment_type);
        }
    }
}

/// Detect credits skip segments from chapter titles for movies.
///
/// Same pattern matching as the episode version, but writes to movie_skip_segments.
async fn detect_skip_segments_from_chapters_movie(
    pool: &PgPool,
    movie_file_id: Uuid,
    chapters: &[probe::ChapterInfo],
) {
    if chapters.is_empty() || is_junk_chapter_set(chapters) {
        return;
    }

    let credits_patterns = ["credits", "ending"];
    let credits_abbrevs = ["ed"];

    for ch in chapters {
        let title = match &ch.title {
            Some(t) => t.to_lowercase(),
            None => continue,
        };
        let title_trimmed = title.trim();

        let is_credits = credits_patterns.iter().any(|p| title_trimmed.contains(p))
            || credits_abbrevs.iter().any(|p| {
                title_trimmed == *p
                    || title_trimmed.starts_with(&format!("{p} "))
                    || title_trimmed.starts_with(&format!("{p}."))
            });

        if is_credits
            && (ch.end_seconds - ch.start_seconds) >= 15.0
            && let Err(e) = movie_skip_segments::upsert(
                pool,
                movie_file_id,
                "credits",
                ch.start_seconds,
                ch.end_seconds,
                "chapter",
                None,
                None,
            )
            .await
        {
            warn!(error = %e, "Failed to insert movie credits skip segment from chapter");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ffmpeg::probe::ChapterInfo;

    fn ch(title: Option<&str>) -> ChapterInfo {
        ChapterInfo {
            index: 0,
            title: title.map(String::from),
            start_seconds: 0.0,
            end_seconds: 1.0,
        }
    }

    #[test]
    fn empty_set_is_not_junk() {
        assert!(!is_junk_chapter_set(&[]));
    }

    #[test]
    fn all_timestamps_are_junk() {
        let chapters = vec![
            ch(Some("00:00:00.000")),
            ch(Some("00:01:23.456")),
            ch(Some("00:15:00.000")),
        ];
        assert!(is_junk_chapter_set(&chapters));
    }

    #[test]
    fn all_scene_detection_are_junk() {
        let chapters = vec![
            ch(Some("Scene 1")),
            ch(Some("Scene 23")),
            ch(Some("No Scene Description")),
        ];
        assert!(is_junk_chapter_set(&chapters));
    }

    #[test]
    fn all_empty_or_null_are_junk() {
        let chapters = vec![ch(None), ch(Some("")), ch(Some("  "))];
        assert!(is_junk_chapter_set(&chapters));
    }

    #[test]
    fn all_bare_numbers_are_junk() {
        let chapters = vec![ch(Some("1")), ch(Some("10.")), ch(Some("3"))];
        assert!(is_junk_chapter_set(&chapters));
    }

    #[test]
    fn mixed_junk_and_meaningful_kept() {
        let chapters = vec![
            ch(Some("00:00:00.000")),
            ch(Some("Intro")),
            ch(Some("Scene 5")),
        ];
        assert!(!is_junk_chapter_set(&chapters));
    }

    #[test]
    fn all_chapter_n_kept() {
        let chapters = vec![
            ch(Some("Chapter 1")),
            ch(Some("Chapter 2")),
            ch(Some("Chapter 3")),
        ];
        assert!(!is_junk_chapter_set(&chapters));
    }

    #[test]
    fn all_meaningful_kept() {
        let chapters = vec![
            ch(Some("Intro")),
            ch(Some("The Journey Begins")),
            ch(Some("Credits")),
        ];
        assert!(!is_junk_chapter_set(&chapters));
    }

    #[test]
    fn mixed_junk_types_all_junk() {
        let chapters = vec![
            ch(None),
            ch(Some("00:05:00.000")),
            ch(Some("Scene 3")),
            ch(Some("42")),
            ch(Some("7.")),
        ];
        assert!(is_junk_chapter_set(&chapters));
    }

    fn ch_timed(title: &str, start: f64, end: f64) -> ChapterInfo {
        ChapterInfo {
            index: 0,
            title: Some(title.to_string()),
            start_seconds: start,
            end_seconds: end,
        }
    }

    #[test]
    fn short_credits_chapter_filtered() {
        // A 6s "credits" bumper card should be filtered out
        let chapters = vec![
            ch_timed("Intro", 0.0, 90.0),
            ch_timed("Credits", 1300.0, 1306.0), // 6s — too short
        ];
        let segs = detect_skip_segments_from_chapter_titles(&chapters);
        assert!(
            segs.iter().all(|s| s.segment_type != "credits"),
            "Short credits chapter should be filtered: {segs:?}"
        );
        // Intro should still be present
        assert!(segs.iter().any(|s| s.segment_type == "intro"));
    }

    #[test]
    fn long_credits_chapter_kept() {
        // A 60s credits chapter should pass the filter
        let chapters = vec![ch_timed("Credits", 1200.0, 1260.0)];
        let segs = detect_skip_segments_from_chapter_titles(&chapters);
        assert_eq!(segs.len(), 1);
        assert_eq!(segs[0].segment_type, "credits");
    }

    #[test]
    fn credits_exactly_15s_kept() {
        let chapters = vec![ch_timed("Credits", 1200.0, 1215.0)];
        let segs = detect_skip_segments_from_chapter_titles(&chapters);
        assert_eq!(segs.len(), 1);
        assert_eq!(segs[0].segment_type, "credits");
    }
}
