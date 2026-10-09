use crate::common::*;
use axum::http::StatusCode;
use serde_json::Value;

/// Test that proxy logging can be disabled entirely.
#[tokio::test]
async fn test_proxy_logging_disabled_creates_no_logs() {
    let server = TestServer::start().await;

    // Disable proxy logging
    server
        .admin_client()
        .patch_json(
            "/api/v1/admin/config",
            &serde_json::json!({ "proxy_logging_enabled": false }),
        )
        .await
        .assert_ok();

    // Create a device and make a request through the proxy
    let user_id = create_user(&server, "nologs@test.com").await;
    let (_, device_token) = create_device(&server, user_id).await;

    // Make a sync request (this would normally be logged)
    server
        .kobo_client()
        .kobo_sync(&device_token)
        .await
        .assert_ok();

    // Check proxy logs - should be empty since logging was disabled
    let resp: Value = server
        .admin_client()
        .get("/api/v1/admin/proxy-logs")
        .await
        .assert_ok()
        .json();
    let logs = resp["logs"].as_array().expect("Expected logs array");

    // No logs should have been created
    assert!(
        logs.is_empty(),
        "Expected no logs when proxy_logging_enabled=false, got {} logs",
        logs.len()
    );
}

/// Test that proxy logging enabled creates logs.
#[tokio::test]
async fn test_proxy_logging_enabled_creates_logs() {
    let server = TestServer::start().await;

    // Ensure proxy logging is enabled (default)
    server
        .admin_client()
        .patch_json(
            "/api/v1/admin/config",
            &serde_json::json!({ "proxy_logging_enabled": true }),
        )
        .await
        .assert_ok();

    // Create a device and make a request through the proxy
    let user_id = create_user(&server, "withlogs@test.com").await;
    let (_, device_token) = create_device(&server, user_id).await;

    // Make a sync request
    server
        .kobo_client()
        .kobo_sync(&device_token)
        .await
        .assert_ok();

    // Check proxy logs - should have at least one entry
    let resp: Value = server
        .admin_client()
        .get("/api/v1/admin/proxy-logs")
        .await
        .assert_ok()
        .json();
    let logs = resp["logs"].as_array().expect("Expected logs array");

    assert!(
        !logs.is_empty(),
        "Expected logs when proxy_logging_enabled=true"
    );

    // Verify the log has expected fields
    let log = &logs[0];
    assert!(log["request_method"].is_string());
    assert!(log["request_path"].is_string());
    assert!(log["response_status"].is_number());
    assert!(log["status"].is_string()); // 'complete', 'in_progress', or 'error'
}

/// Test that completed requests have status='complete'.
#[tokio::test]
async fn test_completed_requests_have_complete_status() {
    let server = TestServer::start().await;

    // Ensure proxy logging is enabled
    server
        .admin_client()
        .patch_json(
            "/api/v1/admin/config",
            &serde_json::json!({ "proxy_logging_enabled": true }),
        )
        .await
        .assert_ok();

    let user_id = create_user(&server, "complete@test.com").await;
    let (_, device_token) = create_device(&server, user_id).await;

    // Make a request
    server
        .kobo_client()
        .kobo_sync(&device_token)
        .await
        .assert_ok();

    // Give a moment for async logging to complete
    tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;

    // Check that the log has status='complete'
    let resp: Value = server
        .admin_client()
        .get("/api/v1/admin/proxy-logs")
        .await
        .assert_ok()
        .json();
    let logs = resp["logs"].as_array().expect("Expected logs array");

    assert!(!logs.is_empty(), "Expected at least one log entry");

    let log = &logs[0];
    assert_eq!(
        log["status"].as_str(),
        Some("complete"),
        "Expected status='complete' for finished request"
    );
}

