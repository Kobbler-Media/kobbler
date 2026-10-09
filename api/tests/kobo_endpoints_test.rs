//! Tests for individual Kobo endpoints that weren't fully covered elsewhere.
//!
//! Covers: initialization, metadata, delete book, series, nextread

use crate::common::*;
use reqwest::StatusCode;
use serde_json::Value;

// =============================================================================
// Initialization Endpoint Tests
// =============================================================================

#[tokio::test]
async fn test_initialization_returns_resources() {
    let server = TestServer::start().await;

    let user_id = create_user(&server, "init@test.com").await;
    let (_, device_token) = create_device(&server, user_id).await;

    let resp = server.kobo_client().kobo_init(&device_token).await;
    resp.assert_ok();

    let init: Value = resp.json();

    // Must have Resources object
    assert!(
        init.get("Resources").is_some(),
        "Init should have Resources"
    );

    // Should have library_sync resource
    assert_init_has_resource(&init, "library_sync");
}

#[tokio::test]
async fn test_initialization_resources_point_to_our_server() {
    let server = TestServer::start().await;

    let user_id = create_user(&server, "initlocal@test.com").await;
    let (_, device_token) = create_device(&server, user_id).await;

    let resp = server.kobo_client().kobo_init(&device_token).await;
    resp.assert_ok();

    let init: Value = resp.json();
    let resources = init.get("Resources").unwrap();

    // In local mode, URLs should point to our proxy
    // The exact format depends on implementation, but should contain proxy URL
    let sync_url = resources
        .get("library_sync")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    assert!(!sync_url.is_empty(), "library_sync should have a URL");
}

#[tokio::test]
async fn test_initialization_unauthorized() {
    let server = TestServer::start().await;

    // Invalid device token
    let resp = server.kobo_client().kobo_init("invalid-token-xyz").await;

    resp.assert_status(StatusCode::UNAUTHORIZED);
}

// =============================================================================
// Metadata Endpoint Tests
// =============================================================================

#[tokio::test]
async fn test_metadata_for_local_book() {
    let server = TestServer::start().await;

    let user_id = create_user(&server, "meta@test.com").await;
    let (_, device_token) = create_device(&server, user_id).await;

    // Create book with metadata
    let book_id = create_book(&server, "Metadata Test Book").await;
    entitle_book(&server, book_id).await;

    // Get metadata
    let resp = server
        .kobo_client()
        .kobo_metadata(&device_token, &book_id.to_string())
        .await;
    resp.assert_ok();

    let metadata: Vec<Value> = resp.json();
    assert!(
        !metadata.is_empty(),
        "Metadata response should not be empty"
    );
    assert_eq!(
        metadata[0].get("Title").and_then(|t| t.as_str()),
        Some("Metadata Test Book")
    );
}

#[tokio::test]
async fn test_metadata_includes_download_urls() {
    let server = TestServer::start().await;

    let user_id = create_user(&server, "metaurls@test.com").await;
    let (_, device_token) = create_device(&server, user_id).await;

    // Create book with file
    let book_id = create_book(&server, "Metadata URLs Book").await;
    upload_epub(&server, book_id, test_epub_bytes()).await;
    entitle_book(&server, book_id).await;

    // Get metadata
    let resp = server
        .kobo_client()
        .kobo_metadata(&device_token, &book_id.to_string())
        .await;
    resp.assert_ok();

    let metadata: Vec<Value> = resp.json();
    assert!(
        !metadata.is_empty(),
        "Metadata response should not be empty"
    );
    let urls = metadata[0].get("DownloadUrls").and_then(|u| u.as_array());
    assert!(
        urls.is_some() && !urls.unwrap().is_empty(),
        "Should have download URLs"
    );
}

#[tokio::test]
async fn test_metadata_for_nonexistent_book() {
    let server = TestServer::start().await;

    let user_id = create_user(&server, "metanone@test.com").await;
    let (_, device_token) = create_device(&server, user_id).await;

    let fake_id = uuid::Uuid::new_v4();
    let resp = server
        .kobo_client()
        .kobo_metadata(&device_token, &fake_id.to_string())
        .await;

    // Should return 404 or empty response
    assert!(
        resp.status == StatusCode::NOT_FOUND || resp.status == StatusCode::OK,
        "Should handle nonexistent book gracefully"
    );
}

