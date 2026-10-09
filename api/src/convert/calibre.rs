//! Wrapper for Calibre CLI tools (ebook-convert, ebook-meta).
//!
//! - ebook-convert: Universal format conversion (MOBI, AZW, PDF, etc. → EPUB)
//! - ebook-meta: Metadata extraction from any ebook format

use super::{ConvertError, ConvertResult};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use tokio::process::Command;
use tracing::{debug, warn};

/// Wrapper for Calibre's ebook-convert CLI tool.
pub struct EbookConvert {
    /// Path to the ebook-convert binary.
    binary_path: PathBuf,
}

impl EbookConvert {
    /// Create a new EbookConvert wrapper, searching for the binary in PATH
    /// or using the provided config path / EBOOK_CONVERT_PATH environment variable.
    pub fn new() -> Self {
        Self::with_config_path(None)
    }

    /// Create with an optional config-provided path. Falls back to env var, then PATH search.
    pub fn with_config_path(config_path: Option<String>) -> Self {
        let binary_path = config_path
            .filter(|s| !s.is_empty())
            .or_else(|| {
                std::env::var("EBOOK_CONVERT_PATH")
                    .ok()
                    .filter(|s| !s.is_empty())
            })
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("ebook-convert"));

        Self { binary_path }
    }

    /// Check if ebook-convert is available and working.
    pub async fn is_available(&self) -> bool {
        match Command::new(&self.binary_path)
            .arg("--version")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .await
        {
            Ok(status) => status.success(),
            Err(e) => {
                debug!("ebook-convert not available: {}", e);
                false
            }
        }
    }

    /// Get the ebook-convert version string.
    pub async fn version(&self) -> Option<String> {
        let output = Command::new(&self.binary_path)
            .arg("--version")
            .output()
            .await
            .ok()?;

        if output.status.success() {
            Some(String::from_utf8_lossy(&output.stdout).trim().to_string())
        } else {
            None
        }
    }

    /// Convert any ebook format to EPUB.
    ///
    /// Returns the path to the converted EPUB file.
    pub async fn convert_to_epub(&self, input: &Path, output_dir: &Path) -> ConvertResult<PathBuf> {
        self.convert(input, output_dir, "epub").await
    }

    /// Convert any ebook format to MOBI (for Kindle devices).
    ///
    /// Returns the path to the converted MOBI file.
    pub async fn convert_to_mobi(&self, input: &Path, output_dir: &Path) -> ConvertResult<PathBuf> {
        self.convert(input, output_dir, "mobi").await
    }

    /// Convert any ebook format to a target format.
    ///
    /// Returns the path to the converted file.
    pub async fn convert(
        &self,
        input: &Path,
        output_dir: &Path,
        target_format: &str,
    ) -> ConvertResult<PathBuf> {
        if !self.is_available().await {
            return Err(ConvertError::EbookConvertNotFound);
        }

        // Ensure output directory exists
        tokio::fs::create_dir_all(output_dir).await?;

        // Build output filename: book.epub -> book.mobi
        // Handle kepub.epub special case
        let input_stem = input.file_stem().and_then(|s| s.to_str()).unwrap_or("book");
        let input_stem = input_stem.strip_suffix(".kepub").unwrap_or(input_stem);
        let output_name = format!("{}.{}", input_stem, target_format.to_lowercase());
        let output_path = output_dir.join(&output_name);

        debug!(
            "Converting {} to {} with ebook-convert",
            input.display(),
            output_path.display()
        );

        // Run ebook-convert
        // ebook-convert input.epub output.mobi
        let output = Command::new(&self.binary_path)
            .arg(input)
            .arg(&output_path)
            .output()
            .await?;

        if output.status.success() {
            debug!(
                "ebook-convert conversion successful: {}",
                output_path.display()
            );
            Ok(output_path)
        } else {
            let stderr = String::from_utf8_lossy(&output.stderr);
            warn!("ebook-convert conversion failed: {}", stderr);
            Err(ConvertError::ConversionFailed(format!(
                "ebook-convert failed: {}",
                stderr.trim()
            )))
        }
    }
}

