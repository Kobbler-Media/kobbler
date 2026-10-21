use crate::common::*;
use reqwest::StatusCode;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sqlx::Row;
use std::io::Write;
use tempfile::TempDir;
use uuid::Uuid;

/// Response type for ingest queue list endpoint (grouped format)
#[derive(Debug, Deserialize, Serialize)]
struct IngestQueueListResponse {
    groups: Vec<IngestQueueGroupResponse>,
    total_groups: i64,
    status_counts: std::collections::HashMap<String, i64>,
}

#[derive(Debug, Deserialize, Serialize)]
struct IngestQueueGroupResponse {
    group_key: String,
    items: Vec<Value>,
    formats: Vec<String>,
    primary_item_id: String,
    display_status: String,
}

// =============================================================================
// Ingest Queue Tests
// =============================================================================

#[tokio::test]
async fn test_list_ingest_queue_empty() {
    let server = TestServer::start_api_only().await;

    let resp = server
        .admin_client()
        .get("/api/v1/admin/ingest-queue")
        .await;
    resp.assert_ok();

    let response: IngestQueueListResponse = resp.json();
    assert!(response.groups.is_empty());
    assert_eq!(response.total_groups, 0);
}

#[tokio::test]
async fn test_ingest_queue_item_visibility() {
    let server = TestServer::start_api_only().await;
    let watch_dir = TempDir::new().unwrap();

    // Create a test KEPUB file
    let book_path = watch_dir.path().join("test_book.kepub.epub");
    std::fs::File::create(&book_path)
        .unwrap()
        .write_all(&test_epub_bytes())
        .unwrap();

    // Add item directly to queue (bypasses async job processing)
    let item_id = add_queue_item(
        &server,
        book_path.to_str().unwrap(),
        "test_book.kepub.epub",
        "KEPUB",
    )
    .await;

    // Verify item appears in queue
    let queue_resp = server
        .admin_client()
        .get("/api/v1/admin/ingest-queue")
        .await;
    queue_resp.assert_ok();

    let response: IngestQueueListResponse = queue_resp.json();
    assert_eq!(response.groups.len(), 1);
    assert_eq!(response.groups[0].items.len(), 1);
    assert_eq!(
        response.groups[0].items[0]["ingest_info"]["file_name"],
        "test_book.kepub.epub"
    );
    assert_eq!(response.groups[0].items[0]["ingest_status"], "pending");
    assert_eq!(response.groups[0].items[0]["id"], item_id.to_string());
}

// Note: Deduplication by hash is tested at the job execution level, not via API
// The scan endpoint now creates jobs instead of scanning directly

#[tokio::test]
async fn test_get_ingest_queue_item() {
    let server = TestServer::start_api_only().await;
    let watch_dir = TempDir::new().unwrap();

    // Create a book file
    let book_path = watch_dir.path().join("get_test.kepub.epub");
    std::fs::File::create(&book_path)
        .unwrap()
        .write_all(&test_epub_bytes())
        .unwrap();

    // Add item directly to queue (bypasses async job processing)
    let item_id = add_queue_item(
        &server,
        book_path.to_str().unwrap(),
        "get_test.kepub.epub",
        "KEPUB",
    )
    .await;
    let item_id = item_id.to_string();

    // Get specific item
    let item_resp = server
        .admin_client()
        .get(&format!("/api/v1/admin/ingest-queue/{}", item_id))
        .await;
    item_resp.assert_ok();

    let item: Value = item_resp.json();
    assert_eq!(item["ingest_info"]["file_name"], "get_test.kepub.epub");
}

#[tokio::test]
async fn test_update_ingest_queue_item() {
    let server = TestServer::start_api_only().await;
    let watch_dir = TempDir::new().unwrap();

    // Create a book file
    let book_path = watch_dir.path().join("update_test.kepub.epub");
    std::fs::File::create(&book_path)
        .unwrap()
        .write_all(&test_epub_bytes())
        .unwrap();

    // Add item directly to queue (bypasses async job processing)
    let item_id = add_queue_item(
        &server,
        book_path.to_str().unwrap(),
        "update_test.kepub.epub",
        "KEPUB",
    )
    .await;
    let item_id = item_id.to_string();

    // Update item metadata via library PATCH (works for both library and ingest items)
    let update_resp = server
        .admin_client()
        .patch_json(
            &format!("/api/v1/library/books/{}", item_id),
            &json!({
                "title": "Updated Title",
                "authors": [{"name": "New Author"}]
            }),
        )
        .await;
    update_resp.assert_success();

    // Re-fetch to verify changes
    let item_resp = server
        .admin_client()
        .get(&format!("/api/v1/admin/ingest-queue/{}", item_id))
        .await;
    let updated: Value = item_resp.json();
    assert_eq!(updated["title"], "Updated Title");
    // Authors are now returned as [{id, name}] - "New Author" is pending (nil UUID)
    let authors = updated["authors"]
        .as_array()
        .expect("authors should be array");
    assert!(
        authors.iter().any(|a| a["name"] == "New Author"),
        "Should contain 'New Author' in authors: {:?}",
        authors
    );
}

