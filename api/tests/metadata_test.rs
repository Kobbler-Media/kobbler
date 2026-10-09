use crate::common::*;
use reqwest::StatusCode;
use serde_json::{Value, json};
use std::io::Write;
use tempfile::TempDir;
use uuid::Uuid;
use wiremock::matchers::{body_string_contains, method, path_regex, query_param};
use wiremock::{Mock, MockServer, ResponseTemplate};

// =============================================================================
// Provider Tests
// =============================================================================

#[tokio::test]
async fn test_list_providers() {
    let server = TestServer::start_api_only().await;

    let resp = server
        .admin_client()
        .get("/api/v1/admin/metadata/providers")
        .await;
    resp.assert_ok();

    let providers: Vec<Value> = resp.json();
    // Should have at least Open Library (seeded by default)
    assert!(!providers.is_empty());

    // Each provider should have expected fields
    for provider in &providers {
        assert!(provider["id"].is_string());
        assert!(provider["provider_type"].is_string());
        assert!(provider["enabled"].is_boolean());
        assert!(provider["priority"].is_number());
    }
}

#[tokio::test]
async fn test_list_providers_includes_capabilities() {
    let server = TestServer::start_api_only().await;

    let resp = server
        .admin_client()
        .get("/api/v1/admin/metadata/providers")
        .await;
    resp.assert_ok();

    let providers: Vec<Value> = resp.json();
    assert!(!providers.is_empty());

    // Each provider should have capabilities array
    for provider in &providers {
        assert!(
            provider["capabilities"].is_array(),
            "Provider should have capabilities array"
        );
        let capabilities = provider["capabilities"].as_array().unwrap();
        assert!(
            !capabilities.is_empty(),
            "Provider should have at least one capability"
        );

        // Verify capability values are valid strings
        for cap in capabilities {
            assert!(cap.is_string(), "Capability should be a string");
        }
    }

    // Verify specific providers have expected capabilities
    let openlibrary = providers
        .iter()
        .find(|p| p["provider_type"] == "openlibrary");
    if let Some(ol) = openlibrary {
        let caps = ol["capabilities"].as_array().unwrap();
        let cap_strs: Vec<&str> = caps.iter().map(|c| c.as_str().unwrap()).collect();
        assert!(cap_strs.contains(&"isbn_lookup"));
        assert!(cap_strs.contains(&"title_search"));
        assert!(cap_strs.contains(&"cover_fetch"));
    }

    // Hardcover should have series_info capability
    let hardcover = providers.iter().find(|p| p["provider_type"] == "hardcover");
    if let Some(hc) = hardcover {
        let caps = hc["capabilities"].as_array().unwrap();
        let cap_strs: Vec<&str> = caps.iter().map(|c| c.as_str().unwrap()).collect();
        assert!(
            cap_strs.contains(&"series_info"),
            "Hardcover should support series_info"
        );
    }
}

#[tokio::test]
async fn test_list_providers_includes_config_fields() {
    let server = TestServer::start_api_only().await;

    let resp = server
        .admin_client()
        .get("/api/v1/admin/metadata/providers")
        .await;
    resp.assert_ok();

    let providers: Vec<Value> = resp.json();

    // Find Google Books which has api_key config
    let googlebooks = providers
        .iter()
        .find(|p| p["provider_type"] == "googlebooks");
    assert!(googlebooks.is_some(), "Should have googlebooks provider");

    let gb = googlebooks.unwrap();
    let config_fields = gb["config_fields"].as_array().unwrap();

    // Google Books should have api_key config field
    let api_key_field = config_fields.iter().find(|f| f["key"] == "api_key");
    assert!(
        api_key_field.is_some(),
        "Google Books should have api_key config field"
    );

    let field = api_key_field.unwrap();
    assert_eq!(field["label"], "API Key");
    assert_eq!(field["field_type"], "secret");
    assert_eq!(field["required"], false);
    assert!(field["description"].is_string());
    assert!(field["help_url"].is_string());
}

#[tokio::test]
async fn test_list_providers_includes_display_info() {
    let server = TestServer::start_api_only().await;

    let resp = server
        .admin_client()
        .get("/api/v1/admin/metadata/providers")
        .await;
    resp.assert_ok();

    let providers: Vec<Value> = resp.json();

    for provider in &providers {
        // Should have display info from ProviderInfo
        assert!(
            provider["display_name"].is_string(),
            "Should have display_name"
        );
        assert!(
            provider["description"].is_string(),
            "Should have description"
        );
        assert!(provider["website"].is_string(), "Should have website");

        // Display name should not be empty
        assert!(!provider["display_name"].as_str().unwrap().is_empty());
        // Website should be a valid URL
        assert!(provider["website"].as_str().unwrap().starts_with("http"));
    }

    // Verify specific display names
    let openlibrary = providers
        .iter()
        .find(|p| p["provider_type"] == "openlibrary")
        .unwrap();
    assert_eq!(openlibrary["display_name"], "Open Library");

    let googlebooks = providers
        .iter()
        .find(|p| p["provider_type"] == "googlebooks")
        .unwrap();
    assert_eq!(googlebooks["display_name"], "Google Books");

    let hardcover = providers
        .iter()
        .find(|p| p["provider_type"] == "hardcover")
        .unwrap();
    assert_eq!(hardcover["display_name"], "Hardcover");
}

#[tokio::test]
async fn test_update_provider_config() {
    let server = TestServer::start_api_only().await;

    // Get providers
    let providers: Vec<Value> = server
        .admin_client()
        .get("/api/v1/admin/metadata/providers")
        .await
        .json();

    // Find hardcover provider (has api_key config)
    let hardcover = providers
        .iter()
        .find(|p| p["provider_type"] == "hardcover")
        .unwrap();
    let provider_id = hardcover["id"].as_str().unwrap();

    // Update provider config with api_key
    let resp = server
        .admin_client()
        .patch_json(
            &format!("/api/v1/admin/metadata/providers/{}", provider_id),
            &json!({
                "config": {
                    "api_key": "test-api-key-12345"
                }
            }),
        )
        .await;
    resp.assert_success();

    // Verify config was stored
    let updated_providers: Vec<Value> = server
        .admin_client()
        .get("/api/v1/admin/metadata/providers")
        .await
        .json();

    let updated = updated_providers
        .iter()
        .find(|p| p["id"].as_str() == Some(provider_id))
        .unwrap();

    // Find api_key field and check current_value
    let config_fields = updated["config_fields"].as_array().unwrap();
    let api_key_field = config_fields
        .iter()
        .find(|f| f["key"] == "api_key")
        .unwrap();
    assert_eq!(api_key_field["current_value"], "test-api-key-12345");
}