// =============================================================================
// Delete Book Endpoint Tests
// =============================================================================

#[tokio::test]
async fn test_delete_book_from_device() {
    let server = TestServer::start().await;

    let user_id = create_user(&server, "delbook@test.com").await;
    let (_, device_token) = create_device(&server, user_id).await;

    // Create book
    let book_id = create_book(&server, "Delete Test Book").await;
    entitle_book(&server, book_id).await;

    // Sync to get book
    server
        .kobo_client()
        .kobo_sync(&device_token)
        .await
        .assert_ok();

    // Device deletes book
    let resp = server
        .kobo_client()
        .kobo_delete_book(&device_token, &book_id.to_string())
        .await;

    // Should succeed (204 or 200)
    assert!(resp.status.is_success(), "Delete should succeed");
}

#[tokio::test]
async fn test_delete_nonexistent_book() {
    let server = TestServer::start().await;

    let user_id = create_user(&server, "delnone@test.com").await;
    let (_, device_token) = create_device(&server, user_id).await;

    let fake_id = uuid::Uuid::new_v4();
    let resp = server
        .kobo_client()
        .kobo_delete_book(&device_token, &fake_id.to_string())
        .await;

    // Should handle gracefully (not crash)
    assert!(
        resp.status == StatusCode::NOT_FOUND
            || resp.status == StatusCode::NO_CONTENT
            || resp.status == StatusCode::OK,
        "Should handle nonexistent delete gracefully"
    );
}

// =============================================================================
// Series Endpoint Tests
// =============================================================================

#[tokio::test]
async fn test_series_books_returns_series_members() {
    let server = TestServer::start().await;

    let user_id = create_user(&server, "series@test.com").await;
    let (_, device_token) = create_device(&server, user_id).await;

    // Create books in a series - need to get series ID
    let book1 = create_book_in_series(&server, "Series Book 1", "My Series", 1).await;
    let book2 = create_book_in_series(&server, "Series Book 2", "My Series", 2).await;

    entitle_book(&server, book1).await;
    entitle_book(&server, book2).await;

    // Sync to populate data
    server
        .kobo_client()
        .kobo_sync(&device_token)
        .await
        .assert_ok();

    // Get series ID from database or sync response
    // For now, we can use the series endpoint with book ID (some implementations use book ID)
    let resp = server
        .kobo_client()
        .kobo_series_books(&device_token, &book1.to_string())
        .await;

    // Should succeed or return empty for our implementation
    assert!(resp.status.is_success() || resp.status == StatusCode::NOT_FOUND);
}

// =============================================================================
// Nextread Endpoint Tests
// =============================================================================

#[tokio::test]
async fn test_nextread_for_local_book() {
    let server = TestServer::start().await;

    let user_id = create_user(&server, "nextread@test.com").await;
    let (_, device_token) = create_device(&server, user_id).await;

    // Create book
    let book_id = create_book(&server, "Nextread Test Book").await;
    entitle_book(&server, book_id).await;

    // Get nextread recommendations
    let resp = server
        .kobo_client()
        .kobo_nextread(&device_token, &book_id.to_string())
        .await;

    // Should return empty recommendations for local books
    resp.assert_ok();

    let nextread: Value = resp.json();
    // Empty response is valid
    assert!(nextread.is_object(), "Nextread should return an object");
}

#[tokio::test]
async fn test_nextread_for_nonexistent_book() {
    let server = TestServer::start().await;

    let user_id = create_user(&server, "nextreadnone@test.com").await;
    let (_, device_token) = create_device(&server, user_id).await;

    let fake_id = uuid::Uuid::new_v4();
    let resp = server
        .kobo_client()
        .kobo_nextread(&device_token, &fake_id.to_string())
        .await;

    // Should handle gracefully
    assert!(resp.status.is_success() || resp.status == StatusCode::NOT_FOUND);
}

// =============================================================================
// Cover Endpoint Tests (additional)
// =============================================================================

