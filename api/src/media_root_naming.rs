//! Simple template engine for media root file naming.
//!
//! Supports `{field}` and `{field:02}` style placeholders where the format
//! specifier after the colon is a zero-padded width (e.g. `:02` pads to 2 digits).

use sqlx::PgPool;
use std::path::PathBuf;
use uuid::Uuid;

/// Convert a DB format string (e.g. "EPUB", "KEPUB", "PDF") to its file extension.
///
/// KEPUB is special: the on-disk extension is `kepub.epub` (compound), which
/// `Path::extension()` cannot round-trip. All other formats lowercase directly.
pub fn format_to_extension(format: &str) -> String {
    match format.to_uppercase().as_str() {
        "KEPUB" => "kepub.epub".to_string(),
        other => other.to_lowercase(),
    }
}

/// Default naming template for TV episodes.
pub fn default_tv_template() -> &'static str {
    "{show_title} ({year})/Season {season:02}/{show_title} - S{season:02}E{episode:02} - {episode_title}.{ext}"
}

/// Default naming template for movies.
pub fn default_movie_template() -> &'static str {
    "{title} ({year})/{title} ({year}).{ext}"
}

/// Default naming template for books.
pub fn default_book_template() -> &'static str {
    "{author}/{title}/{title}.{ext}"
}

/// Default naming template for multi-file audiobooks.
pub fn default_audiobook_template() -> &'static str {
    "{author}/{title}/{title} - {track:03}.{ext}"
}

/// Default naming template for sports events.
pub fn default_sport_template() -> &'static str {
    "{show_title}/{season}/{show_title} - R{round:02} - {episode_title}.{ext}"
}

/// All fields that can be interpolated into a naming template.
#[derive(Debug, Default, Clone)]
pub struct NamingContext {
    /// TV show title
    pub show_title: Option<String>,
    /// Season number
    pub season: Option<u32>,
    /// Episode number
    pub episode: Option<u32>,
    /// Episode title
    pub episode_title: Option<String>,
    /// Movie/general title
    pub title: Option<String>,
    /// Release year
    pub year: Option<u32>,
    /// File extension (without leading dot)
    pub ext: Option<String>,
    /// Book author (first/primary)
    pub author: Option<String>,
    /// Book series name
    pub series: Option<String>,
    /// Book series index (position in series)
    pub series_index: Option<u32>,
    /// Sport round number (extracted from episode sort order)
    pub round: Option<u32>,
    /// Track/file number within a multi-file audiobook
    pub track_number: Option<u32>,
}

impl NamingContext {
    /// Look up a field by name, returning its string representation.
    fn get(&self, field: &str) -> Option<String> {
        match field {
            "show_title" => self.show_title.clone(),
            "season" => self.season.map(|v| v.to_string()),
            "episode" => self.episode.map(|v| v.to_string()),
            "episode_title" => self.episode_title.clone(),
            "title" => self.title.clone(),
            "year" => self.year.map(|v| v.to_string()),
            "ext" => self.ext.clone(),
            "author" => self.author.clone(),
            "series" => self.series.clone(),
            "series_index" => self.series_index.map(|v| v.to_string()),
            "round" => self.round.map(|v| v.to_string()),
            "track" | "track_number" => self.track_number.map(|v| v.to_string()),
            _ => None,
        }
    }

    /// Look up a numeric field by name.
    fn get_numeric(&self, field: &str) -> Option<u32> {
        match field {
            "season" => self.season,
            "episode" => self.episode,
            "year" => self.year,
            "series_index" => self.series_index,
            "round" => self.round,
            "track" | "track_number" => self.track_number,
            _ => None,
        }
    }
}