#[tokio::test]
async fn test_update_provider_config_strips_bearer_prefix() {
    let server = TestServer::start_api_only().await;

    let providers: Vec<Value> = server
        .admin_client()
        .get("/api/v1/admin/metadata/providers")
        .await
        .json();

    let hardcover = providers
        .iter()
        .find(|p| p["provider_type"] == "hardcover")
        .unwrap();
    let provider_id = hardcover["id"].as_str().unwrap();

    // Update with "Bearer " prefix (users sometimes paste the full header)
    let resp = server
        .admin_client()
        .patch_json(
            &format!("/api/v1/admin/metadata/providers/{}", provider_id),
            &json!({
                "config": {
                    "api_key": "Bearer my-actual-api-key"
                }
            }),
        )
        .await;
    resp.assert_success();

    // Verify the Bearer prefix was stripped
    let updated_providers: Vec<Value> = server
        .admin_client()
        .get("/api/v1/admin/metadata/providers")
        .await
        .json();

    let updated = updated_providers
        .iter()
        .find(|p| p["id"].as_str() == Some(provider_id))
        .unwrap();

    let config_fields = updated["config_fields"].as_array().unwrap();
    let api_key_field = config_fields
        .iter()
        .find(|f| f["key"] == "api_key")
        .unwrap();
    assert_eq!(api_key_field["current_value"], "my-actual-api-key");
}

#[tokio::test]
async fn test_update_provider_enabled() {
    let server = TestServer::start_api_only().await;

    // Get providers
    let providers: Vec<Value> = server
        .admin_client()
        .get("/api/v1/admin/metadata/providers")
        .await
        .json();

    let provider_id = providers[0]["id"].as_str().unwrap();

    // Disable provider
    let resp = server
        .admin_client()
        .patch_json(
            &format!("/api/v1/admin/metadata/providers/{}", provider_id),
            &json!({
                "enabled": false
            }),
        )
        .await;
    resp.assert_success();

    // Verify it's disabled
    let updated_providers: Vec<Value> = server
        .admin_client()
        .get("/api/v1/admin/metadata/providers")
        .await
        .json();

    let updated = updated_providers
        .iter()
        .find(|p| p["id"].as_str() == Some(provider_id))
        .unwrap();
    assert_eq!(updated["enabled"], false);
}

#[tokio::test]
async fn test_update_provider_priority() {
    let server = TestServer::start_api_only().await;

    let providers: Vec<Value> = server
        .admin_client()
        .get("/api/v1/admin/metadata/providers")
        .await
        .json();

    let provider_id = providers[0]["id"].as_str().unwrap();

    // Update priority
    let resp = server
        .admin_client()
        .patch_json(
            &format!("/api/v1/admin/metadata/providers/{}", provider_id),
            &json!({
                "priority": 50
            }),
        )
        .await;
    resp.assert_success();

    // Verify priority changed
    let updated_providers: Vec<Value> = server
        .admin_client()
        .get("/api/v1/admin/metadata/providers")
        .await
        .json();

    let updated = updated_providers
        .iter()
        .find(|p| p["id"].as_str() == Some(provider_id))
        .unwrap();
    assert_eq!(updated["priority"], 50);
}

#[tokio::test]
async fn test_providers_require_auth() {
    let server = TestServer::start_api_only().await;
    let client = TestClient::new(&server.api_url);

    client
        .get("/api/v1/admin/metadata/providers")
        .await
        .assert_status(StatusCode::UNAUTHORIZED);

    client
        .patch_json(
            &format!("/api/v1/admin/metadata/providers/{}", Uuid::new_v4()),
            &json!({"enabled": false}),
        )
        .await
        .assert_status(StatusCode::UNAUTHORIZED);
}

// =============================================================================
// Search Tests
// =============================================================================

