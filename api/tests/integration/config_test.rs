use crate::common::*;
use reqwest::StatusCode;
use serde_json::Value;

#[tokio::test]
async fn test_config_includes_stt_and_hw_fields() {
    let server = TestServer::start_api_only().await;

    let resp = server.admin_client().get("/api/v1/admin/config").await;
    resp.assert_ok();

    let json: Value = resp.json();
    // New fields should be present (possibly null)
    assert!(json.get("stt_service_url").is_some());
    assert!(json.get("hw_encoder").is_some());
}

#[tokio::test]
async fn test_config_koblem_ui_disabled_by_default() {
    assert!(
        std::env::var("KOBLEM_UI_ENABLED").is_err(),
        "test process must not inherit KOBLEM_UI_ENABLED for this assertion to be meaningful"
    );

    let server = TestServer::start_api_only().await;

    let resp = server.admin_client().get("/api/v1/admin/config").await;
    resp.assert_ok();

    let json: Value = resp.json();
    assert_eq!(json["koblem_ui_enabled"], Value::Bool(false));
}

#[tokio::test]
async fn test_update_stt_service_url() {
    let server = TestServer::start_api_only().await;

    let resp = server
        .admin_client()
        .patch_json(
            "/api/v1/admin/config",
            &serde_json::json!({
                "stt_service_url": "http://stt-parakeet:8022"
            }),
        )
        .await;
    resp.assert_ok();

    let updated: Value = resp.json();
    assert_eq!(updated["stt_service_url"], "http://stt-parakeet:8022");

    // Verify persistence
    let resp = server.admin_client().get("/api/v1/admin/config").await;
    resp.assert_ok();
    let fetched: Value = resp.json();
    assert_eq!(fetched["stt_service_url"], "http://stt-parakeet:8022");
}

#[tokio::test]
async fn test_update_hw_encoder() {
    let server = TestServer::start_api_only().await;

    for valid in &["auto", "vaapi", "nvenc", "none"] {
        let resp = server
            .admin_client()
            .patch_json(
                "/api/v1/admin/config",
                &serde_json::json!({
                    "hw_encoder": valid
                }),
            )
            .await;
        resp.assert_ok();

        let updated: Value = resp.json();
        assert_eq!(updated["hw_encoder"], *valid);
    }
}

#[tokio::test]
async fn test_update_hw_encoder_invalid_rejected() {
    let server = TestServer::start_api_only().await;

    let resp = server
        .admin_client()
        .patch_json(
            "/api/v1/admin/config",
            &serde_json::json!({
                "hw_encoder": "invalid_value"
            }),
        )
        .await;
    // Should be rejected by validation
    assert_ne!(resp.status, StatusCode::OK);
}

#[tokio::test]
async fn test_get_config() {
    let server = TestServer::start().await;

    let resp = server.admin_client().get("/api/v1/admin/config").await;
    resp.assert_ok();

    let json: Value = resp.json();
    assert!(json["public_proxy_url"].is_string());
    assert!(json["kobo_storeapi_base_url"].is_string());
    assert!(json["kobo_mode"].is_string());
    assert!(json["proxy_logging_enabled"].is_boolean());
    assert!(json["proxy_log_retention_days"].is_number());
    assert!(json["blob_capture_enabled"].is_boolean());
    assert!(json["kobo_sync_batch_size"].is_number());
    assert!(json["activity_limit"].is_number());
    assert!(json["default_page_size"].is_number());
    assert!(json["max_page_size"].is_number());
}

