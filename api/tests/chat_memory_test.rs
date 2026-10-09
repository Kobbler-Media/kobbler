//! Tiers 2 and 3 of the conversation store.
//!
//! Three things are being defended here and they are the ones the design is
//! emphatic about:
//!
//! * **not every session earns a GPU call.** The mock LLM is asserted to have
//!   received *nothing* for a session that just asked what was on;
//! * **a wrong durable fact must be findable and correctable**, so the facts API
//!   is exercised as a person would use it, per user;
//! * **content belongs to the person who said it.** An admin who did not have
//!   the conversation gets counts and no prose, and that is enforced by the
//!   query rather than by the shape of the response.
//!
//! The LLM is mocked. Nothing here depends on the GPU box.

#![allow(clippy::expect_fun_call)]
#![allow(dead_code)]
#![allow(unused_imports)]
// The shared harness is one module; a focused test target compiles a slice of it.
#![allow(clippy::collapsible_if)]

mod common;

use api::chatbot::llm::{LlmConfig, ThinkingLevel};
use api::chatbot::memory;
use api::chatbot::prompt;
use api::chatbot::store::TRANSPORT_WEB;
use api::jobs::handlers::ChatCleanupHandler;
use api::jobs::worker::JobHandler;
use api::repositories::chat_memory;
use api::repositories::chat_profile_facts::{self, MAX_FACTS_PER_USER};
use api::repositories::chat_sessions::{self, NewSession};
use common::*;
use reqwest::StatusCode;
use serde_json::{Value, json};
use tokio_util::sync::CancellationToken;
use uuid::Uuid;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

// =============================================================================
// Harness
// =============================================================================

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

/// A provider that answers every completion with `body`.
async fn mock_llm(body: String) -> MockServer {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .respond_with(ResponseTemplate::new(200).set_body_raw(body, "text/event-stream"))
        .mount(&server)
        .await;
    server
}

/// A provider that answers with one digest, as the summariser expects it.
async fn mock_digest(summary: &str, facts: Value) -> MockServer {
    let digest = json!({"summary": summary, "facts": facts}).to_string();
    mock_llm(sse(&[content(&digest)])).await
}

fn config_for(llm: &MockServer) -> LlmConfig {
    LlmConfig {
        base_url: format!("{}/v1", llm.uri()),
        api_key: None,
        model: "mock-model".to_string(),
        is_koblem: false,
        thinking: ThinkingLevel::Off,
    }
}

async fn calls_to(llm: &MockServer) -> usize {
    llm.received_requests().await.map(|r| r.len()).unwrap_or(0)
}

async fn me(client: &TestClient) -> Uuid {
    let resp = client.get("/api/v1/admin/me").await;
    resp.assert_ok();
    resp.json::<Value>()["user_id"]
        .as_str()
        .unwrap()
        .parse()
        .unwrap()
}

async fn admin_user_id(server: &TestServer) -> Uuid {
    me(&server.admin_client()).await
}

/// A web session with `turns` exchanges. `tools` names the tool calls recorded
/// on the last assistant turn — an empty slice means the session did nothing.
async fn session_with(server: &TestServer, user_id: Uuid, turns: usize, tools: &[&str]) -> Uuid {
    let session = chat_sessions::create_session(
        &server.pool,
        NewSession {
            transport: TRANSPORT_WEB,
            space: None,
            thread: None,
            user_id: Some(user_id),
        },
    )
    .await
    .expect("session")
    .id;

    for n in 0..turns {
        chat_sessions::append_message(
            &server.pool,
            session,
            "user",
            &format!("question {n}"),
            "complete",
        )
        .await
        .expect("user turn");
        let message = chat_sessions::append_message(
            &server.pool,
            session,
            "assistant",
            &format!("answer {n}"),
            "running",
        )
        .await
        .expect("assistant turn");

        let last = n + 1 == turns;
        let steps = match last {
            true => json!(
                tools
                    .iter()
                    .enumerate()
                    .map(|(i, tool)| json!({
                        "index": i,
                        "call_id": format!("c{i}"),
                        "tool": tool,
                        "arguments": "{}",
                        "summary": "",
                        "state": "done",
                        "detail": "",
                        "observation": {"ok": true},
                    }))
                    .collect::<Vec<Value>>()
            ),
            false => json!([]),
        };
        chat_sessions::finish_message(
            &server.pool,
            message,
            &format!("answer {n}"),
            &steps,
            "complete",
        )
        .await
        .expect("finish");
    }
    session
}