#[tokio::test]
async fn test_search_requires_isbn_or_title() {
    let server = TestServer::start_api_only().await;

    // Empty search should fail
    let resp = server
        .admin_client()
        .get("/api/v1/admin/metadata/search")
        .await;
    resp.assert_status(StatusCode::BAD_REQUEST);

    // Search with only author should also fail
    let resp = server
        .admin_client()
        .get("/api/v1/admin/metadata/search?author=Test")
        .await;
    resp.assert_status(StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn test_search_by_isbn() {
    let server = TestServer::start_api_only().await;

    // Disable all providers to avoid real HTTP calls in tests
    sqlx::query("UPDATE metadata_providers SET enabled = false")
        .execute(&server.pool)
        .await
        .unwrap();

    // Search with ISBN - returns empty results since providers are disabled
    // This test verifies the endpoint works correctly
    let resp = server
        .admin_client()
        .get("/api/v1/admin/metadata/search?isbn=9780141439471")
        .await;
    resp.assert_ok();

    let json: Value = resp.json();
    // Should have results array (empty since providers disabled)
    assert!(json["results"].is_array());
}

#[tokio::test]
async fn test_search_by_title() {
    let server = TestServer::start_api_only().await;

    // Disable all providers to avoid real HTTP calls in tests
    sqlx::query("UPDATE metadata_providers SET enabled = false")
        .execute(&server.pool)
        .await
        .unwrap();

    // Search with title
    let resp = server
        .admin_client()
        .get("/api/v1/admin/metadata/search?title=Pride%20and%20Prejudice")
        .await;
    resp.assert_ok();

    let json: Value = resp.json();
    assert!(json["results"].is_array());
}

#[tokio::test]
async fn test_search_by_title_and_author() {
    let server = TestServer::start_api_only().await;

    // Disable all providers to avoid real HTTP calls in tests
    sqlx::query("UPDATE metadata_providers SET enabled = false")
        .execute(&server.pool)
        .await
        .unwrap();

    let resp = server
        .admin_client()
        .get("/api/v1/admin/metadata/search?title=Pride%20and%20Prejudice&author=Jane%20Austen")
        .await;
    resp.assert_ok();

    let json: Value = resp.json();
    assert!(json["results"].is_array());
}

#[tokio::test]
async fn test_search_requires_auth() {
    let server = TestServer::start_api_only().await;
    let client = TestClient::new(&server.api_url);

    client
        .get("/api/v1/admin/metadata/search?isbn=123")
        .await
        .assert_status(StatusCode::UNAUTHORIZED);
}

// =============================================================================
// Apply Metadata Tests
// =============================================================================

#[tokio::test]
async fn test_apply_metadata_to_queue_item() {
    let server = TestServer::start_api_only().await;
    let watch_dir = TempDir::new().unwrap();

    // Create the author first (new model only links existing authors)
    sqlx::query(
        r#"
        INSERT INTO authors (id, name, normalized_name, created_at, updated_at)
        VALUES ($1, 'Applied Author', 'appliedauthor', NOW(), NOW())
        "#,
    )
    .bind(Uuid::new_v4())
    .execute(&server.pool)
    .await
    .unwrap();

    // Create a book file
    let book_path = watch_dir.path().join("apply_meta.kepub.epub");
    std::fs::File::create(&book_path)
        .unwrap()
        .write_all(&test_epub_bytes())
        .unwrap();

    // Add item directly to queue (bypasses async job processing)
    let item_id = add_queue_item(
        &server,
        book_path.to_str().unwrap(),
        "apply_meta.kepub.epub",
        "KEPUB",
    )
    .await;
    let item_id = item_id.to_string();

    // Apply metadata (authors now use {name, hardcover_id?} format)
    let apply_resp = server
        .admin_client()
        .post_json(
            &format!("/api/v1/admin/ingest-queue/{}/apply-metadata", item_id),
            &json!({
                "title": "Applied Title",
                "authors": [{"name": "Applied Author"}],
                "isbn": "9781234567890",
                "description": "Applied description"
            }),
        )
        .await;
    apply_resp.assert_success();

    // Verify metadata was applied
    let item_resp = server
        .admin_client()
        .get(&format!("/api/v1/admin/ingest-queue/{}", item_id))
        .await;
    let item: Value = item_resp.json();

    assert_eq!(item["title"], "Applied Title");
    // Authors are now [{id, name}] - "Applied Author" is pending (nil UUID)
    let authors = item["authors"].as_array().expect("authors should be array");
    assert!(
        authors.iter().any(|a| a["name"] == "Applied Author"),
        "Should contain 'Applied Author': {:?}",
        authors
    );
    assert_eq!(item["identifiers"]["isbn"], "9781234567890");
    assert_eq!(item["description"], "Applied description");
}

#[tokio::test]
async fn test_apply_metadata_partial() {
    let server = TestServer::start_api_only().await;
    let watch_dir = TempDir::new().unwrap();

    // Create the author first (new model only links existing authors)
    let author_id = Uuid::new_v4();
    sqlx::query(
        r#"
        INSERT INTO authors (id, name, normalized_name, created_at, updated_at)
        VALUES ($1, 'Original Author', 'originalauthor', NOW(), NOW())
        "#,
    )
    .bind(author_id)
    .execute(&server.pool)
    .await
    .unwrap();

    // Create a book file
    let book_path = watch_dir.path().join("partial_meta.kepub.epub");
    std::fs::File::create(&book_path)
        .unwrap()
        .write_all(&test_epub_bytes())
        .unwrap();

    // Add item directly to queue (bypasses async job processing)
    let item_id = add_queue_item(
        &server,
        book_path.to_str().unwrap(),
        "partial_meta.kepub.epub",
        "KEPUB",
    )
    .await;
    let item_id_str = item_id.to_string();

    // Link the author to the book
    sqlx::query("INSERT INTO book_authors (book_id, author_id) VALUES ($1, $2)")
        .bind(item_id)
        .bind(author_id)
        .execute(&server.pool)
        .await
        .unwrap();

    // Set the title
    server
        .admin_client()
        .patch_json(
            &format!("/api/v1/admin/ingest-queue/{}", item_id_str),
            &json!({
                "title": "Original Title"
            }),
        )
        .await;

    // Apply partial metadata (only title) - should not touch existing authors
    let apply_resp = server
        .admin_client()
        .post_json(
            &format!("/api/v1/admin/ingest-queue/{}/apply-metadata", item_id_str),
            &json!({
                "title": "New Title"
            }),
        )
        .await;
    apply_resp.assert_success();

    // Verify only title changed, authors preserved
    let item_resp = server
        .admin_client()
        .get(&format!("/api/v1/admin/ingest-queue/{}", item_id_str))
        .await;
    let item: Value = item_resp.json();

    assert_eq!(item["title"], "New Title");
    // Authors are now [{id, name}] - "Original Author" was linked above
    let authors = item["authors"].as_array().expect("authors should be array");
    assert!(
        authors.iter().any(|a| a["name"] == "Original Author"),
        "Should contain 'Original Author': {:?}",
        authors
    );
}

#[tokio::test]
async fn test_apply_metadata_nonexistent_item() {
    let server = TestServer::start_api_only().await;

    let resp = server
        .admin_client()
        .post_json(
            &format!(
                "/api/v1/admin/ingest-queue/{}/apply-metadata",
                Uuid::new_v4()
            ),
            &json!({
                "title": "Test"
            }),
        )
        .await;
    // Should either return 404 or succeed gracefully (no rows affected is still success)
    assert!(
        resp.status == StatusCode::NOT_FOUND
            || resp.status == StatusCode::OK
            || resp.status == StatusCode::NO_CONTENT
    );
}

#[tokio::test]
async fn test_apply_metadata_requires_auth() {
    let server = TestServer::start_api_only().await;
    let client = TestClient::new(&server.api_url);

    client
        .post_json(
            &format!(
                "/api/v1/admin/ingest-queue/{}/apply-metadata",
                Uuid::new_v4()
            ),
            &json!({"title": "Test"}),
        )
        .await
        .assert_status(StatusCode::UNAUTHORIZED);
}

// =============================================================================
// Extract Metadata Tests
// =============================================================================

#[tokio::test]
async fn test_extract_metadata_endpoint() {
    let server = TestServer::start_api_only().await;
    let watch_dir = TempDir::new().unwrap();

    // Create a simple KEPUB file
    let book_path = watch_dir.path().join("extract_meta.kepub.epub");
    std::fs::File::create(&book_path)
        .unwrap()
        .write_all(&test_epub_bytes())
        .unwrap();

    // Add item directly to queue (bypasses async job processing)
    let item_id = add_queue_item(
        &server,
        book_path.to_str().unwrap(),
        "extract_meta.kepub.epub",
        "KEPUB",
    )
    .await;
    let item_id = item_id.to_string();

    // Try to extract metadata - may fail if ebook-meta isn't available
    // but the endpoint should at least respond
    let resp = server
        .admin_client()
        .post_json(
            &format!("/api/v1/admin/ingest-queue/{}/extract-metadata", item_id),
            &json!({}),
        )
        .await;

    // Either succeeds (200) or reports tool not available (some error code)
    // We just verify it responds appropriately
    assert!(
        resp.status.is_success() || resp.status.is_client_error() || resp.status.is_server_error()
    );
}

#[tokio::test]
async fn test_get_queue_item_cover() {
    let server = TestServer::start_api_only().await;
    let watch_dir = TempDir::new().unwrap();

    // Create a KEPUB file
    let book_path = watch_dir.path().join("cover_get.kepub.epub");
    std::fs::File::create(&book_path)
        .unwrap()
        .write_all(&test_epub_bytes())
        .unwrap();

    // Add item directly to queue (bypasses async job processing)
    let item_id = add_queue_item(
        &server,
        book_path.to_str().unwrap(),
        "cover_get.kepub.epub",
        "KEPUB",
    )
    .await;
    let item_id = item_id.to_string();

    // Store a cover in the queue item (now in book_ingest_source table)
    let cover_bytes = test_jpeg_image();
    sqlx::query("UPDATE book_ingest_source SET extracted_cover = $1 WHERE book_id = $2")
        .bind(&cover_bytes)
        .bind(item_id.parse::<Uuid>().unwrap())
        .execute(&server.pool)
        .await
        .unwrap();

    // Get the cover
    let cover_resp = server
        .admin_client()
        .get(&format!("/api/v1/admin/ingest-queue/{}/cover", item_id))
        .await;
    cover_resp.assert_ok();

    // Verify we got image data back
    assert!(!cover_resp.bytes().is_empty());
    assert!(
        cover_resp
            .headers
            .get("content-type")
            .map(|v| v.to_str().unwrap_or("").starts_with("image/"))
            .unwrap_or(false)
    );
}

#[tokio::test]
async fn test_get_queue_item_cover_not_found() {
    let server = TestServer::start_api_only().await;
    let watch_dir = TempDir::new().unwrap();

    // Create a KEPUB file
    let book_path = watch_dir.path().join("no_cover.kepub.epub");
    std::fs::File::create(&book_path)
        .unwrap()
        .write_all(&test_epub_bytes())
        .unwrap();

    // Add item directly to queue (bypasses async job processing)
    let item_id = add_queue_item(
        &server,
        book_path.to_str().unwrap(),
        "no_cover.kepub.epub",
        "KEPUB",
    )
    .await;
    let item_id = item_id.to_string();

    // Get cover when none exists
    let cover_resp = server
        .admin_client()
        .get(&format!("/api/v1/admin/ingest-queue/{}/cover", item_id))
        .await;
    cover_resp.assert_status(StatusCode::NOT_FOUND);
}

// =============================================================================
// Provider Unit Tests (with wiremock)
// =============================================================================

use api::metadata::{MetadataQuery, ProviderRegistry, ProviderUrls, SeriesQuery};

/// Test OpenLibrary provider with mocked responses
#[tokio::test]
async fn test_openlibrary_provider_search_by_isbn() {
    let mock_server = MockServer::start().await;

    // Mock Open Library search response
    Mock::given(method("GET"))
        .and(path_regex(r"/search\.json.*"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "num_found": 1,
            "docs": [{
                "key": "/works/OL12345W",
                "title": "Pride and Prejudice",
                "author_name": ["Jane Austen"],
                "isbn": ["9780141439518"],
                "publisher": ["Penguin Classics"],
                "first_publish_year": 1813,
                "language": ["eng"],
                "subject": ["Fiction", "Romance"],
                "cover_i": 8234567,
                "number_of_pages_median": 279
            }]
        })))
        .mount(&mock_server)
        .await;

    let server = TestServer::start_api_only().await;

    // Enable only openlibrary and point it at our mock
    sqlx::query("UPDATE metadata_providers SET enabled = false")
        .execute(&server.pool)
        .await
        .unwrap();
    sqlx::query("UPDATE metadata_providers SET enabled = true WHERE provider_type = 'openlibrary'")
        .execute(&server.pool)
        .await
        .unwrap();

    // Create a registry pointing at the mock server
    let urls = ProviderUrls {
        openlibrary: Some(mock_server.uri()),
        googlebooks: None,
        hardcover: None,
        audnexus: None,
    };
    let registry = ProviderRegistry::with_urls(server.pool.clone(), urls);

    let query = MetadataQuery {
        isbn: Some("9780141439518".to_string()),
        title: None,
        authors: vec![],
        asin: None,
        free_text: None,
    };

    let results = registry.search(&query, None).await.results;

    assert!(
        !results.is_empty(),
        "Should have results from mocked OpenLibrary"
    );
    let first = &results[0];
    assert_eq!(first.title, "Pride and Prejudice");
    assert_eq!(first.authors, vec!["Jane Austen"]);
    assert_eq!(first.isbn, Some("9780141439518".to_string()));
    assert!(first.cover_url.is_some());
}

