use crate::common::*;
use reqwest::StatusCode;
use serde_json::Value;
use uuid::Uuid;

#[tokio::test]
async fn test_create_book_with_metadata() {
    let server = TestServer::start().await;

    let book_id = Uuid::new_v4();
    let resp = server
        .ingest_client()
        .put_json(
            &format!("/api/v1/books/{}", book_id),
            &serde_json::json!({
                "title": "Test Book Title",
                "description": "A test book description",
                "language": "en",
                "publisher": "Test Publisher",
                "authors": ["Author One", "Author Two"]
            }),
        )
        .await;
    resp.assert_status(StatusCode::NO_CONTENT);
}

#[tokio::test]
async fn test_create_book_minimal() {
    let server = TestServer::start().await;

    let book_id = Uuid::new_v4();
    let resp = server
        .ingest_client()
        .put_json(
            &format!("/api/v1/books/{}", book_id),
            &serde_json::json!({
                "title": "Minimal Book"
            }),
        )
        .await;
    resp.assert_status(StatusCode::NO_CONTENT);
}

#[tokio::test]
async fn test_update_book_metadata() {
    let server = TestServer::start().await;

    let book_id = Uuid::new_v4();

    // Create initial book
    server
        .ingest_client()
        .put_json(
            &format!("/api/v1/books/{}", book_id),
            &serde_json::json!({
                "title": "Original Title"
            }),
        )
        .await
        .assert_status(StatusCode::NO_CONTENT);

    // Update with new title
    server
        .ingest_client()
        .put_json(
            &format!("/api/v1/books/{}", book_id),
            &serde_json::json!({
                "title": "Updated Title",
                "description": "Now with a description"
            }),
        )
        .await
        .assert_status(StatusCode::NO_CONTENT);
}

#[tokio::test]
async fn test_create_book_with_series() {
    let server = TestServer::start().await;

    let book1_id = Uuid::new_v4();
    let book2_id = Uuid::new_v4();

    // Create first book in series
    server
        .ingest_client()
        .put_json(
            &format!("/api/v1/books/{}", book1_id),
            &serde_json::json!({
                "title": "Series Book 1",
                "series_name": "My Test Series",
                "series_index": 1,
                "authors": ["Series Author"]
            }),
        )
        .await
        .assert_status(StatusCode::NO_CONTENT);

    // Create second book in same series
    server
        .ingest_client()
        .put_json(
            &format!("/api/v1/books/{}", book2_id),
            &serde_json::json!({
                "title": "Series Book 2",
                "series_name": "My Test Series",
                "series_index": 2,
                "authors": ["Series Author"]
            }),
        )
        .await
        .assert_status(StatusCode::NO_CONTENT);
}

#[tokio::test]
async fn test_upload_cover() {
    let server = TestServer::start().await;

    let book_id = create_book(&server, "Cover Test Book").await;

    // Upload a simple 1x1 JPEG
    let jpeg_bytes = create_test_jpeg();
    let resp = server
        .ingest_client()
        .put_bytes(
            &format!("/api/v1/books/{}/cover", book_id),
            jpeg_bytes,
            "image/jpeg",
        )
        .await;
    resp.assert_status(StatusCode::NO_CONTENT);
}

#[tokio::test]
async fn test_upload_file() {
    let server = TestServer::start().await;

    let book_id = create_book(&server, "File Test Book").await;

    // Upload a mock EPUB file
    let epub_bytes = create_test_kepub();
    let resp = server
        .ingest_client()
        .put_bytes(
            &format!("/api/v1/books/{}/files/KEPUB", book_id),
            epub_bytes,
            "application/epub+zip",
        )
        .await;
    resp.assert_status(StatusCode::NO_CONTENT);
}

#[tokio::test]
async fn test_create_entitlement_all_users() {
    let server = TestServer::start().await;

    let book_id = create_book(&server, "Entitlement Test").await;

    let resp = server
        .ingest_client()
        .post_json(
            "/api/v1/entitlements",
            &serde_json::json!({
                "book_id": book_id,
                "scope_type": "ALL_USERS"
            }),
        )
        .await;
    resp.assert_status(StatusCode::CREATED);
}

