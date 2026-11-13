//! Ebook format conversion pipeline.
//!
//! Native (in-process) conversion via `kob_ebook` (boko + homegrown kepubifier)
//! for EPUB/MOBI/AZW/AZW3/KFX. Calibre `ebook-convert` is kept as a fallback
//! for PDF/FB2/RTF/TXT/HTML/DOCX/LIT/PDB — formats boko doesn't read.
//!
//! ```text
//!   .epub/.mobi/.azw3/.kfx        → kob_ebook → EPUB / KEPUB
//!   .pdf/.fb2/.rtf/.txt/.html/...  → ebook-convert → EPUB → kob_ebook kepubify → KEPUB
//! ```

mod calibre;

pub use calibre::{EbookConvert, EbookMeta, ExtractedMetadata, MetadataWrite};

use std::path::{Path, PathBuf};
use thiserror::Error;
use tracing::warn;

#[derive(Error, Debug)]
pub enum ConvertError {
    #[error("ebook-convert not found. Install Calibre: https://calibre-ebook.com")]
    EbookConvertNotFound,

    #[error("Unsupported format: {0}")]
    UnsupportedFormat(String),

    #[error("Conversion failed: {0}")]
    ConversionFailed(String),

    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
}

pub type ConvertResult<T> = Result<T, ConvertError>;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EbookFormat {
    Epub,
    Kepub,
    Mobi,
    Azw,
    Azw3,
    Pdf,
    Fb2,
    Cbz,
    Cbr,
    Lit,
    Pdb,
    Rtf,
    Txt,
    Html,
    Docx,
    Unknown,
}

impl EbookFormat {
    pub fn from_path(path: &Path) -> Self {
        let ext = path
            .extension()
            .and_then(|e| e.to_str())
            .map(|e| e.to_lowercase());

        match ext.as_deref() {
            Some("epub") => {
                let stem = path.file_stem().and_then(|s| s.to_str()).unwrap_or("");
                if stem.ends_with(".kepub") {
                    EbookFormat::Kepub
                } else {
                    EbookFormat::Epub
                }
            }
            Some("kepub") => EbookFormat::Kepub,
            Some("mobi") => EbookFormat::Mobi,
            Some("azw") => EbookFormat::Azw,
            Some("azw3") => EbookFormat::Azw3,
            Some("pdf") => EbookFormat::Pdf,
            Some("fb2") => EbookFormat::Fb2,
            Some("cbz") => EbookFormat::Cbz,
            Some("cbr") => EbookFormat::Cbr,
            Some("lit") => EbookFormat::Lit,
            Some("pdb") => EbookFormat::Pdb,
            Some("rtf") => EbookFormat::Rtf,
            Some("txt") => EbookFormat::Txt,
            Some("html") | Some("htm") => EbookFormat::Html,
            Some("docx") => EbookFormat::Docx,
            _ => EbookFormat::Unknown,
        }
    }

    pub fn needs_epub_conversion(&self) -> bool {
        !matches!(self, EbookFormat::Epub | EbookFormat::Kepub)
    }

    pub fn is_kepub(&self) -> bool {
        matches!(self, EbookFormat::Kepub)
    }

    pub fn extension(&self) -> &'static str {
        match self {
            EbookFormat::Epub => "epub",
            EbookFormat::Kepub => "kepub.epub",
            EbookFormat::Mobi => "mobi",
            EbookFormat::Azw => "azw",
            EbookFormat::Azw3 => "azw3",
            EbookFormat::Pdf => "pdf",
            EbookFormat::Fb2 => "fb2",
            EbookFormat::Cbz => "cbz",
            EbookFormat::Cbr => "cbr",
            EbookFormat::Lit => "lit",
            EbookFormat::Pdb => "pdb",
            EbookFormat::Rtf => "rtf",
            EbookFormat::Txt => "txt",
            EbookFormat::Html => "html",
            EbookFormat::Docx => "docx",
            EbookFormat::Unknown => "",
        }
    }
}

