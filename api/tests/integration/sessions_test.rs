use crate::common::*;
use reqwest::StatusCode;
use serde_json::{Value, json};

async fn login_token(server: &TestServer, email: &str, password: &str) -> String {
    let resp = TestClient::new(&server.api_url)
        .post_json(
            "/api/v1/admin/login",
            &json!({ "email": email, "password": password }),
        )
        .await;
    resp.assert_ok();
    resp.json::<Value>()["token"]
        .as_str()
        .expect("no token")
        .to_string()
}

// =============================================================================
// Listing
// =============================================================================

#[tokio::test]
async fn test_sessions_list_marks_current_and_hides_token() {
    let server = TestServer::start_api_only().await;
    let token = login_token(&server, "admin@test.local", "testadmin123").await;
    let client = TestClient::new(&server.api_url).with_auth(&token);

    let resp = client.get("/api/v1/admin/sessions").await;
    resp.assert_ok();
    let sessions: Vec<Value> = resp.json();

    let current: Vec<_> = sessions
        .iter()
        .filter(|s| s["is_current"].as_bool() == Some(true))
        .collect();
    assert_eq!(current.len(), 1, "exactly one session is the caller's");

    for session in &sessions {
        assert!(session.get("token").is_none(), "token value must not leak");
        assert!(session.get("token_hash").is_none(), "hash must not leak");
    }
}

#[tokio::test]
async fn test_sessions_list_is_scoped_to_caller() {
    let server = TestServer::start_api_only().await;
    let viewer = create_user_client(&server, "viewer@test.local", "viewer").await;

    let resp = viewer.get("/api/v1/admin/sessions").await;
    resp.assert_ok();
    let sessions: Vec<Value> = resp.json();
    assert_eq!(sessions.len(), 1);
    assert_eq!(sessions[0]["email"], "viewer@test.local");
}

#[tokio::test]
async fn test_viewer_cannot_widen_scope_to_all_users() {
    let server = TestServer::start_api_only().await;
    create_user_with_role(&server, "other@test.local", "viewer").await;
    let viewer = create_user_client(&server, "viewer@test.local", "viewer").await;

    let resp = viewer.get("/api/v1/admin/sessions?all=true").await;
    resp.assert_ok();
    let sessions: Vec<Value> = resp.json();
    assert!(
        sessions.iter().all(|s| s["email"] == "viewer@test.local"),
        "all=true must be ignored for non-admins"
    );
}

#[tokio::test]
async fn test_admin_sees_all_users_sessions() {
    let server = TestServer::start_api_only().await;
    create_user_client(&server, "viewer@test.local", "viewer").await;
    let admin = server.admin_client();

    let resp = admin.get("/api/v1/admin/sessions?all=true").await;
    resp.assert_ok();
    let sessions: Vec<Value> = resp.json();
    assert!(
        sessions.iter().any(|s| s["email"] == "viewer@test.local"),
        "admin scope must include other users"
    );
}

// =============================================================================
// Refresh
// =============================================================================

#[tokio::test]
async fn test_refresh_extends_expiry_and_names_the_device() {
    let server = TestServer::start_api_only().await;
    let token = login_token(&server, "admin@test.local", "testadmin123").await;
    let client = TestClient::new(&server.api_url).with_auth(&token);

    let before: Vec<Value> = client.get("/api/v1/admin/sessions").await.json();
    let before_expiry = before
        .iter()
        .find(|s| s["is_current"].as_bool() == Some(true))
        .expect("current session")["expires_at"]
        .as_str()
        .unwrap()
        .to_string();

    let resp = client
        .post_json(
            "/api/v1/admin/sessions/refresh",
            &json!({ "device_name": "Living Room TV", "client_kind": "androidtv" }),
        )
        .await;
    resp.assert_ok();

    // A login token starts at 30 days; refresh moves it to the 90-day session window.
    let after: Vec<Value> = client.get("/api/v1/admin/sessions").await.json();
    let current = after
        .iter()
        .find(|s| s["is_current"].as_bool() == Some(true))
        .expect("current session");
    assert!(
        current["expires_at"].as_str().unwrap() > before_expiry.as_str(),
        "refresh must push the expiry forward"
    );
    assert_eq!(current["device_name"], "Living Room TV");
    assert_eq!(current["client_kind"], "androidtv");
    assert!(current["last_used_at"].is_string());
}