/// Render a naming template with the given context.
///
/// Placeholders use `{field}` or `{field:02}` syntax. Unknown fields and fields
/// with `None` values are left as-is (e.g. `{unknown}` stays in the output).
pub fn render_template(template: &str, ctx: &NamingContext) -> String {
    let mut result = String::with_capacity(template.len());
    let mut chars = template.chars().peekable();

    while let Some(ch) = chars.next() {
        if ch == '{' {
            // Collect everything up to the closing brace
            let mut placeholder = String::new();
            let mut found_close = false;
            for inner in chars.by_ref() {
                if inner == '}' {
                    found_close = true;
                    break;
                }
                placeholder.push(inner);
            }

            if !found_close {
                // Unterminated brace - emit literally
                result.push('{');
                result.push_str(&placeholder);
                continue;
            }

            // Split on ':' for optional format spec
            let (field, fmt_spec) = match placeholder.split_once(':') {
                Some((f, s)) => (f, Some(s)),
                None => (placeholder.as_str(), None),
            };

            match fmt_spec {
                Some(spec) if spec.starts_with('0') => {
                    // Zero-padded numeric format like "02" or "03"
                    let width: usize = spec[1..].parse().unwrap_or(0);
                    if let Some(num) = ctx.get_numeric(field) {
                        result.push_str(&format!("{num:0>width$}"));
                    } else {
                        // Not a numeric field or missing - emit placeholder unchanged
                        result.push('{');
                        result.push_str(&placeholder);
                        result.push('}');
                    }
                }
                _ => {
                    // Plain substitution (or unrecognised format spec - treat as plain)
                    if let Some(value) = ctx.get(field) {
                        result.push_str(&value);
                    } else {
                        result.push('{');
                        result.push_str(&placeholder);
                        result.push('}');
                    }
                }
            }
        } else {
            result.push(ch);
        }
    }

    result
}

/// Returns true if the string still contains unresolved `{...}` placeholders.
fn has_unresolved(s: &str) -> bool {
    let mut chars = s.chars();
    while let Some(ch) = chars.next() {
        if ch == '{' {
            // Look for a closing brace — if found, it's an unresolved placeholder.
            for inner in chars.by_ref() {
                if inner == '}' {
                    return true;
                }
            }
        }
    }
    false
}

/// Strip unresolved `{...}` placeholders from a string and clean up artifacts
/// (double spaces, orphaned separators).
fn strip_unresolved(s: &str) -> String {
    // Remove all {…} placeholders
    let mut result = String::with_capacity(s.len());
    let mut chars = s.chars().peekable();
    while let Some(ch) = chars.next() {
        if ch == '{' {
            let mut found_close = false;
            for inner in chars.by_ref() {
                if inner == '}' {
                    found_close = true;
                    break;
                }
            }
            if !found_close {
                result.push('{');
            }
        } else {
            result.push(ch);
        }
    }

    // Clean up separators: collapse runs of " - " / " – ", trim, collapse spaces
    let result = result.replace("  ", " ");
    let result = result.replace(" ()", ""); // e.g. "Title ()" from missing year
    let result = result.replace(" []", ""); // e.g. "Title []" from missing bracket field
    let result = result.trim().to_string();
    let result = result.trim_start_matches(" - ").trim_start_matches("- ");
    let result = result.trim_end_matches(" -").trim_end_matches(" - ");
    let result = result.replace(" - .", "."); // e.g. "Title - .mkv" → "Title.mkv"
    let result = result.replace(" .", "."); // e.g. "Title .mkv" → "Title.mkv"
    let result = result.trim_end_matches('.'); // e.g. "Title." → "Title" (missing ext)
    result.trim().to_string()
}