async fn go_idle(server: &TestServer, session: Uuid, minutes: i64) {
    sqlx::query(
        "UPDATE chat_sessions SET updated_at = NOW() - make_interval(mins => $2) WHERE id = $1",
    )
    .bind(session)
    .bind(minutes as i32)
    .execute(&server.pool)
    .await
    .unwrap();
}

/// `None` covers both "no summary" and "the session is gone" — the retention
/// sweep produces the second and this is the only caller that sees it.
async fn summary_of(server: &TestServer, session: Uuid) -> Option<String> {
    sqlx::query_scalar::<_, Option<String>>("SELECT summary FROM chat_sessions WHERE id = $1")
        .bind(session)
        .fetch_optional(&server.pool)
        .await
        .unwrap()
        .flatten()
}

async fn closed_at(server: &TestServer, session: Uuid) -> Option<time::OffsetDateTime> {
    sqlx::query_scalar::<_, Option<time::OffsetDateTime>>(
        "SELECT closed_at FROM chat_sessions WHERE id = $1",
    )
    .bind(session)
    .fetch_one(&server.pool)
    .await
    .unwrap()
}

async fn add_fact(server: &TestServer, user_id: Uuid, kind: &str, value: &str) -> Uuid {
    chat_profile_facts::insert(&server.pool, user_id, kind, value, None)
        .await
        .expect("fact insert")
        .expect("a new fact")
        .id
}

// =============================================================================
// Tier 2 — closed on idle, summarised only when it earned it
// =============================================================================

#[tokio::test]
async fn a_session_closes_once_it_has_been_idle_long_enough() {
    let llm = mock_digest("They asked about season 24 and it was queued.", json!([])).await;
    let server = TestServer::start_api_only().await;
    let user_id = admin_user_id(&server).await;

    let stale = session_with(&server, user_id, 1, &["search_library"]).await;
    let fresh = session_with(&server, user_id, 1, &["search_library"]).await;
    go_idle(&server, stale, 46).await;
    go_idle(&server, fresh, 44).await;

    let report = memory::sweep_idle_sessions(&server.pool, Some(&config_for(&llm)), 45)
        .await
        .expect("sweep");

    assert_eq!(report.closed, 1, "only the stale session should close");
    assert!(closed_at(&server, stale).await.is_some());
    assert!(
        closed_at(&server, fresh).await.is_none(),
        "a conversation 44 minutes quiet is still live"
    );
}

/// The design's own example. "what's on tonight" is not worth a GPU call, and a
/// context line saying *she asked what was on* is worse than no line.
#[tokio::test]
async fn a_short_session_with_no_tool_call_closes_without_asking_the_model() {
    let llm = mock_digest("should never be written", json!([])).await;
    let server = TestServer::start_api_only().await;
    let user_id = admin_user_id(&server).await;

    let session = session_with(&server, user_id, 1, &[]).await;
    go_idle(&server, session, 60).await;

    let report = memory::sweep_idle_sessions(&server.pool, Some(&config_for(&llm)), 45)
        .await
        .expect("sweep");

    assert_eq!(report.closed, 1);
    assert_eq!(report.summarised, 0);
    assert_eq!(
        calls_to(&llm).await,
        0,
        "a one-shot question must not reach the language model"
    );
    assert!(closed_at(&server, session).await.is_some());
    assert_eq!(summary_of(&server, session).await, None);
}

#[tokio::test]
async fn a_session_that_used_a_tool_is_summarised_and_yields_facts() {
    let llm = mock_digest(
        "They asked for season 24 of someShow; it was queued and had not finished when they stopped.",
        json!([
            {"kind": "likes", "value": "70s horror"},
            {"kind": "avoids", "value": "anything over 2h on a weeknight"},
        ]),
    )
    .await;
    let server = TestServer::start_api_only().await;
    let user_id = admin_user_id(&server).await;

    let session = session_with(&server, user_id, 1, &["want_it"]).await;
    go_idle(&server, session, 60).await;

    let report = memory::sweep_idle_sessions(&server.pool, Some(&config_for(&llm)), 45)
        .await
        .expect("sweep");

    assert_eq!(report.summarised, 1);
    assert_eq!(report.facts_added, 2);
    assert_eq!(calls_to(&llm).await, 1, "one call, not one per tier");

    let summary = summary_of(&server, session).await.expect("a summary");
    assert!(summary.contains("had not finished"), "{summary}");

    let facts = chat_profile_facts::list_for_user(&server.pool, user_id)
        .await
        .unwrap();
    let mut values: Vec<&str> = facts.iter().map(|f| f.value.as_str()).collect();
    values.sort_unstable();
    assert_eq!(
        values,
        vec!["70s horror", "anything over 2h on a weeknight"]
    );
    assert_eq!(
        facts
            .iter()
            .filter(|f| f.source_id == Some(session))
            .count(),
        2
    );
}