pub struct ConversionPipeline {
    ebook_convert: EbookConvert,
}

impl ConversionPipeline {
    pub fn new() -> Self {
        Self {
            ebook_convert: EbookConvert::new(),
        }
    }

    /// Whether the long-tail Calibre fallback is reachable. Native pipeline is
    /// always available — this only matters for PDF/FB2/RTF/etc.
    pub async fn calibre_available(&self) -> bool {
        self.ebook_convert.is_available().await
    }

    /// Convert any supported ebook format to EPUB on disk.
    pub async fn convert_to_epub(&self, input: &Path, output_dir: &Path) -> ConvertResult<PathBuf> {
        let format = EbookFormat::from_path(input);
        match format {
            EbookFormat::Unknown => Err(ConvertError::UnsupportedFormat(
                input
                    .extension()
                    .and_then(|e| e.to_str())
                    .unwrap_or("unknown")
                    .to_string(),
            )),
            // Already EPUB → just copy. Avoid round-tripping through boko.
            EbookFormat::Epub => {
                tokio::fs::create_dir_all(output_dir).await?;
                let out = output_dir.join(input.file_name().unwrap_or_default());
                tokio::fs::copy(input, &out).await?;
                Ok(out)
            }
            f if kob_ebook_handles(f) => {
                let bytes = run_native_to_epub(input).await?;
                if self.native_output_is_broken(input, &bytes).await {
                    return self.ebook_convert.convert_to_epub(input, output_dir).await;
                }
                write_with_extension(input, output_dir, "epub", &bytes).await
            }
            // Calibre-only formats.
            _ => self.ebook_convert.convert_to_epub(input, output_dir).await,
        }
    }

    /// Convert any supported ebook format to KEPUB on disk.
    pub async fn convert_to_kepub(
        &self,
        input: &Path,
        output_dir: &Path,
    ) -> ConvertResult<PathBuf> {
        let format = EbookFormat::from_path(input);
        match format {
            EbookFormat::Unknown => Err(ConvertError::UnsupportedFormat(
                input
                    .extension()
                    .and_then(|e| e.to_str())
                    .unwrap_or("unknown")
                    .to_string(),
            )),
            // Already KEPUB — copy through (no need to re-transform; the
            // kepubifier is idempotent but the I/O is wasteful).
            EbookFormat::Kepub => {
                tokio::fs::create_dir_all(output_dir).await?;
                let out = output_dir.join(input.file_name().unwrap_or_default());
                tokio::fs::copy(input, &out).await?;
                Ok(out)
            }
            f if kob_ebook_handles(f) => {
                let bytes = run_native_to_kepub(input).await?;
                if self.native_output_is_broken(input, &bytes).await {
                    // Same rescue as the EPUB path, then kepubify Calibre's
                    // output in-process so the KEPUB transform still runs.
                    let epub_path = self
                        .ebook_convert
                        .convert_to_epub(input, output_dir)
                        .await?;
                    let epub_bytes = tokio::fs::read(&epub_path).await?;
                    let kepub_bytes = run_kepubify(epub_bytes).await?;
                    let _ = tokio::fs::remove_file(&epub_path).await;
                    return write_kepub_with_stem(input, output_dir, &kepub_bytes).await;
                }
                write_kepub_with_stem(input, output_dir, &bytes).await
            }
            // Calibre fallback: convert source → EPUB on disk, then kepubify
            // those bytes in-process. Drop the intermediate EPUB after.
            _ => {
                let epub_path = self
                    .ebook_convert
                    .convert_to_epub(input, output_dir)
                    .await?;
                let epub_bytes = tokio::fs::read(&epub_path).await?;
                let kepub_bytes = run_kepubify(epub_bytes).await?;
                let _ = tokio::fs::remove_file(&epub_path).await;
                write_kepub_with_stem(input, output_dir, &kepub_bytes).await
            }
        }
    }