#[tokio::test]
async fn test_delete_ingest_queue_item() {
    let server = TestServer::start_api_only().await;
    let watch_dir = TempDir::new().unwrap();

    // Create a book file
    let book_path = watch_dir.path().join("delete_test.kepub.epub");
    std::fs::File::create(&book_path)
        .unwrap()
        .write_all(&test_epub_bytes())
        .unwrap();

    // Add item directly to queue (bypasses async job processing)
    let item_id = add_queue_item(
        &server,
        book_path.to_str().unwrap(),
        "delete_test.kepub.epub",
        "KEPUB",
    )
    .await;
    let item_id = item_id.to_string();

    // Delete item
    let delete_resp = server
        .admin_client()
        .delete(&format!("/api/v1/admin/ingest-queue/{}", item_id))
        .await;
    delete_resp.assert_status(StatusCode::NO_CONTENT);

    // Verify it's gone
    let get_resp = server
        .admin_client()
        .get(&format!("/api/v1/admin/ingest-queue/{}", item_id))
        .await;
    get_resp.assert_status(StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn test_reject_ingest_item() {
    let server = TestServer::start_api_only().await;
    let watch_dir = TempDir::new().unwrap();

    // Create a book file
    let book_path = watch_dir.path().join("reject_test.kepub.epub");
    std::fs::File::create(&book_path)
        .unwrap()
        .write_all(&test_epub_bytes())
        .unwrap();

    // Add item directly to queue (bypasses async job processing)
    let item_id = add_queue_item(
        &server,
        book_path.to_str().unwrap(),
        "reject_test.kepub.epub",
        "KEPUB",
    )
    .await;
    let item_id = item_id.to_string();

    // Reject item
    let reject_resp = server
        .admin_client()
        .post_json(
            &format!("/api/v1/admin/ingest-queue/{}/reject", item_id),
            &json!({}),
        )
        .await;
    reject_resp.assert_success();

    // Verify status is rejected
    let item_resp = server
        .admin_client()
        .get(&format!("/api/v1/admin/ingest-queue/{}", item_id))
        .await;
    let item: Value = item_resp.json();
    assert_eq!(item["ingest_status"], "rejected");
}

// =============================================================================
// Approval Workflow Tests
// =============================================================================

#[tokio::test]
async fn test_approve_ingest_item_creates_book() {
    let server = TestServer::start_api_only().await;
    let watch_dir = TempDir::new().unwrap();

    // Create a test KEPUB file (KEPUB files don't need conversion)
    let book_path = watch_dir.path().join("approve_test.kepub.epub");
    std::fs::File::create(&book_path)
        .unwrap()
        .write_all(&test_epub_bytes())
        .unwrap();

    // Add item directly to queue (bypasses async job processing)
    let item_id = add_queue_item(
        &server,
        book_path.to_str().unwrap(),
        "approve_test.kepub.epub",
        "KEPUB",
    )
    .await;
    let item_id = item_id.to_string();

    // Update with title/authors (using clean field names)
    server
        .admin_client()
        .patch_json(
            &format!("/api/v1/admin/ingest-queue/{}", item_id),
            &json!({
                "title": "Approved Book",
                "authors": [{"name": "Test Author"}]
            }),
        )
        .await;

    // Approve
    let approve_resp = server
        .admin_client()
        .post_json(
            &format!("/api/v1/admin/ingest-queue/{}/approve", item_id),
            &json!({}),
        )
        .await;
    approve_resp.assert_ok();

    let approve_json: Value = approve_resp.json();
    assert!(approve_json["book_id"].is_string());

    let book_id: Uuid = approve_json["book_id"].as_str().unwrap().parse().unwrap();

    // After approval, item is no longer in the ingest queue (it's a library book now)
    let item_resp = server
        .admin_client()
        .get(&format!("/api/v1/admin/ingest-queue/{}", item_id))
        .await;
    item_resp.assert_status(StatusCode::NOT_FOUND);

    // Verify book exists in library
    let books_resp = server.admin_client().get("/api/v1/library/books").await;
    books_resp.assert_ok();

    let books: Value = books_resp.json();
    let book_ids: Vec<&str> = books["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|b| b["id"].as_str().unwrap())
        .collect();
    assert!(book_ids.contains(&book_id.to_string().as_str()));
}

#[tokio::test]
async fn test_approve_item_with_cover() {
    let server = TestServer::start_api_only().await;
    let watch_dir = TempDir::new().unwrap();

    // Create a test KEPUB file
    let book_path = watch_dir.path().join("cover_test.kepub.epub");
    std::fs::File::create(&book_path)
        .unwrap()
        .write_all(&test_epub_bytes())
        .unwrap();

    // Add item directly to queue (bypasses async job processing)
    let item_id = add_queue_item(
        &server,
        book_path.to_str().unwrap(),
        "cover_test.kepub.epub",
        "KEPUB",
    )
    .await;

    // Store cover directly in the queue item via database
    let cover_bytes = test_jpeg_image();
    sqlx::query("UPDATE book_ingest_source SET extracted_cover = $1 WHERE book_id = $2")
        .bind(&cover_bytes)
        .bind(item_id)
        .execute(&server.pool)
        .await
        .unwrap();

    let item_id = item_id.to_string();

    // Set title and approve (using clean field name)
    server
        .admin_client()
        .patch_json(
            &format!("/api/v1/admin/ingest-queue/{}", item_id),
            &json!({
                "title": "Book With Cover"
            }),
        )
        .await;

    let approve_resp = server
        .admin_client()
        .post_json(
            &format!("/api/v1/admin/ingest-queue/{}/approve", item_id),
            &json!({}),
        )
        .await;
    approve_resp.assert_ok();

    let book_id: Uuid = approve_resp.json::<Value>()["book_id"]
        .as_str()
        .unwrap()
        .parse()
        .unwrap();

    // Verify the book exists with a cover by trying to fetch the cover
    let cover_resp = server
        .admin_client()
        .get(&format!("/api/v1/library/books/{}/cover", book_id))
        .await;
    // Cover should be fetchable if it was stored (200 OK with image data)
    cover_resp.assert_ok();
}

#[tokio::test]
async fn test_queue_status_filter() {
    let server = TestServer::start_api_only().await;
    let watch_dir = TempDir::new().unwrap();

    // Create multiple test files
    for name in &["book1.kepub.epub", "book2.kepub.epub", "book3.kepub.epub"] {
        let path = watch_dir.path().join(name);
        std::fs::File::create(&path)
            .unwrap()
            .write_all(&test_epub_bytes())
            .unwrap();
    }

    // Add items directly to queue (bypasses async job processing)
    let mut item_ids = Vec::new();
    for name in &["book1.kepub.epub", "book2.kepub.epub", "book3.kepub.epub"] {
        let path = watch_dir.path().join(name);
        let id = add_queue_item(&server, path.to_str().unwrap(), name, "KEPUB").await;
        item_ids.push(id);
    }

    // Get all items (each book is in its own group since they have different basenames)
    let queue_resp = server
        .admin_client()
        .get("/api/v1/admin/ingest-queue")
        .await;
    let response: IngestQueueListResponse = queue_resp.json();
    assert_eq!(response.groups.len(), 3);

    // Reject one item
    let reject_id = item_ids[0].to_string();
    server
        .admin_client()
        .post_json(
            &format!("/api/v1/admin/ingest-queue/{}/reject", reject_id),
            &json!({}),
        )
        .await;

    // Filter by pending status
    let pending_resp = server
        .admin_client()
        .get("/api/v1/admin/ingest-queue?status=pending")
        .await;
    pending_resp.assert_ok();

    let pending_response: IngestQueueListResponse = pending_resp.json();
    assert_eq!(pending_response.groups.len(), 2);
    for group in &pending_response.groups {
        for item in &group.items {
            assert_eq!(item["ingest_status"], "pending");
        }
    }

    // Filter by rejected status
    let rejected_resp = server
        .admin_client()
        .get("/api/v1/admin/ingest-queue?status=rejected")
        .await;
    rejected_resp.assert_ok();

    let rejected_response: IngestQueueListResponse = rejected_resp.json();
    assert_eq!(rejected_response.groups.len(), 1);
    assert_eq!(rejected_response.groups[0].items.len(), 1);
    assert_eq!(
        rejected_response.groups[0].items[0]["ingest_status"],
        "rejected"
    );
}

#[tokio::test]
async fn test_ingest_queue_requires_auth() {
    let server = TestServer::start_api_only().await;
    let client = TestClient::new(&server.api_url);

    client
        .get("/api/v1/admin/ingest-queue")
        .await
        .assert_status(StatusCode::UNAUTHORIZED);

    client
        .post_json(
            &format!("/api/v1/admin/ingest-queue/{}/approve", Uuid::new_v4()),
            &json!({}),
        )
        .await
        .assert_status(StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn test_approve_nonexistent_item() {
    let server = TestServer::start_api_only().await;

    let resp = server
        .admin_client()
        .post_json(
            &format!("/api/v1/admin/ingest-queue/{}/approve", Uuid::new_v4()),
            &json!({}),
        )
        .await;
    resp.assert_status(StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn test_reject_nonexistent_item() {
    let server = TestServer::start_api_only().await;

    let resp = server
        .admin_client()
        .post_json(
            &format!("/api/v1/admin/ingest-queue/{}/reject", Uuid::new_v4()),
            &json!({}),
        )
        .await;
    resp.assert_status(StatusCode::NOT_FOUND);
}

// =============================================================================
// Metadata Preservation Tests
// =============================================================================

#[tokio::test]
async fn test_approve_preserves_series_metadata() {
    let server = TestServer::start_api_only().await;
    let watch_dir = TempDir::new().unwrap();

    // Create a test KEPUB file
    let book_path = watch_dir.path().join("series_test.kepub.epub");
    std::fs::File::create(&book_path)
        .unwrap()
        .write_all(&test_epub_bytes())
        .unwrap();

    // Add item directly to queue
    let item_id = add_queue_item(
        &server,
        book_path.to_str().unwrap(),
        "series_test.kepub.epub",
        "KEPUB",
    )
    .await;

    // Update with series metadata via library PATCH
    let update_resp = server
        .admin_client()
        .patch_json(
            &format!("/api/v1/library/books/{}", item_id),
            &json!({
                "title": "Book in Series",
                "authors": [{"name": "Series Author"}],
                "series_name": "The Test Series",
                "series_index": 2.5
            }),
        )
        .await;
    update_resp.assert_success();

    // Approve
    let approve_resp = server
        .admin_client()
        .post_json(
            &format!("/api/v1/admin/ingest-queue/{}/approve", item_id),
            &json!({}),
        )
        .await;
    approve_resp.assert_ok();

    let book_id: Uuid = approve_resp.json::<Value>()["book_id"]
        .as_str()
        .unwrap()
        .parse()
        .unwrap();

    // Verify the book was created with series metadata via junction table
    let row = sqlx::query(
        r#"
        SELECT b.title, bs.series_id, bs.series_index, s.name as series_name
        FROM books b
        LEFT JOIN book_series bs ON bs.book_id = b.id AND bs.is_primary = TRUE
        LEFT JOIN series s ON s.id = bs.series_id
        WHERE b.id = $1
        "#,
    )
    .bind(book_id)
    .fetch_one(&server.pool)
    .await
    .unwrap();

    assert_eq!(row.get::<String, _>("title"), "Book in Series");
    assert!(
        row.get::<Option<Uuid>, _>("series_id").is_some(),
        "Book should have series_id"
    );
    assert_eq!(
        row.get::<Option<String>, _>("series_name").unwrap(),
        "The Test Series"
    );
    assert!((row.get::<Option<f64>, _>("series_index").unwrap() - 2.5).abs() < 0.001);
}

#[tokio::test]
async fn test_approve_preserves_language_publisher_pubdate() {
    let server = TestServer::start_api_only().await;
    let watch_dir = TempDir::new().unwrap();

    // Create a test KEPUB file
    let book_path = watch_dir.path().join("metadata_test.kepub.epub");
    std::fs::File::create(&book_path)
        .unwrap()
        .write_all(&test_epub_bytes())
        .unwrap();

    // Add item directly to queue
    let item_id = add_queue_item(
        &server,
        book_path.to_str().unwrap(),
        "metadata_test.kepub.epub",
        "KEPUB",
    )
    .await;

    // Update with additional metadata via database (since the API endpoint may not support all fields)
    sqlx::query(
        r#"
        UPDATE books
        SET title = $1,
            language = $2,
            publisher = $3,
            pubdate = $4
        WHERE id = $5
        "#,
    )
    .bind("Full Metadata Book")
    .bind("en")
    .bind("Test Publisher")
    .bind(time::OffsetDateTime::from_unix_timestamp(1704067200).unwrap()) // 2024-01-01
    .bind(item_id)
    .execute(&server.pool)
    .await
    .unwrap();

    // Approve
    let approve_resp = server
        .admin_client()
        .post_json(
            &format!("/api/v1/admin/ingest-queue/{}/approve", item_id),
            &json!({}),
        )
        .await;
    approve_resp.assert_ok();

    let book_id: Uuid = approve_resp.json::<Value>()["book_id"]
        .as_str()
        .unwrap()
        .parse()
        .unwrap();

    // Verify the book was created with all metadata via database
    let row = sqlx::query(
        r#"
        SELECT title, language, publisher, pubdate
        FROM books
        WHERE id = $1
        "#,
    )
    .bind(book_id)
    .fetch_one(&server.pool)
    .await
    .unwrap();

    assert_eq!(row.get::<String, _>("title"), "Full Metadata Book");
    assert_eq!(row.get::<Option<String>, _>("language").unwrap(), "en");
    assert_eq!(
        row.get::<Option<String>, _>("publisher").unwrap(),
        "Test Publisher"
    );
    assert!(
        row.get::<Option<time::OffsetDateTime>, _>("pubdate")
            .is_some(),
        "pubdate should be set"
    );
}

#[tokio::test]
async fn test_approve_matches_series_by_author() {
    let server = TestServer::start_api_only().await;
    let watch_dir = TempDir::new().unwrap();

    // Create first book
    let book1_path = watch_dir.path().join("series_book1.kepub.epub");
    std::fs::File::create(&book1_path)
        .unwrap()
        .write_all(&test_epub_bytes())
        .unwrap();

    let item1_id = add_queue_item(
        &server,
        book1_path.to_str().unwrap(),
        "series_book1.kepub.epub",
        "KEPUB",
    )
    .await;

    // Set up first book with series via library PATCH
    server
        .admin_client()
        .patch_json(
            &format!("/api/v1/library/books/{}", item1_id),
            &json!({
                "title": "Series Book 1",
                "authors": [{"name": "Shared Author"}],
                "series_name": "Author's Series",
                "series_index": 1.0
            }),
        )
        .await;

    let approve1_resp = server
        .admin_client()
        .post_json(
            &format!("/api/v1/admin/ingest-queue/{}/approve", item1_id),
            &json!({}),
        )
        .await;
    approve1_resp.assert_ok();

    let book1_id: Uuid = approve1_resp.json::<Value>()["book_id"]
        .as_str()
        .unwrap()
        .parse()
        .unwrap();

    // Create second book with same series name and author
    let book2_path = watch_dir.path().join("series_book2.kepub.epub");
    std::fs::File::create(&book2_path)
        .unwrap()
        .write_all(&test_epub_bytes())
        .unwrap();

    let item2_id = add_queue_item(
        &server,
        book2_path.to_str().unwrap(),
        "series_book2.kepub.epub",
        "KEPUB",
    )
    .await;

    server
        .admin_client()
        .patch_json(
            &format!("/api/v1/library/books/{}", item2_id),
            &json!({
                "title": "Series Book 2",
                "authors": [{"name": "Shared Author"}],
                "series_name": "Author's Series",
                "series_index": 2.0
            }),
        )
        .await;

    let approve2_resp = server
        .admin_client()
        .post_json(
            &format!("/api/v1/admin/ingest-queue/{}/approve", item2_id),
            &json!({}),
        )
        .await;
    approve2_resp.assert_ok();

    let book2_id: Uuid = approve2_resp.json::<Value>()["book_id"]
        .as_str()
        .unwrap()
        .parse()
        .unwrap();

    // Verify both books have the same series_id via junction table
    let book1_row = sqlx::query(
        "SELECT series_id, series_index FROM book_series WHERE book_id = $1 AND is_primary = TRUE",
    )
    .bind(book1_id)
    .fetch_one(&server.pool)
    .await
    .unwrap();

    let book2_row = sqlx::query(
        "SELECT series_id, series_index FROM book_series WHERE book_id = $1 AND is_primary = TRUE",
    )
    .bind(book2_id)
    .fetch_one(&server.pool)
    .await
    .unwrap();

    let series1_id = book1_row.get::<Uuid, _>("series_id");
    let series2_id = book2_row.get::<Uuid, _>("series_id");

    assert_eq!(
        series1_id, series2_id,
        "Both books should share the same series"
    );
    assert!((book1_row.get::<Option<f64>, _>("series_index").unwrap() - 1.0).abs() < 0.001);
    assert!((book2_row.get::<Option<f64>, _>("series_index").unwrap() - 2.0).abs() < 0.001);
}
