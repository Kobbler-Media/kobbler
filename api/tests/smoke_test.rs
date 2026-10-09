use crate::common::*;
use reqwest::StatusCode;
use serde_json::json;

#[tokio::test]
async fn test_health_check() {
    let server = TestServer::start_api_only().await;

    let resp = server.api_client().get("/health").await;
    resp.assert_ok();
    assert_eq!(resp.text(), "ok");
}

#[tokio::test]
async fn test_login_success() {
    let server = TestServer::start_api_only().await;

    // The admin user is created during TestServer::start_api_only()
    let resp = server
        .api_client()
        .post_json(
            "/api/v1/admin/login",
            &json!({
                "email": "admin@test.local",
                "password": "testadmin123"
            }),
        )
        .await;

    resp.assert_ok();
    let body: serde_json::Value = resp.json();
    assert!(body["token"].as_str().is_some());
}

#[tokio::test]
async fn test_login_invalid_password() {
    let server = TestServer::start_api_only().await;

    let resp = server
        .api_client()
        .post_json(
            "/api/v1/admin/login",
            &json!({
                "email": "admin@test.local",
                "password": "wrongpassword"
            }),
        )
        .await;

    resp.assert_status(StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn test_protected_endpoint_requires_auth() {
    let server = TestServer::start_api_only().await;

    // Try to list users without auth
    let resp = server.api_client().get("/api/v1/admin/users").await;
    resp.assert_status(StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn test_protected_endpoint_with_auth() {
    let server = TestServer::start_api_only().await;

    // List users with admin auth
    let resp = server.admin_client().get("/api/v1/admin/users").await;
    resp.assert_ok();
}