/// Test that binary responses show placeholder when blob capture is disabled.
#[tokio::test]
async fn test_binary_response_shows_placeholder_without_blob_capture() {
    let server = TestServer::start().await;

    // Enable logging but disable blob capture
    server
        .admin_client()
        .patch_json(
            "/api/v1/admin/config",
            &serde_json::json!({
                "proxy_logging_enabled": true,
                "blob_capture_enabled": false
            }),
        )
        .await
        .assert_ok();

    // Ingest a book with a cover (binary content)
    let user_id = create_user(&server, "binary@test.com").await;
    let (_, device_token) = create_device(&server, user_id).await;
    let book_id = create_book(&server, "Binary Test Book").await;

    // Upload a cover image (binary)
    upload_cover(&server, book_id, test_jpeg_image()).await;

    // Entitle
    entitle_book(&server, book_id).await;

    // Request the cover through the proxy (binary response)
    let cover_url = format!(
        "/kobo/{}/book-images/{}/100/100/80/false/image.jpg",
        device_token, book_id
    );
    let resp = server.kobo_client().get(&cover_url).await;
    // Cover should return 200 with image data
    resp.assert_ok();

    // Give a moment for async logging
    tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;

    // Check proxy logs for the cover request
    let resp: Value = server
        .admin_client()
        .get("/api/v1/admin/proxy-logs")
        .await
        .assert_ok()
        .json();
    let logs = resp["logs"].as_array().expect("Expected logs array");

    // Find the cover request log
    let cover_log = logs.iter().find(|l| {
        l["request_path"]
            .as_str()
            .map(|p| p.contains("image.jpg"))
            .unwrap_or(false)
    });

    if let Some(log) = cover_log {
        // If there was a binary response body, it should show the placeholder
        if log["response_body_size"].as_i64().unwrap_or(0) > 0 {
            let body = log["response_body"].as_str();
            // Either null (no body logged) or placeholder
            if let Some(b) = body {
                assert!(
                    b.starts_with("[Binary:"),
                    "Expected binary placeholder, got: {}",
                    b
                );
            }
        }
    }
}

/// Test that logs include request headers.
#[tokio::test]
async fn test_logs_include_headers() {
    let server = TestServer::start().await;

    // Ensure proxy logging is enabled
    server
        .admin_client()
        .patch_json(
            "/api/v1/admin/config",
            &serde_json::json!({ "proxy_logging_enabled": true }),
        )
        .await
        .assert_ok();

    let user_id = create_user(&server, "headers@test.com").await;
    let (_, device_token) = create_device(&server, user_id).await;

    // Make a request
    server
        .kobo_client()
        .kobo_sync(&device_token)
        .await
        .assert_ok();

    tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;

    let resp: Value = server
        .admin_client()
        .get("/api/v1/admin/proxy-logs")
        .await
        .assert_ok()
        .json();
    let logs = resp["logs"].as_array().expect("Expected logs array");

    assert!(!logs.is_empty());

    let log = &logs[0];
    // Request headers should be present as object (with arrays for duplicate keys)
    assert!(
        log["request_headers"].is_object(),
        "Expected request_headers to be an object"
    );
    // Response headers should be present for completed requests
    assert!(
        log["response_headers"].is_object(),
        "Expected response_headers to be an object"
    );
}

/// Test that JSON response bodies are logged in full.
#[tokio::test]
async fn test_json_response_body_logged() {
    let server = TestServer::start().await;

    server
        .admin_client()
        .patch_json(
            "/api/v1/admin/config",
            &serde_json::json!({ "proxy_logging_enabled": true }),
        )
        .await
        .assert_ok();

    let user_id = create_user(&server, "jsonbody@test.com").await;
    let (_, device_token) = create_device(&server, user_id).await;

    // Sync returns JSON
    server
        .kobo_client()
        .kobo_sync(&device_token)
        .await
        .assert_ok();

    tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;

    let resp: Value = server
        .admin_client()
        .get("/api/v1/admin/proxy-logs")
        .await
        .assert_ok()
        .json();
    let logs = resp["logs"].as_array().expect("Expected logs array");

    let sync_log = logs.iter().find(|l| {
        l["request_path"]
            .as_str()
            .map(|p| p.contains("/library/sync"))
            .unwrap_or(false)
    });

    assert!(sync_log.is_some(), "Should have a sync request logged");

    let log = sync_log.unwrap();
    // JSON responses should have body logged (not placeholder)
    let body = log["response_body"].as_str();
    if let Some(b) = body {
        assert!(
            !b.starts_with("[Binary:"),
            "JSON response should not show binary placeholder"
        );
        // Should be valid JSON
        assert!(
            serde_json::from_str::<Value>(b).is_ok(),
            "Response body should be valid JSON"
        );
    }
}

