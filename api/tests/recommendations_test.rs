//! Tests for book recommendations (nextread endpoint).
//!
//! Tests recommendation logic for:
//! - Next book in series
//! - Same author books
//! - Unread prioritization
//! - Entitlement filtering

use crate::common::*;
use serde_json::{Value, json};

// =============================================================================
// Next In Series Tests
// =============================================================================

#[tokio::test]
async fn test_nextread_recommends_next_in_series() {
    let server = TestServer::start().await;

    let user_id = create_user(&server, "series@test.com").await;
    let (_, device_token) = create_device(&server, user_id).await;

    // Create a series with 3 books
    let book1 = create_book_in_series(&server, "Series Book 1", "Test Series", 1).await;
    let book2 = create_book_in_series(&server, "Series Book 2", "Test Series", 2).await;
    let book3 = create_book_in_series(&server, "Series Book 3", "Test Series", 3).await;

    // Upload files so they're downloadable
    upload_epub(&server, book1, test_epub_bytes()).await;
    upload_epub(&server, book2, test_epub_bytes()).await;
    upload_epub(&server, book3, test_epub_bytes()).await;

    // Entitle all books
    entitle_book(&server, book1).await;
    entitle_book(&server, book2).await;
    entitle_book(&server, book3).await;

    // Get recommendations for book 1 - should recommend book 2 (next in series)
    let resp = server
        .kobo_client()
        .kobo_nextread(&device_token, &book1.to_string())
        .await;
    resp.assert_ok();

    let json: Value = resp.json();
    let recommendations = json
        .get(book1.to_string())
        .and_then(|v| v.as_array())
        .expect("Should have recommendations array");

    assert!(
        !recommendations.is_empty(),
        "Should have at least one recommendation"
    );

    // First recommendation should be book 2 (next in series)
    let first_rec = &recommendations[0];
    assert_eq!(
        first_rec["Id"].as_str().unwrap(),
        book2.to_string(),
        "First recommendation should be next book in series"
    );
    assert_eq!(first_rec["Title"].as_str().unwrap(), "Series Book 2");
}

#[tokio::test]
async fn test_nextread_series_ordering() {
    let server = TestServer::start().await;

    let user_id = create_user(&server, "order@test.com").await;
    let (_, device_token) = create_device(&server, user_id).await;

    // Create a series - insert out of order to ensure sorting works
    let book3 = create_book_in_series(&server, "Book Three", "Ordered Series", 3).await;
    let book1 = create_book_in_series(&server, "Book One", "Ordered Series", 1).await;
    let book5 = create_book_in_series(&server, "Book Five", "Ordered Series", 5).await;
    let book2 = create_book_in_series(&server, "Book Two", "Ordered Series", 2).await;
    let book4 = create_book_in_series(&server, "Book Four", "Ordered Series", 4).await;

    for book in [book1, book2, book3, book4, book5] {
        upload_epub(&server, book, test_epub_bytes()).await;
        entitle_book(&server, book).await;
    }

    // Get recommendations for book 2 - should list 3, 4, 5 in order
    let resp = server
        .kobo_client()
        .kobo_nextread(&device_token, &book2.to_string())
        .await;
    resp.assert_ok();

    let json: Value = resp.json();
    let recommendations = json
        .get(book2.to_string())
        .and_then(|v| v.as_array())
        .expect("Should have recommendations");

    assert!(
        recommendations.len() >= 3,
        "Should have at least 3 recommendations"
    );

    // Verify order: 3, 4, 5
    assert_eq!(recommendations[0]["Title"].as_str().unwrap(), "Book Three");
    assert_eq!(recommendations[1]["Title"].as_str().unwrap(), "Book Four");
    assert_eq!(recommendations[2]["Title"].as_str().unwrap(), "Book Five");
}

// =============================================================================
// Same Author Tests
// =============================================================================

