//! Audiobookshelf `metadata.json` parser and writer.
//!
//! Parses the JSON sidecar format used by Audiobookshelf, which stores book
//! metadata alongside audiobook/ebook files.

use super::{BookSidecar, SidecarChapter, SidecarFormat, SidecarIdentifiers, SidecarMetadata};
use serde::{Deserialize, Serialize};
use std::path::Path;

/// Audiobookshelf metadata.json structure.
#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct AudiobookshelfMetadata {
    #[serde(default)]
    title: Option<String>,
    #[serde(default)]
    subtitle: Option<String>,
    #[serde(default)]
    authors: Vec<String>,
    #[serde(default)]
    narrators: Vec<String>,
    #[serde(default)]
    series: Vec<SeriesEntry>,
    #[serde(default)]
    genres: Vec<String>,
    #[serde(default)]
    tags: Vec<String>,
    #[serde(default)]
    published_year: Option<String>,
    #[serde(default)]
    published_date: Option<String>,
    #[serde(default)]
    publisher: Option<String>,
    #[serde(default)]
    description: Option<String>,
    #[serde(default)]
    isbn: Option<String>,
    #[serde(default)]
    asin: Option<String>,
    #[serde(default)]
    language: Option<String>,
    #[serde(default)]
    explicit: bool,
    #[serde(default)]
    abridged: bool,
    #[serde(default)]
    chapters: Vec<serde_json::Value>,
}

#[derive(Debug, Serialize, Deserialize)]
struct SeriesEntry {
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    sequence: Option<String>,
}

/// Parse an Audiobookshelf `metadata.json` string into `SidecarMetadata`.
pub fn parse_metadata_json(content: &str) -> Result<SidecarMetadata, String> {
    let abs: AudiobookshelfMetadata =
        serde_json::from_str(content).map_err(|e| format!("JSON parse error: {e}"))?;

    let year = abs
        .published_year
        .as_deref()
        .and_then(|y| y.parse::<u16>().ok());

    let (series_name, series_index) = if let Some(first) = abs.series.first() {
        let idx = first
            .sequence
            .as_deref()
            .and_then(|s| s.parse::<f64>().ok());
        (first.name.clone(), idx)
    } else {
        (None, None)
    };

    let mut identifiers = SidecarIdentifiers::default();
    if let Some(isbn) = &abs.isbn
        && !isbn.is_empty()
    {
        identifiers.isbn = Some(isbn.clone());
    }
    if let Some(asin) = &abs.asin
        && !asin.is_empty()
    {
        identifiers.asin = Some(asin.clone());
    }

    // Read kobblerId from the raw JSON (not in AudiobookshelfMetadata struct)
    if let Ok(raw) = serde_json::from_str::<serde_json::Value>(content)
        && let Some(kid) = raw.get("kobblerId").and_then(|v| v.as_str())
        && !kid.is_empty()
    {
        identifiers.kobbler_id = Some(kid.to_string());
    }

    Ok(SidecarMetadata {
        source_format: Some(SidecarFormat::AudiobookshelfJson),
        title: abs.title,
        year,
        plot: abs.description.clone(),
        genres: abs.genres,
        identifiers,
        book: Some(BookSidecar {
            authors: abs.authors,
            narrators: abs.narrators,
            publisher: abs.publisher,
            language: abs.language,
            series: series_name,
            series_index,
            description: abs.description,
            abridged: abs.abridged,
            chapters: parse_abs_chapters(&abs.chapters),
        }),
        ..Default::default()
    })
}

/// Parse Audiobookshelf chapter JSON values into `SidecarChapter` structs.
///
/// ABS chapters use `{ "start": 0, "end": 1234.5, "title": "Ch 1" }` format.
fn parse_abs_chapters(chapters: &[serde_json::Value]) -> Vec<SidecarChapter> {
    chapters
        .iter()
        .filter_map(|ch| {
            let start = ch.get("start").and_then(|v| v.as_f64())?;
            let end = ch.get("end").and_then(|v| v.as_f64())?;
            let title = ch.get("title").and_then(|v| v.as_str()).map(String::from);
            Some(SidecarChapter { title, start, end })
        })
        .collect()
}

