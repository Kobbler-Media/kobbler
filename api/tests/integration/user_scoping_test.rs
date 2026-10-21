//! Per-user scoping of reads that used to be household-wide.
//!
//! Every test here sets up a *second* user — the bugs these cover are all
//! invisible on a single-user install.

use crate::common::*;
use reqwest::StatusCode;
use serde_json::{Value, json};
use std::time::Duration;
use uuid::Uuid;

/// Create a user and log them in. Returns (user_id, bearer token).
async fn user_with_token(server: &TestServer, email: &str, role: &str) -> (Uuid, String) {
    let user_id = create_user_with_role(server, email, role).await;
    let resp = TestClient::new(&server.api_url)
        .post_json(
            "/api/v1/admin/login",
            &json!({ "email": email, "password": "testpass123" }),
        )
        .await;
    resp.assert_ok();
    let token = resp.json::<Value>()["token"]
        .as_str()
        .expect("no token in login response")
        .to_string();
    (user_id, token)
}

fn client_for(server: &TestServer, token: &str) -> TestClient {
    TestClient::new(&server.api_url).with_auth(token)
}

/// Status-only GET. Never reads the body, so it is safe against handlers that
/// return a long-lived SSE stream or a proxied media body.
async fn get_status(server: &TestServer, token: &str, path: &str) -> StatusCode {
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(15))
        .build()
        .unwrap();
    client
        .get(format!("{}{}", server.api_url, path))
        .header("Authorization", format!("Bearer {token}"))
        .send()
        .await
        .expect("request failed")
        .status()
}

async fn insert_session(pool: &sqlx::PgPool, user_id: Uuid, release_title: &str) -> Uuid {
    let session_id = Uuid::now_v7();
    sqlx::query(
        r#"
        INSERT INTO debrid_stream_sessions
            (id, user_id, media_type, media_id, release_title, info_hash,
             debrid_provider, filename, filesize, debrid_url, debrid_url_expires_at)
        VALUES ($1, $2, 'movie', $3, $4, 'deadbeef',
                'realdebrid', 'file.mkv', 1000000,
                'https://127.0.0.1:1/nope.mkv', NULL)
        "#,
    )
    .bind(session_id)
    .bind(user_id)
    .bind(Uuid::now_v7())
    .bind(release_title)
    .execute(pool)
    .await
    .expect("failed to insert stream session");
    session_id
}

// =============================================================================
// #2 — GET /api/v1/library/devices leaked every user's devices
// =============================================================================

#[tokio::test]
async fn test_library_devices_lists_only_own_devices() {
    let server = TestServer::start_api_only().await;

    let (alice_id, alice_token) = user_with_token(&server, "alice-dev@test.com", "viewer").await;
    let bob_id = create_user_with_role(&server, "bob-dev@test.com", "viewer").await;
    let (alice_device, _) = create_device(&server, alice_id).await;
    let (bob_device, _) = create_device(&server, bob_id).await;

    let resp = client_for(&server, &alice_token)
        .get("/api/v1/library/devices")
        .await;
    resp.assert_ok();
    let devices: Vec<Value> = resp.json();
    let ids: Vec<&str> = devices.iter().map(|d| d["id"].as_str().unwrap()).collect();

    assert!(
        ids.contains(&alice_device.to_string().as_str()),
        "alice should see her own device, got {ids:?}"
    );
    assert!(
        !ids.contains(&bob_device.to_string().as_str()),
        "alice must NOT see bob's device, got {ids:?}"
    );
    assert!(
        devices
            .iter()
            .all(|d| d["user_id"].as_str() == Some(alice_id.to_string().as_str())),
        "every listed device must belong to alice: {devices:?}"
    );
}