#[tokio::test]
async fn test_get_config_requires_admin() {
    let server = TestServer::start().await;

    let client = TestClient::new(&server.api_url);
    let resp = client.get("/api/v1/admin/config").await;
    assert_eq!(resp.status, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn test_update_config() {
    let server = TestServer::start().await;

    // Get initial config
    let resp = server.admin_client().get("/api/v1/admin/config").await;
    resp.assert_ok();
    let initial: Value = resp.json();

    // Update some fields
    let resp = server
        .admin_client()
        .patch_json(
            "/api/v1/admin/config",
            &serde_json::json!({
                "proxy_log_retention_days": 14,
                "kobo_sync_batch_size": 200
            }),
        )
        .await;
    resp.assert_ok();

    let updated: Value = resp.json();
    assert_eq!(updated["proxy_log_retention_days"], 14);
    assert_eq!(updated["kobo_sync_batch_size"], 200);
    // Unchanged fields should remain
    assert_eq!(updated["public_proxy_url"], initial["public_proxy_url"]);

    // Verify changes persist
    let resp = server.admin_client().get("/api/v1/admin/config").await;
    resp.assert_ok();
    let fetched: Value = resp.json();
    assert_eq!(fetched["proxy_log_retention_days"], 14);
    assert_eq!(fetched["kobo_sync_batch_size"], 200);
}

#[tokio::test]
async fn test_update_config_requires_admin() {
    let server = TestServer::start().await;

    let client = TestClient::new(&server.api_url);
    let resp = client
        .patch_json(
            "/api/v1/admin/config",
            &serde_json::json!({
                "proxy_log_retention_days": 14
            }),
        )
        .await;
    assert_eq!(resp.status, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn test_update_kobo_mode() {
    let server = TestServer::start().await;

    // Change to passthrough mode
    let resp = server
        .admin_client()
        .patch_json(
            "/api/v1/admin/config",
            &serde_json::json!({
                "kobo_mode": "passthrough"
            }),
        )
        .await;
    resp.assert_ok();

    let updated: Value = resp.json();
    assert_eq!(updated["kobo_mode"], "passthrough");

    // Change back to local mode
    let resp = server
        .admin_client()
        .patch_json(
            "/api/v1/admin/config",
            &serde_json::json!({
                "kobo_mode": "local"
            }),
        )
        .await;
    resp.assert_ok();

    let updated: Value = resp.json();
    assert_eq!(updated["kobo_mode"], "local");
}

#[tokio::test]
async fn test_toggle_proxy_logging() {
    let server = TestServer::start().await;

    // Disable proxy logging
    let resp = server
        .admin_client()
        .patch_json(
            "/api/v1/admin/config",
            &serde_json::json!({
                "proxy_logging_enabled": false
            }),
        )
        .await;
    resp.assert_ok();

    let updated: Value = resp.json();
    assert_eq!(updated["proxy_logging_enabled"], false);

    // Re-enable
    let resp = server
        .admin_client()
        .patch_json(
            "/api/v1/admin/config",
            &serde_json::json!({
                "proxy_logging_enabled": true
            }),
        )
        .await;
    resp.assert_ok();

    let updated: Value = resp.json();
    assert_eq!(updated["proxy_logging_enabled"], true);
}

#[tokio::test]
async fn test_toggle_blob_capture() {
    let server = TestServer::start().await;

    // Enable blob capture
    let resp = server
        .admin_client()
        .patch_json(
            "/api/v1/admin/config",
            &serde_json::json!({
                "blob_capture_enabled": true
            }),
        )
        .await;
    resp.assert_ok();

    let updated: Value = resp.json();
    assert_eq!(updated["blob_capture_enabled"], true);

    // Disable blob capture
    let resp = server
        .admin_client()
        .patch_json(
            "/api/v1/admin/config",
            &serde_json::json!({
                "blob_capture_enabled": false
            }),
        )
        .await;
    resp.assert_ok();

    let updated: Value = resp.json();
    assert_eq!(updated["blob_capture_enabled"], false);
}

#[tokio::test]
async fn test_update_public_proxy_url() {
    let server = TestServer::start().await;

    // Set explicit public_proxy_url
    let proxy_url = "https://kobo.example.com";

    let resp = server
        .admin_client()
        .patch_json(
            "/api/v1/admin/config",
            &serde_json::json!({
                "public_proxy_url": proxy_url
            }),
        )
        .await;
    resp.assert_ok();

    let updated: Value = resp.json();
    assert_eq!(updated["public_proxy_url"], proxy_url);

    // Verify device sync URL uses the explicit proxy URL
    let user_id = create_user(&server, "proxyurltest@test.com").await;
    let resp = server
        .admin_client()
        .post_json(
            "/api/v1/admin/devices",
            &serde_json::json!({
                "user_id": user_id,
                "name": "Proxy URL Test Device"
            }),
        )
        .await;
    resp.assert_ok();

    let device: Value = resp.json();
    let sync_url = device["sync_url"].as_str().unwrap();
    // Sync URL should start with the explicit proxy URL, not public_base_url
    assert!(
        sync_url.starts_with(proxy_url),
        "sync_url '{}' should start with '{}'",
        sync_url,
        proxy_url
    );
}

#[tokio::test]
async fn test_public_proxy_url_used_directly() {
    let server = TestServer::start().await;

    // Set proxy URL explicitly - should be used directly for sync URLs
    let proxy_url = "https://kobo.example.com/sync";

    let resp = server
        .admin_client()
        .patch_json(
            "/api/v1/admin/config",
            &serde_json::json!({
                "public_proxy_url": proxy_url
            }),
        )
        .await;
    resp.assert_ok();

    // Verify device sync URL uses public_proxy_url directly
    let user_id = create_user(&server, "directproxytest@test.com").await;
    let resp = server
        .admin_client()
        .post_json(
            "/api/v1/admin/devices",
            &serde_json::json!({
                "user_id": user_id,
                "name": "Direct Proxy URL Test Device"
            }),
        )
        .await;
    resp.assert_ok();

    let device: Value = resp.json();
    let sync_url = device["sync_url"].as_str().unwrap();
    // Sync URL should start with the explicit proxy URL
    assert!(
        sync_url.starts_with(proxy_url),
        "sync_url '{}' should start with '{}' (public_proxy_url)",
        sync_url,
        proxy_url
    );
}

// =============================================================================
// Public URL — the externally reachable base URL for links opened outside the app
// =============================================================================

#[tokio::test]
async fn public_url_is_present_and_unset_by_default() {
    let server = TestServer::start_api_only().await;

    let resp = server.admin_client().get("/api/v1/admin/config").await;
    resp.assert_ok();
    let json: Value = resp.json();
    assert!(
        json.get("public_url").is_some(),
        "the key is always in the payload"
    );
    assert!(
        json["public_url"].is_null(),
        "unset is the state every existing install is in"
    );
}

#[tokio::test]
async fn public_url_accepts_an_absolute_url_and_strips_the_trailing_slash() {
    let server = TestServer::start_api_only().await;

    for (sent, stored) in [
        ("https://kobbler.example.com", "https://kobbler.example.com"),
        (
            "https://kobbler.example.com/",
            "https://kobbler.example.com",
        ),
        ("  http://192.168.1.10:6767/  ", "http://192.168.1.10:6767"),
        (
            "https://example.com/kobbler/",
            "https://example.com/kobbler",
        ),
    ] {
        let resp = server
            .admin_client()
            .patch_json(
                "/api/v1/admin/config",
                &serde_json::json!({ "public_url": sent }),
            )
            .await;
        resp.assert_ok();
        let updated: Value = resp.json();
        assert_eq!(updated["public_url"], stored, "sent {sent:?}");

        let resp = server.admin_client().get("/api/v1/admin/config").await;
        resp.assert_ok();
        let fetched: Value = resp.json();
        assert_eq!(fetched["public_url"], stored, "did not persist {sent:?}");
    }
}

#[tokio::test]
async fn public_url_rejects_anything_that_would_produce_a_dead_link() {
    let server = TestServer::start_api_only().await;

    for bad in ["kobbler.example.com", "/movies", "ftp://host", "https://"] {
        let resp = server
            .admin_client()
            .patch_json(
                "/api/v1/admin/config",
                &serde_json::json!({ "public_url": bad }),
            )
            .await;
        assert_eq!(
            resp.status,
            StatusCode::BAD_REQUEST,
            "{bad:?} should be refused at the boundary"
        );
    }

    let resp = server.admin_client().get("/api/v1/admin/config").await;
    resp.assert_ok();
    let fetched: Value = resp.json();
    assert!(fetched["public_url"].is_null(), "nothing was stored");
}

#[tokio::test]
async fn public_url_can_be_cleared_again() {
    let server = TestServer::start_api_only().await;

    let resp = server
        .admin_client()
        .patch_json(
            "/api/v1/admin/config",
            &serde_json::json!({ "public_url": "https://kobbler.example.com" }),
        )
        .await;
    resp.assert_ok();

    let resp = server
        .admin_client()
        .patch_json(
            "/api/v1/admin/config",
            &serde_json::json!({"public_url": ""}),
        )
        .await;
    resp.assert_ok();
    let updated: Value = resp.json();
    assert!(updated["public_url"].is_null());
}

// The quickstart sets the real key in .env for the meilisearch container; a value in the
// copied template shadows MEILI_MASTER_KEY and every search call 403s.
#[test]
fn test_config_template_leaves_meili_key_to_env() {
    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../config/kobbler.toml.example"
    );
    let template = api::file_config::FileConfig::load(path).expect("template parses");
    assert_eq!(template.meili_master_key.as_deref().unwrap_or(""), "");
}
