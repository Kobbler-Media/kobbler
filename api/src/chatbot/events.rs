//! The seam between the loop and whatever is delivering the answer.
//!
//! The loop emits these and knows nothing else. The Command Palette maps them
//! onto the SSE frames it already consumes; Google Chat ignores [`TurnEvent::Delta`]
//! entirely (it cannot stream) and coalesces the rest into edits of one message.
//! Anything that only one transport can do — SSE framing, card JSON — belongs
//! above this file, not in it.

use serde::Serialize;
use serde_json::Value;
use tokio::sync::mpsc;

/// A library item the answer cited, enriched with a navigation URL.
///
/// The field names are the wire contract the Command Palette parses
/// (`stores/chat.svelte.ts` reads `cover_url`, not `coverUrl`), so renaming one
/// is a client change.
#[derive(Debug, Clone, Serialize, PartialEq)]
#[cfg_attr(test, derive(serde::Deserialize))]
pub struct Source {
    #[serde(rename = "type")]
    pub item_type: String,
    pub id: String,
    pub title: String,
    pub url: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cover_url: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub year: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum TurnEvent {
    /// User-visible answer text. Never `reasoning_content`.
    Delta(String),
    ToolStarted {
        name: String,
        summary: String,
    },
    ToolFinished {
        name: String,
        detail: String,
        outputs: Value,
    },
    Sources(Vec<Source>),
    /// The turn's whole answer, once. Transports that cannot stream render this
    /// and ignore every `Delta`.
    Done {
        text: String,
    },
    Failed(String),
}

pub type TurnSender = mpsc::Sender<TurnEvent>;
pub type TurnReceiver = mpsc::Receiver<TurnEvent>;

/// A turn produces far more deltas than any transport needs buffered; the
/// bound is backpressure, not a queue, and a slow consumer should slow the loop
/// rather than grow memory.
pub fn channel() -> (TurnSender, TurnReceiver) {
    mpsc::channel(64)
}

/// Compute the frontend navigation URL for a cited item.
pub fn source_url(item_type: &str, id: &str) -> String {
    match item_type {
        "book" => format!("/library/books/{id}"),
        "movie" => format!("/movies/{id}"),
        "tv_show" => format!("/tv/{id}"),
        "tv_season" => format!("/tv/seasons/{id}"),
        "tv_episode" => format!("/tv/episodes/{id}"),
        "author" => format!("/authors/{id}"),
        "series" => format!("/series/{id}"),
        _ => format!("/{item_type}/{id}"),
    }
}

/// Build a [`Source`] from a raw search document.
///
/// Lifted verbatim from the Meili-proxy chat route it replaces: the enriched
/// citation is the best part of that UX and its inference rules were tuned
/// against real documents.
pub fn source_from_doc(doc: &Value) -> Option<Source> {
    let obj = doc.as_object()?;

    let item_type = obj
        .get("_federation")
        .and_then(|f| f.get("indexUid"))
        .and_then(Value::as_str)
        .or_else(|| obj.get("type").and_then(Value::as_str))
        .unwrap_or_else(|| infer_item_type(obj));

    let id = obj
        .get("id")
        .and_then(|v| v.as_str().map(String::from).or_else(|| Some(v.to_string())))?;
    let title = obj
        .get("title")
        .or_else(|| obj.get("name"))
        .and_then(Value::as_str)?;

    let cover_url = obj
        .get("poster_url")
        .or_else(|| obj.get("cover_url"))
        .or_else(|| obj.get("image_url"))
        .and_then(Value::as_str)
        .map(String::from);

    let year = obj.get("year").and_then(Value::as_i64);
    let description = build_source_description(item_type, obj);

    Some(Source {
        url: source_url(item_type, &id),
        item_type: item_type.to_string(),
        id,
        title: title.to_string(),
        cover_url,
        year,
        description,
    })
}

fn infer_item_type(obj: &serde_json::Map<String, Value>) -> &'static str {
    if obj.contains_key("episodes_watched")
        || obj.contains_key("total_episodes")
        || obj.contains_key("available_episode_count")
    {
        return "tv_show";
    }
    if obj.contains_key("display_title") || obj.contains_key("episode_code") {
        return "tv_episode";
    }
    if obj.contains_key("directors") || obj.contains_key("watched") {
        return "movie";
    }
    if obj.contains_key("authors")
        || obj.contains_key("has_audiobook")
        || obj.contains_key("series_name")
    {
        return "book";
    }
    if obj.contains_key("book_count") && !obj.contains_key("title") {
        return "author";
    }
    if obj.contains_key("book_count") {
        return "series";
    }
    if obj.contains_key("movie_count") || obj.contains_key("tv_show_count") {
        return "person";
    }
    "unknown"
}

