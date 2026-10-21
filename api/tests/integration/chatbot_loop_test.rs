//! The no-regression checkpoint for taking the chat loop off MeiliSearch.
//!
//! The assertions about SSE frames are the client contract:
//! `web/src/lib/stores/chat.svelte.ts` parses exactly these and must not have
//! to change. A failure here is a broken Command Palette, not a broken test.
//!
//! The LLM is mocked. Nothing in this file may depend on the GPU box.

use crate::common::*;
use api::chatbot::llm::ChatMessage;
use api::chatbot::store::{self, Step, TRANSPORT_WEB};
use reqwest::StatusCode;
use serde_json::{Value, json};
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

/// One SSE body in the shape an OpenAI-compatible provider streams.
fn sse(chunks: &[Value]) -> String {
    let mut body = String::new();
    for chunk in chunks {
        body.push_str(&format!("data: {chunk}\n\n"));
    }
    body.push_str("data: [DONE]\n\n");
    body
}

fn content(text: &str) -> Value {
    json!({"choices": [{"index": 0, "delta": {"content": text}}]})
}

async fn mock_llm(body: String) -> MockServer {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .respond_with(ResponseTemplate::new(200).set_body_raw(body, "text/event-stream"))
        .mount(&server)
        .await;
    server
}

async fn point_chat_at(server: &TestServer, llm: &MockServer) {
    server
        .admin_client()
        .patch_json(
            "/api/v1/admin/config",
            &json!({
                "chat_enabled": true,
                "chat_llm_provider": "openai",
                "chat_llm_api_key": "test-key",
                "chat_llm_model": "mock-model",
                "chat_llm_base_url": format!("{}/v1", llm.uri()),
            }),
        )
        .await
        .assert_ok();
}

async fn stream_chat(server: &TestServer, messages: Value) -> String {
    let response = reqwest::Client::new()
        .post(format!("{}/api/v1/admin/chat/completions", server.api_url))
        .bearer_auth(&server.admin_token)
        .json(&json!({"messages": messages, "stream": true}))
        .send()
        .await
        .expect("chat completions must answer");
    assert_eq!(response.status(), StatusCode::OK, "chat should stream");
    response.text().await.expect("a readable SSE body")
}

fn deltas(sse_body: &str) -> String {
    sse_body
        .lines()
        .filter(|l| l.starts_with("data: {"))
        .filter_map(|l| serde_json::from_str::<Value>(&l[6..]).ok())
        .filter_map(|v| {
            v["choices"][0]["delta"]["content"]
                .as_str()
                .map(str::to_string)
        })
        .collect()
}

#[tokio::test]
async fn chat_streams_content_and_terminates_with_done() {
    let llm = mock_llm(sse(&[content("You have "), content("The Stand.")])).await;
    let server = TestServer::start_api_only().await;
    point_chat_at(&server, &llm).await;

    let body = stream_chat(&server, json!([{"role": "user", "content": "any King?"}])).await;

    assert_eq!(deltas(&body), "You have The Stand.");
    assert!(
        body.trim_end().ends_with("data: [DONE]"),
        "the client's reader loop ends on [DONE]: {body}"
    );
}

/// The loop's tool calls belong to the loop. A raw `tool_calls` chunk reaching
/// the browser is the bug class the Meili proxy had to filter out by hand.
#[tokio::test]
async fn raw_tool_calls_are_never_forwarded_to_the_client() {
    let llm = mock_llm(sse(&[
        json!({"choices": [{"index": 0, "delta": {"tool_calls": [{
            "index": 0, "id": "c1", "type": "function",
            "function": {"name": "library_stats", "arguments": "{}"},
        }]}}]}),
    ]))
    .await;
    let server = TestServer::start_api_only().await;
    point_chat_at(&server, &llm).await;

    let body = stream_chat(
        &server,
        json!([{"role": "user", "content": "how many books?"}]),
    )
    .await;

    assert!(!body.contains("tool_calls"), "{body}");
    assert!(!body.contains("library_stats"), "{body}");
    assert!(body.trim_end().ends_with("data: [DONE]"));
}

