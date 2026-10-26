//! Audio metadata and chapter extraction via kob-ffmpeg.
//!
//! Opens audio files with kob-ffmpeg's InputContext and reads format-level
//! metadata tags for audiobook-specific fields (narrator, ASIN, series, etc.).

use std::collections::HashMap;
use std::path::Path;

/// Metadata extracted from an audiobook file.
#[derive(Debug, Clone)]
pub struct AudioProbeResult {
    pub duration_seconds: Option<f64>,
    pub bit_rate: Option<i64>,
    /// Narrator, extracted from artist / album_artist / composer / narrator tags.
    pub narrator: Option<String>,
    pub title: Option<String>,
    /// Album tag — for multi-file audiobooks this is typically the book title
    /// (while `title` is the chapter/track title).
    pub album: Option<String>,
    /// Author, extracted from author / album_artist / artist tags (distinct from narrator).
    pub author: Option<String>,
    /// Whether the audiobook is abridged, inferred from comment/custom tags.
    pub is_abridged: Option<bool>,
    /// ASIN (Amazon Standard Identification Number), common in Audible M4B files.
    pub asin: Option<String>,
    /// Series name, from series / grouping / "TXXX:SERIES" tags.
    pub series: Option<String>,
    /// Series position, from series-part / movement / disc tags.
    pub series_index: Option<f64>,
    /// Genre/subject tags.
    pub genre: Option<String>,
    /// Publication year, from date / year tags.
    pub year: Option<String>,
    /// Publisher, from publisher / label tags.
    pub publisher: Option<String>,
    /// Language, from language tag.
    pub language: Option<String>,
    /// Description/comment, from description / comment / synopsis tags.
    pub description: Option<String>,
    /// ISBN from tags.
    pub isbn: Option<String>,
    pub chapters: Vec<ProbeChapter>,
}

/// A single chapter from the file's embedded chapter markers.
#[derive(Debug, Clone)]
pub struct ProbeChapter {
    pub index: i32,
    pub title: Option<String>,
    pub start_seconds: f64,
    pub end_seconds: f64,
}

