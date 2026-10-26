use crate::common::*;
use reqwest::StatusCode;
use serde_json::Value;

// =============================================================================
// Logout
// =============================================================================

#[tokio::test]
async fn test_logout_invalidates_session() {
    let server = TestServer::start_api_only().await;

    // Login to get a fresh token
    let client = TestClient::new(&server.api_url);
    let resp = client
        .post_json(
            "/api/v1/admin/login",
            &serde_json::json!({
                "email": "admin@test.local",
                "password": "testadmin123",
            }),
        )
        .await;
    resp.assert_ok();
    let token = resp.json::<Value>()["token"].as_str().unwrap().to_string();

    let authed = TestClient::new(&server.api_url).with_auth(&token);

    // Verify /me works
    let resp = authed.get("/api/v1/admin/me").await;
    resp.assert_ok();

    // Logout
    let resp = authed.post("/api/v1/admin/logout").await;
    resp.assert_status(StatusCode::NO_CONTENT);

    // /me should now return 401
    let resp = authed.get("/api/v1/admin/me").await;
    resp.assert_status(StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn test_login_returns_email() {
    let server = TestServer::start_api_only().await;

    let client = TestClient::new(&server.api_url);
    let resp = client
        .post_json(
            "/api/v1/admin/login",
            &serde_json::json!({
                "email": "admin@test.local",
                "password": "testadmin123",
            }),
        )
        .await;
    resp.assert_ok();

    let json: Value = resp.json();
    assert_eq!(json["email"], "admin@test.local");
    assert_eq!(json["role"], "admin");
}

// =============================================================================
// Display name
// =============================================================================

#[tokio::test]
async fn test_display_name_set_clear_and_reported_by_me() {
    let server = TestServer::start_api_only().await;
    let admin = server.admin_client();

    // Nobody has set one yet, so /me reports none and the client falls back to the email.
    let resp = admin.get("/api/v1/admin/me").await;
    resp.assert_ok();
    assert!(resp.json::<Value>()["display_name"].is_null());

    let resp = admin
        .put_json(
            "/api/v1/admin/me/display-name",
            &serde_json::json!({ "display_name": "  Ron Weasley  " }),
        )
        .await;
    resp.assert_status(StatusCode::NO_CONTENT);

    let resp = admin.get("/api/v1/admin/me").await;
    resp.assert_ok();
    assert_eq!(resp.json::<Value>()["display_name"], "Ron Weasley");

    // Blank clears it rather than storing an empty name.
    let resp = admin
        .put_json(
            "/api/v1/admin/me/display-name",
            &serde_json::json!({ "display_name": "   " }),
        )
        .await;
    resp.assert_status(StatusCode::NO_CONTENT);

    let resp = admin.get("/api/v1/admin/me").await;
    resp.assert_ok();
    assert!(resp.json::<Value>()["display_name"].is_null());
}

#[tokio::test]
async fn test_display_name_rejects_overlong() {
    let server = TestServer::start_api_only().await;
    let admin = server.admin_client();

    let resp = admin
        .put_json(
            "/api/v1/admin/me/display-name",
            &serde_json::json!({ "display_name": "n".repeat(65) }),
        )
        .await;
    resp.assert_status(StatusCode::BAD_REQUEST);
}

// =============================================================================
// Password change
// =============================================================================

#[tokio::test]
async fn test_change_password() {
    let server = TestServer::start_api_only().await;
    let admin = server.admin_client();

    let resp = admin
        .put_json(
            "/api/v1/admin/me/password",
            &serde_json::json!({
                "current_password": "testadmin123",
                "new_password": "newpassword123",
            }),
        )
        .await;
    resp.assert_status(StatusCode::NO_CONTENT);

    // Old password should fail
    let client = TestClient::new(&server.api_url);
    let resp = client
        .post_json(
            "/api/v1/admin/login",
            &serde_json::json!({
                "email": "admin@test.local",
                "password": "testadmin123",
            }),
        )
        .await;
    resp.assert_status(StatusCode::UNAUTHORIZED);

    // New password should work
    let resp = client
        .post_json(
            "/api/v1/admin/login",
            &serde_json::json!({
                "email": "admin@test.local",
                "password": "newpassword123",
            }),
        )
        .await;
    resp.assert_ok();
}

#[tokio::test]
async fn test_change_password_wrong_current() {
    let server = TestServer::start_api_only().await;
    let admin = server.admin_client();

    let resp = admin
        .put_json(
            "/api/v1/admin/me/password",
            &serde_json::json!({
                "current_password": "wrong",
                "new_password": "newpassword123",
            }),
        )
        .await;
    resp.assert_status(StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn test_change_password_too_short() {
    let server = TestServer::start_api_only().await;
    let admin = server.admin_client();

    let resp = admin
        .put_json(
            "/api/v1/admin/me/password",
            &serde_json::json!({
                "current_password": "testadmin123",
                "new_password": "short",
            }),
        )
        .await;
    resp.assert_status(StatusCode::BAD_REQUEST);
}

// =============================================================================
// Avatar
// =============================================================================

#[tokio::test]
async fn test_avatar_upload_and_serve() {
    let server = TestServer::start_api_only().await;
    let admin = server.admin_client();

    // No avatar initially
    let me: Value = admin.get("/api/v1/admin/me").await.json();
    assert!(me["avatar_url"].is_null());

    // Upload a small JPEG-like payload (not a real image, but valid content type)
    let fake_jpeg = vec![0xFF, 0xD8, 0xFF, 0xE0, 0x00, 0x10]; // JPEG header bytes
    let resp = admin
        .put_bytes("/api/v1/admin/me/avatar", fake_jpeg, "image/jpeg")
        .await;
    resp.assert_status(StatusCode::NO_CONTENT);

    // /me should now have avatar_url
    let me: Value = admin.get("/api/v1/admin/me").await.json();
    let avatar_url = me["avatar_url"].as_str().unwrap();
    assert!(avatar_url.contains("/avatar"));

    // Fetch the avatar
    let resp = admin.get(avatar_url).await;
    resp.assert_ok();

    // Delete avatar
    let resp = admin.delete("/api/v1/admin/me/avatar").await;
    resp.assert_status(StatusCode::NO_CONTENT);

    // avatar_url should be null again
    let me: Value = admin.get("/api/v1/admin/me").await.json();
    assert!(me["avatar_url"].is_null());
}

#[tokio::test]
async fn test_avatar_upload_invalid_content_type() {
    let server = TestServer::start_api_only().await;
    let admin = server.admin_client();

    let resp = admin
        .put_bytes("/api/v1/admin/me/avatar", vec![0u8; 100], "text/plain")
        .await;
    resp.assert_status(StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn test_user_list_carries_each_users_avatar() {
    let server = TestServer::start_api_only().await;
    let admin = server.admin_client();
    let kid = create_user_client(&server, "kid@test.local", "viewer").await;

    kid.put_bytes(
        "/api/v1/admin/me/avatar",
        vec![0xFF, 0xD8, 0xFF, 0xE0],
        "image/jpeg",
    )
    .await
    .assert_status(StatusCode::NO_CONTENT);
    let kid_me: Value = kid.get("/api/v1/admin/me").await.json();
    let kid_avatar = kid_me["avatar_url"].as_str().unwrap();

    let users: Vec<Value> = admin.get("/api/v1/admin/users").await.json();
    let avatar_of = |email: &str| {
        users
            .iter()
            .find(|u| u["email"] == email)
            .map(|u| u["avatar_url"].clone())
            .unwrap()
    };
    assert_eq!(avatar_of("kid@test.local"), kid_avatar);
    assert!(avatar_of("admin@test.local").is_null());
    admin.get(kid_avatar).await.assert_ok();

    let own: Vec<Value> = kid.get("/api/v1/admin/users").await.json();
    assert_eq!(own.len(), 1);
    assert_eq!(own[0]["avatar_url"], kid_avatar);
}

#[tokio::test]
async fn test_replacing_an_avatar_changes_its_url() {
    let server = TestServer::start_api_only().await;
    let admin = server.admin_client();
    let upload = |bytes: Vec<u8>| {
        let admin = &admin;
        async move {
            admin
                .put_bytes("/api/v1/admin/me/avatar", bytes, "image/jpeg")
                .await
                .assert_status(StatusCode::NO_CONTENT);
            let me: Value = admin.get("/api/v1/admin/me").await.json();
            me["avatar_url"].as_str().unwrap().to_owned()
        }
    };

    let first = upload(vec![0xFF, 0xD8, 0xFF, 0xE0]).await;
    let second = upload(vec![0xFF, 0xD8, 0xFF, 0xE1]).await;
    assert_ne!(first, second);
    assert_eq!(upload(vec![0xFF, 0xD8, 0xFF, 0xE1]).await, second);

    let users: Vec<Value> = admin.get("/api/v1/admin/users").await.json();
    assert_eq!(users[0]["avatar_url"], second);
    admin.get(&second).await.assert_ok();
}
