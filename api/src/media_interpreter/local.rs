//! LocalInterpreter - regex-based fallback for when Kobload isn't available.
//!
//! Handles common patterns:
//! - TV: S01E02, 1x02, "Season 01/Episode 02", show folder structure
//! - Movies: "Title (Year)", "Title.Year.Quality"
//! - Books: "Author - Title.epub", parent directory as author fallback

use super::{InterpretItem, InterpretResult, InterpreterError, MediaInterpreter};
use async_trait::async_trait;
use regex::Regex;
use std::sync::LazyLock;

#[derive(Default)]
pub struct LocalInterpreter;

impl LocalInterpreter {
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl MediaInterpreter for LocalInterpreter {
    async fn interpret_batch(
        &self,
        items: Vec<InterpretItem>,
    ) -> Result<Vec<InterpretResult>, InterpreterError> {
        Ok(items.iter().map(interpret_single).collect())
    }
}

fn interpret_single(item: &InterpretItem) -> InterpretResult {
    match item.media_kind.as_str() {
        "tv" => interpret_tv(item),
        "movies" | "movie" => interpret_movie(item),
        "books" | "book" => interpret_book(item),
        "audiobooks" | "audiobook" => interpret_audiobook(item),
        _ => {
            // Any unrecognized kind (including "auto") tries TV then movie.
            // Mirrors kobload's parse_release `_ =>` fallback.
            let tv_result = interpret_tv(item);
            if tv_result.success {
                return tv_result;
            }
            interpret_movie(item)
        }
    }
}

// Regex patterns
static TV_SXXEXX: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)[Ss](\d{1,2})[Ee](\d{1,3})(?:[Ee](\d{1,3}))?").unwrap());
static TV_NX_NN: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)(\d{1,2})x(\d{1,3})").unwrap());
static SEASON_FOLDER: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)Season\s+(\d{1,2})").unwrap());
static MOVIE_YEAR: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^(.+?)[\s._]*[\(\[]?((?:19|20)\d{2})[\)\]]?").unwrap());
static TITLE_CLEAN: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"[._]").unwrap());
use super::{extract_year, strip_year};
static BOOK_SUFFIX: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\s*[\(\[].+?[\)\]]\s*$").unwrap());

fn interpret_tv(item: &InterpretItem) -> InterpretResult {
    let path = &item.relative_path;
    let parts: Vec<&str> = path.split('/').collect();
    let filename = parts.last().copied().unwrap_or(path);

    // Try to extract show name from folder structure
    let show_name_from_folder = if parts.len() >= 2 {
        Some(clean_title(parts[0]))
    } else {
        None
    };

    // Try SxxExx pattern first
    if let Some(caps) = TV_SXXEXX.captures(filename) {
        let season: u32 = caps[1].parse().unwrap_or(0);
        let ep1: u32 = caps[2].parse().unwrap_or(0);
        let mut episodes = vec![ep1];
        if let Some(ep2) = caps.get(3)
            && let Ok(n) = ep2.as_str().parse::<u32>()
        {
            episodes.push(n);
        }

        // Extract show name: text before S01E01 in filename, or from folder
        let show_name = show_name_from_folder.unwrap_or_else(|| {
            let before_pattern = &filename[..caps.get(0).unwrap().start()];
            clean_title(before_pattern)
        });

        let show_year = extract_show_year(&show_name);

        return InterpretResult {
            path: path.to_string(),
            success: true,
            error: None,
            media_type: Some("tv".to_string()),
            show_name: Some(strip_year(&show_name)),
            season: Some(season),
            episodes,
            episode_title: None,
            show_year,
            movie_title: None,
            movie_year: None,
            book_title: None,
            book_authors: vec![],
            raw: None,
            sidecar_ids: None,
        };
    }

    // Try NxNN pattern
    if let Some(caps) = TV_NX_NN.captures(filename) {
        let season: u32 = caps[1].parse().unwrap_or(0);
        let episode: u32 = caps[2].parse().unwrap_or(0);

        let show_name = show_name_from_folder.unwrap_or_else(|| {
            let before_pattern = &filename[..caps.get(0).unwrap().start()];
            clean_title(before_pattern)
        });

        return InterpretResult {
            path: path.to_string(),
            success: true,
            error: None,
            media_type: Some("tv".to_string()),
            show_name: Some(strip_year(&show_name)),
            season: Some(season),
            episodes: vec![episode],
            episode_title: None,
            show_year: extract_show_year(&show_name),
            movie_title: None,
            movie_year: None,
            book_title: None,
            book_authors: vec![],
            raw: None,
            sidecar_ids: None,
        };
    }

    // Try Season folder + episode number in filename
    let season_from_folder = parts.iter().find_map(|part| {
        SEASON_FOLDER
            .captures(part)
            .map(|caps| caps[1].parse::<u32>().unwrap_or(0))
    });

    if let Some(season) = season_from_folder {
        // Try to find episode number in filename
        let ep_re = Regex::new(r"(?i)(?:E|Ep|Episode)[\s._]*(\d{1,3})").unwrap();
        if let Some(caps) = ep_re.captures(filename) {
            let episode: u32 = caps[1].parse().unwrap_or(0);
            let show_name = show_name_from_folder.unwrap_or_else(|| clean_title(filename));

            return InterpretResult {
                path: path.to_string(),
                success: true,
                error: None,
                media_type: Some("tv".to_string()),
                show_name: Some(strip_year(&show_name)),
                season: Some(season),
                episodes: vec![episode],
                episode_title: None,
                show_year: extract_show_year(&show_name),
                movie_title: None,
                movie_year: None,
                book_title: None,
                book_authors: vec![],
                raw: None,
                sidecar_ids: None,
            };
        }
    }

    // Fallback: use folder name as show name
    InterpretResult {
        path: path.to_string(),
        success: false,
        error: Some("Could not parse TV episode pattern".to_string()),
        media_type: Some("tv".to_string()),
        show_name: show_name_from_folder.as_ref().map(|s| strip_year(s)),
        season: season_from_folder,
        episodes: Vec::new(),
        episode_title: None,
        show_year: show_name_from_folder.as_deref().and_then(extract_show_year),
        movie_title: None,
        movie_year: None,
        book_title: None,
        book_authors: vec![],
        raw: None,
        sidecar_ids: None,
    }
}

