//! Checking that a device's saved place survives a re-conversion.
//!
//! A Kobo stores its position as a document plus a koboSpan id:
//!
//! ```json
//! "Location": { "Type": "KoboSpan", "Value": "kobo.1.1", "Source": "text/part0016.html" }
//! ```
//!
//! It reports that shape for a plain EPUB too — the firmware walks the document
//! and numbers the spans itself, whether or not the file contains any. That is
//! the whole reason the kepubifier numbers spans the way pgaskin/kepubify does:
//! written-in ids line up with the ones the firmware would have derived, so a
//! book that gains a KEPUB keeps its place.
//!
//! "Should" is not "does", and the position lives on the device where we cannot
//! inspect it. So rather than assume, this checks: does the document the device
//! named still exist in the new file, and does it still carry that span id? The
//! answer rides along on the conversion job, so a shifted position is something
//! you can see rather than something a reader discovers.

use serde_json::Value;
use sqlx::PgPool;
use uuid::Uuid;

/// Where a device last was: the document it names and the span within it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SavedPosition {
    pub device_name: String,
    pub source: String,
    pub span: String,
}

/// Every KoboSpan position saved against a book, one per device that has one.
///
/// Positions of any other `Type` are skipped: they are not anchored to markup
/// this conversion can move, so there is nothing to verify.
pub async fn saved_positions(pool: &PgPool, book_id: Uuid) -> Vec<SavedPosition> {
    let rows = sqlx::query!(
        r#"
        SELECT COALESCE(d.name, 'device') AS "device_name!", rs.client_payload
        FROM reading_states rs
        JOIN devices d ON d.id = rs.device_id
        WHERE rs.book_id = $1 AND d.device_type = 'kobo' AND rs.client_payload IS NOT NULL
        "#,
        book_id,
    )
    .fetch_all(pool)
    .await
    .unwrap_or_default();

    rows.into_iter()
        .filter_map(|r| {
            let location = r
                .client_payload?
                .get("CurrentBookmark")?
                .get("Location")?
                .clone();
            parse_location(&location).map(|(source, span)| SavedPosition {
                device_name: r.device_name,
                source,
                span,
            })
        })
        .collect()
}

fn parse_location(location: &Value) -> Option<(String, String)> {
    if location.get("Type")?.as_str()? != "KoboSpan" {
        return None;
    }
    let source = location.get("Source")?.as_str()?.to_string();
    let span = location.get("Value")?.as_str()?.to_string();
    (!source.is_empty() && !span.is_empty()).then_some((source, span))
}

/// Whether a saved position still points at something in these EPUB bytes.
///
/// Matched on the tail of the path rather than the whole of it: the device
/// reports the document as the OPF names it, which is not always the full zip
/// entry path, and a suffix match cannot pick the wrong document because zip
/// entry names are unique.
pub fn position_survives(epub: &[u8], position: &SavedPosition) -> bool {
    let Ok(mut archive) = zip::ZipArchive::new(std::io::Cursor::new(epub)) else {
        return false;
    };

    let wanted = position.source.trim_start_matches('/');
    let Some(entry_name) = archive
        .file_names()
        .filter_map(|name| name.ok().map(std::borrow::Cow::into_owned))
        .find(|name| name == wanted || name.ends_with(&format!("/{wanted}")))
    else {
        return false;
    };

    let Ok(mut entry) = archive.by_name(&entry_name) else {
        return false;
    };
    let mut body = String::new();
    if std::io::Read::read_to_string(&mut entry, &mut body).is_err() {
        return false;
    }

    body.contains(&format!("\"{}\"", position.span))
        || body.contains(&format!("'{}'", position.span))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn epub_with(doc_name: &str, body: &str) -> Vec<u8> {
        let mut zip = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
        zip.start_file(doc_name, zip::write::SimpleFileOptions::default())
            .unwrap();
        zip.write_all(body.as_bytes()).unwrap();
        zip.finish().unwrap().into_inner()
    }

    fn position(source: &str, span: &str) -> SavedPosition {
        SavedPosition {
            device_name: "Kobo".to_string(),
            source: source.to_string(),
            span: span.to_string(),
        }
    }

    #[test]
    fn finds_the_span_in_the_named_document() {
        let epub = epub_with(
            "text/part0016.html",
            r#"<p><span class="koboSpan" id="kobo.1.1">Hi.</span></p>"#,
        );
        assert!(position_survives(
            &epub,
            &position("text/part0016.html", "kobo.1.1")
        ));
    }

    #[test]
    fn matches_a_document_nested_deeper_than_the_device_reported() {
        let epub = epub_with(
            "OEBPS/text/part0016.html",
            r#"<span class="koboSpan" id="kobo.4.2">x</span>"#,
        );
        assert!(position_survives(
            &epub,
            &position("text/part0016.html", "kobo.4.2")
        ));
    }

    #[test]
    fn rejects_a_span_that_is_gone() {
        // The plate-page rewrite drops a document's spans — this is what that
        // looks like from here.
        let epub = epub_with("text/ill1.html", r#"<svg viewBox="0 0 800 1200"></svg>"#);
        assert!(!position_survives(
            &epub,
            &position("text/ill1.html", "kobo.1.1")
        ));
    }

    #[test]
    fn rejects_a_document_that_is_gone() {
        let epub = epub_with("text/part0001.html", r#"<span id="kobo.1.1">x</span>"#);
        assert!(!position_survives(
            &epub,
            &position("text/part0016.html", "kobo.1.1")
        ));
    }

    #[test]
    fn does_not_match_a_span_that_merely_shares_a_prefix() {
        let epub = epub_with("a.html", r#"<span id="kobo.1.11">x</span>"#);
        assert!(!position_survives(&epub, &position("a.html", "kobo.1.1")));
    }

    #[test]
    fn ignores_locations_that_are_not_kobospans() {
        let location = serde_json::json!({
            "Type": "KoboProgress", "Value": "0.42", "Source": "text/part0001.html"
        });
        assert_eq!(parse_location(&location), None);
    }

    #[test]
    fn reads_a_kobospan_location() {
        let location = serde_json::json!({
            "Type": "KoboSpan", "Value": "kobo.1.1", "Source": "text/part0016.html"
        });
        assert_eq!(
            parse_location(&location),
            Some(("text/part0016.html".to_string(), "kobo.1.1".to_string()))
        );
    }
}