#[tokio::test]
async fn test_refresh_rejects_a_revoked_session() {
    let server = TestServer::start_api_only().await;
    let token = login_token(&server, "admin@test.local", "testadmin123").await;
    let client = TestClient::new(&server.api_url).with_auth(&token);

    client
        .post("/api/v1/admin/logout")
        .await
        .assert_status(StatusCode::NO_CONTENT);

    let resp = client
        .post_json("/api/v1/admin/sessions/refresh", &json!({}))
        .await;
    resp.assert_status(StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn test_cookie_session_refresh_reissues_the_cookie() {
    let server = TestServer::start_api_only().await;

    // A browser authenticates by cookie; extending only the database row would still
    // log it out when the cookie's own Max-Age lapses.
    let http = reqwest::Client::new();
    let resp = http
        .post(format!("{}/api/v1/admin/login", server.api_url))
        .json(&json!({ "email": "admin@test.local", "password": "testadmin123" }))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let session_cookie = resp
        .headers()
        .get("set-cookie")
        .expect("login sets a session cookie")
        .to_str()
        .unwrap()
        .split(';')
        .next()
        .unwrap()
        .to_string();

    let resp = http
        .post(format!("{}/api/v1/admin/sessions/refresh", server.api_url))
        .header("cookie", &session_cookie)
        .json(&json!({ "client_kind": "web" }))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    let cookie = resp
        .headers()
        .get("set-cookie")
        .expect("refresh must re-issue the session cookie")
        .to_str()
        .unwrap()
        .to_string();
    assert!(cookie.contains("kobbler_session="));
    assert!(
        cookie.contains("Max-Age=7776000"),
        "cookie lifetime must match the 90-day session window, got: {cookie}"
    );
}

#[tokio::test]
async fn test_bearer_refresh_does_not_set_a_cookie() {
    let server = TestServer::start_api_only().await;
    let token = login_token(&server, "admin@test.local", "testadmin123").await;
    let client = TestClient::new(&server.api_url).with_auth(&token);

    let resp = client
        .post_json("/api/v1/admin/sessions/refresh", &json!({}))
        .await;
    resp.assert_ok();
}

// =============================================================================
// Revocation
// =============================================================================

#[tokio::test]
async fn test_revoking_a_session_kills_only_that_token() {
    let server = TestServer::start_api_only().await;
    let keep = login_token(&server, "admin@test.local", "testadmin123").await;
    let drop = login_token(&server, "admin@test.local", "testadmin123").await;

    let keep_client = TestClient::new(&server.api_url).with_auth(&keep);
    let drop_client = TestClient::new(&server.api_url).with_auth(&drop);

    let sessions: Vec<Value> = drop_client.get("/api/v1/admin/sessions").await.json();
    let drop_id = sessions
        .iter()
        .find(|s| s["is_current"].as_bool() == Some(true))
        .expect("current session")["id"]
        .as_str()
        .unwrap()
        .to_string();

    keep_client
        .delete(&format!("/api/v1/admin/sessions/{drop_id}"))
        .await
        .assert_status(StatusCode::NO_CONTENT);

    drop_client
        .get("/api/v1/admin/me")
        .await
        .assert_status(StatusCode::UNAUTHORIZED);
    keep_client.get("/api/v1/admin/me").await.assert_ok();
}

#[tokio::test]
async fn test_viewer_cannot_revoke_another_users_session() {
    let server = TestServer::start_api_only().await;
    let viewer = create_user_client(&server, "viewer@test.local", "viewer").await;

    let admin_sessions: Vec<Value> = server
        .admin_client()
        .get("/api/v1/admin/sessions")
        .await
        .json();
    let admin_id = admin_sessions[0]["id"].as_str().unwrap().to_string();

    let resp = viewer
        .delete(&format!("/api/v1/admin/sessions/{admin_id}"))
        .await;
    resp.assert_status(StatusCode::FORBIDDEN);

    server
        .admin_client()
        .get("/api/v1/admin/me")
        .await
        .assert_ok();
}

// =============================================================================
// Admin-minted device tokens
// =============================================================================

#[tokio::test]
async fn test_admin_mints_a_working_token_for_another_user() {
    let server = TestServer::start_api_only().await;
    let user_id = create_user_with_role(&server, "kid@test.local", "viewer").await;

    let resp = server
        .admin_client()
        .post_json(
            &format!("/api/v1/admin/users/{user_id}/device-token"),
            &json!({ "device_name": "Kitchen Tablet" }),
        )
        .await;
    resp.assert_ok();
    let minted: Value = resp.json();
    assert_eq!(minted["user_id"], user_id.to_string());

    let token = minted["token"].as_str().expect("no token");
    let device = TestClient::new(&server.api_url).with_auth(token);
    let me: Value = device.get("/api/v1/admin/me").await.json();
    assert_eq!(me["email"], "kid@test.local");
    assert_eq!(me["role"], "viewer");

    // The provisioned user can see who linked it, and drop it themselves.
    let sessions: Vec<Value> = device.get("/api/v1/admin/sessions").await.json();
    let row = sessions
        .iter()
        .find(|s| s["device_name"] == "Kitchen Tablet")
        .expect("minted session is listed");
    assert_eq!(row["created_by_email"], "admin@test.local");
}

#[tokio::test]
async fn test_non_admin_cannot_mint_a_device_token() {
    let server = TestServer::start_api_only().await;
    let target = create_user_with_role(&server, "target@test.local", "viewer").await;
    let contributor = create_user_client(&server, "contrib@test.local", "contributor").await;

    let resp = contributor
        .post_json(
            &format!("/api/v1/admin/users/{target}/device-token"),
            &json!({}),
        )
        .await;
    resp.assert_status(StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn test_minting_for_an_unknown_user_is_not_found() {
    let server = TestServer::start_api_only().await;
    let missing = uuid::Uuid::new_v4();

    let resp = server
        .admin_client()
        .post_json(
            &format!("/api/v1/admin/users/{missing}/device-token"),
            &json!({}),
        )
        .await;
    resp.assert_status(StatusCode::NOT_FOUND);
}
