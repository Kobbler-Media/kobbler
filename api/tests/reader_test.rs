use crate::common::*;
use api::repositories::book_files;
use reqwest::StatusCode;
use serde_json::{Value, json};

#[tokio::test]
async fn test_book_file_download_requires_auth() {
    let server = TestServer::start_api_only().await;
    let book_id = create_book(&server, "Test Book").await;
    upload_epub(&server, book_id, test_epub_bytes()).await;

    // Unauthenticated request should fail
    let resp = server
        .api_client()
        .get(&format!("/api/v1/admin/books/{}/file", book_id))
        .await;
    resp.assert_status(StatusCode::UNAUTHORIZED);

    // Authenticated request should succeed
    let resp = server
        .admin_client()
        .get(&format!("/api/v1/admin/books/{}/file", book_id))
        .await;
    resp.assert_ok();
}

#[tokio::test]
async fn test_book_file_download_not_found() {
    let server = TestServer::start_api_only().await;
    let book_id = create_book(&server, "No File Book").await;

    let resp = server
        .admin_client()
        .get(&format!("/api/v1/admin/books/{}/file", book_id))
        .await;
    resp.assert_status(StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn test_save_reader_progress_creates_web_reader_device() {
    let server = TestServer::start_api_only().await;
    let book_id = create_book(&server, "Reading Book").await;
    upload_epub(&server, book_id, test_epub_bytes()).await;

    // Save progress
    let resp = server
        .admin_client()
        .post_json(
            &format!("/api/v1/admin/books/{}/reader-progress", book_id),
            &json!({
                "progress_percent": 42.5,
                "cfi": "epubcfi(/6/14[ch07]!/4/2/1:0)"
            }),
        )
        .await;
    resp.assert_status(StatusCode::NO_CONTENT);

    // Verify a web reader device was created (hidden from API device list, so check DB directly)
    let exists: bool =
        sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM devices WHERE device_type = 'web')")
            .fetch_one(&server.pool)
            .await
            .unwrap();
    assert!(exists, "web reader device should be created");
}

#[tokio::test]
async fn test_get_reader_progress() {
    let server = TestServer::start_api_only().await;
    let book_id = create_book(&server, "Progress Book").await;
    upload_epub(&server, book_id, test_epub_bytes()).await;

    // Initially no progress
    let resp = server
        .admin_client()
        .get(&format!("/api/v1/admin/books/{}/reader-progress", book_id))
        .await;
    resp.assert_ok();
    let data: Value = resp.json();
    assert!(data["progress_percent"].is_null());
    assert!(data["cfi"].is_null());

    // Save progress
    server
        .admin_client()
        .post_json(
            &format!("/api/v1/admin/books/{}/reader-progress", book_id),
            &json!({
                "progress_percent": 55.0,
                "cfi": "epubcfi(/6/10)"
            }),
        )
        .await
        .assert_status(StatusCode::NO_CONTENT);

    // Retrieve saved progress
    let resp = server
        .admin_client()
        .get(&format!("/api/v1/admin/books/{}/reader-progress", book_id))
        .await;
    resp.assert_ok();
    let data: Value = resp.json();
    assert_eq!(data["progress_percent"], 55.0);
    assert_eq!(data["cfi"], "epubcfi(/6/10)");
}

#[tokio::test]
async fn test_save_progress_requires_auth() {
    let server = TestServer::start_api_only().await;
    let book_id = create_book(&server, "Auth Test Book").await;

    let resp = server
        .api_client()
        .post_json(
            &format!("/api/v1/admin/books/{}/reader-progress", book_id),
            &json!({ "progress_percent": 10.0 }),
        )
        .await;
    resp.assert_status(StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn test_book_file_download_on_demand_epub_conversion() {
    let server = TestServer::start_api_only().await;
    let book_id = create_book(&server, "MOBI Only Book 2").await;

    // Insert a MOBI book_file record (no actual file on disk)
    book_files::create(
        &server.pool,
        book_files::BookFileInput {
            book_id,
            format: "MOBI".to_string(),
            size_bytes: 1234,
            file_path: "books/fake/nonexistent.mobi".to_string(),
            source_path: None,
            source_mtime: None,
            file_hash: None,
            source_type: Some("test".to_string()),
            source_metadata: None,
            ingest_queue_id: None,
        },
    )
    .await
    .unwrap();

    // Explicitly request EPUB — no EPUB exists, conversion should be attempted
    // but source file doesn't exist on disk, so it gracefully returns 404
    let resp = server
        .admin_client()
        .get(&format!("/api/v1/admin/books/{}/file?format=EPUB", book_id))
        .await;
    resp.assert_status(StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn test_book_file_download_no_convertible_source() {
    let server = TestServer::start_api_only().await;
    let book_id = create_book(&server, "CBZ Only Book").await;

    // CBZ is not convertible to EPUB
    book_files::create(
        &server.pool,
        book_files::BookFileInput {
            book_id,
            format: "CBZ".to_string(),
            size_bytes: 5678,
            file_path: "books/fake/comic.cbz".to_string(),
            source_path: None,
            source_mtime: None,
            file_hash: None,
            source_type: Some("test".to_string()),
            source_metadata: None,
            ingest_queue_id: None,
        },
    )
    .await
    .unwrap();

    // Request EPUB explicitly — CBZ can't be converted, should 404
    let resp = server
        .admin_client()
        .get(&format!("/api/v1/admin/books/{}/file?format=EPUB", book_id))
        .await;
    resp.assert_status(StatusCode::NOT_FOUND);
}