fn interpret_movie(item: &InterpretItem) -> InterpretResult {
    let path = &item.relative_path;
    let parts: Vec<&str> = path.split('/').collect();

    // Use first folder name (or only part if no subdirectory)
    let name_source = parts[0];
    let cleaned = clean_title(name_source);

    if let Some(caps) = MOVIE_YEAR.captures(&cleaned) {
        let title = caps[1].trim().to_string();
        let year: u32 = caps[2].parse().unwrap_or(0);

        return InterpretResult {
            path: path.to_string(),
            success: true,
            error: None,
            media_type: Some("movie".to_string()),
            show_name: None,
            season: None,
            episodes: Vec::new(),
            episode_title: None,
            show_year: None,
            movie_title: Some(title),
            movie_year: Some(year),
            book_title: None,
            book_authors: vec![],
            raw: None,
            sidecar_ids: None,
        };
    }

    // No year found - just use cleaned title
    InterpretResult {
        path: path.to_string(),
        success: !cleaned.is_empty(),
        error: if cleaned.is_empty() {
            Some("Could not extract movie title".to_string())
        } else {
            None
        },
        media_type: Some("movie".to_string()),
        show_name: None,
        season: None,
        episodes: Vec::new(),
        episode_title: None,
        show_year: None,
        movie_title: Some(cleaned),
        movie_year: None,
        book_title: None,
        book_authors: vec![],
        raw: None,
        sidecar_ids: None,
    }
}

fn interpret_book(item: &InterpretItem) -> InterpretResult {
    let path = &item.relative_path;
    let file_name = std::path::Path::new(path)
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or(path);

    let stem = std::path::Path::new(file_name)
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or(file_name);

    // Strip common parenthetical/bracketed suffixes like (retail), [2020]
    let cleaned = BOOK_SUFFIX.replace_all(stem, "").to_string();

    // Try "Author - Title" pattern (split on first " - ")
    if let Some((author, title)) = cleaned.split_once(" - ") {
        let author = author.trim();
        let title = title.trim();
        if !author.is_empty() && !title.is_empty() {
            return InterpretResult {
                path: path.to_string(),
                success: true,
                error: None,
                media_type: Some("book".to_string()),
                show_name: None,
                season: None,
                episodes: Vec::new(),
                episode_title: None,
                show_year: None,
                movie_title: None,
                movie_year: None,
                book_title: Some(title.to_string()),
                book_authors: vec![author.to_string()],
                raw: None,
                sidecar_ids: None,
            };
        }
    }

    // Fallback: use cleaned stem as title, parent directory as author
    let title = cleaned.trim().to_string();
    let parts: Vec<&str> = path.split('/').collect();
    let parent_author = if parts.len() >= 2 {
        let parent = parts[parts.len() - 2];
        if !parent.is_empty() && !parent.starts_with('.') {
            Some(parent.to_string())
        } else {
            None
        }
    } else {
        None
    };

    InterpretResult {
        path: path.to_string(),
        success: !title.is_empty(),
        error: if title.is_empty() {
            Some("Could not extract book title".to_string())
        } else {
            None
        },
        media_type: Some("book".to_string()),
        show_name: None,
        season: None,
        episodes: Vec::new(),
        episode_title: None,
        show_year: None,
        movie_title: None,
        movie_year: None,
        book_title: Some(title),
        book_authors: parent_author.into_iter().collect(),
        raw: None,
        sidecar_ids: None,
    }
}

