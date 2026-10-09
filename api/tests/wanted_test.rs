use crate::common::*;
use reqwest::StatusCode;
use serde_json::json;

use uuid::Uuid;
use wiremock::matchers::{header, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

#[tokio::test]
async fn test_wanted_requires_auth() {
    let server = TestServer::start_api_only().await;
    let client = TestClient::new(&server.api_url);

    let resp = client.get("/api/v1/admin/wanted").await;
    assert_eq!(resp.status, StatusCode::UNAUTHORIZED);

    let resp = client
        .get("/api/v1/admin/wanted/00000000-0000-0000-0000-000000000000")
        .await;
    assert_eq!(resp.status, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn test_wanted_kobload_flow() {
    let mock_server = MockServer::start().await;

    Mock::given(method("POST"))
        .and(path("/api/v1/search"))
        .and(header("Authorization", "Bearer test-key"))
        .and(wiremock::matchers::body_partial_json(json!({
            "query": "",
            "media_want": {
                "kind": "book",
                "title": "Test Book",
                "author": "Test Author",
                "isbn": "9781234567890"
            }
        })))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "results": [
                {
                    "id": "guid:release-guid",
                    "guid": "release-guid",
                    "title": "Test Book",
                    "size": 12345,
                    "indexer": "MockIndexer",
                    "protocol": "torrent",

                    "profile_type_key": "ebooks",
                    "download_url": "https://example.com/download"
                }
            ]
        })))
        .mount(&mock_server)
        .await;

    let download_id = Uuid::now_v7();
    Mock::given(method("POST"))
        .and(path("/api/v1/downloads"))
        .and(header("Authorization", "Bearer test-key"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "id": download_id,
            "status": "queued"
        })))
        .mount(&mock_server)
        .await;

    let server = TestServer::start_api_only().await;

    let resp = server
        .admin_client()
        .patch_json(
            "/api/v1/admin/config",
            &json!({
                "kobload_base_url": mock_server.uri(),
                "kobload_api_key": "test-key",
                "kobload_ingest_url": "http://localhost:6766/api/v1/ingest/kobload-token"
            }),
        )
        .await;
    resp.assert_ok();

    let create_resp = server
        .admin_client()
        .post_json(
            "/api/v1/admin/wanted",
            &json!({
                "title": "Test Book",
                "author": "Test Author",
                "identifiers": { "isbn13": "9781234567890" }
            }),
        )
        .await;
    create_resp.assert_ok();
    let create_json: serde_json::Value = create_resp.json();
    let wanted_id = create_json["id"].as_str().unwrap();

    let search_resp = server
        .admin_client()
        .get(&format!("/api/v1/admin/books/{}/releases", wanted_id))
        .await;
    search_resp.assert_ok();
    let search_json: serde_json::Value = search_resp.json();
    let releases = search_json["results"].as_array().unwrap();
    assert_eq!(releases.len(), 1);

    let queue_resp = server
        .admin_client()
        .post_json(
            &format!("/api/v1/admin/books/{}/queue", wanted_id),
            &json!({
                "source": "manual",
                "release": releases[0]
            }),
        )
        .await;
    assert_eq!(queue_resp.status, StatusCode::NO_CONTENT);

    let detail_resp = server
        .admin_client()
        .get(&format!("/api/v1/admin/wanted/{}", wanted_id))
        .await;
    detail_resp.assert_ok();
    let detail_json: serde_json::Value = detail_resp.json();
    let attempts = detail_json["attempts"].as_array().unwrap();
    assert_eq!(attempts.len(), 1);
    assert_eq!(attempts[0]["status"], "queued");
    assert_eq!(attempts[0]["external_id"], download_id.to_string());
}

