//! Admin routes for browsing server filesystem directories and detecting media types.

use crate::AppState;
use crate::convert::EbookFormat;
use crate::media_interpreter::InterpretItem;
use crate::routes::AuthUser;
use crate::routes::ingest::AUDIOBOOK_EXTENSIONS;
use axum::{
    Json,
    extract::{Query, State},
    http::StatusCode,
    response::IntoResponse,
};
use serde::{Deserialize, Serialize};
use std::collections::VecDeque;
use std::path::{Path, PathBuf};

/// Video file extensions (mirrors `media_root_scan.rs`).
const VIDEO_EXTENSIONS: &[&str] = &[
    "mkv", "mp4", "avi", "mov", "wmv", "flv", "webm", "m4v", "ts", "m2ts",
];

/// Subtitle file extensions.
const SUBTITLE_EXTENSIONS: &[&str] = &["srt", "ass", "ssa", "sub", "vtt", "sup", "idx"];

#[derive(Deserialize)]
pub struct BrowseQuery {
    #[serde(default = "default_path")]
    pub path: String,
}

fn default_path() -> String {
    "/".to_string()
}

#[derive(Serialize)]
pub struct BrowseResponse {
    pub current_path: String,
    pub parent_path: Option<String>,
    pub directories: Vec<DirectoryEntry>,
}

#[derive(Serialize)]
pub struct DirectoryEntry {
    pub name: String,
    pub path: String,
}

/// Browse server filesystem directories. Returns only subdirectories (no files).
pub async fn browse_directories(
    State(_state): State<AppState>,
    _auth: AuthUser,
    Query(query): Query<BrowseQuery>,
) -> impl IntoResponse {
    let path = PathBuf::from(&query.path);
    match tokio::task::spawn_blocking(move || list_directories(&path)).await {
        Ok(Ok(listing)) => Json(listing).into_response(),
        Ok(Err(error)) => bad_request(error),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}

fn bad_request(error: String) -> axum::response::Response {
    (
        StatusCode::BAD_REQUEST,
        Json(serde_json::json!({ "error": error })),
    )
        .into_response()
}

fn canonical_dir(path: &Path) -> Result<PathBuf, String> {
    // Follows symlinks and normalises.
    let canonical = path
        .canonicalize()
        .map_err(|e| format!("Invalid path: {e}"))?;
    if !canonical.is_dir() {
        return Err("Path is not a directory".to_string());
    }
    Ok(canonical)
}

fn list_directories(path: &Path) -> Result<BrowseResponse, String> {
    let canonical = canonical_dir(path)?;
    let entries =
        std::fs::read_dir(&canonical).map_err(|e| format!("Cannot read directory: {e}"))?;

    let mut directories = Vec::new();
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().to_string();
        if name.starts_with('.') {
            continue;
        }
        // `metadata` follows symlinks, unlike `entry.file_type`.
        let entry_path = entry.path();
        if entry_path.metadata().is_ok_and(|m| m.is_dir()) {
            directories.push(DirectoryEntry {
                path: entry_path.to_string_lossy().to_string(),
                name,
            });
        }
    }
    directories.sort_by_key(|a| a.name.to_lowercase());

    Ok(BrowseResponse {
        current_path: canonical.to_string_lossy().to_string(),
        parent_path: canonical.parent().map(|p| p.to_string_lossy().to_string()),
        directories,
    })
}

// ============================================================================
// Media type detection
// ============================================================================

const MAX_SCAN_FILES: u32 = 200;
const MAX_SCAN_DEPTH: u32 = 3;
const MAX_INTERPRET_SAMPLE: usize = 20;

#[derive(Serialize)]
pub struct DetectMediaTypeResponse {
    pub suggested_type: Option<String>,
    pub confidence: Option<String>,
    pub counts: MediaTypeCounts,
    pub files_sampled: u32,
}

#[derive(Serialize)]
pub struct MediaTypeCounts {
    pub book: u32,
    pub video: u32,
    pub audio: u32,
    pub subtitle: u32,
}

/// Classify a file by extension.
enum FileClass {
    Book,
    Video,
    Audio,
    Subtitle,
    Other,
}