impl Default for EbookConvert {
    fn default() -> Self {
        Self::new()
    }
}

/// Wrapper for Calibre's ebook-meta CLI tool for metadata extraction.
pub struct EbookMeta {
    /// Path to the ebook-meta binary.
    binary_path: PathBuf,
}

/// Extracted metadata from an ebook file.
#[derive(Debug, Clone, Default)]
pub struct ExtractedMetadata {
    pub title: Option<String>,
    pub authors: Vec<String>,
    pub isbn: Option<String>,
    pub publisher: Option<String>,
    pub pubdate: Option<String>,
    pub description: Option<String>,
    pub series: Option<String>,
    pub series_index: Option<f64>,
    pub language: Option<String>,
    pub tags: Vec<String>,
}

/// Metadata to write to an ebook file.
#[derive(Debug, Clone, Default)]
pub struct MetadataWrite {
    pub title: Option<String>,
    pub authors: Option<Vec<String>>,
    pub description: Option<String>,
    pub publisher: Option<String>,
    pub pubdate: Option<String>,
    pub language: Option<String>,
    pub series: Option<String>,
    pub series_index: Option<f64>,
}

impl EbookMeta {
    /// Create a new EbookMeta wrapper, searching for the binary in PATH
    /// or using the provided config path / EBOOK_META_PATH environment variable.
    pub fn new() -> Self {
        Self::with_config_path(None)
    }

    /// Create with an optional config-provided path. Falls back to env var, then PATH search.
    pub fn with_config_path(config_path: Option<String>) -> Self {
        let binary_path = config_path
            .filter(|s| !s.is_empty())
            .or_else(|| {
                std::env::var("EBOOK_META_PATH")
                    .ok()
                    .filter(|s| !s.is_empty())
            })
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("ebook-meta"));

