use crate::common::*;
use reqwest::StatusCode;
use serde_json::Value;

// =============================================================================
// Books Tests
// =============================================================================

#[tokio::test]
async fn test_list_books_empty() {
    let server = TestServer::start_api_only().await;

    let resp = server.admin_client().get("/api/v1/library/books").await;
    resp.assert_ok();

    let json: Value = resp.json();
    assert!(json["items"].as_array().unwrap().is_empty());
    assert!(json["next_cursor"].is_null());
}

#[tokio::test]
async fn test_list_books_with_items() {
    let server = TestServer::start_api_only().await;

    let id1 = create_book(&server, "Test Book 1").await;
    create_stub_book_file(&server.pool, id1).await;
    let id2 = create_book(&server, "Test Book 2").await;
    create_stub_book_file(&server.pool, id2).await;

    let resp = server.admin_client().get("/api/v1/library/books").await;
    resp.assert_ok();

    let json: Value = resp.json();
    let items = json["items"].as_array().unwrap();
    assert_eq!(items.len(), 2);
}

#[tokio::test]
async fn test_list_books_pagination() {
    let server = TestServer::start_api_only().await;

    for i in 0..5 {
        let id = create_book(&server, &format!("Book {}", i)).await;
        create_stub_book_file(&server.pool, id).await;
    }

    let resp = server
        .admin_client()
        .get("/api/v1/library/books?limit=2")
        .await;
    resp.assert_ok();

    let json: Value = resp.json();
    let items = json["items"].as_array().unwrap();
    assert_eq!(items.len(), 2);
    assert!(json["next_cursor"].is_string());

    // Fetch next page
    let cursor = json["next_cursor"].as_str().unwrap();
    let resp = server
        .admin_client()
        .get(&format!("/api/v1/library/books?limit=2&cursor={}", cursor))
        .await;
    resp.assert_ok();

    let json: Value = resp.json();
    let items = json["items"].as_array().unwrap();
    assert_eq!(items.len(), 2);
}

#[tokio::test]
async fn test_list_books_search() {
    let server = TestServer::start_api_only().await;

    let id = create_book(&server, "The Great Gatsby").await;
    create_stub_book_file(&server.pool, id).await;
    let id = create_book(&server, "1984").await;
    create_stub_book_file(&server.pool, id).await;
    let id = create_book(&server, "Great Expectations").await;
    create_stub_book_file(&server.pool, id).await;

    let resp = server
        .admin_client()
        .get("/api/v1/library/books?search=Great")
        .await;
    resp.assert_ok();

    let json: Value = resp.json();
    let items = json["items"].as_array().unwrap();
    assert_eq!(items.len(), 2);
}

#[tokio::test]
async fn test_list_books_search_negation() {
    let server = TestServer::start_api_only().await;

    let id = create_book(&server, "The Great Gatsby").await;
    create_stub_book_file(&server.pool, id).await;
    let id = create_book(&server, "1984").await;
    create_stub_book_file(&server.pool, id).await;
    let id = create_book(&server, "Great Expectations").await;
    create_stub_book_file(&server.pool, id).await;

    let resp = server
        .admin_client()
        .get("/api/v1/library/books?search=Great%20!Gatsby")
        .await;
    resp.assert_ok();

    let json: Value = resp.json();
    let items = json["items"].as_array().unwrap();
    assert_eq!(items.len(), 1);
    assert_eq!(items[0]["title"], "Great Expectations");
}