/// A free-text release search and what it sends to Kobload.
///
/// The body matcher is the point of the test: a typed query replaces the structured
/// terms, and Kobload derives its category filter from whichever of the two it is given.
/// Sending neither searched every indexer uncategorised, which is how a book search came
/// back full of TV — so the media type has to travel with the free text, and may only be
/// dropped when the caller asks for it by name.
async fn search_with_free_text(
    want: serde_json::Value,
    query_suffix: &str,
    expected_body: serde_json::Value,
) {
    let mock_server = MockServer::start().await;

    Mock::given(method("POST"))
        .and(path("/api/v1/search"))
        .and(header("Authorization", "Bearer test-key"))
        .and(wiremock::matchers::body_json(expected_body))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "results": [
                {
                    "id": "guid:release-guid-custom",
                    "guid": "release-guid-custom",
                    "title": "Custom Query Result",
                    "size": 123,
                    "indexer": "MockIndexer",
                    "protocol": "usenet",

                    "profile_type_key": "ebooks",
                    "download_url": "https://example.com/custom"
                }
            ]
        })))
        .mount(&mock_server)
        .await;

    let server = TestServer::start_api_only().await;

    let resp = server
        .admin_client()
        .patch_json(
            "/api/v1/admin/config",
            &json!({
                "kobload_base_url": mock_server.uri(),
                "kobload_api_key": "test-key",
                "kobload_ingest_url": "http://localhost:6766/api/v1/ingest/kobload-token"
            }),
        )
        .await;
    resp.assert_ok();

    let create_resp = server
        .admin_client()
        .post_json("/api/v1/admin/wanted", &want)
        .await;
    create_resp.assert_ok();
    let create_json: serde_json::Value = create_resp.json();
    let wanted_id = create_json["id"].as_str().unwrap();

    let search_resp = server
        .admin_client()
        .get(&format!(
            "/api/v1/admin/books/{wanted_id}/releases?q=custom%20search{query_suffix}"
        ))
        .await;
    search_resp.assert_ok();
    let search_json: serde_json::Value = search_resp.json();
    let releases = search_json["results"].as_array().unwrap();
    assert_eq!(releases.len(), 1);
    assert_eq!(releases[0]["title"], "Custom Query Result");
}

fn ebook_want() -> serde_json::Value {
    json!({
        "title": "Waybound",
        "author": "Will Wight",
        "identifiers": { "isbn10": "1959001108" }
    })
}

#[tokio::test]
async fn test_wanted_kobload_override_query() {
    search_with_free_text(
        ebook_want(),
        "",
        json!({ "query": "custom search", "media_type": "ebooks" }),
    )
    .await;
}

#[tokio::test]
async fn test_free_text_search_takes_its_category_from_the_want_format() {
    let mut want = ebook_want();
    want["want_format"] = json!("audiobook");
    search_with_free_text(
        want,
        "",
        json!({ "query": "custom search", "media_type": "audiobooks" }),
    )
    .await;
}

/// The "search all categories" toggle, for a release filed under Other/Misc rather than
/// Books. It has to drop the filter, which is indistinguishable from the accident above
/// unless the request is inspected.
#[tokio::test]
async fn test_scope_any_drops_the_category_filter() {
    search_with_free_text(
        ebook_want(),
        "&scope=any",
        json!({ "query": "custom search" }),
    )
    .await;
}