#[tokio::test]
async fn test_nextread_recommends_same_author() {
    let server = TestServer::start().await;

    let user_id = create_user(&server, "author@test.com").await;
    let (_, device_token) = create_device(&server, user_id).await;

    // Create books by the same author (no series)
    let author = "Jane Doe";
    let book1 = create_book_with_author(&server, "First Book", author).await;
    let book2 = create_book_with_author(&server, "Second Book", author).await;
    let book3 = create_book_with_author(&server, "Third Book", author).await;

    for book in [book1, book2, book3] {
        upload_epub(&server, book, test_epub_bytes()).await;
        entitle_book(&server, book).await;
    }

    // Get recommendations for book 1
    let resp = server
        .kobo_client()
        .kobo_nextread(&device_token, &book1.to_string())
        .await;
    resp.assert_ok();

    let json: Value = resp.json();
    let recommendations = json
        .get(book1.to_string())
        .and_then(|v| v.as_array())
        .expect("Should have recommendations");

    // Should recommend other books by the same author
    assert!(
        recommendations.len() >= 2,
        "Should recommend other author books"
    );

    let recommended_ids: Vec<&str> = recommendations
        .iter()
        .map(|r| r["Id"].as_str().unwrap())
        .collect();

    assert!(
        recommended_ids.contains(&book2.to_string().as_str()),
        "Should include book2"
    );
    assert!(
        recommended_ids.contains(&book3.to_string().as_str()),
        "Should include book3"
    );
}

#[tokio::test]
async fn test_nextread_series_before_author() {
    let server = TestServer::start().await;

    let user_id = create_user(&server, "priority@test.com").await;
    let (_, device_token) = create_device(&server, user_id).await;

    let author = "John Smith";

    // Create a book in a series by the author
    let series_book1 =
        create_book_in_series_with_author(&server, "Series Book 1", "My Series", 1, author).await;
    let series_book2 =
        create_book_in_series_with_author(&server, "Series Book 2", "My Series", 2, author).await;

    // Create standalone books by the same author
    let standalone = create_book_with_author(&server, "Standalone Book", author).await;

    for book in [series_book1, series_book2, standalone] {
        upload_epub(&server, book, test_epub_bytes()).await;
        entitle_book(&server, book).await;
    }

    // Get recommendations for series_book1
    let resp = server
        .kobo_client()
        .kobo_nextread(&device_token, &series_book1.to_string())
        .await;
    resp.assert_ok();

    let json: Value = resp.json();
    let recommendations = json
        .get(series_book1.to_string())
        .and_then(|v| v.as_array())
        .expect("Should have recommendations");

    // First recommendation should be next in series (series_book2), not standalone
    assert_eq!(
        recommendations[0]["Id"].as_str().unwrap(),
        series_book2.to_string(),
        "Next in series should come before same-author standalone"
    );
}

// =============================================================================
// Entitlement Tests
// =============================================================================

#[tokio::test]
async fn test_nextread_respects_entitlements() {
    let server = TestServer::start().await;

    let user_id = create_user(&server, "entitled@test.com").await;
    let (_, device_token) = create_device(&server, user_id).await;

    // Create series books
    let book1 = create_book_in_series(&server, "Entitled Book 1", "Series", 1).await;
    let book2 = create_book_in_series(&server, "Not Entitled Book 2", "Series", 2).await;
    let book3 = create_book_in_series(&server, "Entitled Book 3", "Series", 3).await;

    for book in [book1, book2, book3] {
        upload_epub(&server, book, test_epub_bytes()).await;
    }

    // Only entitle books 1 and 3 (skip book 2)
    entitle_book(&server, book1).await;
    entitle_book(&server, book3).await;

    // Get recommendations for book 1
    let resp = server
        .kobo_client()
        .kobo_nextread(&device_token, &book1.to_string())
        .await;
    resp.assert_ok();

    let json: Value = resp.json();
    let recommendations = json
        .get(book1.to_string())
        .and_then(|v| v.as_array())
        .expect("Should have recommendations");

    // Should only recommend book 3 (book 2 is not entitled)
    let recommended_ids: Vec<&str> = recommendations
        .iter()
        .map(|r| r["Id"].as_str().unwrap())
        .collect();

    assert!(
        recommended_ids.contains(&book3.to_string().as_str()),
        "Should include entitled book3"
    );
    assert!(
        !recommended_ids.contains(&book2.to_string().as_str()),
        "Should NOT include non-entitled book2"
    );
}

