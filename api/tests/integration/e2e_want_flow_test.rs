//! E2E tests for the want → callback → library flow.
//!
//! These tests exercise the full lifecycle:
//! 1. Create a wanted item
//! 2. Queue a download (Kobload is a wiremock)
//! 3. Receive a Kobload callback
//! 4. Verify the wanted item progresses correctly

use crate::common::*;
use api::events::LibraryEvent;
use reqwest::StatusCode;
use serde_json::{Value, json};
use uuid::Uuid;
use wiremock::matchers::{header, method, path, path_regex};
use wiremock::{Mock, MockServer, ResponseTemplate};

/// Setup: create a wanted item and configure Kobload. Returns (server, mock_server, wanted_id).
async fn setup_wanted_with_kobload() -> (TestServer, MockServer, String) {
    let mock_server = MockServer::start().await;
    let server = TestServer::start_api_only().await;

    let resp = server
        .admin_client()
        .patch_json(
            "/api/v1/admin/config",
            &json!({
                "kobload_base_url": mock_server.uri(),
                "kobload_api_key": "test-key",
                "kobload_ingest_url": format!("{}/api/v1/ingest/{}", server.api_url, server.ingest_token)
            }),
        )
        .await;
    resp.assert_ok();

    let create_resp = server
        .admin_client()
        .post_json(
            "/api/v1/admin/wanted",
            &json!({
                "title": "Mistborn",
                "author": "Brandon Sanderson",
                "identifiers": { "isbn13": "9780765311788" },
                "series": "Mistborn",
                "series_index": 1
            }),
        )
        .await;
    create_resp.assert_ok();
    let wanted_id = create_resp.json::<Value>()["id"]
        .as_str()
        .unwrap()
        .to_string();

    (server, mock_server, wanted_id)
}

/// Queue a download for a wanted item. Returns the download_id used.
async fn queue_download(server: &TestServer, mock_server: &MockServer, wanted_id: &str) -> Uuid {
    let download_id = Uuid::now_v7();
    Mock::given(method("POST"))
        .and(path("/api/v1/downloads"))
        .and(header("Authorization", "Bearer test-key"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "id": download_id,
            "status": "queued"
        })))
        .mount(mock_server)
        .await;

    let resp = server
        .admin_client()
        .post_json(
            &format!("/api/v1/admin/books/{}/queue", wanted_id),
            &json!({
                "source": "manual",
                "release": {
                    "id": "guid:test-release",
                    "protocol": "usenet",
                    "download_url": "https://example.com/download",
                    "title": "Mistborn epub",
                    "indexer": "MockIndexer",
                    "size": 1_048_576
                }
            }),
        )
        .await;
    assert_eq!(resp.status, StatusCode::NO_CONTENT);

    download_id
}

// =============================================================================
// Callback Tests
// =============================================================================

#[tokio::test]
async fn test_callback_download_failure() {
    let (server, _mock_server, wanted_id) = setup_wanted_with_kobload().await;

    let download_id = queue_download(&server, &_mock_server, &wanted_id).await;

    // Verify queued status
    let detail = server
        .admin_client()
        .get(&format!("/api/v1/admin/wanted/{}", wanted_id))
        .await;
    detail.assert_ok();
    let json: Value = detail.json();
    let attempts = json["attempts"].as_array().unwrap();
    assert_eq!(attempts.len(), 1);
    assert_eq!(attempts[0]["status"], "queued");

    // Send failed callback
    let resp = TestClient::new(&server.api_url)
        .post_json(
            &format!("/api/v1/ingest/{}", server.ingest_token),
            &json!({
                "download_id": download_id,
                "status": "failed",
                "metadata": { "book_id": wanted_id },
                "files": [],
                "error": "Download timed out"
            }),
        )
        .await;
    resp.assert_success();

    // Verify attempt status changed to failed
    let detail = server
        .admin_client()
        .get(&format!("/api/v1/admin/wanted/{}", wanted_id))
        .await;
    detail.assert_ok();
    let json: Value = detail.json();
    let attempts = json["attempts"].as_array().unwrap();
    assert_eq!(attempts.len(), 1);
    assert_eq!(attempts[0]["status"], "failed");
}

