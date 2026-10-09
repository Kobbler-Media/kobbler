//! Tests for pending sync status filters in the library UI.
//!
//! These tests verify that:
//! 1. Books with DEVICE scope entitlements (not just ALL_USERS) are correctly identified as synced
//! 2. The "pending" filter shows books modified after last device sync (pending_sync=TRUE)
//! 3. The "synced" filter shows books NOT modified since last sync (pending_sync=FALSE)
//! 4. User-edited books preserve their customizations through background job runs

use crate::common::*;
use serde_json::Value;
use uuid::Uuid;

/// Helper: create a book with DEVICE entitlement (not ALL_USERS).
async fn create_book_with_device_entitlement(
    server: &TestServer,
    title: &str,
    device_id: Uuid,
) -> Uuid {
    let book_id = create_book(server, title).await;

    // Add DEVICE scope entitlement
    let resp = server
        .admin_client()
        .post(&format!(
            "/api/v1/admin/books/{}/devices/{}",
            book_id, device_id
        ))
        .await;
    resp.assert_status(reqwest::StatusCode::NO_CONTENT);

    book_id
}

/// Helper: create a book with ALL_USERS entitlement.
async fn create_book_with_all_users_entitlement(server: &TestServer, title: &str) -> Uuid {
    let book_id = create_book(server, title).await;

    let resp = server
        .admin_client()
        .post_json(
            &format!("/api/v1/admin/books/{}/devices/all", book_id),
            &serde_json::json!({}),
        )
        .await;
    resp.assert_status(reqwest::StatusCode::NO_CONTENT);

    book_id
}

/// The user the shared `admin_client()` authenticates as. Sync state is per-user,
/// so a device has to belong to the caller to answer for their library.
async fn admin_user_id(server: &TestServer) -> Uuid {
    sqlx::query_scalar("SELECT id FROM users WHERE email = 'admin@test.local'")
        .fetch_one(&server.pool)
        .await
        .expect("bootstrap admin user should exist")
}

/// Helper: get book from list by ID.
fn find_book_in_items(items: &[Value], book_id: Uuid) -> Option<&Value> {
    items
        .iter()
        .find(|b| b["id"].as_str().is_some_and(|id| id == book_id.to_string()))
}

/// Helper: list books with filter.
async fn list_books_with_filter(server: &TestServer, filter: &str) -> Vec<Value> {
    let resp = server
        .admin_client()
        .get(&format!("/api/v1/library/books?filter={}", filter))
        .await;
    resp.assert_ok();
    let json: Value = resp.json();
    json["items"].as_array().cloned().unwrap_or_default()
}

// =============================================================================
// Bug Fix Tests: Entitlement Scope Support
// =============================================================================

#[tokio::test]
async fn test_device_entitlement_shows_as_synced() {
    // Bug 2 fix: Books with DEVICE scope entitlements should show as "synced"
    let server = TestServer::start_api_only().await;
    let (device_id, _) = create_device(&server, admin_user_id(&server).await).await;

    // Create book with DEVICE entitlement (not ALL_USERS)
    let book_id =
        create_book_with_device_entitlement(&server, "Device Entitled Book", device_id).await;

    // Should appear in "synced" filter (it has an active entitlement)
    let synced = list_books_with_filter(&server, "synced").await;
    assert!(
        find_book_in_items(&synced, book_id).is_some(),
        "Book with DEVICE entitlement should appear in synced filter"
    );

    // Should NOT appear in "pending" filter (it's synced, not pending)
    let pending = list_books_with_filter(&server, "pending").await;
    assert!(
        find_book_in_items(&pending, book_id).is_none(),
        "Book with DEVICE entitlement should NOT appear in pending filter (unless modified)"
    );

    // Should appear in "all" filter
    let all = list_books_with_filter(&server, "all").await;
    assert!(
        find_book_in_items(&all, book_id).is_some(),
        "Book should appear in all filter"
    );
}

#[tokio::test]
async fn test_all_users_entitlement_shows_as_synced() {
    let server = TestServer::start_api_only().await;

    let book_id = create_book_with_all_users_entitlement(&server, "All Users Book").await;

    // Should appear in "synced" filter
    let synced = list_books_with_filter(&server, "synced").await;
    assert!(
        find_book_in_items(&synced, book_id).is_some(),
        "Book with ALL_USERS entitlement should appear in synced filter"
    );
}

#[tokio::test]
async fn test_book_without_entitlement_not_in_synced() {
    let server = TestServer::start_api_only().await;

    // Create book without any entitlement
    let book_id = create_book(&server, "Unentitled Book").await;

    // Should NOT appear in "synced" filter (no entitlement)
    let synced = list_books_with_filter(&server, "synced").await;
    assert!(
        find_book_in_items(&synced, book_id).is_none(),
        "Book without entitlement should NOT appear in synced filter"
    );

    // Should NOT appear in "pending" filter (pending is for entitled books with changes)
    let pending = list_books_with_filter(&server, "pending").await;
    assert!(
        find_book_in_items(&pending, book_id).is_none(),
        "Book without entitlement should NOT appear in pending filter"
    );

    // Should appear in "all" filter
    let all = list_books_with_filter(&server, "all").await;
    assert!(
        find_book_in_items(&all, book_id).is_some(),
        "Book should appear in all filter"
    );
}

