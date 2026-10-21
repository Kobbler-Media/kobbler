//! Merge mode tests - the primary use case where local books are merged with Kobo store books.
//!
//! In merge mode:
//! - Local books appear alongside Kobo store books in sync
//! - Initialization resources are merged (our URLs injected)
//! - Local book operations (reading state, tags) are intercepted
//! - Store book operations pass through to upstream

use crate::common::*;
use api::config::KoboMode;
use serde_json::Value;

// =============================================================================
// Sync Merge Tests
// =============================================================================

#[tokio::test]
async fn test_merge_mode_local_books_appear_in_sync() {
    let kobo_mock = KoboMock::start().await;
    let server = TestServer::start_with_mode(KoboMode::Merge, None).await;

    let user_id = create_user(&server, "merge@test.com").await;
    let (device_id, device_token) = create_device(&server, user_id).await;

    // Point device at mock server
    set_device_upstream(&server, device_id, kobo_mock.url()).await;

    // Create a local book
    let local_book = create_book(&server, "Local Book").await;
    entitle_book(&server, local_book).await;

    // Sync should contain the local book
    let items: Vec<Value> = server
        .kobo_client()
        .kobo_sync(&device_token)
        .await
        .assert_ok()
        .json();

    assert_sync_contains_book(&items, local_book);
}

#[tokio::test]
async fn test_merge_mode_store_books_appear_in_sync() {
    let kobo_mock = KoboMock::start().await;
    kobo_mock.add_simple_book("store-book-123", "Store Book");

    let server = TestServer::start_with_mode(KoboMode::Merge, None).await;

    let user_id = create_user(&server, "storemerge@test.com").await;
    let (device_id, device_token) = create_device(&server, user_id).await;

    // Point device at mock server
    set_device_upstream(&server, device_id, kobo_mock.url()).await;

    // Sync should contain the store book
    let items: Vec<Value> = server
        .kobo_client()
        .kobo_sync(&device_token)
        .await
        .assert_ok()
        .json();

    assert_sync_contains_book_str(&items, "store-book-123");
}

#[tokio::test]
async fn test_merge_mode_both_local_and_store_books_appear() {
    let kobo_mock = KoboMock::start().await;
    kobo_mock.add_simple_book("store-book-456", "Store Book");

    let server = TestServer::start_with_mode(KoboMode::Merge, None).await;

    let user_id = create_user(&server, "bothmerge@test.com").await;
    let (device_id, device_token) = create_device(&server, user_id).await;

    // Point device at mock server
    set_device_upstream(&server, device_id, kobo_mock.url()).await;

    // Create local book
    let local_book = create_book(&server, "Local Book").await;
    entitle_book(&server, local_book).await;

    // Sync should contain both books
    let items: Vec<Value> = server
        .kobo_client()
        .kobo_sync(&device_token)
        .await
        .assert_ok()
        .json();

    assert_sync_contains_book(&items, local_book);
    assert_sync_contains_book_str(&items, "store-book-456");

    // Verify count
    let book_ids = get_sync_book_id_strings(&items);
    assert!(
        book_ids.contains(&local_book.to_string()),
        "Should contain local book"
    );
    assert!(
        book_ids.contains(&"store-book-456".to_string()),
        "Should contain store book"
    );
}

#[tokio::test]
async fn test_merge_mode_local_books_first_in_sync() {
    let kobo_mock = KoboMock::start().await;
    kobo_mock.add_simple_book("store-book-789", "Store Book");

    let server = TestServer::start_with_mode(KoboMode::Merge, None).await;

    let user_id = create_user(&server, "ordmerge@test.com").await;
    let (device_id, device_token) = create_device(&server, user_id).await;

    // Point device at mock server
    set_device_upstream(&server, device_id, kobo_mock.url()).await;

    // Create local book
    let local_book = create_book(&server, "Local Book").await;
    entitle_book(&server, local_book).await;

    // Sync - local books should appear before store books
    let items: Vec<Value> = server
        .kobo_client()
        .kobo_sync(&device_token)
        .await
        .assert_ok()
        .json();

    let book_ids = get_sync_book_id_strings(&items);
    let local_idx = book_ids.iter().position(|id| id == &local_book.to_string());
    let store_idx = book_ids.iter().position(|id| id == "store-book-789");

    assert!(
        local_idx < store_idx,
        "Local books should appear before store books. Order: {:?}",
        book_ids
    );
}