/// Find an existing file path for an entity within a media root.
///
/// For TV shows, finds a file belonging to an episode of the given show.
/// For movies, a file for the given movie.
/// For books, a file of another book by the *same primary author* — the author
/// `{author}` renders — so a co-authored book cannot be filed under its second
/// author's folder.
///
/// Callers replace the entity directory of a freshly computed path with this
/// file's, so the pick is ordered by path: an arbitrary one scatters an
/// entity's files across the directories it already occupies.
/// Returns the full file path of one existing file (status = 'added'), if any.
pub async fn find_existing_file_for_entity(
    pool: &PgPool,
    media_root_id: Uuid,
    entity_type: &str,
    entity_id: Uuid,
) -> Option<String> {
    match entity_type {
        "tv_show" => sqlx::query_scalar!(
            r#"
            SELECT mrf.file_path
            FROM media_root_files mrf
            WHERE mrf.media_root_id = $1
              AND mrf.match_status = 'added'
              AND mrf.entity_type = 'tv_episode'
              AND mrf.entity_id IN (
                  SELECT e.id FROM tv_episodes e
                  JOIN tv_seasons s ON s.id = e.season_id
                  WHERE s.show_id = $2
              )
            ORDER BY mrf.file_path
            LIMIT 1
            "#,
            media_root_id,
            entity_id,
        )
        .fetch_optional(pool)
        .await
        .ok()?,
        "movie" => sqlx::query_scalar!(
            r#"
            SELECT mrf.file_path
            FROM media_root_files mrf
            WHERE mrf.media_root_id = $1
              AND mrf.match_status = 'added'
              AND mrf.entity_type = 'movie'
              AND mrf.entity_id = $2
            ORDER BY mrf.file_path
            LIMIT 1
            "#,
            media_root_id,
            entity_id,
        )
        .fetch_optional(pool)
        .await
        .ok()?,
        "book" => sqlx::query_scalar!(
            r#"
            SELECT mrf.file_path
            FROM media_root_files mrf
            WHERE mrf.media_root_id = $1
              AND mrf.match_status = 'added'
              AND mrf.entity_type = 'book'
              AND mrf.entity_id IN (
                  SELECT ba2.book_id FROM book_authors ba2
                  WHERE ba2.author_id = (
                      SELECT ba.author_id FROM book_authors ba
                      WHERE ba.book_id = $2
                      ORDER BY ba.author_id
                      LIMIT 1
                  )
              )
            ORDER BY mrf.file_path
            LIMIT 1
            "#,
            media_root_id,
            entity_id,
        )
        .fetch_optional(pool)
        .await
        .ok()?,
        _ => None,
    }
}

/// Extract N parent directory levels from a file path, relative to a root.
///
/// For example, with root `/media/tv`, file `/media/tv/Show/Season 01/file.mkv`,
/// and depth 1, returns `PathBuf::from("/media/tv/Show")`.
/// With depth 2, returns `PathBuf::from("/media/tv/Show/Season 01")`.
pub fn extract_dir_from_existing(
    root_path: &str,
    file_path: &str,
    depth: usize,
) -> Option<PathBuf> {
    let relative = file_path.strip_prefix(root_path)?.trim_start_matches('/');
    let components: Vec<&str> = relative.split('/').collect();
    if components.len() <= depth {
        return None;
    }
    let dir_parts: Vec<&str> = components[..depth].to_vec();
    Some(PathBuf::from(root_path).join(dir_parts.join("/")))
}

/// Given a freshly computed target path and an existing entity directory, replace
/// the first `depth` path components (after the root) with the existing directory.
///
/// For example, with root `/tv`, existing dir `/tv/Old Show Name`,
/// fresh path `/tv/New Show Name/Season 01/file.mkv`, and depth 1:
/// returns `/tv/Old Show Name/Season 01/file.mkv`.
pub fn reuse_existing_dir(
    root_path: &str,
    existing_dir: &std::path::Path,
    fresh_path: &std::path::Path,
    depth: usize,
) -> PathBuf {
    let Some(relative) = fresh_path.strip_prefix(root_path).ok() else {
        return fresh_path.to_path_buf();
    };
    let components: Vec<_> = relative.components().collect();
    if components.len() <= depth {
        return fresh_path.to_path_buf();
    }
    let rest: PathBuf = components[depth..].iter().collect();
    existing_dir.join(rest)
}

/// Sanitize a string by replacing/removing characters that are illegal on
/// Windows/SMB/CIFS filesystems (the 8.3 short-name fallback makes files
/// unreadable). Colons and pipes are replaced with hyphens; other unsafe
/// characters are removed. Leading/trailing whitespace is trimmed.
pub(crate) fn sanitize_for_filesystem(s: &str) -> String {
    s.replace(':', " -")
        .replace('|', "-")
        .replace(['?', '*', '"', '<', '>'], "")
        .trim()
        .to_string()
}

/// Sanitize a path segment for use in a file path.
fn sanitize_path_segment(seg: &str) -> String {
    sanitize_for_filesystem(seg)
}

