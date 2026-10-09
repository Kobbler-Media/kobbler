//! Calibre OPF (Dublin Core) parser and writer.
//!
//! Parses `metadata.opf` files using Dublin Core elements and Calibre extensions.

use super::{BookSidecar, SidecarFormat, SidecarMetadata};
use quick_xml::events::{BytesEnd, BytesStart, BytesText, Event};
use quick_xml::{Reader, Writer};
use std::io::Cursor;
use std::path::Path;

/// Parse a Calibre/OPF metadata file into `SidecarMetadata`.
pub fn parse_opf(content: &str) -> Result<SidecarMetadata, String> {
    let mut reader = Reader::from_str(content.trim());
    reader.config_mut().trim_text(true);

    let mut meta = SidecarMetadata {
        source_format: Some(SidecarFormat::CalibreOpf),
        ..Default::default()
    };
    let mut book = BookSidecar::default();

    let mut depth: Vec<String> = Vec::new();
    // When inside a dc:identifier with a scheme, store the scheme here
    let mut identifier_scheme: Option<String> = None;
    // Buffer for accumulating text across Text and GeneralRef events
    let mut text_buf = String::new();

    loop {
        match reader.read_event() {
            Ok(Event::Empty(ref e)) => {
                let raw_tag = String::from_utf8_lossy(e.name().as_ref()).to_string();
                let tag = strip_ns(&raw_tag);

                // Handle self-closing <meta name="calibre:series" content="..." />
                if tag == "meta" {
                    handle_meta_element(e, &mut book);
                }
            }
            Ok(Event::Start(ref e)) => {
                text_buf.clear();
                let raw_tag = String::from_utf8_lossy(e.name().as_ref()).to_string();
                let tag = strip_ns(&raw_tag);

                // Handle <meta> that has a start tag (unusual but possible)
                if tag == "meta" {
                    handle_meta_element(e, &mut book);
                }

                // Track identifier scheme for dc:identifier elements
                if tag == "identifier" {
                    identifier_scheme = extract_scheme(e);
                }

                depth.push(tag);
            }
            Ok(Event::Text(ref e)) => {
                if let Ok(decoded) = e.decode() {
                    text_buf.push_str(&decoded);
                }
            }
            Ok(Event::GeneralRef(ref e)) => {
                if let Ok(decoded) = e.decode() {
                    text_buf.push_str(&decoded);
                }
            }
            Ok(Event::End(_)) => {
                let text = text_buf.trim().to_string();
                text_buf.clear();

                if !text.is_empty() {
                    let current_tag = depth.last().map(|s| s.as_str()).unwrap_or("");

                    match current_tag {
                        "title" => meta.title = Some(text),
                        "creator" => book.authors.push(text),
                        "publisher" => book.publisher = Some(text),
                        "language" => book.language = Some(text),
                        "description" => book.description = Some(text),
                        "subject" => meta.genres.push(text),
                        "date" => {
                            if let Some(year) = text.get(..4).and_then(|y| y.parse::<u16>().ok()) {
                                meta.year = Some(year);
                            }
                        }
                        "identifier" => {
                            let scheme = identifier_scheme.as_deref().unwrap_or("");
                            match scheme {
                                "isbn" => meta.identifiers.isbn = Some(text),
                                "asin" | "amazon" => meta.identifiers.asin = Some(text),
                                "kobbler" => meta.identifiers.kobbler_id = Some(text),
                                _ => {
                                    // Detect ISBN from URN or bare 13-digit string
                                    if let Some(isbn) = text.strip_prefix("urn:isbn:") {
                                        meta.identifiers.isbn = Some(isbn.to_string());
                                    } else if text.len() == 13
                                        && text.starts_with("978")
                                        && meta.identifiers.isbn.is_none()
                                    {
                                        meta.identifiers.isbn = Some(text);
                                    }
                                }
                            }
                        }
                        _ => {}
                    }
                }

                if depth.last().is_some_and(|d| d == "identifier") {
                    identifier_scheme = None;
                }
                depth.pop();
            }
            Ok(Event::Eof) => break,
            Err(e) => return Err(format!("OPF parse error: {e}")),
            _ => {}
        }
    }

    meta.book = Some(book);
    Ok(meta)
}