/// Convert `SidecarChapter` structs back to Audiobookshelf chapter JSON values.
fn to_abs_chapters(chapters: &[SidecarChapter]) -> Vec<serde_json::Value> {
    chapters
        .iter()
        .enumerate()
        .map(|(i, ch)| {
            let title = ch
                .title
                .clone()
                .unwrap_or_else(|| format!("Chapter {}", i + 1));
            serde_json::json!({
                "id": i,
                "start": ch.start,
                "end": ch.end,
                "title": title,
            })
        })
        .collect()
}

/// Write `SidecarMetadata` as an Audiobookshelf `metadata.json` file.
pub async fn write_metadata_json(
    path: &Path,
    meta: &SidecarMetadata,
) -> Result<(), std::io::Error> {
    let book = meta.book.as_ref();

    let series = book
        .and_then(|b| b.series.as_ref())
        .map(|name| {
            vec![SeriesEntry {
                name: Some(name.clone()),
                sequence: book.and_then(|b| b.series_index.map(|i| i.to_string())),
            }]
        })
        .unwrap_or_default();

    let abs = AudiobookshelfMetadata {
        title: meta.title.clone(),
        subtitle: None,
        authors: book.map(|b| b.authors.clone()).unwrap_or_default(),
        narrators: book.map(|b| b.narrators.clone()).unwrap_or_default(),
        series,
        genres: meta.genres.clone(),
        tags: Vec::new(),
        published_year: meta.year.map(|y| y.to_string()),
        published_date: None,
        publisher: book.and_then(|b| b.publisher.clone()),
        description: book
            .and_then(|b| b.description.clone())
            .or_else(|| meta.plot.clone()),
        isbn: meta.identifiers.isbn.clone(),
        asin: meta.identifiers.asin.clone(),
        language: book.and_then(|b| b.language.clone()),
        explicit: false,
        abridged: book.is_some_and(|b| b.abridged),
        chapters: book
            .map(|b| to_abs_chapters(&b.chapters))
            .unwrap_or_default(),
    };

    let json = serde_json::to_string_pretty(&abs).map_err(std::io::Error::other)?;
    let path = path.to_owned();
    tokio::task::spawn_blocking(move || std::fs::write(&path, json))
        .await
        .map_err(|e| std::io::Error::other(format!("Join error: {e}")))?
}