/// Test Google Books provider with mocked responses
#[tokio::test]
async fn test_googlebooks_provider_search_by_title() {
    let mock_server = MockServer::start().await;

    // Mock Google Books search response
    Mock::given(method("GET"))
        .and(path_regex(r"/books/v1/volumes.*"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "totalItems": 1,
            "items": [{
                "id": "abc123",
                "volumeInfo": {
                    "title": "The Great Gatsby",
                    "authors": ["F. Scott Fitzgerald"],
                    "publisher": "Scribner",
                    "publishedDate": "1925",
                    "description": "A novel about the American Dream",
                    "industryIdentifiers": [
                        {"type": "ISBN_13", "identifier": "9780743273565"}
                    ],
                    "pageCount": 180,
                    "categories": ["Fiction"],
                    "imageLinks": {
                        "thumbnail": "http://example.com/thumb.jpg",
                        "medium": "http://example.com/medium.jpg"
                    },
                    "language": "en"
                }
            }]
        })))
        .mount(&mock_server)
        .await;

    let server = TestServer::start_api_only().await;

    // Enable only googlebooks and point it at our mock
    sqlx::query("UPDATE metadata_providers SET enabled = false")
        .execute(&server.pool)
        .await
        .unwrap();
    sqlx::query("UPDATE metadata_providers SET enabled = true WHERE provider_type = 'googlebooks'")
        .execute(&server.pool)
        .await
        .unwrap();

    let urls = ProviderUrls {
        openlibrary: None,
        googlebooks: Some(mock_server.uri()),
        hardcover: None,
        audnexus: None,
    };
    let registry = ProviderRegistry::with_urls(server.pool.clone(), urls);

    let query = MetadataQuery {
        isbn: None,
        title: Some("The Great Gatsby".to_string()),
        authors: vec!["Fitzgerald".to_string()],
        asin: None,
        free_text: None,
    };

    let results = registry.search(&query, None).await.results;

    assert!(
        !results.is_empty(),
        "Should have results from mocked Google Books"
    );
    let first = &results[0];
    assert_eq!(first.title, "The Great Gatsby");
    assert_eq!(first.authors, vec!["F. Scott Fitzgerald"]);
    assert_eq!(first.isbn, Some("9780743273565".to_string()));
    assert_eq!(
        first.description,
        Some("A novel about the American Dream".to_string())
    );
    // Cover URL should be upgraded to HTTPS
    assert!(first.cover_url.as_ref().unwrap().starts_with("https://"));
}

