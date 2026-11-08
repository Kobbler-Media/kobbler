use crate::common::*;
use serde_json::{Value, json};
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

use api::sync_providers::{SyncAction, SyncProvider, SyncProviderRegistry, SyncProviderUrls};

// ============================================================================
// Unit tests: resolve_edition_id (via trait method)
// ============================================================================

/// Helper: create a Hardcover provider instance for unit testing trait methods.
fn create_test_provider() -> Box<dyn SyncProvider> {
    let registry = SyncProviderRegistry::new();
    registry
        .create_provider("hardcover", "test-key")
        .expect("hardcover provider should be registered")
}

#[test]
fn test_resolve_edition_id_prefers_physical() {
    let provider = create_test_provider();
    let metadata = json!({
        "hardcover": {
            "default_ebook_edition_id": 111,
            "default_physical_edition_id": 222,
            "default_cover_edition_id": 333
        }
    });
    assert_eq!(provider.resolve_edition_id(&metadata), Some(222));
}

#[test]
fn test_resolve_edition_id_falls_back_to_physical() {
    let provider = create_test_provider();
    let metadata = json!({
        "hardcover": {
            "default_physical_edition_id": 222,
            "default_cover_edition_id": 333
        }
    });
    assert_eq!(provider.resolve_edition_id(&metadata), Some(222));
}

#[test]
fn test_resolve_edition_id_falls_back_to_cover() {
    let provider = create_test_provider();
    let metadata = json!({
        "hardcover": {
            "default_cover_edition_id": 333
        }
    });
    assert_eq!(provider.resolve_edition_id(&metadata), Some(333));
}

#[test]
fn test_resolve_edition_id_none_when_missing() {
    let provider = create_test_provider();
    let metadata = json!({ "hardcover": {} });
    assert_eq!(provider.resolve_edition_id(&metadata), None);
}

#[test]
fn test_resolve_edition_id_none_when_no_hardcover_key() {
    let provider = create_test_provider();
    let metadata = json!({});
    assert_eq!(provider.resolve_edition_id(&metadata), None);
}

#[test]
fn test_resolve_edition_id_none_for_null_metadata() {
    let provider = create_test_provider();
    let metadata = Value::Null;
    assert_eq!(provider.resolve_edition_id(&metadata), None);
}

#[test]
fn test_resolve_edition_id_skips_null_values() {
    let provider = create_test_provider();
    let metadata = json!({
        "hardcover": {
            "default_ebook_edition_id": null,
            "default_physical_edition_id": 222
        }
    });
    assert_eq!(provider.resolve_edition_id(&metadata), Some(222));
}

// ============================================================================
// Integration tests: edition_id in GraphQL mutations
// ============================================================================

/// Set up a wiremock Hardcover server that returns success for all mutations.
fn hardcover_graphql_response(user_book_id: i64) -> Value {
    json!({
        "data": {
            "insert_user_book": {
                "error": null,
                "user_book": { "id": user_book_id }
            }
        }
    })
}

fn read_insert_response(read_id: i64) -> Value {
    json!({
        "data": {
            "insert_user_book_read": {
                "error": null,
                "user_book_read": { "id": read_id }
            }
        }
    })
}

fn read_update_response(read_id: i64) -> Value {
    json!({
        "data": {
            "update_user_book_read": {
                "error": null,
                "user_book_read": { "id": read_id }
            }
        }
    })
}

/// Extract the GraphQL variables from a captured request body.
fn extract_variables(request: &wiremock::Request) -> Value {
    let body: Value = serde_json::from_slice(&request.body).expect("request body should be JSON");
    body.get("variables").cloned().unwrap_or(Value::Null)
}

/// Extract the GraphQL query string from a captured request body.
fn extract_query(request: &wiremock::Request) -> String {
    let body: Value = serde_json::from_slice(&request.body).expect("request body should be JSON");
    body.get("query")
        .and_then(|q| q.as_str())
        .unwrap_or("")
        .to_string()
}