// =============================================================================
// Initialization Merge Tests
// =============================================================================

#[tokio::test]
async fn test_merge_mode_initialization_returns_merged_resources() {
    let kobo_mock = KoboMock::start().await;
    let server = TestServer::start_with_mode(KoboMode::Merge, None).await;

    let user_id = create_user(&server, "initmerge@test.com").await;
    let (device_id, device_token) = create_device(&server, user_id).await;

    // Point device at mock server
    set_device_upstream(&server, device_id, kobo_mock.url()).await;

    let resp = server.kobo_client().kobo_init(&device_token).await;
    resp.assert_ok();

    let init: Value = resp.json();

    // Should have Resources
    assert_init_has_resource(&init, "library_sync");
    assert_init_has_resource(&init, "library_items");
}

#[tokio::test]
async fn test_merge_mode_initialization_injects_our_urls() {
    let kobo_mock = KoboMock::start().await;
    let server = TestServer::start_with_mode(KoboMode::Merge, None).await;

    let user_id = create_user(&server, "initurl@test.com").await;
    let (device_id, device_token) = create_device(&server, user_id).await;

    // Point device at mock server
    set_device_upstream(&server, device_id, kobo_mock.url()).await;

    let resp = server.kobo_client().kobo_init(&device_token).await;
    resp.assert_ok();

    let init: Value = resp.json();

    // Our local URLs should be injected (pointing to proxy, not Kobo)
    // The exact URL depends on implementation, but it should contain our proxy base
    assert_init_has_resource(&init, "library_sync");
    assert_init_has_resource(&init, "image_host");
}

// =============================================================================
// Error Handling in Merge Mode
// =============================================================================

#[tokio::test]
async fn test_merge_mode_upstream_error_passed_through() {
    let kobo_mock = KoboMock::start().await;
    // Simulate upstream returning 401 (e.g., expired token)
    kobo_mock.set_sync_error(401);

    let server = TestServer::start_with_mode(KoboMode::Merge, None).await;

    let user_id = create_user(&server, "errmerge@test.com").await;
    let (device_id, device_token) = create_device(&server, user_id).await;

    // Point device at mock server
    set_device_upstream(&server, device_id, kobo_mock.url()).await;

    // Upstream error should be passed through to device
    let resp = server.kobo_client().kobo_sync(&device_token).await;

    // The 401 from upstream should be passed through
    assert_eq!(
        resp.status.as_u16(),
        401,
        "Upstream 401 should be passed through"
    );
}

#[tokio::test]
async fn test_merge_mode_local_books_still_work_with_empty_store() {
    let kobo_mock = KoboMock::start().await;
    // No books in store

    let server = TestServer::start_with_mode(KoboMode::Merge, None).await;

    let user_id = create_user(&server, "emptystore@test.com").await;
    let (device_id, device_token) = create_device(&server, user_id).await;

    // Point device at mock server
    set_device_upstream(&server, device_id, kobo_mock.url()).await;

    // Create local book
    let local_book = create_book(&server, "Local Only Book").await;
    entitle_book(&server, local_book).await;

    // Should still sync successfully with just local book
    let items: Vec<Value> = server
        .kobo_client()
        .kobo_sync(&device_token)
        .await
        .assert_ok()
        .json();

    assert_sync_contains_book(&items, local_book);
    assert_eq!(count_sync_books(&items), 1);
}

// =============================================================================
// Local Book Operations in Merge Mode
// =============================================================================