/// Strip namespace prefix from a tag name (e.g. "dc:title" -> "title").
fn strip_ns(tag: &str) -> String {
    tag.rsplit_once(':')
        .map(|(_, local)| local)
        .unwrap_or(tag)
        .to_lowercase()
}

/// Extract opf:scheme attribute from a dc:identifier element.
fn extract_scheme(e: &BytesStart<'_>) -> Option<String> {
    for attr in e.attributes().flatten() {
        let key = String::from_utf8_lossy(attr.key.as_ref()).to_string();
        let local_key = strip_ns(&key);
        if local_key == "scheme" {
            return Some(String::from_utf8_lossy(&attr.value).to_lowercase());
        }
    }
    None
}

/// Handle a <meta name="calibre:..." content="..." /> element.
fn handle_meta_element(e: &BytesStart<'_>, book: &mut BookSidecar) {
    let mut name = String::new();
    let mut content_val = String::new();
    for attr in e.attributes().flatten() {
        let key = String::from_utf8_lossy(attr.key.as_ref()).to_string();
        let val = String::from_utf8_lossy(&attr.value).to_string();
        match key.as_str() {
            "name" => name = val,
            "content" => content_val = val,
            _ => {}
        }
    }
    match name.as_str() {
        "calibre:series" => book.series = Some(content_val),
        "calibre:series_index" => {
            book.series_index = content_val.parse::<f64>().ok();
        }
        _ => {}
    }
}

/// Write `SidecarMetadata` as a Calibre-compatible OPF file.
pub async fn write_opf(path: &Path, meta: &SidecarMetadata) -> Result<(), std::io::Error> {
    let xml = build_opf_xml(meta).map_err(std::io::Error::other)?;
    let path = path.to_owned();
    tokio::task::spawn_blocking(move || std::fs::write(&path, xml))
        .await
        .map_err(|e| std::io::Error::other(format!("Join error: {e}")))?
}