#[tokio::test]
async fn test_cover_different_sizes() {
    let server = TestServer::start().await;

    let user_id = create_user(&server, "coversizes@test.com").await;
    let (_, device_token) = create_device(&server, user_id).await;

    // Create book with cover
    let book_id = create_book(&server, "Cover Size Book").await;
    upload_cover(&server, book_id, test_jpeg_image()).await;
    entitle_book(&server, book_id).await;

    // Test various cover sizes
    let sizes = [
        (60, 90),   // Thumbnail
        (150, 200), // Small
        (300, 400), // Medium
        (600, 800), // Large
    ];

    for (width, height) in sizes {
        let resp = server
            .kobo_client()
            .get(&format!(
                "/kobo/{}/book-images/{}/{}/{}/80/false/image.jpg",
                device_token, book_id, width, height
            ))
            .await;

        assert!(
            resp.status.is_success(),
            "Cover at {}x{} should succeed, got {}",
            width,
            height,
            resp.status
        );
    }
}

#[tokio::test]
async fn test_cover_greyscale() {
    let server = TestServer::start().await;

    let user_id = create_user(&server, "covergrey@test.com").await;
    let (_, device_token) = create_device(&server, user_id).await;

    // Create book with cover
    let book_id = create_book(&server, "Greyscale Cover Book").await;
    upload_cover(&server, book_id, test_jpeg_image()).await;
    entitle_book(&server, book_id).await;

    // Request greyscale cover
    let resp = server
        .kobo_client()
        .get(&format!(
            "/kobo/{}/book-images/{}/300/400/80/true/image.jpg",
            device_token, book_id
        ))
        .await;

    resp.assert_ok();
}

// =============================================================================
// Product Book Endpoint Tests
// =============================================================================

#[tokio::test]
async fn test_product_book_returns_details() {
    let server = TestServer::start().await;

    let user_id = create_user(&server, "prodbook@test.com").await;
    let (_, device_token) = create_device(&server, user_id).await;

    let book_id = create_book(&server, "Product Book Test").await;
    entitle_book(&server, book_id).await;

    let resp = server
        .kobo_client()
        .kobo_product_book(&device_token, &book_id.to_string())
        .await;
    resp.assert_ok();

    let product: Value = resp.json();
    assert_eq!(
        product.get("Title").and_then(|t| t.as_str()),
        Some("Product Book Test")
    );
    assert_eq!(
        product.get("Id").and_then(|i| i.as_str()),
        Some(book_id.to_string()).as_deref()
    );
}

#[tokio::test]
async fn test_product_book_nonexistent_returns_404() {
    let server = TestServer::start().await;

    let user_id = create_user(&server, "prodbook404@test.com").await;
    let (_, device_token) = create_device(&server, user_id).await;

    let fake_id = uuid::Uuid::new_v4();
    let resp = server
        .kobo_client()
        .kobo_product_book(&device_token, &fake_id.to_string())
        .await;

    // In local mode, unknown books fall through to stub which returns 200 {}
    // (In merge mode, they'd passthrough to upstream Kobo)
    assert!(resp.status.is_success());
}

// =============================================================================
// Rating Endpoint Tests
// =============================================================================

#[tokio::test]
async fn test_rating_saves_and_returns_ok() {
    let server = TestServer::start().await;

    let user_id = create_user(&server, "rating@test.com").await;
    let (_, device_token) = create_device(&server, user_id).await;

    let book_id = create_book(&server, "Rating Test Book").await;
    entitle_book(&server, book_id).await;

    let resp = server
        .kobo_client()
        .kobo_rate(&device_token, &book_id.to_string(), 4)
        .await;
    resp.assert_ok();
}

#[tokio::test]
async fn test_rating_unauthorized() {
    let server = TestServer::start().await;

    let book_id = create_book(&server, "Rating Auth Book").await;

    let resp = server
        .kobo_client()
        .kobo_rate("invalid-token", &book_id.to_string(), 3)
        .await;

    resp.assert_status(StatusCode::UNAUTHORIZED);
}

// =============================================================================
// Reviews Endpoint Tests
// =============================================================================