// =============================================================================
// Bug Fix Tests: Pending Sync Logic
// =============================================================================

#[tokio::test]
async fn test_modified_book_shows_as_pending() {
    // Bug 1 fix: "pending" filter should show books modified after last sync
    let server = TestServer::start_api_only().await;
    let (device_id, _) = create_device(&server, admin_user_id(&server).await).await;

    // Create book with entitlement
    let book_id = create_book_with_device_entitlement(&server, "Modified Book", device_id).await;

    // Simulate device sync by setting last_sync_at in the past
    // Then modify the book to trigger pending_sync
    sqlx::query(
        "INSERT INTO device_sync_state (device_id, last_sync_at) VALUES ($1, NOW() - INTERVAL '1 hour')
         ON CONFLICT (device_id) DO UPDATE SET last_sync_at = NOW() - INTERVAL '1 hour'",
    )
    .bind(device_id)
    .execute(&server.pool)
    .await
    .unwrap();

    // Touch the book to set updated_at to NOW
    let resp = server
        .admin_client()
        .post_json(
            "/api/v1/library/books/touch",
            &serde_json::json!({ "book_ids": [book_id] }),
        )
        .await;
    resp.assert_status(reqwest::StatusCode::NO_CONTENT);

    // Book should now show as pending (updated after sync)
    let pending = list_books_with_filter(&server, "pending").await;
    assert!(
        find_book_in_items(&pending, book_id).is_some(),
        "Book modified after last sync should appear in pending filter"
    );
}

// =============================================================================
// User Edited Protection Tests
// =============================================================================

#[tokio::test]
async fn test_user_edit_sets_user_edited_at() {
    let server = TestServer::start_api_only().await;

    let book_id = create_book(&server, "User Edit Test").await;

    // Edit the book via API
    let resp = server
        .admin_client()
        .patch_json(
            &format!("/api/v1/library/books/{}", book_id),
            &serde_json::json!({ "title": "User Edited Title" }),
        )
        .await;
    resp.assert_status(reqwest::StatusCode::NO_CONTENT);

    // Check that user_edited_at is set
    let row: Option<time::OffsetDateTime> =
        sqlx::query_scalar("SELECT user_edited_at FROM books WHERE id = $1")
            .bind(book_id)
            .fetch_optional(&server.pool)
            .await
            .unwrap();

    assert!(
        row.is_some(),
        "user_edited_at should be set after user edits a book"
    );
}

#[tokio::test]
async fn test_mark_user_edited_preserves_through_book_update() {
    let server = TestServer::start_api_only().await;

    let book_id = create_book(&server, "Preserved Edit Test").await;

    // Manually set user_edited_at
    sqlx::query("UPDATE books SET user_edited_at = NOW() WHERE id = $1")
        .bind(book_id)
        .execute(&server.pool)
        .await
        .unwrap();

    // Do an update that would come from a background job (using repository directly)
    let update = api::repositories::books::BookUpdate {
        description: Some("New description from background job".to_string()),
        ..Default::default()
    };
    api::repositories::books::update(&server.pool, book_id, update)
        .await
        .unwrap();

    // user_edited_at should still be set (background jobs check this before updating)
    let row: Option<time::OffsetDateTime> =
        sqlx::query_scalar("SELECT user_edited_at FROM books WHERE id = $1")
            .bind(book_id)
            .fetch_optional(&server.pool)
            .await
            .unwrap();

    assert!(
        row.is_some(),
        "user_edited_at should persist through book updates"
    );
}

// =============================================================================
// Conditional updated_at Tests
// =============================================================================

#[tokio::test]
async fn test_update_with_no_changes_preserves_updated_at() {
    let server = TestServer::start_api_only().await;

    let book_id = create_book(&server, "No Change Test").await;

    // Get original updated_at
    let original: time::OffsetDateTime =
        sqlx::query_scalar("SELECT updated_at FROM books WHERE id = $1")
            .bind(book_id)
            .fetch_one(&server.pool)
            .await
            .unwrap();

    // Small delay to ensure timestamp would be different if updated
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;

    // Update with empty/null values (no actual changes)
    let update = api::repositories::books::BookUpdate::default();
    api::repositories::books::update(&server.pool, book_id, update)
        .await
        .unwrap();

    // Get new updated_at
    let after: time::OffsetDateTime =
        sqlx::query_scalar("SELECT updated_at FROM books WHERE id = $1")
            .bind(book_id)
            .fetch_one(&server.pool)
            .await
            .unwrap();

    assert_eq!(
        original, after,
        "updated_at should NOT change when no values change"
    );
}