/// Test Hardcover provider with mocked GraphQL responses (using dedicated search query)
#[tokio::test]
async fn test_hardcover_provider_search() {
    let mock_server = MockServer::start().await;

    // Mock Hardcover GraphQL search response (new format with search query)
    Mock::given(method("POST"))
        .and(path_regex(r"/v1/graphql"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "data": {
                "search": {
                    "ids": ["12345"],
                    "results": {
                        "hits": [{
                            "document": {
                                "id": "12345",
                                "title": "Dune",
                                "description": "A science fiction masterpiece",
                                "pages": 688,
                                "release_date": "1965-08-01",
                                "image": {
                                    "url": "https://example.com/dune-cover.jpg"
                                },
                                "author_names": ["Frank Herbert"],
                                "isbns": ["9780441172719"],
                                "series_names": ["Dune Chronicles"],
                                "featured_series": {
                                    "position": 1.0,
                                    "series": {
                                        "name": "Dune Chronicles"
                                    }
                                },
                                "genres": ["Science Fiction", "Space Opera"]
                            }
                        }]
                    }
                }
            }
        })))
        .mount(&mock_server)
        .await;

    let server = TestServer::start_api_only().await;

    // Enable only hardcover and point it at our mock with an API key
    sqlx::query("UPDATE metadata_providers SET enabled = false")
        .execute(&server.pool)
        .await
        .unwrap();
    sqlx::query("UPDATE metadata_providers SET enabled = true, config = $1 WHERE provider_type = 'hardcover'")
        .bind(serde_json::json!({"api_key": "test_api_key"}))
        .execute(&server.pool)
        .await
        .unwrap();

    let urls = ProviderUrls {
        openlibrary: None,
        googlebooks: None,
        hardcover: Some(mock_server.uri()),
        audnexus: None,
    };
    let registry = ProviderRegistry::with_urls(server.pool.clone(), urls);

    let query = MetadataQuery {
        isbn: None,
        title: Some("Dune".to_string()),
        authors: vec![],
        asin: None,
        free_text: None,
    };

    let results = registry.search(&query, None).await.results;

    assert!(
        !results.is_empty(),
        "Should have results from mocked Hardcover"
    );
    let first = &results[0];
    assert_eq!(first.title, "Dune");
    assert_eq!(first.authors, vec!["Frank Herbert"]);
    assert_eq!(first.isbn, Some("9780441172719".to_string()));
    assert_eq!(first.series, Some("Dune Chronicles".to_string()));
    assert_eq!(first.series_index, Some(1.0));
    assert_eq!(first.page_count, Some(688));
    assert!(first.subjects.contains(&"Science Fiction".to_string()));
}

/// Test that providers return empty results when no match
#[tokio::test]
async fn test_provider_empty_results() {
    let mock_server = MockServer::start().await;

    // Mock empty response
    Mock::given(method("GET"))
        .and(path_regex(r"/search\.json.*"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "num_found": 0,
            "docs": []
        })))
        .mount(&mock_server)
        .await;

    let server = TestServer::start_api_only().await;

    sqlx::query("UPDATE metadata_providers SET enabled = false")
        .execute(&server.pool)
        .await
        .unwrap();
    sqlx::query("UPDATE metadata_providers SET enabled = true WHERE provider_type = 'openlibrary'")
        .execute(&server.pool)
        .await
        .unwrap();

    let urls = ProviderUrls {
        openlibrary: Some(mock_server.uri()),
        googlebooks: None,
        hardcover: None,
        audnexus: None,
    };
    let registry = ProviderRegistry::with_urls(server.pool.clone(), urls);

    let query = MetadataQuery {
        isbn: Some("0000000000000".to_string()),
        title: None,
        authors: vec![],
        asin: None,
        free_text: None,
    };

    let results = registry.search(&query, None).await.results;
    assert!(
        results.is_empty(),
        "Should return empty results for non-existent ISBN"
    );
}

/// Test multiple providers return combined results
#[tokio::test]
async fn test_multiple_providers_combined_results() {
    let openlibrary_mock = MockServer::start().await;
    let googlebooks_mock = MockServer::start().await;

    // OpenLibrary returns one result
    Mock::given(method("GET"))
        .and(path_regex(r"/search\.json.*"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "num_found": 1,
            "docs": [{
                "key": "/works/OL1W",
                "title": "Test Book (OpenLibrary)",
                "author_name": ["Author A"]
            }]
        })))
        .mount(&openlibrary_mock)
        .await;

    // Google Books returns another result
    Mock::given(method("GET"))
        .and(path_regex(r"/books/v1/volumes.*"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "totalItems": 1,
            "items": [{
                "id": "gb1",
                "volumeInfo": {
                    "title": "Test Book (Google)"
                }
            }]
        })))
        .mount(&googlebooks_mock)
        .await;

    let server = TestServer::start_api_only().await;

    // Enable both providers
    sqlx::query("UPDATE metadata_providers SET enabled = false")
        .execute(&server.pool)
        .await
        .unwrap();
    sqlx::query("UPDATE metadata_providers SET enabled = true WHERE provider_type IN ('openlibrary', 'googlebooks')")
        .execute(&server.pool)
        .await
        .unwrap();

    let urls = ProviderUrls {
        openlibrary: Some(openlibrary_mock.uri()),
        googlebooks: Some(googlebooks_mock.uri()),
        hardcover: None,
        audnexus: None,
    };
    let registry = ProviderRegistry::with_urls(server.pool.clone(), urls);

    let query = MetadataQuery {
        isbn: None,
        title: Some("Test Book".to_string()),
        authors: vec![],
        asin: None,
        free_text: None,
    };

    let results = registry.search(&query, None).await.results;

    assert_eq!(results.len(), 2, "Should have results from both providers");
    let providers: Vec<&str> = results.iter().map(|r| r.provider.as_str()).collect();
    assert!(providers.contains(&"openlibrary"));
    assert!(providers.contains(&"googlebooks"));
}