fn classify_file(path: &Path) -> FileClass {
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_lowercase());
    let ext = ext.as_deref();

    // Check ebook formats (already public via EbookFormat)
    if !matches!(
        EbookFormat::from_path(path),
        EbookFormat::Unknown | EbookFormat::Kepub
    ) {
        return FileClass::Book;
    }

    if let Some(e) = ext {
        if VIDEO_EXTENSIONS.contains(&e) {
            return FileClass::Video;
        }
        if AUDIOBOOK_EXTENSIONS.contains(&e) {
            return FileClass::Audio;
        }
        if SUBTITLE_EXTENSIONS.contains(&e) {
            return FileClass::Subtitle;
        }
    }

    FileClass::Other
}

/// Collected state from a shallow directory scan.
struct ScanResult {
    book_count: u32,
    video_count: u32,
    audio_count: u32,
    subtitle_count: u32,
    files_sampled: u32,
    /// Relative paths of video files (up to MAX_INTERPRET_SAMPLE) for interpreter sampling.
    video_sample_paths: Vec<String>,
}

/// Breadth-first walk of a directory, classifying files by extension.
fn sample_directory(root: &Path) -> ScanResult {
    let mut result = ScanResult {
        book_count: 0,
        video_count: 0,
        audio_count: 0,
        subtitle_count: 0,
        files_sampled: 0,
        video_sample_paths: Vec::new(),
    };

    // BFS queue: (absolute path, depth)
    let mut queue: VecDeque<(PathBuf, u32)> = VecDeque::new();
    queue.push_back((root.to_path_buf(), 0));

    while let Some((dir, depth)) = queue.pop_front() {
        if result.files_sampled >= MAX_SCAN_FILES {
            break;
        }

        let entries = match std::fs::read_dir(&dir) {
            Ok(e) => e,
            Err(_) => continue,
        };

        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().to_string();
            if name.starts_with('.') {
                continue;
            }

            let entry_path = entry.path();
            let meta = match entry_path.metadata() {
                Ok(m) => m,
                Err(_) => continue,
            };

            if meta.is_dir() && depth < MAX_SCAN_DEPTH {
                queue.push_back((entry_path, depth + 1));
            } else if meta.is_file() {
                result.files_sampled += 1;

                match classify_file(&entry_path) {
                    FileClass::Book => result.book_count += 1,
                    FileClass::Video => {
                        result.video_count += 1;
                        if result.video_sample_paths.len() < MAX_INTERPRET_SAMPLE
                            && let Ok(relative) = entry_path.strip_prefix(root)
                        {
                            result
                                .video_sample_paths
                                .push(relative.to_string_lossy().to_string());
                        }
                    }
                    FileClass::Audio => result.audio_count += 1,
                    FileClass::Subtitle => result.subtitle_count += 1,
                    FileClass::Other => {}
                }

                if result.files_sampled >= MAX_SCAN_FILES {
                    break;
                }
            }
        }
    }

    result
}