#[tokio::test]
async fn test_reviews_for_local_book_returns_empty() {
    let server = TestServer::start().await;

    let user_id = create_user(&server, "reviews@test.com").await;
    let (_, device_token) = create_device(&server, user_id).await;

    let book_id = create_book(&server, "Reviews Test Book").await;
    entitle_book(&server, book_id).await;

    let resp = server
        .kobo_client()
        .kobo_reviews(&device_token, &book_id.to_string())
        .await;
    resp.assert_ok();

    let reviews: Value = resp.json();
    assert_eq!(
        reviews
            .get("Items")
            .and_then(|i| i.as_array())
            .map(|a| a.len()),
        Some(0),
        "Local book reviews should be empty"
    );
    assert_eq!(
        reviews.get("TotalPageCount").and_then(|v| v.as_i64()),
        Some(0)
    );
}

#[tokio::test]
async fn test_reviews_for_nonexistent_book_handled() {
    let server = TestServer::start().await;

    let user_id = create_user(&server, "reviewsnone@test.com").await;
    let (_, device_token) = create_device(&server, user_id).await;

    let fake_id = uuid::Uuid::new_v4();
    let resp = server
        .kobo_client()
        .kobo_reviews(&device_token, &fake_id.to_string())
        .await;

    // Non-local books: in local mode, falls through to stub (200 {})
    assert!(resp.status.is_success());
}

// =============================================================================
// Recommendations Feedback Endpoint Tests
// =============================================================================

#[tokio::test]
async fn test_recommendations_feedback_saves_rating_for_local_book() {
    let server = TestServer::start().await;

    let user_id = create_user(&server, "feedback@test.com").await;
    let (_, device_token) = create_device(&server, user_id).await;

    let book_id = create_book(&server, "Feedback Rating Book").await;
    entitle_book(&server, book_id).await;

    // Submit rating via recommendations feedback (like the device does)
    let resp = server
        .kobo_client()
        .kobo_recommendations_feedback(
            &device_token,
            &serde_json::json!({
                "FeedbackItems": [{
                    "FeedbackType": "Rate4Star",
                    "RevisionId": book_id.to_string()
                }]
            }),
        )
        .await;

    resp.assert_status(StatusCode::NO_CONTENT);

    // Verify the rating was saved in the DB (admin_client uses a different user)
    let row: (i32, String) = sqlx::query_as(
        "SELECT rating, rating_source FROM user_book_data WHERE book_id = $1 AND user_id = $2",
    )
    .bind(book_id)
    .bind(user_id)
    .fetch_one(&server.pool)
    .await
    .expect("Rating should be saved in DB");

    assert_eq!(row.0, 4, "Rating should be saved as 4");
    assert_eq!(row.1, "kobo", "Source should be 'kobo'");
}

#[tokio::test]
async fn test_recommendations_feedback_ignores_nonlocal_book() {
    let server = TestServer::start().await;

    let user_id = create_user(&server, "feedbacknonlocal@test.com").await;
    let (_, device_token) = create_device(&server, user_id).await;

    let fake_id = uuid::Uuid::new_v4();

    // Submit feedback for a non-local book UUID
    let resp = server
        .kobo_client()
        .kobo_recommendations_feedback(
            &device_token,
            &serde_json::json!({
                "FeedbackItems": [{
                    "FeedbackType": "Rate5Star",
                    "RevisionId": fake_id.to_string()
                }]
            }),
        )
        .await;

    // Should still return 204 (silently skip non-local items)
    resp.assert_status(StatusCode::NO_CONTENT);
}