#[tokio::test]
async fn test_wanted_queue_requires_kobload_config() {
    let server = TestServer::start_api_only().await;

    let create_resp = server
        .admin_client()
        .post_json(
            "/api/v1/admin/wanted",
            &json!({
                "title": "Waybound",
                "author": "Will Wight",
                "identifiers": { "isbn10": "1959001108" }
            }),
        )
        .await;
    create_resp.assert_ok();
    let create_json: serde_json::Value = create_resp.json();
    let wanted_id = create_json["id"].as_str().unwrap();

    let queue_resp = server
        .admin_client()
        .post_json(
            &format!("/api/v1/admin/books/{}/queue", wanted_id),
            &json!({
                "source": "manual",
                "release": {
                    "id": "guid:test-release",
                    "protocol": "torrent",
                    "download_url": "https://example.com/download",
                    "title": "Test Release",
                    "indexer": "MockIndexer",
                    "size": 12345
                }
            }),
        )
        .await;
    assert_eq!(queue_resp.status, StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn test_list_wanted_with_search() {
    let server = TestServer::start_api_only().await;

    // Create multiple wanted items
    let resp = server
        .admin_client()
        .post_json(
            "/api/v1/admin/wanted",
            &json!({
                "title": "The Hobbit",
                "author": "J.R.R. Tolkien"
            }),
        )
        .await;
    resp.assert_ok();

    let resp = server
        .admin_client()
        .post_json(
            "/api/v1/admin/wanted",
            &json!({
                "title": "1984",
                "author": "George Orwell"
            }),
        )
        .await;
    resp.assert_ok();

    let resp = server
        .admin_client()
        .post_json(
            "/api/v1/admin/wanted",
            &json!({
                "title": "Fellowship of the Ring",
                "author": "J.R.R. Tolkien",
                "series": "Lord of the Rings"
            }),
        )
        .await;
    resp.assert_ok();

    // Search by title
    let resp = server
        .admin_client()
        .get("/api/v1/admin/wanted?search=hobbit")
        .await;
    resp.assert_ok();
    let json: serde_json::Value = resp.json();
    let items = json["items"].as_array().unwrap();
    assert_eq!(items.len(), 1);
    assert_eq!(items[0]["title"], "The Hobbit");

    // Search by author
    let resp = server
        .admin_client()
        .get("/api/v1/admin/wanted?search=tolkien")
        .await;
    resp.assert_ok();
    let json: serde_json::Value = resp.json();
    let items = json["items"].as_array().unwrap();
    assert_eq!(items.len(), 2);

    // Search by series
    let resp = server
        .admin_client()
        .get("/api/v1/admin/wanted?search=lord%20of%20the%20rings")
        .await;
    resp.assert_ok();
    let json: serde_json::Value = resp.json();
    let items = json["items"].as_array().unwrap();
    assert_eq!(items.len(), 1);
    assert_eq!(items[0]["title"], "Fellowship of the Ring");
}

#[tokio::test]
async fn test_list_wanted_with_status_filter() {
    let server = TestServer::start_api_only().await;

    // Create a wanted item (now starts as 'wanted')
    let resp = server
        .admin_client()
        .post_json(
            "/api/v1/admin/wanted",
            &json!({
                "title": "Test Book",
                "author": "Test Author"
            }),
        )
        .await;
    resp.assert_ok();

    // Filter by 'wanted' status (the new default status)
    let resp = server
        .admin_client()
        .get("/api/v1/admin/wanted?status=wanted")
        .await;
    resp.assert_ok();
    let json: serde_json::Value = resp.json();
    let items = json["items"].as_array().unwrap();
    assert_eq!(items.len(), 1);

    // Filter by 'pending' status (should be empty)
    let resp = server
        .admin_client()
        .get("/api/v1/admin/wanted?status=pending")
        .await;
    resp.assert_ok();
    let json: serde_json::Value = resp.json();
    let items = json["items"].as_array().unwrap();
    assert_eq!(items.len(), 0);
}

#[tokio::test]
async fn test_list_wanted_includes_status_counts() {
    let server = TestServer::start_api_only().await;

    // Create a wanted item
    let resp = server
        .admin_client()
        .post_json(
            "/api/v1/admin/wanted",
            &json!({
                "title": "Test Book",
                "author": "Test Author"
            }),
        )
        .await;
    resp.assert_ok();

    // List wanted and check status_counts
    let resp = server.admin_client().get("/api/v1/admin/wanted").await;
    resp.assert_ok();
    let json: serde_json::Value = resp.json();

    assert!(json["status_counts"].is_object());
    let counts = &json["status_counts"];
    // New schema uses 'wanted' status for manually created wanted items
    assert_eq!(counts["pending"], 0);
    assert_eq!(counts["wanted"], 1);
    assert_eq!(counts["queued"], 0);
    assert_eq!(counts["downloading"], 0);
    assert_eq!(counts["kobload_error"], 0);
    assert_eq!(counts["needs_review"], 0);
    assert_eq!(counts["completed"], 0);
    assert_eq!(counts["failed"], 0);
}

#[tokio::test]
async fn test_wanted_files_endpoint_requires_auth() {
    let server = TestServer::start_api_only().await;
    let client = TestClient::new(&server.api_url);

    let resp = client
        .get("/api/v1/admin/wanted/00000000-0000-0000-0000-000000000000/files")
        .await;
    assert_eq!(resp.status, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn test_wanted_ingest_files_endpoint_requires_auth() {
    let server = TestServer::start_api_only().await;
    let client = TestClient::new(&server.api_url);

    let resp = client
        .post_json(
            "/api/v1/admin/wanted/00000000-0000-0000-0000-000000000000/ingest-files",
            &json!({
                "as_single_book": true,
                "files": []
            }),
        )
        .await;
    assert_eq!(resp.status, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn test_wanted_skip_endpoint_requires_auth() {
    let server = TestServer::start_api_only().await;
    let client = TestClient::new(&server.api_url);

    let resp = client
        .post_json(
            "/api/v1/admin/wanted/00000000-0000-0000-0000-000000000000/skip",
            &json!({}),
        )
        .await;
    assert_eq!(resp.status, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn test_wanted_files_endpoint_not_found() {
    let server = TestServer::start_api_only().await;

    let resp = server
        .admin_client()
        .get("/api/v1/admin/wanted/00000000-0000-0000-0000-000000000000/files")
        .await;
    assert_eq!(resp.status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn test_wanted_files_endpoint_requires_needs_review_status() {
    let server = TestServer::start_api_only().await;

    // Create a wanted item (starts as 'new')
    let resp = server
        .admin_client()
        .post_json(
            "/api/v1/admin/wanted",
            &json!({
                "title": "Test Book",
                "author": "Test Author"
            }),
        )
        .await;
    resp.assert_ok();
    let json: serde_json::Value = resp.json();
    let wanted_id = json["id"].as_str().unwrap();

    // Try to get files (should fail - status is 'new', not 'needs_review')
    let resp = server
        .admin_client()
        .get(&format!("/api/v1/admin/wanted/{}/files", wanted_id))
        .await;
    // Returns NOT_FOUND because there's no attempt with files_json
    assert_eq!(resp.status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn test_wanted_with_cover_url() {
    let server = TestServer::start_api_only().await;

    // Create a wanted item with cover_url
    let resp = server
        .admin_client()
        .post_json(
            "/api/v1/admin/wanted",
            &json!({
                "title": "Test Book",
                "author": "Test Author",
                "cover_url": "https://example.com/cover.jpg"
            }),
        )
        .await;
    resp.assert_ok();
    let json: serde_json::Value = resp.json();
    let wanted_id = json["id"].as_str().unwrap();

    // Verify cover_url is returned in list
    let resp = server.admin_client().get("/api/v1/admin/wanted").await;
    resp.assert_ok();
    let json: serde_json::Value = resp.json();
    let items = json["items"].as_array().unwrap();
    assert_eq!(items.len(), 1);
    let cover_url = items[0]["cover_url"].as_str().unwrap();
    assert!(
        cover_url.starts_with("/api/v1/library/books/") && cover_url.ends_with("/cover"),
        "Expected local endpoint URL, got: {cover_url}"
    );

    // Verify cover_url is returned in detail
    let resp = server
        .admin_client()
        .get(&format!("/api/v1/admin/wanted/{}", wanted_id))
        .await;
    resp.assert_ok();
    let json: serde_json::Value = resp.json();
    let detail_cover_url = json["item"]["cover_url"].as_str().unwrap();
    assert!(
        detail_cover_url.starts_with("/api/v1/library/books/")
            && detail_cover_url.ends_with("/cover"),
        "Expected local endpoint URL, got: {detail_cover_url}"
    );
}

#[tokio::test]
async fn test_delete_wanted_requires_auth() {
    let server = TestServer::start_api_only().await;
    let client = TestClient::new(&server.api_url);

    let resp = client
        .delete("/api/v1/admin/wanted/00000000-0000-0000-0000-000000000000")
        .await;
    assert_eq!(resp.status, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn test_delete_wanted_not_found() {
    let server = TestServer::start_api_only().await;

    let resp = server
        .admin_client()
        .delete("/api/v1/admin/wanted/00000000-0000-0000-0000-000000000000")
        .await;
    assert_eq!(resp.status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn test_delete_wanted_success() {
    let server = TestServer::start_api_only().await;

    // Create a wanted item
    let create_resp = server
        .admin_client()
        .post_json(
            "/api/v1/admin/wanted",
            &json!({
                "title": "Delete Me",
                "author": "Test Author"
            }),
        )
        .await;
    create_resp.assert_ok();
    let create_json: serde_json::Value = create_resp.json();
    let wanted_id = create_json["id"].as_str().unwrap();

    // Verify it exists
    let get_resp = server
        .admin_client()
        .get(&format!("/api/v1/admin/wanted/{}", wanted_id))
        .await;
    get_resp.assert_ok();

    // Delete it (sets want_status='skipped')
    let delete_resp = server
        .admin_client()
        .delete(&format!("/api/v1/admin/wanted/{}", wanted_id))
        .await;
    assert_eq!(delete_resp.status, StatusCode::NO_CONTENT);

    // Verify it's no longer returned as wanted (filtered out by want_status)
    let get_resp = server
        .admin_client()
        .get(&format!("/api/v1/admin/wanted/{}", wanted_id))
        .await;
    assert_eq!(get_resp.status, StatusCode::NOT_FOUND);
}