/// `reasoning_content` is its own delta key on the local model and must never
/// become user-visible text.
#[tokio::test]
async fn reasoning_traces_do_not_leak_into_the_answer() {
    let llm = mock_llm(sse(&[
        json!({"choices": [{"index": 0, "delta": {"reasoning_content": "The user is asking about"}}]}),
        content("Two of them."),
    ]))
    .await;
    let server = TestServer::start_api_only().await;
    point_chat_at(&server, &llm).await;

    let body = stream_chat(&server, json!([{"role": "user", "content": "how many?"}])).await;

    assert_eq!(deltas(&body), "Two of them.");
    assert!(!body.contains("The user is asking about"), "{body}");
}

#[tokio::test]
async fn an_unreachable_provider_still_terminates_the_stream() {
    let server = TestServer::start_api_only().await;
    server
        .admin_client()
        .patch_json(
            "/api/v1/admin/config",
            &json!({
                "chat_enabled": true,
                "chat_llm_provider": "openai",
                "chat_llm_api_key": "test-key",
                "chat_llm_base_url": "http://127.0.0.1:1/v1",
            }),
        )
        .await
        .assert_ok();

    let body = stream_chat(&server, json!([{"role": "user", "content": "hello"}])).await;

    assert!(body.trim_end().ends_with("data: [DONE]"), "{body}");
    assert!(!deltas(&body).is_empty(), "silence reads as broken: {body}");
}

#[tokio::test]
async fn chat_completions_requires_auth() {
    let server = TestServer::start_api_only().await;
    let resp = TestClient::new(&server.api_url)
        .post_json(
            "/api/v1/admin/chat/completions",
            &json!({"messages": [{"role": "user", "content": "hi"}], "stream": true}),
        )
        .await;
    assert_eq!(resp.status, StatusCode::UNAUTHORIZED);
}

/// Every role may ask; this phase's tools are all read-only.
#[tokio::test]
async fn a_viewer_may_use_the_chat() {
    let llm = mock_llm(sse(&[content("Sure.")])).await;
    let server = TestServer::start_api_only().await;
    point_chat_at(&server, &llm).await;

    let viewer = create_user_client(&server, "viewer-chat@example.com", "viewer").await;
    let resp = viewer
        .post_json(
            "/api/v1/admin/chat/completions",
            &json!({"messages": [{"role": "user", "content": "hi"}], "stream": true}),
        )
        .await;
    assert_eq!(resp.status, StatusCode::OK);
}