        Self { binary_path }
    }

    /// Check if ebook-meta is available and working.
    pub async fn is_available(&self) -> bool {
        match Command::new(&self.binary_path)
            .arg("--version")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .await
        {
            Ok(status) => status.success(),
            Err(e) => {
                debug!("ebook-meta not available at {:?}: {}", self.binary_path, e);
                false
            }
        }
    }

    /// Extract metadata from an ebook file. Native (boko) for
    /// EPUB/MOBI/AZW/AZW3/KFX, Calibre `ebook-meta` for the long-tail formats.
    pub async fn extract(&self, input: &Path) -> ConvertResult<ExtractedMetadata> {
        if kob_ebook::native_supports(input) {
            let owned = input.to_owned();
            let m = tokio::task::spawn_blocking(move || kob_ebook::read_metadata(&owned))
                .await
                .map_err(|e| ConvertError::ConversionFailed(format!("kob-ebook task panic: {e}")))?
                .map_err(|e| ConvertError::ConversionFailed(format!("kob-ebook: {e}")))?;
            return Ok(ExtractedMetadata {
                title: m.title,
                authors: m.authors,
                isbn: m.isbn,
                publisher: m.publisher,
                pubdate: m.pubdate,
                description: m.description,
                series: m.series,
                series_index: m.series_index,
                language: m.language,
                tags: m.tags,
            });
        }

        if !self.is_available().await {
            return Err(ConvertError::EbookConvertNotFound);
        }

        // Calibre fallback for PDF/FB2/RTF/TXT/HTML/DOCX/LIT/PDB.
        let output = Command::new(&self.binary_path).arg(input).output().await?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            warn!("ebook-meta extraction failed: {}", stderr);
            return Err(ConvertError::ConversionFailed(format!(
                "ebook-meta failed: {}",
                stderr.trim()
            )));
        }

        let stdout = String::from_utf8_lossy(&output.stdout);
        Ok(Self::parse_output(&stdout))
    }

    /// Extract cover image from an ebook file. Native (boko) for boko-supported
    /// formats, Calibre `ebook-meta --get-cover` otherwise.
    ///
    /// Returns the path to the extracted cover image. The native path always
    /// writes a `.jpg` regardless of source mime — callers historically expect
    /// `.jpg` and image decoders downstream are happy either way.
    pub async fn extract_cover(&self, input: &Path, output_dir: &Path) -> ConvertResult<PathBuf> {
        let input_stem = input
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("cover");
        let cover_path = output_dir.join(format!("{input_stem}_cover.jpg"));

        if kob_ebook::native_supports(input) {
            tokio::fs::create_dir_all(output_dir).await?;
            let owned = input.to_owned();
            let bytes = tokio::task::spawn_blocking(move || kob_ebook::read_cover_bytes(&owned))
                .await
                .map_err(|e| ConvertError::ConversionFailed(format!("kob-ebook task panic: {e}")))?
                .map_err(|e| ConvertError::ConversionFailed(format!("kob-ebook: {e}")))?
                .ok_or_else(|| ConvertError::ConversionFailed("No cover image found".into()))?;
            tokio::fs::write(&cover_path, bytes).await?;
            return Ok(cover_path);
        }

        if !self.is_available().await {
            return Err(ConvertError::EbookConvertNotFound);
        }

        tokio::fs::create_dir_all(output_dir).await?;
        debug!(
            "Extracting cover from {} to {}",
            input.display(),
            cover_path.display()
        );

        let output = Command::new(&self.binary_path)
            .arg(input)
            .arg("--get-cover")
            .arg(&cover_path)
            .output()
            .await?;

        if output.status.success() && cover_path.exists() {
            debug!("Cover extracted: {}", cover_path.display());
            Ok(cover_path)
        } else {
            let stderr = String::from_utf8_lossy(&output.stderr);
            debug!("Cover extraction failed: {}", stderr);
            Err(ConvertError::ConversionFailed(
                "No cover image found".to_string(),
            ))
        }
    }

    /// Write metadata to an ebook file.
    ///
    /// Only writes fields that have values - does not clear existing metadata.
    pub async fn write_metadata(
        &self,
        input: &Path,
        metadata: &MetadataWrite,
    ) -> ConvertResult<()> {
        if !self.is_available().await {
            return Err(ConvertError::EbookConvertNotFound);
        }

        let mut cmd = Command::new(&self.binary_path);
        cmd.arg(input);

        // Add flags for each field that has a value
        if let Some(title) = &metadata.title {
            cmd.arg("--title").arg(title);
        }
        if let Some(authors) = &metadata.authors
            && !authors.is_empty()
        {
            cmd.arg("--authors").arg(authors.join(" & "));
        }
        if let Some(description) = &metadata.description {
            cmd.arg("--comments").arg(description);
        }
        if let Some(publisher) = &metadata.publisher {
            cmd.arg("--publisher").arg(publisher);
        }
        if let Some(pubdate) = &metadata.pubdate {
            cmd.arg("--date").arg(pubdate);
        }
        if let Some(language) = &metadata.language {
            cmd.arg("--language").arg(language);
        }
        if let Some(series) = &metadata.series {
            cmd.arg("--series").arg(series);
        }
        if let Some(index) = &metadata.series_index {
            cmd.arg("--index").arg(index.to_string());
        }

        debug!("Writing metadata to {} with ebook-meta", input.display());

        let output = cmd.output().await?;

        if output.status.success() {
            debug!("Metadata written successfully to {}", input.display());
            Ok(())
        } else {
            let stderr = String::from_utf8_lossy(&output.stderr);
            warn!("ebook-meta write failed: {}", stderr);
            Err(ConvertError::ConversionFailed(format!(
                "ebook-meta write failed: {}",
                stderr.trim()
            )))
        }
    }

    /// Parse ebook-meta output into structured metadata.
    fn parse_output(output: &str) -> ExtractedMetadata {
        /// Normalize author name by:
        /// 1. Stripping bracketed author sort suffix like "[Dylan, Bob]"
        /// 2. Reversing "Last, First" format to "First Last"
        fn normalize_author_name(name: &str) -> String {
            let trimmed = name.trim();

            // 1. Strip bracketed author sort suffix like "[Dylan, Bob]"
            let without_brackets = if let Some(bracket_pos) = trimmed.rfind('[') {
                trimmed[..bracket_pos].trim()
            } else {
                trimmed
            };

            // 2. Reverse "Last, First" or "Last,First" format to "First Last"
            // Only if there's exactly one comma and it looks like a name (not "Jr." etc.)
            if let Some(comma_pos) = without_brackets.find(',') {
                let last = without_brackets[..comma_pos].trim();
                let first = without_brackets[comma_pos + 1..].trim();

                // Simple heuristic: if both parts exist and first doesn't contain
                // common suffixes, it's likely "Last, First" format
                let is_suffix = first.eq_ignore_ascii_case("Jr")
                    || first.eq_ignore_ascii_case("Jr.")
                    || first.eq_ignore_ascii_case("Sr")
                    || first.eq_ignore_ascii_case("Sr.")
                    || first.eq_ignore_ascii_case("III")
                    || first.eq_ignore_ascii_case("II");

                if !first.is_empty() && !is_suffix {
                    return format!("{} {}", first, last);
                }
            }

            without_brackets.to_string()
        }

        /// Extract series info from title patterns like "Title (Series Book N)"
        fn parse_series_from_title(title: &str) -> Option<(String, String, Option<f64>)> {
            // Find last occurrence of "(...)" in title
            let close_paren = title.rfind(')')?;
            let open_paren = title[..close_paren].rfind('(')?;

            let base_title = title[..open_paren].trim().to_string();
            let paren_content = title[open_paren + 1..close_paren].trim();

            // Don't extract if base title is empty
            if base_title.is_empty() {
                return None;
            }

            // Try to find "Book N" or "#N" pattern
            let (series_name, index) =
                if let Some(book_pos) = paren_content.to_lowercase().find(" book ") {
                    let series = paren_content[..book_pos].trim_end_matches(',').trim();
                    let num_str = paren_content[book_pos + 6..].trim();
                    let idx = num_str.parse::<f64>().ok();
                    (series.to_string(), idx)
                } else if let Some(hash_pos) = paren_content.rfind('#') {
                    let series = paren_content[..hash_pos].trim_end_matches(',').trim();
                    let num_str = paren_content[hash_pos + 1..].trim();
                    let idx = num_str.parse::<f64>().ok();
                    (series.to_string(), idx)
                } else {
                    // No book number found, just use content as series name
                    (paren_content.to_string(), None)
                };

            if series_name.is_empty() {
                return None;
            }

            Some((base_title, series_name, index))
        }

        let mut metadata = ExtractedMetadata::default();
        let mut fields: HashMap<&str, String> = HashMap::new();

        for line in output.lines() {
            if let Some((key, value)) = line.split_once(':') {
                let key = key.trim();
                let value = value.trim().to_string();
                if !value.is_empty() {
                    fields.insert(key, value);
                }
            }
        }

        // Parse individual fields
        metadata.title = fields.get("Title").cloned();

        if let Some(authors) = fields.get("Author(s)") {
            metadata.authors = authors
                .split('&')
                .map(normalize_author_name)
                .filter(|a| !a.is_empty())
                .collect();
        }

        // Look for ISBN in identifiers
        if let Some(ids) = fields.get("Identifiers") {
            for id in ids.split(',') {
                let id = id.trim();
                if let Some(isbn) = id.strip_prefix("isbn:") {
                    metadata.isbn = Some(isbn.to_string());
                    break;
                }
            }
        }

        metadata.publisher = fields.get("Publisher").cloned();
        metadata.pubdate = fields.get("Published").cloned();
        metadata.description = fields.get("Comments").cloned();
        metadata.language = fields.get("Language").cloned();

        // Parse series info
        if let Some(series) = fields.get("Series") {
            // Format: "Series Name #1.0"
            if let Some((name, index)) = series.rsplit_once('#') {
                metadata.series = Some(name.trim().to_string());
                metadata.series_index = index.trim().parse().ok();
            } else {
                metadata.series = Some(series.clone());
            }
        }

        // Fallback: try to extract series from title if not found in metadata
        // Handles patterns like "Soulsmith (Cradle Book 2)"
        if metadata.series.is_none()
            && let Some(title) = &metadata.title
            && let Some((base_title, series_name, series_index)) = parse_series_from_title(title)
        {
            metadata.title = Some(base_title);
            metadata.series = Some(series_name);
            metadata.series_index = series_index;
        }

        // Parse tags
        if let Some(tags) = fields.get("Tags") {
            metadata.tags = tags
                .split(',')
                .map(|t| t.trim().to_string())
                .filter(|t| !t.is_empty())
                .collect();
        }

        metadata
    }
}

