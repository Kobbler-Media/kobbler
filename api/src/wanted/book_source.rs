//! How a wanted book is keyed in Kobload. Kobload keeps one watch pattern per
//! `(source_system, source_id)`, and a book wanted as both an ebook and an
//! audiobook needs a pattern for each, so those two get a format suffix.

use uuid::Uuid;

use super::SOURCE_SYSTEM;

const FORMATS: [&str; 2] = ["ebook", "audiobook"];

pub fn want_sources(book_id: Uuid, want_format: &str) -> Vec<(&'static str, String)> {
    match want_format {
        "both" => FORMATS
            .iter()
            .map(|format| (*format, format!("{book_id}:{format}")))
            .collect(),
        "audiobook" => vec![("audiobook", book_id.to_string())],
        _ => vec![("ebook", book_id.to_string())],
    }
}

pub fn all_source_ids(book_id: Uuid) -> Vec<String> {
    std::iter::once(book_id.to_string())
        .chain(FORMATS.iter().map(|format| format!("{book_id}:{format}")))
        .collect()
}

/// Wanting a book creates watch patterns, so anything that takes the want back
/// has to tear them down or Kobload keeps hunting for a book nobody is waiting
/// for any more.
pub async fn delete_book_patterns(publisher: &nats_jobs::Publisher, book_id: Uuid) {
    for source_id in all_source_ids(book_id) {
        let delete = nats_jobs::WatchPatternDelete {
            source_system: SOURCE_SYSTEM.to_string(),
            source_id,
        };
        if let Ok(payload) = serde_json::to_value(&delete)
            && let Err(e) = publisher
                .enqueue_job("kobload", "watch_pattern_delete", payload)
                .await
        {
            tracing::warn!(book_id = %book_id, error = %e, "Failed to queue a watch pattern delete");
        }
    }
    tracing::info!(book_id = %book_id, "Watch pattern deletes queued");
}

/// Whether `source_id` is a key the book's want is kept under now. Kobload
/// reports every pattern it deletes, including ones under a key the book has
/// moved off, and only losing a current one means the book is no longer wanted.
pub fn is_current_source(book_id: Uuid, want_format: &str, source_id: &str) -> bool {
    want_sources(book_id, want_format)
        .iter()
        .any(|(_, id)| id == source_id)
}

pub fn entity_id(source_id: &str) -> Option<Uuid> {
    let bare = match source_id.rsplit_once(':') {
        Some((id, format)) if FORMATS.contains(&format) => id,
        _ => source_id,
    };
    Uuid::parse_str(bare).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_format_of_a_book_has_its_own_key() {
        let id = Uuid::nil();
        assert_eq!(want_sources(id, "ebook"), vec![("ebook", id.to_string())]);
        assert_eq!(
            want_sources(id, "audiobook"),
            vec![("audiobook", id.to_string())]
        );
        assert_eq!(
            want_sources(id, "both"),
            vec![
                ("ebook", format!("{id}:ebook")),
                ("audiobook", format!("{id}:audiobook")),
            ]
        );
        assert_eq!(all_source_ids(id).len(), 3);
    }

    #[test]
    fn source_ids_resolve_to_their_entity() {
        let id = Uuid::now_v7();
        for source in [
            id.to_string(),
            format!("{id}:ebook"),
            format!("{id}:audiobook"),
        ] {
            assert_eq!(entity_id(&source), Some(id), "{source}");
        }
        assert_eq!(entity_id(&format!("{id}:paperback")), None);
        assert_eq!(entity_id("not-a-uuid"), None);
    }

    #[test]
    fn only_a_current_key_is_the_books_own() {
        let id = Uuid::now_v7();
        assert!(!is_current_source(id, "both", &id.to_string()));
        assert!(is_current_source(id, "both", &format!("{id}:audiobook")));
        assert!(is_current_source(id, "ebook", &id.to_string()));
        assert!(!is_current_source(id, "ebook", &format!("{id}:ebook")));
    }
}