fn build_opf_xml(meta: &SidecarMetadata) -> Result<String, String> {
    let mut writer = Writer::new_with_indent(Cursor::new(Vec::new()), b' ', 2);

    writer
        .write_event(Event::Decl(quick_xml::events::BytesDecl::new(
            "1.0",
            Some("UTF-8"),
            None,
        )))
        .map_err(|e| e.to_string())?;

    let mut package = BytesStart::new("package");
    package.push_attribute(("xmlns", "http://www.idpf.org/2007/opf"));
    package.push_attribute(("xmlns:dc", "http://purl.org/dc/elements/1.1/"));
    package.push_attribute(("xmlns:opf", "http://www.idpf.org/2007/opf"));
    writer
        .write_event(Event::Start(package))
        .map_err(|e| e.to_string())?;

    writer
        .write_event(Event::Start(BytesStart::new("metadata")))
        .map_err(|e| e.to_string())?;

    if let Some(title) = &meta.title {
        write_dc_element(&mut writer, "dc:title", title)?;
    }

    let book = meta.book.as_ref();

    if let Some(b) = book {
        for author in &b.authors {
            write_dc_element(&mut writer, "dc:creator", author)?;
        }
        if let Some(pub_) = &b.publisher {
            write_dc_element(&mut writer, "dc:publisher", pub_)?;
        }
        if let Some(lang) = &b.language {
            write_dc_element(&mut writer, "dc:language", lang)?;
        }
        if let Some(desc) = &b.description {
            write_dc_element(&mut writer, "dc:description", desc)?;
        }
    }

    if let Some(year) = meta.year {
        write_dc_element(&mut writer, "dc:date", &format!("{year}-01-01"))?;
    }

    for genre in &meta.genres {
        write_dc_element(&mut writer, "dc:subject", genre)?;
    }

    if let Some(isbn) = &meta.identifiers.isbn {
        let mut elem = BytesStart::new("dc:identifier");
        elem.push_attribute(("opf:scheme", "ISBN"));
        writer
            .write_event(Event::Start(elem))
            .map_err(|e| e.to_string())?;
        writer
            .write_event(Event::Text(BytesText::new(isbn)))
            .map_err(|e| e.to_string())?;
        writer
            .write_event(Event::End(BytesEnd::new("dc:identifier")))
            .map_err(|e| e.to_string())?;
    }
    if let Some(asin) = &meta.identifiers.asin {
        let mut elem = BytesStart::new("dc:identifier");
        elem.push_attribute(("opf:scheme", "ASIN"));
        writer
            .write_event(Event::Start(elem))
            .map_err(|e| e.to_string())?;
        writer
            .write_event(Event::Text(BytesText::new(asin)))
            .map_err(|e| e.to_string())?;
        writer
            .write_event(Event::End(BytesEnd::new("dc:identifier")))
            .map_err(|e| e.to_string())?;
    }
    if let Some(kobbler_id) = &meta.identifiers.kobbler_id {
        let mut elem = BytesStart::new("dc:identifier");
        elem.push_attribute(("opf:scheme", "KOBBLER"));
        writer
            .write_event(Event::Start(elem))
            .map_err(|e| e.to_string())?;
        writer
            .write_event(Event::Text(BytesText::new(kobbler_id)))
            .map_err(|e| e.to_string())?;
        writer
            .write_event(Event::End(BytesEnd::new("dc:identifier")))
            .map_err(|e| e.to_string())?;
    }

    // Calibre series metadata
    if let Some(b) = book {
        if let Some(series) = &b.series {
            let mut elem = BytesStart::new("meta");
            elem.push_attribute(("name", "calibre:series"));
            elem.push_attribute(("content", series.as_str()));
            writer
                .write_event(Event::Empty(elem))
                .map_err(|e| e.to_string())?;
        }
        if let Some(idx) = b.series_index {
            let mut elem = BytesStart::new("meta");
            elem.push_attribute(("name", "calibre:series_index"));
            elem.push_attribute(("content", idx.to_string().as_str()));
            writer
                .write_event(Event::Empty(elem))
                .map_err(|e| e.to_string())?;
        }
    }

    writer
        .write_event(Event::End(BytesEnd::new("metadata")))
        .map_err(|e| e.to_string())?;
    writer
        .write_event(Event::End(BytesEnd::new("package")))
        .map_err(|e| e.to_string())?;

    String::from_utf8(writer.into_inner().into_inner()).map_err(|e| e.to_string())
}