/// A conversation that ran on is worth compressing even with no tool call.
#[tokio::test]
async fn a_long_conversation_is_summarised_without_a_tool_call() {
    let llm = mock_digest(
        "They talked through what to watch and settled on nothing.",
        json!([]),
    )
    .await;
    let server = TestServer::start_api_only().await;
    let user_id = admin_user_id(&server).await;

    let session = session_with(&server, user_id, 5, &[]).await;
    go_idle(&server, session, 60).await;

    let report = memory::sweep_idle_sessions(&server.pool, Some(&config_for(&llm)), 45)
        .await
        .expect("sweep");

    assert_eq!(report.summarised, 1);
    assert!(summary_of(&server, session).await.is_some());
}

/// No provider configured is not a reason to leave sessions open forever.
#[tokio::test]
async fn with_no_language_model_configured_sessions_still_close() {
    let server = TestServer::start_api_only().await;
    let user_id = admin_user_id(&server).await;
    let session = session_with(&server, user_id, 3, &["search_library"]).await;
    go_idle(&server, session, 60).await;

    let report = memory::sweep_idle_sessions(&server.pool, None, 45)
        .await
        .expect("sweep");

    assert_eq!(report.closed, 1);
    assert_eq!(report.summarised, 0);
    assert!(closed_at(&server, session).await.is_some());
}

// =============================================================================
// Tier 3 — the facts, and the prompt they ride in
// =============================================================================

#[tokio::test]
async fn the_profile_is_capped_and_the_oldest_goes_first() {
    let server = TestServer::start_api_only().await;
    let user_id = admin_user_id(&server).await;
    let session = session_with(&server, user_id, 1, &[]).await;

    for n in 0..MAX_FACTS_PER_USER {
        add_fact(&server, user_id, "likes", &format!("thing {n}")).await;
    }
    // Backdate so "oldest" is unambiguous rather than a clock race.
    sqlx::query("UPDATE chat_profile_facts SET created_at = NOW() - INTERVAL '1 day' WHERE user_id = $1 AND value = 'thing 0'")
        .bind(user_id)
        .execute(&server.pool)
        .await
        .unwrap();

    memory::store_facts(
        &server.pool,
        user_id,
        session,
        &[api::chatbot::memory::NewFact {
            kind: "prefers".to_string(),
            value: "subtitles over dubs".to_string(),
        }],
    )
    .await
    .expect("store");

    let facts = chat_profile_facts::list_for_user(&server.pool, user_id)
        .await
        .unwrap();
    assert_eq!(facts.len() as i64, MAX_FACTS_PER_USER, "the cap held");
    let values: Vec<&str> = facts.iter().map(|f| f.value.as_str()).collect();
    assert!(values.contains(&"subtitles over dubs"));
    assert!(
        !values.contains(&"thing 0"),
        "the oldest fact should have been evicted"
    );
}

#[tokio::test]
async fn the_same_fact_twice_is_one_fact() {
    let server = TestServer::start_api_only().await;
    let user_id = admin_user_id(&server).await;

    add_fact(&server, user_id, "likes", "70s horror").await;
    let again = chat_profile_facts::insert(&server.pool, user_id, "likes", "70s Horror", None)
        .await
        .expect("insert");

    assert!(again.is_none(), "a re-extraction must not duplicate");
    assert_eq!(
        chat_profile_facts::count_for_user(&server.pool, user_id)
            .await
            .unwrap(),
        1
    );
}