/// Admins keep the household-wide view — the ingest-queue "send to device"
/// picker is admin-gated and does dispatch to other users' readers.
#[tokio::test]
async fn test_library_devices_admin_sees_all_devices() {
    let server = TestServer::start_api_only().await;

    let alice_id = create_user_with_role(&server, "alice-dev-admin@test.com", "viewer").await;
    let bob_id = create_user_with_role(&server, "bob-dev-admin@test.com", "viewer").await;
    let (alice_device, _) = create_device(&server, alice_id).await;
    let (bob_device, _) = create_device(&server, bob_id).await;

    let resp = server.admin_client().get("/api/v1/library/devices").await;
    resp.assert_ok();
    let devices: Vec<Value> = resp.json();
    let ids: Vec<&str> = devices.iter().map(|d| d["id"].as_str().unwrap()).collect();

    assert!(ids.contains(&alice_device.to_string().as_str()), "{ids:?}");
    assert!(ids.contains(&bob_device.to_string().as_str()), "{ids:?}");
}

// =============================================================================
// #3 — library stats shelves_count was a global COUNT(*)
// =============================================================================

#[tokio::test]
async fn test_library_stats_shelves_count_is_per_user() {
    let server = TestServer::start_api_only().await;

    let (_, alice_token) = user_with_token(&server, "alice-shelf@test.com", "viewer").await;
    let (_, bob_token) = user_with_token(&server, "bob-shelf@test.com", "viewer").await;
    let alice = client_for(&server, &alice_token);
    let bob = client_for(&server, &bob_token);

    for name in ["Alice A", "Alice B"] {
        alice
            .post_json("/api/v1/library/shelves", &json!({ "name": name }))
            .await
            .assert_ok();
    }
    bob.post_json("/api/v1/library/shelves", &json!({ "name": "Bob only" }))
        .await
        .assert_ok();

    let alice_count = alice.get("/api/v1/library/stats").await.json::<Value>()["shelves_count"]
        .as_i64()
        .unwrap();
    let bob_count = bob.get("/api/v1/library/stats").await.json::<Value>()["shelves_count"]
        .as_i64()
        .unwrap();

    assert_eq!(
        alice_count, 2,
        "alice owns 2 shelves, badge said {alice_count}"
    );
    assert_eq!(bob_count, 1, "bob owns 1 shelf, badge said {bob_count}");
}

/// The badge must match the list the shelves page renders, not the table size.
#[tokio::test]
async fn test_library_stats_shelves_count_matches_shelf_list() {
    let server = TestServer::start_api_only().await;

    let (_, alice_token) = user_with_token(&server, "alice-shelf-list@test.com", "viewer").await;
    let (_, bob_token) = user_with_token(&server, "bob-shelf-list@test.com", "viewer").await;
    let alice = client_for(&server, &alice_token);
    let bob = client_for(&server, &bob_token);

    alice
        .post_json("/api/v1/library/shelves", &json!({ "name": "Only Mine" }))
        .await
        .assert_ok();
    for name in ["Bob 1", "Bob 2", "Bob 3"] {
        bob.post_json("/api/v1/library/shelves", &json!({ "name": name }))
            .await
            .assert_ok();
    }

    let listed = alice.get("/api/v1/library/shelves").await.json::<Value>()["items"]
        .as_array()
        .unwrap()
        .len() as i64;
    let badge = alice.get("/api/v1/library/stats").await.json::<Value>()["shelves_count"]
        .as_i64()
        .unwrap();

    assert_eq!(
        badge, listed,
        "sidebar badge {badge} != listed shelves {listed}"
    );
}

// =============================================================================
// #4 — GET /api/v1/admin/stream/recent returned the household's last 10
// =============================================================================