/// Write `SidecarMetadata` as a merged Audiobookshelf `metadata.json` file.
///
/// Reads the existing file (if any), overlays managed fields only when our value
/// is present, and preserves any unknown keys (narrators, tags, chapters, custom).
pub async fn write_metadata_json_merged(
    path: &Path,
    meta: &SidecarMetadata,
) -> Result<(), std::io::Error> {
    // Read existing file or start with empty object
    let read_path = path.to_owned();
    let mut existing: serde_json::Value = tokio::task::spawn_blocking(move || {
        if !read_path.exists() {
            return serde_json::json!({});
        }
        match std::fs::read_to_string(&read_path) {
            Ok(content) => serde_json::from_str(&content).unwrap_or(serde_json::json!({})),
            Err(_) => serde_json::json!({}),
        }
    })
    .await
    .unwrap_or(serde_json::json!({}));

    let obj = existing
        .as_object_mut()
        .ok_or_else(|| std::io::Error::other("existing metadata.json is not a JSON object"))?;

    let book = meta.book.as_ref();

    // Overlay managed fields only when our value is present
    if let Some(title) = &meta.title {
        obj.insert("title".to_string(), serde_json::json!(title));
    }
    if let Some(b) = book {
        if !b.authors.is_empty() {
            obj.insert("authors".to_string(), serde_json::json!(b.authors));
        }
        if !b.narrators.is_empty() {
            obj.insert("narrators".to_string(), serde_json::json!(b.narrators));
        }
        if let Some(pub_) = &b.publisher {
            obj.insert("publisher".to_string(), serde_json::json!(pub_));
        }
        if let Some(lang) = &b.language {
            obj.insert("language".to_string(), serde_json::json!(lang));
        }
        if let Some(desc) = &b.description {
            obj.insert("description".to_string(), serde_json::json!(desc));
        }
        if b.series.is_some() {
            let series = vec![serde_json::json!({
                "name": b.series,
                "sequence": b.series_index.map(|i| i.to_string()),
            })];
            obj.insert("series".to_string(), serde_json::json!(series));
        }
        if b.abridged {
            obj.insert("abridged".to_string(), serde_json::json!(true));
        }
        if !b.chapters.is_empty() {
            obj.insert(
                "chapters".to_string(),
                serde_json::json!(to_abs_chapters(&b.chapters)),
            );
        }
    }
    if let Some(year) = meta.year {
        obj.insert(
            "publishedYear".to_string(),
            serde_json::json!(year.to_string()),
        );
    }
    if !meta.genres.is_empty() {
        obj.insert("genres".to_string(), serde_json::json!(meta.genres));
    }
    if let Some(isbn) = &meta.identifiers.isbn {
        obj.insert("isbn".to_string(), serde_json::json!(isbn));
    }
    if let Some(asin) = &meta.identifiers.asin {
        obj.insert("asin".to_string(), serde_json::json!(asin));
    }

    // Always set kobblerId
    if let Some(kid) = &meta.identifiers.kobbler_id {
        obj.insert("kobblerId".to_string(), serde_json::json!(kid));
    }

    let json = serde_json::to_string_pretty(&existing).map_err(std::io::Error::other)?;
    let path = path.to_owned();
    tokio::task::spawn_blocking(move || std::fs::write(&path, json))
        .await
        .map_err(|e| std::io::Error::other(format!("Join error: {e}")))?
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_audiobookshelf_metadata() {
        let json = include_str!("test_fixtures/book_metadata.json");
        let meta = parse_metadata_json(json).expect("Failed to parse metadata.json");

        assert_eq!(meta.title.as_deref(), Some("Richter 10"));
        assert_eq!(meta.year, Some(2012));
        assert_eq!(meta.identifiers.isbn.as_deref(), Some("978-0-7953-2578-6"));

        let book = meta.book.expect("Should have book metadata");
        assert_eq!(book.authors, vec!["Arthur C. Clarke"]);
        assert_eq!(book.publisher.as_deref(), Some("RosettaBooks"));
        assert_eq!(book.language.as_deref(), Some("en-US"));
    }

    #[test]
    fn test_parse_metadata_with_series() {
        let json = r#"{
            "title": "Test Book",
            "authors": ["Author One"],
            "series": [{"name": "My Series", "sequence": "3"}],
            "genres": ["Fiction"],
            "isbn": "978-1234567890",
            "language": "en"
        }"#;
        let meta = parse_metadata_json(json).expect("Failed to parse");

        let book = meta.book.expect("Should have book metadata");
        assert_eq!(book.series.as_deref(), Some("My Series"));
        assert_eq!(book.series_index, Some(3.0));
    }

    #[test]
    fn test_parse_minimal_metadata() {
        let json = r#"{"title": "Minimal"}"#;
        let meta = parse_metadata_json(json).expect("Failed to parse minimal");
        assert_eq!(meta.title.as_deref(), Some("Minimal"));
    }

    #[test]
    fn test_roundtrip_metadata_json() {
        let original = SidecarMetadata {
            source_format: Some(SidecarFormat::AudiobookshelfJson),
            title: Some("Test Book".to_string()),
            year: Some(2024),
            genres: vec!["Fiction".to_string()],
            identifiers: SidecarIdentifiers {
                isbn: Some("978-1234567890".to_string()),
                ..Default::default()
            },
            book: Some(BookSidecar {
                authors: vec!["Author".to_string()],
                publisher: Some("Publisher".to_string()),
                language: Some("en".to_string()),
                series: Some("Series".to_string()),
                series_index: Some(2.0),
                description: Some("A description".to_string()),
                ..Default::default()
            }),
            ..Default::default()
        };

        let json = serde_json::to_string_pretty(&{
            let book = original.book.as_ref().unwrap();
            AudiobookshelfMetadata {
                title: original.title.clone(),
                subtitle: None,
                authors: book.authors.clone(),
                narrators: book.narrators.clone(),
                series: vec![SeriesEntry {
                    name: book.series.clone(),
                    sequence: book.series_index.map(|i| i.to_string()),
                }],
                genres: original.genres.clone(),
                tags: Vec::new(),
                published_year: original.year.map(|y| y.to_string()),
                published_date: None,
                publisher: book.publisher.clone(),
                description: book.description.clone(),
                isbn: original.identifiers.isbn.clone(),
                asin: None,
                language: book.language.clone(),
                explicit: false,
                abridged: false,
                chapters: Vec::new(),
            }
        })
        .unwrap();

        let parsed = parse_metadata_json(&json).expect("Failed to parse written JSON");
        assert_eq!(parsed.title, original.title);
        assert_eq!(parsed.year, original.year);
        assert_eq!(parsed.identifiers.isbn, original.identifiers.isbn);
        let book = parsed.book.expect("Should have book");
        assert_eq!(book.authors, vec!["Author"]);
        assert_eq!(book.series.as_deref(), Some("Series"));
        assert_eq!(book.series_index, Some(2.0));
    }

    #[test]
    fn test_parse_kobbler_id() {
        let json = r#"{"title": "Test", "kobblerId": "abc-123"}"#;
        let meta = parse_metadata_json(json).expect("Failed to parse");
        assert_eq!(meta.identifiers.kobbler_id.as_deref(), Some("abc-123"));
    }

    #[tokio::test]
    async fn test_write_metadata_json_merged_preserves_unknown_keys() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("metadata.json");

        // Write an existing file with narrators, tags, chapters, custom field
        let existing = serde_json::json!({
            "title": "Old Title",
            "authors": ["Old Author"],
            "narrators": ["Narrator One", "Narrator Two"],
            "tags": ["tag1", "tag2"],
            "chapters": [{"start": 0, "end": 100, "title": "Ch 1"}],
            "customField": "keep me"
        });
        tokio::fs::write(&path, serde_json::to_string_pretty(&existing).unwrap())
            .await
            .unwrap();

        // Merge with new metadata
        let meta = SidecarMetadata {
            title: Some("New Title".to_string()),
            year: Some(2024),
            identifiers: SidecarIdentifiers {
                isbn: Some("978-1234567890".to_string()),
                kobbler_id: Some("entity-uuid-123".to_string()),
                ..Default::default()
            },
            book: Some(BookSidecar {
                authors: vec!["New Author".to_string()],
                ..Default::default()
            }),
            ..Default::default()
        };

        write_metadata_json_merged(&path, &meta).await.unwrap();

        let content = tokio::fs::read_to_string(&path).await.unwrap();
        let result: serde_json::Value = serde_json::from_str(&content).unwrap();

        // Managed fields updated
        assert_eq!(result["title"], "New Title");
        assert_eq!(result["authors"], serde_json::json!(["New Author"]));
        assert_eq!(result["isbn"], "978-1234567890");
        assert_eq!(result["publishedYear"], "2024");
        assert_eq!(result["kobblerId"], "entity-uuid-123");

        // Unknown keys preserved
        assert_eq!(
            result["narrators"],
            serde_json::json!(["Narrator One", "Narrator Two"])
        );
        assert_eq!(result["tags"], serde_json::json!(["tag1", "tag2"]));
        assert!(result["chapters"].is_array());
        assert_eq!(result["customField"], "keep me");
    }

    #[tokio::test]
    async fn test_write_metadata_json_merged_creates_new_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("metadata.json");

        let meta = SidecarMetadata {
            title: Some("Brand New".to_string()),
            identifiers: SidecarIdentifiers {
                kobbler_id: Some("new-uuid".to_string()),
                ..Default::default()
            },
            ..Default::default()
        };

        write_metadata_json_merged(&path, &meta).await.unwrap();

        let content = tokio::fs::read_to_string(&path).await.unwrap();
        let result: serde_json::Value = serde_json::from_str(&content).unwrap();
        assert_eq!(result["title"], "Brand New");
        assert_eq!(result["kobblerId"], "new-uuid");
    }
}