    /// Whether native output is damaged badly enough to be worth re-doing with
    /// Calibre.
    ///
    /// Today that means exactly one thing: stylesheets the documents link but
    /// the archive does not contain — see [`dangling_stylesheets`]. Calibre
    /// extracts KF8 CSS flows correctly, so it is a real rescue and not just a
    /// second opinion.
    ///
    /// An EPUB/KEPUB source is exempt: if its own stylesheet is missing, that
    /// is the book, not the conversion, and re-running it through Calibre would
    /// lose fidelity without recovering anything.
    async fn native_output_is_broken(&self, input: &Path, bytes: &[u8]) -> bool {
        if matches!(
            EbookFormat::from_path(input),
            EbookFormat::Epub | EbookFormat::Kepub
        ) {
            return false;
        }
        let epub = bytes.to_vec();
        let missing = tokio::task::spawn_blocking(move || dangling_stylesheets(&epub))
            .await
            .unwrap_or_default();
        if missing.is_empty() {
            return false;
        }
        if !self.ebook_convert.is_available().await {
            warn!(
                source = %input.display(),
                missing = ?missing,
                "native conversion dropped stylesheets and Calibre is unavailable to \
                 re-do it; the book will render unstyled"
            );
            return false;
        }
        warn!(
            source = %input.display(),
            missing = ?missing,
            "native conversion linked stylesheets it did not emit; re-converting with Calibre"
        );
        true
    }

    /// Convert any supported ebook format to MOBI (for Kindle delivery).
    /// Always shells out to Calibre — boko doesn't write MOBI.
    pub async fn convert_to_mobi(&self, input: &Path, output_dir: &Path) -> ConvertResult<PathBuf> {
        let format = EbookFormat::from_path(input);
        if format == EbookFormat::Mobi {
            tokio::fs::create_dir_all(output_dir).await?;
            let output = output_dir.join(input.file_name().unwrap_or_default());
            tokio::fs::copy(input, &output).await?;
            return Ok(output);
        }
        if format == EbookFormat::Unknown {
            return Err(ConvertError::UnsupportedFormat(
                input
                    .extension()
                    .and_then(|e| e.to_str())
                    .unwrap_or("unknown")
                    .to_string(),
            ));
        }
        self.ebook_convert.convert_to_mobi(input, output_dir).await
    }
}

impl Default for ConversionPipeline {
    fn default() -> Self {
        Self::new()
    }
}

/// Why an EPUB/KEPUB we produced is not fit to serve, or an empty list if it is
/// fine.
///
/// Deliberately narrow. These are faults that make a book render wrong on a
/// device with nothing in any log to say so, and that re-running the current
/// conversion pipeline actually fixes:
///
/// - the file is not a readable zip at all (a mislabelled MOBI, a truncated
///   download), so no reader can open it;
/// - the documents link stylesheets the archive does not contain — the KF8
///   flow bug described on [`dangling_stylesheets`].
///
/// It is not a validator. Cosmetic publisher sloppiness — an absent
/// `page-template.xpgt`, a promo page's missing CSS — is left alone, because
/// re-converting over it would cost fidelity and buy nothing.
///
/// Which is what `source` is for: pass the file this one was converted from and
/// any hole it already had stops counting. A conversion cannot repair what it
/// inherited, so reporting it would pin the book to the rework list forever —
/// every sweep re-making a file that comes out exactly as flawed, and every
/// re-make handing the device a new `RevisionId` to re-download for nothing.
/// Pass `None` when there is no source to compare against.
pub(crate) fn epub_defects(bytes: &[u8], source: Option<&[u8]>) -> Vec<String> {
    if zip::ZipArchive::new(std::io::Cursor::new(bytes)).is_err() {
        return vec!["not a readable zip archive".to_string()];
    }
    // A source that is not a zip (AZW3/MOBI) has no stylesheet entries to be
    // missing, so it cannot be where a hole came from.
    let inherited: std::collections::HashSet<String> = source
        .map(|s| dangling_stylesheets(s).into_iter().collect())
        .unwrap_or_default();

    let missing: Vec<String> = dangling_stylesheets(bytes)
        .into_iter()
        .filter(|sheet| !inherited.contains(sheet))
        .collect();
    if missing.is_empty() {
        return Vec::new();
    }
    vec![format!(
        "stylesheets linked but not present: {}",
        missing.join(", ")
    )]
}

