//! Chapter resolution fallback chain for audiobooks.
//!
//! Priority:
//! 1. Embedded chapters from ffprobe
//! 2. Sidecar chapters (pre-parsed `ChapterInput` vec)
//! 3. Provider chapters (e.g. from Audnexus, pre-fetched by caller)
//! 4. CUE sheet (`.cue` file alongside the audio file)
//! 5. Silence detection via ffmpeg

use std::path::{Path, PathBuf};

use regex::Regex;
use tracing::{debug, info, warn};

use crate::repositories::audiobook_chapters::ChapterInput;

use super::probe;

/// Chapters shorter than this (seconds) get merged into a neighbor.
/// Matches the constant in `chapter_detect.rs`.
const MIN_CHAPTER_DURATION: f64 = 60.0;

/// Maximum number of chapters silence detection should produce.
const MAX_SILENCE_CHAPTERS: usize = 100;

/// Target chapter length for estimating how many chapters to select.
/// 20 minutes — middle of the ACX standard range (5-120 min).
const TARGET_CHAPTER_DURATION: f64 = 1200.0;

/// A silence gap detected by ffmpeg's silencedetect filter.
#[derive(Debug, Clone)]
pub struct SilenceGap {
    pub start: f64,
    pub end: f64,
    pub duration: f64,
    pub midpoint: f64,
}

/// Resolve chapters for an audiobook file using a fallback chain.
///
/// Tries, in order: embedded chapters, sidecar metadata, provider chapters
/// (e.g. Audnexus), CUE sheet, silence detection.
/// Returns the first non-empty result, or an empty vec.
pub async fn resolve_chapters(
    file_path: &Path,
    sidecar_chapters: Option<Vec<ChapterInput>>,
    provider_chapters: Option<Vec<ChapterInput>>,
) -> Vec<ChapterInput> {
    // 1. Try embedded chapters via ffprobe
    match probe::probe_audio(file_path).await {
        Ok(result) if !result.chapters.is_empty() => {
            info!(
                path = %file_path.display(),
                count = result.chapters.len(),
                "resolved chapters from embedded metadata"
            );
            return result
                .chapters
                .into_iter()
                .map(|ch| ChapterInput {
                    chapter_index: ch.index,
                    title: ch.title,
                    start_seconds: ch.start_seconds,
                    end_seconds: ch.end_seconds,
                    file_path: None,
                    source: "embedded".to_string(),
                })
                .collect();
        }
        Ok(_) => {
            debug!(
                path = %file_path.display(),
                "no embedded chapters found, trying sidecar"
            );
        }
        Err(e) => {
            warn!(
                path = %file_path.display(),
                error = %e,
                "ffprobe failed, trying sidecar chapters"
            );
        }
    }

    // 2. Try sidecar chapters
    if let Some(chapters) = sidecar_chapters
        && !chapters.is_empty()
    {
        info!(
            path = %file_path.display(),
            count = chapters.len(),
            "resolved chapters from sidecar metadata"
        );
        return chapters;
    }

    // 3. Try provider chapters (e.g. Audnexus)
    if let Some(chapters) = provider_chapters
        && !chapters.is_empty()
    {
        info!(
            path = %file_path.display(),
            count = chapters.len(),
            "resolved chapters from metadata provider"
        );
        return chapters;
    }

    // 4. Try CUE sheet
    if let Some(cue_path) = find_cue_file(file_path) {
        match tokio::fs::read_to_string(&cue_path).await {
            Ok(contents) => {
                let parsed = parse_cue_chapters(&contents);
                if !parsed.is_empty() {
                    info!(
                        path = %file_path.display(),
                        cue = %cue_path.display(),
                        count = parsed.len(),
                        "resolved chapters from CUE sheet"
                    );
                    return parsed;
                }
            }
            Err(e) => {
                warn!(
                    cue = %cue_path.display(),
                    error = %e,
                    "failed to read CUE file"
                );
            }
        }
    }

    // 5. Try silence detection
    let detected = detect_silence_chapters(file_path).await;
    if !detected.is_empty() {
        info!(
            path = %file_path.display(),
            count = detected.len(),
            "resolved chapters from silence detection"
        );
        return detected;
    }

    info!(
        path = %file_path.display(),
        "no chapters found from any source"
    );
    Vec::new()
}