/// Render a template and join it under a root path.
///
/// Segments containing unresolved placeholders have those placeholders stripped
/// and separator artifacts cleaned up. If the cleaned segment is empty, it is
/// omitted from the path. All segments are sanitized to remove filesystem-
/// unfriendly characters (e.g. colons).
pub fn compute_target_path(root_path: &str, template: &str, ctx: &NamingContext) -> PathBuf {
    let rendered = render_template(template, ctx);

    let clean: Vec<String> = rendered
        .split('/')
        .filter_map(|seg| {
            if has_unresolved(seg) {
                let cleaned = strip_unresolved(seg);
                if cleaned.is_empty() {
                    None
                } else {
                    Some(sanitize_path_segment(&cleaned))
                }
            } else {
                Some(sanitize_path_segment(seg))
            }
        })
        .collect();

    let relative = clean.join("/");
    PathBuf::from(root_path).join(relative)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_tv_template_with_year() {
        let ctx = NamingContext {
            show_title: Some("Breaking Bad".into()),
            season: Some(2),
            episode: Some(5),
            episode_title: Some("Breakage".into()),
            year: Some(2008),
            ext: Some("mkv".into()),
            ..Default::default()
        };
        let out = render_template(default_tv_template(), &ctx);
        assert_eq!(
            out,
            "Breaking Bad (2008)/Season 02/Breaking Bad - S02E05 - Breakage.mkv"
        );
    }

    #[test]
    fn test_movie_template() {
        let ctx = NamingContext {
            title: Some("Inception".into()),
            year: Some(2010),
            ext: Some("mp4".into()),
            ..Default::default()
        };
        let out = render_template(default_movie_template(), &ctx);
        assert_eq!(out, "Inception (2010)/Inception (2010).mp4");
    }

    #[test]
    fn test_compute_target_path() {
        let ctx = NamingContext {
            title: Some("Dune".into()),
            year: Some(2021),
            ext: Some("mkv".into()),
            ..Default::default()
        };
        let path = compute_target_path("/media/movies", default_movie_template(), &ctx);
        assert_eq!(
            path,
            PathBuf::from("/media/movies/Dune (2021)/Dune (2021).mkv")
        );
    }

    #[test]
    fn test_missing_field_preserved() {
        let ctx = NamingContext::default();
        let out = render_template("{title} ({year})", &ctx);
        assert_eq!(out, "{title} ({year})");
    }

    #[test]
    fn test_zero_padding() {
        let ctx = NamingContext {
            season: Some(1),
            episode: Some(9),
            ..Default::default()
        };
        assert_eq!(render_template("S{season:02}E{episode:02}", &ctx), "S01E09");
        assert_eq!(
            render_template("S{season:03}E{episode:04}", &ctx),
            "S001E0009"
        );
    }

    #[test]
    fn test_unterminated_brace() {
        let ctx = NamingContext::default();
        let out = render_template("hello {world", &ctx);
        assert_eq!(out, "hello {world");
    }

    #[test]
    fn test_standalone_book_strips_series_segment() {
        let ctx = NamingContext {
            title: Some("Project Hail Mary".into()),
            author: Some("Andy Weir".into()),
            ext: Some("epub".into()),
            // No series or series_index
            ..Default::default()
        };
        let path = compute_target_path("/books", "{author}/{series}/{title}.{ext}", &ctx);
        assert_eq!(
            path,
            PathBuf::from("/books/Andy Weir/Project Hail Mary.epub")
        );
    }

    #[test]
    fn test_series_book_keeps_series_segment() {
        let ctx = NamingContext {
            title: Some("The Way of Kings".into()),
            author: Some("Brandon Sanderson".into()),
            series: Some("Stormlight Archive".into()),
            series_index: Some(1),
            ext: Some("epub".into()),
            ..Default::default()
        };
        let path = compute_target_path("/books", "{author}/{series}/{title}.{ext}", &ctx);
        assert_eq!(
            path,
            PathBuf::from("/books/Brandon Sanderson/Stormlight Archive/The Way of Kings.epub")
        );
    }

    #[test]
    fn test_episode_without_title_cleans_filename() {
        let ctx = NamingContext {
            show_title: Some("Lost".into()),
            season: Some(1),
            episode: Some(3),
            year: Some(2004),
            // No episode_title
            ext: Some("mkv".into()),
            ..Default::default()
        };
        let path = compute_target_path("/tv", default_tv_template(), &ctx);
        assert_eq!(
            path,
            PathBuf::from("/tv/Lost (2004)/Season 01/Lost - S01E03.mkv")
        );
    }

    #[test]
    fn test_episode_without_year_strips_placeholder() {
        let ctx = NamingContext {
            show_title: Some("Lost".into()),
            season: Some(1),
            episode: Some(3),
            // No year — strip_unresolved cleans "Lost ({year})" → "Lost"
            ext: Some("mkv".into()),
            ..Default::default()
        };
        let path = compute_target_path("/tv", default_tv_template(), &ctx);
        assert_eq!(path, PathBuf::from("/tv/Lost/Season 01/Lost - S01E03.mkv"));
    }

    #[test]
    fn test_has_unresolved() {
        assert!(has_unresolved("{foo}"));
        assert!(has_unresolved("prefix {foo} suffix"));
        assert!(!has_unresolved("no braces"));
        assert!(!has_unresolved("open { but no close"));
        assert!(!has_unresolved("resolved text"));
    }

    #[test]
    fn test_strip_unresolved() {
        assert_eq!(strip_unresolved("{title}.mkv"), ".mkv");
        assert_eq!(
            strip_unresolved("Show - S01E03 - {episode_title}.mkv"),
            "Show - S01E03.mkv"
        );
        assert_eq!(strip_unresolved("{a} {b}"), "");
        // Missing extension leaves trailing dot
        assert_eq!(
            strip_unresolved("The Way of Kings.{ext}"),
            "The Way of Kings"
        );
        // Empty parens from missing year placeholder
        assert_eq!(strip_unresolved("Scrubs ({year})"), "Scrubs");
    }

    #[test]
    fn test_standalone_book_mixed_segment() {
        // User's exact template: optional series_index mixed with required title
        let ctx = NamingContext {
            title: Some("Project Hail Mary".into()),
            author: Some("Andy Weir".into()),
            ext: Some("epub".into()),
            ..Default::default()
        };
        let path = compute_target_path(
            "/books",
            "{author}/{series}/{series_index:02} - {title}/{title}.{ext}",
            &ctx,
        );
        // {series} segment is empty after strip → dropped
        // {series_index:02} - Project Hail Mary → stripped to "Project Hail Mary" → kept
        assert_eq!(
            path,
            PathBuf::from("/books/Andy Weir/Project Hail Mary/Project Hail Mary.epub")
        );
    }

    #[test]
    fn test_series_book_mixed_segment() {
        let ctx = NamingContext {
            title: Some("The Way of Kings".into()),
            author: Some("Brandon Sanderson".into()),
            series: Some("Stormlight Archive".into()),
            series_index: Some(1),
            ext: Some("epub".into()),
            ..Default::default()
        };
        let path = compute_target_path(
            "/books",
            "{author}/{series}/{series_index:02} - {title}/{title}.{ext}",
            &ctx,
        );
        assert_eq!(
            path,
            PathBuf::from(
                "/books/Brandon Sanderson/Stormlight Archive/01 - The Way of Kings/The Way of Kings.epub"
            )
        );
    }

    #[test]
    fn test_book_missing_extension() {
        let ctx = NamingContext {
            title: Some("The Way of Kings".into()),
            author: Some("Brandon Sanderson".into()),
            series: Some("The Stormlight Archive".into()),
            series_index: Some(1),
            // No ext — file had no extension
            ..Default::default()
        };
        let path = compute_target_path(
            "/books",
            "{author}/{series}/{series_index:02} - {title}/{title}.{ext}",
            &ctx,
        );
        // Should NOT produce a trailing dot
        assert_eq!(
            path,
            PathBuf::from(
                "/books/Brandon Sanderson/The Stormlight Archive/01 - The Way of Kings/The Way of Kings"
            )
        );
    }

    #[test]
    fn test_colon_sanitized_in_path() {
        let ctx = NamingContext {
            show_title: Some("Law & Order Toronto: Criminal Intent".into()),
            season: Some(1),
            episode: Some(3),
            episode_title: Some("Proof of Concept".into()),
            year: Some(2024),
            ext: Some("mkv".into()),
            ..Default::default()
        };
        let path = compute_target_path("/tv", default_tv_template(), &ctx);
        assert_eq!(
            path,
            PathBuf::from(
                "/tv/Law & Order Toronto - Criminal Intent (2024)/Season 01/Law & Order Toronto - Criminal Intent - S01E03 - Proof of Concept.mkv"
            )
        );
    }

    #[test]
    fn test_audiobook_template_multi_file() {
        for (track, expected_suffix) in [(1, "001"), (2, "002"), (12, "012")] {
            let ctx = NamingContext {
                title: Some("Alcatraz Versus the Evil Librarians".into()),
                author: Some("Brandon Sanderson".into()),
                track_number: Some(track),
                ext: Some("mp3".into()),
                ..Default::default()
            };
            let path = compute_target_path("/audiobooks", default_audiobook_template(), &ctx);
            assert_eq!(
                path,
                PathBuf::from(format!(
                    "/audiobooks/Brandon Sanderson/Alcatraz Versus the Evil Librarians/Alcatraz Versus the Evil Librarians - {expected_suffix}.mp3"
                ))
            );
        }
    }

    #[test]
    fn test_audiobook_template_single_file_no_track() {
        let ctx = NamingContext {
            title: Some("Project Hail Mary".into()),
            author: Some("Andy Weir".into()),
            ext: Some("m4b".into()),
            // No track_number — single-file audiobook
            ..Default::default()
        };
        let path = compute_target_path("/audiobooks", default_audiobook_template(), &ctx);
        assert_eq!(
            path,
            PathBuf::from("/audiobooks/Andy Weir/Project Hail Mary/Project Hail Mary.m4b")
        );
    }

    #[test]
    fn test_smb_unsafe_chars_sanitized_in_path() {
        // Question mark — the original bug (Invincible "WHAT HAVE I DONE?")
        let ctx = NamingContext {
            show_title: Some("Invincible".into()),
            season: Some(2),
            episode: Some(4),
            episode_title: Some("IT'S NOT THAT SIMPLE".into()),
            year: Some(2023),
            ext: Some("mkv".into()),
            ..Default::default()
        };
        let path = compute_target_path("/tv", default_tv_template(), &ctx);
        assert!(
            !path.to_string_lossy().contains('?'),
            "path must not contain '?': {path:?}"
        );

        // All SMB-unsafe characters in a title
        let ctx = NamingContext {
            show_title: Some(r#"Test: "Show" <With> *All* |Bad| Chars?"#.into()),
            season: Some(1),
            episode: Some(1),
            episode_title: Some("Pilot".into()),
            year: Some(2024),
            ext: Some("mkv".into()),
            ..Default::default()
        };
        let path = compute_target_path("/tv", default_tv_template(), &ctx);
        let path_str = path.to_string_lossy();
        for ch in ['?', '*', '"', '<', '>', '|'] {
            assert!(
                !path_str.contains(ch),
                "path must not contain '{ch}': {path_str}"
            );
        }
        // Colon should still be replaced with " -"
        assert!(
            !path_str.contains(':'),
            "path must not contain ':': {path_str}"
        );
    }

    #[test]
    fn test_sanitize_path_segment_smb_chars() {
        assert_eq!(
            sanitize_path_segment("WHAT HAVE I DONE?"),
            "WHAT HAVE I DONE"
        );
        assert_eq!(sanitize_path_segment("Star*Wars"), "StarWars");
        assert_eq!(sanitize_path_segment(r#"He said "hello""#), "He said hello");
        assert_eq!(sanitize_path_segment("<intro>"), "intro");
        assert_eq!(sanitize_path_segment("foo|bar"), "foo-bar");
        assert_eq!(sanitize_path_segment("Title: Subtitle"), "Title - Subtitle");
    }

    #[test]
    fn test_audiobook_distinct_paths_per_track() {
        let paths: Vec<PathBuf> = (1..=3)
            .map(|track| {
                let ctx = NamingContext {
                    title: Some("Dune".into()),
                    author: Some("Frank Herbert".into()),
                    track_number: Some(track),
                    ext: Some("mp3".into()),
                    ..Default::default()
                };
                compute_target_path("/audio", default_audiobook_template(), &ctx)
            })
            .collect();
        // All paths must be distinct
        assert_ne!(paths[0], paths[1]);
        assert_ne!(paths[1], paths[2]);
        assert_ne!(paths[0], paths[2]);
    }
}