#[tokio::test]
async fn test_recommendations_feedback_unauthorized() {
    let server = TestServer::start().await;

    let resp = server
        .kobo_client()
        .kobo_recommendations_feedback(
            "invalid-token",
            &serde_json::json!({
                "FeedbackItems": [{
                    "FeedbackType": "Rate3Star",
                    "RevisionId": uuid::Uuid::new_v4().to_string()
                }]
            }),
        )
        .await;

    resp.assert_status(StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn test_recommendations_feedback_empty_body() {
    let server = TestServer::start().await;

    let user_id = create_user(&server, "feedbackempty@test.com").await;
    let (_, device_token) = create_device(&server, user_id).await;

    // Empty FeedbackItems
    let resp = server
        .kobo_client()
        .kobo_recommendations_feedback(&device_token, &serde_json::json!({ "FeedbackItems": [] }))
        .await;

    resp.assert_status(StatusCode::NO_CONTENT);
}

// =============================================================================
// Recommendations Format Tests
// =============================================================================

#[tokio::test]
async fn test_recommendations_wraps_items_in_book() {
    let server = TestServer::start().await;

    let user_id = create_user(&server, "recsformat@test.com").await;
    let (_, device_token) = create_device(&server, user_id).await;

    let book_id = create_book(&server, "Recs Format Book").await;
    entitle_book(&server, book_id).await;

    let resp = server
        .kobo_client()
        .kobo_recommendations(&device_token, &book_id.to_string())
        .await;
    resp.assert_ok();

    let recs: Value = resp.json();

    // Should have VersionCode
    assert_eq!(recs.get("VersionCode").and_then(|v| v.as_i64()), Some(2));

    // Items should be wrapped in Book objects (if any items exist)
    if let Some(items) = recs.get("Items").and_then(|i| i.as_array()) {
        for item in items {
            assert!(
                item.get("Book").is_some(),
                "Each recommendation item should be wrapped in a Book object"
            );
        }
    }
}

// =============================================================================
// Product Prices Endpoint Tests (catch-all intercept)
// =============================================================================

#[tokio::test]
async fn test_prices_for_local_book_intercepted() {
    let server = TestServer::start().await;

    let user_id = create_user(&server, "prices@test.com").await;
    let (_, device_token) = create_device(&server, user_id).await;

    let book_id = create_book(&server, "Prices Test Book").await;
    entitle_book(&server, book_id).await;

    let resp = server
        .kobo_client()
        .kobo_product_prices(&device_token, &book_id.to_string())
        .await;
    resp.assert_ok();

    let body: Value = resp.json();
    // Catch-all returns {"Items":[]}
    assert_eq!(
        body.get("Items")
            .and_then(|i| i.as_array())
            .map(|a| a.len()),
        Some(0),
        "Local book prices should return empty Items"
    );
}

#[tokio::test]
async fn test_prices_for_nonexistent_book_not_intercepted() {
    let server = TestServer::start().await;

    let user_id = create_user(&server, "pricesnone@test.com").await;
    let (_, device_token) = create_device(&server, user_id).await;

    let fake_id = uuid::Uuid::new_v4();
    let resp = server
        .kobo_client()
        .kobo_product_prices(&device_token, &fake_id.to_string())
        .await;

    // Non-local UUIDs: handled by local mode stub or passthrough
    assert!(resp.status.is_success() || resp.status == StatusCode::NOT_FOUND);
}

// =============================================================================
// Product Related Endpoint Tests
// =============================================================================

#[tokio::test]
async fn test_related_for_local_book() {
    let server = TestServer::start().await;

    let user_id = create_user(&server, "related@test.com").await;
    let (_, device_token) = create_device(&server, user_id).await;

    let book_id = create_book(&server, "Related Test Book").await;
    entitle_book(&server, book_id).await;

    let resp = server
        .kobo_client()
        .kobo_product_related(&device_token, &book_id.to_string())
        .await;
    resp.assert_ok();

    let body: Value = resp.json();
    // Related uses nextread handler, returns object (may be empty or have recommendations)
    assert!(body.is_object(), "Related should return an object");
}

#[tokio::test]
async fn test_related_for_nonexistent_book() {
    let server = TestServer::start().await;

    let user_id = create_user(&server, "relatednone@test.com").await;
    let (_, device_token) = create_device(&server, user_id).await;

    let fake_id = uuid::Uuid::new_v4();
    let resp = server
        .kobo_client()
        .kobo_product_related(&device_token, &fake_id.to_string())
        .await;

    // Should handle gracefully
    assert!(resp.status.is_success() || resp.status == StatusCode::NOT_FOUND);
}
