//! Passthrough mode tests - Kobbler is invisible, everything goes to Kobo upstream.
//!
//! In passthrough mode:
//! - All requests pass through to upstream unchanged
//! - No local books are served
//! - No merging occurs
//! - Useful for debugging or when you want pure Kobo sync

use crate::common::*;
use api::config::KoboMode;
use serde_json::Value;

// =============================================================================
// Passthrough Sync Tests
// =============================================================================

#[tokio::test]
async fn test_passthrough_mode_forwards_sync_to_upstream() {
    let kobo_mock = KoboMock::start().await;
    kobo_mock.add_simple_book("pass-book-1", "Passthrough Book");

    let server = TestServer::start_with_mode(KoboMode::Passthrough, None).await;

    let user_id = create_user(&server, "pass@test.com").await;
    let (device_id, device_token) = create_device(&server, user_id).await;
    set_device_upstream(&server, device_id, kobo_mock.url()).await;

    // Sync should go to upstream
    let items: Vec<Value> = server
        .kobo_client()
        .kobo_sync(&device_token)
        .await
        .assert_ok()
        .json();

    // Should only contain store books
    assert_sync_contains_book_str(&items, "pass-book-1");
    assert!(
        kobo_mock.was_called("GET", "/v1/library/sync"),
        "Sync should hit upstream"
    );
}

#[tokio::test]
async fn test_passthrough_mode_local_books_not_visible() {
    let kobo_mock = KoboMock::start().await;

    let server = TestServer::start_with_mode(KoboMode::Passthrough, None).await;

    let user_id = create_user(&server, "passlocal@test.com").await;
    let (device_id, device_token) = create_device(&server, user_id).await;
    set_device_upstream(&server, device_id, kobo_mock.url()).await;

    // Create a local book (it should NOT appear in passthrough mode)
    let local_book = create_book(&server, "Invisible Local Book").await;
    entitle_book(&server, local_book).await;

    // Sync should NOT contain local books
    let items: Vec<Value> = server
        .kobo_client()
        .kobo_sync(&device_token)
        .await
        .assert_ok()
        .json();

    // Local book should not appear (upstream returns empty)
    assert_sync_excludes_book(&items, local_book);
}

#[tokio::test]
async fn test_passthrough_mode_initialization_forwards() {
    let kobo_mock = KoboMock::start().await;

    let server = TestServer::start_with_mode(KoboMode::Passthrough, None).await;

    let user_id = create_user(&server, "passinit@test.com").await;
    let (device_id, device_token) = create_device(&server, user_id).await;
    set_device_upstream(&server, device_id, kobo_mock.url()).await;

    // Init should go to upstream
    let resp = server.kobo_client().kobo_init(&device_token).await;
    resp.assert_ok();

    assert!(
        kobo_mock.was_called("GET", "/v1/initialization"),
        "Init should hit upstream"
    );
}

#[tokio::test]
async fn test_passthrough_mode_reading_state_forwards() {
    let kobo_mock = KoboMock::start().await;
    kobo_mock.add_simple_book("pass-state-book", "Passthrough State Book");

    let server = TestServer::start_with_mode(KoboMode::Passthrough, None).await;

    let user_id = create_user(&server, "passstate@test.com").await;
    let (device_id, device_token) = create_device(&server, user_id).await;
    set_device_upstream(&server, device_id, kobo_mock.url()).await;

    // Reading state operations should go to upstream
    server
        .kobo_client()
        .kobo_get_state(&device_token, "pass-state-book")
        .await;

    assert!(
        kobo_mock.was_called("GET", "/state"),
        "Get state should hit upstream"
    );
}

#[tokio::test]
async fn test_passthrough_mode_metadata_forwards() {
    let kobo_mock = KoboMock::start().await;
    kobo_mock.add_simple_book("pass-meta-book", "Passthrough Meta Book");

    let server = TestServer::start_with_mode(KoboMode::Passthrough, None).await;

    let user_id = create_user(&server, "passmeta@test.com").await;
    let (device_id, device_token) = create_device(&server, user_id).await;
    set_device_upstream(&server, device_id, kobo_mock.url()).await;

    // Metadata requests should go to upstream
    server
        .kobo_client()
        .kobo_metadata(&device_token, "pass-meta-book")
        .await;

    assert!(
        kobo_mock.was_called("GET", "/metadata"),
        "Metadata should hit upstream"
    );
}

#[tokio::test]
async fn test_passthrough_mode_tags_forward() {
    let kobo_mock = KoboMock::start().await;

    let server = TestServer::start_with_mode(KoboMode::Passthrough, None).await;

    let user_id = create_user(&server, "passtag@test.com").await;
    let (device_id, device_token) = create_device(&server, user_id).await;
    set_device_upstream(&server, device_id, kobo_mock.url()).await;

    // Tag operations should go to upstream
    server
        .kobo_client()
        .kobo_create_tag(
            &device_token,
            &serde_json::json!({
                "Name": "Passthrough Tag"
            }),
        )
        .await;

    assert!(
        kobo_mock.was_called("POST", "/tags"),
        "Create tag should hit upstream"
    );
}

#[tokio::test]
async fn test_passthrough_mode_preserves_upstream_errors() {
    let kobo_mock = KoboMock::start().await;
    kobo_mock.set_sync_error(503); // Service unavailable

    let server = TestServer::start_with_mode(KoboMode::Passthrough, None).await;

    let user_id = create_user(&server, "passerr@test.com").await;
    let (device_id, device_token) = create_device(&server, user_id).await;
    set_device_upstream(&server, device_id, kobo_mock.url()).await;

    // Upstream error should be passed through
    let resp = server.kobo_client().kobo_sync(&device_token).await;

    assert_eq!(
        resp.status.as_u16(),
        503,
        "Upstream 503 should be passed through"
    );
}

#[tokio::test]
async fn test_passthrough_mode_preserves_upstream_headers() {
    let kobo_mock = KoboMock::start().await;
    kobo_mock.add_simple_book("header-book", "Header Test Book");

    let server = TestServer::start_with_mode(KoboMode::Passthrough, None).await;

    let user_id = create_user(&server, "passheader@test.com").await;
    let (device_id, device_token) = create_device(&server, user_id).await;
    set_device_upstream(&server, device_id, kobo_mock.url()).await;

    // Response should preserve upstream content-type
    let resp = server.kobo_client().kobo_sync(&device_token).await;
    resp.assert_ok();

    // Content type should be JSON
    let content_type = resp
        .headers
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    assert!(
        content_type.contains("application/json"),
        "Should preserve content-type"
    );
}
