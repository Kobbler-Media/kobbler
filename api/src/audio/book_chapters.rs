//! Which chapters each file of an audiobook should hold, worked out from
//! everything the book has: the files' own chapter markers, a provider's
//! list, the transcript, the files themselves, and silence in the audio.
//!
//! A file's own chapters always win. Files without any take the book's
//! transcript chapters when the transcript announces them often enough to
//! be believed. Otherwise a book made of chapter-sized files gets one
//! chapter per file, and a book of long files is split at its silences.

use std::collections::{BTreeMap, HashMap, HashSet};

use sqlx::PgPool;
use uuid::Uuid;

use super::chapter_detect::{DetectedChapter, DetectionSegment, detect_chapters};
use super::{auto_chapters, chapters, path};
use crate::repositories::Result;
use crate::repositories::audiobook_chapters::{self, AudiobookChapter, ChapterInput};
use crate::repositories::audiobook_files::{self, AudiobookFileInfo};
use crate::repositories::audiobook_transcripts;

/// Longer than any real chapter. Chapters that average more than this missed
/// most of the book's boundaries, and files that average less are chapters.
pub const LONG_CHAPTER_SECONDS: f64 = 5400.0;

pub enum Silence {
    Detect,
    Skip,
}

pub enum Transcript {
    Chapters(Vec<(Uuid, Vec<ChapterInput>)>),
    Skipped(&'static str),
}

pub struct Change {
    pub file_id: Uuid,
    pub reason: String,
    /// Absent when the replacement needs silence detection the caller skipped.
    pub chapters: Option<Vec<ChapterInput>>,
}

pub struct BookPlan {
    pub changes: Vec<Change>,
    pub own: HashMap<Uuid, Vec<ChapterInput>>,
}

pub fn files_are_chapters(files: &[AudiobookFileInfo]) -> bool {
    let known: Vec<f64> = files.iter().filter_map(|f| f.duration_seconds).collect();
    files.len() > 1
        && !known.is_empty()
        && known.iter().sum::<f64>() / known.len() as f64 <= LONG_CHAPTER_SECONDS
}

fn is_own(source: &str) -> bool {
    matches!(source, "embedded" | "provider")
}

pub fn files_without_own_chapters(
    files: &[AudiobookFileInfo],
    stored: &HashMap<Uuid, Vec<AudiobookChapter>>,
) -> HashSet<Uuid> {
    files
        .iter()
        .filter(|f| {
            !stored
                .get(&f.id)
                .is_some_and(|c| c.iter().any(|c| is_own(&c.source)))
        })
        .map(|f| f.id)
        .collect()
}

pub async fn stored_chapters(
    pool: &PgPool,
    files: &[AudiobookFileInfo],
) -> Result<HashMap<Uuid, Vec<AudiobookChapter>>> {
    let mut stored = HashMap::new();
    for file in files {
        stored.insert(
            file.id,
            audiobook_chapters::get_chapters(pool, file.id).await?,
        );
    }
    Ok(stored)
}

/// Detect chapters across the whole book's transcript and hand back the ones
/// that fall in `targets`.
///
/// Detection runs book-wide because multi-file rips often announce a single
/// chapter per file, which per-file detection would discard for having fewer
/// than two cues.
pub async fn transcript_chapters(
    pool: &PgPool,
    files: &[AudiobookFileInfo],
    targets: &HashSet<Uuid>,
) -> Result<Transcript> {
    let mut layout = Vec::with_capacity(files.len());
    let mut segments = Vec::new();
    let mut offset = 0.0;
    for file in files {
        let transcript = audiobook_transcripts::get_for_file(pool, file.id).await?;
        let duration = file.duration_seconds.unwrap_or_else(|| {
            transcript
                .iter()
                .map(|t| t.end_seconds)
                .fold(0.0_f64, f64::max)
        });
        segments.extend(transcript.into_iter().map(|t| DetectionSegment {
            start_seconds: offset + t.start_seconds,
            end_seconds: offset + t.end_seconds,
            text: t.text,
            no_speech_prob: t.no_speech_prob,
        }));
        layout.push(FileSpan {
            file_id: file.id,
            start: offset,
            end: offset + duration,
        });
        offset += duration;
    }
    if segments.is_empty() {
        return Ok(Transcript::Skipped("no_transcripts"));
    }
    segments.sort_by(|a, b| a.start_seconds.total_cmp(&b.start_seconds));

    let detected = detect_chapters(&segments);
    let by_file: Vec<(Uuid, Vec<ChapterInput>)> = group_by_file(&detected, &layout, &segments)
        .into_iter()
        .filter(|(id, _)| targets.contains(id))
        .collect();
    let count: usize = by_file.iter().map(|(_, c)| c.len()).sum();
    if count == 0 {
        return Ok(Transcript::Skipped("no_cues"));
    }
    let span: f64 = layout
        .iter()
        .filter(|s| targets.contains(&s.file_id))
        .map(|s| s.end - s.start)
        .sum();
    if span / count as f64 > LONG_CHAPTER_SECONDS {
        return Ok(Transcript::Skipped("too_sparse"));
    }
    Ok(Transcript::Chapters(by_file))
}

pub async fn plan(
    pool: &PgPool,
    storage_root: &str,
    book_id: Uuid,
    silence: Silence,
) -> Result<BookPlan> {
    let files = audiobook_files::list_by_book(pool, book_id).await?;
    let stored = stored_chapters(pool, &files).await?;

    let mut own = HashMap::new();
    let mut missing = HashSet::new();
    for file in &files {
        let full = path::resolve_file_path(storage_root, &file.file_path);
        if !full.is_file() {
            missing.insert(file.id);
            continue;
        }
        let found = chapters::resolve_chapters_without_silence(&full, None, None).await;
        if !found.is_empty() {
            own.insert(file.id, found);
        }
    }

    let mut wanted: Vec<(Uuid, Option<Vec<ChapterInput>>)> = Vec::new();
    let mut rest = HashSet::new();
    for file in &files {
        if missing.contains(&file.id) {
            continue;
        }
        if let Some(found) = own.get(&file.id) {
            wanted.push((file.id, Some(clone_inputs(found))));
        } else if !stored[&file.id].iter().any(|c| c.source == "provider") {
            rest.insert(file.id);
        }
    }

    if !rest.is_empty() {
        match transcript_chapters(pool, &files, &rest).await? {
            Transcript::Chapters(by_file) => {
                let mut by_file: HashMap<Uuid, Vec<ChapterInput>> = by_file.into_iter().collect();
                for id in &rest {
                    wanted.push((*id, Some(by_file.remove(id).unwrap_or_default())));
                }
            }
            Transcript::Skipped(_) if files_are_chapters(&files) => {
                let per_file: Vec<(Uuid, String, Option<f64>)> = files
                    .iter()
                    .filter(|f| rest.contains(&f.id))
                    .map(|f| (f.id, f.file_path.clone(), f.duration_seconds))
                    .collect();
                for (id, chapters) in auto_chapters::generate_file_per_chapter(&per_file) {
                    wanted.push((id, Some(chapters)));
                }
            }
            Transcript::Skipped(_) => {
                for file in files.iter().filter(|f| rest.contains(&f.id)) {
                    if silence_chapters_hold_up(&stored[&file.id]) {
                        continue;
                    }
                    let detected = match silence {
                        Silence::Detect => Some(
                            chapters::detect_silence_chapters(&path::resolve_file_path(
                                storage_root,
                                &file.file_path,
                            ))
                            .await,
                        ),
                        Silence::Skip => None,
                    };
                    wanted.push((file.id, detected));
                }
            }
        }
    }

    let changes = wanted
        .into_iter()
        .filter_map(|(file_id, chapters)| {
            let current = &stored[&file_id];
            let reason = match &chapters {
                Some(next) if same(current, next) => return None,
                Some(next) => format!("{} → {}", describe_stored(current), describe(next)),
                None => format!("{} → silence detection", describe_stored(current)),
            };
            Some(Change {
                file_id,
                reason,
                chapters,
            })
        })
        .collect();
    Ok(BookPlan { changes, own })
}

pub async fn apply(pool: &PgPool, plan: &BookPlan) -> Result<usize> {
    let mut written = 0;
    for change in &plan.changes {
        if let Some(chapters) = &change.chapters {
            audiobook_chapters::replace_chapters(pool, change.file_id, chapters).await?;
            written += 1;
        }
    }
    Ok(written)
}

fn silence_chapters_hold_up(stored: &[AudiobookChapter]) -> bool {
    stored.len() >= 2
        && stored.iter().all(|c| {
            c.source == "detected"
                && c.end_seconds - c.start_seconds >= chapters::MIN_CHAPTER_DURATION - 0.5
        })
        && stored
            .windows(2)
            .all(|w| w[1].start_seconds >= w[0].end_seconds - 0.5)
}

fn same(stored: &[AudiobookChapter], wanted: &[ChapterInput]) -> bool {
    let mut wanted = audiobook_chapters::usable(wanted);
    wanted.sort_by(|a, b| a.start_seconds.total_cmp(&b.start_seconds));
    stored.len() == wanted.len()
        && stored.iter().zip(wanted).all(|(s, w)| {
            s.source == w.source
                && s.title == w.title
                && (s.start_seconds - w.start_seconds).abs() < 0.01
                && (s.end_seconds - w.end_seconds).abs() < 0.01
        })
}

fn describe_stored(stored: &[AudiobookChapter]) -> String {
    summary(stored.len(), stored.first().map(|c| c.source.as_str()))
}

fn describe(wanted: &[ChapterInput]) -> String {
    summary(wanted.len(), wanted.first().map(|c| c.source.as_str()))
}

fn summary(count: usize, source: Option<&str>) -> String {
    match source {
        Some(source) => format!("{count} {source}"),
        None => "no chapters".to_string(),
    }
}

fn clone_inputs(chapters: &[ChapterInput]) -> Vec<ChapterInput> {
    chapters
        .iter()
        .map(|c| ChapterInput {
            chapter_index: c.chapter_index,
            title: c.title.clone(),
            start_seconds: c.start_seconds,
            end_seconds: c.end_seconds,
            file_path: c.file_path.clone(),
            source: c.source.clone(),
        })
        .collect()
}

#[derive(Debug, Clone, Copy)]
struct FileSpan {
    file_id: Uuid,
    start: f64,
    end: f64,
}

/// Map globally-timed detected chapters back to per-file `ChapterInput` lists.
/// Each chapter's start time picks the file whose `[start, end)` window
/// contains it; the chapter then runs until the next chapter or the file's
/// own end (whichever comes first), so chapters never bleed across files.
fn group_by_file(
    detected: &[DetectedChapter],
    layout: &[FileSpan],
    segments: &[DetectionSegment],
) -> Vec<(Uuid, Vec<ChapterInput>)> {
    let last_segment_end = segments.last().map(|s| s.end_seconds).unwrap_or(0.0);
    let mut grouped: BTreeMap<Uuid, Vec<ChapterInput>> = BTreeMap::new();

    for (i, ch) in detected.iter().enumerate() {
        let Some(span) = find_span(layout, ch.start_seconds) else {
            continue;
        };
        let next_global = detected
            .get(i + 1)
            .map(|n| n.start_seconds)
            .unwrap_or(last_segment_end);
        let local_start = (ch.start_seconds - span.start).max(0.0);
        let effective_end_global = next_global.min(span.end);
        let local_end = (effective_end_global - span.start).max(local_start);

        let entry = grouped.entry(span.file_id).or_default();
        let chapter_index = entry.len() as i32;
        entry.push(ChapterInput {
            chapter_index,
            title: Some(ch.title.clone()),
            start_seconds: local_start,
            end_seconds: local_end,
            file_path: None,
            source: "transcript".to_string(),
        });
    }

    grouped.into_iter().collect()
}

fn find_span(layout: &[FileSpan], t: f64) -> Option<&FileSpan> {
    layout
        .iter()
        .find(|s| t >= s.start && t < s.end)
        .or_else(|| layout.last().filter(|s| t >= s.start))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn span(file_id: Uuid, start: f64, end: f64) -> FileSpan {
        FileSpan {
            file_id,
            start,
            end,
        }
    }

    fn seg(start: f64, end: f64, text: &str) -> DetectionSegment {
        DetectionSegment {
            start_seconds: start,
            end_seconds: end,
            text: text.to_string(),
            no_speech_prob: Some(0.1),
        }
    }

    /// Multi-file book where each file announces exactly one chapter.
    /// Per-file detection would have discarded each (< 2 cues per file);
    /// book-level detection sees 3 cues across the book and commits.
    #[test]
    fn each_file_one_chapter_book_level() {
        let f0 = Uuid::new_v4();
        let f1 = Uuid::new_v4();
        let f2 = Uuid::new_v4();
        let layout = vec![
            span(f0, 0.0, 600.0),
            span(f1, 600.0, 1200.0),
            span(f2, 1200.0, 1800.0),
        ];
        let segments = vec![
            seg(0.0, 5.0, "Chapter one Benny's Discovery"),
            seg(5.0, 590.0, "It was a sunny afternoon at the beach."),
            seg(600.0, 605.0, "Chapter two The Legend of John Finney"),
            seg(605.0, 1190.0, "Tom's bed and breakfast was very large."),
            seg(1200.0, 1205.0, "Chapter three Benny Becomes Famous"),
            seg(
                1205.0,
                1790.0,
                "After a night of dreams about the treasure.",
            ),
        ];
        let detected = detect_chapters(&segments);
        assert_eq!(detected.len(), 3, "should find 3 chapters globally");

        let grouped = group_by_file(&detected, &layout, &segments);
        assert_eq!(grouped.len(), 3, "one chapter per file");
        for (file_id, inputs) in &grouped {
            assert_eq!(inputs.len(), 1, "file {} has exactly one chapter", file_id);
            assert_eq!(inputs[0].chapter_index, 0);
            assert_eq!(inputs[0].source, "transcript");
            assert!(
                inputs[0]
                    .title
                    .as_deref()
                    .unwrap()
                    .to_lowercase()
                    .starts_with("chapter")
            );
            assert!(
                inputs[0].start_seconds < 10.0,
                "starts near the file's beginning"
            );
        }
    }

    /// Chapter announcement falling at exactly the file boundary lands in the
    /// later file (start belongs to that file's [start, end) window).
    #[test]
    fn chapter_at_file_boundary_lands_in_later_file() {
        let f0 = Uuid::new_v4();
        let f1 = Uuid::new_v4();
        let layout = vec![span(f0, 0.0, 600.0), span(f1, 600.0, 1200.0)];
        let detected = vec![
            DetectedChapter {
                title: "Chapter one".into(),
                start_seconds: 0.0,
            },
            DetectedChapter {
                title: "Chapter two".into(),
                start_seconds: 600.0,
            },
        ];
        let segments = vec![seg(0.0, 1190.0, "...")];
        let grouped: std::collections::HashMap<Uuid, Vec<ChapterInput>> =
            group_by_file(&detected, &layout, &segments)
                .into_iter()
                .collect();
        assert_eq!(grouped.get(&f0).unwrap().len(), 1);
        assert_eq!(grouped.get(&f1).unwrap().len(), 1);
        assert_eq!(grouped.get(&f1).unwrap()[0].start_seconds, 0.0);
    }
}