// =============================================================================
// Empty Recommendations Tests
// =============================================================================

#[tokio::test]
async fn test_nextread_empty_for_standalone_book() {
    let server = TestServer::start().await;

    let user_id = create_user(&server, "standalone@test.com").await;
    let (_, device_token) = create_device(&server, user_id).await;

    // Create a standalone book with a unique author
    let book = create_book_with_author(&server, "Lonely Book", "Unique Author 12345").await;
    upload_epub(&server, book, test_epub_bytes()).await;
    entitle_book(&server, book).await;

    // Get recommendations
    let resp = server
        .kobo_client()
        .kobo_nextread(&device_token, &book.to_string())
        .await;
    resp.assert_ok();

    let json: Value = resp.json();

    // Should return empty object {} - no other books by this author or in a series
    assert_eq!(
        json,
        json!({}),
        "Standalone book with unique author should return empty object"
    );
}

#[tokio::test]
async fn test_nextread_for_nonexistent_book() {
    let server = TestServer::start().await;

    let user_id = create_user(&server, "nonexistent@test.com").await;
    let (_, device_token) = create_device(&server, user_id).await;

    let fake_id = uuid::Uuid::new_v4();
    let resp = server
        .kobo_client()
        .kobo_nextread(&device_token, &fake_id.to_string())
        .await;

    resp.assert_ok();

    let json: Value = resp.json();

    // For nonexistent books, we return empty object {} (matches Kobo API)
    assert_eq!(
        json,
        json!({}),
        "Nonexistent book should return empty object"
    );
}

// =============================================================================
// Reading State / Progress Tests
// =============================================================================

#[tokio::test]
async fn test_nextread_prioritizes_unread_books() {
    let server = TestServer::start().await;

    let user_id = create_user(&server, "progress@test.com").await;
    let (device_id, device_token) = create_device(&server, user_id).await;

    let author = "Prolific Writer";

    // Create 3 books by the same author
    let book_source = create_book_with_author(&server, "Source Book", author).await;
    let book_unread = create_book_with_author(&server, "Unread Book", author).await;
    let book_started = create_book_with_author(&server, "Started Book", author).await;
    let book_finished = create_book_with_author(&server, "Finished Book", author).await;

    for book in [book_source, book_unread, book_started, book_finished] {
        upload_epub(&server, book, test_epub_bytes()).await;
        entitle_book(&server, book).await;
    }

    // Set reading progress: started at 50%, finished at 100%
    set_reading_progress(&server, device_id, user_id, book_started, 50.0).await;
    set_reading_progress(&server, device_id, user_id, book_finished, 100.0).await;
    // book_unread has no progress (NULL)

    // Get recommendations for source book
    let resp = server
        .kobo_client()
        .kobo_nextread(&device_token, &book_source.to_string())
        .await;
    resp.assert_ok();

    let json: Value = resp.json();
    let recommendations = json
        .get(book_source.to_string())
        .and_then(|v| v.as_array())
        .expect("Should have recommendations");

    assert!(recommendations.len() >= 3, "Should have 3 recommendations");

    // Verify order: unread first, then started, then finished
    let first_id = recommendations[0]["Id"].as_str().unwrap();
    let last_id = recommendations[recommendations.len() - 1]["Id"]
        .as_str()
        .unwrap();

    assert_eq!(
        first_id,
        book_unread.to_string(),
        "Unread book should be first"
    );
    assert_eq!(
        last_id,
        book_finished.to_string(),
        "Finished book should be last"
    );
}

// =============================================================================
// Web API Endpoint Tests
// =============================================================================