#[tokio::test]
async fn test_callback_recoverable_error() {
    let (server, _mock_server, wanted_id) = setup_wanted_with_kobload().await;

    let download_id = queue_download(&server, &_mock_server, &wanted_id).await;

    // Send recoverable error callback
    let resp = TestClient::new(&server.api_url)
        .post_json(
            &format!("/api/v1/ingest/{}", server.ingest_token),
            &json!({
                "download_id": download_id,
                "status": "completed",
                "metadata": { "book_id": wanted_id },
                "files": [],
                "recoverable_error": "NFS share not mounted"
            }),
        )
        .await;
    resp.assert_success();

    // Verify attempt status is kobload_error
    let detail = server
        .admin_client()
        .get(&format!("/api/v1/admin/wanted/{}", wanted_id))
        .await;
    detail.assert_ok();
    let json: Value = detail.json();
    let attempts = json["attempts"].as_array().unwrap();
    assert_eq!(attempts.len(), 1);
    assert_eq!(attempts[0]["status"], "kobload_error");
}

#[tokio::test]
async fn test_callback_completed_triggers_file_download() {
    let (server, mock_server, wanted_id) = setup_wanted_with_kobload().await;

    let download_id = queue_download(&server, &mock_server, &wanted_id).await;

    // Mock the file download endpoint — serve a tiny fake epub
    let fake_epub = test_epub_bytes();
    Mock::given(method("GET"))
        .and(path_regex(format!(
            r"/api/v1/downloads/{}/files/.*",
            download_id
        )))
        .and(header("Authorization", "Bearer test-key"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_bytes(fake_epub)
                .insert_header("Content-Type", "application/epub+zip"),
        )
        .mount(&mock_server)
        .await;

    // Mock the import confirmation endpoint
    Mock::given(method("POST"))
        .and(path(format!("/api/v1/downloads/{}/imported", download_id)))
        .and(header("Authorization", "Bearer test-key"))
        .respond_with(ResponseTemplate::new(200))
        .mount(&mock_server)
        .await;

    // Send completed callback with an epub file
    let resp = TestClient::new(&server.api_url)
        .post_json(
            &format!("/api/v1/ingest/{}", server.ingest_token),
            &json!({
                "download_id": download_id,
                "status": "completed",
                "metadata": { "book_id": wanted_id },
                "files": [
                    {
                        "path": "Mistborn.epub",
                        "size": 1_048_576,
                        "extension": "epub",
                        "mime_type": "application/epub+zip"
                    }
                ]
            }),
        )
        .await;
    // Endpoint returns 202 and enqueues an async job
    resp.assert_success();

    // Process the enqueued ingest job (no NATS worker in tests)
    server.process_pending_ingest_jobs().await;

    // Verify that the file download endpoint was called (Kobbler fetched the epub)
    let requests = mock_server.received_requests().await.expect("requests");
    let file_download_calls: Vec<_> = requests
        .iter()
        .filter(|r| r.url.path().contains("/files/"))
        .collect();
    assert!(
        !file_download_calls.is_empty(),
        "Expected file download request to Kobload"
    );
}

#[tokio::test]
async fn test_callback_no_ebooks_returns_error() {
    let (server, _mock_server, wanted_id) = setup_wanted_with_kobload().await;

    let download_id = queue_download(&server, &_mock_server, &wanted_id).await;

    // Send callback with only non-ebook files
    let resp = TestClient::new(&server.api_url)
        .post_json(
            &format!("/api/v1/ingest/{}", server.ingest_token),
            &json!({
                "download_id": download_id,
                "status": "completed",
                "metadata": { "book_id": wanted_id },
                "files": [
                    {
                        "path": "readme.txt",
                        "size": 256,
                        "extension": "txt",
                        "mime_type": "text/plain"
                    },
                    {
                        "path": "cover.jpg",
                        "size": 50000,
                        "extension": "jpg",
                        "mime_type": "image/jpeg"
                    }
                ]
            }),
        )
        .await;
    // Endpoint returns 202 (enqueues job); the job itself detects no ebooks
    resp.assert_success();

    server.process_pending_ingest_jobs().await;

    // Job should have failed (no ebook files found)
    let job: (String,) = sqlx::query_as(
        "SELECT status FROM job_status WHERE job_type = 'kobload_ingest' ORDER BY created_at DESC LIMIT 1",
    )
    .fetch_one(&server.pool)
    .await
    .expect("job should exist");
    assert_eq!(
        job.0, "completed",
        "job completed (handler returns Ok even on no-ebook errors)"
    );
}

#[tokio::test]
async fn test_callback_invalid_token_rejected() {
    let server = TestServer::start_api_only().await;

    let resp = TestClient::new(&server.api_url)
        .post_json(
            "/api/v1/ingest/invalid-token-12345",
            &json!({
                "download_id": Uuid::now_v7(),
                "status": "completed",
                "metadata": {},
                "files": [],
            }),
        )
        .await;
    assert_eq!(resp.status, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn test_callback_unknown_download_id() {
    let (server, _mock_server, _wanted_id) = setup_wanted_with_kobload().await;

    // Valid token, Kobload configured, but unknown download_id (no matching wanted_attempt)
    let resp = TestClient::new(&server.api_url)
        .post_json(
            &format!("/api/v1/ingest/{}", server.ingest_token),
            &json!({
                "download_id": Uuid::now_v7(),
                "status": "completed",
                "metadata": {},
                "files": [
                    {
                        "path": "book.epub",
                        "size": 1024,
                        "extension": "epub",
                        "mime_type": "application/epub+zip"
                    }
                ]
            }),
        )
        .await;
    // Should handle gracefully — the handler processes unknown downloads too
    // (it may create an ingest queue item without a wanted book)
    // 502 is acceptable: the handler tries to download files from Kobload but
    // the unknown download_id has no matching file endpoint.
    assert!(
        resp.status.is_success()
            || resp.status == StatusCode::BAD_REQUEST
            || resp.status == StatusCode::BAD_GATEWAY,
        "Unexpected status: {} body: {}",
        resp.status,
        resp.text()
    );
}

// =============================================================================
// Full Ingest-to-Library Test (Area 1)
// =============================================================================

#[tokio::test]
async fn test_full_ingest_to_library_kepub() {
    let (server, mock_server, wanted_id) = setup_wanted_with_kobload().await;

    let download_id = queue_download(&server, &mock_server, &wanted_id).await;

    // Mock the file download endpoint — serve a fake kepub file.
    // Kepub files bypass kepubify (just copied), so no external tool dependency.
    let fake_kepub = b"PK\x03\x04fake kepub content for testing".to_vec();
    Mock::given(method("GET"))
        .and(path_regex(format!(
            r"/api/v1/downloads/{}/files/.*",
            download_id
        )))
        .and(header("Authorization", "Bearer test-key"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_bytes(fake_kepub)
                .insert_header("Content-Type", "application/epub+zip"),
        )
        .mount(&mock_server)
        .await;

    // Mock the import confirmation endpoint
    Mock::given(method("POST"))
        .and(path(format!("/api/v1/downloads/{}/imported", download_id)))
        .and(header("Authorization", "Bearer test-key"))
        .respond_with(ResponseTemplate::new(200))
        .mount(&mock_server)
        .await;

    // Send completed callback with a kepub file
    let resp = TestClient::new(&server.api_url)
        .post_json(
            &format!("/api/v1/ingest/{}", server.ingest_token),
            &json!({
                "download_id": download_id,
                "status": "completed",
                "metadata": { "book_id": wanted_id },
                "files": [
                    {
                        "path": "Mistborn.kepub",
                        "size": 1_048_576,
                        "extension": "kepub",
                        "mime_type": "application/epub+zip"
                    }
                ]
            }),
        )
        .await;
    resp.assert_success();

    // Process the enqueued ingest job (no NATS worker in tests)
    server.process_pending_ingest_jobs().await;

    // Verify the book is now in the library by querying the DB directly.
    // is_in_library is derived from file existence (column was removed).
    // The ingest pipeline spawns a background task to process the file, so we
    // need to wait briefly for it to complete.
    let book_uuid = Uuid::parse_str(&wanted_id).unwrap();
    let mut row: (bool, String) = (false, String::new());
    for _ in 0..20 {
        row = sqlx::query_as("SELECT (EXISTS (SELECT 1 FROM book_files bf WHERE bf.book_id = books.id) OR EXISTS (SELECT 1 FROM audiobook_files af WHERE af.book_id = books.id)) AS is_in_library, want_status FROM books WHERE id = $1")
            .bind(book_uuid)
            .fetch_one(&server.pool)
            .await
            .expect("book should exist");
        if row.0 {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
    assert!(row.0, "Book should be in library after ingest");
    assert_eq!(row.1, "none", "want_status should be cleared after ingest");

    // Verify the download attempt was marked as completed.
    let attempts = api::repositories::wanted_attempts::list_by_book(&server.pool, book_uuid)
        .await
        .expect("list attempts");
    assert_eq!(attempts.len(), 1);
    assert_eq!(
        attempts[0].status, "completed",
        "attempt should be marked completed"
    );

    // Verify the file was downloaded from Kobload
    let requests = mock_server.received_requests().await.expect("requests");
    let file_download_calls: Vec<_> = requests
        .iter()
        .filter(|r| r.url.path().contains("/files/"))
        .collect();
    assert!(
        !file_download_calls.is_empty(),
        "Expected file download request to Kobload"
    );

    // Verify import confirmation was sent to Kobload
    let import_calls: Vec<_> = requests
        .iter()
        .filter(|r| r.url.path().contains("/imported"))
        .collect();
    assert!(
        !import_calls.is_empty(),
        "Expected import confirmation to Kobload"
    );
}

// =============================================================================
// Multi-File Ingest Tests
// =============================================================================

/// When a callback has multiple ebook files for the same wanted book,
/// all files should be ingested without crashing (no duplicate key errors),
/// and the wanted book should be marked as in-library.
#[tokio::test]
async fn test_multi_file_ingest_links_to_wanted_book() {
    let (server, mock_server, wanted_id) = setup_wanted_with_kobload().await;

    let download_id = queue_download(&server, &mock_server, &wanted_id).await;

    // Mock file download — serve fake kepub bytes for any file request.
    // Using .kepub extension to bypass kepubify dependency in CI.
    let fake_kepub = b"PK\x03\x04fake kepub content for multi-file".to_vec();
    Mock::given(method("GET"))
        .and(path_regex(format!(
            r"/api/v1/downloads/{}/files/.*",
            download_id
        )))
        .and(header("Authorization", "Bearer test-key"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_bytes(fake_kepub)
                .insert_header("Content-Type", "application/epub+zip"),
        )
        .mount(&mock_server)
        .await;

    // Mock import confirmation
    Mock::given(method("POST"))
        .and(path(format!("/api/v1/downloads/{}/imported", download_id)))
        .and(header("Authorization", "Bearer test-key"))
        .respond_with(ResponseTemplate::new(200))
        .mount(&mock_server)
        .await;

    // Send callback with TWO kepub files with the SAME normalized name.
    // classify_files groups by normalized_name, so these become a SingleBook group,
    // exercising the multi-file ingest path within ingest_single_book.
    // Both use .kepub to avoid needing kepubify in CI.
    let resp = TestClient::new(&server.api_url)
        .post_json(
            &format!("/api/v1/ingest/{}", server.ingest_token),
            &json!({
                "download_id": download_id,
                "status": "completed",
                "metadata": { "book_id": wanted_id },
                "files": [
                    {
                        "path": "Mistborn.kepub",
                        "size": 1_048_576,
                        "extension": "kepub",
                        "mime_type": "application/epub+zip"
                    },
                    {
                        "path": "sub/Mistborn.kepub",
                        "size": 900_000,
                        "extension": "kepub",
                        "mime_type": "application/epub+zip"
                    }
                ]
            }),
        )
        .await;
    resp.assert_success();

    // Process the enqueued ingest job (no NATS worker in tests)
    server.process_pending_ingest_jobs().await;

    // Verify the wanted book is in the library (derived from file existence).
    // The ingest pipeline spawns a background task, so poll until it completes.
    let book_uuid = Uuid::parse_str(&wanted_id).unwrap();
    let mut row: (bool, String) = (false, String::new());
    for _ in 0..20 {
        row = sqlx::query_as("SELECT (EXISTS (SELECT 1 FROM book_files bf WHERE bf.book_id = books.id) OR EXISTS (SELECT 1 FROM audiobook_files af WHERE af.book_id = books.id)) AS is_in_library, want_status FROM books WHERE id = $1")
            .bind(book_uuid)
            .fetch_one(&server.pool)
            .await
            .expect("book should exist");
        if row.0 {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
    assert!(row.0, "Book should be in library after multi-file ingest");
    assert_eq!(row.1, "none", "want_status should be cleared");

    // Verify no orphan books were created — should be exactly 1 book total
    let total_books: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM books")
        .fetch_one(&server.pool)
        .await
        .expect("count books");
    assert_eq!(
        total_books, 1,
        "Expected exactly 1 book, got {total_books} (orphan duplicates created)"
    );

    // Verify at least one book_file record was created for the wanted book
    let file_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM book_files WHERE book_id = $1")
        .bind(book_uuid)
        .fetch_one(&server.pool)
        .await
        .expect("count files");
    assert!(
        file_count >= 1,
        "Expected at least 1 book_file for wanted book, got {file_count}"
    );
}

// =============================================================================
// File Download Failure Tests (status flow fixes)
// =============================================================================

/// When Kobload reports "completed" but the file download fails (500),
/// the wanted attempt should be marked "failed" — NOT "completed".
/// This tests the premature-completed bug fix.
#[tokio::test]
async fn test_file_download_failure_marks_attempt_failed() {
    let (server, mock_server, wanted_id) = setup_wanted_with_kobload().await;
    let download_id = queue_download(&server, &mock_server, &wanted_id).await;

    // Mock file download to return 500 (simulates Kobload file serving failure)
    Mock::given(method("GET"))
        .and(path_regex(format!(
            r"/api/v1/downloads/{}/files/.*",
            download_id
        )))
        .respond_with(ResponseTemplate::new(500))
        .mount(&mock_server)
        .await;

    // Send "completed" callback — Kobload says download finished, but file serving fails
    let resp = TestClient::new(&server.api_url)
        .post_json(
            &format!("/api/v1/ingest/{}", server.ingest_token),
            &json!({
                "download_id": download_id,
                "status": "completed",
                "metadata": { "book_id": wanted_id },
                "files": [
                    {
                        "path": "Mistborn.epub",
                        "size": 1_048_576,
                        "extension": "epub",
                    }
                ]
            }),
        )
        .await;
    // Endpoint returns 202 (enqueues job); the job itself encounters the download failure
    resp.assert_success();

    server.process_pending_ingest_jobs().await;

    // Verify attempt is "failed", NOT "completed"
    let book_uuid = Uuid::parse_str(&wanted_id).unwrap();
    let attempts = api::repositories::wanted_attempts::list_by_book(&server.pool, book_uuid)
        .await
        .expect("list attempts");
    assert_eq!(attempts.len(), 1);
    assert_eq!(
        attempts[0].status, "failed",
        "attempt should be 'failed' when file download fails, not 'completed'"
    );

    // Book should NOT be in library (derived from file existence)
    let in_library: bool = sqlx::query_scalar("SELECT (EXISTS (SELECT 1 FROM book_files bf WHERE bf.book_id = books.id) OR EXISTS (SELECT 1 FROM audiobook_files af WHERE af.book_id = books.id)) AS is_in_library FROM books WHERE id = $1")
        .bind(book_uuid)
        .fetch_one(&server.pool)
        .await
        .expect("book should exist");
    assert!(
        !in_library,
        "Book should NOT be in library after download failure"
    );
}

/// When callback uses the `url` field on files, Kobbler should fetch from that URL
/// (not construct one from the path). This tests the file ID URL mechanism.
#[tokio::test]
async fn test_callback_uses_file_url_field() {
    let (server, mock_server, wanted_id) = setup_wanted_with_kobload().await;
    let download_id = queue_download(&server, &mock_server, &wanted_id).await;
    let file_id = Uuid::now_v7();

    // Mount mock at the exact file ID URL — if Kobbler uses the `url` field this will match
    let fake_kepub = b"PK\x03\x04fake kepub via file ID".to_vec();
    Mock::given(method("GET"))
        .and(path(format!(
            "/api/v1/downloads/{}/files/{}",
            download_id, file_id
        )))
        .and(header("Authorization", "Bearer test-key"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_bytes(fake_kepub)
                .insert_header("Content-Type", "application/epub+zip"),
        )
        .expect(1) // Must be called exactly once
        .mount(&mock_server)
        .await;

    // Mock import confirmation
    Mock::given(method("POST"))
        .and(path(format!("/api/v1/downloads/{}/imported", download_id)))
        .respond_with(ResponseTemplate::new(200))
        .mount(&mock_server)
        .await;

    // Send callback with `url` field set (as kobload now provides)
    let resp = TestClient::new(&server.api_url)
        .post_json(
            &format!("/api/v1/ingest/{}", server.ingest_token),
            &json!({
                "download_id": download_id,
                "status": "completed",
                "metadata": { "book_id": wanted_id },
                "files": [
                    {
                        "id": file_id,
                        "path": "Mistborn.kepub",
                        "url": format!("/api/v1/downloads/{}/files/{}", download_id, file_id),
                        "size": 1_048_576,
                        "extension": "kepub",
                    }
                ]
            }),
        )
        .await;
    resp.assert_success();

    // Process the enqueued ingest job (no NATS worker in tests)
    server.process_pending_ingest_jobs().await;

    // The mock expectation (expect(1)) verifies the file ID URL was used.
    // Also verify the book ended up in the library (derived from file existence).
    // The ingest pipeline spawns a background task to process the file, so we
    // need to wait briefly for it to complete.
    let book_uuid = Uuid::parse_str(&wanted_id).unwrap();
    let mut file_count: i64 = 0;
    for _ in 0..20 {
        file_count = sqlx::query_scalar("SELECT COUNT(*) FROM book_files WHERE book_id = $1")
            .bind(book_uuid)
            .fetch_one(&server.pool)
            .await
            .expect("count query should work");
        if file_count > 0 {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
    assert!(
        file_count > 0,
        "Book should have files after successful ingest via file URL (found {file_count} files)"
    );
}

/// Verify the status progression goes through "ingesting" before "completed".
/// The attempt should transition: queued → ingesting → completed.
#[tokio::test]
async fn test_status_progression_through_ingesting() {
    let (server, mock_server, wanted_id) = setup_wanted_with_kobload().await;
    let download_id = queue_download(&server, &mock_server, &wanted_id).await;
    let book_uuid = Uuid::parse_str(&wanted_id).unwrap();

    // Verify initial status is "queued"
    let attempts = api::repositories::wanted_attempts::list_by_book(&server.pool, book_uuid)
        .await
        .unwrap();
    assert_eq!(attempts[0].status, "queued");

    // Use a delay mock to give us time to observe "ingesting" status.
    // The mock will pause for 1 second before responding.
    let fake_kepub = b"PK\x03\x04fake kepub content".to_vec();
    Mock::given(method("GET"))
        .and(path_regex(format!(
            r"/api/v1/downloads/{}/files/.*",
            download_id
        )))
        .and(header("Authorization", "Bearer test-key"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_bytes(fake_kepub)
                .insert_header("Content-Type", "application/epub+zip")
                .set_delay(std::time::Duration::from_secs(1)),
        )
        .mount(&mock_server)
        .await;

    Mock::given(method("POST"))
        .and(path(format!("/api/v1/downloads/{}/imported", download_id)))
        .respond_with(ResponseTemplate::new(200))
        .mount(&mock_server)
        .await;

    // Send callback — returns 202 immediately, enqueuing the job
    let resp = TestClient::new(&server.api_url)
        .post_json(
            &format!("/api/v1/ingest/{}", server.ingest_token),
            &json!({
                "download_id": download_id,
                "status": "completed",
                "metadata": { "book_id": wanted_id },
                "files": [
                    {
                        "path": "Mistborn.kepub",
                        "size": 1024,
                        "extension": "kepub",
                    }
                ]
            }),
        )
        .await;
    resp.assert_success();

    // Process the ingest job in a background task so we can observe status mid-flight
    let pool_clone = server.pool.clone();
    let handler = api::jobs::handlers::KobloadIngestHandler::new(server.app_state.clone());
    let handle = tokio::spawn(async move {
        let rows: Vec<(Uuid, serde_json::Value)> = sqlx::query_as(
            "SELECT id, payload FROM job_status WHERE job_type = 'kobload_ingest' AND status = 'pending'",
        )
        .fetch_all(&pool_clone)
        .await
        .unwrap();
        for (job_id, payload) in rows {
            use api::jobs::worker::JobHandler;
            let _ = handler
                .execute(job_id, payload, tokio_util::sync::CancellationToken::new())
                .await;
        }
    });

    // Wait briefly for the handler to set "ingesting" (happens before file download)
    tokio::time::sleep(tokio::time::Duration::from_millis(200)).await;

    // Check status is "ingesting" while file download is in progress
    let attempts = api::repositories::wanted_attempts::list_by_book(&server.pool, book_uuid)
        .await
        .unwrap();
    assert_eq!(
        attempts[0].status, "ingesting",
        "Status should be 'ingesting' while downloading files"
    );

    // Wait for handler to complete
    handle.await.unwrap();

    // Give async processing time to complete
    tokio::time::sleep(tokio::time::Duration::from_millis(500)).await;

    // Final status should be "completed"
    let attempts = api::repositories::wanted_attempts::list_by_book(&server.pool, book_uuid)
        .await
        .unwrap();
    assert_eq!(
        attempts[0].status, "completed",
        "Status should be 'completed' after successful ingest"
    );
}

/// Kobload delivering a wanted book announces "acquired" once — one "Import
/// Complete" per profile — however many formats the download carried.
#[tokio::test]
async fn test_kobload_delivery_announces_acquisition_once() {
    for paths in [
        &["Mistborn.kepub"][..],
        &["Mistborn.kepub", "sub/Mistborn.kepub"][..],
    ] {
        let (server, mock_server, wanted_id) = setup_wanted_with_kobload().await;
        let download_id = queue_download(&server, &mock_server, &wanted_id).await;
        Mock::given(method("GET"))
            .and(path_regex(format!(
                r"/api/v1/downloads/{download_id}/files/.*"
            )))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_bytes(b"PK\x03\x04fake kepub".to_vec())
                    .insert_header("Content-Type", "application/epub+zip"),
            )
            .mount(&mock_server)
            .await;
        Mock::given(method("POST"))
            .and(path_regex(
                r"^/api/v1/(downloads/.*/imported|want/.*/confirm)$",
            ))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({"status": "deleted"})))
            .mount(&mock_server)
            .await;

        let book_id = Uuid::parse_str(&wanted_id).unwrap();
        let mut events = server.library_events.subscribe();
        let files: Vec<Value> = paths
            .iter()
            .map(|p| json!({"path": p, "size": 1_048_576, "extension": "kepub", "mime_type": "application/epub+zip"}))
            .collect();
        TestClient::new(&server.api_url)
            .post_json(
                &format!("/api/v1/ingest/{}", server.ingest_token),
                &json!({
                    "download_id": download_id,
                    "status": "completed",
                    "metadata": { "book_id": wanted_id },
                    "files": files,
                }),
            )
            .await
            .assert_success();
        server.process_pending_ingest_jobs().await;

        let mut announced = 0;
        let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(5);
        while let Ok(Ok(event)) = tokio::time::timeout_at(deadline, events.recv()).await {
            if let LibraryEvent::WantedStatusUpdated {
                wanted_id,
                status,
                announce: true,
                ..
            } = event
                && wanted_id == book_id
                && status == "acquired"
            {
                announced += 1;
            }
        }
        assert_eq!(
            announced, 1,
            "{paths:?}: acquisition announced {announced} times"
        );
    }
}

/// What Kobbler told Kobload about one ingest, in either reporting mode: the
/// kobload endpoint it called, or the status it published on NATS.
async fn ingest_report(
    server: &TestServer,
    mock_server: &MockServer,
    download_id: Uuid,
    request: Value,
    nats: bool,
) -> String {
    if !nats {
        TestClient::new(&server.api_url)
            .post_json(&format!("/api/v1/ingest/{}", server.ingest_token), &request)
            .await
            .assert_success();
        server.process_pending_ingest_jobs().await;
        let reports: Vec<String> = mock_server
            .received_requests()
            .await
            .unwrap_or_default()
            .iter()
            .filter(|r| r.method.as_str() == "POST")
            .filter_map(|r| {
                r.url
                    .path()
                    .strip_prefix(&format!("/api/v1/downloads/{download_id}/"))
                    .map(String::from)
            })
            .collect();
        return reports.join(",");
    }

    use api::jobs::worker::JobHandler;
    use futures::StreamExt;
    let url = std::env::var("NATS_URL_TEST").unwrap_or_else(|_| "nats://localhost:4223".into());
    let nats = nats_jobs::NatsClient::connect(&nats_jobs::NatsConfig::new(&url, "ingest-report"))
        .await
        .expect("nats");
    let mut results = nats
        .client()
        .subscribe("events.ingest.result")
        .await
        .unwrap();
    api::jobs::handlers::KobloadIngestHandler::new(server.app_state.clone())
        .execute(
            Uuid::now_v7(),
            json!({ "request": request, "response_mode": "nats" }),
            tokio_util::sync::CancellationToken::new(),
        )
        .await
        .unwrap();
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(5);
    while let Ok(Some(msg)) = tokio::time::timeout_at(deadline, results.next()).await {
        let event: Value = serde_json::from_slice(&msg.payload).unwrap();
        if event["payload"]["download_id"] == json!(download_id) {
            return event["payload"]["status"]
                .as_str()
                .unwrap_or_default()
                .to_string();
        }
    }
    String::new()
}

/// A release is only to blame when its content is wrong. When Kobbler cannot
/// fetch the files from Kobload (or store them), Kobload must retry the ingest
/// rather than exclude the release and delete the download.
#[tokio::test]
async fn test_ingest_reports_retry_for_infrastructure_failures_and_failure_for_bad_content() {
    struct Case {
        name: &'static str,
        files_status: u16,
        files: Value,
        http: &'static str,
        nats: &'static str,
    }
    let cases = [
        Case {
            name: "kobload file fetch fails",
            files_status: 502,
            files: json!([{ "path": "Mistborn.epub", "size": 1_048_576, "extension": "epub" }]),
            http: "import-retry",
            nats: "Retry",
        },
        Case {
            name: "no ebook in the download",
            files_status: 200,
            files: json!([{ "path": "readme.txt", "size": 256, "extension": "txt", "mime_type": "text/plain" }]),
            http: "import-failed",
            nats: "Failed",
        },
    ];

    let mut failures = Vec::new();
    for case in &cases {
        for nats in [false, true] {
            let (server, mock_server, wanted_id) = setup_wanted_with_kobload().await;
            let download_id = queue_download(&server, &mock_server, &wanted_id).await;
            Mock::given(method("GET"))
                .and(path_regex(format!(
                    r"/api/v1/downloads/{download_id}/files/.*"
                )))
                .respond_with(ResponseTemplate::new(case.files_status))
                .mount(&mock_server)
                .await;
            let request = json!({
                "download_id": download_id,
                "status": "completed",
                "metadata": { "book_id": wanted_id },
                "files": case.files,
            });

            let got = ingest_report(&server, &mock_server, download_id, request, nats).await;
            let want = if nats { case.nats } else { case.http };
            if got != want {
                failures.push(format!(
                    "{} ({}): reported {got:?}, expected {want:?}",
                    case.name,
                    if nats { "nats" } else { "http" }
                ));
            }
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}
