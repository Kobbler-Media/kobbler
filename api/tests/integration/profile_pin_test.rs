use crate::common::*;
use reqwest::StatusCode;
use serde_json::{Value, json};

const ME: &str = "/api/v1/admin/me";
const PIN: &str = "/api/v1/admin/me/profile-pin";
const VERIFY: &str = "/api/v1/admin/me/verify-pin";

#[tokio::test]
async fn me_has_pin_false_by_default() {
    let server = TestServer::start_api_only().await;
    let resp = server.admin_client().get(ME).await;
    resp.assert_ok();
    let json: Value = resp.json();
    assert_eq!(json["has_pin"], false);
}

#[tokio::test]
async fn set_pin_then_me_has_pin_true() {
    let server = TestServer::start_api_only().await;
    let client = server.admin_client();

    client
        .put_json(PIN, &json!({ "pin": "4242" }))
        .await
        .assert_status(StatusCode::NO_CONTENT);

    let me: Value = client.get(ME).await.json();
    assert_eq!(me["has_pin"], true);
}

#[tokio::test]
async fn verify_matches_accepts_correct_pin() {
    let server = TestServer::start_api_only().await;
    let client = server.admin_client();

    client
        .put_json(PIN, &json!({ "pin": "1357" }))
        .await
        .assert_success();

    client
        .post_json(VERIFY, &json!({ "pin": "1357" }))
        .await
        .assert_status(StatusCode::NO_CONTENT);
}

#[tokio::test]
async fn verify_rejects_wrong_pin() {
    let server = TestServer::start_api_only().await;
    let client = server.admin_client();

    client
        .put_json(PIN, &json!({ "pin": "1357" }))
        .await
        .assert_success();

    client
        .post_json(VERIFY, &json!({ "pin": "0000" }))
        .await
        .assert_status(StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn verify_without_pin_returns_conflict() {
    let server = TestServer::start_api_only().await;
    let resp = server
        .admin_client()
        .post_json(VERIFY, &json!({ "pin": "1234" }))
        .await;
    resp.assert_status(StatusCode::CONFLICT);
}

#[tokio::test]
async fn clear_pin_removes_it() {
    let server = TestServer::start_api_only().await;
    let client = server.admin_client();

    client
        .put_json(PIN, &json!({ "pin": "9999" }))
        .await
        .assert_success();
    client
        .delete(PIN)
        .await
        .assert_status(StatusCode::NO_CONTENT);

    let me: Value = client.get(ME).await.json();
    assert_eq!(me["has_pin"], false);

    client
        .post_json(VERIFY, &json!({ "pin": "9999" }))
        .await
        .assert_status(StatusCode::CONFLICT);
}

#[tokio::test]
async fn set_pin_replaces_existing() {
    let server = TestServer::start_api_only().await;
    let client = server.admin_client();

    client
        .put_json(PIN, &json!({ "pin": "1111" }))
        .await
        .assert_success();
    client
        .put_json(PIN, &json!({ "pin": "2222" }))
        .await
        .assert_success();

    client
        .post_json(VERIFY, &json!({ "pin": "1111" }))
        .await
        .assert_status(StatusCode::UNAUTHORIZED);
    client
        .post_json(VERIFY, &json!({ "pin": "2222" }))
        .await
        .assert_status(StatusCode::NO_CONTENT);
}

#[tokio::test]
async fn pin_too_short_rejected() {
    let server = TestServer::start_api_only().await;
    server
        .admin_client()
        .put_json(PIN, &json!({ "pin": "ab" }))
        .await
        .assert_status(StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn pin_with_whitespace_rejected() {
    let server = TestServer::start_api_only().await;
    server
        .admin_client()
        .put_json(PIN, &json!({ "pin": "12 34" }))
        .await
        .assert_status(StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn pins_are_user_scoped() {
    let server = TestServer::start_api_only().await;
    let admin = server.admin_client();
    let viewer = create_user_client(&server, "viewer@test.com", "viewer").await;

    admin
        .put_json(PIN, &json!({ "pin": "1234" }))
        .await
        .assert_success();
    viewer
        .put_json(PIN, &json!({ "pin": "5678" }))
        .await
        .assert_success();

    admin
        .post_json(VERIFY, &json!({ "pin": "1234" }))
        .await
        .assert_status(StatusCode::NO_CONTENT);
    admin
        .post_json(VERIFY, &json!({ "pin": "5678" }))
        .await
        .assert_status(StatusCode::UNAUTHORIZED);
    viewer
        .post_json(VERIFY, &json!({ "pin": "5678" }))
        .await
        .assert_status(StatusCode::NO_CONTENT);
}

#[tokio::test]
async fn unauthenticated_pin_endpoints_rejected() {
    let server = TestServer::start_api_only().await;
    let anon = TestClient::new(&server.api_url);

    anon.put_json(PIN, &json!({ "pin": "1234" }))
        .await
        .assert_status(StatusCode::UNAUTHORIZED);
    anon.delete(PIN)
        .await
        .assert_status(StatusCode::UNAUTHORIZED);
    anon.post_json(VERIFY, &json!({ "pin": "1234" }))
        .await
        .assert_status(StatusCode::UNAUTHORIZED);
}
