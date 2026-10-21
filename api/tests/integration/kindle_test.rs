use crate::common::*;
use reqwest::StatusCode;
use serde_json::{Value, json};
use uuid::Uuid;

// =============================================================================
// Auth Tests
// =============================================================================

#[tokio::test]
async fn test_kindle_endpoints_require_auth() {
    let server = TestServer::start_api_only().await;
    let client = TestClient::new(&server.api_url);

    client
        .get("/api/v1/admin/kindle/account")
        .await
        .assert_status(StatusCode::UNAUTHORIZED);

    client
        .get("/api/v1/admin/kindle/oauth-url")
        .await
        .assert_status(StatusCode::UNAUTHORIZED);

    client
        .post_json(
            "/api/v1/admin/kindle/oauth-complete",
            &json!({"redirect_url": "https://example.com"}),
        )
        .await
        .assert_status(StatusCode::UNAUTHORIZED);

    client
        .post("/api/v1/admin/kindle/refresh-devices")
        .await
        .assert_status(StatusCode::UNAUTHORIZED);

    client
        .delete("/api/v1/admin/kindle/account")
        .await
        .assert_status(StatusCode::UNAUTHORIZED);

    client
        .patch_json(
            &format!("/api/v1/admin/kindle/targets/{}", Uuid::new_v4()),
            &json!({"enabled": false}),
        )
        .await
        .assert_status(StatusCode::UNAUTHORIZED);
}

// =============================================================================
// Account Tests
// =============================================================================

#[tokio::test]
async fn test_kindle_account_not_connected() {
    let server = TestServer::start_api_only().await;

    let resp = server
        .admin_client()
        .get("/api/v1/admin/kindle/account")
        .await;
    resp.assert_ok();

    let json: Value = resp.json();
    assert_eq!(json["connected"], false);
    assert!(json["targets"].as_array().unwrap().is_empty());
}

#[tokio::test]
async fn test_kindle_disconnect_not_found() {
    let server = TestServer::start_api_only().await;

    let resp = server
        .admin_client()
        .delete("/api/v1/admin/kindle/account")
        .await;
    resp.assert_status(StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn test_kindle_refresh_devices_not_found() {
    let server = TestServer::start_api_only().await;

    let resp = server
        .admin_client()
        .post("/api/v1/admin/kindle/refresh-devices")
        .await;
    resp.assert_status(StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn test_kindle_update_target_not_found() {
    let server = TestServer::start_api_only().await;

    let resp = server
        .admin_client()
        .patch_json(
            &format!("/api/v1/admin/kindle/targets/{}", Uuid::new_v4()),
            &json!({"enabled": true}),
        )
        .await;
    resp.assert_status(StatusCode::NOT_FOUND);
}

// =============================================================================
// Delivery Tests
// =============================================================================

#[tokio::test]
async fn test_kindle_book_deliveries_empty() {
    let server = TestServer::start_api_only().await;
    let book_id = Uuid::new_v4();

    let resp = server
        .admin_client()
        .get(&format!(
            "/api/v1/admin/books/{}/kindle/deliveries",
            book_id
        ))
        .await;
    resp.assert_ok();

    let json: Value = resp.json();
    assert!(json["deliveries"].as_array().unwrap().is_empty());
}

#[tokio::test]
async fn test_kindle_book_deliveries_require_auth() {
    let server = TestServer::start_api_only().await;
    let client = TestClient::new(&server.api_url);

    client
        .get(&format!(
            "/api/v1/admin/books/{}/kindle/deliveries",
            Uuid::new_v4()
        ))
        .await
        .assert_status(StatusCode::UNAUTHORIZED);

    client
        .post_json(
            &format!("/api/v1/admin/books/{}/kindle", Uuid::new_v4()),
            &json!({"target_id": Uuid::new_v4()}),
        )
        .await
        .assert_status(StatusCode::UNAUTHORIZED);
}
