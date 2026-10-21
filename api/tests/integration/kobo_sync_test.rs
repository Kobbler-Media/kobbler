use crate::common::*;
use reqwest::StatusCode;
use serde_json::Value;
use uuid::Uuid;

#[tokio::test]
async fn test_sync_returns_entitled_books() {
    let server = TestServer::start().await;

    let user_id = create_user(&server, "test@example.com").await;
    let (_, device_token) = create_device(&server, user_id).await;

    let book_id = create_book(&server, "Test Book").await;
    entitle_book(&server, book_id).await;

    let resp = server.kobo_client().kobo_sync(&device_token).await;
    resp.assert_ok();

    let items: Vec<Value> = resp.json();
    assert_sync_contains_book(&items, book_id);
}

#[tokio::test]
async fn test_sync_excludes_unentitled_books() {
    let server = TestServer::start().await;

    let user_id = create_user(&server, "test@example.com").await;
    let (_, device_token) = create_device(&server, user_id).await;

    let book_id = create_book(&server, "Unentitled Book").await;

    let resp = server.kobo_client().kobo_sync(&device_token).await;
    resp.assert_ok();

    let items: Vec<Value> = resp.json();
    assert_sync_excludes_book(&items, book_id);
}

#[tokio::test]
async fn test_sync_respects_user_scope() {
    let server = TestServer::start().await;

    let user1_id = create_user(&server, "user1@test.com").await;
    let (_, device1_token) = create_device(&server, user1_id).await;
    let book1 = create_book(&server, "User1 Book").await;
    entitle_book_to_user(&server, book1, user1_id).await;

    let user2_id = create_user(&server, "user2@test.com").await;
    let (_, device2_token) = create_device(&server, user2_id).await;

    let items: Vec<Value> = server
        .kobo_client()
        .kobo_sync(&device1_token)
        .await
        .assert_ok()
        .json();
    assert_sync_contains_book(&items, book1);

    let items: Vec<Value> = server
        .kobo_client()
        .kobo_sync(&device2_token)
        .await
        .assert_ok()
        .json();
    assert_sync_excludes_book(&items, book1);
}

#[tokio::test]
async fn test_sync_respects_device_scope() {
    let server = TestServer::start().await;

    let user_id = create_user(&server, "test@example.com").await;
    let (device1_id, device1_token) = create_device(&server, user_id).await;
    let (_, device2_token) = create_device(&server, user_id).await;

    let book = create_book(&server, "Device1 Book").await;
    entitle_book_to_device(&server, book, device1_id).await;

    let items: Vec<Value> = server
        .kobo_client()
        .kobo_sync(&device1_token)
        .await
        .assert_ok()
        .json();
    assert_sync_contains_book(&items, book);

    let items: Vec<Value> = server
        .kobo_client()
        .kobo_sync(&device2_token)
        .await
        .assert_ok()
        .json();
    assert_sync_excludes_book(&items, book);
}

#[tokio::test]
async fn test_sync_all_users_scope_visible_to_all() {
    let server = TestServer::start().await;

    let user1_id = create_user(&server, "user1@test.com").await;
    let (_, device1_token) = create_device(&server, user1_id).await;

    let user2_id = create_user(&server, "user2@test.com").await;
    let (_, device2_token) = create_device(&server, user2_id).await;

    let book = create_book(&server, "Public Book").await;
    entitle_book(&server, book).await;

    let items: Vec<Value> = server
        .kobo_client()
        .kobo_sync(&device1_token)
        .await
        .assert_ok()
        .json();
    assert_sync_contains_book(&items, book);

    let items: Vec<Value> = server
        .kobo_client()
        .kobo_sync(&device2_token)
        .await
        .assert_ok()
        .json();
    assert_sync_contains_book(&items, book);
}