#[tokio::test]
async fn test_merge_mode_local_book_reading_state_intercepted() {
    let kobo_mock = KoboMock::start().await;
    let server = TestServer::start_with_mode(KoboMode::Merge, None).await;

    let user_id = create_user(&server, "localstate@test.com").await;
    let (device_id, device_token) = create_device(&server, user_id).await;

    // Point device at mock server
    set_device_upstream(&server, device_id, kobo_mock.url()).await;

    // Create local book
    let local_book = create_book(&server, "Local State Book").await;
    entitle_book(&server, local_book).await;

    // Update reading state - should be intercepted and handled locally
    let resp = server
        .kobo_client()
        .kobo_update_state(
            &device_token,
            &local_book.to_string(),
            &serde_json::json!({
                "ReadingStates": [{
                    "CurrentBookmark": {
                        "ProgressPercent": 50.0
                    }
                }]
            }),
        )
        .await;
    resp.assert_ok();

    // Verify it was NOT sent to upstream (our mock tracks requests)
    assert!(
        !kobo_mock.was_called("PUT", &local_book.to_string()),
        "Local book reading state should NOT go to upstream"
    );

    // Verify state was saved locally
    let resp = server
        .kobo_client()
        .kobo_get_state(&device_token, &local_book.to_string())
        .await;
    let states: Vec<Value> = resp.json();
    assert_eq!(
        states[0]["CurrentBookmark"]["ProgressPercent"]
            .as_f64()
            .unwrap(),
        50.0
    );
}

#[tokio::test]
async fn test_merge_mode_local_book_download_intercepted() {
    let kobo_mock = KoboMock::start().await;
    let server = TestServer::start_with_mode(KoboMode::Merge, None).await;

    let user_id = create_user(&server, "localdown@test.com").await;
    let (device_id, device_token) = create_device(&server, user_id).await;

    // Point device at mock server
    set_device_upstream(&server, device_id, kobo_mock.url()).await;

    // Create local book with file
    let local_book = create_book(&server, "Local Download Book").await;
    entitle_book(&server, local_book).await;
    upload_epub(&server, local_book, test_epub_bytes()).await;

    // Download - should be intercepted
    let resp = server
        .kobo_client()
        .kobo_download(&device_token, &local_book.to_string(), "KEPUB")
        .await;
    resp.assert_ok();

    // Should NOT go to upstream
    assert!(
        !kobo_mock.was_called("GET", "/download"),
        "Local book download should NOT go to upstream"
    );
}

#[tokio::test]
async fn test_merge_mode_local_book_metadata_intercepted() {
    let kobo_mock = KoboMock::start().await;
    let server = TestServer::start_with_mode(KoboMode::Merge, None).await;

    let user_id = create_user(&server, "localmeta@test.com").await;
    let (device_id, device_token) = create_device(&server, user_id).await;

    // Point device at mock server
    set_device_upstream(&server, device_id, kobo_mock.url()).await;

    // Create local book
    let local_book = create_book(&server, "Local Metadata Book").await;
    entitle_book(&server, local_book).await;

    // Get metadata - should be intercepted
    let resp = server
        .kobo_client()
        .kobo_metadata(&device_token, &local_book.to_string())
        .await;
    resp.assert_ok();

    // Should NOT go to upstream
    assert!(
        !kobo_mock.was_called("GET", "/metadata"),
        "Local book metadata should NOT go to upstream"
    );
}

// =============================================================================
// Store Book Operations Pass Through
// =============================================================================

