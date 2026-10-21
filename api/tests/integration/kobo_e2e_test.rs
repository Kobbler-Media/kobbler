//! End-to-end flow tests that verify complete user journeys.
//!
//! These tests simulate real-world usage patterns from start to finish:
//! - Device setup → book ingest → sync → read → sync again
//! - Multiple devices syncing
//! - Book updates and re-sync
//!
//! If these tests pass, you can be confident the system works end-to-end.

use crate::common::*;
use api::config::KoboMode;
use serde_json::Value;

// =============================================================================
// Complete User Journey Tests
// =============================================================================

/// The full happy path: setup device, ingest book, sync, read, sync again
#[tokio::test]
async fn test_complete_user_journey_local_mode() {
    let server = TestServer::start().await;

    // === Phase 1: Admin sets up a user and device ===
    let user_id = create_user(&server, "journey@test.com").await;
    let (_, device_token) = create_device(&server, user_id).await;

    // === Phase 2: Ingest a book with cover and file ===
    let book_id = create_book(&server, "Journey Book").await;
    upload_cover(&server, book_id, test_jpeg_image()).await;
    upload_epub(&server, book_id, test_epub_bytes()).await;
    entitle_book(&server, book_id).await;

    // === Phase 3: First sync - book appears ===
    let resp = server.kobo_client().kobo_sync(&device_token).await;
    resp.assert_ok();
    let token = resp.get_sync_token().expect("should have sync token");
    let items: Vec<Value> = resp.json();

    assert_sync_contains_book(&items, book_id);
    assert_sync_book_title(&items, book_id, "Journey Book");

    // === Phase 4: Second sync with token - book does NOT appear again ===
    let resp = server
        .kobo_client()
        .kobo_sync_with_token(&device_token, &token)
        .await;
    resp.assert_ok();
    let token = resp.get_sync_token().expect("should have sync token");
    let items: Vec<Value> = resp.json();

    assert_sync_excludes_book(&items, book_id);

    // === Phase 5: User reads the book, updates progress ===
    server
        .kobo_client()
        .kobo_update_state(
            &device_token,
            &book_id.to_string(),
            &serde_json::json!({
                "ReadingStates": [{
                    "CurrentBookmark": {
                        "ProgressPercent": 50.0,
                        "ContentSourceProgressPercent": 50.0
                    },
                    "StatusInfo": {
                        "Status": "Reading"
                    }
                }]
            }),
        )
        .await
        .assert_ok();

    // === Phase 6: Verify reading state is persisted ===
    let resp = server
        .kobo_client()
        .kobo_get_state(&device_token, &book_id.to_string())
        .await;
    let states: Vec<Value> = resp.json();
    assert_reading_state_progress(&states[0], 50.0);
    assert_reading_state_status(&states[0], "Reading");

    // === Phase 7: Admin touches book to trigger re-sync ===
    touch_book(&server, book_id).await;

    // === Phase 8: Third sync with token - book appears again (was touched) ===
    let resp = server
        .kobo_client()
        .kobo_sync_with_token(&device_token, &token)
        .await;
    resp.assert_ok();
    let token = resp.get_sync_token().expect("should have sync token");
    let items: Vec<Value> = resp.json();

    assert_sync_contains_book(&items, book_id);

    // === Phase 9: Add a new book ===
    let book2_id = create_book(&server, "Second Journey Book").await;
    entitle_book(&server, book2_id).await;

    // === Phase 10: Fourth sync with token - only new book appears ===
    let resp = server
        .kobo_client()
        .kobo_sync_with_token(&device_token, &token)
        .await;
    resp.assert_ok();
    let items: Vec<Value> = resp.json();

    assert_sync_excludes_book(&items, book_id); // Already synced after touch
    assert_sync_contains_book(&items, book2_id); // New book appears

    // === Phase 11: Simulate device reset by dropping token (full resync) ===

    // === Phase 12: Sync without token - ALL books appear again ===
    let items: Vec<Value> = server
        .kobo_client()
        .kobo_sync(&device_token)
        .await
        .assert_ok()
        .json();

    assert_sync_contains_book(&items, book_id);
    assert_sync_contains_book(&items, book2_id);
}

