use std::io::Write;

/// One chapter's XHTML: a heading over its paragraphs.
pub fn chapter_xhtml(heading: &str, paragraphs: &[&str]) -> String {
    let body: String = paragraphs.iter().map(|p| format!("<p>{p}</p>")).collect();
    format!(
        r#"<?xml version="1.0" encoding="utf-8"?>
<html xmlns="http://www.w3.org/1999/xhtml"><head><title>{heading}</title></head>
<body><h1>{heading}</h1>{body}</body></html>"#
    )
}

/// A minimal valid EPUB 3 whose spine is `chapters` in order, at
/// `OEBPS/ch{n}.xhtml` (n from 1).
pub fn epub_with(chapters: &[String]) -> Vec<u8> {
    epub_book("Pack Test", Some("Test Author"), None, "en", chapters)
}

/// [`epub_with`], with the title, creator, date and language the OPF declares.
pub fn epub_book(
    title: &str,
    creator: Option<&str>,
    date: Option<&str>,
    language: &str,
    chapters: &[String],
) -> Vec<u8> {
    let mut zip = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
    let opts = zip::write::SimpleFileOptions::default();
    let mut put = |name: &str, body: &str| {
        zip.start_file(name, opts).unwrap();
        zip.write_all(body.as_bytes()).unwrap();
    };
    put("mimetype", "application/epub+zip");
    put(
        "META-INF/container.xml",
        r#"<?xml version="1.0"?><container version="1.0" xmlns="urn:oasis:names:tc:opendocument:xmlns:container"><rootfiles><rootfile full-path="OEBPS/content.opf" media-type="application/oebps-package+xml"/></rootfiles></container>"#,
    );
    let items: String = (1..=chapters.len())
        .map(|n| {
            format!(r#"<item id="ch{n}" href="ch{n}.xhtml" media-type="application/xhtml+xml"/>"#)
        })
        .collect();
    let creator = creator
        .map(|c| format!("<dc:creator>{c}</dc:creator>"))
        .unwrap_or_default();
    let date = date
        .map(|d| format!("<dc:date>{d}</dc:date>"))
        .unwrap_or_default();
    let spine: String = (1..=chapters.len())
        .map(|n| format!(r#"<itemref idref="ch{n}"/>"#))
        .collect();
    put(
        "OEBPS/content.opf",
        &format!(
            r#"<?xml version="1.0"?><package xmlns="http://www.idpf.org/2007/opf" version="3.0" unique-identifier="id"><metadata xmlns:dc="http://purl.org/dc/elements/1.1/"><dc:identifier id="id">pack-test</dc:identifier><dc:title>{title}</dc:title>{creator}{date}<dc:language>{language}</dc:language></metadata><manifest>{items}</manifest><spine>{spine}</spine></package>"#
        ),
    );
    for (i, chapter) in chapters.iter().enumerate() {
        put(&format!("OEBPS/ch{}.xhtml", i + 1), chapter);
    }
    zip.finish().unwrap().into_inner()
}
