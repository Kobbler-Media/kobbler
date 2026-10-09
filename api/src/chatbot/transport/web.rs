//! The Command Palette's view of a turn.
//!
//! This is the no-regression seam: the frames below are byte-for-byte the ones
//! the MeiliSearch proxy used to emit, because `stores/chat.svelte.ts` parses
//! them and must not change. `Delta` becomes an OpenAI content chunk, `Sources`
//! becomes the enriched `event: sources` frame, and the stream ends on
//! `data: [DONE]`.
//!
//! Citations are held back until the end on purpose. The old route could only
//! emit them once it had the whole reply, and it filtered them to the items the
//! model actually named — showing four books when the answer discussed one is
//! the failure that made the filter worth having.

use bytes::Bytes;
use futures::Stream;
use serde_json::{Value, json};

use crate::chatbot::events::{Source, TurnEvent, TurnReceiver};

/// One OpenAI-shaped content chunk, the frame the client reads `delta.content`
/// out of.
pub fn content_frame(text: &str) -> String {
    let payload = json!({
        "choices": [{"index": 0, "delta": {"content": text}, "finish_reason": null}],
    });
    format!("data: {payload}\n\n")
}

pub fn role_frame() -> String {
    let payload = json!({
        "choices": [{"index": 0, "delta": {"role": "assistant", "content": null}, "finish_reason": null}],
    });
    format!("data: {payload}\n\n")
}

pub fn finish_frame() -> String {
    let payload = json!({
        "choices": [{"index": 0, "delta": {}, "finish_reason": "stop"}],
    });
    format!("data: {payload}\n\n")
}

pub const DONE_FRAME: &str = "data: [DONE]\n\n";

/// The enriched citation frame.
///
/// Filtered to items whose title the answer actually mentions; when nothing
/// matches — the model paraphrased everything — the whole set goes out, which
/// is what the Meili-proxy route did and is better than citing nothing.
pub fn sources_frame(sources: &[Source], answer: &str) -> Option<String> {
    if sources.is_empty() {
        return None;
    }
    let lowered = answer.to_lowercase();
    let mentioned: Vec<&Source> = sources
        .iter()
        .filter(|s| lowered.contains(&s.title.to_lowercase()))
        .collect();
    let chosen: Vec<&Source> = match mentioned.is_empty() {
        true => sources.iter().collect(),
        false => mentioned,
    };
    let data = serde_json::to_string(&chosen).ok()?;
    Some(format!("event: sources\ndata: {data}\n\n"))
}

/// Render a turn's events as the SSE stream the Command Palette expects.
pub fn sse_stream(
    mut events: TurnReceiver,
) -> impl Stream<Item = Result<Bytes, std::io::Error>> + Send + 'static {
    async_stream::stream! {
        yield Ok(Bytes::from(role_frame()));

        let mut sources: Vec<Source> = Vec::new();
        let mut streamed = String::new();

        while let Some(event) = events.recv().await {
            match event {
                TurnEvent::Delta(text) => {
                    streamed.push_str(&text);
                    yield Ok(Bytes::from(content_frame(&text)));
                }
                TurnEvent::Sources(mut found) => sources.append(&mut found),
                // Progress belongs to transports that cannot stream. The
                // Command Palette is already showing tokens arrive, and a
                // "searching…" line inside the answer text would be noise.
                TurnEvent::ToolStarted { .. } | TurnEvent::ToolFinished { .. } => {}
                TurnEvent::Done { text } => {
                    // A provider that did not stream still has to render: emit
                    // whatever the turn produced that the client has not seen.
                    if streamed.is_empty() && !text.is_empty() {
                        streamed.push_str(&text);
                        yield Ok(Bytes::from(content_frame(&text)));
                    }
                    if let Some(frame) = sources_frame(&sources, &streamed) {
                        yield Ok(Bytes::from(frame));
                    }
                    yield Ok(Bytes::from(finish_frame()));
                    yield Ok(Bytes::from(DONE_FRAME));
                    return;
                }
                TurnEvent::Failed(reason) => {
                    // Never silence: an error the user can read beats a stream
                    // that simply stops.
                    if streamed.is_empty() {
                        yield Ok(Bytes::from(content_frame(&reason)));
                    }
                    yield Ok(Bytes::from(finish_frame()));
                    yield Ok(Bytes::from(DONE_FRAME));
                    return;
                }
            }
        }

        // The loop dropped its sender without a terminal event. Close the frame
        // set anyway so the client's reader loop ends.
        if let Some(frame) = sources_frame(&sources, &streamed) {
            yield Ok(Bytes::from(frame));
        }
        yield Ok(Bytes::from(DONE_FRAME));
    }
}