#[tokio::test]
async fn test_stream_recent_lists_only_own_sessions() {
    let server = TestServer::start_api_only().await;

    let (alice_id, alice_token) = user_with_token(&server, "alice-stream@test.com", "viewer").await;
    let bob_id = create_user_with_role(&server, "bob-stream@test.com", "viewer").await;
    insert_session(&server.pool, alice_id, "Alice.Watched.This.2025").await;
    insert_session(&server.pool, bob_id, "Bobs.Private.Viewing.2025").await;

    let resp = client_for(&server, &alice_token)
        .get("/api/v1/admin/stream/recent")
        .await;
    resp.assert_ok();
    let body: Value = resp.json();
    let titles: Vec<&str> = body["sessions"]
        .as_array()
        .unwrap()
        .iter()
        .map(|s| s["release_title"].as_str().unwrap())
        .collect();

    assert_eq!(
        titles,
        vec!["Alice.Watched.This.2025"],
        "recent streams must be the caller's own only"
    );
}

/// Recents is a personal "continue watching" feed, so an admin gets their own
/// sessions too — not the household's.
#[tokio::test]
async fn test_stream_recent_admin_sees_only_own_sessions() {
    let server = TestServer::start_api_only().await;

    let bob_id = create_user_with_role(&server, "bob-stream-admin@test.com", "viewer").await;
    insert_session(&server.pool, bob_id, "Bobs.Private.Viewing.2025").await;

    let resp = server
        .admin_client()
        .get("/api/v1/admin/stream/recent")
        .await;
    resp.assert_ok();
    let body: Value = resp.json();
    assert!(
        body["sessions"].as_array().unwrap().is_empty(),
        "admin has no sessions of their own, got {body}"
    );
}

// =============================================================================
// #4 (adjacent) — session-addressed endpoints had no user_id predicate
// =============================================================================

#[tokio::test]
async fn test_stream_session_endpoints_reject_non_owner() {
    let server = TestServer::start_api_only().await;

    let alice_id = create_user_with_role(&server, "alice-idor@test.com", "viewer").await;
    let (_, bob_token) = user_with_token(&server, "bob-idor@test.com", "viewer").await;
    let bob = client_for(&server, &bob_token);

    let session = insert_session(&server.pool, alice_id, "Alice.Session.2025").await;

    // Bodies must be valid — a malformed body 422s before the ownership check.
    let progress = bob
        .post_json(
            &format!("/api/v1/admin/stream/{session}/progress"),
            &json!({ "progress_seconds": 42.0 }),
        )
        .await;
    assert_eq!(
        progress.status,
        StatusCode::NOT_FOUND,
        "progress on another user's session. Body: {}",
        progress.text()
    );

    let keep = bob
        .post(&format!("/api/v1/admin/stream/{session}/keep"))
        .await;
    assert_eq!(
        keep.status,
        StatusCode::NOT_FOUND,
        "keep of another user's session. Body: {}",
        keep.text()
    );

    let keep_state = bob
        .get(&format!("/api/v1/admin/stream/{session}/keep"))
        .await;
    assert_eq!(
        keep_state.status,
        StatusCode::NOT_FOUND,
        "keep state of another user's session. Body: {}",
        keep_state.text()
    );

    let pin = bob
        .post(&format!("/api/v1/admin/stream/{session}/pin"))
        .await;
    assert_eq!(
        pin.status,
        StatusCode::NOT_FOUND,
        "pin. Body: {}",
        pin.text()
    );

    let unpin = bob
        .delete(&format!("/api/v1/admin/stream/{session}/pin"))
        .await;
    assert_eq!(
        unpin.status,
        StatusCode::NOT_FOUND,
        "unpin. Body: {}",
        unpin.text()
    );

    // Status-only: these stream their bodies (SSE / proxied media).
    assert_eq!(
        get_status(
            &server,
            &bob_token,
            &format!("/api/v1/admin/stream/{session}/status")
        )
        .await,
        StatusCode::NOT_FOUND,
        "SSE status on another user's session"
    );
    assert_eq!(
        get_status(
            &server,
            &bob_token,
            &format!("/api/v1/admin/stream/{session}/play")
        )
        .await,
        StatusCode::NOT_FOUND,
        "play of another user's session"
    );

    let delete = bob.delete(&format!("/api/v1/admin/stream/{session}")).await;
    assert_eq!(
        delete.status,
        StatusCode::NOT_FOUND,
        "delete. Body: {}",
        delete.text()
    );

    // Nothing above may have mutated Alice's row.
    let row = sqlx::query_as::<_, (bool, Option<f64>)>(
        "SELECT pinned, progress_seconds FROM debrid_stream_sessions WHERE id = $1",
    )
    .bind(session)
    .fetch_optional(&server.pool)
    .await
    .unwrap();
    let (pinned, progress_seconds) = row.expect("alice's session must still exist");
    assert!(!pinned, "bob pinned alice's session");
    assert_eq!(
        progress_seconds.unwrap_or(0.0),
        0.0,
        "bob moved alice's playback position"
    );
}