#[tokio::test]
async fn facts_reach_the_prompt_for_their_owner_and_nobody_else() {
    let server = TestServer::start_api_only().await;
    let owner_id = admin_user_id(&server).await;
    let other = create_user_client(&server, "other@test.com", "contributor").await;
    let other_id = me(&other).await;

    add_fact(&server, owner_id, "likes", "70s horror").await;
    add_fact(&server, owner_id, "prefers", "subtitles over dubs").await;

    let owner_session = session_with(&server, owner_id, 1, &[]).await;
    let other_session = session_with(&server, other_id, 1, &[]).await;

    let mine = prompt::memory_prompt(&server.pool, owner_id, owner_session)
        .await
        .expect("a memory line");
    assert!(mine.contains("70s horror"), "{mine}");
    assert!(mine.contains("prefers: subtitles over dubs"), "{mine}");

    let theirs = prompt::memory_prompt(&server.pool, other_id, other_session).await;
    assert!(
        theirs.is_none(),
        "another user's prompt must not carry these facts: {theirs:?}"
    );
}

#[tokio::test]
async fn the_last_summary_in_this_conversation_is_carried_forward() {
    let server = TestServer::start_api_only().await;
    let user_id = admin_user_id(&server).await;

    let yesterday = session_with(&server, user_id, 1, &["want_it"]).await;
    chat_memory::close_with_summary(
        &server.pool,
        yesterday,
        Some("They queued season 24; it had not finished."),
    )
    .await
    .unwrap();

    let today = session_with(&server, user_id, 1, &[]).await;
    let line = prompt::memory_prompt(&server.pool, user_id, today)
        .await
        .expect("a memory line");
    assert!(line.contains("had not finished"), "{line}");

    let stranger = create_user_client(&server, "stranger@test.com", "contributor").await;
    let stranger_id = me(&stranger).await;
    let theirs = session_with(&server, stranger_id, 1, &[]).await;
    assert!(
        prompt::memory_prompt(&server.pool, stranger_id, theirs)
            .await
            .is_none(),
        "a summary must not carry across to a different person"
    );
}

// =============================================================================
// The facts API — see it, correct it, remove it
// =============================================================================

#[tokio::test]
async fn a_person_can_list_add_correct_and_remove_their_own_facts() {
    let server = TestServer::start_api_only().await;
    let client = server.admin_client();

    let created: Value = {
        let resp = client
            .post_json(
                "/api/v1/admin/chat/memory/facts",
                &json!({"kind": "likes", "value": "70s horror"}),
            )
            .await;
        resp.assert_status(StatusCode::CREATED);
        resp.json()
    };
    let fact_id = created["id"].as_str().unwrap().to_string();

    let listed: Value = {
        let resp = client.get("/api/v1/admin/chat/memory").await;
        resp.assert_ok();
        resp.json()
    };
    assert_eq!(listed["facts"][0]["value"], "70s horror");
    assert_eq!(listed["max_facts"], MAX_FACTS_PER_USER);

    let corrected: Value = {
        let resp = client
            .patch_json(
                &format!("/api/v1/admin/chat/memory/facts/{fact_id}"),
                &json!({"kind": "avoids", "value": "70s horror"}),
            )
            .await;
        resp.assert_ok();
        resp.json()
    };
    assert_eq!(corrected["kind"], "avoids");

    client
        .delete(&format!("/api/v1/admin/chat/memory/facts/{fact_id}"))
        .await
        .assert_status(StatusCode::NO_CONTENT);

    let after: Value = {
        let resp = client.get("/api/v1/admin/chat/memory").await;
        resp.assert_ok();
        resp.json()
    };
    assert_eq!(after["facts"].as_array().unwrap().len(), 0);
}

#[tokio::test]
async fn a_nonsense_fact_is_rejected() {
    let server = TestServer::start_api_only().await;
    let client = server.admin_client();

    for body in [
        json!({"kind": "hates", "value": "musicals"}),
        json!({"kind": "likes", "value": "   "}),
        json!({"kind": "likes", "value": "x".repeat(201)}),
    ] {
        client
            .post_json("/api/v1/admin/chat/memory/facts", &body)
            .await
            .assert_status(StatusCode::BAD_REQUEST);
    }
}

