//! Transcript-based chapter detection job handler.
//!
//! Builds a globally-timestamped transcript across all audio files in the
//! book, runs the cue detector once, then maps the resulting chapters back
//! to the file each one falls in. This catches the common multi-file shape
//! where each file announces a single chapter — per-file detection would
//! discard those because the algorithm requires ≥ 2 cues to commit.

use crate::audio::chapter_detect::{DetectedChapter, DetectionSegment, detect_chapters};
use crate::jobs::JobResult;
use crate::jobs::worker::JobHandler;
use crate::repositories::audiobook_chapters::{self, ChapterInput};
use crate::repositories::{audiobook_files, audiobook_transcripts};
use crate::search::SearchBackend;
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sqlx::PgPool;
use std::sync::Arc;
use tokio_util::sync::CancellationToken;
use tracing::{info, warn};
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChapterDetectPayload {
    pub book_id: Uuid,
}

pub struct ChapterDetectHandler {
    pool: PgPool,
    search: Arc<dyn SearchBackend>,
}

impl ChapterDetectHandler {
    pub fn new(pool: PgPool, search: Arc<dyn SearchBackend>) -> Self {
        Self { pool, search }
    }
}

#[async_trait]
impl JobHandler for ChapterDetectHandler {
    fn job_type(&self) -> &'static str {
        "chapter_detect"
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
        let payload: ChapterDetectPayload = serde_json::from_value(payload)?;
        nats_jobs::check_cancelled(&cancel)?;
        let book_id = payload.book_id;

        let files = audiobook_files::list_by_book(&self.pool, book_id).await?;
        if files.is_empty() {
            return Ok(json!({"status": "no_files"}));
        }

        let mut layout: Vec<FileSpan> = Vec::with_capacity(files.len());
        let mut all_segments: Vec<DetectionSegment> = Vec::new();
        let mut offset_seconds: f64 = 0.0;

        for file in &files {
            let transcripts = audiobook_transcripts::get_for_file(&self.pool, file.id).await?;
            let duration = file.duration_seconds.unwrap_or_else(|| {
                transcripts
                    .iter()
                    .map(|t| t.end_seconds)
                    .fold(0.0_f64, f64::max)
            });
            let span = FileSpan {
                file_id: file.id,
                start: offset_seconds,
                end: offset_seconds + duration,
            };
            for t in &transcripts {
                all_segments.push(DetectionSegment {
                    start_seconds: offset_seconds + t.start_seconds,
                    end_seconds: offset_seconds + t.end_seconds,
                    text: t.text.clone(),
                    no_speech_prob: t.no_speech_prob,
                });
            }
            layout.push(span);
            offset_seconds += duration;
        }

        if all_segments.is_empty() {
            return Ok(json!({"detected_chapters": 0, "reason": "no_transcripts"}));
        }

        all_segments.sort_by(|a, b| a.start_seconds.partial_cmp(&b.start_seconds).unwrap());

        let detected = detect_chapters(&all_segments);
        if detected.is_empty() {
            return Ok(json!({"detected_chapters": 0, "reason": "no_cues"}));
        }

        let by_file = group_by_file(&detected, &layout, &all_segments);

        for file in &files {
            for source in ["transcript", "filename", "detected"] {
                if let Err(e) =
                    audiobook_chapters::delete_by_source(&self.pool, file.id, source).await
                {
                    warn!(error = %e, file_id = %file.id, source, "Failed to delete prior chapters");
                }
            }
        }

        let mut total_detected = 0u64;
        for (file_id, inputs) in by_file {
            let count = inputs.len() as u64;
            if let Err(e) = audiobook_chapters::insert_chapters(&self.pool, file_id, &inputs).await
            {
                warn!(error = %e, file_id = %file_id, "Failed to store detected chapters");
            } else {
                total_detected += count;
                info!(file_id = %file_id, chapters = count, "Stored detected chapters");
            }
        }

        info!(
            book_id = %book_id,
            total_detected,
            files = files.len(),
            "Book-level chapter detection complete"
        );

        if total_detected > 0
            && let Err(e) = crate::search::index_transcript_segments_for_book(
                &self.pool,
                self.search.as_ref(),
                book_id,
            )
            .await
        {
            warn!(error = %e, book_id = %book_id, "Failed to re-index transcript segments after chapter detection");
        }

        Ok(json!({"detected_chapters": total_detected}))
    }
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
    let mut grouped: std::collections::BTreeMap<Uuid, Vec<ChapterInput>> =
        std::collections::BTreeMap::new();

    for (i, ch) in detected.iter().enumerate() {
        let Some(span) = find_span(layout, ch.start_seconds) else {
            continue;
        };
        let next_global = detected
            .get(i + 1)
            .map(|n| n.start_seconds)
            .unwrap_or(last_segment_end);
        let local_start = (ch.start_seconds - span.start).max(0.0);
        // Cap end at the file boundary so a chapter that spans into the next
        // file gets clipped here; the next chapter (which sits in that next
        // file) will pick up at the boundary.
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