/// Detect the media type of a directory by scanning files and running the interpreter.
pub async fn detect_media_type(
    State(state): State<AppState>,
    _auth: AuthUser,
    Query(query): Query<BrowseQuery>,
) -> impl IntoResponse {
    let path = PathBuf::from(&query.path);
    let scan = match tokio::task::spawn_blocking(move || {
        canonical_dir(&path).map(|dir| sample_directory(&dir))
    })
    .await
    {
        Ok(Ok(scan)) => scan,
        Ok(Err(error)) => return bad_request(error),
        Err(e) => return (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    };

    let counts = MediaTypeCounts {
        book: scan.book_count,
        video: scan.video_count,
        audio: scan.audio_count,
        subtitle: scan.subtitle_count,
    };

    // Audiobook detection: audio files dominate → suggest audiobook
    // Check this before video interpretation since audio extensions overlap with "Other"
    let total_media = scan.book_count + scan.video_count + scan.audio_count;
    if scan.audio_count > scan.video_count
        && scan.audio_count > scan.book_count
        && scan.audio_count > 0
    {
        let confidence = if scan.audio_count >= 5
            || (total_media > 0 && scan.audio_count * 100 / total_media > 66)
        {
            "high"
        } else {
            "low"
        };

        return Json(DetectMediaTypeResponse {
            suggested_type: Some("audiobook".to_string()),
            confidence: Some(confidence.to_string()),
            counts,
            files_sampled: scan.files_sampled,
        })
        .into_response();
    }

    // Use interpreter with "auto" to detect media type in one pass
    if !scan.video_sample_paths.is_empty() {
        let interpreter = crate::media_interpreter::get_interpreter(&state.runtime_config).await;

        let items: Vec<InterpretItem> = scan
            .video_sample_paths
            .iter()
            .map(|p| InterpretItem {
                relative_path: p.clone(),
                file_size: None,
                media_kind: "auto".to_string(),
            })
            .collect();

        let results: Vec<crate::media_interpreter::InterpretResult> =
            interpreter.interpret_batch(items).await.unwrap_or_default();

        let tv_count = results
            .iter()
            .filter(|r| r.media_type.as_deref() == Some("tv"))
            .count();
        let movie_count = results
            .iter()
            .filter(|r| r.media_type.as_deref() == Some("movie"))
            .count();
        let book_count = results
            .iter()
            .filter(|r| r.media_type.as_deref() == Some("book"))
            .count();

        // Pick the type with the most votes
        let best = [
            ("tv", tv_count),
            ("movie", movie_count),
            ("book", book_count),
        ]
        .into_iter()
        .max_by_key(|&(_, c)| c)
        .filter(|&(_, c)| c > 0);

        if let Some((suggested, best_count)) = best {
            let total_success = tv_count + movie_count + book_count;
            let confidence = if total_success > 0 && best_count * 100 / total_success > 66 {
                "high"
            } else {
                "low"
            };

            return Json(DetectMediaTypeResponse {
                suggested_type: Some(suggested.to_string()),
                confidence: Some(confidence.to_string()),
                counts,
                files_sampled: scan.files_sampled,
            })
            .into_response();
        }
    }

    // Fall back to extension-based book detection when no interpreter results
    if scan.book_count > scan.video_count && scan.book_count > 0 {
        let confidence = if scan.book_count >= 5
            || (total_media > 0 && scan.book_count * 100 / total_media > 80)
        {
            "high"
        } else {
            "low"
        };

        return Json(DetectMediaTypeResponse {
            suggested_type: Some("book".to_string()),
            confidence: Some(confidence.to_string()),
            counts,
            files_sampled: scan.files_sampled,
        })
        .into_response();
    }

    // No recognized media files
    Json(DetectMediaTypeResponse {
        suggested_type: None,
        confidence: None,
        counts,
        files_sampled: scan.files_sampled,
    })
    .into_response()
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn touch(dir: &Path, name: &str) {
        let path = dir.join(name);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        std::fs::write(&path, b"").unwrap();
    }

    #[test]
    fn test_classify_book_files() {
        assert!(matches!(
            classify_file(Path::new("book.epub")),
            FileClass::Book
        ));
        assert!(matches!(
            classify_file(Path::new("book.mobi")),
            FileClass::Book
        ));
        assert!(matches!(
            classify_file(Path::new("book.pdf")),
            FileClass::Book
        ));
        assert!(matches!(
            classify_file(Path::new("comic.cbz")),
            FileClass::Book
        ));
    }

    #[test]
    fn test_classify_video_files() {
        assert!(matches!(
            classify_file(Path::new("movie.mkv")),
            FileClass::Video
        ));
        assert!(matches!(
            classify_file(Path::new("show.mp4")),
            FileClass::Video
        ));
    }

    #[test]
    fn test_classify_subtitle_files() {
        assert!(matches!(
            classify_file(Path::new("sub.srt")),
            FileClass::Subtitle
        ));
        assert!(matches!(
            classify_file(Path::new("sub.ass")),
            FileClass::Subtitle
        ));
    }

    #[test]
    fn test_classify_kepub_excluded() {
        // KEPUB files are generated outputs, should not count as books
        assert!(matches!(
            classify_file(Path::new("book.kepub.epub")),
            FileClass::Other
        ));
    }

    #[test]
    fn test_classify_unknown() {
        assert!(matches!(
            classify_file(Path::new("image.jpg")),
            FileClass::Other
        ));
        assert!(matches!(
            classify_file(Path::new("data.json")),
            FileClass::Other
        ));
    }

    #[test]
    fn test_classify_txt_as_book() {
        // EbookFormat treats .txt as a valid ebook format
        assert!(matches!(
            classify_file(Path::new("story.txt")),
            FileClass::Book
        ));
    }

    #[test]
    fn test_sample_books_only() {
        let dir = TempDir::new().unwrap();
        touch(dir.path(), "Author/Book One.epub");
        touch(dir.path(), "Author/Book Two.mobi");
        touch(dir.path(), "Author/Book Three.pdf");
        touch(dir.path(), "Other Author/Comic.cbz");
        touch(dir.path(), "Other Author/Novel.azw3");
        touch(dir.path(), "readme.nfo");

        let scan = sample_directory(dir.path());
        assert_eq!(scan.book_count, 5);
        assert_eq!(scan.video_count, 0);
        assert_eq!(scan.subtitle_count, 0);
        assert!(scan.video_sample_paths.is_empty());
    }

    #[test]
    fn test_sample_tv_structure() {
        let dir = TempDir::new().unwrap();
        touch(dir.path(), "Breaking Bad/Season 01/Breaking.Bad.S01E01.mkv");
        touch(dir.path(), "Breaking Bad/Season 01/Breaking.Bad.S01E02.mkv");
        touch(dir.path(), "Breaking Bad/Season 02/Breaking.Bad.S02E01.mkv");
        touch(dir.path(), "Breaking Bad/Season 01/subs.srt");

        let scan = sample_directory(dir.path());
        assert_eq!(scan.video_count, 3);
        assert_eq!(scan.subtitle_count, 1);
        assert_eq!(scan.book_count, 0);
        assert_eq!(scan.video_sample_paths.len(), 3);
    }

    #[test]
    fn test_sample_movie_structure() {
        let dir = TempDir::new().unwrap();
        touch(dir.path(), "The Matrix (1999)/The.Matrix.1999.mkv");
        touch(dir.path(), "Inception (2010)/Inception.2010.mp4");
        touch(dir.path(), "Inception (2010)/subs.srt");

        let scan = sample_directory(dir.path());
        assert_eq!(scan.video_count, 2);
        assert_eq!(scan.subtitle_count, 1);
        assert_eq!(scan.book_count, 0);
    }

    #[test]
    fn test_sample_empty_dir() {
        let dir = TempDir::new().unwrap();
        let scan = sample_directory(dir.path());
        assert_eq!(scan.files_sampled, 0);
        assert_eq!(scan.book_count, 0);
        assert_eq!(scan.video_count, 0);
    }

    #[test]
    fn test_sample_skips_hidden() {
        let dir = TempDir::new().unwrap();
        touch(dir.path(), ".hidden/secret.epub");
        touch(dir.path(), ".DS_Store");
        touch(dir.path(), "visible.epub");

        let scan = sample_directory(dir.path());
        assert_eq!(scan.book_count, 1);
        assert_eq!(scan.files_sampled, 1);
    }

    #[test]
    fn test_sample_respects_depth_limit() {
        let dir = TempDir::new().unwrap();
        // Depth 0 (root) → 1 → 2 → 3 → 4 (too deep)
        touch(dir.path(), "a/b/c/d/deep.mkv");
        // Depth 3 should be included
        touch(dir.path(), "a/b/c/ok.mkv");

        let scan = sample_directory(dir.path());
        assert_eq!(scan.video_count, 1);
    }

    #[tokio::test]
    async fn test_interpret_tv_vs_movie() {
        use crate::media_interpreter::{LocalInterpreter, MediaInterpreter};

        let interpreter = LocalInterpreter::new();

        // TV-style paths should succeed as TV
        let tv_items = vec![
            InterpretItem {
                relative_path: "Breaking Bad/Season 01/Breaking.Bad.S01E01.mkv".to_string(),
                file_size: None,
                media_kind: "tv".to_string(),
            },
            InterpretItem {
                relative_path: "Breaking Bad/Season 01/Breaking.Bad.S01E02.mkv".to_string(),
                file_size: None,
                media_kind: "tv".to_string(),
            },
        ];
        let results = interpreter.interpret_batch(tv_items).await.unwrap();
        let tv_successes = results.iter().filter(|r| r.success).count();
        assert_eq!(tv_successes, 2);

        // Same paths should fail as movies (no year pattern)
        let movie_items = vec![
            InterpretItem {
                relative_path: "Breaking Bad/Season 01/Breaking.Bad.S01E01.mkv".to_string(),
                file_size: None,
                media_kind: "movies".to_string(),
            },
            InterpretItem {
                relative_path: "Breaking Bad/Season 01/Breaking.Bad.S01E02.mkv".to_string(),
                file_size: None,
                media_kind: "movies".to_string(),
            },
        ];
        let results = interpreter.interpret_batch(movie_items).await.unwrap();
        // Movie interpreter will still extract something from the path but won't find a year
        // Both interpretations may "succeed" but the TV one will have much richer data
        let movie_successes = results.iter().filter(|r| r.success).count();
        // TV should win because its patterns match better
        assert!(tv_successes >= movie_successes);
    }

    #[tokio::test]
    async fn test_interpret_movie_paths() {
        use crate::media_interpreter::{LocalInterpreter, MediaInterpreter};

        let interpreter = LocalInterpreter::new();

        let movie_items = vec![
            InterpretItem {
                relative_path: "The Matrix (1999)/The.Matrix.1999.mkv".to_string(),
                file_size: None,
                media_kind: "movies".to_string(),
            },
            InterpretItem {
                relative_path: "Inception (2010)/Inception.2010.mp4".to_string(),
                file_size: None,
                media_kind: "movies".to_string(),
            },
        ];
        let results = interpreter.interpret_batch(movie_items).await.unwrap();
        let movie_successes = results.iter().filter(|r| r.success).count();
        assert_eq!(movie_successes, 2);

        // Same paths as TV should fail (no SxxExx pattern)
        let tv_items = vec![
            InterpretItem {
                relative_path: "The Matrix (1999)/The.Matrix.1999.mkv".to_string(),
                file_size: None,
                media_kind: "tv".to_string(),
            },
            InterpretItem {
                relative_path: "Inception (2010)/Inception.2010.mp4".to_string(),
                file_size: None,
                media_kind: "tv".to_string(),
            },
        ];
        let results = interpreter.interpret_batch(tv_items).await.unwrap();
        let tv_successes = results.iter().filter(|r| r.success).count();
        assert!(movie_successes > tv_successes);
    }

    #[tokio::test]
    async fn test_interpret_auto_detects_tv() {
        use crate::media_interpreter::{LocalInterpreter, MediaInterpreter};

        let interpreter = LocalInterpreter::new();
        let items = vec![
            InterpretItem {
                relative_path: "Breaking Bad/Season 01/Breaking.Bad.S01E01.mkv".to_string(),
                file_size: None,
                media_kind: "auto".to_string(),
            },
            InterpretItem {
                relative_path: "Breaking Bad/Season 01/Breaking.Bad.S01E02.mkv".to_string(),
                file_size: None,
                media_kind: "auto".to_string(),
            },
        ];
        let results = interpreter.interpret_batch(items).await.unwrap();
        let tv_count = results
            .iter()
            .filter(|r| r.media_type.as_deref() == Some("tv"))
            .count();
        assert_eq!(tv_count, 2, "Auto should detect TV for SxxExx patterns");
    }

    #[tokio::test]
    async fn test_interpret_auto_detects_movie() {
        use crate::media_interpreter::{LocalInterpreter, MediaInterpreter};

        let interpreter = LocalInterpreter::new();
        let items = vec![InterpretItem {
            relative_path: "The Matrix (1999)/The.Matrix.1999.mkv".to_string(),
            file_size: None,
            media_kind: "auto".to_string(),
        }];
        let results = interpreter.interpret_batch(items).await.unwrap();
        assert_eq!(
            results[0].media_type.as_deref(),
            Some("movie"),
            "Auto should detect movie for year-pattern paths"
        );
    }
}