impl Default for EbookMeta {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_metadata_output() {
        let output = r#"
Title               : The Great Book
Author(s)           : John Doe & Jane Smith
Publisher           : Example Press
Published           : 2023-01-15
Identifiers         : isbn:9781234567890, uuid:abc123
Language            : eng
Series              : Amazing Series #2.5
Tags                : fiction, adventure
Comments            : A wonderful story about something.
        "#;

        let metadata = EbookMeta::parse_output(output);

        assert_eq!(metadata.title, Some("The Great Book".to_string()));
        assert_eq!(
            metadata.authors,
            vec!["John Doe".to_string(), "Jane Smith".to_string()]
        );
        assert_eq!(metadata.isbn, Some("9781234567890".to_string()));
        assert_eq!(metadata.publisher, Some("Example Press".to_string()));
        assert_eq!(metadata.series, Some("Amazing Series".to_string()));
        assert_eq!(metadata.series_index, Some(2.5));
        assert_eq!(metadata.language, Some("eng".to_string()));
        assert_eq!(metadata.tags, vec!["fiction", "adventure"]);
    }

    #[test]
    fn test_author_name_normalization_brackets() {
        // Test stripping bracketed author sort suffix
        let output = r#"
Title               : Test Book
Author(s)           : Bob Dylan [Dylan, Bob]
        "#;

        let metadata = EbookMeta::parse_output(output);
        assert_eq!(metadata.authors, vec!["Bob Dylan".to_string()]);
    }