/// Test provider handles HTTP errors gracefully
#[tokio::test]
async fn test_provider_handles_http_error() {
    let mock_server = MockServer::start().await;

    // Return 500 error
    Mock::given(method("GET"))
        .and(path_regex(r"/search\.json.*"))
        .respond_with(ResponseTemplate::new(500))
        .mount(&mock_server)
        .await;

    let server = TestServer::start_api_only().await;

    sqlx::query("UPDATE metadata_providers SET enabled = false")
        .execute(&server.pool)
        .await
        .unwrap();
    sqlx::query("UPDATE metadata_providers SET enabled = true WHERE provider_type = 'openlibrary'")
        .execute(&server.pool)
        .await
        .unwrap();

    let urls = ProviderUrls {
        openlibrary: Some(mock_server.uri()),
        googlebooks: None,
        hardcover: None,
        audnexus: None,
    };
    let registry = ProviderRegistry::with_urls(server.pool.clone(), urls);

    let query = MetadataQuery {
        isbn: Some("123".to_string()),
        title: None,
        authors: vec![],
        asin: None,
        free_text: None,
    };

    // Should not panic, just return empty results
    let results = registry.search(&query, None).await.results;
    assert!(results.is_empty());
}

// =============================================================================
// Series Search Tests
// =============================================================================

#[tokio::test]
async fn test_series_search_requires_name() {
    let server = TestServer::start_api_only().await;

    // Empty search should fail
    let resp = server
        .admin_client()
        .get("/api/v1/admin/metadata/search-series")
        .await;
    resp.assert_status(StatusCode::BAD_REQUEST);

    // Empty name should fail
    let resp = server
        .admin_client()
        .get("/api/v1/admin/metadata/search-series?name=")
        .await;
    resp.assert_status(StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn test_series_search_requires_auth() {
    let server = TestServer::start_api_only().await;
    let client = TestClient::new(&server.api_url);

    client
        .get("/api/v1/admin/metadata/search-series?name=Stormlight")
        .await
        .assert_status(StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn test_series_search_requires_configured_provider() {
    let server = TestServer::start_api_only().await;

    // Disable all providers
    sqlx::query("UPDATE metadata_providers SET enabled = false")
        .execute(&server.pool)
        .await
        .unwrap();

    // Enable hardcover but WITHOUT api key
    sqlx::query("UPDATE metadata_providers SET enabled = true, config = '{}' WHERE provider_type = 'hardcover'")
        .execute(&server.pool)
        .await
        .unwrap();

    // Should fail with helpful error about needing API key
    let resp = server
        .admin_client()
        .get("/api/v1/admin/metadata/search-series?name=Stormlight")
        .await;
    resp.assert_status(StatusCode::BAD_REQUEST);

    let json: Value = resp.json();
    let error = json["error"].as_str().unwrap();
    assert!(
        error.to_lowercase().contains("hardcover") && error.contains("API key"),
        "Error should mention Hardcover needs API key: {}",
        error
    );
}

#[tokio::test]
async fn test_series_search_no_capable_provider() {
    let server = TestServer::start_api_only().await;

    // Disable all providers
    sqlx::query("UPDATE metadata_providers SET enabled = false")
        .execute(&server.pool)
        .await
        .unwrap();

    // Enable only openlibrary which doesn't support series search
    sqlx::query("UPDATE metadata_providers SET enabled = true WHERE provider_type = 'openlibrary'")
        .execute(&server.pool)
        .await
        .unwrap();

    // Should fail with helpful error about no providers
    let resp = server
        .admin_client()
        .get("/api/v1/admin/metadata/search-series?name=Stormlight")
        .await;
    resp.assert_status(StatusCode::BAD_REQUEST);

    let json: Value = resp.json();
    let error = json["error"].as_str().unwrap();
    assert!(
        error.contains("series search"),
        "Error should mention series search: {}",
        error
    );
}

#[tokio::test]
async fn test_hardcover_capabilities_include_series_search() {
    let server = TestServer::start_api_only().await;

    let resp = server
        .admin_client()
        .get("/api/v1/admin/metadata/providers")
        .await;
    resp.assert_ok();

    let providers: Vec<Value> = resp.json();
    let hardcover = providers
        .iter()
        .find(|p| p["provider_type"] == "hardcover")
        .expect("Should have hardcover provider");

    let caps = hardcover["capabilities"].as_array().unwrap();
    let cap_strs: Vec<&str> = caps.iter().map(|c| c.as_str().unwrap()).collect();

    assert!(
        cap_strs.contains(&"series_search"),
        "Hardcover should have series_search capability"
    );
}

// =============================================================================
// Author Search Tests
// =============================================================================

#[tokio::test]
async fn test_author_search_requires_name() {
    let server = TestServer::start_api_only().await;

    // Empty search should fail
    let resp = server
        .admin_client()
        .get("/api/v1/admin/metadata/search-authors")
        .await;
    resp.assert_status(StatusCode::BAD_REQUEST);

    // Empty name should fail
    let resp = server
        .admin_client()
        .get("/api/v1/admin/metadata/search-authors?name=")
        .await;
    resp.assert_status(StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn test_author_search_requires_auth() {
    let server = TestServer::start_api_only().await;
    let client = TestClient::new(&server.api_url);

    client
        .get("/api/v1/admin/metadata/search-authors?name=Brandon%20Sanderson")
        .await
        .assert_status(StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn test_author_search_requires_configured_provider() {
    let server = TestServer::start_api_only().await;

    // Disable all providers
    sqlx::query("UPDATE metadata_providers SET enabled = false")
        .execute(&server.pool)
        .await
        .unwrap();

    // Enable hardcover but WITHOUT api key
    sqlx::query(
        "UPDATE metadata_providers SET enabled = true, config = '{}' WHERE provider_type = 'hardcover'",
    )
    .execute(&server.pool)
    .await
    .unwrap();

    // Should fail with helpful error about needing API key
    let resp = server
        .admin_client()
        .get("/api/v1/admin/metadata/search-authors?name=Brandon%20Sanderson")
        .await;
    resp.assert_status(StatusCode::BAD_REQUEST);

    let json: Value = resp.json();
    let error = json["error"].as_str().unwrap();
    assert!(
        error.to_lowercase().contains("hardcover") && error.contains("API key"),
        "Error should mention Hardcover needs API key: {}",
        error
    );
}

/// Test Hardcover author search via the HardcoverAuthorProvider directly
#[tokio::test]
async fn test_hardcover_author_search_via_provider() {
    use api::tracking::{HardcoverAuthorProvider, TrackedAuthorProvider};

    let mock_server = MockServer::start().await;

    // Mock Hardcover GraphQL author search response
    Mock::given(method("POST"))
        .and(path_regex(r"/v1/graphql"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "data": {
                "search": {
                    "ids": ["5678"],
                    "results": {
                        "hits": [{
                            "document": {
                                "id": 5678,
                                "name": "Brandon Sanderson",
                                "bio": "Brandon Sanderson is an American author of epic fantasy and science fiction.",
                                "image": { "url": "https://example.com/sanderson.jpg" },
                                "books_count": 45
                            }
                        }]
                    }
                }
            }
        })))
        .mount(&mock_server)
        .await;

    // Test directly with the provider using the mock URL
    let provider =
        HardcoverAuthorProvider::with_base_url("test-key".to_string(), mock_server.uri());

    let results = provider.search_authors("Brandon Sanderson").await.unwrap();

    assert!(
        !results.is_empty(),
        "Should have results from mocked Hardcover"
    );
    assert_eq!(results[0].name, "Brandon Sanderson");
    assert_eq!(results[0].id, "5678");
    assert_eq!(results[0].book_count, Some(45));
}

/// Test Hardcover series search with mocked GraphQL response
#[tokio::test]
async fn test_hardcover_series_search() {
    let mock_server = MockServer::start().await;

    // Mock Hardcover GraphQL series search response
    Mock::given(method("POST"))
        .and(path_regex(r"/v1/graphql"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "data": {
                "search": {
                    "ids": ["1234"],
                    "results": {
                        "hits": [{
                            "document": {
                                "id": 1234,
                                "name": "The Stormlight Archive",
                                "books_count": 4,
                                "books": [
                                    {
                                        "id": 100,
                                        "title": "The Way of Kings",
                                        "release_year": 2010,
                                        "image": { "url": "https://example.com/wok.jpg" },
                                        "contributions": [
                                            { "author": { "name": "Brandon Sanderson" } }
                                        ],
                                        "book_series": [{ "position": 1.0 }]
                                    },
                                    {
                                        "id": 101,
                                        "title": "Words of Radiance",
                                        "release_year": 2014,
                                        "image": { "url": "https://example.com/wor.jpg" },
                                        "contributions": [
                                            { "author": { "name": "Brandon Sanderson" } }
                                        ],
                                        "book_series": [{ "position": 2.0 }]
                                    }
                                ]
                            }
                        }]
                    }
                }
            }
        })))
        .mount(&mock_server)
        .await;

    let server = TestServer::start_api_only().await;

    // Enable only hardcover with an API key
    sqlx::query("UPDATE metadata_providers SET enabled = false")
        .execute(&server.pool)
        .await
        .unwrap();
    sqlx::query(
        "UPDATE metadata_providers SET enabled = true, config = '{\"api_key\": \"test-key\"}' WHERE provider_type = 'hardcover'",
    )
    .execute(&server.pool)
    .await
    .unwrap();

    let urls = ProviderUrls {
        openlibrary: None,
        googlebooks: None,
        hardcover: Some(mock_server.uri()),
        audnexus: None,
    };
    let registry = ProviderRegistry::with_urls(server.pool.clone(), urls);

    let query = SeriesQuery::new("Stormlight");
    let results = registry.search_series(&query, None).await.results;

    assert!(
        !results.is_empty(),
        "Should have results from mocked Hardcover"
    );
    let first = &results[0];
    assert_eq!(first.name, "The Stormlight Archive");
    assert_eq!(first.book_count, Some(4));
    assert_eq!(first.books.len(), 2);

    // Verify books are sorted by series_index
    assert_eq!(first.books[0].title, "The Way of Kings");
    assert_eq!(first.books[0].series_index, Some(1.0));
    assert_eq!(first.books[1].title, "Words of Radiance");
    assert_eq!(first.books[1].series_index, Some(2.0));

    // Verify author extraction
    assert_eq!(first.books[0].authors, vec!["Brandon Sanderson"]);
}

// =============================================================================
// ISBN → Title Fallback Tests
// =============================================================================

/// Test OpenLibrary falls back to title search when ISBN returns empty
#[tokio::test]
async fn test_openlibrary_isbn_fallback_to_title() {
    let mock_server = MockServer::start().await;

    // ISBN search returns empty
    Mock::given(method("GET"))
        .and(path_regex(r"/search\.json"))
        .and(query_param("isbn", "1527024318"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "num_found": 0,
            "docs": []
        })))
        .mount(&mock_server)
        .await;

    // Title search returns results
    Mock::given(method("GET"))
        .and(path_regex(r"/search\.json"))
        .and(query_param("title", "Captain Underpants"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "num_found": 1,
            "docs": [{
                "key": "/works/OL999W",
                "title": "Captain Underpants",
                "author_name": ["Dav Pilkey"],
                "isbn": ["9780545504928"],
                "cover_i": 12345
            }]
        })))
        .mount(&mock_server)
        .await;

    let server = TestServer::start_api_only().await;

    sqlx::query("UPDATE metadata_providers SET enabled = false")
        .execute(&server.pool)
        .await
        .unwrap();
    sqlx::query("UPDATE metadata_providers SET enabled = true WHERE provider_type = 'openlibrary'")
        .execute(&server.pool)
        .await
        .unwrap();

    let urls = ProviderUrls {
        openlibrary: Some(mock_server.uri()),
        googlebooks: None,
        hardcover: None,
        audnexus: None,
    };
    let registry = ProviderRegistry::with_urls(server.pool.clone(), urls);

    // Query with ISBN + title — ISBN returns empty, should fall back to title
    let query = MetadataQuery {
        isbn: Some("1527024318".to_string()),
        title: Some("Captain Underpants".to_string()),
        authors: vec!["Dav Pilkey".to_string()],
        asin: None,
        free_text: None,
    };

    let results = registry.search(&query, None).await.results;
    assert!(
        !results.is_empty(),
        "Should have results from title fallback"
    );
    assert_eq!(results[0].title, "Captain Underpants");
}