/// Whether a fresh conversion came out worse than the source it was made from,
/// and why — `None` when it is safe to put in place of an older copy.
///
/// The comparison against the source is the point. A conversion inherits its
/// source's faults: plenty of publisher EPUBs link a stylesheet they do not
/// ship, and judging the output on its own would condemn the conversion for
/// something it did not do — refusing, in particular, to re-make the very books
/// most in need of it. What must never be written out is a fault the conversion
/// *introduced*, which is exactly the KF8 stylesheet loss that
/// [`dangling_stylesheets`] describes.
pub(crate) fn conversion_regressed(produced: &[u8], source: &[u8]) -> Option<String> {
    if zip::ZipArchive::new(std::io::Cursor::new(produced)).is_err() {
        return Some("output is not a readable zip archive".to_string());
    }
    let missing = dangling_stylesheets(produced);
    if missing.is_empty() {
        return None;
    }
    // The source may not be a zip at all (AZW3/MOBI), in which case it cannot
    // be the origin of a missing stylesheet and the fault is the conversion's.
    if !dangling_stylesheets(source).is_empty() {
        return None;
    }
    Some(format!(
        "output links stylesheets it does not contain: {}",
        missing.join(", ")
    ))
}

/// Stylesheet hrefs the EPUB's documents link but the archive does not contain.
///
/// boko rewrites KF8's `kindle:flow:NNNN` references to `styles/styleNNNN.css`
/// when converting a MOBI/AZW3, but its asset discovery only knows how to emit
/// images and fonts — the CSS flows themselves are never written out. What comes
/// back is a structurally valid EPUB in which every document links a stylesheet
/// that is not in the zip. On a reflowable novel that reads as slightly plain
/// text. On a fixed-layout book it is total: the absolute positioning and the
/// per-page background art live entirely in that stylesheet, so the page
/// collapses into loose lines scattered down the screen.
///
/// Nothing downstream reports a missing stylesheet — a reader just renders
/// without it — so the conversion has to notice for itself.
///
/// Returns an empty list for bytes that are not a readable zip: that is a
/// different (and worse) defect, and [`epub_defects`] is what distinguishes
/// them. Here, "no archive" simply means "no evidence of this fault".
pub(crate) fn dangling_stylesheets(epub: &[u8]) -> Vec<String> {
    let Ok(mut archive) = zip::ZipArchive::new(std::io::Cursor::new(epub)) else {
        return Vec::new();
    };

    let present: std::collections::HashSet<String> = archive
        .file_names()
        .filter_map(|name| name.ok().map(std::borrow::Cow::into_owned))
        .collect();

    let docs: Vec<String> = present
        .iter()
        .filter(|n| {
            let lower = n.to_ascii_lowercase();
            lower.ends_with(".xhtml") || lower.ends_with(".html") || lower.ends_with(".htm")
        })
        .cloned()
        .collect();

    let mut missing = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for name in docs {
        let mut body = String::new();
        {
            let Ok(mut entry) = archive.by_name(&name) else {
                continue;
            };
            // A document we cannot read as text tells us nothing either way.
            if std::io::Read::read_to_string(&mut entry, &mut body).is_err() {
                continue;
            }
        }
        for href in stylesheet_hrefs(&body) {
            let resolved = resolve_href(&name, &href);
            if !present.contains(&resolved) && seen.insert(resolved.clone()) {
                missing.push(resolved);
            }
        }
    }
    missing
}