fn build_source_description(
    item_type: &str,
    obj: &serde_json::Map<String, Value>,
) -> Option<String> {
    match item_type {
        "movie" => {
            let directors = joined(obj, "directors", usize::MAX);
            let genres = joined(obj, "genres", 2);
            directors.or(genres)
        }
        "book" => joined(obj, "authors", usize::MAX),
        "tv_show" => {
            let network = obj.get("network").and_then(Value::as_str);
            let status = obj.get("status").and_then(Value::as_str);
            let parts: Vec<&str> = [network, status].into_iter().flatten().collect();
            match parts.is_empty() {
                true => None,
                false => Some(parts.join(" · ")),
            }
        }
        "tv_episode" => obj
            .get("show_title")
            .and_then(Value::as_str)
            .map(String::from),
        "author" => obj
            .get("book_count")
            .and_then(Value::as_i64)
            .map(|n| format!("{n} book{}", if n != 1 { "s" } else { "" })),
        _ => None,
    }
}

fn joined(obj: &serde_json::Map<String, Value>, key: &str, take: usize) -> Option<String> {
    obj.get(key)
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .filter_map(Value::as_str)
                .take(take)
                .collect::<Vec<_>>()
                .join(", ")
        })
        .filter(|s| !s.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn a_book_document_keeps_its_library_navigation_url() {
        let s = source_from_doc(&json!({
            "id": "019cdce6-c787-7bf0-8dd8-85b7b15524a0",
            "title": "The Stand",
            "authors": ["Stephen King"],
            "cover_url": "/api/v1/library/books/019cdce6-c787-7bf0-8dd8-85b7b15524a0/cover",
        }))
        .expect("a titled document is a source");
        assert_eq!(s.item_type, "book");
        assert_eq!(s.url, "/library/books/019cdce6-c787-7bf0-8dd8-85b7b15524a0");
        assert_eq!(s.description.as_deref(), Some("Stephen King"));
        assert!(s.cover_url.is_some());
    }

    #[test]
    fn the_federation_index_uid_wins_over_inference() {
        let s = source_from_doc(&json!({
            "_federation": {"indexUid": "movie"},
            "id": "1", "title": "Carrie", "authors": ["nonsense"], "year": 1976,
        }))
        .expect("a titled document is a source");
        assert_eq!(s.item_type, "movie");
        assert_eq!(s.url, "/movies/1");
        assert_eq!(s.year, Some(1976));
    }

    #[test]
    fn an_untitled_document_is_not_a_source() {
        assert!(source_from_doc(&json!({"id": "1"})).is_none());
        assert!(source_from_doc(&json!({"title": "no id"})).is_none());
    }

    #[test]
    fn the_wire_shape_is_the_one_the_command_palette_parses() {
        let s = Source {
            item_type: "movie".into(),
            id: "1".into(),
            title: "Carrie".into(),
            url: "/movies/1".into(),
            cover_url: None,
            year: None,
            description: None,
        };
        let v = serde_json::to_value(&s).unwrap();
        assert_eq!(v["type"], "movie");
        assert!(v.get("cover_url").is_none(), "absent fields stay absent");
        assert!(v.get("year").is_none());
    }
}