fn interpret_audiobook(item: &InterpretItem) -> InterpretResult {
    // Reuse book parsing logic — audiobooks follow the same naming conventions
    let mut result = interpret_book(item);
    result.media_type = Some("audiobook".to_string());
    result
}

/// Clean dots and underscores from a title.
fn clean_title(s: &str) -> String {
    let cleaned = TITLE_CLEAN.replace_all(s, " ");
    cleaned.trim().trim_end_matches('-').trim().to_string()
}

/// Extract year from end of a show name like "Show Name (2020)".
fn extract_show_year(name: &str) -> Option<u32> {
    extract_year(name)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tv_item(path: &str) -> InterpretItem {
        InterpretItem {
            relative_path: path.to_string(),
            file_size: None,
            media_kind: "tv".to_string(),
        }
    }

    fn movie_item(path: &str) -> InterpretItem {
        InterpretItem {
            relative_path: path.to_string(),
            file_size: None,
            media_kind: "movies".to_string(),
        }
    }

    #[test]
    fn test_tv_sxxexx() {
        let result = interpret_single(&tv_item(
            "Breaking Bad/Season 01/Breaking.Bad.S01E05.720p.mkv",
        ));
        assert!(result.success);
        assert_eq!(result.show_name.as_deref(), Some("Breaking Bad"));
        assert_eq!(result.season, Some(1));
        assert_eq!(result.episodes, vec![5]);
    }

    #[test]
    fn test_tv_multi_episode() {
        let result = interpret_single(&tv_item("Show/S02E03E04.mkv"));
        assert!(result.success);
        assert_eq!(result.season, Some(2));
        assert_eq!(result.episodes, vec![3, 4]);
    }

    #[test]
    fn test_tv_nxnn() {
        let result = interpret_single(&tv_item("Seinfeld/1x05.avi"));
        assert!(result.success);
        assert_eq!(result.show_name.as_deref(), Some("Seinfeld"));
        assert_eq!(result.season, Some(1));
        assert_eq!(result.episodes, vec![5]);
    }

    #[test]
    fn test_movie_with_year() {
        let result = interpret_single(&movie_item("The Matrix (1999)/The.Matrix.1999.1080p.mkv"));
        assert!(result.success);
        assert_eq!(result.movie_title.as_deref(), Some("The Matrix"));
        assert_eq!(result.movie_year, Some(1999));
    }

    #[test]
    fn test_movie_dotted() {
        let result = interpret_single(&movie_item("Inception.2010.1080p.BluRay.mkv"));
        assert!(result.success);
        assert_eq!(result.movie_title.as_deref(), Some("Inception"));
        assert_eq!(result.movie_year, Some(2010));
    }

    #[test]
    fn test_show_with_year() {
        let result = interpret_single(&tv_item("Doctor Who (2005)/Season 01/S01E01.mkv"));
        assert!(result.success);
        assert_eq!(result.show_name.as_deref(), Some("Doctor Who"));
        assert_eq!(result.show_year, Some(2005));
    }

    fn book_item(path: &str) -> InterpretItem {
        InterpretItem {
            relative_path: path.to_string(),
            file_size: None,
            media_kind: "books".to_string(),
        }
    }

    #[test]
    fn test_book_author_title() {
        let result = interpret_single(&book_item(
            "Arthur C. Clarke/2061 - Odyssey Three/Arthur C. Clarke - 2061 - Odyssey Three.epub",
        ));
        assert!(result.success);
        assert_eq!(result.book_title.as_deref(), Some("2061 - Odyssey Three"));
        assert_eq!(result.book_authors, vec!["Arthur C. Clarke"]);
    }

    #[test]
    fn test_book_title_only() {
        let result = interpret_single(&book_item("Some Book.epub"));
        assert!(result.success);
        assert_eq!(result.book_title.as_deref(), Some("Some Book"));
        assert!(result.book_authors.is_empty());
    }

    #[test]
    fn test_book_strips_suffix() {
        let result = interpret_single(&book_item("Author Name - Great Book (retail).epub"));
        assert!(result.success);
        assert_eq!(result.book_title.as_deref(), Some("Great Book"));
        assert_eq!(result.book_authors, vec!["Author Name"]);
    }

    #[test]
    fn test_book_parent_dir_author() {
        let result = interpret_single(&book_item("Brandon Sanderson/Mistborn.epub"));
        assert!(result.success);
        assert_eq!(result.book_title.as_deref(), Some("Mistborn"));
        assert_eq!(result.book_authors, vec!["Brandon Sanderson"]);
    }
}