    #[test]
    fn test_author_name_normalization_reverse_order() {
        // Test reversing "Last, First" format
        let output = r#"
Title               : Test Book
Author(s)           : Wight,Will
        "#;

        let metadata = EbookMeta::parse_output(output);
        assert_eq!(metadata.authors, vec!["Will Wight".to_string()]);

        // With space after comma
        let output2 = r#"
Title               : Test Book
Author(s)           : King, Stephen
        "#;

        let metadata2 = EbookMeta::parse_output(output2);
        assert_eq!(metadata2.authors, vec!["Stephen King".to_string()]);
    }

    #[test]
    fn test_author_name_normalization_preserves_suffixes() {
        // Test that suffixes like Jr., Sr., III are preserved
        let output = r#"
Title               : Test Book
Author(s)           : Martin, Jr.
        "#;

        let metadata = EbookMeta::parse_output(output);
        assert_eq!(metadata.authors, vec!["Martin, Jr.".to_string()]);
    }

    #[test]
    fn test_author_name_normalization_no_comma() {
        // Test that normal names are unchanged
        let output = r#"
Title               : Test Book
Author(s)           : George R.R. Martin
        "#;

        let metadata = EbookMeta::parse_output(output);
        assert_eq!(metadata.authors, vec!["George R.R. Martin".to_string()]);
    }