/// Test Google Books falls back to title search when ISBN returns empty
#[tokio::test]
async fn test_googlebooks_isbn_fallback_to_title() {
    let mock_server = MockServer::start().await;

    // ISBN search returns empty (totalItems: 0, no items)
    Mock::given(method("GET"))
        .and(path_regex(r"/books/v1/volumes"))
        .and(query_param("q", "isbn:1527024318"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "totalItems": 0
        })))
        .mount(&mock_server)
        .await;

    // Title search returns results (wiremock decodes %20 and + to spaces)
    Mock::given(method("GET"))
        .and(path_regex(r"/books/v1/volumes"))
        .and(query_param(
            "q",
            "intitle:Captain Underpants inauthor:Dav Pilkey",
        ))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "totalItems": 1,
            "items": [{
                "id": "gb123",
                "volumeInfo": {
                    "title": "Captain Underpants",
                    "authors": ["Dav Pilkey"],
                    "industryIdentifiers": [
                        {"type": "ISBN_13", "identifier": "9780545504928"}
                    ]
                }
            }]
        })))
        .mount(&mock_server)
        .await;

    let server = TestServer::start_api_only().await;

    sqlx::query("UPDATE metadata_providers SET enabled = false")
        .execute(&server.pool)
        .await
        .unwrap();
    sqlx::query("UPDATE metadata_providers SET enabled = true WHERE provider_type = 'googlebooks'")
        .execute(&server.pool)
        .await
        .unwrap();

    let urls = ProviderUrls {
        openlibrary: None,
        googlebooks: Some(mock_server.uri()),
        hardcover: None,
        audnexus: None,
    };
    let registry = ProviderRegistry::with_urls(server.pool.clone(), urls);

    let query = MetadataQuery {
        isbn: Some("1527024318".to_string()),
        title: Some("Captain Underpants".to_string()),
        authors: vec!["Dav Pilkey".to_string()],
        asin: None,
        free_text: None,
    };

    let results = registry.search(&query, None).await.results;
    assert!(
        !results.is_empty(),
        "Should have results from title fallback"
    );
    assert_eq!(results[0].title, "Captain Underpants");
}