/// Complete journey with merge mode (local + store books)
/// Note: In merge mode, the proxy forwards to real Kobo upstream for store operations.
/// This test verifies local book operations work in merge mode.
#[tokio::test]
async fn test_complete_user_journey_merge_mode() {
    // Note: KoboMock isn't used because the proxy determines upstream from request headers,
    // not from config. We can only test local book operations in merge mode without mocking DNS.
    let server = TestServer::start_with_mode(KoboMode::Merge, None).await;

    // Setup
    let user_id = create_user(&server, "mergejourney@test.com").await;
    let (_, device_token) = create_device(&server, user_id).await;

    // Add local book
    let local_book = create_book(&server, "Local Journey Book").await;
    entitle_book(&server, local_book).await;

    // User updates local book reading state - should be intercepted and work
    let resp = server
        .kobo_client()
        .kobo_update_state(
            &device_token,
            &local_book.to_string(),
            &serde_json::json!({
                "ReadingStates": [{
                    "StatusInfo": { "Status": "Reading" }
                }]
            }),
        )
        .await;
    resp.assert_ok();

    // Verify local state was saved
    let states: Vec<Value> = server
        .kobo_client()
        .kobo_get_state(&device_token, &local_book.to_string())
        .await
        .json();
    assert_reading_state_status(&states[0], "Reading");

    // Local book metadata should be intercepted
    let resp = server
        .kobo_client()
        .kobo_metadata(&device_token, &local_book.to_string())
        .await;
    resp.assert_ok();

    let metadata: Vec<Value> = resp.json();
    assert!(
        !metadata.is_empty(),
        "Metadata response should not be empty"
    );
    assert_eq!(
        metadata[0].get("Title").and_then(|t| t.as_str()),
        Some("Local Journey Book")
    );

    // Local book download should be intercepted
    upload_epub(&server, local_book, test_epub_bytes()).await;
    let resp = server
        .kobo_client()
        .kobo_download(&device_token, &local_book.to_string(), "KEPUB")
        .await;
    resp.assert_ok();
}

// =============================================================================
// Multi-Device Sync Tests
// =============================================================================

#[tokio::test]
async fn test_multi_device_sync_independence() {
    let server = TestServer::start().await;

    let user_id = create_user(&server, "multidev@test.com").await;
    let (_, device1_token) = create_device(&server, user_id).await;
    let (_, device2_token) = create_device(&server, user_id).await;

    let book_id = create_book(&server, "Multi-Device Book").await;
    entitle_book(&server, book_id).await;

    // Device 1 syncs - sees book, captures token
    let resp = server.kobo_client().kobo_sync(&device1_token).await;
    resp.assert_ok();
    let token1 = resp.get_sync_token().expect("should have sync token");
    let items1: Vec<Value> = resp.json();
    assert_sync_contains_book(&items1, book_id);

    // Device 1 syncs again with token - doesn't see book (state advanced)
    let items1b: Vec<Value> = server
        .kobo_client()
        .kobo_sync_with_token(&device1_token, &token1)
        .await
        .assert_ok()
        .json();
    assert_sync_excludes_book(&items1b, book_id);

    // Device 2 syncs for first time (no token) - SHOULD see book (independent state)
    let items2: Vec<Value> = server
        .kobo_client()
        .kobo_sync(&device2_token)
        .await
        .assert_ok()
        .json();
    assert_sync_contains_book(&items2, book_id);
}

#[tokio::test]
async fn test_multi_device_reading_state_independence() {
    let server = TestServer::start().await;

    let user_id = create_user(&server, "multistate@test.com").await;
    let (_, device1_token) = create_device(&server, user_id).await;
    let (_, device2_token) = create_device(&server, user_id).await;

    let book_id = create_book(&server, "Multi-State Book").await;
    entitle_book(&server, book_id).await;

    // Device 1 reads to 75%
    server
        .kobo_client()
        .kobo_update_state(
            &device1_token,
            &book_id.to_string(),
            &serde_json::json!({
                "ReadingStates": [{
                    "CurrentBookmark": { "ProgressPercent": 75.0 }
                }]
            }),
        )
        .await
        .assert_ok();

    // Device 1 sees 75%
    let states1: Vec<Value> = server
        .kobo_client()
        .kobo_get_state(&device1_token, &book_id.to_string())
        .await
        .json();
    assert_reading_state_progress(&states1[0], 75.0);

    // Device 2 sees fresh state (0% or default)
    let states2: Vec<Value> = server
        .kobo_client()
        .kobo_get_state(&device2_token, &book_id.to_string())
        .await
        .json();
    assert_reading_state_status(&states2[0], "ReadyToRead");
}

// =============================================================================
// Book Removal Flow Tests
// =============================================================================