    #[test]
    fn test_author_name_normalization_truncated_sort_form() {
        // Truncated sort forms like "Arthur, C." (missing surname) get flipped
        // to "C. Arthur". This is expected — the guard in metadata_extraction
        // prevents these from being linked when provider authors exist.
        let output = r#"
Title               : Test Book
Author(s)           : Arthur, C.
        "#;

        let metadata = EbookMeta::parse_output(output);
        assert_eq!(metadata.authors, vec!["C. Arthur".to_string()]);
    }

    #[test]
    fn test_series_extraction_from_title_book_pattern() {
        // Test "Title (Series Book N)" pattern
        let output = r#"
Title               : Soulsmith (Cradle Book 2)
Author(s)           : Will Wight
        "#;

        let metadata = EbookMeta::parse_output(output);
        assert_eq!(metadata.title, Some("Soulsmith".to_string()));
        assert_eq!(metadata.series, Some("Cradle".to_string()));
        assert_eq!(metadata.series_index, Some(2.0));
    }

    #[test]
    fn test_series_extraction_from_title_hash_pattern() {
        // Test "Title (Series #N)" pattern
        let output = r#"
Title               : Elantris (Cosmere #1)
Author(s)           : Brandon Sanderson
        "#;

        let metadata = EbookMeta::parse_output(output);
        assert_eq!(metadata.title, Some("Elantris".to_string()));
        assert_eq!(metadata.series, Some("Cosmere".to_string()));
        assert_eq!(metadata.series_index, Some(1.0));
    }

    #[test]
    fn test_series_extraction_from_title_with_comma() {
        // Test "Title (Series, Book N)" pattern
        let output = r#"
Title               : The Way of Kings (The Stormlight Archive, Book 1)
Author(s)           : Brandon Sanderson
        "#;

        let metadata = EbookMeta::parse_output(output);
        assert_eq!(metadata.title, Some("The Way of Kings".to_string()));
        assert_eq!(metadata.series, Some("The Stormlight Archive".to_string()));
        assert_eq!(metadata.series_index, Some(1.0));
    }

    #[test]
    fn test_series_extraction_from_title_no_index() {
        // Test pattern with no book number
        let output = r#"
Title               : Warbreaker (A Cosmere Novel)
Author(s)           : Brandon Sanderson
        "#;

        let metadata = EbookMeta::parse_output(output);
        assert_eq!(metadata.title, Some("Warbreaker".to_string()));
        assert_eq!(metadata.series, Some("A Cosmere Novel".to_string()));
        assert_eq!(metadata.series_index, None);
    }

    #[test]
    fn test_series_extraction_does_not_override_existing() {
        // Test that existing series from metadata is not overridden
        let output = r#"
Title               : Soulsmith (Cradle Book 2)
Author(s)           : Will Wight
Series              : Actual Series #5.0
        "#;

        let metadata = EbookMeta::parse_output(output);
        // Title should remain unchanged when series exists in metadata
        assert_eq!(
            metadata.title,
            Some("Soulsmith (Cradle Book 2)".to_string())
        );
        assert_eq!(metadata.series, Some("Actual Series".to_string()));
        assert_eq!(metadata.series_index, Some(5.0));
    }

    #[test]
    fn test_plain_title_unchanged() {
        // Test that titles without parentheses are unchanged
        let output = r#"
Title               : Plain Title Without Series
Author(s)           : Some Author
        "#;

        let metadata = EbookMeta::parse_output(output);
        assert_eq!(
            metadata.title,
            Some("Plain Title Without Series".to_string())
        );
        assert_eq!(metadata.series, None);
        assert_eq!(metadata.series_index, None);
    }
}