#[tokio::test]
async fn one_person_cannot_touch_another_persons_facts() {
    let server = TestServer::start_api_only().await;
    let owner_id = admin_user_id(&server).await;
    let fact_id = add_fact(&server, owner_id, "likes", "70s horror").await;

    let other_admin = create_user_client(&server, "admin2@test.com", "admin").await;

    let listed: Value = {
        let resp = other_admin.get("/api/v1/admin/chat/memory").await;
        resp.assert_ok();
        resp.json()
    };
    assert_eq!(listed["facts"].as_array().unwrap().len(), 0);
    assert!(!listed.to_string().contains("70s horror"));

    other_admin
        .patch_json(
            &format!("/api/v1/admin/chat/memory/facts/{fact_id}"),
            &json!({"kind": "avoids", "value": "hijacked"}),
        )
        .await
        .assert_status(StatusCode::NOT_FOUND);
    other_admin
        .delete(&format!("/api/v1/admin/chat/memory/facts/{fact_id}"))
        .await
        .assert_status(StatusCode::NOT_FOUND);

    let facts = chat_profile_facts::list_for_user(&server.pool, owner_id)
        .await
        .unwrap();
    assert_eq!(facts.len(), 1);
    assert_eq!(facts[0].value, "70s horror");
}

#[tokio::test]
async fn chat_memory_requires_auth() {
    let server = TestServer::start_api_only().await;
    let anonymous = TestClient::new(&server.api_url);
    anonymous
        .get("/api/v1/admin/chat/memory")
        .await
        .assert_status(StatusCode::UNAUTHORIZED);
}

// =============================================================================
// Privacy — content belongs to the person who said it
// =============================================================================

const CONTENT_FIELDS: [&str; 2] = ["facts", "summaries"];

fn assert_no_content(row: &Value) {
    assert!(
        row.get("content").is_none(),
        "content must be absent for a non-owner: {row}"
    );
    let serialised = row.to_string();
    for field in CONTENT_FIELDS {
        assert!(
            !serialised.contains(field),
            "{field} leaked into a non-owner row: {serialised}"
        );
    }
}

#[tokio::test]
async fn an_admin_sees_metadata_but_never_another_persons_facts_or_summaries() {
    let server = TestServer::start_api_only().await;
    let other_admin = create_user_client(&server, "admin2@test.com", "admin").await;
    let other_id = me(&other_admin).await;

    add_fact(&server, other_id, "likes", "70s horror").await;
    let session = session_with(&server, other_id, 2, &["want_it"]).await;
    chat_memory::close_with_summary(
        &server.pool,
        session,
        Some("They queued season 24 of someShow."),
    )
    .await
    .unwrap();

    let rows: Value = {
        let resp = server
            .admin_client()
            .get("/api/v1/admin/chat/memory/users")
            .await;
        resp.assert_ok();
        resp.json()
    };
    let body = rows.to_string();
    assert!(
        !body.contains("70s horror"),
        "a fact value leaked to another admin: {body}"
    );
    assert!(
        !body.contains("queued season 24"),
        "a session summary leaked to another admin: {body}"
    );

    let theirs = rows
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["user_id"] == other_id.to_string())
        .expect("their row");
    assert_no_content(theirs);
    // The metadata an admin is meant to have survives the split.
    assert_eq!(theirs["fact_count"], 1);
    assert_eq!(theirs["summary_count"], 1);
    assert_eq!(theirs["message_count"], 4);
    assert!(theirs["last_seen"].is_string());

    // Their own row still carries their own content.
    let mine: Value = {
        let resp = other_admin.get("/api/v1/admin/chat/memory/users").await;
        resp.assert_ok();
        resp.json()
    };
    let own = mine
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["user_id"] == other_id.to_string())
        .expect("own row");
    assert_eq!(own["content"]["facts"][0]["value"], "70s horror");
    assert_eq!(
        own["content"]["summaries"][0]["summary"],
        "They queued season 24 of someShow."
    );
}

#[tokio::test]
async fn the_admin_metadata_view_is_admin_only() {
    let server = TestServer::start_api_only().await;
    let viewer = create_user_client(&server, "viewer@test.com", "viewer").await;
    viewer
        .get("/api/v1/admin/chat/memory/users")
        .await
        .assert_status(StatusCode::FORBIDDEN);
    // Their own memory is theirs regardless of role.
    viewer.get("/api/v1/admin/chat/memory").await.assert_ok();
}

// =============================================================================
// Retention
// =============================================================================