#[tokio::test]
async fn a_request_with_no_user_message_is_rejected() {
    let llm = mock_llm(sse(&[content("never reached")])).await;
    let server = TestServer::start_api_only().await;
    point_chat_at(&server, &llm).await;

    let resp = server
        .admin_client()
        .post_json(
            "/api/v1/admin/chat/completions",
            &json!({"messages": [], "stream": true}),
        )
        .await;
    assert_eq!(resp.status, StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn chat_is_refused_while_disabled() {
    let server = TestServer::start_api_only().await;
    let resp = server
        .admin_client()
        .post_json(
            "/api/v1/admin/chat/completions",
            &json!({"messages": [{"role": "user", "content": "hi"}], "stream": true}),
        )
        .await;
    assert_eq!(resp.status, StatusCode::BAD_REQUEST);
}

/// A conversation that keeps growing keeps its session; one that restarts at a
/// single user message forks. This is what makes "no, the other one" resolvable
/// without the Command Palette having to hold a session id it never had.
#[tokio::test]
async fn a_continuing_conversation_reuses_its_session_and_a_fresh_one_forks() {
    let server = TestServer::start_api_only().await;
    let user_id = create_user(&server, "session-owner@example.com").await;

    let first = store::open_user_session(&server.pool, TRANSPORT_WEB, user_id, 1)
        .await
        .expect("a first turn opens a session");
    store::record_user_turn(&server.pool, first.id, "find carrie")
        .await
        .unwrap();
    let assistant = store::start_assistant_turn(&server.pool, first.id)
        .await
        .unwrap();
    store::finish_assistant_turn(
        &server.pool,
        assistant,
        "Two matches.",
        &[Step {
            index: 0,
            call_id: "c1".to_string(),
            tool: "search_library".to_string(),
            arguments: r#"{"query":"carrie"}"#.to_string(),
            summary: "searching for carrie".to_string(),
            state: "done".to_string(),
            detail: "2 results".to_string(),
            observation: json!({"results": ["Carrie", "Carrie (2013)"]}),
        }],
        "complete",
    )
    .await
    .unwrap();

    let second = store::open_user_session(&server.pool, TRANSPORT_WEB, user_id, 2)
        .await
        .expect("a second turn continues");
    assert_eq!(
        second.id, first.id,
        "a growing conversation keeps its session"
    );

    let replayed: Vec<ChatMessage> = store::window(&server.pool, second.id, None)
        .await
        .expect("history reads back");
    assert!(
        replayed.iter().any(|m| m.role == "tool"),
        "the newest tool result must survive into the next turn"
    );
    assert!(replayed.iter().any(|m| m.content == "find carrie"));
    assert!(replayed.iter().any(|m| m.content == "Two matches."));

    let restarted = store::open_user_session(&server.pool, TRANSPORT_WEB, user_id, 1)
        .await
        .expect("a cleared chat opens a new session");
    assert_ne!(restarted.id, first.id, "a cleared conversation forks");
    assert!(
        store::window(&server.pool, restarted.id, None)
            .await
            .unwrap()
            .is_empty()
    );
}

/// Google Chat's key. The web keying above is deliberately different because
/// A conversation that goes quiet for longer than the idle window and then resumes.
///
/// `open_addressed_session` closes the stale row and inserts a fresh one on the same
/// (transport, space, thread). The unique index guarding that address did not exclude
/// closed sessions, so the insert collided with the row just closed and the turn died
/// before it started — the sender got "I couldn't get to the library just then", and
/// every later message in that DM hit the same wall, because the closed row kept
/// occupying the slot. It never recovered, which is the opposite of what the apology
/// promises.
#[tokio::test]
async fn a_conversation_resumes_after_it_goes_stale() {
    let server = TestServer::start_api_only().await;
    let user_id = create_user(&server, "stale-session@example.com").await;

    let first = store::open_addressed_session(
        &server.pool,
        "discord",
        "1122334455",
        "1122334455",
        Some(user_id),
    )
    .await
    .expect("first session opens");

    // Idle is 45 minutes; a real gap here is hours.
    sqlx::query("UPDATE chat_sessions SET updated_at = NOW() - INTERVAL '6 hours' WHERE id = $1")
        .bind(first.id)
        .execute(&server.pool)
        .await
        .expect("aged");

    let resumed = store::open_addressed_session(
        &server.pool,
        "discord",
        "1122334455",
        "1122334455",
        Some(user_id),
    )
    .await
    .expect("a stale conversation must be resumable, not fatal");
    assert_ne!(resumed.id, first.id, "the stale session is replaced");

    // And again, because the failure was permanent rather than one-off.
    sqlx::query("UPDATE chat_sessions SET updated_at = NOW() - INTERVAL '6 hours' WHERE id = $1")
        .bind(resumed.id)
        .execute(&server.pool)
        .await
        .expect("aged");
    let third = store::open_addressed_session(
        &server.pool,
        "discord",
        "1122334455",
        "1122334455",
        Some(user_id),
    )
    .await
    .expect("still resumable on the next day too");
    assert_ne!(third.id, resumed.id);
}

/// the browser has no conversation address to key on.
#[tokio::test]
async fn an_addressed_session_is_stable_for_one_space_and_thread() {
    let server = TestServer::start_api_only().await;
    let user_id = create_user(&server, "space-user@example.com").await;

    let first = store::open_addressed_session(
        &server.pool,
        "google_chat",
        "spaces/A",
        "threads/1",
        Some(user_id),
    )
    .await
    .unwrap();
    let again = store::open_addressed_session(
        &server.pool,
        "google_chat",
        "spaces/A",
        "threads/1",
        Some(user_id),
    )
    .await
    .unwrap();
    assert_eq!(first.id, again.id);

    let other = store::open_addressed_session(
        &server.pool,
        "google_chat",
        "spaces/A",
        "threads/2",
        Some(user_id),
    )
    .await
    .unwrap();
    assert_ne!(
        first.id, other.id,
        "a second thread is a second conversation"
    );
}

#[tokio::test]
async fn a_turn_is_persisted_so_a_disconnect_does_not_lose_it() {
    let llm = mock_llm(sse(&[content("Stored.")])).await;
    let server = TestServer::start_api_only().await;
    point_chat_at(&server, &llm).await;

    let body = stream_chat(
        &server,
        json!([{"role": "user", "content": "remember this"}]),
    )
    .await;
    assert!(body.contains("Stored."));

    // The turn owns its own task, so the answer lands a moment after the stream
    // closes — that ordering is the point (a disconnect must not lose the work),
    // so the assertion waits for it rather than racing it.
    let mut rows: Vec<(String, String)> = Vec::new();
    for _ in 0..50 {
        rows = sqlx::query_as("SELECT role, content FROM chat_messages ORDER BY created_at")
            .fetch_all(&server.pool)
            .await
            .expect("the turn is on disk");
        if rows.iter().any(|(r, c)| r == "assistant" && c == "Stored.") {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
    assert!(
        rows.iter()
            .any(|(r, c)| r == "user" && c == "remember this")
    );
    assert!(
        rows.iter().any(|(r, c)| r == "assistant" && c == "Stored."),
        "{rows:?}"
    );
}

/// Every turn leaves a row, success or failure — it is how the resolver gets
/// tuned, so a turn that writes nothing is a turn nobody can debug.
#[tokio::test]
async fn a_turn_writes_its_own_log_row() {
    let llm = mock_llm(sse(&[content("Logged.")])).await;
    let server = TestServer::start_api_only().await;
    point_chat_at(&server, &llm).await;

    stream_chat(&server, json!([{"role": "user", "content": "log this"}])).await;

    let mut rows: Vec<(String, bool, Option<String>)> = Vec::new();
    for _ in 0..50 {
        rows = sqlx::query_as(
            "SELECT transport, succeeded, assistant_text FROM chat_turn_logs ORDER BY id",
        )
        .fetch_all(&server.pool)
        .await
        .expect("chat_turn_logs is readable");
        if !rows.is_empty() {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
    assert_eq!(rows.len(), 1, "{rows:?}");
    assert_eq!(rows[0].0, "web");
    assert!(rows[0].1, "the turn answered");
    assert_eq!(rows[0].2.as_deref(), Some("Logged."));
}

/// A failed turn is the row worth having.
#[tokio::test]
async fn a_failed_turn_is_logged_too() {
    let server = TestServer::start_api_only().await;
    server
        .admin_client()
        .patch_json(
            "/api/v1/admin/config",
            &json!({
                "chat_enabled": true,
                "chat_llm_provider": "openai",
                "chat_llm_api_key": "test-key",
                "chat_llm_base_url": "http://127.0.0.1:1/v1",
            }),
        )
        .await
        .assert_ok();

    stream_chat(
        &server,
        json!([{"role": "user", "content": "this will fail"}]),
    )
    .await;

    let mut rows: Vec<(bool, Option<String>)> = Vec::new();
    for _ in 0..50 {
        rows = sqlx::query_as("SELECT succeeded, turn_error FROM chat_turn_logs")
            .fetch_all(&server.pool)
            .await
            .expect("chat_turn_logs is readable");
        if !rows.is_empty() {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
    assert_eq!(rows.len(), 1, "{rows:?}");
    assert!(!rows[0].0);
    assert!(rows[0].1.is_some(), "the failure needs a reason to show");
}