#[tokio::test]
async fn test_mark_reading_sends_edition_id() {
    let mock_server = MockServer::start().await;

    // Mock both the insert_user_book and any follow-up calls
    Mock::given(method("POST"))
        .and(path("/v1/graphql"))
        .respond_with(ResponseTemplate::new(200).set_body_json(hardcover_graphql_response(42)))
        .expect(1)
        .mount(&mock_server)
        .await;

    let registry = SyncProviderRegistry::with_urls(
        SyncProviderUrls::default().with_override("hardcover", mock_server.uri()),
    );

    let identifiers = json!({ "hardcover": "12345" });
    let provider_metadata = json!({
        "hardcover": {
            "default_ebook_edition_id": 99001
        }
    });

    let result = registry
        .execute_sync(
            "hardcover",
            "test-api-key",
            &identifiers,
            &SyncAction::Started,
            None,
            &provider_metadata,
        )
        .await;

    assert!(result.is_ok(), "Sync should succeed: {:?}", result);

    let requests = mock_server.received_requests().await.expect("requests");
    assert_eq!(requests.len(), 1, "Should have made 1 GraphQL call");

    let vars = extract_variables(&requests[0]);
    let edition_id = vars
        .get("object")
        .and_then(|o| o.get("edition_id"))
        .and_then(|e| e.as_i64());
    assert_eq!(
        edition_id,
        Some(99001),
        "insert_user_book should include edition_id, got variables: {}",
        vars
    );
}

#[tokio::test]
async fn test_mark_reading_omits_edition_id_when_missing() {
    let mock_server = MockServer::start().await;

    Mock::given(method("POST"))
        .and(path("/v1/graphql"))
        .respond_with(ResponseTemplate::new(200).set_body_json(hardcover_graphql_response(42)))
        .expect(1)
        .mount(&mock_server)
        .await;

    let registry = SyncProviderRegistry::with_urls(
        SyncProviderUrls::default().with_override("hardcover", mock_server.uri()),
    );

    let identifiers = json!({ "hardcover": "12345" });
    let provider_metadata = json!({}); // No hardcover metadata

    let result = registry
        .execute_sync(
            "hardcover",
            "test-api-key",
            &identifiers,
            &SyncAction::Started,
            None,
            &provider_metadata,
        )
        .await;

    assert!(result.is_ok());

    let requests = mock_server.received_requests().await.expect("requests");
    let vars = extract_variables(&requests[0]);
    let has_edition_id = vars
        .get("object")
        .and_then(|o| o.get("edition_id"))
        .is_some();
    assert!(
        !has_edition_id,
        "edition_id should be omitted when not in metadata, got variables: {}",
        vars
    );
}

#[tokio::test]
async fn test_update_progress_insert_sends_edition_id() {
    let mock_server = MockServer::start().await;

    // First call: insert_user_book, second call: insert_user_book_read
    Mock::given(method("POST"))
        .and(path("/v1/graphql"))
        .respond_with(ResponseTemplate::new(200).set_body_json(hardcover_graphql_response(42)))
        .up_to_n_times(1)
        .expect(1)
        .mount(&mock_server)
        .await;

    Mock::given(method("POST"))
        .and(path("/v1/graphql"))
        .respond_with(ResponseTemplate::new(200).set_body_json(read_insert_response(1001)))
        .expect(1)
        .mount(&mock_server)
        .await;

    let registry = SyncProviderRegistry::with_urls(
        SyncProviderUrls::default().with_override("hardcover", mock_server.uri()),
    );

    let identifiers = json!({ "hardcover": "12345" });
    let provider_metadata = json!({
        "hardcover": {
            "default_ebook_edition_id": 88001
        }
    });

    let result = registry
        .execute_sync(
            "hardcover",
            "test-api-key",
            &identifiers,
            &SyncAction::Progress {
                percent: 50.0,
                page_count: Some(300),
                started_at: None,
            },
            None, // No existing read ID → INSERT path
            &provider_metadata,
        )
        .await;

    assert!(result.is_ok(), "Sync should succeed: {:?}", result);

    let requests = mock_server.received_requests().await.expect("requests");
    assert_eq!(requests.len(), 2, "Should have 2 calls (user_book + read)");

    // First call: insert_user_book should have edition_id
    let user_book_vars = extract_variables(&requests[0]);
    assert_eq!(
        user_book_vars
            .get("object")
            .and_then(|o| o.get("edition_id"))
            .and_then(|e| e.as_i64()),
        Some(88001),
        "insert_user_book should include edition_id"
    );

    // Second call: insert_user_book_read should have edition_id
    let read_vars = extract_variables(&requests[1]);
    let read_edition = read_vars
        .get("user_book_read")
        .and_then(|r| r.get("edition_id"))
        .and_then(|e| e.as_i64());
    assert_eq!(
        read_edition,
        Some(88001),
        "insert_user_book_read should include edition_id, got variables: {}",
        read_vars
    );
}