#[tokio::test]
async fn retention_drops_raw_turns_at_thirty_days_and_summaries_at_a_year() {
    let server = TestServer::start_api_only().await;
    let user_id = admin_user_id(&server).await;

    let old = session_with(&server, user_id, 1, &[]).await;
    let recent = session_with(&server, user_id, 1, &[]).await;
    let ancient = session_with(&server, user_id, 1, &[]).await;

    chat_memory::close_with_summary(&server.pool, old, Some("an old conversation"))
        .await
        .unwrap();
    chat_memory::close_with_summary(&server.pool, ancient, Some("an ancient conversation"))
        .await
        .unwrap();

    // Hours rather than days: two statements evaluate NOW() microseconds apart,
    // so a row aged exactly to the boundary lands on the wrong side of it.
    let age = |session: Uuid, hours: i32| {
        let pool = server.pool.clone();
        async move {
            sqlx::query("UPDATE chat_messages SET created_at = NOW() - make_interval(hours => $2) WHERE session_id = $1")
                .bind(session)
                .bind(hours)
                .execute(&pool)
                .await
                .unwrap();
            sqlx::query("UPDATE chat_sessions SET updated_at = NOW() - make_interval(hours => $2), closed_at = NOW() - make_interval(hours => $2) WHERE id = $1")
                .bind(session)
                .bind(hours)
                .execute(&pool)
                .await
                .unwrap();
        }
    };
    age(old, 31 * 24).await;
    age(recent, 30 * 24 - 1).await;
    age(ancient, 366 * 24).await;

    add_fact(&server, user_id, "likes", "70s horror").await;

    let handler =
        ChatCleanupHandler::new(server.pool.clone(), server.app_state.runtime_config.clone());
    let result = handler
        .execute(
            Uuid::now_v7(),
            json!({"mode": "retention"}),
            CancellationToken::new(),
        )
        .await
        .expect("retention job");

    assert_eq!(result["messages_deleted"], 4, "31- and 366-day turns go");
    assert_eq!(
        result["sessions_deleted"], 1,
        "only the year-old session goes"
    );

    let remaining_messages: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM chat_messages WHERE session_id = $1")
            .bind(recent)
            .fetch_one(&server.pool)
            .await
            .unwrap();
    assert_eq!(
        remaining_messages, 2,
        "a 30-day-old turn is still inside the window"
    );

    // The summary outlives the turns it replaced.
    assert_eq!(
        summary_of(&server, old).await.as_deref(),
        Some("an old conversation")
    );
    assert!(
        summary_of(&server, ancient).await.is_none(),
        "the year-old session should be gone entirely"
    );

    assert_eq!(
        chat_profile_facts::count_for_user(&server.pool, user_id)
            .await
            .unwrap(),
        1,
        "profile facts are kept until their owner removes them"
    );
}

// =============================================================================
// Forget everything
// =============================================================================

#[tokio::test]
async fn forget_everything_clears_all_three_tiers_and_only_for_that_person() {
    let server = TestServer::start_api_only().await;
    let mine = create_user_client(&server, "mine@test.com", "contributor").await;
    let mine_id = me(&mine).await;
    let theirs_id = admin_user_id(&server).await;

    for user in [mine_id, theirs_id] {
        let session = session_with(&server, user, 2, &["search_library"]).await;
        chat_memory::close_with_summary(&server.pool, session, Some("a summarised conversation"))
            .await
            .unwrap();
        add_fact(&server, user, "likes", "70s horror").await;
    }

    let result: Value = {
        let resp = mine.delete("/api/v1/admin/chat/memory").await;
        resp.assert_ok();
        resp.json()
    };
    assert_eq!(result["sessions"], 1);
    assert_eq!(result["messages"], 4);
    assert_eq!(result["facts"], 1);

    let after: Value = {
        let resp = mine.get("/api/v1/admin/chat/memory").await;
        resp.assert_ok();
        resp.json()
    };
    assert_eq!(after["facts"].as_array().unwrap().len(), 0);
    assert_eq!(after["summaries"].as_array().unwrap().len(), 0);

    let sessions: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM chat_sessions WHERE user_id = $1")
        .bind(mine_id)
        .fetch_one(&server.pool)
        .await
        .unwrap();
    assert_eq!(sessions, 0);

    // Nobody else lost anything.
    assert_eq!(
        chat_profile_facts::count_for_user(&server.pool, theirs_id)
            .await
            .unwrap(),
        1
    );
    let theirs_sessions: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM chat_sessions WHERE user_id = $1")
            .bind(theirs_id)
            .fetch_one(&server.pool)
            .await
            .unwrap();
    assert_eq!(theirs_sessions, 1);
    let theirs_messages: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM chat_messages m JOIN chat_sessions s ON s.id = m.session_id WHERE s.user_id = $1",
    )
    .bind(theirs_id)
    .fetch_one(&server.pool)
    .await
    .unwrap();
    assert_eq!(theirs_messages, 4);
}