#[tokio::test]
async fn test_sync_state_advances_via_token() {
    let server = TestServer::start().await;

    let user_id = create_user(&server, "test@example.com").await;
    let (_, device_token) = create_device(&server, user_id).await;

    let book1 = create_book(&server, "Book 1").await;
    entitle_book(&server, book1).await;

    // First sync - should see book1, capture token
    let resp = server.kobo_client().kobo_sync(&device_token).await;
    resp.assert_ok();
    let token = resp.get_sync_token().expect("should have sync token");
    let items: Vec<Value> = resp.json();
    assert_sync_contains_book(&items, book1);

    // Second sync with token - book1 should NOT appear again
    let resp = server
        .kobo_client()
        .kobo_sync_with_token(&device_token, &token)
        .await;
    resp.assert_ok();
    let token = resp.get_sync_token().expect("should have sync token");
    let items: Vec<Value> = resp.json();
    assert_sync_excludes_book(&items, book1);

    // Add a new book
    let book2 = create_book(&server, "Book 2").await;
    entitle_book(&server, book2).await;

    // Third sync with token - only book2 should appear
    let resp = server
        .kobo_client()
        .kobo_sync_with_token(&device_token, &token)
        .await;
    resp.assert_ok();
    let items: Vec<Value> = resp.json();
    assert_sync_excludes_book(&items, book1);
    assert_sync_contains_book(&items, book2);
}

#[tokio::test]
async fn test_sync_without_token_returns_all() {
    let server = TestServer::start().await;

    let user_id = create_user(&server, "test@example.com").await;
    let (_, device_token) = create_device(&server, user_id).await;

    let book = create_book(&server, "Test Book").await;
    entitle_book(&server, book).await;

    // First sync - get book and token
    let resp = server.kobo_client().kobo_sync(&device_token).await;
    resp.assert_ok();
    let items: Vec<Value> = resp.json();
    assert_sync_contains_book(&items, book);

    // Sync again WITHOUT token (simulating lost token) = full resync
    let resp = server.kobo_client().kobo_sync(&device_token).await;
    resp.assert_ok();
    let items: Vec<Value> = resp.json();
    assert_sync_contains_book(&items, book);
}

#[tokio::test]
async fn test_sync_corrupt_token_returns_all() {
    let server = TestServer::start().await;

    let user_id = create_user(&server, "test@example.com").await;
    let (_, device_token) = create_device(&server, user_id).await;

    let book = create_book(&server, "Test Book").await;
    entitle_book(&server, book).await;

    // Sync with garbage token = full resync (graceful fallback)
    let resp = server
        .kobo_client()
        .kobo_sync_with_token(&device_token, "garbage-not-a-jwt")
        .await;
    resp.assert_ok();
    let items: Vec<Value> = resp.json();
    assert_sync_contains_book(&items, book);
}

#[tokio::test]
async fn test_sync_token_header_is_set() {
    let server = TestServer::start().await;

    let user_id = create_user(&server, "test@example.com").await;
    let (_, device_token) = create_device(&server, user_id).await;

    let book = create_book(&server, "Test Book").await;
    entitle_book(&server, book).await;

    let resp = server.kobo_client().kobo_sync(&device_token).await;
    resp.assert_ok();
    assert!(
        resp.get_sync_token().is_some(),
        "Sync response should include x-kobo-synctoken header"
    );
}

#[tokio::test]
async fn test_sync_batching_with_continue_header() {
    let server = TestServer::start().await;

    let user_id = create_user(&server, "test@example.com").await;
    let (_, device_token) = create_device(&server, user_id).await;

    for i in 0..15 {
        let book = create_book(&server, &format!("Book {}", i)).await;
        entitle_book(&server, book).await;
    }

    // First sync - should have continue header and 10 books
    let resp = server.kobo_client().kobo_sync(&device_token).await;
    resp.assert_ok();
    assert!(
        resp.has_continue_sync(),
        "Should have x-kobo-sync: continue"
    );
    let token = resp.get_sync_token().expect("should have sync token");
    let items: Vec<Value> = resp.json();
    assert_eq!(
        count_sync_books(&items),
        10,
        "First batch should have 10 books"
    );

    // Second sync with token - should return remaining 5 books without continue
    let resp = server
        .kobo_client()
        .kobo_sync_with_token(&device_token, &token)
        .await;
    resp.assert_ok();
    assert!(
        !resp.has_continue_sync(),
        "Should not continue after last batch"
    );
    let items: Vec<Value> = resp.json();
    assert_eq!(
        count_sync_books(&items),
        5,
        "Second batch should have 5 books"
    );
}

