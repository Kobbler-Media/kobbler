use crate::common::*;
use reqwest::StatusCode;
use serde_json::Value;

#[tokio::test]
async fn test_chat_status_requires_auth() {
    let server = TestServer::start_api_only().await;

    let client = TestClient::new(&server.api_url);
    let resp = client.get("/api/v1/admin/chat/status").await;
    assert_eq!(resp.status, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn test_chat_status_returns_disabled_by_default() {
    let server = TestServer::start_api_only().await;

    let resp = server.admin_client().get("/api/v1/admin/chat/status").await;
    resp.assert_ok();

    let json: Value = resp.json();
    assert_eq!(json["enabled"], false);
    assert_eq!(json["configured"], false);
    assert!(json["provider"].is_null());
    assert!(json["model"].is_null());
}

#[tokio::test]
async fn test_chat_completions_requires_auth() {
    let server = TestServer::start_api_only().await;

    let client = TestClient::new(&server.api_url);
    let resp = client
        .post_json(
            "/api/v1/admin/chat/completions",
            &serde_json::json!({
                "model": "test",
                "messages": [{"role": "user", "content": "hello"}],
                "stream": true
            }),
        )
        .await;
    assert_eq!(resp.status, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn test_chat_completions_rejects_when_disabled() {
    let server = TestServer::start_api_only().await;

    let resp = server
        .admin_client()
        .post_json(
            "/api/v1/admin/chat/completions",
            &serde_json::json!({
                "model": "test",
                "messages": [{"role": "user", "content": "hello"}],
                "stream": true
            }),
        )
        .await;
    assert_eq!(resp.status, StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn test_chat_config_fields_in_system_config() {
    let server = TestServer::start_api_only().await;

    let resp = server.admin_client().get("/api/v1/admin/config").await;
    resp.assert_ok();

    let json: Value = resp.json();
    assert_eq!(json["chat_enabled"], false);
    assert!(json.get("chat_llm_provider").is_some());
    assert!(json.get("chat_llm_base_url").is_some());
    assert!(json.get("chat_llm_api_key").is_some());
    assert!(json.get("chat_llm_model").is_some());
}

#[tokio::test]
async fn test_update_chat_config() {
    let server = TestServer::start_api_only().await;

    let resp = server
        .admin_client()
        .patch_json(
            "/api/v1/admin/config",
            &serde_json::json!({
                "chat_enabled": true,
                "chat_llm_provider": "gemini",
                "chat_llm_api_key": "test-key-123",
                "chat_llm_model": "gemini-2.0-flash"
            }),
        )
        .await;
    resp.assert_ok();

    let updated: Value = resp.json();
    assert_eq!(updated["chat_enabled"], true);
    assert_eq!(updated["chat_llm_provider"], "gemini");
    assert_eq!(updated["chat_llm_api_key"], "test-key-123");
    assert_eq!(updated["chat_llm_model"], "gemini-2.0-flash");

    // Verify persistence
    let resp = server.admin_client().get("/api/v1/admin/config").await;
    resp.assert_ok();
    let fetched: Value = resp.json();
    assert_eq!(fetched["chat_enabled"], true);
    assert_eq!(fetched["chat_llm_provider"], "gemini");
}

#[tokio::test]
async fn test_chat_status_reflects_config() {
    let server = TestServer::start_api_only().await;

    // Enable chat with all required fields
    server
        .admin_client()
        .patch_json(
            "/api/v1/admin/config",
            &serde_json::json!({
                "chat_enabled": true,
                "chat_llm_provider": "openai",
                "chat_llm_api_key": "sk-test",
                "chat_llm_model": "gpt-4o-mini"
            }),
        )
        .await;

    let resp = server.admin_client().get("/api/v1/admin/chat/status").await;
    resp.assert_ok();

    let json: Value = resp.json();
    assert_eq!(json["enabled"], true);
    assert_eq!(json["configured"], true);
    assert_eq!(json["provider"], "openai");
    assert_eq!(json["model"], "gpt-4o-mini");
}

#[tokio::test]
async fn test_chat_status_not_configured_without_key() {
    let server = TestServer::start_api_only().await;

    // Enable chat but don't set API key
    server
        .admin_client()
        .patch_json(
            "/api/v1/admin/config",
            &serde_json::json!({
                "chat_enabled": true,
                "chat_llm_provider": "gemini"
            }),
        )
        .await;

    let resp = server.admin_client().get("/api/v1/admin/chat/status").await;
    resp.assert_ok();

    let json: Value = resp.json();
    assert_eq!(json["enabled"], true);
    assert_eq!(json["configured"], false);
}
