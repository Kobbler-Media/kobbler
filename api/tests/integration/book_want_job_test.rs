//! Tests for the BookWantHandler job handler.
//!
//! These tests exercise the handler directly, verifying it correctly
//! enqueues want_request jobs to Kobload via NATS and handles edge cases.

use crate::common::*;
use api::events::LibraryEvent;
use api::jobs::handlers::BookWantHandler;
use api::jobs::worker::JobHandler;
use serde_json::{Value, json};
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

/// Create a wanted book and configure Kobload settings. Returns the wanted book ID.
async fn setup_wanted_book(server: &TestServer) -> String {
    // Configure Kobload settings
    let resp = server
        .admin_client()
        .patch_json(
            "/api/v1/admin/config",
            &json!({
                "kobload_base_url": "http://localhost:9999",
                "kobload_api_key": "test-key",
                "kobload_ingest_url": format!("{}/api/v1/ingest/{}", server.api_url, server.ingest_token)
            }),
        )
        .await;
    resp.assert_ok();

    // Create a wanted book
    let create_resp = server
        .admin_client()
        .post_json(
            "/api/v1/admin/wanted",
            &json!({
                "title": "The Way of Kings",
                "author": "Brandon Sanderson",
                "identifiers": { "isbn13": "9780765326355" },
                "series": "The Stormlight Archive",
                "series_index": 1
            }),
        )
        .await;
    create_resp.assert_ok();
    create_resp.json::<Value>()["id"]
        .as_str()
        .unwrap()
        .to_string()
}

// =============================================================================
// Tests
// =============================================================================

#[tokio::test]
async fn test_book_want_enqueues_to_nats() {
    let server = TestServer::start_api_only().await;
    let wanted_id = setup_wanted_book(&server).await;

    let handler = BookWantHandler::new(
        server.pool.clone(),
        server.runtime_config.clone(),
        server.nats_publisher.clone(),
        server.library_events.clone(),
    );

    let result = handler
        .execute(
            Uuid::now_v7(),
            json!({ "book_id": wanted_id }),
            CancellationToken::new(),
        )
        .await;
    assert!(result.is_ok(), "handler should succeed: {:?}", result.err());

    let result_json = result.unwrap();
    assert_eq!(result_json["status"], "enqueued");
    assert_eq!(result_json["book_id"], wanted_id);

    // Verify want_status was updated to "searching"
    let book_resp = server
        .admin_client()
        .get(&format!("/api/v1/admin/wanted/{}", wanted_id))
        .await;
    book_resp.assert_ok();
    let book_json: Value = book_resp.json();
    assert_eq!(
        book_json["item"]["want_status"], "searching",
        "want_status should be 'searching' after enqueue"
    );
}

#[tokio::test]
async fn test_book_want_skips_without_config() {
    let server = TestServer::start_api_only().await;

    // Create wanted book WITHOUT configuring Kobload (no kobload_ingest_url)
    let create_resp = server
        .admin_client()
        .post_json(
            "/api/v1/admin/wanted",
            &json!({
                "title": "Elantris",
                "author": "Brandon Sanderson",
            }),
        )
        .await;
    create_resp.assert_ok();
    let wanted_id = create_resp.json::<Value>()["id"]
        .as_str()
        .unwrap()
        .to_string();

    let handler = BookWantHandler::new(
        server.pool.clone(),
        server.runtime_config.clone(),
        server.nats_publisher.clone(),
        server.library_events.clone(),
    );

    let result = handler
        .execute(
            Uuid::now_v7(),
            json!({ "book_id": wanted_id }),
            CancellationToken::new(),
        )
        .await;
    assert!(
        result.is_ok(),
        "handler should succeed (skip): {:?}",
        result.err()
    );

    let result_json = result.unwrap();
    assert_eq!(result_json["status"], "skipped");
    assert_eq!(result_json["reason"], "ingest_url_not_configured");
}

#[tokio::test]
async fn test_book_want_nonexistent_book() {
    let server = TestServer::start_api_only().await;

    // Configure Kobload properly so the handler doesn't skip early
    let resp = server
        .admin_client()
        .patch_json(
            "/api/v1/admin/config",
            &json!({
                "kobload_ingest_url": "http://localhost:9999/callback"
            }),
        )
        .await;
    resp.assert_ok();

    let handler = BookWantHandler::new(
        server.pool.clone(),
        server.runtime_config.clone(),
        server.nats_publisher.clone(),
        server.library_events.clone(),
    );

    // Use a random UUID that doesn't exist
    let fake_id = Uuid::now_v7();
    let result = handler
        .execute(
            Uuid::now_v7(),
            json!({ "book_id": fake_id }),
            CancellationToken::new(),
        )
        .await;
    assert!(result.is_err(), "handler should fail for nonexistent book");

    let err = result.unwrap_err();
    let err_msg = err.to_string();
    assert!(
        err_msg.contains("not found") || err_msg.contains("Book not found"),
        "Error should mention book not found, got: {err_msg}"
    );
}

#[tokio::test]
async fn test_book_want_broadcasts_library_updated() {
    let server = TestServer::start_api_only().await;
    let wanted_id = setup_wanted_book(&server).await;

    // Subscribe BEFORE executing so we catch the broadcast
    let mut rx = server.library_events.subscribe();

    let handler = BookWantHandler::new(
        server.pool.clone(),
        server.runtime_config.clone(),
        server.nats_publisher.clone(),
        server.library_events.clone(),
    );

    let result = handler
        .execute(
            Uuid::now_v7(),
            json!({ "book_id": wanted_id }),
            CancellationToken::new(),
        )
        .await;
    assert!(result.is_ok(), "handler should succeed: {:?}", result.err());

    // Verify library_updated event was broadcast with the correct book_id
    let event = tokio::time::timeout(std::time::Duration::from_secs(2), rx.recv())
        .await
        .expect("should receive event within timeout")
        .expect("channel should not be closed");

    match event {
        LibraryEvent::LibraryUpdated { book_id } => {
            let expected_id: Uuid = wanted_id.parse().unwrap();
            assert_eq!(
                book_id,
                Some(expected_id),
                "library_updated should contain the wanted book's ID"
            );
        }
        other => panic!("Expected LibraryUpdated event, got: {}", other.kind()),
    }
}
