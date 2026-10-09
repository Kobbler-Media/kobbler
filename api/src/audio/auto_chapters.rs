//! Auto-chapter generation for multi-file audiobooks.
//!
//! When a book has multiple audio files and none have embedded chapters,
//! generates one chapter per file using the filename stem as the title.

use std::path::Path;

use uuid::Uuid;

use crate::repositories::audiobook_chapters::ChapterInput;

/// Generate one chapter per file for multi-file audiobooks.
///
/// Each file gets a single chapter spanning its full duration, with the
/// filename stem as the chapter title.
pub fn generate_file_per_chapter(
    files: &[(Uuid, String, Option<f64>)],
) -> Vec<(Uuid, Vec<ChapterInput>)> {
    files
        .iter()
        .map(|(file_id, file_path, duration)| {
            let title = Path::new(file_path)
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or("Untitled")
                .to_string();

            let end = duration.unwrap_or(0.0);
            let chapter = ChapterInput {
                chapter_index: 0,
                title: Some(title),
                start_seconds: 0.0,
                end_seconds: end,
                file_path: None,
                source: "filename".to_string(),
            };

            (*file_id, vec![chapter])
        })
        .collect()
}