fn write_dc_element(
    writer: &mut Writer<Cursor<Vec<u8>>>,
    tag: &str,
    text: &str,
) -> Result<(), String> {
    writer
        .write_event(Event::Start(BytesStart::new(tag)))
        .map_err(|e| e.to_string())?;
    writer
        .write_event(Event::Text(BytesText::new(text)))
        .map_err(|e| e.to_string())?;
    writer
        .write_event(Event::End(BytesEnd::new(tag)))
        .map_err(|e| e.to_string())?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sidecar::SidecarIdentifiers;

    #[test]
    fn test_parse_opf() {
        let opf = r#"<?xml version="1.0" encoding="UTF-8"?>
<package xmlns="http://www.idpf.org/2007/opf" xmlns:dc="http://purl.org/dc/elements/1.1/" xmlns:opf="http://www.idpf.org/2007/opf">
  <metadata>
    <dc:title>The Great Novel</dc:title>
    <dc:creator opf:role="aut">Jane Author</dc:creator>
    <dc:publisher>Big Publisher</dc:publisher>
    <dc:language>en</dc:language>
    <dc:description>A great novel about things.</dc:description>
    <dc:subject>Fiction</dc:subject>
    <dc:subject>Literary</dc:subject>
    <dc:date>2023-06-15</dc:date>
    <dc:identifier opf:scheme="ISBN">9781234567890</dc:identifier>
    <dc:identifier opf:scheme="ASIN">B00EXAMPLE</dc:identifier>
    <meta name="calibre:series" content="The Series" />
    <meta name="calibre:series_index" content="3.0" />
  </metadata>
</package>"#;

        let meta = parse_opf(opf).expect("Failed to parse OPF");

        assert_eq!(meta.title.as_deref(), Some("The Great Novel"));
        assert_eq!(meta.year, Some(2023));
        assert_eq!(meta.identifiers.isbn.as_deref(), Some("9781234567890"));
        assert_eq!(meta.identifiers.asin.as_deref(), Some("B00EXAMPLE"));
        assert!(meta.genres.contains(&"Fiction".to_string()));
        assert!(meta.genres.contains(&"Literary".to_string()));

        let book = meta.book.expect("Should have book metadata");
        assert_eq!(book.authors, vec!["Jane Author"]);
        assert_eq!(book.publisher.as_deref(), Some("Big Publisher"));
        assert_eq!(book.language.as_deref(), Some("en"));
        assert_eq!(book.series.as_deref(), Some("The Series"));
        assert_eq!(book.series_index, Some(3.0));
        assert_eq!(
            book.description.as_deref(),
            Some("A great novel about things.")
        );
    }

    #[test]
    fn test_parse_opf_urn_isbn() {
        let opf = r#"<?xml version="1.0"?>
<package xmlns="http://www.idpf.org/2007/opf" xmlns:dc="http://purl.org/dc/elements/1.1/">
  <metadata>
    <dc:title>URN Test</dc:title>
    <dc:identifier>urn:isbn:9780123456789</dc:identifier>
  </metadata>
</package>"#;

        let meta = parse_opf(opf).expect("Failed to parse");
        assert_eq!(meta.identifiers.isbn.as_deref(), Some("9780123456789"));
    }

    #[test]
    fn test_kobbler_id_roundtrip_opf() {
        let original = SidecarMetadata {
            title: Some("Kobbler Test".to_string()),
            identifiers: SidecarIdentifiers {
                kobbler_id: Some("uuid-abc-123".to_string()),
                isbn: Some("9780000000001".to_string()),
                ..Default::default()
            },
            book: Some(BookSidecar {
                authors: vec!["Author".to_string()],
                ..Default::default()
            }),
            ..Default::default()
        };

        let xml = build_opf_xml(&original).expect("Failed to build OPF");
        let parsed = parse_opf(&xml).expect("Failed to parse written OPF");

        assert_eq!(
            parsed.identifiers.kobbler_id.as_deref(),
            Some("uuid-abc-123")
        );
        assert_eq!(parsed.identifiers.isbn.as_deref(), Some("9780000000001"));
    }

    #[test]
    fn test_roundtrip_opf() {
        let original = SidecarMetadata {
            source_format: Some(SidecarFormat::CalibreOpf),
            title: Some("Round Trip".to_string()),
            year: Some(2024),
            genres: vec!["Sci-Fi".to_string()],
            identifiers: SidecarIdentifiers {
                isbn: Some("9780000000000".to_string()),
                ..Default::default()
            },
            book: Some(BookSidecar {
                authors: vec!["Test Author".to_string()],
                publisher: Some("Test Pub".to_string()),
                language: Some("en".to_string()),
                series: Some("Test Series".to_string()),
                series_index: Some(1.0),
                description: Some("A test".to_string()),
                ..Default::default()
            }),
            ..Default::default()
        };

        let xml = build_opf_xml(&original).expect("Failed to build OPF");
        let parsed = parse_opf(&xml).expect("Failed to parse written OPF");

        assert_eq!(parsed.title, original.title);
        assert_eq!(parsed.identifiers.isbn, original.identifiers.isbn);
        let book = parsed.book.expect("Should have book");
        assert_eq!(book.authors, vec!["Test Author"]);
        assert_eq!(book.series.as_deref(), Some("Test Series"));
        assert_eq!(book.series_index, Some(1.0));
    }
}