#[tokio::test]
async fn test_create_entitlement_user_scope() {
    let server = TestServer::start().await;

    let user_id = create_user(&server, "scopeuser@test.com").await;
    let book_id = create_book(&server, "User Scope Book").await;

    let resp = server
        .ingest_client()
        .post_json(
            "/api/v1/entitlements",
            &serde_json::json!({
                "book_id": book_id,
                "scope_type": "USER",
                "scope_id": user_id
            }),
        )
        .await;
    resp.assert_status(StatusCode::CREATED);
}

#[tokio::test]
async fn test_create_entitlement_device_scope() {
    let server = TestServer::start().await;

    let user_id = create_user(&server, "devicescope@test.com").await;
    let (device_id, _) = create_device(&server, user_id).await;
    let book_id = create_book(&server, "Device Scope Book").await;

    let resp = server
        .ingest_client()
        .post_json(
            "/api/v1/entitlements",
            &serde_json::json!({
                "book_id": book_id,
                "scope_type": "DEVICE",
                "scope_id": device_id
            }),
        )
        .await;
    resp.assert_status(StatusCode::CREATED);
}

#[tokio::test]
async fn test_create_entitlement_requires_scope_id_for_user() {
    let server = TestServer::start().await;

    let book_id = create_book(&server, "Invalid Entitlement").await;

    let resp = server
        .ingest_client()
        .post_json(
            "/api/v1/entitlements",
            &serde_json::json!({
                "book_id": book_id,
                "scope_type": "USER"
                // Missing scope_id
            }),
        )
        .await;
    resp.assert_status(StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn test_create_entitlement_requires_scope_id_for_device() {
    let server = TestServer::start().await;

    let book_id = create_book(&server, "Invalid Device Entitlement").await;

    let resp = server
        .ingest_client()
        .post_json(
            "/api/v1/entitlements",
            &serde_json::json!({
                "book_id": book_id,
                "scope_type": "DEVICE"
                // Missing scope_id
            }),
        )
        .await;
    resp.assert_status(StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn test_create_entitlement_invalid_scope_type() {
    let server = TestServer::start().await;

    let book_id = create_book(&server, "Invalid Scope Type").await;

    let resp = server
        .ingest_client()
        .post_json(
            "/api/v1/entitlements",
            &serde_json::json!({
                "book_id": book_id,
                "scope_type": "INVALID"
            }),
        )
        .await;
    resp.assert_status(StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn test_ingest_requires_auth() {
    let server = TestServer::start().await;
    let client = TestClient::new(&server.api_url);

    let book_id = Uuid::new_v4();

    // All ingest endpoints should require auth
    client
        .put_json(
            &format!("/api/v1/books/{}", book_id),
            &serde_json::json!({"title": "Test"}),
        )
        .await
        .assert_status(StatusCode::UNAUTHORIZED);

    client
        .put_bytes(
            &format!("/api/v1/books/{}/cover", book_id),
            vec![0u8; 10],
            "image/jpeg",
        )
        .await
        .assert_status(StatusCode::UNAUTHORIZED);

    client
        .put_bytes(
            &format!("/api/v1/books/{}/files/EPUB", book_id),
            vec![0u8; 10],
            "application/epub+zip",
        )
        .await
        .assert_status(StatusCode::UNAUTHORIZED);

    client
        .post_json(
            "/api/v1/entitlements",
            &serde_json::json!({"book_id": book_id}),
        )
        .await
        .assert_status(StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn test_ingest_invalid_book_id() {
    let server = TestServer::start().await;

    let resp = server
        .ingest_client()
        .put_json(
            "/api/v1/books/not-a-uuid",
            &serde_json::json!({"title": "Test"}),
        )
        .await;
    resp.assert_status(StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn test_full_ingest_flow() {
    let server = TestServer::start().await;

    let user_id = create_user(&server, "fullflow@test.com").await;
    let (_, device_token) = create_device(&server, user_id).await;

    // 1. Create book with metadata
    let book_id = Uuid::new_v4();
    server
        .ingest_client()
        .put_json(
            &format!("/api/v1/books/{}", book_id),
            &serde_json::json!({
                "title": "Full Flow Test Book",
                "description": "Testing the complete ingest flow",
                "authors": ["Test Author"],
                "language": "en"
            }),
        )
        .await
        .assert_status(StatusCode::NO_CONTENT);

    // 2. Upload cover
    let jpeg_bytes = create_test_jpeg();
    server
        .ingest_client()
        .put_bytes(
            &format!("/api/v1/books/{}/cover", book_id),
            jpeg_bytes,
            "image/jpeg",
        )
        .await
        .assert_status(StatusCode::NO_CONTENT);

    // 3. Upload KEPUB file
    let kepub_bytes = create_test_kepub();
    server
        .ingest_client()
        .put_bytes(
            &format!("/api/v1/books/{}/files/KEPUB", book_id),
            kepub_bytes,
            "application/epub+zip",
        )
        .await
        .assert_status(StatusCode::NO_CONTENT);

    // 4. Create entitlement
    server
        .ingest_client()
        .post_json(
            "/api/v1/entitlements",
            &serde_json::json!({
                "book_id": book_id,
                "scope_type": "ALL_USERS"
            }),
        )
        .await
        .assert_status(StatusCode::CREATED);

    // 5. Verify book appears in sync
    let items: Vec<Value> = server
        .kobo_client()
        .kobo_sync(&device_token)
        .await
        .assert_ok()
        .json();
    assert_sync_contains_book(&items, book_id);
}

// =============================================================================
// Atomic Ingest Endpoint Tests
// =============================================================================

#[tokio::test]
async fn test_atomic_ingest_success() {
    let server = TestServer::start().await;

    let user_id = create_user(&server, "atomic@test.com").await;
    let (_, device_token) = create_device(&server, user_id).await;

    let book_id = Uuid::new_v4();

    // Create multipart form with all required fields
    let metadata = serde_json::json!({
        "title": "Atomic Ingest Book",
        "description": "Testing atomic ingest",
        "authors": ["Test Author"],
        "language": "en"
    });

    let form = reqwest::multipart::Form::new()
        .text("metadata", metadata.to_string())
        .text("format", "KEPUB")
        .part(
            "cover",
            reqwest::multipart::Part::bytes(create_test_jpeg())
                .file_name("cover.jpg")
                .mime_str("image/jpeg")
                .unwrap(),
        )
        .part(
            "file",
            reqwest::multipart::Part::bytes(create_test_kepub())
                .file_name("book.kepub.epub")
                .mime_str("application/epub+zip")
                .unwrap(),
        );

    let resp = server
        .ingest_client()
        .put_multipart(&format!("/api/v1/books/{}/ingest", book_id), form)
        .await;
    resp.assert_status(StatusCode::CREATED);

    let json: Value = resp.json();
    assert_eq!(json["book_id"], book_id.to_string());

    // Verify book appears in sync
    let items: Vec<Value> = server
        .kobo_client()
        .kobo_sync(&device_token)
        .await
        .assert_ok()
        .json();
    assert_sync_contains_book(&items, book_id);
}

#[tokio::test]
async fn test_atomic_ingest_minimal() {
    let server = TestServer::start().await;

    let book_id = Uuid::new_v4();

    // Create multipart form with only required fields (no cover)
    let metadata = serde_json::json!({
        "title": "Minimal Atomic Book"
    });

    let form = reqwest::multipart::Form::new()
        .text("metadata", metadata.to_string())
        .text("format", "KEPUB")
        .part(
            "file",
            reqwest::multipart::Part::bytes(create_test_kepub())
                .file_name("book.kepub.epub")
                .mime_str("application/epub+zip")
                .unwrap(),
        );

    let resp = server
        .ingest_client()
        .put_multipart(&format!("/api/v1/books/{}/ingest", book_id), form)
        .await;
    resp.assert_status(StatusCode::CREATED);

    let json: Value = resp.json();
    assert_eq!(json["book_id"], book_id.to_string());
}

#[tokio::test]
async fn test_atomic_ingest_requires_auth() {
    let server = TestServer::start().await;
    let client = TestClient::new(&server.api_url);

    let book_id = Uuid::new_v4();

    let metadata = serde_json::json!({
        "title": "Unauthorized Book"
    });

    let form = reqwest::multipart::Form::new()
        .text("metadata", metadata.to_string())
        .text("format", "KEPUB")
        .part(
            "file",
            reqwest::multipart::Part::bytes(create_test_kepub())
                .file_name("book.kepub.epub")
                .mime_str("application/epub+zip")
                .unwrap(),
        );

    let resp = client
        .put_multipart(&format!("/api/v1/books/{}/ingest", book_id), form)
        .await;
    resp.assert_status(StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn test_atomic_ingest_missing_metadata() {
    let server = TestServer::start().await;

    let book_id = Uuid::new_v4();

    // Missing metadata field
    let form = reqwest::multipart::Form::new()
        .text("format", "KEPUB")
        .part(
            "file",
            reqwest::multipart::Part::bytes(create_test_kepub())
                .file_name("book.kepub.epub")
                .mime_str("application/epub+zip")
                .unwrap(),
        );

    let resp = server
        .ingest_client()
        .put_multipart(&format!("/api/v1/books/{}/ingest", book_id), form)
        .await;
    resp.assert_status(StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn test_atomic_ingest_missing_file() {
    let server = TestServer::start().await;

    let book_id = Uuid::new_v4();

    let metadata = serde_json::json!({
        "title": "No File Book"
    });

    // Missing file field
    let form = reqwest::multipart::Form::new()
        .text("metadata", metadata.to_string())
        .text("format", "KEPUB");

    let resp = server
        .ingest_client()
        .put_multipart(&format!("/api/v1/books/{}/ingest", book_id), form)
        .await;
    resp.assert_status(StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn test_atomic_ingest_missing_format() {
    let server = TestServer::start().await;

    let book_id = Uuid::new_v4();

    let metadata = serde_json::json!({
        "title": "No Format Book"
    });

    // Missing format field AND no file extension or content type that could infer format
    let form = reqwest::multipart::Form::new()
        .text("metadata", metadata.to_string())
        .part(
            "file",
            reqwest::multipart::Part::bytes(create_test_kepub())
                .file_name("book") // No extension
                .mime_str("application/octet-stream") // Generic content type
                .unwrap(),
        );

    let resp = server
        .ingest_client()
        .put_multipart(&format!("/api/v1/books/{}/ingest", book_id), form)
        .await;
    resp.assert_status(StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn test_atomic_ingest_updates_existing_book() {
    let server = TestServer::start().await;

    let book_id = Uuid::new_v4();

    // First ingest
    let metadata1 = serde_json::json!({
        "title": "Original Title"
    });

    let form1 = reqwest::multipart::Form::new()
        .text("metadata", metadata1.to_string())
        .text("format", "KEPUB")
        .part(
            "file",
            reqwest::multipart::Part::bytes(create_test_kepub())
                .file_name("book.kepub.epub")
                .mime_str("application/epub+zip")
                .unwrap(),
        );

    server
        .ingest_client()
        .put_multipart(&format!("/api/v1/books/{}/ingest", book_id), form1)
        .await
        .assert_status(StatusCode::CREATED);

    // Second ingest with updated title
    let metadata2 = serde_json::json!({
        "title": "Updated Title"
    });

    let form2 = reqwest::multipart::Form::new()
        .text("metadata", metadata2.to_string())
        .text("format", "KEPUB")
        .part(
            "file",
            reqwest::multipart::Part::bytes(create_test_kepub())
                .file_name("book.kepub.epub")
                .mime_str("application/epub+zip")
                .unwrap(),
        );

    let resp = server
        .ingest_client()
        .put_multipart(&format!("/api/v1/books/{}/ingest", book_id), form2)
        .await;
    resp.assert_status(StatusCode::CREATED);

    // Verify the book has the updated title in the library
    let resp: Value = server
        .admin_client()
        .get("/api/v1/library/books")
        .await
        .assert_ok()
        .json();

    let books = resp["items"].as_array().expect("Expected items array");
    let book = books
        .iter()
        .find(|b| b["id"].as_str() == Some(&book_id.to_string()))
        .expect("Book not found in library");

    assert_eq!(book["title"], "Updated Title");
}

#[tokio::test]
async fn test_atomic_ingest_with_series() {
    let server = TestServer::start().await;

    let user_id = create_user(&server, "series@test.com").await;
    let (_, device_token) = create_device(&server, user_id).await;

    // Ingest first book in series
    let book_id_1 = Uuid::new_v4();
    let metadata1 = serde_json::json!({
        "title": "The Stormlight Archive Book 1",
        "series_name": "The Stormlight Archive",
        "series_index": 1
    });

    let form1 = reqwest::multipart::Form::new()
        .text("metadata", metadata1.to_string())
        .text("format", "KEPUB")
        .part(
            "file",
            reqwest::multipart::Part::bytes(create_test_kepub())
                .file_name("book1.kepub.epub")
                .mime_str("application/epub+zip")
                .unwrap(),
        );

    server
        .ingest_client()
        .put_multipart(&format!("/api/v1/books/{}/ingest", book_id_1), form1)
        .await
        .assert_status(StatusCode::CREATED);

    // Ingest second book in same series (tests upsert behavior)
    let book_id_2 = Uuid::new_v4();
    let metadata2 = serde_json::json!({
        "title": "The Stormlight Archive Book 2",
        "series_name": "The Stormlight Archive",
        "series_index": 2
    });

    let form2 = reqwest::multipart::Form::new()
        .text("metadata", metadata2.to_string())
        .text("format", "KEPUB")
        .part(
            "file",
            reqwest::multipart::Part::bytes(create_test_kepub())
                .file_name("book2.kepub.epub")
                .mime_str("application/epub+zip")
                .unwrap(),
        );

    server
        .ingest_client()
        .put_multipart(&format!("/api/v1/books/{}/ingest", book_id_2), form2)
        .await
        .assert_status(StatusCode::CREATED);

    // Verify both books appear in sync with series info
    let items: Vec<Value> = server
        .kobo_client()
        .kobo_sync(&device_token)
        .await
        .assert_ok()
        .json();

    assert_sync_contains_book(&items, book_id_1);
    assert_sync_contains_book(&items, book_id_2);

    // Verify series data in library
    let resp: Value = server
        .admin_client()
        .get("/api/v1/library/series")
        .await
        .assert_ok()
        .json();

    let series_list = resp["items"].as_array().expect("Expected items array");
    let series = series_list
        .iter()
        .find(|s| s["name"].as_str() == Some("The Stormlight Archive"))
        .expect("Series not found");

    // Series should have 2 books
    assert_eq!(series["book_count"], 2);
}

// Helper to create a minimal valid JPEG
fn create_test_jpeg() -> Vec<u8> {
    // Minimal 1x1 white JPEG
    vec![
        0xFF, 0xD8, 0xFF, 0xE0, 0x00, 0x10, 0x4A, 0x46, 0x49, 0x46, 0x00, 0x01, 0x01, 0x00, 0x00,
        0x01, 0x00, 0x01, 0x00, 0x00, 0xFF, 0xDB, 0x00, 0x43, 0x00, 0x08, 0x06, 0x06, 0x07, 0x06,
        0x05, 0x08, 0x07, 0x07, 0x07, 0x09, 0x09, 0x08, 0x0A, 0x0C, 0x14, 0x0D, 0x0C, 0x0B, 0x0B,
        0x0C, 0x19, 0x12, 0x13, 0x0F, 0x14, 0x1D, 0x1A, 0x1F, 0x1E, 0x1D, 0x1A, 0x1C, 0x1C, 0x20,
        0x24, 0x2E, 0x27, 0x20, 0x22, 0x2C, 0x23, 0x1C, 0x1C, 0x28, 0x37, 0x29, 0x2C, 0x30, 0x31,
        0x34, 0x34, 0x34, 0x1F, 0x27, 0x39, 0x3D, 0x38, 0x32, 0x3C, 0x2E, 0x33, 0x34, 0x32, 0xFF,
        0xC0, 0x00, 0x0B, 0x08, 0x00, 0x01, 0x00, 0x01, 0x01, 0x01, 0x11, 0x00, 0xFF, 0xC4, 0x00,
        0x1F, 0x00, 0x00, 0x01, 0x05, 0x01, 0x01, 0x01, 0x01, 0x01, 0x01, 0x00, 0x00, 0x00, 0x00,
        0x00, 0x00, 0x00, 0x00, 0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07, 0x08, 0x09, 0x0A, 0x0B,
        0xFF, 0xC4, 0x00, 0xB5, 0x10, 0x00, 0x02, 0x01, 0x03, 0x03, 0x02, 0x04, 0x03, 0x05, 0x05,
        0x04, 0x04, 0x00, 0x00, 0x01, 0x7D, 0x01, 0x02, 0x03, 0x00, 0x04, 0x11, 0x05, 0x12, 0x21,
        0x31, 0x41, 0x06, 0x13, 0x51, 0x61, 0x07, 0x22, 0x71, 0x14, 0x32, 0x81, 0x91, 0xA1, 0x08,
        0x23, 0x42, 0xB1, 0xC1, 0x15, 0x52, 0xD1, 0xF0, 0x24, 0x33, 0x62, 0x72, 0x82, 0x09, 0x0A,
        0x16, 0x17, 0x18, 0x19, 0x1A, 0x25, 0x26, 0x27, 0x28, 0x29, 0x2A, 0x34, 0x35, 0x36, 0x37,
        0x38, 0x39, 0x3A, 0x43, 0x44, 0x45, 0x46, 0x47, 0x48, 0x49, 0x4A, 0x53, 0x54, 0x55, 0x56,
        0x57, 0x58, 0x59, 0x5A, 0x63, 0x64, 0x65, 0x66, 0x67, 0x68, 0x69, 0x6A, 0x73, 0x74, 0x75,
        0x76, 0x77, 0x78, 0x79, 0x7A, 0x83, 0x84, 0x85, 0x86, 0x87, 0x88, 0x89, 0x8A, 0x92, 0x93,
        0x94, 0x95, 0x96, 0x97, 0x98, 0x99, 0x9A, 0xA2, 0xA3, 0xA4, 0xA5, 0xA6, 0xA7, 0xA8, 0xA9,
        0xAA, 0xB2, 0xB3, 0xB4, 0xB5, 0xB6, 0xB7, 0xB8, 0xB9, 0xBA, 0xC2, 0xC3, 0xC4, 0xC5, 0xC6,
        0xC7, 0xC8, 0xC9, 0xCA, 0xD2, 0xD3, 0xD4, 0xD5, 0xD6, 0xD7, 0xD8, 0xD9, 0xDA, 0xE1, 0xE2,
        0xE3, 0xE4, 0xE5, 0xE6, 0xE7, 0xE8, 0xE9, 0xEA, 0xF1, 0xF2, 0xF3, 0xF4, 0xF5, 0xF6, 0xF7,
        0xF8, 0xF9, 0xFA, 0xFF, 0xDA, 0x00, 0x08, 0x01, 0x01, 0x00, 0x00, 0x3F, 0x00, 0xFB, 0xD5,
        0xDB, 0x20, 0xA8, 0xF2, 0x59, 0xCF, 0xD5, 0x9B, 0x57, 0x14, 0xDB, 0x90, 0xB0, 0xA0, 0x48,
        0x1B, 0x1B, 0x60, 0x19, 0xBF, 0xFF, 0xD9,
    ]
}

// Helper to create a minimal KEPUB file
fn create_test_kepub() -> Vec<u8> {
    // A minimal ZIP file that passes as an EPUB/KEPUB
    // This is just the ZIP structure, not a valid EPUB, but good enough for testing file storage
    vec![
        0x50, 0x4B, 0x03, 0x04, 0x0A, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x21, 0x00, 0x00,
        0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x08, 0x00, 0x00, 0x00,
        0x6D, 0x69, 0x6D, 0x65, 0x74, 0x79, 0x70, 0x65, 0x50, 0x4B, 0x01, 0x02, 0x14, 0x00, 0x0A,
        0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x21, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
        0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x08, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x01,
        0x00, 0x00, 0x00, 0xA4, 0x81, 0x00, 0x00, 0x00, 0x00, 0x6D, 0x69, 0x6D, 0x65, 0x74, 0x79,
        0x70, 0x65, 0x50, 0x4B, 0x05, 0x06, 0x00, 0x00, 0x00, 0x00, 0x01, 0x00, 0x01, 0x00, 0x36,
        0x00, 0x00, 0x00, 0x26, 0x00, 0x00, 0x00, 0x00, 0x00,
    ]
}

#[tokio::test]
async fn test_atomic_ingest_path_traversal_rejected() {
    let server = TestServer::start().await;

    let book_id = Uuid::new_v4();

    let metadata = serde_json::json!({
        "title": "Path Traversal Atomic Test"
    });

    // Attempt path traversal in format field
    // Use a filename without recognized extension so format field is used
    let form = reqwest::multipart::Form::new()
        .text("metadata", metadata.to_string())
        .text("format", "../../../etc/passwd")
        .part(
            "file",
            reqwest::multipart::Part::bytes(create_test_kepub())
                .file_name("book") // No extension - format field will be used
                .mime_str("application/octet-stream")
                .unwrap(),
        );

    let resp = server
        .ingest_client()
        .put_multipart(&format!("/api/v1/books/{}/ingest", book_id), form)
        .await;
    resp.assert_status(StatusCode::BAD_REQUEST);
}