#[tokio::test]
async fn test_list_books_requires_auth() {
    let server = TestServer::start_api_only().await;
    let client = TestClient::new(&server.api_url);

    client
        .get("/api/v1/library/books")
        .await
        .assert_status(StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn test_update_book() {
    let server = TestServer::start_api_only().await;
    let book_id = create_book(&server, "Original Title").await;
    create_stub_book_file(&server.pool, book_id).await;

    let resp = server
        .admin_client()
        .patch_json(
            &format!("/api/v1/library/books/{}", book_id),
            &serde_json::json!({
                "title": "Updated Title",
                "description": "A great book"
            }),
        )
        .await;
    resp.assert_status(StatusCode::NO_CONTENT);

    // Verify the update
    let resp = server.admin_client().get("/api/v1/library/books").await;
    let json: Value = resp.json();
    let items = json["items"].as_array().unwrap();
    assert_eq!(items[0]["title"], "Updated Title");
    assert_eq!(items[0]["description"], "A great book");
}

#[tokio::test]
async fn test_update_book_not_found() {
    let server = TestServer::start_api_only().await;

    let resp = server
        .admin_client()
        .patch_json(
            &format!("/api/v1/library/books/{}", uuid::Uuid::new_v4()),
            &serde_json::json!({ "title": "New Title" }),
        )
        .await;
    resp.assert_status(StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn test_update_book_requires_auth() {
    let server = TestServer::start_api_only().await;
    let book_id = create_book(&server, "Test Book").await;
    let client = TestClient::new(&server.api_url);

    client
        .patch_json(
            &format!("/api/v1/library/books/{}", book_id),
            &serde_json::json!({ "title": "Hacked" }),
        )
        .await
        .assert_status(StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn test_touch_books() {
    let server = TestServer::start_api_only().await;
    let book_id = create_book(&server, "Touch Me").await;

    server
        .admin_client()
        .post_json(
            "/api/v1/library/books/touch",
            &serde_json::json!({ "book_ids": [book_id] }),
        )
        .await
        .assert_status(StatusCode::NO_CONTENT);
}

// =============================================================================
// Series Tests
// =============================================================================

#[tokio::test]
async fn test_list_series_empty() {
    let server = TestServer::start_api_only().await;

    let resp = server.admin_client().get("/api/v1/library/series").await;
    resp.assert_ok();

    let json: Value = resp.json();
    assert!(json["items"].as_array().unwrap().is_empty());
}

#[tokio::test]
async fn test_list_series_with_items() {
    let server = TestServer::start_api_only().await;

    let id = create_book_in_series(&server, "Book 1", "My Series", 1).await;
    create_stub_book_file(&server.pool, id).await;
    let id = create_book_in_series(&server, "Book 2", "My Series", 2).await;
    create_stub_book_file(&server.pool, id).await;

    let resp = server.admin_client().get("/api/v1/library/series").await;
    resp.assert_ok();

    let json: Value = resp.json();
    let items = json["items"].as_array().unwrap();
    assert_eq!(items.len(), 1);
    assert_eq!(items[0]["name"], "My Series");
    assert_eq!(items[0]["book_count"], 2);
}

#[tokio::test]
async fn test_create_series() {
    let server = TestServer::start_api_only().await;

    let resp = server
        .admin_client()
        .post_json(
            "/api/v1/library/series",
            &serde_json::json!({ "name": "New Series" }),
        )
        .await;
    resp.assert_ok();

    let json: Value = resp.json();
    let series_id = json["id"].as_str().expect("id should be a string");
    assert!(!series_id.is_empty());

    // Verify series was created (include_no_books=true to see series without library books)
    let resp = server
        .admin_client()
        .get("/api/v1/library/series?include_no_books=true")
        .await;
    let json: Value = resp.json();
    let items = json["items"].as_array().unwrap();
    assert_eq!(items.len(), 1);
    assert_eq!(items[0]["name"], "New Series");
}

#[tokio::test]
async fn test_create_series_idempotent() {
    let server = TestServer::start_api_only().await;

    let resp1 = server
        .admin_client()
        .post_json(
            "/api/v1/library/series",
            &serde_json::json!({ "name": "Same Series" }),
        )
        .await;
    resp1.assert_ok();
    let json1: Value = resp1.json();
    let id1 = json1["id"].as_str().expect("id should be a string");

    let resp2 = server
        .admin_client()
        .post_json(
            "/api/v1/library/series",
            &serde_json::json!({ "name": "Same Series" }),
        )
        .await;
    resp2.assert_ok();
    let json2: Value = resp2.json();
    let id2 = json2["id"].as_str().expect("id should be a string");

    assert_eq!(id1, id2);
}

#[tokio::test]
async fn test_create_series_empty_name() {
    let server = TestServer::start_api_only().await;

    let resp = server
        .admin_client()
        .post_json("/api/v1/library/series", &serde_json::json!({ "name": "" }))
        .await;
    resp.assert_status(StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn test_rename_series() {
    let server = TestServer::start_api_only().await;

    let resp = server
        .admin_client()
        .post_json(
            "/api/v1/library/series",
            &serde_json::json!({ "name": "Old Name" }),
        )
        .await;
    let json: Value = resp.json();
    let series_id = json["id"].as_str().expect("id should be a string");

    server
        .admin_client()
        .patch_json(
            &format!("/api/v1/library/series/{}", series_id),
            &serde_json::json!({ "name": "New Name" }),
        )
        .await
        .assert_status(StatusCode::NO_CONTENT);

    // include_no_books=true to see series without library books
    let resp = server
        .admin_client()
        .get("/api/v1/library/series?include_no_books=true")
        .await;
    let json: Value = resp.json();
    let items = json["items"].as_array().unwrap();
    assert_eq!(items[0]["name"], "New Name");
}

#[tokio::test]
async fn test_merge_series() {
    let server = TestServer::start_api_only().await;

    let id = create_book_in_series(&server, "Book A", "Source Series", 1).await;
    create_stub_book_file(&server.pool, id).await;
    let id = create_book_in_series(&server, "Book B", "Target Series", 1).await;
    create_stub_book_file(&server.pool, id).await;

    let resp = server.admin_client().get("/api/v1/library/series").await;
    let json: Value = resp.json();
    let items = json["items"].as_array().unwrap();
    assert_eq!(items.len(), 2);

    let source_id = items.iter().find(|s| s["name"] == "Source Series").unwrap()["id"]
        .as_str()
        .unwrap();
    let target_id = items.iter().find(|s| s["name"] == "Target Series").unwrap()["id"]
        .as_str()
        .unwrap();

    server
        .admin_client()
        .post_json(
            "/api/v1/library/series/merge",
            &serde_json::json!({
                "source_id": source_id,
                "target_id": target_id
            }),
        )
        .await
        .assert_status(StatusCode::NO_CONTENT);

    // Verify merge
    let resp = server.admin_client().get("/api/v1/library/series").await;
    let json: Value = resp.json();
    let items = json["items"].as_array().unwrap();
    assert_eq!(items.len(), 1);
    assert_eq!(items[0]["name"], "Target Series");
    assert_eq!(items[0]["book_count"], 2);
}

#[tokio::test]
async fn test_merge_series_same_id() {
    let server = TestServer::start_api_only().await;

    let resp = server
        .admin_client()
        .post_json(
            "/api/v1/library/series",
            &serde_json::json!({ "name": "Test Series" }),
        )
        .await;
    let json: Value = resp.json();
    let series_id = json["id"].as_str().expect("id should be a string");

    server
        .admin_client()
        .post_json(
            "/api/v1/library/series/merge",
            &serde_json::json!({
                "source_id": series_id,
                "target_id": series_id
            }),
        )
        .await
        .assert_status(StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn test_list_series_books() {
    let server = TestServer::start_api_only().await;

    let id = create_book_in_series(&server, "Book 1", "Test Series", 1).await;
    create_stub_book_file(&server.pool, id).await;
    let id = create_book_in_series(&server, "Book 2", "Test Series", 2).await;
    create_stub_book_file(&server.pool, id).await;
    let id = create_book(&server, "Standalone Book").await;
    create_stub_book_file(&server.pool, id).await;

    let resp = server.admin_client().get("/api/v1/library/series").await;
    let json: Value = resp.json();
    let series_id = json["items"][0]["id"].as_str().unwrap();

    let resp = server
        .admin_client()
        .get(&format!("/api/v1/library/series/{}/books", series_id))
        .await;
    resp.assert_ok();

    let books: Vec<Value> = resp.json();
    assert_eq!(books.len(), 2);
}

#[tokio::test]
async fn test_reorder_series() {
    let server = TestServer::start_api_only().await;

    let book_id = create_book_in_series(&server, "Book 1", "Test Series", 1).await;
    create_stub_book_file(&server.pool, book_id).await;

    let resp = server.admin_client().get("/api/v1/library/series").await;
    let json: Value = resp.json();
    let series_id = json["items"][0]["id"].as_str().unwrap();

    server
        .admin_client()
        .post_json(
            &format!("/api/v1/library/series/{}/reorder", series_id),
            &serde_json::json!({
                "book_id": book_id,
                "series_index": 5.5
            }),
        )
        .await
        .assert_status(StatusCode::NO_CONTENT);

    // Verify reorder
    let resp = server
        .admin_client()
        .get(&format!("/api/v1/library/series/{}/books", series_id))
        .await;
    let books: Vec<Value> = resp.json();
    assert_eq!(books[0]["series_index"], 5.5);
}

// =============================================================================
// Shelves Tests
// =============================================================================

#[tokio::test]
async fn test_list_shelves_empty() {
    let server = TestServer::start_api_only().await;

    let resp = server.admin_client().get("/api/v1/library/shelves").await;
    resp.assert_ok();

    let json: Value = resp.json();
    assert!(json["items"].as_array().unwrap().is_empty());
}

#[tokio::test]
async fn test_create_shelf() {
    let server = TestServer::start_api_only().await;

    let resp = server
        .admin_client()
        .post_json(
            "/api/v1/library/shelves",
            &serde_json::json!({ "name": "My Shelf" }),
        )
        .await;
    resp.assert_ok();

    let shelf_id: String = resp.json();
    assert!(!shelf_id.is_empty());

    let resp = server.admin_client().get("/api/v1/library/shelves").await;
    let json: Value = resp.json();
    let items = json["items"].as_array().unwrap();
    assert_eq!(items.len(), 1);
    assert_eq!(items[0]["name"], "My Shelf");
}

#[tokio::test]
async fn test_create_shelf_allows_duplicate_names() {
    let server = TestServer::start_api_only().await;

    // Currently the API allows duplicate shelf names
    let resp1 = server
        .admin_client()
        .post_json(
            "/api/v1/library/shelves",
            &serde_json::json!({ "name": "Same Name" }),
        )
        .await;
    resp1.assert_ok();
    let id1: String = resp1.json();

    let resp2 = server
        .admin_client()
        .post_json(
            "/api/v1/library/shelves",
            &serde_json::json!({ "name": "Same Name" }),
        )
        .await;
    resp2.assert_ok();
    let id2: String = resp2.json();

    // Should create two different shelves
    assert_ne!(id1, id2);
}

#[tokio::test]
async fn test_rename_shelf() {
    let server = TestServer::start_api_only().await;

    let resp = server
        .admin_client()
        .post_json(
            "/api/v1/library/shelves",
            &serde_json::json!({ "name": "Old Name" }),
        )
        .await;
    let shelf_id: String = resp.json();

    server
        .admin_client()
        .patch_json(
            &format!("/api/v1/library/shelves/{}", shelf_id),
            &serde_json::json!({ "name": "New Name" }),
        )
        .await
        .assert_status(StatusCode::NO_CONTENT);

    let resp = server.admin_client().get("/api/v1/library/shelves").await;
    let json: Value = resp.json();
    assert_eq!(json["items"][0]["name"], "New Name");
}

#[tokio::test]
async fn test_delete_shelf() {
    let server = TestServer::start_api_only().await;

    let resp = server
        .admin_client()
        .post_json(
            "/api/v1/library/shelves",
            &serde_json::json!({ "name": "To Delete" }),
        )
        .await;
    let shelf_id: String = resp.json();

    server
        .admin_client()
        .delete(&format!("/api/v1/library/shelves/{}", shelf_id))
        .await
        .assert_status(StatusCode::NO_CONTENT);

    let resp = server.admin_client().get("/api/v1/library/shelves").await;
    let json: Value = resp.json();
    assert!(json["items"].as_array().unwrap().is_empty());
}

#[tokio::test]
async fn test_delete_shelf_not_found() {
    let server = TestServer::start_api_only().await;

    server
        .admin_client()
        .delete(&format!("/api/v1/library/shelves/{}", uuid::Uuid::new_v4()))
        .await
        .assert_status(StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn test_add_and_list_shelf_items() {
    let server = TestServer::start_api_only().await;

    let book_id1 = create_book(&server, "Book 1").await;
    let book_id2 = create_book(&server, "Book 2").await;

    let resp = server
        .admin_client()
        .post_json(
            "/api/v1/library/shelves",
            &serde_json::json!({ "name": "My Shelf" }),
        )
        .await;
    let shelf_id: String = resp.json();

    server
        .admin_client()
        .post_json(
            &format!("/api/v1/library/shelves/{}/items", shelf_id),
            &serde_json::json!({ "book_ids": [book_id1, book_id2] }),
        )
        .await
        .assert_status(StatusCode::NO_CONTENT);

    let resp = server
        .admin_client()
        .get(&format!("/api/v1/library/shelves/{}/items", shelf_id))
        .await;
    resp.assert_ok();

    let books: Vec<Value> = resp.json();
    assert_eq!(books.len(), 2);
}

#[tokio::test]
async fn test_remove_shelf_items() {
    let server = TestServer::start_api_only().await;

    let book_id = create_book(&server, "Book 1").await;

    let resp = server
        .admin_client()
        .post_json(
            "/api/v1/library/shelves",
            &serde_json::json!({ "name": "My Shelf" }),
        )
        .await;
    let shelf_id: String = resp.json();

    // Add item
    server
        .admin_client()
        .post_json(
            &format!("/api/v1/library/shelves/{}/items", shelf_id),
            &serde_json::json!({ "book_ids": [book_id] }),
        )
        .await
        .assert_status(StatusCode::NO_CONTENT);

    // Verify item is there
    let books: Vec<Value> = server
        .admin_client()
        .get(&format!("/api/v1/library/shelves/{}/items", shelf_id))
        .await
        .json();
    assert_eq!(books.len(), 1);

    // Remove item
    server
        .admin_client()
        .delete_json(
            &format!("/api/v1/library/shelves/{}/items", shelf_id),
            &serde_json::json!({ "book_ids": [book_id] }),
        )
        .await
        .assert_status(StatusCode::NO_CONTENT);

    // Verify empty
    let books: Vec<Value> = server
        .admin_client()
        .get(&format!("/api/v1/library/shelves/{}/items", shelf_id))
        .await
        .json();
    assert!(books.is_empty());
}

// =============================================================================
// Authors Tests
// =============================================================================

#[tokio::test]
async fn test_list_authors_empty() {
    let server = TestServer::start_api_only().await;

    let resp = server.admin_client().get("/api/v1/library/authors").await;
    resp.assert_ok();

    let json: Value = resp.json();
    assert!(json["items"].as_array().unwrap().is_empty());
}

#[tokio::test]
async fn test_list_authors_with_items() {
    let server = TestServer::start_api_only().await;

    let id = create_book(&server, "Test Book").await;
    create_stub_book_file(&server.pool, id).await;

    let resp = server.admin_client().get("/api/v1/library/authors").await;
    resp.assert_ok();

    let json: Value = resp.json();
    let items = json["items"].as_array().unwrap();
    assert_eq!(items.len(), 1);
    assert_eq!(items[0]["name"], "Test Author");
    assert_eq!(items[0]["book_count"], 1);
}

#[tokio::test]
async fn test_create_author() {
    let server = TestServer::start_api_only().await;

    let resp = server
        .admin_client()
        .post_json(
            "/api/v1/library/authors",
            &serde_json::json!({ "name": "New Author" }),
        )
        .await;
    resp.assert_ok();

    let body: Value = resp.json();
    let author_id = body["id"].as_str().unwrap();
    assert!(!author_id.is_empty());

    // include_no_books=true to see authors without library books
    let resp = server
        .admin_client()
        .get("/api/v1/library/authors?include_no_books=true")
        .await;
    let json: Value = resp.json();
    let items = json["items"].as_array().unwrap();
    assert_eq!(items.len(), 1);
    assert_eq!(items[0]["name"], "New Author");
}

#[tokio::test]
async fn test_create_author_idempotent() {
    let server = TestServer::start_api_only().await;

    let resp1 = server
        .admin_client()
        .post_json(
            "/api/v1/library/authors",
            &serde_json::json!({ "name": "Same Author" }),
        )
        .await;
    let body1: Value = resp1.json();
    let id1 = body1["id"].as_str().unwrap();

    let resp2 = server
        .admin_client()
        .post_json(
            "/api/v1/library/authors",
            &serde_json::json!({ "name": "Same Author" }),
        )
        .await;
    let body2: Value = resp2.json();
    let id2 = body2["id"].as_str().unwrap();

    assert_eq!(id1, id2);
}

#[tokio::test]
async fn test_rename_author() {
    let server = TestServer::start_api_only().await;

    let resp = server
        .admin_client()
        .post_json(
            "/api/v1/library/authors",
            &serde_json::json!({ "name": "Old Name" }),
        )
        .await;
    let body: Value = resp.json();
    let author_id = body["id"].as_str().unwrap();

    server
        .admin_client()
        .patch_json(
            &format!("/api/v1/library/authors/{}", author_id),
            &serde_json::json!({ "name": "New Name" }),
        )
        .await
        .assert_status(StatusCode::NO_CONTENT);

    // include_no_books=true to see authors without library books
    let resp = server
        .admin_client()
        .get("/api/v1/library/authors?include_no_books=true")
        .await;
    let json: Value = resp.json();
    assert_eq!(json["items"][0]["name"], "New Name");
}

#[tokio::test]
async fn test_merge_authors() {
    let server = TestServer::start_api_only().await;

    // Create books with different authors
    let book1_id = uuid::Uuid::new_v4();
    server
        .ingest_client()
        .put_json(
            &format!("/api/v1/books/{}", book1_id),
            &serde_json::json!({
                "title": "Book by Source",
                "authors": ["Source Author"]
            }),
        )
        .await
        .assert_success();
    create_stub_book_file(&server.pool, book1_id).await;

    let book2_id = uuid::Uuid::new_v4();
    server
        .ingest_client()
        .put_json(
            &format!("/api/v1/books/{}", book2_id),
            &serde_json::json!({
                "title": "Book by Target",
                "authors": ["Target Author"]
            }),
        )
        .await
        .assert_success();
    create_stub_book_file(&server.pool, book2_id).await;

    let resp = server.admin_client().get("/api/v1/library/authors").await;
    let json: Value = resp.json();
    let items = json["items"].as_array().unwrap();
    assert_eq!(items.len(), 2);

    let source_id = items.iter().find(|a| a["name"] == "Source Author").unwrap()["id"]
        .as_str()
        .unwrap();
    let target_id = items.iter().find(|a| a["name"] == "Target Author").unwrap()["id"]
        .as_str()
        .unwrap();

    server
        .admin_client()
        .post_json(
            "/api/v1/library/authors/merge",
            &serde_json::json!({
                "source_id": source_id,
                "target_id": target_id
            }),
        )
        .await
        .assert_status(StatusCode::NO_CONTENT);

    // Verify merge
    let resp = server.admin_client().get("/api/v1/library/authors").await;
    let json: Value = resp.json();
    let items = json["items"].as_array().unwrap();
    assert_eq!(items.len(), 1);
    assert_eq!(items[0]["name"], "Target Author");
    assert_eq!(items[0]["book_count"], 2);
}

#[tokio::test]
async fn test_list_author_books() {
    let server = TestServer::start_api_only().await;

    let id = create_book(&server, "Book 1").await;
    create_stub_book_file(&server.pool, id).await;
    let id = create_book(&server, "Book 2").await;
    create_stub_book_file(&server.pool, id).await;

    let resp = server.admin_client().get("/api/v1/library/authors").await;
    let json: Value = resp.json();
    let author_id = json["items"][0]["id"].as_str().unwrap();

    let resp = server
        .admin_client()
        .get(&format!("/api/v1/library/authors/{}/books", author_id))
        .await;
    resp.assert_ok();

    let books: Vec<Value> = resp.json();
    assert_eq!(books.len(), 2);
}

// =============================================================================
// Activity Tests
// =============================================================================

#[tokio::test]
async fn test_list_activity() {
    let server = TestServer::start_api_only().await;

    // Create a book to generate an activity event
    create_book(&server, "Activity Test Book").await;

    // Activity events are created by various operations
    let resp = server.admin_client().get("/api/v1/library/activity").await;
    resp.assert_ok();

    // Activity returns a paginated response
    let json: Value = resp.json();
    assert!(json["items"].is_array());
}

// =============================================================================
// Cover Tests
// =============================================================================

#[tokio::test]
async fn test_get_cover_not_found() {
    let server = TestServer::start_api_only().await;

    let resp = server
        .admin_client()
        .get(&format!(
            "/api/v1/library/books/{}/cover",
            uuid::Uuid::new_v4()
        ))
        .await;
    resp.assert_status(StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn test_get_cover_no_image() {
    let server = TestServer::start_api_only().await;
    let book_id = create_book(&server, "No Cover Book").await;

    let resp = server
        .admin_client()
        .get(&format!("/api/v1/library/books/{}/cover", book_id))
        .await;
    resp.assert_status(StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn test_get_cover_with_image() {
    let server = TestServer::start_api_only().await;
    let book_id = create_book(&server, "Book With Cover").await;
    upload_cover(&server, book_id, test_jpeg_image()).await;

    let resp = server
        .admin_client()
        .get(&format!("/api/v1/library/books/{}/cover", book_id))
        .await;
    resp.assert_ok();
}

// =============================================================================
// Auth Tests for All Library Endpoints
// =============================================================================

#[tokio::test]
async fn test_library_endpoints_require_auth() {
    let server = TestServer::start_api_only().await;
    let client = TestClient::new(&server.api_url);
    let fake_id = uuid::Uuid::new_v4();

    // Books
    client
        .get("/api/v1/library/books")
        .await
        .assert_status(StatusCode::UNAUTHORIZED);
    client
        .patch_json(
            &format!("/api/v1/library/books/{}", fake_id),
            &serde_json::json!({}),
        )
        .await
        .assert_status(StatusCode::UNAUTHORIZED);
    client
        .post_json(
            "/api/v1/library/books/touch",
            &serde_json::json!({ "book_ids": [] }),
        )
        .await
        .assert_status(StatusCode::UNAUTHORIZED);

    // Series
    client
        .get("/api/v1/library/series")
        .await
        .assert_status(StatusCode::UNAUTHORIZED);
    client
        .post_json(
            "/api/v1/library/series",
            &serde_json::json!({ "name": "x" }),
        )
        .await
        .assert_status(StatusCode::UNAUTHORIZED);

    // Shelves
    client
        .get("/api/v1/library/shelves")
        .await
        .assert_status(StatusCode::UNAUTHORIZED);
    client
        .post_json(
            "/api/v1/library/shelves",
            &serde_json::json!({ "name": "x" }),
        )
        .await
        .assert_status(StatusCode::UNAUTHORIZED);

    // Authors
    client
        .get("/api/v1/library/authors")
        .await
        .assert_status(StatusCode::UNAUTHORIZED);
    client
        .post_json(
            "/api/v1/library/authors",
            &serde_json::json!({ "name": "x" }),
        )
        .await
        .assert_status(StatusCode::UNAUTHORIZED);

    // Activity
    client
        .get("/api/v1/library/activity")
        .await
        .assert_status(StatusCode::UNAUTHORIZED);

    // Sync
    client
        .get("/api/v1/library/devices")
        .await
        .assert_status(StatusCode::UNAUTHORIZED);
}

// =============================================================================
// Sync Devices Tests
// =============================================================================

#[tokio::test]
async fn test_list_library_devices() {
    let server = TestServer::start_api_only().await;

    let resp = server.admin_client().get("/api/v1/library/devices").await;
    resp.assert_ok();

    let devices: Vec<Value> = resp.json();
    // Should be empty initially (no devices created)
    assert!(devices.is_empty());
}

#[tokio::test]
async fn test_list_library_devices_with_items() {
    let server = TestServer::start_api_only().await;

    let user_id = create_user(&server, "device@test.com").await;
    create_device(&server, user_id).await;

    let resp = server.admin_client().get("/api/v1/library/devices").await;
    resp.assert_ok();

    let devices: Vec<Value> = resp.json();
    assert_eq!(devices.len(), 1);
}

#[tokio::test]
async fn test_sync_books_to_devices() {
    let server = TestServer::start_api_only().await;

    let book_id = create_book(&server, "Sync Test Book").await;

    // Sync books creates/updates entitlements
    server
        .admin_client()
        .post_json(
            "/api/v1/library/sync",
            &serde_json::json!({
                "book_ids": [book_id]
            }),
        )
        .await
        .assert_status(StatusCode::NO_CONTENT);
}

// =============================================================================
// Freeform Search Tests (UUID and series name)
// =============================================================================

#[tokio::test]
async fn test_search_by_series_uuid() {
    let server = TestServer::start_api_only().await;

    // Create books in a series
    let id = create_book_in_series(&server, "Cradle Book 1", "Cradle Series", 1).await;
    create_stub_book_file(&server.pool, id).await;
    let id = create_book_in_series(&server, "Cradle Book 2", "Cradle Series", 2).await;
    create_stub_book_file(&server.pool, id).await;
    let id = create_book(&server, "Unrelated Book").await;
    create_stub_book_file(&server.pool, id).await;

    // Get the series UUID
    let resp = server.admin_client().get("/api/v1/library/series").await;
    let json: Value = resp.json();
    let series_id = json["items"][0]["id"].as_str().unwrap();

    // Search by series UUID should return books in that series
    let resp = server
        .admin_client()
        .get(&format!("/api/v1/library/books?search={}", series_id))
        .await;
    resp.assert_ok();

    let json: Value = resp.json();
    let items = json["items"].as_array().unwrap();
    assert_eq!(items.len(), 2, "Should find 2 books in the series");
    assert!(
        items.iter().all(|b| b["series_name"] == "Cradle Series"),
        "All results should be in Cradle Series"
    );
}

#[tokio::test]
async fn test_search_by_author_uuid() {
    let server = TestServer::start_api_only().await;

    // Create books with different authors using the ingest API directly
    let book1_id = uuid::Uuid::new_v4();
    server
        .ingest_client()
        .put_json(
            &format!("/api/v1/books/{}", book1_id),
            &serde_json::json!({
                "title": "Cradle",
                "authors": ["Will Wight"]
            }),
        )
        .await
        .assert_success();
    create_stub_book_file(&server.pool, book1_id).await;

    let book2_id = uuid::Uuid::new_v4();
    server
        .ingest_client()
        .put_json(
            &format!("/api/v1/books/{}", book2_id),
            &serde_json::json!({
                "title": "Wintersteel",
                "authors": ["Will Wight"]
            }),
        )
        .await
        .assert_success();
    create_stub_book_file(&server.pool, book2_id).await;

    let book3_id = uuid::Uuid::new_v4();
    server
        .ingest_client()
        .put_json(
            &format!("/api/v1/books/{}", book3_id),
            &serde_json::json!({
                "title": "Mistborn",
                "authors": ["Brandon Sanderson"]
            }),
        )
        .await
        .assert_success();
    create_stub_book_file(&server.pool, book3_id).await;

    // Get the author UUID for Will Wight
    let resp = server.admin_client().get("/api/v1/library/authors").await;
    let json: Value = resp.json();
    let items = json["items"].as_array().unwrap();
    let will_wight_id = items.iter().find(|a| a["name"] == "Will Wight").unwrap()["id"]
        .as_str()
        .unwrap();

    // Search by author UUID should return books by that author
    let resp = server
        .admin_client()
        .get(&format!("/api/v1/library/books?search={}", will_wight_id))
        .await;
    resp.assert_ok();

    let json: Value = resp.json();
    let items = json["items"].as_array().unwrap();
    assert_eq!(items.len(), 2, "Should find 2 books by Will Wight");
}

#[tokio::test]
async fn test_search_by_series_name() {
    let server = TestServer::start_api_only().await;

    // Create books in a series
    let id = create_book_in_series(&server, "Unsouled", "Cradle", 1).await;
    create_stub_book_file(&server.pool, id).await;
    let id = create_book_in_series(&server, "Soulsmith", "Cradle", 2).await;
    create_stub_book_file(&server.pool, id).await;
    let id = create_book(&server, "Unrelated Book").await;
    create_stub_book_file(&server.pool, id).await;

    // Search by series name should return books in that series
    let resp = server
        .admin_client()
        .get("/api/v1/library/books?search=Cradle")
        .await;
    resp.assert_ok();

    let json: Value = resp.json();
    let items = json["items"].as_array().unwrap();
    assert_eq!(items.len(), 2, "Should find 2 books in Cradle series");
    assert!(
        items.iter().all(|b| b["series_name"] == "Cradle"),
        "All results should be in Cradle series"
    );
}

#[tokio::test]
async fn test_search_by_book_uuid() {
    let server = TestServer::start_api_only().await;

    let book_id = create_book(&server, "Specific Book").await;
    create_stub_book_file(&server.pool, book_id).await;
    let id = create_book(&server, "Other Book").await;
    create_stub_book_file(&server.pool, id).await;

    // Search by book UUID should return just that book
    let resp = server
        .admin_client()
        .get(&format!("/api/v1/library/books?search={}", book_id))
        .await;
    resp.assert_ok();

    let json: Value = resp.json();
    let items = json["items"].as_array().unwrap();
    assert_eq!(items.len(), 1, "Should find exactly 1 book");
    assert_eq!(items[0]["id"], book_id.to_string());
}

// =============================================================================
// Author Metadata Tests
// =============================================================================

#[tokio::test]
async fn test_apply_author_metadata() {
    let server = TestServer::start_api_only().await;

    // Create an author via a book
    let id = create_book(&server, "Test Book").await;
    create_stub_book_file(&server.pool, id).await;

    // Get the author ID
    let resp = server.admin_client().get("/api/v1/library/authors").await;
    let json: Value = resp.json();
    let author_id = json["items"][0]["id"].as_str().unwrap();

    // Apply metadata
    let resp = server
        .admin_client()
        .post_json(
            &format!("/api/v1/library/authors/{}/apply-metadata", author_id),
            &serde_json::json!({
                "provider": "hardcover",
                "provider_id": "12345",
                "image_url": "https://example.com/author.jpg",
                "bio": "A famous test author"
            }),
        )
        .await;
    resp.assert_status(StatusCode::NO_CONTENT);

    // Verify metadata was applied
    let resp = server.admin_client().get("/api/v1/library/authors").await;
    let json: Value = resp.json();
    let author = &json["items"][0];
    assert_eq!(author["identifiers"]["hardcover"], "12345");
    let image_url = author["image_url"].as_str().unwrap();
    assert!(
        image_url.starts_with("/api/v1/library/authors/") && image_url.ends_with("/image"),
        "Expected local endpoint URL, got: {image_url}"
    );
    assert_eq!(author["bio"], "A famous test author");
}

#[tokio::test]
async fn test_apply_author_metadata_not_found() {
    let server = TestServer::start_api_only().await;

    let resp = server
        .admin_client()
        .post_json(
            &format!(
                "/api/v1/library/authors/{}/apply-metadata",
                uuid::Uuid::new_v4()
            ),
            &serde_json::json!({
                "provider": "hardcover",
                "provider_id": "12345"
            }),
        )
        .await;
    resp.assert_status(StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn test_apply_author_metadata_requires_auth() {
    let server = TestServer::start_api_only().await;
    let client = TestClient::new(&server.api_url);

    client
        .post_json(
            &format!(
                "/api/v1/library/authors/{}/apply-metadata",
                uuid::Uuid::new_v4()
            ),
            &serde_json::json!({
                "provider": "hardcover",
                "provider_id": "12345"
            }),
        )
        .await
        .assert_status(StatusCode::UNAUTHORIZED);
}

// =============================================================================
// Series Metadata Tests
// =============================================================================

#[tokio::test]
async fn test_apply_series_metadata() {
    let server = TestServer::start_api_only().await;

    // Create a series via a book
    let id = create_book_in_series(&server, "Book 1", "Test Series", 1).await;
    create_stub_book_file(&server.pool, id).await;

    // Get the series ID
    let resp = server.admin_client().get("/api/v1/library/series").await;
    let json: Value = resp.json();
    let series_id = json["items"][0]["id"].as_str().unwrap();

    // Apply metadata
    let resp = server
        .admin_client()
        .post_json(
            &format!("/api/v1/library/series/{}/apply-metadata", series_id),
            &serde_json::json!({
                "provider": "hardcover",
                "provider_id": "67890",
                "image_url": "https://example.com/series.jpg",
                "description": "An epic series",
                "total_book_count": 10
            }),
        )
        .await;
    resp.assert_status(StatusCode::NO_CONTENT);

    // Verify metadata was applied
    let resp = server.admin_client().get("/api/v1/library/series").await;
    let json: Value = resp.json();
    let series = &json["items"][0];
    assert_eq!(series["identifiers"]["hardcover"], "67890");
    let image_url = series["image_url"].as_str().unwrap();
    assert!(
        image_url.starts_with("/api/v1/library/series/") && image_url.ends_with("/image"),
        "Expected local endpoint URL, got: {image_url}"
    );
    assert_eq!(series["description"], "An epic series");
    assert_eq!(series["total_book_count"], 10);
}

#[tokio::test]
async fn test_apply_series_metadata_not_found() {
    let server = TestServer::start_api_only().await;

    let resp = server
        .admin_client()
        .post_json(
            &format!(
                "/api/v1/library/series/{}/apply-metadata",
                uuid::Uuid::new_v4()
            ),
            &serde_json::json!({
                "provider": "hardcover",
                "provider_id": "67890"
            }),
        )
        .await;
    resp.assert_status(StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn test_apply_series_metadata_requires_auth() {
    let server = TestServer::start_api_only().await;
    let client = TestClient::new(&server.api_url);

    client
        .post_json(
            &format!(
                "/api/v1/library/series/{}/apply-metadata",
                uuid::Uuid::new_v4()
            ),
            &serde_json::json!({
                "provider": "hardcover",
                "provider_id": "67890"
            }),
        )
        .await
        .assert_status(StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn test_apply_series_metadata_enqueues_enrichment_job() {
    let server = TestServer::start_api_only().await;

    // Create a series via a book
    let id = create_book_in_series(&server, "Book 1", "Test Series", 1).await;
    create_stub_book_file(&server.pool, id).await;

    // Get the series ID
    let resp = server.admin_client().get("/api/v1/library/series").await;
    let json: Value = resp.json();
    let series_id = json["items"][0]["id"].as_str().unwrap();

    // Apply metadata (which should enqueue a series_enrich job)
    let resp = server
        .admin_client()
        .post_json(
            &format!("/api/v1/library/series/{}/apply-metadata", series_id),
            &serde_json::json!({
                "provider": "hardcover",
                "provider_id": "hc-12345",
                "description": "Test series description"
            }),
        )
        .await;
    resp.assert_status(StatusCode::NO_CONTENT);

    // Verify a series_enrich job was enqueued
    let resp = server
        .admin_client()
        .get("/api/v1/admin/jobs?job_type=series_enrich&exclude_routine=false")
        .await;
    resp.assert_ok();

    let json: Value = resp.json();
    let jobs = json["jobs"].as_array().unwrap();
    assert!(!jobs.is_empty(), "series_enrich job should be enqueued");

    // Verify the job payload contains the correct series_id
    let job = &jobs[0];
    assert_eq!(job["job_type"].as_str().unwrap(), "series_enrich");
    assert_eq!(job["payload"]["series_id"].as_str().unwrap(), series_id);
}

#[tokio::test]
async fn test_refresh_series_metadata() {
    let server = TestServer::start_api_only().await;

    // Create a series via a book
    let id = create_book_in_series(&server, "Book 1", "Test Series", 1).await;
    create_stub_book_file(&server.pool, id).await;

    // Get the series ID
    let resp = server.admin_client().get("/api/v1/library/series").await;
    let json: Value = resp.json();
    let series_id = json["items"][0]["id"].as_str().unwrap();

    // First link the series to hardcover (required for refresh)
    server
        .admin_client()
        .post_json(
            &format!("/api/v1/library/series/{}/apply-metadata", series_id),
            &serde_json::json!({
                "provider": "hardcover",
                "provider_id": "hc-refresh-test"
            }),
        )
        .await
        .assert_status(StatusCode::NO_CONTENT);

    // Now refresh the series
    let resp = server
        .admin_client()
        .post(&format!("/api/v1/library/series/{}/refresh", series_id))
        .await;
    resp.assert_ok();

    let json: Value = resp.json();
    assert!(
        json["message"]
            .as_str()
            .unwrap()
            .contains("Metadata refresh queued")
    );
}

#[tokio::test]
async fn test_refresh_series_metadata_not_found() {
    let server = TestServer::start_api_only().await;

    let resp = server
        .admin_client()
        .post(&format!(
            "/api/v1/library/series/{}/refresh",
            uuid::Uuid::new_v4()
        ))
        .await;
    resp.assert_status(StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn test_refresh_series_metadata_requires_auth() {
    let server = TestServer::start_api_only().await;
    let client = TestClient::new(&server.api_url);

    client
        .post(&format!(
            "/api/v1/library/series/{}/refresh",
            uuid::Uuid::new_v4()
        ))
        .await
        .assert_status(StatusCode::UNAUTHORIZED);
}

// =============================================================================
// Include Not-Owned Books Tests
// =============================================================================

#[tokio::test]
async fn test_author_books_include_not_owned() {
    let server = TestServer::start_api_only().await;

    // Create a book to get an author
    let id = create_book(&server, "Library Book").await;
    create_stub_book_file(&server.pool, id).await;

    // Get the author ID
    let resp = server.admin_client().get("/api/v1/library/authors").await;
    let json: Value = resp.json();
    let author_id = json["items"][0]["id"].as_str().unwrap();

    // Request with include_not_owned=true
    let resp = server
        .admin_client()
        .get(&format!(
            "/api/v1/library/authors/{}/books?include_not_owned=true",
            author_id
        ))
        .await;
    resp.assert_ok();

    // Should return books with is_in_library field
    let books: Vec<Value> = resp.json();
    assert!(!books.is_empty());
}

#[tokio::test]
async fn test_series_books_include_not_owned() {
    let server = TestServer::start_api_only().await;

    // Create a book in a series
    let id = create_book_in_series(&server, "Book 1", "Test Series", 1).await;
    create_stub_book_file(&server.pool, id).await;

    // Get the series ID
    let resp = server.admin_client().get("/api/v1/library/series").await;
    let json: Value = resp.json();
    let series_id = json["items"][0]["id"].as_str().unwrap();

    // Request with include_not_owned=true
    let resp = server
        .admin_client()
        .get(&format!(
            "/api/v1/library/series/{}/books?include_not_owned=true",
            series_id
        ))
        .await;
    resp.assert_ok();

    // Should return books with is_in_library field
    let books: Vec<Value> = resp.json();
    assert!(!books.is_empty());
}

// =============================================================================
// Want/Skip Book Tests
// =============================================================================

/// Helper to create a book that is NOT in the library (discovered, not owned).
/// Uses want_status = 'pending' which is the default for newly discovered books.
async fn create_discovered_book(server: &TestServer, title: &str) -> uuid::Uuid {
    let book_id = uuid::Uuid::new_v4();
    sqlx::query(
        r#"
        INSERT INTO books (id, title, want_status, created_at, updated_at)
        VALUES ($1, $2, 'pending', NOW(), NOW())
        "#,
    )
    .bind(book_id)
    .bind(title)
    .execute(&server.pool)
    .await
    .expect("Failed to create discovered book");
    book_id
}

/// Helper to create a library book directly in the database (no author linking).
/// Used when we need a library book without the default "Test Author" that create_book uses.
/// Creates a stub book file so the book appears in-library.
async fn create_library_book_db(server: &TestServer, title: &str) -> uuid::Uuid {
    let book_id = uuid::Uuid::new_v4();
    sqlx::query(
        r#"
        INSERT INTO books (id, title, want_status, created_at, updated_at)
        VALUES ($1, $2, 'none', NOW(), NOW())
        "#,
    )
    .bind(book_id)
    .bind(title)
    .execute(&server.pool)
    .await
    .expect("Failed to create library book");
    create_stub_book_file(&server.pool, book_id).await;
    book_id
}

#[tokio::test]
async fn test_want_book() {
    let server = TestServer::start_api_only().await;
    let book_id = create_discovered_book(&server, "Discovered Book").await;

    let resp = server
        .admin_client()
        .post_json(
            &format!("/api/v1/library/books/{}/want", book_id),
            &serde_json::json!({}),
        )
        .await;
    resp.assert_status(StatusCode::NO_CONTENT);

    // Verify the status was updated
    let row = sqlx::query_scalar::<_, String>("SELECT want_status FROM books WHERE id = $1")
        .bind(book_id)
        .fetch_one(&server.pool)
        .await
        .unwrap();
    assert_eq!(row, "wanted");
}

#[tokio::test]
async fn test_want_book_not_found() {
    let server = TestServer::start_api_only().await;

    let resp = server
        .admin_client()
        .post_json(
            &format!("/api/v1/library/books/{}/want", uuid::Uuid::new_v4()),
            &serde_json::json!({}),
        )
        .await;
    resp.assert_status(StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn test_want_book_already_in_library() {
    let server = TestServer::start_api_only().await;
    // create_book + stub file makes the book appear in library
    let book_id = create_book(&server, "Library Book").await;
    create_stub_book_file(&server.pool, book_id).await;

    // Rewanting an in-library book is allowed (for upgrades)
    let resp = server
        .admin_client()
        .post_json(
            &format!("/api/v1/library/books/{}/want", book_id),
            &serde_json::json!({}),
        )
        .await;
    resp.assert_status(StatusCode::NO_CONTENT);
}

#[tokio::test]
async fn test_want_book_requires_auth() {
    let server = TestServer::start_api_only().await;
    let client = TestClient::new(&server.api_url);

    client
        .post_json(
            &format!("/api/v1/library/books/{}/want", uuid::Uuid::new_v4()),
            &serde_json::json!({}),
        )
        .await
        .assert_status(StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn test_skip_book() {
    let server = TestServer::start_api_only().await;
    let book_id = create_discovered_book(&server, "Discovered Book").await;

    let resp = server
        .admin_client()
        .post_json(
            &format!("/api/v1/library/books/{}/skip", book_id),
            &serde_json::json!({}),
        )
        .await;
    resp.assert_status(StatusCode::NO_CONTENT);

    // Verify the status was updated
    let row = sqlx::query_scalar::<_, String>("SELECT want_status FROM books WHERE id = $1")
        .bind(book_id)
        .fetch_one(&server.pool)
        .await
        .unwrap();
    assert_eq!(row, "skipped");
}

#[tokio::test]
async fn test_skip_book_not_found() {
    let server = TestServer::start_api_only().await;

    let resp = server
        .admin_client()
        .post_json(
            &format!("/api/v1/library/books/{}/skip", uuid::Uuid::new_v4()),
            &serde_json::json!({}),
        )
        .await;
    resp.assert_status(StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn test_skip_book_already_in_library() {
    let server = TestServer::start_api_only().await;
    // create_book + stub file makes the book appear in library
    let book_id = create_book(&server, "Library Book").await;
    create_stub_book_file(&server.pool, book_id).await;

    // Should fail because book is in library
    let resp = server
        .admin_client()
        .post_json(
            &format!("/api/v1/library/books/{}/skip", book_id),
            &serde_json::json!({}),
        )
        .await;
    resp.assert_status(StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn test_skip_book_requires_auth() {
    let server = TestServer::start_api_only().await;
    let client = TestClient::new(&server.api_url);

    client
        .post_json(
            &format!("/api/v1/library/books/{}/skip", uuid::Uuid::new_v4()),
            &serde_json::json!({}),
        )
        .await
        .assert_status(StatusCode::UNAUTHORIZED);
}

// =============================================================================
// Bulk Want Tests (want all pending for author/series)
// =============================================================================

/// Helper to create an author directly in the database
async fn create_author_db(server: &TestServer, name: &str) -> uuid::Uuid {
    let author_id = uuid::Uuid::new_v4();
    let normalized_name = name.to_lowercase();
    sqlx::query(
        r#"
        INSERT INTO authors (id, name, normalized_name, created_at, updated_at)
        VALUES ($1, $2, $3, NOW(), NOW())
        "#,
    )
    .bind(author_id)
    .bind(name)
    .bind(&normalized_name)
    .execute(&server.pool)
    .await
    .expect("Failed to create author");
    author_id
}

/// Helper to create a series directly in the database
async fn create_series_db(server: &TestServer, name: &str) -> uuid::Uuid {
    let series_id = uuid::Uuid::new_v4();
    let sort_key = name.to_lowercase();
    sqlx::query(
        r#"
        INSERT INTO series (id, name, sort_key, created_at, updated_at)
        VALUES ($1, $2, $3, NOW(), NOW())
        "#,
    )
    .bind(series_id)
    .bind(name)
    .bind(&sort_key)
    .execute(&server.pool)
    .await
    .expect("Failed to create series");
    series_id
}

/// Helper to link a book to an author
async fn link_book_to_author(server: &TestServer, book_id: uuid::Uuid, author_id: uuid::Uuid) {
    sqlx::query(
        r#"
        INSERT INTO book_authors (book_id, author_id)
        VALUES ($1, $2)
        "#,
    )
    .bind(book_id)
    .bind(author_id)
    .execute(&server.pool)
    .await
    .expect("Failed to link book to author");
}

/// Helper to set a book's series via junction table
async fn set_book_series(server: &TestServer, book_id: uuid::Uuid, series_id: uuid::Uuid) {
    sqlx::query(
        r#"
        INSERT INTO book_series (book_id, series_id, is_primary)
        VALUES ($1, $2, TRUE)
        ON CONFLICT (book_id, series_id) DO UPDATE SET is_primary = TRUE
        "#,
    )
    .bind(book_id)
    .bind(series_id)
    .execute(&server.pool)
    .await
    .expect("Failed to set book series");
}

#[tokio::test]
async fn test_want_all_pending_for_author() {
    let server = TestServer::start_api_only().await;

    // Create an author
    let author_id = create_author_db(&server, "Test Author").await;

    // Create 3 discovered books (not in library)
    let book1 = create_discovered_book(&server, "Book 1").await;
    let book2 = create_discovered_book(&server, "Book 2").await;
    let book3 = create_discovered_book(&server, "Book 3").await;

    // Link all books to the author
    link_book_to_author(&server, book1, author_id).await;
    link_book_to_author(&server, book2, author_id).await;
    link_book_to_author(&server, book3, author_id).await;

    // Call bulk want endpoint
    let resp = server
        .admin_client()
        .post_json(
            &format!("/api/v1/library/authors/{}/want-pending", author_id),
            &serde_json::json!({}),
        )
        .await;
    resp.assert_ok();

    let body: Value = resp.json();
    assert_eq!(body["wanted_count"], 3);

    // Verify all books are now wanted
    let statuses: Vec<String> =
        sqlx::query_scalar("SELECT want_status FROM books WHERE id = ANY($1) ORDER BY title")
            .bind([book1, book2, book3])
            .fetch_all(&server.pool)
            .await
            .unwrap();

    assert!(statuses.iter().all(|s| s == "wanted"));
}

#[tokio::test]
async fn test_want_all_pending_for_author_skips_already_wanted() {
    let server = TestServer::start_api_only().await;

    let author_id = create_author_db(&server, "Test Author").await;

    // Create books with different statuses
    let pending_book = create_discovered_book(&server, "Pending Book").await;
    let wanted_book = create_discovered_book(&server, "Wanted Book").await;
    let skipped_book = create_discovered_book(&server, "Skipped Book").await;

    // Set wanted_book to already wanted
    sqlx::query("UPDATE books SET want_status = 'wanted' WHERE id = $1")
        .bind(wanted_book)
        .execute(&server.pool)
        .await
        .unwrap();

    // Set skipped_book to skipped
    sqlx::query("UPDATE books SET want_status = 'skipped' WHERE id = $1")
        .bind(skipped_book)
        .execute(&server.pool)
        .await
        .unwrap();

    link_book_to_author(&server, pending_book, author_id).await;
    link_book_to_author(&server, wanted_book, author_id).await;
    link_book_to_author(&server, skipped_book, author_id).await;

    let resp = server
        .admin_client()
        .post_json(
            &format!("/api/v1/library/authors/{}/want-pending", author_id),
            &serde_json::json!({}),
        )
        .await;
    resp.assert_ok();

    // Should only update the pending book (none status)
    let body: Value = resp.json();
    assert_eq!(body["wanted_count"], 1);

    // Verify pending book is now wanted
    let status: String = sqlx::query_scalar("SELECT want_status FROM books WHERE id = $1")
        .bind(pending_book)
        .fetch_one(&server.pool)
        .await
        .unwrap();
    assert_eq!(status, "wanted");

    // Verify skipped book remains skipped
    let status: String = sqlx::query_scalar("SELECT want_status FROM books WHERE id = $1")
        .bind(skipped_book)
        .fetch_one(&server.pool)
        .await
        .unwrap();
    assert_eq!(status, "skipped");
}

#[tokio::test]
async fn test_want_all_pending_for_author_skips_library_books() {
    let server = TestServer::start_api_only().await;

    // Use a unique name to avoid conflict with create_book's default "Test Author"
    let author_id = create_author_db(&server, "Bulk Want Test Author").await;

    // Create one discovered book and one library book
    let discovered_book = create_discovered_book(&server, "Discovered").await;
    // Create library book directly (not via create_book which links to "Test Author")
    let library_book = create_library_book_db(&server, "In Library").await;

    link_book_to_author(&server, discovered_book, author_id).await;
    link_book_to_author(&server, library_book, author_id).await;

    let resp = server
        .admin_client()
        .post_json(
            &format!("/api/v1/library/authors/{}/want-pending", author_id),
            &serde_json::json!({}),
        )
        .await;
    resp.assert_ok();

    // Should only update the discovered book
    let body: Value = resp.json();
    assert_eq!(body["wanted_count"], 1);
}

#[tokio::test]
async fn test_want_all_pending_for_author_empty() {
    let server = TestServer::start_api_only().await;

    let author_id = create_author_db(&server, "Author With No Books").await;

    let resp = server
        .admin_client()
        .post_json(
            &format!("/api/v1/library/authors/{}/want-pending", author_id),
            &serde_json::json!({}),
        )
        .await;
    resp.assert_ok();

    let body: Value = resp.json();
    assert_eq!(body["wanted_count"], 0);
}

#[tokio::test]
async fn test_want_all_pending_for_author_requires_auth() {
    let server = TestServer::start_api_only().await;
    let client = TestClient::new(&server.api_url);

    client
        .post_json(
            &format!(
                "/api/v1/library/authors/{}/want-pending",
                uuid::Uuid::new_v4()
            ),
            &serde_json::json!({}),
        )
        .await
        .assert_status(StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn test_want_all_pending_for_series() {
    let server = TestServer::start_api_only().await;

    // Create a series
    let series_id = create_series_db(&server, "Test Series").await;

    // Create 3 discovered books
    let book1 = create_discovered_book(&server, "Book 1").await;
    let book2 = create_discovered_book(&server, "Book 2").await;
    let book3 = create_discovered_book(&server, "Book 3").await;

    // Set series for all books
    set_book_series(&server, book1, series_id).await;
    set_book_series(&server, book2, series_id).await;
    set_book_series(&server, book3, series_id).await;

    // Call bulk want endpoint
    let resp = server
        .admin_client()
        .post_json(
            &format!("/api/v1/library/series/{}/want-pending", series_id),
            &serde_json::json!({}),
        )
        .await;
    resp.assert_ok();

    let body: Value = resp.json();
    assert_eq!(body["wanted_count"], 3);

    // Verify all books are now wanted
    let statuses: Vec<String> =
        sqlx::query_scalar("SELECT want_status FROM books WHERE id = ANY($1) ORDER BY title")
            .bind([book1, book2, book3])
            .fetch_all(&server.pool)
            .await
            .unwrap();

    assert!(statuses.iter().all(|s| s == "wanted"));
}

#[tokio::test]
async fn test_want_all_pending_for_series_skips_already_wanted() {
    let server = TestServer::start_api_only().await;

    let series_id = create_series_db(&server, "Test Series").await;

    // Create books with different statuses
    let pending_book = create_discovered_book(&server, "Pending Book").await;
    let wanted_book = create_discovered_book(&server, "Wanted Book").await;
    let skipped_book = create_discovered_book(&server, "Skipped Book").await;

    // Set wanted_book to already wanted
    sqlx::query("UPDATE books SET want_status = 'wanted' WHERE id = $1")
        .bind(wanted_book)
        .execute(&server.pool)
        .await
        .unwrap();

    // Set skipped_book to skipped
    sqlx::query("UPDATE books SET want_status = 'skipped' WHERE id = $1")
        .bind(skipped_book)
        .execute(&server.pool)
        .await
        .unwrap();

    set_book_series(&server, pending_book, series_id).await;
    set_book_series(&server, wanted_book, series_id).await;
    set_book_series(&server, skipped_book, series_id).await;

    let resp = server
        .admin_client()
        .post_json(
            &format!("/api/v1/library/series/{}/want-pending", series_id),
            &serde_json::json!({}),
        )
        .await;
    resp.assert_ok();

    // Should only update the pending book (none status)
    let body: Value = resp.json();
    assert_eq!(body["wanted_count"], 1);

    // Verify pending book is now wanted
    let status: String = sqlx::query_scalar("SELECT want_status FROM books WHERE id = $1")
        .bind(pending_book)
        .fetch_one(&server.pool)
        .await
        .unwrap();
    assert_eq!(status, "wanted");

    // Verify skipped book remains skipped
    let status: String = sqlx::query_scalar("SELECT want_status FROM books WHERE id = $1")
        .bind(skipped_book)
        .fetch_one(&server.pool)
        .await
        .unwrap();
    assert_eq!(status, "skipped");
}

#[tokio::test]
async fn test_want_all_pending_for_series_skips_library_books() {
    let server = TestServer::start_api_only().await;

    let series_id = create_series_db(&server, "Test Series").await;

    // Create one discovered book and one library book
    let discovered_book = create_discovered_book(&server, "Discovered").await;
    let library_book = create_book(&server, "In Library").await;
    create_stub_book_file(&server.pool, library_book).await;

    set_book_series(&server, discovered_book, series_id).await;
    set_book_series(&server, library_book, series_id).await;

    let resp = server
        .admin_client()
        .post_json(
            &format!("/api/v1/library/series/{}/want-pending", series_id),
            &serde_json::json!({}),
        )
        .await;
    resp.assert_ok();

    // Should only update the discovered book
    let body: Value = resp.json();
    assert_eq!(body["wanted_count"], 1);
}

#[tokio::test]
async fn test_want_all_pending_for_series_empty() {
    let server = TestServer::start_api_only().await;

    let series_id = create_series_db(&server, "Series With No Books").await;

    let resp = server
        .admin_client()
        .post_json(
            &format!("/api/v1/library/series/{}/want-pending", series_id),
            &serde_json::json!({}),
        )
        .await;
    resp.assert_ok();

    let body: Value = resp.json();
    assert_eq!(body["wanted_count"], 0);
}

#[tokio::test]
async fn test_want_all_pending_for_series_requires_auth() {
    let server = TestServer::start_api_only().await;
    let client = TestClient::new(&server.api_url);

    client
        .post_json(
            &format!(
                "/api/v1/library/series/{}/want-pending",
                uuid::Uuid::new_v4()
            ),
            &serde_json::json!({}),
        )
        .await
        .assert_status(StatusCode::UNAUTHORIZED);
}

// =============================================================================
// Book Files Tests
// =============================================================================

#[tokio::test]
async fn test_get_book_files() {
    let server = TestServer::start_api_only().await;
    let book_id = create_book(&server, "Book with Files").await;

    // Create a book file entry
    let file_id = uuid::Uuid::new_v4();
    sqlx::query(
        r#"
        INSERT INTO book_files (id, book_id, format, size_bytes, file_path, file_hash, source_path, source_type, source_metadata, added_at)
        VALUES ($1, $2, 'EPUB', 12345, '/library/test/path.epub', 'abc123', '/test/path.epub', 'watch_directory', '{}', NOW())
        "#,
    )
    .bind(file_id)
    .bind(book_id)
    .execute(&server.pool)
    .await
    .expect("Failed to create book file");

    let resp = server
        .admin_client()
        .get(&format!("/api/v1/library/books/{}/files", book_id))
        .await;
    resp.assert_ok();

    let json: Value = resp.json();
    let files = json["files"].as_array().unwrap();
    assert_eq!(files.len(), 1);
    assert_eq!(files[0]["format"], "EPUB");
    assert_eq!(files[0]["size_bytes"], 12345);
    assert_eq!(files[0]["source_type"], "watch_directory");
}

#[tokio::test]
async fn test_get_book_files_empty() {
    let server = TestServer::start_api_only().await;
    let book_id = create_book(&server, "Book without Files").await;

    let resp = server
        .admin_client()
        .get(&format!("/api/v1/library/books/{}/files", book_id))
        .await;
    resp.assert_ok();

    let json: Value = resp.json();
    let files = json["files"].as_array().unwrap();
    assert!(files.is_empty());
}

#[tokio::test]
async fn test_get_book_files_not_found() {
    let server = TestServer::start_api_only().await;

    let resp = server
        .admin_client()
        .get(&format!(
            "/api/v1/library/books/{}/files",
            uuid::Uuid::new_v4()
        ))
        .await;
    resp.assert_status(StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn test_get_book_files_requires_auth() {
    let server = TestServer::start_api_only().await;
    let client = TestClient::new(&server.api_url);

    client
        .get(&format!(
            "/api/v1/library/books/{}/files",
            uuid::Uuid::new_v4()
        ))
        .await
        .assert_status(StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn test_delete_book_file() {
    let server = TestServer::start_api_only().await;
    let book_id = create_book(&server, "Book with Deletable File").await;

    // Create a book file entry
    let file_id = uuid::Uuid::new_v4();
    sqlx::query(
        r#"
        INSERT INTO book_files (id, book_id, format, size_bytes, file_path, file_hash, source_path, source_type, source_metadata, added_at)
        VALUES ($1, $2, 'EPUB', 12345, '/library/nonexistent/test.epub', 'abc123', '/nonexistent/test.epub', 'watch_directory', '{}', NOW())
        "#,
    )
    .bind(file_id)
    .bind(book_id)
    .execute(&server.pool)
    .await
    .expect("Failed to create book file");

    // Delete the file
    let resp = server
        .admin_client()
        .delete(&format!(
            "/api/v1/library/books/{}/files/{}",
            book_id, file_id
        ))
        .await;
    resp.assert_status(StatusCode::NO_CONTENT);

    // Verify file is gone
    let resp = server
        .admin_client()
        .get(&format!("/api/v1/library/books/{}/files", book_id))
        .await;
    let json: Value = resp.json();
    let files = json["files"].as_array().unwrap();
    assert!(files.is_empty());
}

#[tokio::test]
async fn test_delete_book_file_not_found() {
    let server = TestServer::start_api_only().await;
    let book_id = create_book(&server, "Book").await;

    let resp = server
        .admin_client()
        .delete(&format!(
            "/api/v1/library/books/{}/files/{}",
            book_id,
            uuid::Uuid::new_v4()
        ))
        .await;
    resp.assert_status(StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn test_delete_book_file_wrong_book() {
    let server = TestServer::start_api_only().await;
    let book_id = create_book(&server, "Book 1").await;
    let book_id2 = create_book(&server, "Book 2").await;

    // Create a file for book 1
    let file_id = uuid::Uuid::new_v4();
    sqlx::query(
        r#"
        INSERT INTO book_files (id, book_id, format, size_bytes, file_path, file_hash, source_path, source_type, source_metadata, added_at)
        VALUES ($1, $2, 'EPUB', 12345, '/library/test.epub', 'abc123', '/test.epub', 'watch_directory', '{}', NOW())
        "#,
    )
    .bind(file_id)
    .bind(book_id)
    .execute(&server.pool)
    .await
    .expect("Failed to create book file");

    // Try to delete via book 2 - should fail
    let resp = server
        .admin_client()
        .delete(&format!(
            "/api/v1/library/books/{}/files/{}",
            book_id2, file_id
        ))
        .await;
    resp.assert_status(StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn test_delete_book_file_requires_auth() {
    let server = TestServer::start_api_only().await;
    let client = TestClient::new(&server.api_url);

    client
        .delete(&format!(
            "/api/v1/library/books/{}/files/{}",
            uuid::Uuid::new_v4(),
            uuid::Uuid::new_v4()
        ))
        .await
        .assert_status(StatusCode::UNAUTHORIZED);
}

// =============================================================================
// Library Search Tests
// =============================================================================

#[tokio::test]
async fn test_search_releases_requires_auth() {
    let server = TestServer::start_api_only().await;
    let client = TestClient::new(&server.api_url);

    client
        .get("/api/v1/library/search/releases?q=test")
        .await
        .assert_status(StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn test_download_to_library_requires_auth() {
    let server = TestServer::start_api_only().await;
    let client = TestClient::new(&server.api_url);

    client
        .post_json(
            "/api/v1/library/search/download",
            &serde_json::json!({
                "release": {
                    "title": "Test Release",
                    "guid": "test-guid"
                }
            }),
        )
        .await
        .assert_status(StatusCode::UNAUTHORIZED);
}

// =============================================================================
// Around Parameter Tests (Bidirectional Pagination)
// =============================================================================

/// Tests that the `around` parameter returns books centered around the target.
/// The target book should be present in the results, with items both before and after.
#[tokio::test]
async fn test_books_around_parameter_centers_on_target() {
    let server = TestServer::start_api_only().await;

    // Create 10 books with sequential creation times
    let mut book_ids = Vec::new();
    for i in 0..10 {
        let id = create_book(&server, &format!("Book {:02}", i)).await;
        create_stub_book_file(&server.pool, id).await;
        book_ids.push(id.to_string());
        // Small delay to ensure distinct created_at timestamps
        tokio::time::sleep(tokio::time::Duration::from_millis(10)).await;
    }

    // Get a book from the middle (book 5)
    let target_id = &book_ids[5];

    // Request with around parameter
    let resp = server
        .admin_client()
        .get(&format!("/api/v1/library/books?around={}", target_id))
        .await;
    resp.assert_ok();

    let json: Value = resp.json();
    let items = json["items"].as_array().unwrap();

    // Target should be in the results
    let target_found = items
        .iter()
        .any(|item| item["id"].as_str() == Some(target_id.as_str()));
    assert!(
        target_found,
        "Target book should be in results when using around parameter"
    );

    // Should have items (the exact count depends on limit and position)
    assert!(!items.is_empty(), "Results should not be empty");
}

/// Tests that normal forward pagination and around parameter produce consistent results.
/// When the target is at the beginning, around should behave like normal pagination.
#[tokio::test]
async fn test_books_around_at_beginning_matches_forward() {
    let server = TestServer::start_api_only().await;

    // Create books
    for i in 0..5 {
        let id = create_book(&server, &format!("Book {:02}", i)).await;
        create_stub_book_file(&server.pool, id).await;
        tokio::time::sleep(tokio::time::Duration::from_millis(10)).await;
    }

    // Get normal forward pagination result
    let resp = server.admin_client().get("/api/v1/library/books").await;
    resp.assert_ok();
    let forward_json: Value = resp.json();
    let forward_items = forward_json["items"].as_array().unwrap();

    // The most recent book is first (books sort by created_at DESC)
    let first_book_id = forward_items[0]["id"].as_str().unwrap();

    // Request with around parameter for the first book
    let resp = server
        .admin_client()
        .get(&format!("/api/v1/library/books?around={}", first_book_id))
        .await;
    resp.assert_ok();

    let around_json: Value = resp.json();
    let around_items = around_json["items"].as_array().unwrap();

    // First item should be the same in both cases
    assert_eq!(
        around_items[0]["id"].as_str().unwrap(),
        first_book_id,
        "First book should be at the start when around targets the first item"
    );
}

/// Tests the around parameter for authors.
#[tokio::test]
async fn test_authors_around_parameter() {
    let server = TestServer::start_api_only().await;

    // Create books with distinct authors (authors are sorted by name ASC)
    let id = create_book_with_author(&server, "Book A", "Alice Author").await;
    create_stub_book_file(&server.pool, id).await;
    let id = create_book_with_author(&server, "Book B", "Bob Author").await;
    create_stub_book_file(&server.pool, id).await;
    let id = create_book_with_author(&server, "Book C", "Carol Author").await;
    create_stub_book_file(&server.pool, id).await;
    let id = create_book_with_author(&server, "Book D", "Diana Author").await;
    create_stub_book_file(&server.pool, id).await;
    let id = create_book_with_author(&server, "Book E", "Eve Author").await;
    create_stub_book_file(&server.pool, id).await;

    // Get list of authors to find a middle one
    let resp = server.admin_client().get("/api/v1/library/authors").await;
    resp.assert_ok();
    let json: Value = resp.json();
    let items = json["items"].as_array().unwrap();

    // Find Carol (should be in the middle alphabetically)
    let carol = items
        .iter()
        .find(|a| a["name"].as_str() == Some("Carol Author"))
        .unwrap();
    let carol_id = carol["id"].as_str().unwrap();

    // Request with around parameter
    let resp = server
        .admin_client()
        .get(&format!("/api/v1/library/authors?around={}", carol_id))
        .await;
    resp.assert_ok();

    let around_json: Value = resp.json();
    let around_items = around_json["items"].as_array().unwrap();

    // Carol should be in the results
    let carol_found = around_items
        .iter()
        .any(|a| a["id"].as_str() == Some(carol_id));
    assert!(
        carol_found,
        "Target author should be in results when using around parameter"
    );
}

/// Tests the around parameter for series.
#[tokio::test]
async fn test_series_around_parameter() {
    let server = TestServer::start_api_only().await;

    // Create books in distinct series (series are sorted by name ASC)
    let id = create_book_in_series(&server, "Book A1", "Alpha Series", 1).await;
    create_stub_book_file(&server.pool, id).await;
    let id = create_book_in_series(&server, "Book B1", "Beta Series", 1).await;
    create_stub_book_file(&server.pool, id).await;
    let id = create_book_in_series(&server, "Book G1", "Gamma Series", 1).await;
    create_stub_book_file(&server.pool, id).await;
    let id = create_book_in_series(&server, "Book D1", "Delta Series", 1).await;
    create_stub_book_file(&server.pool, id).await;
    let id = create_book_in_series(&server, "Book E1", "Epsilon Series", 1).await;
    create_stub_book_file(&server.pool, id).await;

    // Get list of series to find a middle one
    let resp = server.admin_client().get("/api/v1/library/series").await;
    resp.assert_ok();
    let json: Value = resp.json();
    let items = json["items"].as_array().unwrap();

    // Find Delta (should be in the middle alphabetically)
    let delta = items
        .iter()
        .find(|s| s["name"].as_str() == Some("Delta Series"))
        .unwrap();
    let delta_id = delta["id"].as_str().unwrap();

    // Request with around parameter
    let resp = server
        .admin_client()
        .get(&format!("/api/v1/library/series?around={}", delta_id))
        .await;
    resp.assert_ok();

    let around_json: Value = resp.json();
    let around_items = around_json["items"].as_array().unwrap();

    // Delta should be in the results
    let delta_found = around_items
        .iter()
        .any(|s| s["id"].as_str() == Some(delta_id));
    assert!(
        delta_found,
        "Target series should be in results when using around parameter"
    );
}

/// Tests that around with non-existent ID returns 404.
#[tokio::test]
async fn test_books_around_invalid_id() {
    let server = TestServer::start_api_only().await;

    let id = create_book(&server, "Test Book").await;
    create_stub_book_file(&server.pool, id).await;

    // Request with non-existent ID (valid UUID format)
    let fake_id = "00000000-0000-0000-0000-000000000000";
    let resp = server
        .admin_client()
        .get(&format!("/api/v1/library/books?around={}", fake_id))
        .await;

    // Should return 404 for non-existent ID
    resp.assert_status(StatusCode::NOT_FOUND);
}

/// Tests that around returns both prev_cursor and next_cursor when target is in the middle.
/// This test catches the bug where has_prev/has_next were always false because
/// we queried for `limit` items but checked if `count > limit` (impossible).
#[tokio::test]
async fn test_books_around_returns_both_cursors_for_middle_item() {
    let server = TestServer::start_api_only().await;

    // Create enough books to have items before and after
    let mut book_ids = Vec::new();
    for i in 0..15 {
        let id = create_book(&server, &format!("Book {:02}", i)).await;
        create_stub_book_file(&server.pool, id).await;
        book_ids.push(id.to_string());
        tokio::time::sleep(tokio::time::Duration::from_millis(10)).await;
    }

    // Books are sorted by created_at DESC, so book 14 is first, book 0 is last
    // Pick a book from the middle (book 7 should be around position 7-8 from the end)
    let target_id = &book_ids[7];

    // Use a small limit to ensure we have items on both sides
    let resp = server
        .admin_client()
        .get(&format!(
            "/api/v1/library/books?around={}&limit=5",
            target_id
        ))
        .await;
    resp.assert_ok();

    let json: Value = resp.json();

    // With 15 books and target at position ~7, limit=5 should give us
    // 2 before + target + 2 after = 5 items, with more on both sides
    assert!(
        json["prev_cursor"].is_string(),
        "prev_cursor should be set when there are items before the window. Got: {:?}",
        json["prev_cursor"]
    );
    assert!(
        json["next_cursor"].is_string(),
        "next_cursor should be set when there are items after the window. Got: {:?}",
        json["next_cursor"]
    );
}

/// Tests that around returns no prev_cursor when target is at the beginning.
#[tokio::test]
async fn test_books_around_no_prev_cursor_at_start() {
    let server = TestServer::start_api_only().await;

    // Create books
    let mut book_ids = Vec::new();
    for i in 0..10 {
        let id = create_book(&server, &format!("Book {:02}", i)).await;
        create_stub_book_file(&server.pool, id).await;
        book_ids.push(id.to_string());
    }

    // Books are sorted by title ASC (alphabetically), so "Book 00" is first
    let first_book_id = &book_ids[0];

    let resp = server
        .admin_client()
        .get(&format!(
            "/api/v1/library/books?around={}&limit=5",
            first_book_id
        ))
        .await;
    resp.assert_ok();

    let json: Value = resp.json();

    // First item should have no prev_cursor
    assert!(
        json["prev_cursor"].is_null(),
        "prev_cursor should be null when at the beginning. Got: {:?}",
        json["prev_cursor"]
    );
    // But should have next_cursor since there are more items
    assert!(
        json["next_cursor"].is_string(),
        "next_cursor should be set when there are items after. Got: {:?}",
        json["next_cursor"]
    );
}

/// Tests that around returns no next_cursor when target is at the end.
#[tokio::test]
async fn test_books_around_no_next_cursor_at_end() {
    let server = TestServer::start_api_only().await;

    // Create books
    let mut book_ids = Vec::new();
    for i in 0..10 {
        let id = create_book(&server, &format!("Book {:02}", i)).await;
        create_stub_book_file(&server.pool, id).await;
        book_ids.push(id.to_string());
    }

    // Books are sorted by title ASC (alphabetically), so "Book 09" is last
    let last_book_id = &book_ids[9];

    let resp = server
        .admin_client()
        .get(&format!(
            "/api/v1/library/books?around={}&limit=5",
            last_book_id
        ))
        .await;
    resp.assert_ok();

    let json: Value = resp.json();

    // Last item should have no next_cursor
    assert!(
        json["next_cursor"].is_null(),
        "next_cursor should be null when at the end. Got: {:?}",
        json["next_cursor"]
    );
    // But should have prev_cursor since there are more items before
    assert!(
        json["prev_cursor"].is_string(),
        "prev_cursor should be set when there are items before. Got: {:?}",
        json["prev_cursor"]
    );
}

/// Tests that authors around returns both cursors when target is in the middle.
#[tokio::test]
async fn test_authors_around_returns_both_cursors_for_middle_item() {
    let server = TestServer::start_api_only().await;

    // Create books with distinct authors (sorted by name ASC)
    let author_names = [
        "Alice", "Bob", "Carol", "Diana", "Eve", "Frank", "Grace", "Henry", "Ivy", "Jack",
    ];
    for name in &author_names {
        let id = create_book_with_author(&server, &format!("Book by {}", name), name).await;
        create_stub_book_file(&server.pool, id).await;
    }

    // Get list of authors
    let resp = server.admin_client().get("/api/v1/library/authors").await;
    resp.assert_ok();
    let json: Value = resp.json();
    let items = json["items"].as_array().unwrap();

    // Find a middle author (Eve should be in the middle alphabetically)
    let eve = items
        .iter()
        .find(|a| a["name"].as_str() == Some("Eve"))
        .unwrap();
    let eve_id = eve["id"].as_str().unwrap();

    // Request with around and small limit
    let resp = server
        .admin_client()
        .get(&format!(
            "/api/v1/library/authors?around={}&limit=5",
            eve_id
        ))
        .await;
    resp.assert_ok();

    let json: Value = resp.json();

    assert!(
        json["prev_cursor"].is_string(),
        "prev_cursor should be set for middle author. Got: {:?}",
        json["prev_cursor"]
    );
    assert!(
        json["next_cursor"].is_string(),
        "next_cursor should be set for middle author. Got: {:?}",
        json["next_cursor"]
    );
}

/// Tests that series around returns both cursors when target is in the middle.
#[tokio::test]
async fn test_series_around_returns_both_cursors_for_middle_item() {
    let server = TestServer::start_api_only().await;

    // Create books in distinct series (sorted by name ASC)
    let series_names = [
        "Alpha", "Beta", "Charlie", "Delta", "Echo", "Foxtrot", "Golf", "Hotel", "India", "Juliet",
    ];
    for (i, name) in series_names.iter().enumerate() {
        let id = create_book_in_series(&server, &format!("Book {}", i), name, 1).await;
        create_stub_book_file(&server.pool, id).await;
    }

    // Get list of series
    let resp = server.admin_client().get("/api/v1/library/series").await;
    resp.assert_ok();
    let json: Value = resp.json();
    let items = json["items"].as_array().unwrap();

    // Find a middle series (Echo should be in the middle alphabetically)
    let echo = items
        .iter()
        .find(|s| s["name"].as_str() == Some("Echo"))
        .unwrap();
    let echo_id = echo["id"].as_str().unwrap();

    // Request with around and small limit
    let resp = server
        .admin_client()
        .get(&format!(
            "/api/v1/library/series?around={}&limit=5",
            echo_id
        ))
        .await;
    resp.assert_ok();

    let json: Value = resp.json();

    assert!(
        json["prev_cursor"].is_string(),
        "prev_cursor should be set for middle series. Got: {:?}",
        json["prev_cursor"]
    );
    assert!(
        json["next_cursor"].is_string(),
        "next_cursor should be set for middle series. Got: {:?}",
        json["next_cursor"]
    );
}

/// Tests that backward pagination returns prev_cursor and items in correct order.
#[tokio::test]
async fn test_books_backward_pagination() {
    let server = TestServer::start_api_only().await;

    // Create enough books to paginate (20 books for 4 pages of 5)
    for i in 0..20 {
        let id = create_book(&server, &format!("Book {:02}", i)).await;
        create_stub_book_file(&server.pool, id).await;
        tokio::time::sleep(tokio::time::Duration::from_millis(10)).await;
    }

    // Navigate forward through multiple pages to get a cursor deep in the list
    // Page 1
    let resp = server
        .admin_client()
        .get("/api/v1/library/books?limit=5")
        .await;
    resp.assert_ok();
    let json: Value = resp.json();
    let cursor1 = json["next_cursor"].as_str().unwrap();

    // Page 2
    let resp = server
        .admin_client()
        .get(&format!("/api/v1/library/books?cursor={}&limit=5", cursor1))
        .await;
    resp.assert_ok();
    let json: Value = resp.json();
    let cursor2 = json["next_cursor"].as_str().unwrap();

    // Page 3
    let resp = server
        .admin_client()
        .get(&format!("/api/v1/library/books?cursor={}&limit=5", cursor2))
        .await;
    resp.assert_ok();
    let json: Value = resp.json();
    let cursor3 = json["next_cursor"].as_str().unwrap();

    // Now go backward from cursor3 (which is after page 3)
    // This should return items from before that position
    let resp = server
        .admin_client()
        .get(&format!(
            "/api/v1/library/books?cursor={}&direction=backward&limit=5",
            cursor3
        ))
        .await;
    resp.assert_ok();

    let json: Value = resp.json();

    // Backward pagination should return items
    let backward_items = json["items"].as_array().unwrap();
    assert!(
        !backward_items.is_empty(),
        "Should get items from backward pagination"
    );

    // Since we're going backward from deep in the list, there should be more items before
    assert!(
        json["prev_cursor"].is_string(),
        "backward pagination should return prev_cursor when there are more items before. Got: {:?}",
        json["prev_cursor"]
    );
}

/// Tests that authors backward pagination works correctly.
#[tokio::test]
async fn test_authors_backward_pagination() {
    let server = TestServer::start_api_only().await;

    // Create books with distinct authors (15 authors for multiple pages)
    let author_names = [
        "Alice", "Bob", "Carol", "Diana", "Eve", "Frank", "Grace", "Henry", "Ivy", "Jack", "Kate",
        "Liam", "Mia", "Noah", "Olivia",
    ];
    for name in &author_names {
        let id = create_book_with_author(&server, &format!("Book by {}", name), name).await;
        create_stub_book_file(&server.pool, id).await;
    }

    // Navigate forward through multiple pages
    // Page 1
    let resp = server
        .admin_client()
        .get("/api/v1/library/authors?limit=5")
        .await;
    resp.assert_ok();
    let json: Value = resp.json();
    let cursor1 = json["next_cursor"].as_str().unwrap();

    // Page 2
    let resp = server
        .admin_client()
        .get(&format!(
            "/api/v1/library/authors?cursor={}&limit=5",
            cursor1
        ))
        .await;
    resp.assert_ok();
    let json: Value = resp.json();
    let cursor2 = json["next_cursor"].as_str().unwrap();

    // Go backward from cursor2 (which is after page 2)
    let resp = server
        .admin_client()
        .get(&format!(
            "/api/v1/library/authors?cursor={}&direction=backward&limit=5",
            cursor2
        ))
        .await;
    resp.assert_ok();

    let json: Value = resp.json();

    // Backward pagination should return items
    let backward_items = json["items"].as_array().unwrap();
    assert!(
        !backward_items.is_empty(),
        "Should get items from backward pagination"
    );

    // Since we're going backward from deep in the list, there should be more items before
    assert!(
        json["prev_cursor"].is_string(),
        "backward pagination should return prev_cursor when there are more items before. Got: {:?}",
        json["prev_cursor"]
    );
}

/// Tests that series backward pagination works correctly.
#[tokio::test]
async fn test_series_backward_pagination() {
    let server = TestServer::start_api_only().await;

    // Create books in distinct series (15 series for multiple pages)
    let series_names = [
        "Alpha", "Beta", "Charlie", "Delta", "Echo", "Foxtrot", "Golf", "Hotel", "India", "Juliet",
        "Kilo", "Lima", "Mike", "November", "Oscar",
    ];
    for (i, name) in series_names.iter().enumerate() {
        let id = create_book_in_series(&server, &format!("Book {}", i), name, 1).await;
        create_stub_book_file(&server.pool, id).await;
    }

    // Navigate forward through multiple pages
    // Page 1
    let resp = server
        .admin_client()
        .get("/api/v1/library/series?limit=5")
        .await;
    resp.assert_ok();
    let json: Value = resp.json();
    let cursor1 = json["next_cursor"].as_str().unwrap();

    // Page 2
    let resp = server
        .admin_client()
        .get(&format!(
            "/api/v1/library/series?cursor={}&limit=5",
            cursor1
        ))
        .await;
    resp.assert_ok();
    let json: Value = resp.json();
    let cursor2 = json["next_cursor"].as_str().unwrap();

    // Go backward from cursor2 (which is after page 2)
    let resp = server
        .admin_client()
        .get(&format!(
            "/api/v1/library/series?cursor={}&direction=backward&limit=5",
            cursor2
        ))
        .await;
    resp.assert_ok();

    let json: Value = resp.json();

    // Backward pagination should return items
    let backward_items = json["items"].as_array().unwrap();
    assert!(
        !backward_items.is_empty(),
        "Should get items from backward pagination"
    );

    // Since we're going backward from deep in the list, there should be more items before
    assert!(
        json["prev_cursor"].is_string(),
        "backward pagination should return prev_cursor when there are more items before. Got: {:?}",
        json["prev_cursor"]
    );
}

// =============================================================================
// Format Filter Tests
// =============================================================================

/// Helper: create a wanted book (not in library) with a specific want_format
async fn create_wanted_book(server: &TestServer, title: &str, want_format: &str) -> uuid::Uuid {
    let book_id = uuid::Uuid::new_v4();
    sqlx::query(
        r#"
        INSERT INTO books (id, title, want_status, want_format, created_at, updated_at)
        VALUES ($1, $2, 'wanted', $3, NOW(), NOW())
        "#,
    )
    .bind(book_id)
    .bind(title)
    .bind(want_format)
    .execute(&server.pool)
    .await
    .expect("Failed to create wanted book");
    book_id
}

#[tokio::test]
async fn test_list_books_format_ebook() {
    let server = TestServer::start_api_only().await;

    let ebook = create_book(&server, "Ebook Only").await;
    add_ebook_file(&server, ebook).await;

    let audiobook = create_book(&server, "Audiobook Only").await;
    add_audiobook_file(&server, audiobook).await;

    let both = create_book(&server, "Both Formats").await;
    add_ebook_file(&server, both).await;
    add_audiobook_file(&server, both).await;

    // format=ebook should return books with ebook files
    let resp = server
        .admin_client()
        .get("/api/v1/library/books?format=ebook")
        .await;
    resp.assert_ok();

    let json: Value = resp.json();
    let items = json["items"].as_array().unwrap();
    let titles: Vec<&str> = items.iter().map(|i| i["title"].as_str().unwrap()).collect();
    assert!(
        titles.contains(&"Ebook Only"),
        "Should include ebook-only book"
    );
    assert!(
        titles.contains(&"Both Formats"),
        "Should include both-formats book"
    );
    assert!(
        !titles.contains(&"Audiobook Only"),
        "Should exclude audiobook-only book"
    );
}

#[tokio::test]
async fn test_list_books_format_audiobook() {
    let server = TestServer::start_api_only().await;

    let ebook = create_book(&server, "Ebook Only").await;
    add_ebook_file(&server, ebook).await;

    let audiobook = create_book(&server, "Audiobook Only").await;
    add_audiobook_file(&server, audiobook).await;

    let both = create_book(&server, "Both Formats").await;
    add_ebook_file(&server, both).await;
    add_audiobook_file(&server, both).await;

    // format=audiobook should return books with audiobook files
    let resp = server
        .admin_client()
        .get("/api/v1/library/books?format=audiobook")
        .await;
    resp.assert_ok();

    let json: Value = resp.json();
    let items = json["items"].as_array().unwrap();
    let titles: Vec<&str> = items.iter().map(|i| i["title"].as_str().unwrap()).collect();
    assert!(
        titles.contains(&"Audiobook Only"),
        "Should include audiobook-only book"
    );
    assert!(
        titles.contains(&"Both Formats"),
        "Should include both-formats book"
    );
    assert!(
        !titles.contains(&"Ebook Only"),
        "Should exclude ebook-only book"
    );
}

#[tokio::test]
async fn test_list_books_format_includes_wants() {
    let server = TestServer::start_api_only().await;

    // Library book with ebook file
    let ebook = create_book(&server, "Library Ebook").await;
    add_ebook_file(&server, ebook).await;

    // Wanted ebook (no files)
    create_wanted_book(&server, "Wanted Ebook", "ebook").await;

    // Wanted audiobook (no files)
    create_wanted_book(&server, "Wanted Audiobook", "audiobook").await;

    // Wanted both formats
    create_wanted_book(&server, "Wanted Both", "both").await;

    // format=ebook should include wanted ebooks and wanted "both"
    let resp = server
        .admin_client()
        .get("/api/v1/library/books?format=ebook&want_status=library_visible")
        .await;
    resp.assert_ok();

    let json: Value = resp.json();
    let items = json["items"].as_array().unwrap();
    let titles: Vec<&str> = items.iter().map(|i| i["title"].as_str().unwrap()).collect();
    assert!(
        titles.contains(&"Library Ebook"),
        "Should include library ebook"
    );
    assert!(
        titles.contains(&"Wanted Ebook"),
        "Should include wanted ebook"
    );
    assert!(
        titles.contains(&"Wanted Both"),
        "Should include wanted both"
    );
    assert!(
        !titles.contains(&"Wanted Audiobook"),
        "Should exclude wanted audiobook"
    );

    // format=audiobook should include wanted audiobooks and wanted "both"
    let resp = server
        .admin_client()
        .get("/api/v1/library/books?format=audiobook&want_status=library_visible")
        .await;
    resp.assert_ok();

    let json: Value = resp.json();
    let items = json["items"].as_array().unwrap();
    let titles: Vec<&str> = items.iter().map(|i| i["title"].as_str().unwrap()).collect();
    assert!(
        titles.contains(&"Wanted Audiobook"),
        "Should include wanted audiobook"
    );
    assert!(
        titles.contains(&"Wanted Both"),
        "Should include wanted both"
    );
    assert!(
        !titles.contains(&"Wanted Ebook"),
        "Should exclude wanted ebook"
    );
}

#[tokio::test]
async fn test_list_books_has_audiobook_backward_compat() {
    let server = TestServer::start_api_only().await;

    let ebook = create_book(&server, "Ebook Only").await;
    add_ebook_file(&server, ebook).await;

    let audiobook = create_book(&server, "Audiobook Book").await;
    add_audiobook_file(&server, audiobook).await;

    // has_audiobook=true should behave like format=audiobook
    let resp = server
        .admin_client()
        .get("/api/v1/library/books?has_audiobook=true")
        .await;
    resp.assert_ok();

    let json: Value = resp.json();
    let items = json["items"].as_array().unwrap();
    assert_eq!(items.len(), 1);
    assert_eq!(items[0]["title"], "Audiobook Book");
}

#[tokio::test]
async fn test_list_series_format_filter() {
    let server = TestServer::start_api_only().await;

    // Create series with only ebook
    let ebook_book = create_book_in_series(&server, "Ebook Series Book", "Ebook Series", 1).await;
    add_ebook_file(&server, ebook_book).await;

    // Create series with only audiobook
    let audio_book = create_book_in_series(&server, "Audio Series Book", "Audio Series", 1).await;
    add_audiobook_file(&server, audio_book).await;

    // format=ebook should only return the ebook series
    let resp = server
        .admin_client()
        .get("/api/v1/library/series?format=ebook")
        .await;
    resp.assert_ok();

    let json: Value = resp.json();
    let items = json["items"].as_array().unwrap();
    assert_eq!(items.len(), 1);
    assert_eq!(items[0]["name"], "Ebook Series");

    // format=audiobook should only return the audio series
    let resp = server
        .admin_client()
        .get("/api/v1/library/series?format=audiobook")
        .await;
    resp.assert_ok();

    let json: Value = resp.json();
    let items = json["items"].as_array().unwrap();
    assert_eq!(items.len(), 1);
    assert_eq!(items[0]["name"], "Audio Series");
}

#[tokio::test]
async fn test_list_authors_format_filter() {
    let server = TestServer::start_api_only().await;

    // Create books by different authors with different formats
    let ebook_book = create_book_with_author(&server, "Ebook by Alice", "Alice").await;
    add_ebook_file(&server, ebook_book).await;

    let audio_book = create_book_with_author(&server, "Audiobook by Bob", "Bob").await;
    add_audiobook_file(&server, audio_book).await;

    // format=ebook should only return Alice
    let resp = server
        .admin_client()
        .get("/api/v1/library/authors?format=ebook")
        .await;
    resp.assert_ok();

    let json: Value = resp.json();
    let items = json["items"].as_array().unwrap();
    assert_eq!(items.len(), 1);
    assert_eq!(items[0]["name"], "Alice");

    // format=audiobook should only return Bob
    let resp = server
        .admin_client()
        .get("/api/v1/library/authors?format=audiobook")
        .await;
    resp.assert_ok();

    let json: Value = resp.json();
    let items = json["items"].as_array().unwrap();
    assert_eq!(items.len(), 1);
    assert_eq!(items[0]["name"], "Bob");
}

/// "Unwant All" for an author must take back the wants it can, and nothing else:
/// a pending book was never asked for, a skipped one was refused on purpose, and
/// an owned one is already here.
#[tokio::test]
async fn test_unwant_all_for_author() {
    let server = TestServer::start_api_only().await;
    let author_id = create_author_db(&server, "Unwant Author").await;

    let wanted = create_discovered_book(&server, "Wanted Book").await;
    let searching = create_discovered_book(&server, "Searching Book").await;
    let pending = create_discovered_book(&server, "Pending Book").await;
    let skipped = create_discovered_book(&server, "Skipped Book").await;
    let owned = create_library_book_db(&server, "Owned Book").await;

    for (book, status) in [
        (wanted, "wanted"),
        (searching, "searching"),
        (skipped, "skipped"),
        (owned, "wanted"),
    ] {
        sqlx::query("UPDATE books SET want_status = $2 WHERE id = $1")
            .bind(book)
            .bind(status)
            .execute(&server.pool)
            .await
            .unwrap();
    }
    for book in [wanted, searching, pending, skipped, owned] {
        link_book_to_author(&server, book, author_id).await;
    }

    let resp = server
        .admin_client()
        .delete(&format!(
            "/api/v1/library/authors/{}/want-pending",
            author_id
        ))
        .await;
    resp.assert_ok();
    assert_eq!(resp.json::<Value>()["unwanted_count"], 2);

    for (book, expected) in [
        (wanted, "none"),
        (searching, "none"),
        (pending, "pending"),
        (skipped, "skipped"),
        (owned, "wanted"),
    ] {
        let status: String = sqlx::query_scalar("SELECT want_status FROM books WHERE id = $1")
            .bind(book)
            .fetch_one(&server.pool)
            .await
            .unwrap();
        assert_eq!(status, expected, "book {book}");
    }
}

#[tokio::test]
async fn test_unwant_all_for_series() {
    let server = TestServer::start_api_only().await;
    let series_id = create_series_db(&server, "Unwant Series").await;

    let wanted = create_discovered_book(&server, "Series Wanted").await;
    let pending = create_discovered_book(&server, "Series Pending").await;
    sqlx::query("UPDATE books SET want_status = 'wanted' WHERE id = $1")
        .bind(wanted)
        .execute(&server.pool)
        .await
        .unwrap();
    set_book_series(&server, wanted, series_id).await;
    set_book_series(&server, pending, series_id).await;

    let resp = server
        .admin_client()
        .delete(&format!(
            "/api/v1/library/series/{}/want-pending",
            series_id
        ))
        .await;
    resp.assert_ok();
    assert_eq!(resp.json::<Value>()["unwanted_count"], 1);

    let status: String = sqlx::query_scalar("SELECT want_status FROM books WHERE id = $1")
        .bind(wanted)
        .fetch_one(&server.pool)
        .await
        .unwrap();
    assert_eq!(status, "none");
    let status: String = sqlx::query_scalar("SELECT want_status FROM books WHERE id = $1")
        .bind(pending)
        .fetch_one(&server.pool)
        .await
        .unwrap();
    assert_eq!(status, "pending");
}

#[tokio::test]
async fn test_unwant_all_requires_auth() {
    let server = TestServer::start_api_only().await;
    let author_id = create_author_db(&server, "Auth Check Author").await;

    let client = TestClient::new(&server.api_url);
    let resp = client
        .delete(&format!(
            "/api/v1/library/authors/{}/want-pending",
            author_id
        ))
        .await;
    assert_eq!(resp.status, StatusCode::UNAUTHORIZED);
}

// =============================================================================
// Alphabet bar under the audiobook filter
// =============================================================================

/// The alphabet bar's jump target has to answer for the same shelf the grid is showing.
/// Without a format to filter on it answered for every book, so on an audiobooks-only
/// view it jumped to ebooks that the grid then dropped, leaving the letter apparently
/// empty.
///
/// Only the jump target is covered here: the letter list itself is served from the
/// search backend, which is stubbed out in tests.
#[tokio::test]
async fn test_first_by_letter_respects_the_format_filter() {
    let server = TestServer::start_api_only().await;

    let ebook = create_book(&server, "Alpha").await;
    add_ebook_file(&server, ebook).await;
    let audiobook = create_book(&server, "Bravo").await;
    add_audiobook_file(&server, audiobook).await;

    let first = async |query: &str| {
        server
            .admin_client()
            .get(&format!(
                "/api/v1/library/books/first-by-letter?in_library=true&{query}"
            ))
            .await
    };

    let unfiltered = first("letter=A").await;
    unfiltered.assert_ok();
    assert_eq!(unfiltered.json::<Value>()["title"].as_str(), Some("Alpha"));

    first("letter=A&format=audiobook")
        .await
        .assert_status(StatusCode::NOT_FOUND);

    let audio = first("letter=B&format=audiobook").await;
    audio.assert_ok();
    assert_eq!(audio.json::<Value>()["title"].as_str(), Some("Bravo"));

    first("letter=B&format=ebook")
        .await
        .assert_status(StatusCode::NOT_FOUND);
}

async fn available_letters(server: &TestServer, query: &str) -> Vec<String> {
    let resp = server
        .admin_client()
        .get(&format!("/api/v1/library/books/available-letters?{query}"))
        .await;
    resp.assert_ok();
    resp.json::<Value>()["letters"]
        .as_array()
        .unwrap()
        .iter()
        .map(|l| l.as_str().unwrap().to_string())
        .collect()
}

/// The bar itself, served from SQL because the search index is not available — which is
/// exactly the path these tests run on, and the path a kobbler running without a healthy
/// Meili runs on. It used to answer 500, so every library page lost its alphabet.
#[tokio::test]
async fn test_available_letters_fall_back_to_sql() {
    let server = TestServer::start_api_only().await;

    let ebook = create_book(&server, "Alpha").await;
    add_ebook_file(&server, ebook).await;
    let audiobook = create_book(&server, "Bravo").await;
    add_audiobook_file(&server, audiobook).await;
    let numeric = create_book(&server, "1984").await;
    add_ebook_file(&server, numeric).await;
    // Wanted, no file: a record of an intention, not a book on the shelf.
    create_book(&server, "Zulu").await;

    assert_eq!(
        available_letters(&server, "in_library=true").await,
        vec!["A".to_string(), "B".to_string(), "#".to_string()],
        "a non-alphabetic title files under # and # sorts last"
    );

    assert_eq!(
        available_letters(&server, "in_library=true&format=audiobook").await,
        vec!["B".to_string()],
    );

    assert_eq!(
        available_letters(&server, "in_library=true&format=ebook").await,
        vec!["A".to_string(), "#".to_string()],
    );

    assert_eq!(
        available_letters(&server, "in_library=false").await,
        vec!["Z".to_string()],
        "the un-owned shelf is the complement, not everything"
    );
}