#[tokio::test]
async fn test_update_progress_update_sends_edition_id() {
    let mock_server = MockServer::start().await;

    // First call: insert_user_book, second call: update_user_book_read
    Mock::given(method("POST"))
        .and(path("/v1/graphql"))
        .respond_with(ResponseTemplate::new(200).set_body_json(hardcover_graphql_response(42)))
        .up_to_n_times(1)
        .expect(1)
        .mount(&mock_server)
        .await;

    Mock::given(method("POST"))
        .and(path("/v1/graphql"))
        .respond_with(ResponseTemplate::new(200).set_body_json(read_update_response(500)))
        .expect(1)
        .mount(&mock_server)
        .await;

    let registry = SyncProviderRegistry::with_urls(
        SyncProviderUrls::default().with_override("hardcover", mock_server.uri()),
    );

    let identifiers = json!({ "hardcover": "12345" });
    let provider_metadata = json!({
        "hardcover": {
            "default_physical_edition_id": 77001
        }
    });

    let result = registry
        .execute_sync(
            "hardcover",
            "test-api-key",
            &identifiers,
            &SyncAction::Progress {
                percent: 75.0,
                page_count: Some(400),
                started_at: None,
            },
            Some(500), // Existing read ID → UPDATE path
            &provider_metadata,
        )
        .await;

    assert!(result.is_ok(), "Sync should succeed: {:?}", result);

    let requests = mock_server.received_requests().await.expect("requests");
    assert_eq!(requests.len(), 2, "Should have 2 calls (user_book + read)");

    // Second call: update_user_book_read should have edition_id
    let update_query = extract_query(&requests[1]);
    assert!(
        update_query.contains("UpdateUserBookRead"),
        "Second call should be update_user_book_read"
    );
    let update_vars = extract_variables(&requests[1]);
    let update_edition = update_vars
        .get("object")
        .and_then(|o| o.get("edition_id"))
        .and_then(|e| e.as_i64());
    assert_eq!(
        update_edition,
        Some(77001),
        "update_user_book_read should include edition_id, got variables: {}",
        update_vars
    );
}

#[tokio::test]
async fn test_mark_finished_sends_edition_id() {
    let mock_server = MockServer::start().await;

    // First call: insert_user_book, second call: update_user_book_read (finished_at)
    Mock::given(method("POST"))
        .and(path("/v1/graphql"))
        .respond_with(ResponseTemplate::new(200).set_body_json(hardcover_graphql_response(42)))
        .up_to_n_times(1)
        .expect(1)
        .mount(&mock_server)
        .await;

    Mock::given(method("POST"))
        .and(path("/v1/graphql"))
        .respond_with(ResponseTemplate::new(200).set_body_json(read_update_response(500)))
        .expect(1)
        .mount(&mock_server)
        .await;

    let registry = SyncProviderRegistry::with_urls(
        SyncProviderUrls::default().with_override("hardcover", mock_server.uri()),
    );

    let identifiers = json!({ "hardcover": "12345" });
    let provider_metadata = json!({
        "hardcover": {
            "default_cover_edition_id": 66001
        }
    });

    let result = registry
        .execute_sync(
            "hardcover",
            "test-api-key",
            &identifiers,
            &SyncAction::Finished,
            Some(500), // Existing read ID to close
            &provider_metadata,
        )
        .await;

    assert!(result.is_ok(), "Sync should succeed: {:?}", result);

    let requests = mock_server.received_requests().await.expect("requests");
    assert_eq!(
        requests.len(),
        2,
        "Should have 2 calls (user_book + close read)"
    );

    // First call: insert_user_book should have edition_id
    let user_book_vars = extract_variables(&requests[0]);
    assert_eq!(
        user_book_vars
            .get("object")
            .and_then(|o| o.get("edition_id"))
            .and_then(|e| e.as_i64()),
        Some(66001),
        "insert_user_book should include edition_id on finish"
    );

    // Second call: update_user_book_read should have edition_id + finished_at
    let update_vars = extract_variables(&requests[1]);
    let update_edition = update_vars
        .get("object")
        .and_then(|o| o.get("edition_id"))
        .and_then(|e| e.as_i64());
    assert_eq!(
        update_edition,
        Some(66001),
        "update_user_book_read (finish) should include edition_id, got variables: {}",
        update_vars
    );
    assert!(
        update_vars
            .get("object")
            .and_then(|o| o.get("finished_at"))
            .is_some(),
        "update_user_book_read should include finished_at"
    );
}

