//! Chat turn log recording, the admin viewer and its privacy split.

use crate::common::*;
use api::chatbot::logs::{TRANSPORT_GOOGLE_CHAT, TRANSPORT_WEB, TurnLog};
use api::repositories::chat_turn_logs;
use reqwest::StatusCode;
use serde_json::{Value, json};
use std::time::Instant;
use uuid::Uuid;

// =============================================================================
// Helpers
// =============================================================================

async fn admin_user_id(server: &TestServer) -> Uuid {
    let resp = server.admin_client().get("/api/v1/admin/me").await;
    resp.assert_ok();
    resp.json::<Value>()["user_id"]
        .as_str()
        .unwrap()
        .parse()
        .unwrap()
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

type Outcome = Result<Value, String>;

/// A successful turn: extract + resolve + one tool call, then an answer.
async fn log_successful_turn(server: &TestServer, user_id: Uuid, message: &str) -> Uuid {
    let mut turn = TurnLog::open(TRANSPORT_WEB, None, Some(user_id), message);
    let id = turn.id();
    let extracted: Outcome = Ok(json!({ "title": "someShow", "season": 24, "episode": null }));
    let candidates: Result<Vec<Value>, String> = Ok(vec![json!({ "ref": "episode:1" })]);
    let tool_out: Outcome = Ok(json!({ "queued": true }));
    turn.llm("koblem", "gemma-4-12b-it")
        .llm_round(std::time::Duration::from_millis(12), Some(400), Some(30))
        .extract(Instant::now(), extracted.as_ref())
        .resolve(
            Instant::now(),
            "someShow season 24",
            Some("episode:1"),
            candidates.as_deref(),
        )
        .action(
            Instant::now(),
            "want_it",
            &json!({ "ref": "episode:1" }),
            tool_out.as_ref(),
        );
    turn.finish(&server.pool, "Grabbing S24E01.").await;
    id
}

/// A turn that failed at one stage. `stage` is extract | resolve | action.
async fn log_failed_turn(server: &TestServer, user_id: Uuid, stage: &str) -> Uuid {
    let mut turn = TurnLog::open(
        TRANSPORT_GOOGLE_CHAT,
        None,
        Some(user_id),
        "get me the new one",
    );
    let id = turn.id();
    let boom: Outcome = Err("boom".to_string());
    let ok: Outcome = Ok(json!({}));
    let candidates: Result<Vec<Value>, String> = Ok(vec![]);
    match stage {
        "extract" => {
            turn.extract(Instant::now(), boom.as_ref());
        }
        "resolve" => {
            turn.extract(Instant::now(), ok.as_ref());
            let failed: Result<Vec<Value>, String> = Err("boom".to_string());
            turn.resolve(Instant::now(), "the new one", None, failed.as_deref());
        }
        "action" => {
            turn.extract(Instant::now(), ok.as_ref());
            turn.resolve(
                Instant::now(),
                "the new one",
                Some("movie:1"),
                candidates.as_deref(),
            );
            turn.action(Instant::now(), "want_it", &json!({}), boom.as_ref());
        }
        other => panic!("unknown stage {other}"),
    }
    turn.fail(&server.pool, &"turn failed").await;
    id
}

fn find(logs: &[Value], id: Uuid) -> &Value {
    logs.iter()
        .find(|l| l["id"].as_str() == Some(&id.to_string()))
        .unwrap_or_else(|| panic!("turn {id} not in response"))
}

// =============================================================================
// Recording
// =============================================================================

#[tokio::test]
async fn test_successful_turn_is_recorded() {
    let server = TestServer::start_api_only().await;
    let user_id = admin_user_id(&server).await;
    let id = log_successful_turn(&server, user_id, "download season 24 of someShow").await;

    let resp = server
        .admin_client()
        .get(&format!("/api/v1/admin/chat/logs/{id}"))
        .await;
    resp.assert_ok();
    let log: Value = resp.json();

    assert_eq!(log["succeeded"], true);
    assert!(log["failed_stage"].is_null());
    assert_eq!(log["transport"], "web");
    assert_eq!(log["llm_model"], "gemma-4-12b-it");
    assert_eq!(log["rounds"], 1);
    assert_eq!(log["prompt_tokens"], 400);
    assert_eq!(log["resolve_candidate_count"], 1);
    assert_eq!(
        log["extract_shape"],
        json!({ "title": "string", "season": "integer", "episode": null })
    );
    assert_eq!(log["action_name"], "want_it");
    assert_eq!(log["tool_call_count"], 1);
    assert_eq!(log["tool_names"], json!(["want_it"]));
    assert_eq!(
        log["content"]["user_message"],
        "download season 24 of someShow"
    );
    assert_eq!(log["content"]["assistant_text"], "Grabbing S24E01.");
}

/// Each stage failure lands in its own column and leaves the other two null,
/// and the row is written even though the turn never answered.
#[tokio::test]
async fn test_failed_turn_populates_only_its_own_error_column() {
    let server = TestServer::start_api_only().await;
    let user_id = admin_user_id(&server).await;

    for stage in ["extract", "resolve", "action"] {
        let id = log_failed_turn(&server, user_id, stage).await;
        let resp = server
            .admin_client()
            .get(&format!("/api/v1/admin/chat/logs/{id}"))
            .await;
        resp.assert_ok();
        let log: Value = resp.json();

        assert_eq!(log["succeeded"], false, "{stage}");
        assert_eq!(log["failed_stage"], stage, "{stage}");
        assert_eq!(log["turn_error"], "turn failed", "{stage}");
        for other in ["extract", "resolve", "action"] {
            let column = format!("{other}_error");
            if other == stage {
                assert_eq!(log[&column], "boom", "{stage}/{column}");
            } else {
                assert!(log[&column].is_null(), "{stage}/{column} should be null");
            }
        }
    }
}

// =============================================================================
// List endpoint
// =============================================================================

#[tokio::test]
async fn test_list_returns_turns_newest_first() {
    let server = TestServer::start_api_only().await;
    let user_id = admin_user_id(&server).await;
    let first = log_successful_turn(&server, user_id, "first").await;
    let second = log_successful_turn(&server, user_id, "second").await;

    let resp = server.admin_client().get("/api/v1/admin/chat/logs").await;
    resp.assert_ok();
    let body: Value = resp.json();
    let logs = body["logs"].as_array().unwrap();

    assert_eq!(body["total"], 2);
    assert_eq!(logs[0]["id"], second.to_string());
    assert_eq!(logs[1]["id"], first.to_string());
}

#[tokio::test]
async fn test_list_filters() {
    let server = TestServer::start_api_only().await;
    let user_id = admin_user_id(&server).await;
    let other = create_user_with_role(&server, "other@test.com", "contributor").await;

    let ok_id = log_successful_turn(&server, user_id, "download it").await;
    let extract_id = log_failed_turn(&server, user_id, "extract").await;
    let resolve_id = log_failed_turn(&server, user_id, "resolve").await;
    let other_id = log_successful_turn(&server, other, "someone else's turn").await;

    let cases: Vec<(&str, Vec<Uuid>)> = vec![
        ("?status=ok", vec![ok_id, other_id]),
        ("?status=failed", vec![extract_id, resolve_id]),
        ("?stage=extract", vec![extract_id]),
        ("?stage=resolve", vec![resolve_id]),
        ("?stage=action", vec![]),
        ("?transport=web", vec![ok_id, other_id]),
        ("?transport=google_chat", vec![extract_id, resolve_id]),
        ("?q=download", vec![ok_id]),
        ("?q=someone", vec![]), // free text only matches the caller's own turns
    ];

    for (query, expected) in cases {
        let resp = server
            .admin_client()
            .get(&format!("/api/v1/admin/chat/logs{query}"))
            .await;
        resp.assert_ok();
        let body: Value = resp.json();
        let mut got: Vec<String> = body["logs"]
            .as_array()
            .unwrap()
            .iter()
            .map(|l| l["id"].as_str().unwrap().to_string())
            .collect();
        got.sort();
        let mut want: Vec<String> = expected.iter().map(|u| u.to_string()).collect();
        want.sort();
        assert_eq!(got, want, "filter {query}");
        assert_eq!(body["total"], want.len(), "total for {query}");
    }

    let resp = server
        .admin_client()
        .get(&format!("/api/v1/admin/chat/logs?user_id={other}"))
        .await;
    resp.assert_ok();
    let body: Value = resp.json();
    assert_eq!(body["total"], 1);
    assert_eq!(body["logs"][0]["id"], other_id.to_string());
}

#[tokio::test]
async fn test_list_pagination() {
    let server = TestServer::start_api_only().await;
    let user_id = admin_user_id(&server).await;
    let mut ids = Vec::new();
    for i in 0..5 {
        ids.push(log_successful_turn(&server, user_id, &format!("turn {i}")).await);
    }
    ids.reverse();

    let resp = server
        .admin_client()
        .get("/api/v1/admin/chat/logs?limit=2&offset=2")
        .await;
    resp.assert_ok();
    let body: Value = resp.json();
    assert_eq!(body["total"], 5);
    let logs = body["logs"].as_array().unwrap();
    assert_eq!(logs.len(), 2);
    assert_eq!(logs[0]["id"], ids[2].to_string());
    assert_eq!(logs[1]["id"], ids[3].to_string());
}

#[tokio::test]
async fn test_bad_filter_values_are_rejected() {
    let server = TestServer::start_api_only().await;
    for query in ["?stage=nonsense", "?status=maybe", "?since=yesterday"] {
        let resp = server
            .admin_client()
            .get(&format!("/api/v1/admin/chat/logs{query}"))
            .await;
        resp.assert_status(StatusCode::BAD_REQUEST);
    }
}

#[tokio::test]
async fn test_detail_404_for_unknown_id() {
    let server = TestServer::start_api_only().await;
    let resp = server
        .admin_client()
        .get(&format!("/api/v1/admin/chat/logs/{}", Uuid::now_v7()))
        .await;
    resp.assert_status(StatusCode::NOT_FOUND);
}

// =============================================================================
// Auth
// =============================================================================

#[tokio::test]
async fn test_unauthenticated_is_401() {
    let server = TestServer::start_api_only().await;
    let anon = TestClient::new(&server.api_url);

    anon.get("/api/v1/admin/chat/logs")
        .await
        .assert_status(StatusCode::UNAUTHORIZED);
    anon.get(&format!("/api/v1/admin/chat/logs/{}", Uuid::now_v7()))
        .await
        .assert_status(StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn test_non_admin_is_403() {
    let server = TestServer::start_api_only().await;
    let admin_id = admin_user_id(&server).await;
    let id = log_successful_turn(&server, admin_id, "download it").await;

    for role in ["viewer", "contributor"] {
        let client = create_user_client(&server, &format!("{role}@test.com"), role).await;
        client
            .get("/api/v1/admin/chat/logs")
            .await
            .assert_status(StatusCode::FORBIDDEN);
        client
            .get(&format!("/api/v1/admin/chat/logs/{id}"))
            .await
            .assert_status(StatusCode::FORBIDDEN);
    }
}

// =============================================================================
// Privacy split: content belongs to the person who had the conversation
// =============================================================================

const CONTENT_FIELDS: [&str; 9] = [
    "user_message",
    "assistant_text",
    "extract_output",
    "resolve_query",
    "resolved_ref",
    "resolve_candidates",
    "action_args",
    "action_result",
    "tool_calls",
];

fn assert_no_content(log: &Value) {
    assert!(
        log.get("content").is_none(),
        "content must be absent for a non-owner, got {log}"
    );
    let serialised = log.to_string();
    for field in CONTENT_FIELDS {
        assert!(
            !serialised.contains(field),
            "{field} leaked into a non-owner response: {serialised}"
        );
    }
}

#[tokio::test]
async fn test_owner_sees_content_other_admin_sees_metadata_only() {
    let server = TestServer::start_api_only().await;
    let other_admin = create_user_client(&server, "admin2@test.com", "admin").await;
    let other_admin_id = me(&other_admin).await;

    let id = log_successful_turn(&server, other_admin_id, "get me the new Sanderson").await;

    let owner_view: Value = {
        let resp = other_admin
            .get(&format!("/api/v1/admin/chat/logs/{id}"))
            .await;
        resp.assert_ok();
        resp.json()
    };
    assert_eq!(
        owner_view["content"]["user_message"],
        "get me the new Sanderson"
    );
    assert_eq!(owner_view["content"]["tool_calls"][0]["name"], "want_it");

    let stranger_view: Value = {
        let resp = server
            .admin_client()
            .get(&format!("/api/v1/admin/chat/logs/{id}"))
            .await;
        resp.assert_ok();
        resp.json()
    };
    assert_no_content(&stranger_view);

    // Metadata the resolver-tuning use case depends on survives the split.
    assert_eq!(stranger_view["transport"], "web");
    assert_eq!(stranger_view["succeeded"], true);
    assert_eq!(stranger_view["action_name"], "want_it");
    assert_eq!(stranger_view["tool_names"], json!(["want_it"]));
    assert_eq!(stranger_view["resolve_candidate_count"], 1);
    assert_eq!(
        stranger_view["extract_shape"],
        json!({ "title": "string", "season": "integer", "episode": null }),
        "the extraction shape is metadata and must survive the split"
    );
    assert_eq!(stranger_view["llm_model"], "gemma-4-12b-it");
    assert_eq!(stranger_view["user_id"], other_admin_id.to_string());
}

#[tokio::test]
async fn test_list_applies_the_same_split() {
    let server = TestServer::start_api_only().await;
    let admin_id = admin_user_id(&server).await;
    let other_admin = create_user_client(&server, "admin2@test.com", "admin").await;
    let other_admin_id = me(&other_admin).await;

    let mine = log_successful_turn(&server, admin_id, "my own turn").await;
    let theirs = log_successful_turn(&server, other_admin_id, "their private turn").await;

    let resp = server.admin_client().get("/api/v1/admin/chat/logs").await;
    resp.assert_ok();
    let body: Value = resp.json();
    let logs = body["logs"].as_array().unwrap();

    assert_eq!(find(logs, mine)["content"]["user_message"], "my own turn");
    assert_no_content(find(logs, theirs));
    assert!(
        !body.to_string().contains("their private turn"),
        "another user's message leaked into the list response"
    );
}

#[tokio::test]
async fn test_stage_errors_are_visible_to_a_non_owner() {
    let server = TestServer::start_api_only().await;
    let other_admin = create_user_client(&server, "admin2@test.com", "admin").await;
    let other_admin_id = me(&other_admin).await;
    let id = log_failed_turn(&server, other_admin_id, "resolve").await;

    let resp = server
        .admin_client()
        .get(&format!("/api/v1/admin/chat/logs/{id}"))
        .await;
    resp.assert_ok();
    let log: Value = resp.json();

    assert_no_content(&log);
    assert_eq!(log["failed_stage"], "resolve");
    assert_eq!(log["resolve_error"], "boom");
    assert!(log["resolve_ms"].is_i64());
}

/// The tuning workflow — "which slots came back null across these turns" — has to
/// work for an admin who did not have the conversation, without the values.
#[tokio::test]
async fn test_non_owner_sees_extract_shape_but_not_extract_output() {
    let server = TestServer::start_api_only().await;
    let other_admin = create_user_client(&server, "admin2@test.com", "admin").await;
    let other_admin_id = me(&other_admin).await;
    let id = log_successful_turn(&server, other_admin_id, "download season 24 of someShow").await;

    let owner_view: Value = {
        let resp = other_admin
            .get(&format!("/api/v1/admin/chat/logs/{id}"))
            .await;
        resp.assert_ok();
        resp.json()
    };
    assert_eq!(owner_view["content"]["extract_output"]["season"], 24);
    assert_eq!(owner_view["content"]["extract_output"]["title"], "someShow");

    for path in [
        format!("/api/v1/admin/chat/logs/{id}"),
        "/api/v1/admin/chat/logs".to_string(),
    ] {
        let resp = server.admin_client().get(&path).await;
        resp.assert_ok();
        let body: Value = resp.json();
        let log = if body["logs"].is_array() {
            find(body["logs"].as_array().unwrap(), id).clone()
        } else {
            body
        };

        assert_no_content(&log);
        assert_eq!(
            log["extract_shape"],
            json!({ "title": "string", "season": "integer", "episode": null }),
            "{path}"
        );
        assert!(
            !log.to_string().contains("someShow"),
            "an extracted value leaked into a non-owner response: {path}"
        );
    }
}

// =============================================================================
// Retention
// =============================================================================

#[tokio::test]
async fn test_retention_sweep_deletes_only_old_turns() {
    let server = TestServer::start_api_only().await;
    let user_id = admin_user_id(&server).await;
    let old = log_successful_turn(&server, user_id, "old turn").await;
    let recent = log_successful_turn(&server, user_id, "recent turn").await;

    sqlx::query("UPDATE chat_turn_logs SET created_at = NOW() - INTERVAL '31 days' WHERE id = $1")
        .bind(old)
        .execute(&server.pool)
        .await
        .unwrap();

    let deleted = chat_turn_logs::delete_older_than_days(&server.pool, 30)
        .await
        .unwrap();
    assert_eq!(deleted, 1);

    let resp = server.admin_client().get("/api/v1/admin/chat/logs").await;
    resp.assert_ok();
    let body: Value = resp.json();
    assert_eq!(body["total"], 1);
    assert_eq!(body["logs"][0]["id"], recent.to_string());
}