/// The user-visible text of the newest user message, and how many user
/// messages the client believes the conversation holds.
pub fn read_request(body: &Value) -> Option<(String, usize)> {
    let messages = body.get("messages")?.as_array()?;
    let user_turns = messages
        .iter()
        .filter(|m| m.get("role").and_then(Value::as_str) == Some("user"))
        .count();
    let last = messages
        .iter()
        .rev()
        .find(|m| m.get("role").and_then(Value::as_str) == Some("user"))?
        .get("content")
        .and_then(Value::as_str)?
        .trim()
        .to_string();
    match last.is_empty() {
        true => None,
        false => Some((last, user_turns)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::chatbot::events;
    use futures::StreamExt as _;

    fn source(title: &str) -> Source {
        Source {
            item_type: "book".to_string(),
            id: title.to_lowercase(),
            title: title.to_string(),
            url: format!("/library/books/{}", title.to_lowercase()),
            cover_url: Some("/cover".to_string()),
            year: None,
            description: Some("Stephen King".to_string()),
        }
    }

    async fn collect(events: Vec<TurnEvent>) -> String {
        let (tx, rx) = events::channel();
        tokio::spawn(async move {
            for e in events {
                let _ = tx.send(e).await;
            }
        });
        sse_stream(rx)
            .filter_map(|r| async { r.ok() })
            .map(|b| String::from_utf8_lossy(&b).to_string())
            .collect::<Vec<_>>()
            .await
            .join("")
    }

    /// The frame contract the Command Palette is built against. Changing any
    /// assertion here is a client change.
    #[tokio::test]
    async fn a_turn_streams_content_then_sources_then_done() {
        let output = collect(vec![
            TurnEvent::Delta("The Stand".to_string()),
            TurnEvent::Sources(vec![source("The Stand"), source("The Shining")]),
            TurnEvent::Delta(" is in the library.".to_string()),
            TurnEvent::Done {
                text: "The Stand is in the library.".to_string(),
            },
        ])
        .await;

        assert!(output.contains("event: sources\n"), "{output}");
        assert!(output.trim_end().ends_with("data: [DONE]"), "{output}");

        let sources_line = output
            .lines()
            .find(|l| l.starts_with("data: ["))
            .expect("a sources data line");
        let items: Vec<Source> = serde_json::from_str(&sources_line[6..]).unwrap();
        assert_eq!(items.len(), 1, "only the title the answer named is cited");
        assert_eq!(items[0].title, "The Stand");
        assert_eq!(items[0].url, "/library/books/the stand");

        let deltas: Vec<String> = output
            .lines()
            .filter(|l| l.starts_with("data: {"))
            .filter_map(|l| serde_json::from_str::<Value>(&l[6..]).ok())
            .filter_map(|v| {
                v["choices"][0]["delta"]["content"]
                    .as_str()
                    .map(str::to_string)
            })
            .collect();
        assert_eq!(deltas.join(""), "The Stand is in the library.");
    }

    /// Tool calls are the loop's business. A raw tool call reaching the browser
    /// was the old route's bug class and must stay impossible.
    #[tokio::test]
    async fn tool_activity_never_reaches_the_client() {
        let output = collect(vec![
            TurnEvent::ToolStarted {
                name: "search_library".to_string(),
                summary: "searching for carrie".to_string(),
            },
            TurnEvent::ToolFinished {
                name: "search_library".to_string(),
                detail: "3 results".to_string(),
                outputs: json!({"results": ["Carrie"]}),
            },
            TurnEvent::Delta("Carrie is here.".to_string()),
            TurnEvent::Done {
                text: "Carrie is here.".to_string(),
            },
        ])
        .await;
        assert!(!output.contains("search_library"), "{output}");
        assert!(!output.contains("tool_calls"), "{output}");
        assert!(output.contains("Carrie is here."));
    }

    #[tokio::test]
    async fn every_source_is_cited_when_the_answer_paraphrased_them_all() {
        let output = collect(vec![
            TurnEvent::Delta("You have a couple of King novels.".to_string()),
            TurnEvent::Sources(vec![source("The Stand"), source("The Shining")]),
            TurnEvent::Done {
                text: "You have a couple of King novels.".to_string(),
            },
        ])
        .await;
        let line = output.lines().find(|l| l.starts_with("data: [")).unwrap();
        let items: Vec<Source> = serde_json::from_str(&line[6..]).unwrap();
        assert_eq!(items.len(), 2);
    }

    #[tokio::test]
    async fn a_failed_turn_still_terminates_the_stream() {
        let output = collect(vec![TurnEvent::Failed(
            "the language model is unreachable".to_string(),
        )])
        .await;
        assert!(output.contains("the language model is unreachable"));
        assert!(output.trim_end().ends_with("data: [DONE]"));
    }

    #[tokio::test]
    async fn a_non_streaming_provider_still_renders_its_answer() {
        let output = collect(vec![TurnEvent::Done {
            text: "All of it arrived at once.".to_string(),
        }])
        .await;
        assert!(output.contains("All of it arrived at once."));
        assert!(output.trim_end().ends_with("data: [DONE]"));
    }

    #[tokio::test]
    async fn a_dropped_loop_still_closes_the_stream() {
        let (tx, rx) = events::channel();
        drop(tx);
        let output: String = sse_stream(rx)
            .filter_map(|r| async { r.ok() })
            .map(|b| String::from_utf8_lossy(&b).to_string())
            .collect::<Vec<_>>()
            .await
            .join("");
        assert!(output.trim_end().ends_with("data: [DONE]"));
    }

    #[test]
    fn the_newest_user_message_and_the_turn_count_come_off_the_request() {
        let body = json!({"messages": [
            {"role": "user", "content": "hi"},
            {"role": "assistant", "content": "hello"},
            {"role": "user", "content": "  what horror do we have?  "},
        ]});
        assert_eq!(
            read_request(&body),
            Some(("what horror do we have?".to_string(), 2))
        );
    }

    #[test]
    fn a_request_with_nothing_to_answer_is_rejected() {
        assert_eq!(read_request(&json!({"messages": []})), None);
        assert_eq!(read_request(&json!({})), None);
        assert_eq!(
            read_request(&json!({"messages": [{"role": "user", "content": "   "}]})),
            None
        );
    }
}