// ============================================================================
// DB test: get_identifiers_and_metadata
// ============================================================================

#[tokio::test]
async fn test_get_identifiers_and_metadata_returns_both() {
    let server = TestServer::start_api_only().await;
    let book_id = create_book(&server, "Edition ID Book").await;

    // Set identifiers and provider_metadata via direct DB update
    sqlx::query(
        r#"UPDATE books SET
            identifiers = $2,
            provider_metadata = $3
        WHERE id = $1"#,
    )
    .bind(book_id)
    .bind(json!({ "hardcover": "54321" }))
    .bind(json!({
        "hardcover": {
            "default_ebook_edition_id": 11111,
            "default_physical_edition_id": 22222
        }
    }))
    .execute(&server.pool)
    .await
    .expect("update book");

    let result = api::repositories::books::get_identifiers_and_metadata(&server.pool, book_id)
        .await
        .expect("query should succeed");

    let (identifiers, provider_metadata) = result.expect("book should exist");
    assert_eq!(
        identifiers.get("hardcover").and_then(|v| v.as_str()),
        Some("54321"),
        "Should return identifiers"
    );
    assert_eq!(
        provider_metadata
            .get("hardcover")
            .and_then(|h| h.get("default_ebook_edition_id"))
            .and_then(|v| v.as_i64()),
        Some(11111),
        "Should return provider_metadata with edition IDs"
    );
}

#[tokio::test]
async fn test_get_identifiers_and_metadata_returns_empty_for_null_metadata() {
    let server = TestServer::start_api_only().await;
    let book_id = create_book(&server, "No Metadata Book").await;

    let result = api::repositories::books::get_identifiers_and_metadata(&server.pool, book_id)
        .await
        .expect("query should succeed");

    let (identifiers, provider_metadata) = result.expect("book should exist");
    // identifiers defaults to {} on book creation
    assert!(identifiers.is_object(), "Should return identifiers object");
    // provider_metadata COALESCE should return {} for NULL
    assert_eq!(
        provider_metadata,
        json!({}),
        "Should return empty object for null provider_metadata"
    );
}

// ============================================================================
// Pull sync: Hardcover sends ratings as numeric (half stars allowed)
// ============================================================================

#[tokio::test]
async fn test_pull_user_books_accepts_decimal_ratings() {
    let mock_server = MockServer::start().await;

    Mock::given(method("POST"))
        .and(path("/v1/graphql"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "data": { "me": [{ "user_books": [
                { "book_id": 1, "status_id": 3, "rating": 5.0,
                  "updated_at": "2026-11-07T10:35:33.544039+00:00",
                  "user_book_reads": [{ "id": 11, "progress": 100, "finished_at": "2026-11-06" }] },
                { "book_id": 2, "status_id": 3, "rating": 3.5,
                  "updated_at": null, "user_book_reads": [] },
                { "book_id": 3, "status_id": 2, "rating": null,
                  "updated_at": null,
                  "user_book_reads": [{ "id": 33, "progress": 2.01, "finished_at": null }] }
            ]}]}
        })))
        .mount(&mock_server)
        .await;

    let registry = SyncProviderRegistry::with_urls(
        SyncProviderUrls::default().with_override("hardcover", mock_server.uri()),
    );
    let provider = registry
        .create_provider("hardcover", "test-api-key")
        .expect("hardcover provider should be registered");

    let books = provider
        .pull_user_books()
        .await
        .expect("decimal ratings must not fail the pull");

    let ratings: Vec<_> = books
        .iter()
        .map(|b| (b.external_id.as_str(), b.rating))
        .collect();
    assert_eq!(
        ratings,
        vec![("1", Some(5)), ("2", Some(4)), ("3", None)],
        "ratings round to whole stars"
    );
    assert_eq!(books[2].active_read_id, Some(33));
}
