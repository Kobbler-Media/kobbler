//! EPUB text extraction.
//!
//! Extracts plain text from EPUB files by parsing the ZIP container,
//! reading the OPF spine for reading order, and stripping HTML tags
//! from each XHTML document.

use quick_xml::Reader;
use quick_xml::events::Event;
use std::collections::HashMap;
use std::io::Read;
use std::path::Path;
use thiserror::Error;

/// A single extracted text segment from an EPUB.
#[derive(Debug, Clone)]
pub struct EbookTextSegment {
    pub segment_index: i32,
    pub text: String,
    pub chapter_title: Option<String>,
}

/// Errors that can occur during EPUB text extraction.
#[derive(Error, Debug)]
pub enum ExtractError {
    #[error("I/O error: {0}")]
    IoError(#[from] std::io::Error),

    #[error("ZIP error: {0}")]
    ZipError(#[from] zip::result::ZipError),

    #[error("Parse error: {0}")]
    ParseError(String),
}

/// Extract text segments from an EPUB file.
///
/// Opens the EPUB as a ZIP, finds the OPF via `META-INF/container.xml`,
/// reads the spine ordering, then extracts text from each XHTML document
/// in spine order.
pub fn extract_epub_text(path: &Path) -> Result<Vec<EbookTextSegment>, ExtractError> {
    let file = std::fs::File::open(path)?;
    let mut archive = zip::ZipArchive::new(file)?;

    // Step 1: Find OPF path from container.xml
    let opf_path = read_opf_path(&mut archive)?;
    let opf_dir = opf_path
        .rfind('/')
        .map(|i| &opf_path[..i + 1])
        .unwrap_or("")
        .to_string();

    // Step 2: Parse OPF to get manifest and spine
    let opf_content = read_zip_entry(&mut archive, &opf_path)?;
    let (manifest, spine) = parse_opf(&opf_content)?;

    // Step 3: Extract text from each spine item
    let mut segments = Vec::new();
    let mut segment_index: i32 = 0;

    for idref in spine.iter() {
        let href = match manifest.get(idref.as_str()) {
            Some(h) => h,
            None => continue,
        };

        // Resolve href relative to OPF directory
        let full_path = if let Some(stripped) = href.strip_prefix('/') {
            stripped.to_string()
        } else {
            format!("{}{}", opf_dir, href)
        };

        let content = match read_zip_entry(&mut archive, &full_path) {
            Ok(c) => c,
            Err(_) => continue,
        };

        let doc_segments = extract_text_from_xhtml(&content, &mut segment_index);
        segments.extend(doc_segments);
    }

    Ok(segments)
}

/// Read the OPF path from META-INF/container.xml.
fn read_opf_path(archive: &mut zip::ZipArchive<std::fs::File>) -> Result<String, ExtractError> {
    let content = read_zip_entry(archive, "META-INF/container.xml")?;
    let mut reader = Reader::from_str(&content);
    reader.config_mut().trim_text(true);

    loop {
        match reader.read_event() {
            Ok(Event::Empty(ref e)) | Ok(Event::Start(ref e)) => {
                let name = e.name();
                let tag = String::from_utf8_lossy(name.as_ref());
                if tag.ends_with("rootfile") || tag == "rootfile" {
                    for attr in e.attributes().flatten() {
                        if attr.key.as_ref() == b"full-path" {
                            return Ok(String::from_utf8_lossy(&attr.value).to_string());
                        }
                    }
                }
            }
            Ok(Event::Eof) => break,
            Err(e) => {
                return Err(ExtractError::ParseError(format!(
                    "container.xml parse error: {e}"
                )));
            }
            _ => {}
        }
    }

    Err(ExtractError::ParseError(
        "No rootfile found in container.xml".to_string(),
    ))
}

/// Read a UTF-8 text entry from the ZIP archive.
fn read_zip_entry(
    archive: &mut zip::ZipArchive<std::fs::File>,
    name: &str,
) -> Result<String, ExtractError> {
    let mut entry = archive.by_name(name)?;
    let mut buf = String::new();
    entry.read_to_string(&mut buf)?;
    Ok(buf)
}

/// Parse the OPF to extract manifest (id -> href) and spine (ordered idrefs).
fn parse_opf(content: &str) -> Result<(HashMap<String, String>, Vec<String>), ExtractError> {
    let mut reader = Reader::from_str(content);
    reader.config_mut().trim_text(true);

    let mut manifest: HashMap<String, String> = HashMap::new();
    let mut spine: Vec<String> = Vec::new();

    loop {
        match reader.read_event() {
            Ok(Event::Empty(ref e)) | Ok(Event::Start(ref e)) => {
                let tag = String::from_utf8_lossy(e.name().as_ref()).to_string();
                let local = strip_ns(&tag);

                if local == "item" {
                    let mut id = None;
                    let mut href = None;
                    for attr in e.attributes().flatten() {
                        match attr.key.as_ref() {
                            b"id" => id = Some(String::from_utf8_lossy(&attr.value).to_string()),
                            b"href" => {
                                href = Some(String::from_utf8_lossy(&attr.value).to_string())
                            }
                            _ => {}
                        }
                    }
                    if let Some(id) = id
                        && let Some(href) = href
                    {
                        // URL-decode the href
                        let decoded = percent_decode(&href);
                        manifest.insert(id, decoded);
                    }
                } else if local == "itemref" {
                    for attr in e.attributes().flatten() {
                        if attr.key.as_ref() == b"idref" {
                            spine.push(String::from_utf8_lossy(&attr.value).to_string());
                        }
                    }
                }
            }
            Ok(Event::Eof) => break,
            Err(e) => {
                return Err(ExtractError::ParseError(format!("OPF parse error: {e}")));
            }
            _ => {}
        }
    }

    if spine.is_empty() {
        return Err(ExtractError::ParseError("Empty spine in OPF".to_string()));
    }

    Ok((manifest, spine))
}

/// Strip XML namespace prefix from a tag name.
fn strip_ns(tag: &str) -> &str {
    tag.rsplit_once(':').map_or(tag, |(_, local)| local)
}

/// Simple percent-decoding for href values.
fn percent_decode(input: &str) -> String {
    let mut result = Vec::with_capacity(input.len());
    let bytes = input.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%'
            && i + 2 < bytes.len()
            && let Ok(val) = u8::from_str_radix(&String::from_utf8_lossy(&bytes[i + 1..i + 3]), 16)
        {
            result.push(val);
            i += 3;
            continue;
        }
        result.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&result).into_owned()
}

/// Extract text segments from an XHTML document.
///
/// Tracks heading elements for chapter titles and splits text by paragraphs.
/// Generates CFI-like references for each segment.
fn extract_text_from_xhtml(content: &str, segment_index: &mut i32) -> Vec<EbookTextSegment> {
    let mut reader = Reader::from_str(content);
    reader.config_mut().trim_text(true);

    let mut segments = Vec::new();
    let mut current_chapter: Option<String> = None;
    let mut current_text = String::new();
    let mut depth: Vec<String> = Vec::new();
    let mut in_heading = false;
    let mut heading_text = String::new();
    // Track when we're inside <body> to avoid extracting <head> content
    let mut in_body = false;

    loop {
        match reader.read_event() {
            Ok(Event::Start(ref e)) => {
                let tag = String::from_utf8_lossy(e.name().as_ref()).to_string();
                let local = strip_ns(&tag).to_lowercase();

                if local == "body" {
                    in_body = true;
                }

                if in_body {
                    // Check for heading tags
                    if matches!(local.as_str(), "h1" | "h2" | "h3" | "h4" | "h5" | "h6") {
                        // Flush any pending text before heading
                        flush_segment(
                            &mut current_text,
                            &current_chapter,
                            segment_index,
                            &mut segments,
                        );
                        in_heading = true;
                        heading_text.clear();
                    }

                    // Block elements start a new paragraph
                    if is_block_element(&local) && !current_text.is_empty() {
                        flush_segment(
                            &mut current_text,
                            &current_chapter,
                            segment_index,
                            &mut segments,
                        );
                    }
                }

                depth.push(local);
            }
            Ok(Event::End(ref e)) => {
                let tag = String::from_utf8_lossy(e.name().as_ref()).to_string();
                let local = strip_ns(&tag).to_lowercase();

                if local == "body" {
                    in_body = false;
                }

                if in_body || local == "body" {
                    if in_heading
                        && matches!(local.as_str(), "h1" | "h2" | "h3" | "h4" | "h5" | "h6")
                    {
                        in_heading = false;
                        let title = heading_text.trim().to_string();
                        if !title.is_empty() {
                            current_chapter = Some(title);
                        }
                    }

                    // Block elements end a paragraph
                    if is_block_element(&local) && !current_text.is_empty() {
                        flush_segment(
                            &mut current_text,
                            &current_chapter,
                            segment_index,
                            &mut segments,
                        );
                    }
                }

                depth.pop();
            }
            Ok(Event::Text(ref e)) => {
                if in_body && let Ok(decoded) = e.decode() {
                    let text = decoded.trim();
                    if !text.is_empty() {
                        if in_heading {
                            if !heading_text.is_empty() {
                                heading_text.push(' ');
                            }
                            heading_text.push_str(text);
                        }
                        if !current_text.is_empty() {
                            current_text.push(' ');
                        }
                        current_text.push_str(text);
                    }
                }
            }
            Ok(Event::Eof) => break,
            _ => {}
        }
    }

    // Flush any remaining text
    flush_segment(
        &mut current_text,
        &current_chapter,
        segment_index,
        &mut segments,
    );

    segments
}

/// Flush accumulated text into a new segment.
fn flush_segment(
    text: &mut String,
    chapter_title: &Option<String>,
    segment_index: &mut i32,
    segments: &mut Vec<EbookTextSegment>,
) {
    let trimmed = text.trim().to_string();
    if trimmed.is_empty() {
        return;
    }

    segments.push(EbookTextSegment {
        segment_index: *segment_index,
        text: trimmed,
        chapter_title: chapter_title.clone(),
    });

    *segment_index += 1;
    text.clear();
}

/// Check if an HTML tag is a block-level element that should cause paragraph breaks.
fn is_block_element(tag: &str) -> bool {
    matches!(
        tag,
        "p" | "div"
            | "section"
            | "article"
            | "blockquote"
            | "pre"
            | "li"
            | "dt"
            | "dd"
            | "h1"
            | "h2"
            | "h3"
            | "h4"
            | "h5"
            | "h6"
            | "hr"
            | "figure"
            | "figcaption"
            | "header"
            | "footer"
            | "nav"
            | "aside"
            | "main"
            | "tr"
    )
}

/// Extract a short, distinctive text sample from an EPUB for content-based identification.
///
/// Skips front matter (title pages, copyright, TOC) and returns the first
/// substantive paragraph of actual prose content, truncated to `max_chars`.
pub fn extract_content_sample(
    path: &Path,
    max_chars: usize,
) -> Result<Option<String>, ExtractError> {
    let segments = extract_epub_text(path)?;

    const FRONT_MATTER: &[&str] = &[
        "copyright",
        "table of contents",
        "contents",
        "dedication",
        "acknowledgment",
        "also by",
        "about the author",
        "title page",
        "cover",
        "epigraph",
        "foreword",
        "preface",
    ];

    for segment in &segments {
        // Skip very early segments (likely title/copyright)
        if segment.segment_index < 3 {
            continue;
        }

        // Skip segments with front-matter chapter titles
        if let Some(ref title) = segment.chapter_title {
            let lower = title.to_lowercase();
            if FRONT_MATTER.iter().any(|fm| lower.contains(fm)) {
                continue;
            }
        }

        // Need at least 50 chars of actual text
        let text = segment.text.trim();
        if text.len() < 50 {
            continue;
        }

        // Truncate to max_chars at a word boundary
        let sample = if text.len() > max_chars {
            match text[..max_chars].rfind(' ') {
                Some(pos) => &text[..pos],
                None => &text[..max_chars],
            }
        } else {
            text
        };

        return Ok(Some(sample.to_string()));
    }

    Ok(None)
}