#[tokio::test]
async fn test_library_recommendations_endpoint() {
    let server = TestServer::start().await;

    // Create books in a series
    let book1 = create_book_in_series(&server, "API Test Book 1", "API Series", 1).await;
    let book2 = create_book_in_series(&server, "API Test Book 2", "API Series", 2).await;

    upload_epub(&server, book1, test_epub_bytes()).await;
    upload_epub(&server, book2, test_epub_bytes()).await;
    entitle_book(&server, book1).await;
    entitle_book(&server, book2).await;

    // Get recommendations via admin API
    let resp = server
        .admin_client()
        .get(&format!("/api/v1/library/books/{}/recommendations", book1))
        .await;
    resp.assert_ok();

    let recommendations: Vec<Value> = resp.json();
    assert!(!recommendations.is_empty(), "Should have recommendations");

    // Check the response format
    let rec = &recommendations[0];
    assert!(rec.get("id").is_some(), "Should have id");
    assert!(rec.get("title").is_some(), "Should have title");
    assert!(rec.get("authors").is_some(), "Should have authors");
    assert!(rec.get("cover_url").is_some(), "Should have cover_url");
    assert!(rec.get("reason").is_some(), "Should have reason");
    assert!(
        rec.get("progress_percent").is_some(),
        "Should have progress_percent (can be null)"
    );

    assert_eq!(rec["reason"].as_str().unwrap(), "next_in_series");
}

#[tokio::test]
async fn test_library_recommendations_requires_auth() {
    let server = TestServer::start().await;

    let book_id = uuid::Uuid::new_v4();

    // Try without auth
    let resp = server
        .api_client()
        .get(&format!(
            "/api/v1/library/books/{}/recommendations",
            book_id
        ))
        .await;

    assert_eq!(resp.status, reqwest::StatusCode::UNAUTHORIZED);
}

// =============================================================================
// Helper Functions
// =============================================================================

/// Create a book with a specific author (no series).
async fn create_book_with_author(server: &TestServer, title: &str, author: &str) -> uuid::Uuid {
    let book_id = uuid::Uuid::new_v4();
    let resp = server
        .ingest_client()
        .put_json(
            &format!("/api/v1/books/{}", book_id),
            &serde_json::json!({
                "title": title,
                "authors": [author]
            }),
        )
        .await;
    resp.assert_success();
    book_id
}

/// Create a book in a series with a specific author.
async fn create_book_in_series_with_author(
    server: &TestServer,
    title: &str,
    series_name: &str,
    series_index: i32,
    author: &str,
) -> uuid::Uuid {
    let book_id = uuid::Uuid::new_v4();
    let resp = server
        .ingest_client()
        .put_json(
            &format!("/api/v1/books/{}", book_id),
            &serde_json::json!({
                "title": title,
                "authors": [author],
                "series_name": series_name,
                "series_index": series_index
            }),
        )
        .await;
    resp.assert_success();
    book_id
}

/// Set reading progress for a book on a device.
async fn set_reading_progress(
    server: &TestServer,
    device_id: uuid::Uuid,
    user_id: uuid::Uuid,
    book_id: uuid::Uuid,
    progress_percent: f64,
) {
    // Insert reading state directly into the database
    let state_id = uuid::Uuid::new_v4();

    // Create reading_states entry
    sqlx::query(
        r#"
        INSERT INTO reading_states (id, book_id, user_id, device_id)
        VALUES ($1, $2, $3, $4)
        ON CONFLICT (book_id, user_id, device_id) DO UPDATE SET updated_at = NOW()
        RETURNING id
        "#,
    )
    .bind(state_id)
    .bind(book_id)
    .bind(user_id)
    .bind(device_id)
    .fetch_one(&server.pool)
    .await
    .expect("Failed to create reading state");

    // Get the actual state_id (in case of conflict)
    let actual_state_id: uuid::Uuid = sqlx::query_scalar(
        "SELECT id FROM reading_states WHERE book_id = $1 AND user_id = $2 AND device_id = $3",
    )
    .bind(book_id)
    .bind(user_id)
    .bind(device_id)
    .fetch_one(&server.pool)
    .await
    .expect("Failed to get reading state id");

    // Insert reading_bookmark with progress
    sqlx::query(
        r#"
        INSERT INTO reading_bookmark (reading_state_id, progress_percent)
        VALUES ($1, $2)
        ON CONFLICT (reading_state_id) DO UPDATE SET progress_percent = $2
        "#,
    )
    .bind(actual_state_id)
    .bind(progress_percent)
    .execute(&server.pool)
    .await
    .expect("Failed to set reading progress");
}