/// Test that clear_proxy_logs deletes all logs.
#[tokio::test]
async fn test_clear_proxy_logs() {
    let server = TestServer::start().await;

    // Enable proxy logging
    server
        .admin_client()
        .patch_json(
            "/api/v1/admin/config",
            &serde_json::json!({ "proxy_logging_enabled": true }),
        )
        .await
        .assert_ok();

    // Create some logs by making requests
    let user_id = create_user(&server, "clear@test.com").await;
    let (_, device_token) = create_device(&server, user_id).await;

    // Make a few requests to generate logs
    for _ in 0..3 {
        server
            .kobo_client()
            .kobo_sync(&device_token)
            .await
            .assert_ok();
    }

    tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;

    // Verify we have logs
    let resp: Value = server
        .admin_client()
        .get("/api/v1/admin/proxy-logs")
        .await
        .assert_ok()
        .json();
    let logs = resp["logs"].as_array().expect("Expected logs array");
    assert!(!logs.is_empty(), "Should have logs before clearing");

    // Clear all logs
    let resp: Value = server
        .admin_client()
        .delete("/api/v1/admin/proxy-logs")
        .await
        .assert_ok()
        .json();

    assert!(
        resp["deleted"].as_i64().unwrap() > 0,
        "Should report deleted count"
    );

    // Verify logs are gone
    let resp: Value = server
        .admin_client()
        .get("/api/v1/admin/proxy-logs")
        .await
        .assert_ok()
        .json();
    let logs = resp["logs"].as_array().expect("Expected logs array");
    assert!(logs.is_empty(), "Logs should be cleared");
}

/// Test that clear_proxy_logs requires authentication.
#[tokio::test]
async fn test_clear_proxy_logs_requires_auth() {
    let server = TestServer::start().await;
    let client = TestClient::new(&server.api_url);

    // Try without auth
    client
        .delete("/api/v1/admin/proxy-logs")
        .await
        .assert_status(StatusCode::UNAUTHORIZED);
}

/// Test that export_proxy_logs returns a ZIP file.
#[tokio::test]
async fn test_export_proxy_logs() {
    let server = TestServer::start().await;

    // Enable logging
    server
        .admin_client()
        .patch_json(
            "/api/v1/admin/config",
            &serde_json::json!({ "proxy_logging_enabled": true }),
        )
        .await
        .assert_ok();

    // Generate some logs
    let user_id = create_user(&server, "export@test.com").await;
    let (_, device_token) = create_device(&server, user_id).await;

    server
        .kobo_client()
        .kobo_sync(&device_token)
        .await
        .assert_ok();

    tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;

    // Export all logs - returns a ZIP file
    let resp = server
        .admin_client()
        .get("/api/v1/admin/proxy-logs/export")
        .await;

    resp.assert_ok();
    // Should have application/zip content type
    let content_type = resp.headers.get("content-type");
    assert!(content_type.is_some(), "Should have content-type header");
    let content_type_str = content_type.unwrap().to_str().unwrap();
    assert!(
        content_type_str.contains("zip"),
        "Should be a ZIP file, got: {}",
        content_type_str
    );
}

/// Test that export_proxy_logs requires auth.
#[tokio::test]
async fn test_export_proxy_logs_requires_auth() {
    let server = TestServer::start().await;
    let client = TestClient::new(&server.api_url);

    client
        .get("/api/v1/admin/proxy-logs/export")
        .await
        .assert_status(StatusCode::UNAUTHORIZED);
}

/// Test that proxy logs include entity_refs and has_alert fields.
#[tokio::test]
async fn test_proxy_logs_entity_refs_returned() {
    let server = TestServer::start().await;

    server
        .admin_client()
        .patch_json(
            "/api/v1/admin/config",
            &serde_json::json!({ "proxy_logging_enabled": true }),
        )
        .await
        .assert_ok();

    let user_id = create_user(&server, "entity_refs@test.com").await;
    let (_, device_token) = create_device(&server, user_id).await;
    let book_id = create_book(&server, "Entity Refs Test Book").await;
    entitle_book(&server, book_id).await;

    // Request a book's metadata (path will contain book UUID)
    let metadata_url = format!("/kobo/{}/v1/library/{}/metadata", device_token, book_id);
    server.kobo_client().get(&metadata_url).await;

    tokio::time::sleep(tokio::time::Duration::from_millis(200)).await;

    let resp: Value = server
        .admin_client()
        .get("/api/v1/admin/proxy-logs")
        .await
        .assert_ok()
        .json();
    let logs = resp["logs"].as_array().expect("Expected logs array");

    // Find the metadata request log
    let metadata_log = logs.iter().find(|l| {
        l["request_path"]
            .as_str()
            .map(|p| p.contains("metadata"))
            .unwrap_or(false)
    });

    assert!(
        metadata_log.is_some(),
        "Should have a metadata request logged"
    );
    let log = metadata_log.unwrap();

    // entity_refs should be an array
    assert!(
        log["entity_refs"].is_array(),
        "entity_refs should be an array, got: {:?}",
        log["entity_refs"]
    );

    // has_alert should be a boolean
    assert!(
        log["has_alert"].is_boolean(),
        "has_alert should be a boolean"
    );

    // For an intercepted request without catch-all, has_alert should be false
    if log["decision"].as_str() == Some("intercept") && log["alert_message"].is_null() {
        assert_eq!(
            log["has_alert"].as_bool(),
            Some(false),
            "Normal intercepted requests should not be flagged as alerts"
        );
    }

    // entity_refs should contain the book
    let refs = log["entity_refs"].as_array().unwrap();
    let has_book_ref = refs.iter().any(|r| {
        r["entity_type"].as_str() == Some("book")
            && r["uuid"].as_str() == Some(&book_id.to_string())
    });
    assert!(
        has_book_ref,
        "entity_refs should contain the book UUID {}. Got: {:?}",
        book_id, refs
    );
}