/// Pull the `href`s out of `<link rel="stylesheet">` tags.
///
/// A scan rather than an XML parse on purpose: this runs over every document in
/// the book, the answer only has to be good enough to spot a whole missing
/// stylesheet, and converter output is not reliably well-formed. Lowercasing is
/// ASCII-only so byte offsets stay valid against the original string.
///
/// External URLs and `data:` URIs are skipped — they are not archive entries, so
/// their absence proves nothing.
fn stylesheet_hrefs(html: &str) -> Vec<String> {
    let lower = html.to_ascii_lowercase();
    let bytes = lower.as_bytes();
    let mut out = Vec::new();
    let mut from = 0usize;

    while let Some(rel) = lower[from..].find("<link") {
        let tag_start = from + rel;
        let Some(gt) = lower[tag_start..].find('>') else {
            break;
        };
        let tag_end = tag_start + gt;
        from = tag_end + 1;

        let tag = &lower[tag_start..tag_end];
        if !tag.contains("stylesheet") {
            continue;
        }
        let Some(href_rel) = tag.find("href") else {
            continue;
        };

        let mut i = tag_start + href_rel + 4;
        while i < tag_end && bytes[i].is_ascii_whitespace() {
            i += 1;
        }
        if i >= tag_end || bytes[i] != b'=' {
            continue;
        }
        i += 1;
        while i < tag_end && bytes[i].is_ascii_whitespace() {
            i += 1;
        }
        let quote = match bytes.get(i).copied() {
            Some(b'"') => '"',
            Some(b'\'') => '\'',
            _ => continue,
        };
        i += 1;
        let Some(close) = html[i..].find(quote) else {
            continue;
        };

        let href = html[i..i + close].trim();
        if href.is_empty()
            || href.starts_with("data:")
            || href.starts_with("//")
            || href.contains("://")
        {
            continue;
        }
        // Fragments and queries are not part of the archive path.
        let href = href.split(['#', '?']).next().unwrap_or(href);
        out.push(percent_decode(href));
    }
    out
}

/// Resolve a document-relative href to a zip entry path.
fn resolve_href(doc_path: &str, href: &str) -> String {
    if let Some(abs) = href.strip_prefix('/') {
        return abs.to_string();
    }
    let mut parts: Vec<&str> = doc_path.split('/').collect();
    parts.pop(); // drop the linking document's own filename
    for seg in href.split('/') {
        match seg {
            "" | "." => {}
            ".." => {
                parts.pop();
            }
            s => parts.push(s),
        }
    }
    parts.join("/")
}