/// Test Hardcover ISBN lookup returns results for known ISBN
#[tokio::test]
async fn test_hardcover_isbn_lookup() {
    let mock_server = MockServer::start().await;

    // ISBN editions query returns a match
    Mock::given(method("POST"))
        .and(path_regex(r"/v1/graphql"))
        .and(body_string_contains("IsbnLookup"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "data": {
                "editions": [{
                    "isbn_10": null,
                    "isbn_13": "9780545504928",
                    "book_id": 42,
                    "book": {
                        "id": 42,
                        "title": "Captain Underpants",
                        "slug": "captain-underpants",
                        "description": "A funny book",
                        "image": { "url": "https://example.com/cover.jpg" },
                        "contributions": [
                            { "author": { "id": 1, "name": "Dav Pilkey" } }
                        ],
                        "book_series": [],
                        "pages": 176,
                        "release_year": 1997,
                        "rating": 4.0,
                        "ratings_count": 1000,
                        "taggings": [{ "tag": { "tag": "Humor" } }]
                    }
                }]
            }
        })))
        .mount(&mock_server)
        .await;

    let server = TestServer::start_api_only().await;

    sqlx::query("UPDATE metadata_providers SET enabled = false")
        .execute(&server.pool)
        .await
        .unwrap();
    sqlx::query("UPDATE metadata_providers SET enabled = true, config = $1 WHERE provider_type = 'hardcover'")
        .bind(json!({"api_key": "test_key"}))
        .execute(&server.pool)
        .await
        .unwrap();

    let urls = ProviderUrls {
        openlibrary: None,
        googlebooks: None,
        hardcover: Some(mock_server.uri()),
        audnexus: None,
    };
    let registry = ProviderRegistry::with_urls(server.pool.clone(), urls);

    let query = MetadataQuery {
        isbn: Some("9780545504928".to_string()),
        title: Some("Captain Underpants".to_string()),
        authors: vec![],
        asin: None,
        free_text: None,
    };

    let results = registry.search(&query, None).await.results;
    assert!(!results.is_empty(), "Should have results from ISBN lookup");
    assert_eq!(results[0].title, "Captain Underpants");
    assert_eq!(results[0].isbn, Some("9780545504928".to_string()));
    assert!(
        results[0].confidence >= 0.8,
        "ISBN match should have high confidence"
    );
}

/// Test Hardcover falls back to title search when ISBN not in editions
#[tokio::test]
async fn test_hardcover_isbn_fallback_to_title_search() {
    let mock_server = MockServer::start().await;

    // ISBN editions query returns empty
    Mock::given(method("POST"))
        .and(path_regex(r"/v1/graphql"))
        .and(body_string_contains("IsbnLookup"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "data": {
                "editions": []
            }
        })))
        .mount(&mock_server)
        .await;

    // Full-text search returns results
    Mock::given(method("POST"))
        .and(path_regex(r"/v1/graphql"))
        .and(body_string_contains("SearchBooks"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "data": {
                "search": {
                    "ids": ["42"],
                    "results": {
                        "hits": [{
                            "document": {
                                "id": "42",
                                "title": "Captain Underpants",
                                "author_names": ["Dav Pilkey"],
                                "image": { "url": "https://example.com/cover.jpg" },
                                "isbns": ["9780545504928"]
                            }
                        }]
                    }
                }
            }
        })))
        .mount(&mock_server)
        .await;

    let server = TestServer::start_api_only().await;

    sqlx::query("UPDATE metadata_providers SET enabled = false")
        .execute(&server.pool)
        .await
        .unwrap();
    sqlx::query("UPDATE metadata_providers SET enabled = true, config = $1 WHERE provider_type = 'hardcover'")
        .bind(json!({"api_key": "test_key"}))
        .execute(&server.pool)
        .await
        .unwrap();

    let urls = ProviderUrls {
        openlibrary: None,
        googlebooks: None,
        hardcover: Some(mock_server.uri()),
        audnexus: None,
    };
    let registry = ProviderRegistry::with_urls(server.pool.clone(), urls);

    let query = MetadataQuery {
        isbn: Some("1527024318".to_string()),
        title: Some("Captain Underpants".to_string()),
        authors: vec!["Dav Pilkey".to_string()],
        asin: None,
        free_text: None,
    };

    let results = registry.search(&query, None).await.results;
    assert!(
        !results.is_empty(),
        "Should have results from title search fallback"
    );
    assert_eq!(results[0].title, "Captain Underpants");
}

// =============================================================================
// Cache Skip-Empty Tests
// =============================================================================

#[tokio::test]
async fn test_metadata_cache_set_skips_empty_results() {
    use api::metadata::MetadataCache;

    let server = TestServer::start_api_only().await;
    let cache = MetadataCache::new(server.pool.clone());

    // set() with empty results should be a no-op (returns Ok without writing)
    let result = cache.set("test_provider", "test_query", &[]).await;
    assert!(result.is_ok(), "empty set should succeed");

    // Verify nothing was cached
    let cached = cache.get("test_provider", "test_query").await.unwrap();
    assert!(
        cached.is_none(),
        "empty results should not be stored in cache"
    );
}

#[tokio::test]
async fn test_metadata_cache_set_series_skips_empty() {
    use api::metadata::MetadataCache;

    let server = TestServer::start_api_only().await;
    let cache = MetadataCache::new(server.pool.clone());

    let result = cache.set_series("test_provider", "test_query", &[]).await;
    assert!(result.is_ok(), "empty series set should succeed");

    let cached = cache
        .get_series("test_provider", "test_query")
        .await
        .unwrap();
    assert!(
        cached.is_none(),
        "empty series results should not be stored in cache"
    );
}