/// Find a `.cue` file for the given audio file.
///
/// Checks for a file with the same stem (e.g. `book.cue` for `book.m4b`),
/// then falls back to any single `.cue` file in the same directory.
fn find_cue_file(audio_path: &Path) -> Option<PathBuf> {
    let parent = audio_path.parent()?;
    let stem = audio_path.file_stem()?;

    // Exact stem match first
    let exact = parent.join(stem).with_extension("cue");
    if exact.exists() {
        return Some(exact);
    }

    // Fall back to any .cue in the same directory (only if exactly one)
    let cues: Vec<_> = std::fs::read_dir(parent)
        .ok()?
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| {
            p.extension()
                .is_some_and(|ext| ext.eq_ignore_ascii_case("cue"))
        })
        .collect();

    if cues.len() == 1 {
        Some(cues.into_iter().next().unwrap())
    } else {
        None
    }
}

/// Parse a CUE sheet into chapter inputs.
///
/// Extracts TRACK/INDEX 01 timestamps and TITLE fields. CUE timestamps
/// are in MM:SS:FF format where FF is frames (1/75th of a second).
fn parse_cue_chapters(contents: &str) -> Vec<ChapterInput> {
    let mut chapters: Vec<(Option<String>, f64)> = Vec::new();
    let mut current_title: Option<String> = None;

    let index_re = Regex::new(r"INDEX\s+01\s+(\d+):(\d+):(\d+)").unwrap();
    let title_re = Regex::new(r#"TITLE\s+"([^"]+)""#).unwrap();
    let mut in_track = false;

    for line in contents.lines() {
        let trimmed = line.trim();

        if trimmed.starts_with("TRACK") {
            in_track = true;
            current_title = None;
        } else if in_track {
            if let Some(caps) = title_re.captures(trimmed) {
                current_title = Some(caps[1].to_string());
            }
            if let Some(caps) = index_re.captures(trimmed) {
                let minutes: f64 = caps[1].parse().unwrap_or(0.0);
                let seconds: f64 = caps[2].parse().unwrap_or(0.0);
                let frames: f64 = caps[3].parse().unwrap_or(0.0);
                let total_seconds = minutes * 60.0 + seconds + frames / 75.0;
                chapters.push((current_title.take(), total_seconds));
                in_track = false;
            }
        }
    }

    // Convert start times into ChapterInput with end_seconds derived from the
    // next chapter's start (last chapter gets end = start, caller should patch
    // with actual file duration).
    chapters
        .iter()
        .enumerate()
        .map(|(i, (title, start))| {
            let end = chapters.get(i + 1).map(|(_, s)| *s).unwrap_or(*start);
            ChapterInput {
                chapter_index: i as i32,
                title: title.clone(),
                start_seconds: *start,
                end_seconds: end,
                file_path: None,
                source: "embedded".to_string(),
            }
        })
        .collect()
}

/// Detect silence gaps in an audio file using kob-ffmpeg's silencedetect filter.
///
/// Returns parsed silence gaps with durations and the file's total duration.
pub async fn detect_silence_gaps(file_path: &Path) -> (Vec<SilenceGap>, Option<f64>) {
    let path = file_path.to_owned();
    let result = tokio::task::spawn_blocking(move || {
        // Probe for duration
        let duration = kob_ffmpeg::probe::probe(&path)
            .ok()
            .and_then(|p| p.format.duration_seconds);

        // Detect silence intervals
        let intervals =
            kob_ffmpeg::filter::detect::detect_silence(&path, -30.0, 2.0, None).unwrap_or_default();

        let gaps: Vec<SilenceGap> = intervals
            .into_iter()
            .map(|si| SilenceGap {
                start: si.start_secs,
                end: si.end_secs,
                duration: si.duration_secs,
                midpoint: (si.start_secs + si.end_secs) / 2.0,
            })
            .collect();

        (gaps, duration)
    })
    .await;

    match result {
        Ok(r) => r,
        Err(e) => {
            warn!(
                path = %file_path.display(),
                error = %e,
                "failed to run silence detection"
            );
            (Vec::new(), None)
        }
    }
}

/// Detect chapters by finding silence gaps via ffmpeg's silencedetect filter.
///
/// Uses a selection-based approach: ranks silences by duration (longest first)
/// and picks the top N as chapter boundaries, matching m4b-tool behavior.
pub async fn detect_silence_chapters(file_path: &Path) -> Vec<ChapterInput> {
    let (gaps, duration) = detect_silence_gaps(file_path).await;
    select_chapter_boundaries(&gaps, duration)
}

/// Parse ffmpeg silencedetect stderr output into `SilenceGap` structs.
///
/// Extracts silence_start, silence_end, and silence_duration from ffmpeg stderr.
pub fn parse_silence_gaps(stderr: &str) -> Vec<SilenceGap> {
    let start_re = Regex::new(r"silence_start:\s*([\d.]+)").unwrap();
    let end_re =
        Regex::new(r"silence_end:\s*([\d.]+)\s*\|\s*silence_duration:\s*([\d.]+)").unwrap();

    let starts: Vec<f64> = start_re
        .captures_iter(stderr)
        .filter_map(|c| c[1].parse().ok())
        .collect();

    let mut gaps = Vec::new();
    for (i, caps) in end_re.captures_iter(stderr).enumerate() {
        let end: f64 = match caps[1].parse() {
            Ok(v) => v,
            Err(_) => continue,
        };
        let duration: f64 = match caps[2].parse() {
            Ok(v) => v,
            Err(_) => continue,
        };
        let start = starts.get(i).copied().unwrap_or(end - duration);
        gaps.push(SilenceGap {
            start,
            end,
            duration,
            midpoint: (start + end) / 2.0,
        });
    }

    gaps
}

/// Select chapter boundaries from silence gaps using a selection-based approach.
///
/// Ranks silences by duration (longest = most likely real chapter break),
/// picks the top N based on total file duration, then builds chapters
/// chronologically. This matches m4b-tool's approach.
pub fn select_chapter_boundaries(
    gaps: &[SilenceGap],
    total_duration: Option<f64>,
) -> Vec<ChapterInput> {
    if gaps.is_empty() {
        return Vec::new();
    }

    // Estimate total duration from the last gap if not provided
    let duration = total_duration
        .or_else(|| gaps.last().map(|g| g.end))
        .unwrap_or(0.0);

    if duration < MIN_CHAPTER_DURATION {
        return Vec::new();
    }

    // Estimate target chapter count from total duration
    let target_count = (duration / TARGET_CHAPTER_DURATION)
        .round()
        .max(2.0)
        .min(MAX_SILENCE_CHAPTERS as f64) as usize;

    // Pick the N longest silences as boundaries (selection-based approach)
    let mut ranked: Vec<_> = gaps.iter().collect();
    ranked.sort_by(|a, b| b.duration.partial_cmp(&a.duration).unwrap());
    ranked.truncate(target_count);

    if ranked.is_empty() {
        return Vec::new();
    }

    // Sort selected boundaries chronologically
    ranked.sort_by(|a, b| a.midpoint.partial_cmp(&b.midpoint).unwrap());

    // Build chapters from boundaries
    build_chapters_from_boundaries(&ranked, duration)
}

/// Build ChapterInput vec from selected boundaries.
///
/// Creates chapters spanning: 0 → first boundary → ... → last boundary → total_duration.
/// Applies safety merge for any chapters shorter than MIN_CHAPTER_DURATION.
fn build_chapters_from_boundaries(
    boundaries: &[&SilenceGap],
    total_duration: f64,
) -> Vec<ChapterInput> {
    let mut starts = vec![0.0_f64];
    for gap in boundaries {
        starts.push(gap.midpoint);
    }

    let mut chapters: Vec<ChapterInput> = starts
        .windows(2)
        .enumerate()
        .map(|(i, w)| ChapterInput {
            chapter_index: i as i32,
            title: Some(format!("Chapter {}", i + 1)),
            start_seconds: w[0],
            end_seconds: w[1],
            file_path: None,
            source: "detected".to_string(),
        })
        .collect();

    // Add final chapter from last boundary to total duration
    if let Some(last_boundary) = boundaries.last()
        && total_duration > last_boundary.midpoint
    {
        chapters.push(ChapterInput {
            chapter_index: chapters.len() as i32,
            title: Some(format!("Chapter {}", chapters.len() + 1)),
            start_seconds: last_boundary.midpoint,
            end_seconds: total_duration,
            file_path: None,
            source: "detected".to_string(),
        });
    }

    // Safety merge for any short chapters (shouldn't fire often with selection)
    merge_short_chapters(chapters)
}

/// Merge chapters shorter than `MIN_CHAPTER_DURATION` into their neighbors.
///
/// Iteratively finds the shortest chapter and merges it into the shorter
/// adjacent neighbor (or the only neighbor for first/last chapters).
/// After merging, renumbers chapters. Returns empty if fewer than 2 remain.
fn merge_short_chapters(mut chapters: Vec<ChapterInput>) -> Vec<ChapterInput> {
    loop {
        if chapters.len() <= 1 {
            break;
        }

        let (min_idx, min_dur) = chapters
            .iter()
            .enumerate()
            .map(|(i, ch)| (i, ch.end_seconds - ch.start_seconds))
            .min_by(|a, b| a.1.partial_cmp(&b.1).unwrap())
            .unwrap();

        if min_dur >= MIN_CHAPTER_DURATION {
            break;
        }

        if min_idx == 0 {
            chapters[1].start_seconds = chapters[0].start_seconds;
            chapters.remove(0);
        } else if min_idx == chapters.len() - 1 {
            let last = chapters.len() - 1;
            chapters[last - 1].end_seconds = chapters[last].end_seconds;
            chapters.remove(last);
        } else {
            let prev_dur = chapters[min_idx - 1].end_seconds - chapters[min_idx - 1].start_seconds;
            let next_dur = chapters[min_idx + 1].end_seconds - chapters[min_idx + 1].start_seconds;
            if prev_dur <= next_dur {
                chapters[min_idx - 1].end_seconds = chapters[min_idx].end_seconds;
            } else {
                chapters[min_idx + 1].start_seconds = chapters[min_idx].start_seconds;
            }
            chapters.remove(min_idx);
        }
    }

    if chapters.len() < 2 {
        return Vec::new();
    }

    // Renumber and retitle
    for (i, ch) in chapters.iter_mut().enumerate() {
        ch.chapter_index = i as i32;
        ch.title = Some(format!("Chapter {}", i + 1));
    }

    chapters
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Thin wrapper for tests: parses silence gaps from ffmpeg-style stderr, selects boundaries.
    fn parse_silence_output(stderr: &str) -> Vec<ChapterInput> {
        let gaps = parse_silence_gaps(stderr);
        // Parse duration from "Duration: HH:MM:SS.xx" line
        let duration = {
            let re = Regex::new(r"Duration:\s*(\d+):(\d+):(\d+)\.(\d+)").unwrap();
            re.captures(stderr).and_then(|caps| {
                let hours: f64 = caps[1].parse().ok()?;
                let minutes: f64 = caps[2].parse().ok()?;
                let seconds: f64 = caps[3].parse().ok()?;
                let centis: f64 = caps[4].parse().ok()?;
                Some(hours * 3600.0 + minutes * 60.0 + seconds + centis / 100.0)
            })
        };
        select_chapter_boundaries(&gaps, duration)
    }

    #[test]
    fn test_parse_cue_chapters() {
        let cue = r#"
FILE "audiobook.m4b" WAVE
  TRACK 01 AUDIO
    TITLE "Introduction"
    INDEX 01 00:00:00
  TRACK 02 AUDIO
    TITLE "Chapter One"
    INDEX 01 05:30:00
  TRACK 03 AUDIO
    TITLE "Chapter Two"
    INDEX 01 12:45:37
"#;
        let result = parse_cue_chapters(cue);
        assert_eq!(result.len(), 3);

        assert_eq!(result[0].chapter_index, 0);
        assert_eq!(result[0].title.as_deref(), Some("Introduction"));
        assert!((result[0].start_seconds - 0.0).abs() < 0.001);
        assert!((result[0].end_seconds - 330.0).abs() < 0.001); // 5:30

        assert_eq!(result[1].chapter_index, 1);
        assert_eq!(result[1].title.as_deref(), Some("Chapter One"));
        assert!((result[1].start_seconds - 330.0).abs() < 0.001);
        // 12*60 + 45 + 37/75 = 765.493...
        assert!((result[1].end_seconds - 765.493).abs() < 0.01);

        assert_eq!(result[2].chapter_index, 2);
        assert_eq!(result[2].title.as_deref(), Some("Chapter Two"));
        // Last chapter: end == start (no next chapter to derive from)
        assert!((result[2].end_seconds - result[2].start_seconds).abs() < 0.001);
    }

    #[test]
    fn test_parse_cue_chapters_no_title() {
        let cue = r#"
FILE "audiobook.mp3" MP3
  TRACK 01 AUDIO
    INDEX 01 00:00:00
  TRACK 02 AUDIO
    INDEX 01 10:00:00
"#;
        let result = parse_cue_chapters(cue);
        assert_eq!(result.len(), 2);
        assert!(result[0].title.is_none());
        assert!(result[1].title.is_none());
        assert!((result[0].end_seconds - 600.0).abs() < 0.001);
    }

    #[test]
    fn test_parse_cue_chapters_empty() {
        let result = parse_cue_chapters("");
        assert!(result.is_empty());
    }

    #[test]
    fn test_parse_silence_gaps() {
        let stderr = "\
[silencedetect @ 0x1] silence_start: 300.5\n\
[silencedetect @ 0x1] silence_end: 303.2 | silence_duration: 2.7\n\
[silencedetect @ 0x1] silence_start: 650.1\n\
[silencedetect @ 0x1] silence_end: 652.8 | silence_duration: 2.7\n\
[silencedetect @ 0x1] silence_start: 1200.0\n\
[silencedetect @ 0x1] silence_end: 1203.5 | silence_duration: 3.5\n";
        let gaps = parse_silence_gaps(stderr);
        assert_eq!(gaps.len(), 3);
        assert!((gaps[0].start - 300.5).abs() < 0.01);
        assert!((gaps[0].duration - 2.7).abs() < 0.01);
        assert!((gaps[2].duration - 3.5).abs() < 0.01);
    }

    #[test]
    fn test_select_boundaries_picks_longest_silences() {
        // 10-hour audiobook with 5 silences of varying duration
        // Selection should prefer the longest ones
        let gaps = vec![
            SilenceGap {
                start: 1800.0,
                end: 1802.0,
                duration: 2.0,
                midpoint: 1801.0,
            },
            SilenceGap {
                start: 3600.0,
                end: 3610.0,
                duration: 10.0,
                midpoint: 3605.0,
            },
            SilenceGap {
                start: 7200.0,
                end: 7215.0,
                duration: 15.0,
                midpoint: 7207.5,
            },
            SilenceGap {
                start: 10800.0,
                end: 10803.0,
                duration: 3.0,
                midpoint: 10801.5,
            },
            SilenceGap {
                start: 14400.0,
                end: 14412.0,
                duration: 12.0,
                midpoint: 14406.0,
            },
        ];
        let total_duration = 36000.0; // 10 hours
        // target = 36000/1200 = 30, but only 5 gaps, so all 5 get picked
        let result = select_chapter_boundaries(&gaps, Some(total_duration));
        // With 5 boundaries + start, we get 6 chapters
        assert_eq!(result.len(), 6);
        assert!((result[0].start_seconds - 0.0).abs() < 0.001);
        assert_eq!(result[0].chapter_index, 0);
    }

    #[test]
    fn test_select_boundaries_limits_to_target() {
        // 1-hour audiobook with 20 silences — should pick only ~3 (3600/1200)
        let mut gaps = Vec::new();
        for i in 0..20 {
            let t = (i + 1) as f64 * 170.0;
            gaps.push(SilenceGap {
                start: t,
                end: t + 2.0 + (i as f64 * 0.5), // varying durations
                duration: 2.0 + (i as f64 * 0.5),
                midpoint: t + 1.0 + (i as f64 * 0.25),
            });
        }
        let result = select_chapter_boundaries(&gaps, Some(3600.0));
        // target = 3600/1200 = 3, so we pick 3 boundaries → 4 chapters
        // (after merge, might be fewer if any are <60s)
        assert!(
            result.len() >= 2 && result.len() <= 5,
            "got {} chapters",
            result.len()
        );
    }

    #[test]
    fn test_parse_silence_output_three_clear_gaps() {
        // Simulate ffmpeg output with duration header and 3 silences
        let stderr = "\
  Duration: 00:20:03.50, start: 0.000000, bitrate: 128 kb/s\n\
[silencedetect @ 0x1234] silence_start: 300.5\n\
[silencedetect @ 0x1234] silence_end: 303.2 | silence_duration: 2.7\n\
[silencedetect @ 0x1234] silence_start: 650.1\n\
[silencedetect @ 0x1234] silence_end: 652.8 | silence_duration: 2.7\n\
[silencedetect @ 0x1234] silence_start: 1200.0\n\
[silencedetect @ 0x1234] silence_end: 1203.5 | silence_duration: 3.5\n";
        let result = parse_silence_output(stderr);
        // 20 min audiobook, target = 1 chapter per 20 min ≈ round(1) = 2 minimum
        // 3 gaps available, target ≥ 2, so picks 2-3 gaps
        assert!(result.len() >= 2, "got {} chapters", result.len());
        assert_eq!(result[0].chapter_index, 0);
        assert!((result[0].start_seconds - 0.0).abs() < 0.001);
    }

    #[test]
    fn test_parse_silence_output_no_silence() {
        let result = parse_silence_output("some random ffmpeg output\nno silence here\n");
        assert!(result.is_empty());
    }

    #[test]
    fn test_parse_silence_output_single_gap() {
        // Single gap in a short file → < 2 chapters → empty
        let stderr = "\
  Duration: 00:08:22.00, start: 0.000000\n\
[silencedetect @ 0x1234] silence_start: 500.0\n\
[silencedetect @ 0x1234] silence_end: 502.0 | silence_duration: 2.0\n";
        let result = parse_silence_output(stderr);
        // 1 boundary → 2 chapters, but 8 min file means target=2 min → picks 1 gap → 2 chapters
        // Both chapters are >60s so they survive merge
        assert!(result.len() <= 2);
    }

    #[test]
    fn test_parse_silence_short_file() {
        // File shorter than MIN_CHAPTER_DURATION → no chapters
        let stderr = "\
  Duration: 00:00:45.00, start: 0.000000\n\
[silencedetect @ 0x1234] silence_start: 20.0\n\
[silencedetect @ 0x1234] silence_end: 22.0 | silence_duration: 2.0\n";
        let result = parse_silence_output(stderr);
        assert!(result.is_empty());
    }

    #[test]
    fn test_merge_short_chapters_direct() {
        let chapters = vec![
            ChapterInput {
                chapter_index: 0,
                title: Some("Ch 1".into()),
                start_seconds: 0.0,
                end_seconds: 10.0,
                file_path: None,
                source: "detected".into(),
            },
            ChapterInput {
                chapter_index: 1,
                title: Some("Ch 2".into()),
                start_seconds: 10.0,
                end_seconds: 400.0,
                file_path: None,
                source: "detected".into(),
            },
            ChapterInput {
                chapter_index: 2,
                title: Some("Ch 3".into()),
                start_seconds: 400.0,
                end_seconds: 800.0,
                file_path: None,
                source: "detected".into(),
            },
        ];
        let result = merge_short_chapters(chapters);
        assert_eq!(result.len(), 2);
        assert!((result[0].start_seconds - 0.0).abs() < 0.001);
        assert!((result[0].end_seconds - 400.0).abs() < 0.001);
        assert_eq!(result[0].chapter_index, 0);
        assert_eq!(result[0].title.as_deref(), Some("Chapter 1"));
        assert_eq!(result[1].chapter_index, 1);
        assert_eq!(result[1].title.as_deref(), Some("Chapter 2"));
    }
}
