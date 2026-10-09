use crate::common::*;
use reqwest::StatusCode;
use serde_json::json;

#[tokio::test]
async fn test_recents_requires_auth() {
    let server = TestServer::start_api_only().await;

    let resp = server.api_client().get("/api/v1/admin/recents").await;
    resp.assert_status(StatusCode::UNAUTHORIZED);

    let resp = server
        .api_client()
        .post_json(
            "/api/v1/admin/recents",
            &json!({"entity_type": "page", "entity_id": "dashboard"}),
        )
        .await;
    resp.assert_status(StatusCode::UNAUTHORIZED);

    let resp = server.api_client().delete("/api/v1/admin/recents").await;
    resp.assert_status(StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn test_recents_empty_on_start() {
    let server = TestServer::start_api_only().await;

    let resp = server.admin_client().get("/api/v1/admin/recents").await;
    resp.assert_ok();
    let items: Vec<serde_json::Value> = resp.json();
    assert!(items.is_empty());
}

#[tokio::test]
async fn test_recents_round_trip() {
    let server = TestServer::start_api_only().await;

    // Record a page access
    let resp = server
        .admin_client()
        .post_json(
            "/api/v1/admin/recents",
            &json!({"entity_type": "page", "entity_id": "dashboard"}),
        )
        .await;
    resp.assert_status(StatusCode::NO_CONTENT);

    // Verify it shows up
    let resp = server.admin_client().get("/api/v1/admin/recents").await;
    resp.assert_ok();
    let items: Vec<serde_json::Value> = resp.json();
    assert_eq!(items.len(), 1);
    assert_eq!(items[0]["entity_type"], "page");
    assert_eq!(items[0]["entity_id"], "dashboard");
    assert_eq!(items[0]["access_count"], 1);
    // Static types should have null label (resolved by frontend)
    assert!(items[0]["label"].is_null());
}

#[tokio::test]
async fn test_recents_book_has_live_label() {
    let server = TestServer::start_api_only().await;

    // Create a book
    let book_id = create_book(&server, "The Great Gatsby").await;

    // Record access to the book
    let resp = server
        .admin_client()
        .post_json(
            "/api/v1/admin/recents",
            &json!({"entity_type": "book", "entity_id": book_id.to_string()}),
        )
        .await;
    resp.assert_status(StatusCode::NO_CONTENT);

    // Should show up with live title
    let resp = server.admin_client().get("/api/v1/admin/recents").await;
    resp.assert_ok();
    let items: Vec<serde_json::Value> = resp.json();
    assert_eq!(items.len(), 1);
    assert_eq!(items[0]["label"], "The Great Gatsby");
}

#[tokio::test]
async fn test_recents_stale_book_filtered() {
    let server = TestServer::start_api_only().await;

    // Record access to a non-existent book
    let fake_id = uuid::Uuid::new_v4();
    let resp = server
        .admin_client()
        .post_json(
            "/api/v1/admin/recents",
            &json!({"entity_type": "book", "entity_id": fake_id.to_string()}),
        )
        .await;
    resp.assert_status(StatusCode::NO_CONTENT);

    // Should be filtered out
    let resp = server.admin_client().get("/api/v1/admin/recents").await;
    resp.assert_ok();
    let items: Vec<serde_json::Value> = resp.json();
    assert!(items.is_empty());
}

#[tokio::test]
async fn test_recents_frecency_order() {
    let server = TestServer::start_api_only().await;

    // Record page A once
    server
        .admin_client()
        .post_json(
            "/api/v1/admin/recents",
            &json!({"entity_type": "page", "entity_id": "a"}),
        )
        .await;

    // Record page B three times to boost its frecency
    for _ in 0..3 {
        server
            .admin_client()
            .post_json(
                "/api/v1/admin/recents",
                &json!({"entity_type": "page", "entity_id": "b"}),
            )
            .await;
    }

    let resp = server.admin_client().get("/api/v1/admin/recents").await;
    resp.assert_ok();
    let items: Vec<serde_json::Value> = resp.json();
    assert_eq!(items.len(), 2);
    // B should be first (higher access count = higher frecency)
    assert_eq!(items[0]["entity_id"], "b");
    assert_eq!(items[0]["access_count"], 3);
    assert_eq!(items[1]["entity_id"], "a");
    assert_eq!(items[1]["access_count"], 1);
}

#[tokio::test]
async fn test_recents_clear() {
    let server = TestServer::start_api_only().await;

    // Record a few items
    for id in ["a", "b", "c"] {
        server
            .admin_client()
            .post_json(
                "/api/v1/admin/recents",
                &json!({"entity_type": "page", "entity_id": id}),
            )
            .await;
    }

    // Verify they exist
    let resp = server.admin_client().get("/api/v1/admin/recents").await;
    let items: Vec<serde_json::Value> = resp.json();
    assert_eq!(items.len(), 3);

    // Clear all
    let resp = server.admin_client().delete("/api/v1/admin/recents").await;
    resp.assert_status(StatusCode::NO_CONTENT);

    // Verify empty
    let resp = server.admin_client().get("/api/v1/admin/recents").await;
    let items: Vec<serde_json::Value> = resp.json();
    assert!(items.is_empty());
}

#[tokio::test]
async fn test_recents_upsert_increments_count() {
    let server = TestServer::start_api_only().await;

    // Record same item twice
    for _ in 0..2 {
        server
            .admin_client()
            .post_json(
                "/api/v1/admin/recents",
                &json!({"entity_type": "command", "entity_id": "toggle-theme"}),
            )
            .await;
    }

    let resp = server.admin_client().get("/api/v1/admin/recents").await;
    resp.assert_ok();
    let items: Vec<serde_json::Value> = resp.json();
    assert_eq!(items.len(), 1);
    assert_eq!(items[0]["access_count"], 2);
}