#[tokio::test]
async fn test_merge_mode_store_book_reading_state_passes_through() {
    let kobo_mock = KoboMock::start().await;
    kobo_mock.add_simple_book("store-state-book", "Store State Book");

    let server = TestServer::start_with_mode(KoboMode::Merge, None).await;

    let user_id = create_user(&server, "storestate@test.com").await;
    let (device_id, device_token) = create_device(&server, user_id).await;

    // Point device at mock server
    set_device_upstream(&server, device_id, kobo_mock.url()).await;

    // Update reading state for store book - should pass through
    server
        .kobo_client()
        .kobo_update_state(
            &device_token,
            "store-state-book",
            &serde_json::json!({
                "ReadingStates": [{
                    "CurrentBookmark": {
                        "ProgressPercent": 75.0
                    }
                }]
            }),
        )
        .await;

    // Should have gone to upstream
    assert!(
        kobo_mock.was_called("PUT", "store-state-book"),
        "Store book reading state should go to upstream"
    );
}

#[tokio::test]
async fn test_merge_mode_store_book_metadata_passes_through() {
    let kobo_mock = KoboMock::start().await;
    kobo_mock.add_simple_book("store-meta-book", "Store Metadata Book");

    let server = TestServer::start_with_mode(KoboMode::Merge, None).await;

    let user_id = create_user(&server, "storemeta@test.com").await;
    let (device_id, device_token) = create_device(&server, user_id).await;

    // Point device at mock server
    set_device_upstream(&server, device_id, kobo_mock.url()).await;

    // Get metadata for store book - should pass through
    server
        .kobo_client()
        .kobo_metadata(&device_token, "store-meta-book")
        .await;

    // Should have gone to upstream
    assert!(
        kobo_mock.was_called("GET", "store-meta-book"),
        "Store book metadata should go to upstream"
    );
}

// =============================================================================
// Tags in Merge Mode
// =============================================================================

#[tokio::test]
async fn test_merge_mode_tags_work_for_local_books() {
    let kobo_mock = KoboMock::start().await;
    let server = TestServer::start_with_mode(KoboMode::Merge, None).await;

    let user_id = create_user(&server, "mergetag@test.com").await;
    let (device_id, device_token) = create_device(&server, user_id).await;

    // Point device at mock server
    set_device_upstream(&server, device_id, kobo_mock.url()).await;

    // Create local book
    let local_book = create_book(&server, "Tagged Book").await;
    entitle_book(&server, local_book).await;

    // Create tag with local book
    let resp = server
        .kobo_client()
        .kobo_create_tag(
            &device_token,
            &serde_json::json!({
                "Name": "My Shelf",
                "Items": [{
                    "RevisionId": local_book.to_string(),
                    "Type": "ProductRevisionTagItem"
                }]
            }),
        )
        .await;
    resp.assert_ok();

    // Tag should NOT go to upstream (it's our tag)
    assert!(
        !kobo_mock.was_called("POST", "/tags"),
        "Tags for local books should NOT go to upstream"
    );
}

// =============================================================================
// Batching in Merge Mode
// =============================================================================

#[tokio::test]
async fn test_merge_mode_batching_works_correctly() {
    let kobo_mock = KoboMock::start().await;
    // Add some store books
    for i in 0..5 {
        kobo_mock.add_simple_book(
            &format!("store-batch-{}", i),
            &format!("Store Batch Book {}", i),
        );
    }

    let server = TestServer::start_with_mode(KoboMode::Merge, None).await;

    let user_id = create_user(&server, "batchmerge@test.com").await;
    let (device_id, device_token) = create_device(&server, user_id).await;

    // Point device at mock server
    set_device_upstream(&server, device_id, kobo_mock.url()).await;

    // Create more local books than batch size (10 in test config)
    for i in 0..8 {
        let book = create_book(&server, &format!("Local Batch Book {}", i)).await;
        entitle_book(&server, book).await;
    }

    // First sync - should have continue header (8 local + 5 store = 13 > 10 batch)
    let resp = server.kobo_client().kobo_sync(&device_token).await;
    resp.assert_ok();
    let token = resp.get_sync_token().expect("should have sync token");

    // Should have books from first batch
    let items: Vec<Value> = resp.json();
    assert!(
        count_sync_books(&items) > 0,
        "Should have books in first batch"
    );

    // If continue, do second sync with token
    if resp.has_continue_sync() {
        let resp2 = server
            .kobo_client()
            .kobo_sync_with_token(&device_token, &token)
            .await;
        resp2.assert_ok();

        let items2: Vec<Value> = resp2.json();
        assert!(
            count_sync_books(&items2) > 0,
            "Should have books in second batch"
        );
    }
}