#[tokio::test]
async fn test_book_removal_and_restore_flow() {
    let server = TestServer::start().await;

    let user_id = create_user(&server, "removal@test.com").await;
    let (_, device_token) = create_device(&server, user_id).await;

    let book_id = create_book(&server, "Removal Test Book").await;
    entitle_book(&server, book_id).await;

    // First sync - book appears, capture token
    let resp = server.kobo_client().kobo_sync(&device_token).await;
    resp.assert_ok();
    let token = resp.get_sync_token().expect("should have sync token");
    let items: Vec<Value> = resp.json();
    assert_sync_contains_book(&items, book_id);

    // Admin removes book
    server
        .admin_client()
        .post_json(
            &format!("/api/v1/admin/books/{}/remove", book_id),
            &serde_json::json!({}),
        )
        .await
        .assert_success();

    // Sync with token shows book as removed
    let resp = server
        .kobo_client()
        .kobo_sync_with_token(&device_token, &token)
        .await;
    resp.assert_ok();
    let token = resp.get_sync_token().expect("should have sync token");
    let items: Vec<Value> = resp.json();
    assert_book_is_removed(&items, book_id);

    // Admin restores book
    server
        .admin_client()
        .post_json(
            &format!("/api/v1/admin/books/{}/restore", book_id),
            &serde_json::json!({}),
        )
        .await
        .assert_success();

    // Sync with token shows book as available again
    let items: Vec<Value> = server
        .kobo_client()
        .kobo_sync_with_token(&device_token, &token)
        .await
        .assert_ok()
        .json();
    assert_sync_contains_book(&items, book_id);
}

// =============================================================================
// Series Sync Flow Tests
// =============================================================================

#[tokio::test]
async fn test_series_books_sync_flow() {
    let server = TestServer::start().await;

    let user_id = create_user(&server, "series@test.com").await;
    let (_, device_token) = create_device(&server, user_id).await;

    let book1 = create_book_in_series(&server, "Series Book 1", "Test Series", 1).await;
    let book2 = create_book_in_series(&server, "Series Book 2", "Test Series", 2).await;
    let book3 = create_book_in_series(&server, "Series Book 3", "Test Series", 3).await;

    entitle_book(&server, book1).await;
    entitle_book(&server, book2).await;
    entitle_book(&server, book3).await;

    let items: Vec<Value> = server
        .kobo_client()
        .kobo_sync(&device_token)
        .await
        .assert_ok()
        .json();

    assert_sync_contains_book(&items, book1);
    assert_sync_contains_book(&items, book2);
    assert_sync_contains_book(&items, book3);
}

// =============================================================================
// Tag/Shelf Flow Tests
// =============================================================================

#[tokio::test]
async fn test_tag_sync_flow() {
    let server = TestServer::start().await;

    let user_id = create_user(&server, "tagflow@test.com").await;
    let (_, device_token) = create_device(&server, user_id).await;

    let book_id = create_book(&server, "Tagged Flow Book").await;
    entitle_book(&server, book_id).await;

    // Sync to get book
    server
        .kobo_client()
        .kobo_sync(&device_token)
        .await
        .assert_ok();

    // Create tag with book
    let resp = server
        .kobo_client()
        .kobo_create_tag(
            &device_token,
            &serde_json::json!({
                "Name": "My Reading List",
                "Items": [{
                    "RevisionId": book_id.to_string(),
                    "Type": "ProductRevisionTagItem"
                }]
            }),
        )
        .await;
    resp.assert_ok();
    let tag_id: String = resp.json();

    // Sync without token to see all items including the tag
    let items: Vec<Value> = server
        .kobo_client()
        .kobo_sync(&device_token)
        .await
        .assert_ok()
        .json();

    let tag_id_uuid = uuid::Uuid::parse_str(&tag_id).unwrap();
    assert_sync_contains_tag(&items, tag_id_uuid, "My Reading List");
}

// =============================================================================
// Download Flow Tests
// =============================================================================

#[tokio::test]
async fn test_download_flow() {
    let server = TestServer::start().await;

    let user_id = create_user(&server, "download@test.com").await;
    let (_, device_token) = create_device(&server, user_id).await;

    let book_id = create_book(&server, "Download Flow Book").await;
    let epub_bytes = test_epub_bytes();
    let epub_len = epub_bytes.len();
    upload_epub(&server, book_id, epub_bytes).await;
    entitle_book(&server, book_id).await;

    let items: Vec<Value> = server
        .kobo_client()
        .kobo_sync(&device_token)
        .await
        .assert_ok()
        .json();
    assert_sync_contains_book(&items, book_id);

    let resp = server
        .kobo_client()
        .kobo_download(&device_token, &book_id.to_string(), "KEPUB")
        .await;
    resp.assert_ok();

    assert_eq!(
        resp.bytes().len(),
        epub_len,
        "Downloaded file should match uploaded size"
    );
}

// =============================================================================
// Initialization Flow Tests
// =============================================================================

#[tokio::test]
async fn test_initialization_flow_local_mode() {
    let server = TestServer::start().await;

    let user_id = create_user(&server, "initflow@test.com").await;
    let (_, device_token) = create_device(&server, user_id).await;

    let resp = server.kobo_client().kobo_init(&device_token).await;
    resp.assert_ok();

    let init: Value = resp.json();

    assert_init_has_resource(&init, "library_sync");
}
