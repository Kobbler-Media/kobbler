use crate::common::*;
use reqwest::StatusCode;
use serde_json::Value;
use uuid::Uuid;

// =============================================================================
// Reading Data Tests
// =============================================================================

#[tokio::test]
async fn test_get_reading_data_requires_auth() {
    let server = TestServer::start_api_only().await;
    let book_id = create_book(&server, "Test Book").await;
    let client = TestClient::new(&server.api_url);

    client
        .get(&format!("/api/v1/admin/books/{}/reading", book_id))
        .await
        .assert_status(StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn test_get_reading_data_book_not_found() {
    let server = TestServer::start_api_only().await;
    let fake_id = uuid::Uuid::new_v4();

    let resp = server
        .admin_client()
        .get(&format!("/api/v1/admin/books/{}/reading", fake_id))
        .await;
    resp.assert_status(StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn test_get_reading_data_empty_state() {
    let server = TestServer::start_api_only().await;
    let book_id = create_book(&server, "Test Book").await;

    let resp = server
        .admin_client()
        .get(&format!("/api/v1/admin/books/{}/reading", book_id))
        .await;
    resp.assert_ok();

    let json: Value = resp.json();
    // Should return data with null/default values since no reading activity yet
    assert!(json["status"].is_null() || json["status"].as_str() == Some("ReadyToRead"));
    assert!(json["progress_percent"].is_null());
    assert!(json["rating"].is_null());
}

#[tokio::test]
async fn test_update_reading_data_requires_auth() {
    let server = TestServer::start_api_only().await;
    let book_id = create_book(&server, "Test Book").await;
    let client = TestClient::new(&server.api_url);

    client
        .put_json(
            &format!("/api/v1/admin/books/{}/reading", book_id),
            &serde_json::json!({ "rating": 4 }),
        )
        .await
        .assert_status(StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn test_update_reading_data_book_not_found() {
    let server = TestServer::start_api_only().await;
    let fake_id = uuid::Uuid::new_v4();

    let resp = server
        .admin_client()
        .put_json(
            &format!("/api/v1/admin/books/{}/reading", fake_id),
            &serde_json::json!({ "rating": 4 }),
        )
        .await;
    resp.assert_status(StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn test_update_rating() {
    let server = TestServer::start_api_only().await;
    let book_id = create_book(&server, "Test Book").await;

    // Update rating (returns 204 No Content on success)
    let resp = server
        .admin_client()
        .put_json(
            &format!("/api/v1/admin/books/{}/reading", book_id),
            &serde_json::json!({ "rating": 4 }),
        )
        .await;
    resp.assert_status(StatusCode::NO_CONTENT);

    // Verify the rating was saved
    let resp = server
        .admin_client()
        .get(&format!("/api/v1/admin/books/{}/reading", book_id))
        .await;
    resp.assert_ok();

    let json: Value = resp.json();
    assert_eq!(json["rating"], 4);
    assert_eq!(json["rating_source"], "manual");
}

#[tokio::test]
async fn test_update_rating_validates_range() {
    let server = TestServer::start_api_only().await;
    let book_id = create_book(&server, "Test Book").await;

    // Try to set rating > 5 - should fail
    let resp = server
        .admin_client()
        .put_json(
            &format!("/api/v1/admin/books/{}/reading", book_id),
            &serde_json::json!({ "rating": 10 }),
        )
        .await;
    resp.assert_status(StatusCode::BAD_REQUEST);

    // Try to set rating < 1 (negative) - should fail
    let resp = server
        .admin_client()
        .put_json(
            &format!("/api/v1/admin/books/{}/reading", book_id),
            &serde_json::json!({ "rating": -1 }),
        )
        .await;
    resp.assert_status(StatusCode::BAD_REQUEST);

    // Rating 0 is valid - it clears the rating
    let resp = server
        .admin_client()
        .put_json(
            &format!("/api/v1/admin/books/{}/reading", book_id),
            &serde_json::json!({ "rating": 0 }),
        )
        .await;
    resp.assert_status(StatusCode::NO_CONTENT);
}

#[tokio::test]
async fn test_update_reading_status() {
    let server = TestServer::start_api_only().await;
    let book_id = create_book(&server, "Test Book").await;

    // Set status to reading (returns 204 No Content on success)
    let resp = server
        .admin_client()
        .put_json(
            &format!("/api/v1/admin/books/{}/reading", book_id),
            &serde_json::json!({ "status": "reading" }),
        )
        .await;
    resp.assert_status(StatusCode::NO_CONTENT);

    // Verify status
    let resp = server
        .admin_client()
        .get(&format!("/api/v1/admin/books/{}/reading", book_id))
        .await;
    resp.assert_ok();

    let json: Value = resp.json();
    assert_eq!(json["status"], "reading");
}

#[tokio::test]
async fn test_update_progress() {
    let server = TestServer::start_api_only().await;
    let book_id = create_book(&server, "Test Book").await;

    // Set progress — auto-creates a reading state via the web reader device
    let resp = server
        .admin_client()
        .put_json(
            &format!("/api/v1/admin/books/{}/reading", book_id),
            &serde_json::json!({ "progress_percent": 45.5 }),
        )
        .await;
    resp.assert_status(StatusCode::NO_CONTENT);

    // Verify progress was saved
    let resp = server
        .admin_client()
        .get(&format!("/api/v1/admin/books/{}/reading", book_id))
        .await;
    resp.assert_ok();

    let json: Value = resp.json();
    let progress = json["progress_percent"].as_f64().unwrap();
    assert!((progress - 45.5).abs() < 0.01);
}

#[tokio::test]
async fn test_update_progress_incremental() {
    let server = TestServer::start_api_only().await;
    let book_id = create_book(&server, "Test Book").await;

    // First progress update creates the reading state
    server
        .admin_client()
        .put_json(
            &format!("/api/v1/admin/books/{}/reading", book_id),
            &serde_json::json!({ "progress_percent": 25.0 }),
        )
        .await
        .assert_status(StatusCode::NO_CONTENT);

    // Second update reuses the existing reading state
    server
        .admin_client()
        .put_json(
            &format!("/api/v1/admin/books/{}/reading", book_id),
            &serde_json::json!({ "progress_percent": 75.0 }),
        )
        .await
        .assert_status(StatusCode::NO_CONTENT);

    let resp = server
        .admin_client()
        .get(&format!("/api/v1/admin/books/{}/reading", book_id))
        .await;
    resp.assert_ok();

    let json: Value = resp.json();
    let progress = json["progress_percent"].as_f64().unwrap();
    assert!((progress - 75.0).abs() < 0.01);
}

#[tokio::test]
async fn test_update_multiple_fields() {
    let server = TestServer::start_api_only().await;
    let book_id = create_book(&server, "Test Book").await;

    // Update multiple fields at once (returns 204 No Content on success)
    let resp = server
        .admin_client()
        .put_json(
            &format!("/api/v1/admin/books/{}/reading", book_id),
            &serde_json::json!({
                "status": "reading",
                "rating": 5
            }),
        )
        .await;
    resp.assert_status(StatusCode::NO_CONTENT);

    // Verify status and rating (these persist to user_book_data)
    let resp = server
        .admin_client()
        .get(&format!("/api/v1/admin/books/{}/reading", book_id))
        .await;
    resp.assert_ok();

    let json: Value = resp.json();
    assert_eq!(json["status"], "reading");
    assert_eq!(json["rating"], 5);
}

#[tokio::test]
async fn test_log_reading_time_requires_auth() {
    let server = TestServer::start_api_only().await;
    let book_id = create_book(&server, "Test Book").await;
    let client = TestClient::new(&server.api_url);

    client
        .post_json(
            &format!("/api/v1/admin/books/{}/reading/log-time", book_id),
            &serde_json::json!({ "minutes": 30 }),
        )
        .await
        .assert_status(StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn test_log_reading_time_book_not_found() {
    let server = TestServer::start_api_only().await;
    let fake_id = uuid::Uuid::new_v4();

    let resp = server
        .admin_client()
        .post_json(
            &format!("/api/v1/admin/books/{}/reading/log-time", fake_id),
            &serde_json::json!({ "minutes": 30 }),
        )
        .await;
    resp.assert_status(StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn test_log_reading_time() {
    let server = TestServer::start_api_only().await;
    let book_id = create_book(&server, "Test Book").await;

    // Log some reading time (returns 204 No Content on success)
    // Note: Without a device reading state, time is logged but won't accumulate
    // This endpoint is primarily for Hardcover sync (it queues a sync job)
    let resp = server
        .admin_client()
        .post_json(
            &format!("/api/v1/admin/books/{}/reading/log-time", book_id),
            &serde_json::json!({ "minutes": 30 }),
        )
        .await;
    resp.assert_status(StatusCode::NO_CONTENT);

    // Verify endpoint accepts the request (actual accumulation requires device sync)
    let resp = server
        .admin_client()
        .get(&format!("/api/v1/admin/books/{}/reading", book_id))
        .await;
    resp.assert_ok();

    // spent_reading_minutes will be null without device reading state
    let json: Value = resp.json();
    assert!(json["spent_reading_minutes"].is_null());
}

#[tokio::test]
async fn test_log_reading_time_requires_positive_minutes() {
    let server = TestServer::start_api_only().await;
    let book_id = create_book(&server, "Test Book").await;

    // Try to log 0 minutes
    let resp = server
        .admin_client()
        .post_json(
            &format!("/api/v1/admin/books/{}/reading/log-time", book_id),
            &serde_json::json!({ "minutes": 0 }),
        )
        .await;
    resp.assert_status(StatusCode::BAD_REQUEST);

    // Try to log negative minutes
    let resp = server
        .admin_client()
        .post_json(
            &format!("/api/v1/admin/books/{}/reading/log-time", book_id),
            &serde_json::json!({ "minutes": -10 }),
        )
        .await;
    resp.assert_status(StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn test_reading_data_includes_community_rating() {
    let server = TestServer::start_api_only().await;

    // Create a book with community rating via ingest API
    let book_id = uuid::Uuid::new_v4();
    let resp = server
        .ingest_client()
        .put_json(
            &format!("/api/v1/books/{}", book_id),
            &serde_json::json!({
                "title": "Rated Book",
                "authors": ["Test Author"],
                "rating": 4.2,
                "rating_count": 1500
            }),
        )
        .await;
    resp.assert_success();

    // Get reading data
    let resp = server
        .admin_client()
        .get(&format!("/api/v1/admin/books/{}/reading", book_id))
        .await;
    resp.assert_ok();

    let json: Value = resp.json();
    // Should include community rating from book metadata
    let community_rating = json["community_rating"].as_f64().unwrap();
    assert!((community_rating - 4.2).abs() < 0.01);
    assert_eq!(json["community_rating_count"], 1500);
}

#[tokio::test]
async fn test_reading_data_includes_page_count() {
    let server = TestServer::start_api_only().await;

    // Create a book with page count
    let book_id = uuid::Uuid::new_v4();
    let resp = server
        .ingest_client()
        .put_json(
            &format!("/api/v1/books/{}", book_id),
            &serde_json::json!({
                "title": "Long Book",
                "authors": ["Test Author"],
                "page_count": 500
            }),
        )
        .await;
    resp.assert_success();

    // Get reading data
    let resp = server
        .admin_client()
        .get(&format!("/api/v1/admin/books/{}/reading", book_id))
        .await;
    resp.assert_ok();

    let json: Value = resp.json();
    assert_eq!(json["page_count"], 500);
}

// =============================================================================
// Pull Sync Progress Tests
// =============================================================================

/// Helper: get the admin user's UUID.
async fn admin_user_id(server: &TestServer) -> Uuid {
    let resp = server.admin_client().get("/api/v1/admin/me").await;
    let json: Value = resp.json();
    json["user_id"].as_str().unwrap().parse::<Uuid>().unwrap()
}

/// Simulate what pull sync does: store progress in user_book_data + reading_state.
/// This mirrors the logic in SyncPullHandler::sync_user().
async fn simulate_pull_sync_progress(
    pool: &sqlx::PgPool,
    user_id: Uuid,
    book_id: Uuid,
    progress_percent: f64,
    status: Option<&str>,
) {
    use api::repositories::{devices, reading_states, user_book_data};

    // 1. Update user_book_data (status/rating) — same as real pull sync
    let _ = user_book_data::update_from_sync_provider(
        pool,
        user_id,
        book_id,
        None,
        status,
        true, // progress_changed
        None,
        "hardcover",
    )
    .await
    .expect("update_from_sync_provider");

    // 2. Store progress in reading_state — same as the new pull sync code
    let existing = reading_states::get_aggregated_for_user_book(pool, user_id, book_id)
        .await
        .ok()
        .flatten();

    let should_update = match &existing {
        Some(agg) => agg.progress_percent.unwrap_or(0.0) < progress_percent,
        None => true,
    };

    if should_update {
        let device_id = devices::get_or_create_web_reader(pool, user_id)
            .await
            .expect("get_or_create_web_reader");

        let state_id =
            existing
                .map(|agg| agg.reading_state_id)
                .or(
                    reading_states::upsert_with_children(pool, user_id, device_id, book_id)
                        .await
                        .expect("upsert_with_children"),
                );

        if let Some(state_id) = state_id {
            reading_states::update_bookmark(
                pool,
                state_id,
                Some(progress_percent),
                None,
                None,
                None,
                None,
            )
            .await
            .expect("update_bookmark");
            reading_states::touch(pool, state_id).await.expect("touch");
        }
    }
}

#[tokio::test]
async fn test_pull_sync_creates_reading_state_with_progress() {
    let server = TestServer::start_api_only().await;
    let book_id = create_book(&server, "Hardcover Synced Book").await;
    let user_id = admin_user_id(&server).await;

    // Simulate Hardcover pull sync with 60% progress
    simulate_pull_sync_progress(&server.pool, user_id, book_id, 60.0, Some("reading")).await;

    // Verify progress is visible via the reading data endpoint
    let resp = server
        .admin_client()
        .get(&format!("/api/v1/admin/books/{}/reading", book_id))
        .await;
    resp.assert_ok();

    let json: Value = resp.json();
    let progress = json["progress_percent"].as_f64().unwrap();
    assert!(
        (progress - 60.0).abs() < 0.01,
        "expected 60%, got {progress}"
    );
    assert_eq!(json["status"], "reading");
}

#[tokio::test]
async fn test_pull_sync_does_not_regress_local_progress() {
    let server = TestServer::start_api_only().await;
    let book_id = create_book(&server, "Locally Read Book").await;
    let user_id = admin_user_id(&server).await;

    // User reads locally to 80%
    server
        .admin_client()
        .put_json(
            &format!("/api/v1/admin/books/{}/reading", book_id),
            &serde_json::json!({ "progress_percent": 80.0 }),
        )
        .await
        .assert_status(StatusCode::NO_CONTENT);

    // Hardcover pull sync comes in with older 40% progress
    simulate_pull_sync_progress(&server.pool, user_id, book_id, 40.0, Some("reading")).await;

    // Local progress should still be 80%
    let resp = server
        .admin_client()
        .get(&format!("/api/v1/admin/books/{}/reading", book_id))
        .await;
    resp.assert_ok();

    let json: Value = resp.json();
    let progress = json["progress_percent"].as_f64().unwrap();
    assert!(
        (progress - 80.0).abs() < 0.01,
        "expected 80%, got {progress}"
    );
}

#[tokio::test]
async fn test_pull_sync_updates_when_provider_progress_higher() {
    let server = TestServer::start_api_only().await;
    let book_id = create_book(&server, "Multi-device Book").await;
    let user_id = admin_user_id(&server).await;

    // First sync: 30%
    simulate_pull_sync_progress(&server.pool, user_id, book_id, 30.0, Some("reading")).await;

    // Second sync: 70% (user read more on another device synced via Hardcover)
    simulate_pull_sync_progress(&server.pool, user_id, book_id, 70.0, Some("reading")).await;

    let resp = server
        .admin_client()
        .get(&format!("/api/v1/admin/books/{}/reading", book_id))
        .await;
    resp.assert_ok();

    let json: Value = resp.json();
    let progress = json["progress_percent"].as_f64().unwrap();
    assert!(
        (progress - 70.0).abs() < 0.01,
        "expected 70%, got {progress}"
    );
}