#[tokio::test]
async fn test_stream_session_endpoints_allow_owner() {
    let server = TestServer::start_api_only().await;

    let (alice_id, alice_token) = user_with_token(&server, "alice-own@test.com", "viewer").await;
    let alice = client_for(&server, &alice_token);
    let session = insert_session(&server.pool, alice_id, "Alice.Own.2025").await;

    alice
        .post_json(
            &format!("/api/v1/admin/stream/{session}/progress"),
            &json!({ "progress_seconds": 12.5 }),
        )
        .await
        .assert_status(StatusCode::NO_CONTENT);
    alice
        .post(&format!("/api/v1/admin/stream/{session}/pin"))
        .await
        .assert_status(StatusCode::NO_CONTENT);
    alice
        .delete(&format!("/api/v1/admin/stream/{session}"))
        .await
        .assert_status(StatusCode::NO_CONTENT);
}

// =============================================================================
// Stremio /file/{session} must look the session up as the key's owner. It
// passed a nil user after stream_play became owner-scoped, so every Stremio
// playback 404'd.
// =============================================================================

async fn insert_stremio_key(pool: &sqlx::PgPool, user_id: Uuid, api_key: &str) {
    sqlx::query("INSERT INTO stremio_api_keys (user_id, api_key) VALUES ($1, $2)")
        .bind(user_id)
        .bind(api_key)
        .execute(pool)
        .await
        .expect("failed to insert stremio key");
}

async fn get_status_unauthed(server: &TestServer, path: &str) -> StatusCode {
    reqwest::Client::builder()
        .timeout(Duration::from_secs(15))
        .build()
        .unwrap()
        .get(format!("{}{}", server.api_url, path))
        .send()
        .await
        .expect("request failed")
        .status()
}

#[tokio::test]
async fn test_stremio_file_scoped_to_key_owner() {
    let server = TestServer::start_api_only().await;

    let alice_id = create_user_with_role(&server, "alice-stremio@test.com", "viewer").await;
    let bob_id = create_user_with_role(&server, "bob-stremio@test.com", "viewer").await;
    insert_stremio_key(&server.pool, alice_id, "alice-stremio-key").await;
    insert_stremio_key(&server.pool, bob_id, "bob-stremio-key").await;
    let session = insert_session(&server.pool, alice_id, "Alice.Stremio.2025").await;

    // The upstream URL is unreachable, so the owner gets an upstream error —
    // but never "session not found".
    assert_ne!(
        get_status_unauthed(
            &server,
            &format!("/stremio/alice-stremio-key/file/{session}")
        )
        .await,
        StatusCode::NOT_FOUND,
        "owner's stremio key must find the session"
    );
    assert_eq!(
        get_status_unauthed(&server, &format!("/stremio/bob-stremio-key/file/{session}")).await,
        StatusCode::NOT_FOUND,
        "another user's stremio key must not reach the session"
    );
    assert_eq!(
        get_status_unauthed(&server, &format!("/stremio/not-a-key/file/{session}")).await,
        StatusCode::UNAUTHORIZED,
        "unknown stremio key"
    );
}