/// Errors from audio probing.
#[derive(Debug, thiserror::Error)]
pub enum AudioProbeError {
    #[error("I/O error running ffprobe: {0}")]
    IoError(#[from] std::io::Error),
    #[error("failed to parse ffprobe JSON output: {0}")]
    ParseError(String),
    #[error("ffprobe failed with exit code {code}: {stderr}")]
    ProbeError { code: i32, stderr: String },
    #[error("ffmpeg error: {0}")]
    Ffmpeg(#[from] kob_ffmpeg::FfmpegError),
}

impl From<tokio::task::JoinError> for AudioProbeError {
    fn from(e: tokio::task::JoinError) -> Self {
        AudioProbeError::IoError(std::io::Error::other(e))
    }
}

/// Probe an audio file for metadata and chapters.
pub async fn probe_audio(path: &Path) -> Result<AudioProbeResult, AudioProbeError> {
    let path = path.to_owned();
    tokio::task::spawn_blocking(move || probe_audio_sync(&path)).await?
}

/// Synchronous audio probe implementation using kob-ffmpeg.
fn probe_audio_sync(path: &Path) -> Result<AudioProbeResult, AudioProbeError> {
    let mut ctx = kob_ffmpeg::format::input::InputContext::open_path(path)?;
    ctx.find_stream_info()?;

    let duration_seconds = ctx.duration_secs();

    // Collect format-level metadata into a HashMap for tag lookup
    let tags: HashMap<String, String> = ctx.metadata().iter().collect();

    // Extract chapters
    let chapters: Vec<ProbeChapter> = ctx
        .chapters()
        .enumerate()
        .map(|(i, ch)| ProbeChapter {
            index: i as i32,
            title: ch.title(),
            start_seconds: ch.start_secs(),
            end_seconds: ch.end_secs(),
        })
        .collect();

    Ok(AudioProbeResult {
        bit_rate: ctx.bit_rate(),
        ..parse_audio_tags(tags, duration_seconds, chapters)
    })
}

fn parse_audio_tags(
    tags: HashMap<String, String>,
    duration_seconds: Option<f64>,
    chapters: Vec<ProbeChapter>,
) -> AudioProbeResult {
    let album = get_tag(&tags, "album");
    let title = get_tag(&tags, "title").or_else(|| album.clone());

    // Author: prefer explicit author tag, fall back to album_artist
    let author = get_tag(&tags, "author")
        .or_else(|| get_tag(&tags, "album_artist"))
        .or_else(|| get_tag(&tags, "TXXX:AUTHOR"));

    // Narrator: prefer explicit narrator tag, then artist/composer
    // Skip artist if it matches the author
    let narrator = get_tag(&tags, "narrator")
        .or_else(|| get_tag(&tags, "TXXX:NARRATOR"))
        .or_else(|| {
            let artist = get_tag(&tags, "artist")?;
            if author
                .as_ref()
                .is_some_and(|a| a.eq_ignore_ascii_case(&artist))
            {
                None
            } else {
                Some(artist)
            }
        })
        .or_else(|| {
            let comp = get_tag(&tags, "composer")?;
            if author
                .as_ref()
                .is_some_and(|a| a.eq_ignore_ascii_case(&comp))
            {
                None
            } else {
                Some(comp)
            }
        });

    let is_abridged = detect_abridged(&tags);

    let asin = get_tag(&tags, "asin")
        .or_else(|| get_tag(&tags, "TXXX:ASIN"))
        .or_else(|| get_tag(&tags, "----:com.audible.info:asin"))
        .filter(|v| v.len() == 10 && v.chars().all(|c| c.is_ascii_alphanumeric()));

    let series = get_tag(&tags, "series")
        .or_else(|| get_tag(&tags, "TXXX:SERIES"))
        .or_else(|| get_tag(&tags, "grouping"))
        .or_else(|| get_tag(&tags, "MVNM"))
        .or_else(|| get_tag(&tags, "----:com.audible.info:series"));

    let series_index = get_tag(&tags, "series-part")
        .or_else(|| get_tag(&tags, "TXXX:SERIES-PART"))
        .or_else(|| get_tag(&tags, "movement"))
        .or_else(|| get_tag(&tags, "MVIN"))
        .or_else(|| get_tag(&tags, "disc"))
        .and_then(|s| s.parse::<f64>().ok());

    let genre = get_tag(&tags, "genre");

    let year = get_tag(&tags, "date")
        .or_else(|| get_tag(&tags, "year"))
        .map(|d| if d.len() >= 4 { d[..4].to_string() } else { d });

    let publisher = get_tag(&tags, "publisher")
        .or_else(|| get_tag(&tags, "label"))
        .or_else(|| get_tag(&tags, "TXXX:PUBLISHER"));

    let language = get_tag(&tags, "language");

    let description = get_tag(&tags, "description")
        .or_else(|| get_tag(&tags, "synopsis"))
        .or_else(|| get_tag(&tags, "comment"))
        .filter(|d| d.len() > 20);

    let isbn = get_tag(&tags, "isbn")
        .or_else(|| get_tag(&tags, "TXXX:ISBN"))
        .filter(|v| v.len() >= 10 && v.len() <= 17);

    AudioProbeResult {
        duration_seconds,
        bit_rate: None,
        narrator,
        title,
        album,
        author,
        is_abridged,
        asin,
        series,
        series_index,
        genre,
        year,
        publisher,
        language,
        description,
        isbn,
        chapters,
    }
}

/// Case-insensitive tag lookup.
fn get_tag(tags: &HashMap<String, String>, key: &str) -> Option<String> {
    tags.iter()
        .find(|(k, _)| k.eq_ignore_ascii_case(key))
        .map(|(_, v)| v.clone())
        .filter(|v| !v.is_empty())
}

/// Check if any tag/comment mentions "abridged".
fn detect_abridged(tags: &HashMap<String, String>) -> Option<bool> {
    let haystack: Vec<&str> = ["comment", "description", "genre"]
        .iter()
        .filter_map(|key| {
            tags.iter()
                .find(|(k, _)| k.eq_ignore_ascii_case(key))
                .map(|(_, v)| v.as_str())
        })
        .collect();

    if haystack.is_empty() {
        return None;
    }

    let text = haystack.join(" ").to_lowercase();
    if text.contains("unabridged") {
        Some(false)
    } else if text.contains("abridged") {
        Some(true)
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_audio_tags() {
        let mut tags = HashMap::new();
        tags.insert("author".to_string(), "John Author".to_string());
        tags.insert("artist".to_string(), "Jane Narrator".to_string());
        tags.insert("title".to_string(), "The Great Novel".to_string());
        tags.insert("comment".to_string(), "Unabridged edition".to_string());
        tags.insert("asin".to_string(), "B08C6YJ1LS".to_string());
        tags.insert("series".to_string(), "Great Series".to_string());
        tags.insert("series-part".to_string(), "3".to_string());
        tags.insert("genre".to_string(), "Fiction".to_string());
        tags.insert("date".to_string(), "2023-01-15".to_string());
        tags.insert("publisher".to_string(), "Penguin Audio".to_string());

        let result = parse_audio_tags(tags, Some(36542.123), vec![]);

        assert!((result.duration_seconds.unwrap() - 36542.123).abs() < 0.001);
        assert_eq!(result.author.as_deref(), Some("John Author"));
        assert_eq!(result.narrator.as_deref(), Some("Jane Narrator"));
        assert_eq!(result.title.as_deref(), Some("The Great Novel"));
        assert_eq!(result.is_abridged, Some(false));
        assert_eq!(result.asin.as_deref(), Some("B08C6YJ1LS"));
        assert_eq!(result.series.as_deref(), Some("Great Series"));
        assert_eq!(result.series_index, Some(3.0));
        assert_eq!(result.genre.as_deref(), Some("Fiction"));
        assert_eq!(result.year.as_deref(), Some("2023"));
        assert_eq!(result.publisher.as_deref(), Some("Penguin Audio"));
    }

    #[test]
    fn test_author_narrator_separation() {
        let mut tags = HashMap::new();
        tags.insert("album_artist".to_string(), "The Author".to_string());
        tags.insert("artist".to_string(), "The Narrator".to_string());
        tags.insert("title".to_string(), "Some Book".to_string());

        let result = parse_audio_tags(tags, Some(100.0), vec![]);
        assert_eq!(result.author.as_deref(), Some("The Author"));
        assert_eq!(result.narrator.as_deref(), Some("The Narrator"));
    }

    #[test]
    fn test_narrator_skipped_when_matches_author() {
        let mut tags = HashMap::new();
        tags.insert("album_artist".to_string(), "Same Person".to_string());
        tags.insert("artist".to_string(), "Same Person".to_string());
        tags.insert("title".to_string(), "Some Book".to_string());

        let result = parse_audio_tags(tags, Some(100.0), vec![]);
        assert_eq!(result.author.as_deref(), Some("Same Person"));
        assert_eq!(result.narrator, None);
    }

    #[test]
    fn test_abridged_detection() {
        let mut tags = HashMap::new();
        tags.insert(
            "comment".to_string(),
            "This is an abridged version".to_string(),
        );
        assert_eq!(detect_abridged(&tags), Some(true));

        tags.clear();
        tags.insert("comment".to_string(), "Unabridged audiobook".to_string());
        assert_eq!(detect_abridged(&tags), Some(false));

        tags.clear();
        tags.insert("comment".to_string(), "Great book".to_string());
        assert_eq!(detect_abridged(&tags), None);

        tags.clear();
        assert_eq!(detect_abridged(&tags), None);
    }

    #[test]
    fn test_case_insensitive_tags() {
        let mut tags = HashMap::new();
        tags.insert("ARTIST".to_string(), "Narrator Name".to_string());
        assert_eq!(get_tag(&tags, "artist"), Some("Narrator Name".to_string()));
    }

    #[test]
    fn test_no_chapters() {
        let result = parse_audio_tags(HashMap::new(), Some(3600.0), vec![]);
        assert!(result.chapters.is_empty());
        assert!(result.narrator.is_none());
    }
}