/// Test that uuid-resolve endpoint works.
#[tokio::test]
async fn test_uuid_resolve_endpoint() {
    let server = TestServer::start_api_only().await;

    let book_id = create_book(&server, "Resolve Test Book").await;

    // Resolve known book UUID
    let resp: Value = server
        .admin_client()
        .get(&format!("/api/v1/admin/uuid-resolve?ids={}", book_id))
        .await
        .assert_ok()
        .json();

    let entities = resp["entities"]
        .as_object()
        .expect("Expected entities object");
    let book_entry = &entities[&book_id.to_string()];
    assert!(book_entry.is_object(), "Book should be resolved");
    assert_eq!(book_entry["entity_type"].as_str(), Some("book"));
    assert_eq!(book_entry["name"].as_str(), Some("Resolve Test Book"));
}

/// Test that uuid-resolve returns null for unknown UUIDs.
#[tokio::test]
async fn test_uuid_resolve_unknown() {
    let server = TestServer::start_api_only().await;

    let unknown_uuid = uuid::Uuid::new_v4();
    let resp: Value = server
        .admin_client()
        .get(&format!("/api/v1/admin/uuid-resolve?ids={}", unknown_uuid))
        .await
        .assert_ok()
        .json();

    let entities = resp["entities"]
        .as_object()
        .expect("Expected entities object");
    let entry = &entities[&unknown_uuid.to_string()];
    assert!(entry.is_null(), "Unknown UUID should resolve to null");
}

/// Test that uuid-resolve requires auth.
#[tokio::test]
async fn test_uuid_resolve_requires_auth() {
    let server = TestServer::start_api_only().await;
    let client = TestClient::new(&server.api_url);

    client
        .get("/api/v1/admin/uuid-resolve?ids=12345678-1234-1234-1234-123456789abc")
        .await
        .assert_status(StatusCode::UNAUTHORIZED);
}

/// Test that has_alert filter works.
#[tokio::test]
async fn test_proxy_logs_filter_by_alert() {
    let server = TestServer::start().await;

    server
        .admin_client()
        .patch_json(
            "/api/v1/admin/config",
            &serde_json::json!({ "proxy_logging_enabled": true }),
        )
        .await
        .assert_ok();

    let user_id = create_user(&server, "alert_filter@test.com").await;
    let (_, device_token) = create_device(&server, user_id).await;

    // Make a sync request (no book UUID, should not be an alert)
    server
        .kobo_client()
        .kobo_sync(&device_token)
        .await
        .assert_ok();

    tokio::time::sleep(tokio::time::Duration::from_millis(200)).await;

    // Filter with has_alert=true - should return 0 results for basic sync
    let resp: Value = server
        .admin_client()
        .get("/api/v1/admin/proxy-logs?has_alert=true")
        .await
        .assert_ok()
        .json();
    let logs = resp["logs"].as_array().expect("Expected logs array");

    // Basic sync should not produce alerts
    assert!(
        logs.is_empty(),
        "Basic sync should not produce alert-flagged logs, got {} logs",
        logs.len()
    );

    // Without filter, should have logs
    let resp: Value = server
        .admin_client()
        .get("/api/v1/admin/proxy-logs")
        .await
        .assert_ok()
        .json();
    let all_logs = resp["logs"].as_array().expect("Expected logs array");
    assert!(!all_logs.is_empty(), "Should have logs without filter");
}