#[tokio::test]
async fn test_update_with_actual_changes_bumps_updated_at() {
    let server = TestServer::start_api_only().await;

    let book_id = create_book(&server, "Change Test").await;

    // Get original updated_at
    let original: time::OffsetDateTime =
        sqlx::query_scalar("SELECT updated_at FROM books WHERE id = $1")
            .bind(book_id)
            .fetch_one(&server.pool)
            .await
            .unwrap();

    // Small delay to ensure timestamp would be different if updated
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;

    // Update with actual changes
    let update = api::repositories::books::BookUpdate {
        title: Some("Changed Title".to_string()),
        ..Default::default()
    };
    api::repositories::books::update(&server.pool, book_id, update)
        .await
        .unwrap();

    // Get new updated_at
    let after: time::OffsetDateTime =
        sqlx::query_scalar("SELECT updated_at FROM books WHERE id = $1")
            .bind(book_id)
            .fetch_one(&server.pool)
            .await
            .unwrap();

    assert!(
        after > original,
        "updated_at SHOULD change when values actually change"
    );
}

#[tokio::test]
async fn test_update_with_same_value_preserves_updated_at() {
    let server = TestServer::start_api_only().await;

    // Create book with specific title
    let book_id = create_book(&server, "Same Value Test").await;

    // Get original updated_at
    let original: time::OffsetDateTime =
        sqlx::query_scalar("SELECT updated_at FROM books WHERE id = $1")
            .bind(book_id)
            .fetch_one(&server.pool)
            .await
            .unwrap();

    // Small delay
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;

    // Update with same title value (no actual change)
    let update = api::repositories::books::BookUpdate {
        title: Some("Same Value Test".to_string()), // Same as original
        ..Default::default()
    };
    api::repositories::books::update(&server.pool, book_id, update)
        .await
        .unwrap();

    // Get new updated_at
    let after: time::OffsetDateTime =
        sqlx::query_scalar("SELECT updated_at FROM books WHERE id = $1")
            .bind(book_id)
            .fetch_one(&server.pool)
            .await
            .unwrap();

    assert_eq!(
        original, after,
        "updated_at should NOT change when value is same as before"
    );
}

#[tokio::test]
async fn test_rating_change_does_not_bump_updated_at() {
    let server = TestServer::start_api_only().await;

    let book_id = create_book(&server, "Rating Drift Test").await;

    // Set an initial rating
    let update = api::repositories::books::BookUpdate {
        rating: Some(4.25),
        rating_count: Some(100),
        ..Default::default()
    };
    api::repositories::books::update(&server.pool, book_id, update)
        .await
        .unwrap();

    // Get updated_at after setting initial rating
    let original: time::OffsetDateTime =
        sqlx::query_scalar("SELECT updated_at FROM books WHERE id = $1")
            .bind(book_id)
            .fetch_one(&server.pool)
            .await
            .unwrap();

    tokio::time::sleep(std::time::Duration::from_millis(50)).await;

    // Update with a slightly different rating (simulates Hardcover float drift)
    let update = api::repositories::books::BookUpdate {
        rating: Some(4.2500001),
        rating_count: Some(101),
        ..Default::default()
    };
    api::repositories::books::update(&server.pool, book_id, update)
        .await
        .unwrap();

    let after: time::OffsetDateTime =
        sqlx::query_scalar("SELECT updated_at FROM books WHERE id = $1")
            .bind(book_id)
            .fetch_one(&server.pool)
            .await
            .unwrap();

    assert_eq!(
        original, after,
        "updated_at should NOT change when only rating/rating_count change (cosmetic metadata)"
    );

    // But values should still be updated
    let (rating, count): (Option<f64>, Option<i32>) =
        sqlx::query_as("SELECT rating, rating_count FROM books WHERE id = $1")
            .bind(book_id)
            .fetch_one(&server.pool)
            .await
            .unwrap();

    assert!(
        (rating.unwrap() - 4.2500001).abs() < 0.0001,
        "Rating value should be updated even though updated_at wasn't bumped"
    );
    assert_eq!(count.unwrap(), 101, "Rating count should be updated");
}

#[tokio::test]
async fn test_title_change_with_rating_bumps_updated_at() {
    let server = TestServer::start_api_only().await;

    let book_id = create_book(&server, "Title Plus Rating Test").await;

    let original: time::OffsetDateTime =
        sqlx::query_scalar("SELECT updated_at FROM books WHERE id = $1")
            .bind(book_id)
            .fetch_one(&server.pool)
            .await
            .unwrap();

    tokio::time::sleep(std::time::Duration::from_millis(50)).await;

    // Update with both a rating change AND a title change
    let update = api::repositories::books::BookUpdate {
        title: Some("Changed Title".to_string()),
        rating: Some(4.5),
        ..Default::default()
    };
    api::repositories::books::update(&server.pool, book_id, update)
        .await
        .unwrap();

    let after: time::OffsetDateTime =
        sqlx::query_scalar("SELECT updated_at FROM books WHERE id = $1")
            .bind(book_id)
            .fetch_one(&server.pool)
            .await
            .unwrap();

    assert!(
        after > original,
        "updated_at SHOULD change when title changes (even alongside rating)"
    );
}