#[tokio::test]
async fn test_sync_invalid_device_token() {
    let server = TestServer::start().await;

    let resp = server.kobo_client().kobo_sync("invalid-token-12345").await;
    resp.assert_status(StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn test_removed_book_syncs_with_is_removed() {
    let server = TestServer::start().await;

    let user_id = create_user(&server, "test@example.com").await;
    let (_, device_token) = create_device(&server, user_id).await;

    let book = create_book(&server, "To Be Removed").await;
    entitle_book(&server, book).await;

    // First sync - book should appear, capture token
    let resp = server.kobo_client().kobo_sync(&device_token).await;
    resp.assert_ok();
    let token = resp.get_sync_token().expect("should have sync token");
    let items: Vec<Value> = resp.json();
    assert_sync_contains_book(&items, book);

    // Remove the book via admin API
    server
        .admin_client()
        .post_json(
            &format!("/api/v1/admin/books/{}/remove", book),
            &serde_json::json!({}),
        )
        .await
        .assert_success();

    // Second sync with token - book should appear with IsRemoved=true
    let resp = server
        .kobo_client()
        .kobo_sync_with_token(&device_token, &token)
        .await;
    resp.assert_ok();
    let items: Vec<Value> = resp.json();
    assert_book_is_removed(&items, book);
}

#[tokio::test]
async fn test_resync_device_full_sync_without_token() {
    let server = TestServer::start().await;

    let user_id = create_user(&server, "test@example.com").await;
    let (_, device_token) = create_device(&server, user_id).await;

    let book = create_book(&server, "Test Book").await;
    entitle_book(&server, book).await;

    // First sync - book appears
    let resp = server.kobo_client().kobo_sync(&device_token).await;
    resp.assert_ok();
    let token = resp.get_sync_token().expect("should have sync token");
    let items: Vec<Value> = resp.json();
    assert_sync_contains_book(&items, book);

    // Second sync with token - book doesn't appear (already synced)
    let resp = server
        .kobo_client()
        .kobo_sync_with_token(&device_token, &token)
        .await;
    resp.assert_ok();
    let items: Vec<Value> = resp.json();
    assert_sync_excludes_book(&items, book);

    // "Resync" by dropping the token (sending no token = full sync from epoch)
    let resp = server.kobo_client().kobo_sync(&device_token).await;
    resp.assert_ok();
    let items: Vec<Value> = resp.json();
    assert_sync_contains_book(&items, book);
}

// =============================================================================
// File Availability Tests
// =============================================================================

#[tokio::test]
async fn test_sync_excludes_books_with_all_files_unavailable() {
    let server = TestServer::start().await;

    let user_id = create_user(&server, "test@example.com").await;
    let (_, device_token) = create_device(&server, user_id).await;

    let book = create_book(&server, "Unavailable Book").await;
    upload_epub(&server, book, test_epub_bytes()).await;
    entitle_book(&server, book).await;

    // First sync - book appears, capture token
    let resp = server.kobo_client().kobo_sync(&device_token).await;
    resp.assert_ok();
    let token = resp.get_sync_token().expect("should have sync token");
    let items: Vec<Value> = resp.json();
    assert_sync_contains_book(&items, book);

    // Mark all files as unavailable
    sqlx::query("UPDATE book_files SET available = FALSE WHERE book_id = $1")
        .bind(book)
        .execute(&server.pool)
        .await
        .unwrap();

    // Touch book to trigger re-sync
    touch_book(&server, book).await;

    // Second sync with token - book should NOT appear (all files unavailable)
    let resp = server
        .kobo_client()
        .kobo_sync_with_token(&device_token, &token)
        .await;
    resp.assert_ok();
    let items: Vec<Value> = resp.json();
    assert_sync_excludes_book(&items, book);
}

#[tokio::test]
async fn test_sync_includes_books_with_some_files_available() {
    let server = TestServer::start().await;

    let user_id = create_user(&server, "test@example.com").await;
    let (_, device_token) = create_device(&server, user_id).await;

    let book = create_book(&server, "Partially Available Book").await;
    upload_epub(&server, book, test_epub_bytes()).await;
    entitle_book(&server, book).await;

    // First sync - book appears
    let items: Vec<Value> = server
        .kobo_client()
        .kobo_sync(&device_token)
        .await
        .assert_ok()
        .json();
    assert_sync_contains_book(&items, book);

    // Add another file entry and mark the first as unavailable
    sqlx::query(
        "INSERT INTO book_files (id, book_id, format, file_path, size_bytes, available) VALUES ($1, $2, 'PDF', '/fake/path.pdf', 1000, TRUE)"
    )
    .bind(Uuid::new_v4())
    .bind(book)
    .execute(&server.pool)
    .await
    .unwrap();

    sqlx::query("UPDATE book_files SET available = FALSE WHERE book_id = $1 AND format = 'KEPUB'")
        .bind(book)
        .execute(&server.pool)
        .await
        .unwrap();

    touch_book(&server, book).await;

    // Create new device to get clean sync state (no token = full sync)
    let (_, device2_token) = create_device(&server, user_id).await;

    let items: Vec<Value> = server
        .kobo_client()
        .kobo_sync(&device2_token)
        .await
        .assert_ok()
        .json();
    assert_sync_contains_book(&items, book);
}

#[tokio::test]
async fn test_sync_includes_books_without_files() {
    let server = TestServer::start().await;

    let user_id = create_user(&server, "test@example.com").await;
    let (_, device_token) = create_device(&server, user_id).await;

    let book = create_book(&server, "Metadata Only Book").await;
    entitle_book(&server, book).await;

    let items: Vec<Value> = server
        .kobo_client()
        .kobo_sync(&device_token)
        .await
        .assert_ok()
        .json();
    assert_sync_contains_book(&items, book);
}

#[tokio::test]
async fn test_sync_unavailable_book_reappears_when_available() {
    let server = TestServer::start().await;

    let user_id = create_user(&server, "test@example.com").await;
    let (_, device_token) = create_device(&server, user_id).await;

    let book = create_book(&server, "Reappearing Book").await;
    upload_epub(&server, book, test_epub_bytes()).await;
    entitle_book(&server, book).await;

    // First sync - book appears, capture token
    let resp = server.kobo_client().kobo_sync(&device_token).await;
    resp.assert_ok();
    let token = resp.get_sync_token().expect("should have sync token");
    let items: Vec<Value> = resp.json();
    assert_sync_contains_book(&items, book);

    // Mark file as unavailable
    sqlx::query("UPDATE book_files SET available = FALSE WHERE book_id = $1")
        .bind(book)
        .execute(&server.pool)
        .await
        .unwrap();

    // Touch and try sync with token - book should not appear
    touch_book(&server, book).await;
    let resp = server
        .kobo_client()
        .kobo_sync_with_token(&device_token, &token)
        .await;
    resp.assert_ok();
    let _token = resp.get_sync_token().expect("should have sync token");
    let items: Vec<Value> = resp.json();
    assert_sync_excludes_book(&items, book);

    // Mark file as available again
    sqlx::query("UPDATE book_files SET available = TRUE WHERE book_id = $1")
        .bind(book)
        .execute(&server.pool)
        .await
        .unwrap();

    // Touch book to update timestamp
    touch_book(&server, book).await;

    // Sync without token = full resync, should see the book again
    let resp = server.kobo_client().kobo_sync(&device_token).await;
    resp.assert_ok();
    let items: Vec<Value> = resp.json();
    assert_sync_contains_book(&items, book);
}

/// Validates token-based sync state across multiple batches.
#[tokio::test]
async fn test_sync_state_deferred_across_batches() {
    let server = TestServer::start().await;

    let user_id = create_user(&server, "test@example.com").await;
    let (_, device_token) = create_device(&server, user_id).await;

    // Create 25 books (batch size is 10 in tests)
    let mut all_books = Vec::new();
    for i in 0..25 {
        let book = create_book(&server, &format!("Deferred Batch Book {}", i)).await;
        entitle_book(&server, book).await;
        all_books.push(book);
    }

    // Batch 1: 10 books, continue header
    let resp = server.kobo_client().kobo_sync(&device_token).await;
    resp.assert_ok();
    assert!(resp.has_continue_sync());
    let token = resp.get_sync_token().expect("should have sync token");
    let batch1: Vec<Value> = resp.json();
    let batch1_ids = get_sync_book_ids(&batch1);
    assert_eq!(batch1_ids.len(), 10);

    // Batch 2: 10 books, continue header (forward token)
    let resp = server
        .kobo_client()
        .kobo_sync_with_token(&device_token, &token)
        .await;
    resp.assert_ok();
    assert!(resp.has_continue_sync());
    let token = resp.get_sync_token().expect("should have sync token");
    let batch2: Vec<Value> = resp.json();
    let batch2_ids = get_sync_book_ids(&batch2);
    assert_eq!(batch2_ids.len(), 10);

    // Verify no overlap between batches
    for id in &batch2_ids {
        assert!(
            !batch1_ids.contains(id),
            "Book {} appeared in both batch 1 and batch 2",
            id
        );
    }

    // Batch 3: remaining 5 books, no continue
    let resp = server
        .kobo_client()
        .kobo_sync_with_token(&device_token, &token)
        .await;
    resp.assert_ok();
    assert!(!resp.has_continue_sync());
    let token = resp.get_sync_token().expect("should have sync token");
    let batch3: Vec<Value> = resp.json();
    let batch3_ids = get_sync_book_ids(&batch3);
    assert_eq!(batch3_ids.len(), 5);

    // Verify batch 3 doesn't overlap with previous batches
    for id in &batch3_ids {
        assert!(
            !batch1_ids.contains(id) && !batch2_ids.contains(id),
            "Book {} appeared in multiple batches",
            id
        );
    }

    // All 25 books accounted for
    assert_eq!(batch1_ids.len() + batch2_ids.len() + batch3_ids.len(), 25);

    // Final sync with token: nothing new
    let resp = server
        .kobo_client()
        .kobo_sync_with_token(&device_token, &token)
        .await;
    resp.assert_ok();
    let items: Vec<Value> = resp.json();
    assert_eq!(
        count_sync_books(&items),
        0,
        "No more books should appear after all batches delivered"
    );
}

// =============================================================================
// Entitlement Deduplication Tests
// =============================================================================

/// When a book has a removed ALL_USERS entitlement and an active DEVICE entitlement,
/// the sync should return exactly one entry with IsRemoved: false (device scope wins).
#[tokio::test]
async fn test_sync_device_scope_wins_over_removed_all_users() {
    let server = TestServer::start().await;

    let user_id = create_user(&server, "test@example.com").await;
    let (device_id, device_token) = create_device(&server, user_id).await;

    let book = create_book(&server, "Scope Priority Book").await;

    // Create ALL_USERS entitlement, then remove it
    entitle_book(&server, book).await;
    server
        .admin_client()
        .post_json(
            &format!("/api/v1/admin/books/{}/remove", book),
            &serde_json::json!({}),
        )
        .await
        .assert_success();

    // Create active DEVICE entitlement
    entitle_book_to_device(&server, book, device_id).await;

    // Sync should show the book as NOT removed (device scope wins over removed ALL_USERS)
    let resp = server.kobo_client().kobo_sync(&device_token).await;
    resp.assert_ok();
    let items: Vec<Value> = resp.json();

    // Should appear exactly once
    let matching: Vec<_> = get_sync_book_ids(&items)
        .into_iter()
        .filter(|id| *id == book)
        .collect();
    assert_eq!(
        matching.len(),
        1,
        "Book should appear exactly once in sync response, found {}",
        matching.len()
    );

    // And it should NOT be removed
    assert_book_is_not_removed(&items, book);
}

/// When a book has entitlements at multiple scopes (ALL_USERS + DEVICE),
/// the sync should return exactly one entry (no duplicates).
#[tokio::test]
async fn test_sync_no_duplicate_entries() {
    let server = TestServer::start().await;

    let user_id = create_user(&server, "test@example.com").await;
    let (device_id, device_token) = create_device(&server, user_id).await;

    let book = create_book(&server, "Multi-Scope Book").await;

    // Create entitlements at multiple scopes
    entitle_book(&server, book).await; // ALL_USERS
    entitle_book_to_device(&server, book, device_id).await; // DEVICE

    // Sync should return exactly one entry for this book
    let resp = server.kobo_client().kobo_sync(&device_token).await;
    resp.assert_ok();
    let items: Vec<Value> = resp.json();

    let matching: Vec<_> = get_sync_book_ids(&items)
        .into_iter()
        .filter(|id| *id == book)
        .collect();
    assert_eq!(
        matching.len(),
        1,
        "Book should appear exactly once in sync response, found {}",
        matching.len()
    );
}

// =============================================================================
// Rating Drift Tests
// =============================================================================

/// Rating-only changes should NOT cause books to reappear in sync.
/// This prevents the author_sync_check job (which fetches ratings from Hardcover
/// every 5 minutes) from triggering a full resync of all entitled books.
#[tokio::test]
async fn test_rating_change_does_not_trigger_resync() {
    let server = TestServer::start().await;

    let user_id = create_user(&server, "test@example.com").await;
    let (_, device_token) = create_device(&server, user_id).await;

    let book = create_book(&server, "Rating Drift Book").await;
    entitle_book(&server, book).await;

    // First sync - book appears, capture token
    let resp = server.kobo_client().kobo_sync(&device_token).await;
    resp.assert_ok();
    let token = resp.get_sync_token().expect("should have sync token");
    let items: Vec<Value> = resp.json();
    assert_sync_contains_book(&items, book);

    // Update only rating (simulates Hardcover float drift during author_sync_check)
    let update = api::repositories::books::BookUpdate {
        rating: Some(4.25489396411093),
        rating_count: Some(1226),
        ..Default::default()
    };
    api::repositories::books::update(&server.pool, book, update)
        .await
        .unwrap();

    // Second sync with token - book should NOT reappear (rating is cosmetic)
    let resp = server
        .kobo_client()
        .kobo_sync_with_token(&device_token, &token)
        .await;
    resp.assert_ok();
    let items: Vec<Value> = resp.json();
    assert_sync_excludes_book(&items, book);
}

/// Title change should still trigger resync even when combined with rating change.
#[tokio::test]
async fn test_title_change_triggers_resync_despite_rating() {
    let server = TestServer::start().await;

    let user_id = create_user(&server, "test@example.com").await;
    let (_, device_token) = create_device(&server, user_id).await;

    let book = create_book(&server, "Title Change Book").await;
    entitle_book(&server, book).await;

    // First sync - book appears, capture token
    let resp = server.kobo_client().kobo_sync(&device_token).await;
    resp.assert_ok();
    let token = resp.get_sync_token().expect("should have sync token");
    let items: Vec<Value> = resp.json();
    assert_sync_contains_book(&items, book);

    // Update title AND rating
    let update = api::repositories::books::BookUpdate {
        title: Some("Updated Title".to_string()),
        rating: Some(4.5),
        ..Default::default()
    };
    api::repositories::books::update(&server.pool, book, update)
        .await
        .unwrap();

    // Second sync - book SHOULD reappear (title change is significant)
    let resp = server
        .kobo_client()
        .kobo_sync_with_token(&device_token, &token)
        .await;
    resp.assert_ok();
    let items: Vec<Value> = resp.json();
    assert_sync_contains_book(&items, book);
}