/// Minimal percent-decoding for href paths. Zip entry names are stored decoded,
/// so `my%20styles.css` has to be matched against `my styles.css`.
fn percent_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%'
            && i + 2 < bytes.len()
            && let Ok(hex) = std::str::from_utf8(&bytes[i + 1..i + 3])
            && let Ok(b) = u8::from_str_radix(hex, 16)
        {
            out.push(b);
            i += 3;
            continue;
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn kob_ebook_handles(f: EbookFormat) -> bool {
    matches!(
        f,
        EbookFormat::Epub
            | EbookFormat::Kepub
            | EbookFormat::Mobi
            | EbookFormat::Azw
            | EbookFormat::Azw3,
    )
}

async fn run_native_to_epub(input: &Path) -> ConvertResult<Vec<u8>> {
    let owned = input.to_owned();
    tokio::task::spawn_blocking(move || kob_ebook::convert_to_epub_bytes(&owned))
        .await
        .map_err(|e| ConvertError::ConversionFailed(format!("kob-ebook task panic: {e}")))?
        .map_err(|e| ConvertError::ConversionFailed(format!("kob-ebook: {e}")))
}

async fn run_native_to_kepub(input: &Path) -> ConvertResult<Vec<u8>> {
    let owned = input.to_owned();
    tokio::task::spawn_blocking(move || kob_ebook::convert_to_kepub_bytes(&owned))
        .await
        .map_err(|e| ConvertError::ConversionFailed(format!("kob-ebook task panic: {e}")))?
        .map_err(|e| ConvertError::ConversionFailed(format!("kob-ebook: {e}")))
}

async fn run_kepubify(epub_bytes: Vec<u8>) -> ConvertResult<Vec<u8>> {
    tokio::task::spawn_blocking(move || kob_ebook::kepubify_bytes(&epub_bytes))
        .await
        .map_err(|e| ConvertError::ConversionFailed(format!("kepubify task panic: {e}")))?
        .map_err(|e| ConvertError::ConversionFailed(format!("kepubify: {e}")))
}

async fn write_with_extension(
    input: &Path,
    output_dir: &Path,
    target_ext: &str,
    bytes: &[u8],
) -> ConvertResult<PathBuf> {
    tokio::fs::create_dir_all(output_dir).await?;
    let stem = input.file_stem().and_then(|s| s.to_str()).unwrap_or("book");
    let stem = stem.strip_suffix(".kepub").unwrap_or(stem);
    let out = output_dir.join(format!("{stem}.{target_ext}"));
    tokio::fs::write(&out, bytes).await?;
    Ok(out)
}

async fn write_kepub_with_stem(
    input: &Path,
    output_dir: &Path,
    bytes: &[u8],
) -> ConvertResult<PathBuf> {
    write_with_extension(input, output_dir, "kepub.epub", bytes).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_format_detection() {
        assert_eq!(
            EbookFormat::from_path(Path::new("book.epub")),
            EbookFormat::Epub
        );
        assert_eq!(
            EbookFormat::from_path(Path::new("book.kepub.epub")),
            EbookFormat::Kepub
        );
        assert_eq!(
            EbookFormat::from_path(Path::new("book.mobi")),
            EbookFormat::Mobi
        );
        assert_eq!(
            EbookFormat::from_path(Path::new("book.pdf")),
            EbookFormat::Pdf
        );
        assert_eq!(
            EbookFormat::from_path(Path::new("book.unknown")),
            EbookFormat::Unknown
        );
    }

    #[test]
    fn test_needs_conversion() {
        assert!(!EbookFormat::Epub.needs_epub_conversion());
        assert!(!EbookFormat::Kepub.needs_epub_conversion());
        assert!(EbookFormat::Mobi.needs_epub_conversion());
        assert!(EbookFormat::Pdf.needs_epub_conversion());
    }

    /// Build a minimal EPUB zip from (name, bytes) pairs.
    fn zip_of(entries: &[(&str, &str)]) -> Vec<u8> {
        use std::io::Write as _;
        let mut w = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
        let opts: zip::write::SimpleFileOptions = Default::default();
        for (name, body) in entries {
            w.start_file(*name, opts).unwrap();
            w.write_all(body.as_bytes()).unwrap();
        }
        w.finish().unwrap().into_inner()
    }

    /// The boko/KF8 failure this whole path exists for: every document links
    /// `styles/style0000.css` and the archive has no such entry.
    #[test]
    fn dangling_stylesheet_is_reported() {
        let epub = zip_of(&[(
            "OEBPS/part0000.html",
            r#"<html><head><link rel="stylesheet" type="text/css" href="styles/style0000.css"/></head><body/></html>"#,
        )]);
        assert_eq!(
            dangling_stylesheets(&epub),
            vec!["OEBPS/styles/style0000.css"]
        );
    }

    #[test]
    fn present_stylesheet_is_not_reported() {
        let epub = zip_of(&[
            (
                "OEBPS/part0000.html",
                r#"<html><head><link rel="stylesheet" href="styles/style0000.css"/></head></html>"#,
            ),
            ("OEBPS/styles/style0000.css", ".pos { position: absolute; }"),
        ]);
        assert!(dangling_stylesheets(&epub).is_empty());
    }

    /// Each missing sheet is named once however many documents link it — a
    /// 353-part book would otherwise log the same href 353 times.
    #[test]
    fn a_missing_sheet_is_reported_once_across_documents() {
        let doc = r#"<link rel="stylesheet" href="../styles/s.css"/>"#;
        let epub = zip_of(&[("OEBPS/text/a.html", doc), ("OEBPS/text/b.html", doc)]);
        assert_eq!(dangling_stylesheets(&epub), vec!["OEBPS/styles/s.css"]);
    }

    /// A remote sheet is not an archive entry, so its absence is not a defect.
    #[test]
    fn remote_and_data_stylesheets_are_ignored() {
        let epub = zip_of(&[(
            "a.html",
            r#"<link rel="stylesheet" href="https://cdn.example/x.css"/>
               <link rel="stylesheet" href="data:text/css,body{}"/>"#,
        )]);
        assert!(dangling_stylesheets(&epub).is_empty());
    }

    /// `<link rel="icon">` and friends must not be mistaken for stylesheets.
    #[test]
    fn non_stylesheet_links_are_ignored() {
        let epub = zip_of(&[("a.html", r#"<link rel="icon" href="missing.png"/>"#)]);
        assert!(dangling_stylesheets(&epub).is_empty());
    }

    /// Words of Radiance ships without the `page-template.xpgt` it links, so
    /// every KEPUB made from it links it too. Re-converting cannot put the file
    /// there — flagging it would queue the book for repair on every sweep and
    /// cost the device a download each time.
    #[test]
    fn a_fault_the_source_already_had_is_not_a_defect() {
        let doc = r#"<link rel="stylesheet" href="page-template.xpgt"/>"#;
        let source = zip_of(&[("a.html", doc)]);
        let produced = zip_of(&[("a.html", doc)]);
        assert!(epub_defects(&produced, Some(&source)).is_empty());
        assert_eq!(epub_defects(&produced, None).len(), 1);
    }

    /// But a hole the conversion opened is still a defect, even when the source
    /// has holes of its own — that is the boko case, and the whole point.
    #[test]
    fn a_fault_the_conversion_introduced_is_still_reported() {
        let source = zip_of(&[("a.html", r#"<link rel="stylesheet" href="theirs.css"/>"#)]);
        let produced = zip_of(&[(
            "a.html",
            r#"<link rel="stylesheet" href="theirs.css"/>
               <link rel="stylesheet" href="styles/style0000.css"/>"#,
        )]);
        let reasons = epub_defects(&produced, Some(&source));
        assert_eq!(reasons.len(), 1);
        assert!(reasons[0].contains("styles/style0000.css"), "{reasons:?}");
        assert!(!reasons[0].contains("theirs.css"), "{reasons:?}");
    }

    #[test]
    fn hrefs_resolve_relative_to_the_linking_document() {
        assert_eq!(
            resolve_href("OEBPS/text/ch01.xhtml", "../styles/main.css"),
            "OEBPS/styles/main.css"
        );
        assert_eq!(
            resolve_href("OEBPS/part0000.html", "styles/s.css"),
            "OEBPS/styles/s.css"
        );
        assert_eq!(resolve_href("OEBPS/a.html", "/root.css"), "root.css");
    }

    /// Entry names are stored decoded, so the href has to be decoded to match.
    #[test]
    fn percent_escapes_are_decoded_before_matching() {
        let epub = zip_of(&[
            (
                "a.html",
                r#"<link rel="stylesheet" href="my%20styles.css"/>"#,
            ),
            ("my styles.css", "p{}"),
        ]);
        assert!(dangling_stylesheets(&epub).is_empty());
    }

    /// Attribute order and quoting vary by producer; the scan must not care.
    #[test]
    fn href_is_found_whatever_the_attribute_order_or_quoting() {
        assert_eq!(
            stylesheet_hrefs(r#"<link href='a.css' rel='stylesheet'>"#),
            vec!["a.css"]
        );
        assert_eq!(
            stylesheet_hrefs(r#"<LINK REL="Stylesheet" HREF = "b.css" />"#),
            vec!["b.css"]
        );
    }

    #[test]
    fn kob_ebook_routes_for_native_formats() {
        assert!(kob_ebook_handles(EbookFormat::Epub));
        assert!(kob_ebook_handles(EbookFormat::Mobi));
        assert!(kob_ebook_handles(EbookFormat::Azw3));
        assert!(!kob_ebook_handles(EbookFormat::Pdf));
        assert!(!kob_ebook_handles(EbookFormat::Fb2));
    }
}