// =============================================================================
// Image URL Format Tests
// =============================================================================

#[tokio::test]
async fn test_merge_mode_initialization_image_urls_have_correct_format() {
    let kobo_mock = KoboMock::start().await;
    let server = TestServer::start_with_mode(KoboMode::Merge, None).await;

    let user_id = create_user(&server, "imgurl@test.com").await;
    let (device_id, device_token) = create_device(&server, user_id).await;

    // Point device at mock server
    set_device_upstream(&server, device_id, kobo_mock.url()).await;

    let resp = server.kobo_client().kobo_init(&device_token).await;
    resp.assert_ok();

    let init: Value = resp.json();
    let resources = init.get("Resources").expect("Resources should exist");

    // Check image_url_template has correct format
    let template = resources
        .get("image_url_template")
        .and_then(|v| v.as_str())
        .expect("image_url_template should exist");

    assert!(
        template.contains("/book-images/"),
        "image_url_template should contain /book-images/: {}",
        template
    );
    assert!(
        template.contains("__upstream_host=cdn.kobo.com"),
        "image_url_template should contain __upstream_host=cdn.kobo.com: {}",
        template
    );

    // Check image_url_quality_template has correct format
    let quality_template = resources
        .get("image_url_quality_template")
        .and_then(|v| v.as_str())
        .expect("image_url_quality_template should exist");

    assert!(
        quality_template.contains("/book-images/"),
        "image_url_quality_template should contain /book-images/: {}",
        quality_template
    );
    assert!(
        quality_template.contains("__upstream_host=cdn.kobo.com"),
        "image_url_quality_template should contain __upstream_host=cdn.kobo.com: {}",
        quality_template
    );
}

#[tokio::test]
async fn test_merge_mode_local_book_cover_intercepted() {
    let kobo_mock = KoboMock::start().await;
    let server = TestServer::start_with_mode(KoboMode::Merge, None).await;

    let user_id = create_user(&server, "localcover@test.com").await;
    let (device_id, device_token) = create_device(&server, user_id).await;

    // Point device at mock server
    set_device_upstream(&server, device_id, kobo_mock.url()).await;

    // Create local book with cover
    let local_book = create_book(&server, "Local Cover Book").await;
    entitle_book(&server, local_book).await;
    upload_cover(&server, local_book, test_jpeg_image()).await;

    // Request cover using new /book-images/ path format
    let resp = server
        .kobo_client()
        .kobo_cover(&device_token, &local_book.to_string(), 355, 530)
        .await;
    resp.assert_ok();

    // Should NOT go to upstream mock - we serve local covers
    assert!(
        !kobo_mock.was_called("GET", "book-images"),
        "Local book cover should NOT go to upstream"
    );
}

#[tokio::test]
async fn test_merge_mode_store_book_cover_passes_through() {
    let kobo_mock = KoboMock::start().await;
    kobo_mock.add_simple_book("store-cover-book", "Store Cover Book");

    let server = TestServer::start_with_mode(KoboMode::Merge, None).await;

    let user_id = create_user(&server, "storecover@test.com").await;
    let (device_id, device_token) = create_device(&server, user_id).await;

    // Point device at mock server
    set_device_upstream(&server, device_id, kobo_mock.url()).await;

    // Request cover for store book - should passthrough to mock (simulating cdn.kobo.com)
    let resp = server
        .kobo_client()
        .kobo_cover(&device_token, "store-cover-book", 355, 530)
        .await;
    resp.assert_ok();

    // Should have gone to upstream
    assert!(
        kobo_mock.was_called("GET", "store-cover-book"),
        "Store book cover should go to upstream"
    );
}
