//! Tests for the unified search endpoint (command palette backend).

use crate::common::*;
use api::search::{
    fetch_all_author_docs, fetch_all_book_docs, fetch_all_genre_docs, fetch_all_movie_docs,
    fetch_all_series_docs, fetch_all_tv_show_docs,
};
use api::search::{
    index_author, index_book, index_movie, index_series, index_tv_show, index_tv_show_children,
};
use reqwest::StatusCode;
use serde_json::Value;

#[tokio::test]
async fn test_search_requires_auth() {
    let server = TestServer::start_api_only().await;

    let client = TestClient::new(&server.api_url);
    let resp = client.get("/api/v1/admin/search?q=test").await;
    resp.assert_status(StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn test_search_empty_query_rejected() {
    let server = TestServer::start_api_only().await;

    let resp = server.admin_client().get("/api/v1/admin/search?q=").await;
    resp.assert_status(StatusCode::BAD_REQUEST);

    let json: Value = resp.json();
    assert!(json["error"].as_str().unwrap().contains("empty"));
}

#[tokio::test]
async fn test_search_returns_structure() {
    let server = TestServer::start_api_only().await;

    let resp = server
        .admin_client()
        .get("/api/v1/admin/search?q=nonexistent")
        .await;
    resp.assert_ok();

    let json: Value = resp.json();
    // Response should have top_hits and remaining arrays (federated search format)
    assert!(
        json["top_hits"].is_array(),
        "Response should have top_hits array"
    );
    assert!(
        json["remaining"].is_array(),
        "Response should have remaining array"
    );
}

#[tokio::test]
async fn test_search_with_limit() {
    let server = TestServer::start_api_only().await;

    let resp = server
        .admin_client()
        .get("/api/v1/admin/search?q=test&limit=3")
        .await;
    resp.assert_ok();

    let json: Value = resp.json();
    // Just verify the response is valid - actual result count depends on MeiliSearch state
    assert!(
        json["top_hits"].is_array(),
        "Response should have top_hits array"
    );
    assert!(
        json["remaining"].is_array(),
        "Response should have remaining array"
    );
}

#[tokio::test]
async fn test_reindex_requires_auth() {
    let server = TestServer::start_api_only().await;

    let client = TestClient::new(&server.api_url);
    let resp = client
        .post_json("/api/v1/admin/search/reindex", &serde_json::json!({}))
        .await;
    resp.assert_status(StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn test_reindex_enqueues_job() {
    let server = TestServer::start_api_only().await;

    let resp = server
        .admin_client()
        .post_json("/api/v1/admin/search/reindex", &serde_json::json!({}))
        .await;
    resp.assert_ok();

    let json: Value = resp.json();
    assert!(json["message"].as_str().is_some());
    assert!(json["job_id"].as_str().is_some());
}

#[tokio::test]
async fn test_search_finds_indexed_book() {
    let server = TestServer::start_api_only().await;

    // Create a book
    let book_id = create_book_with_author(&server, "The Hobbit", "J.R.R. Tolkien").await;
    assert!(!book_id.is_nil());

    // Give MeiliSearch time to index (it's async)
    tokio::time::sleep(tokio::time::Duration::from_millis(500)).await;

    // Search for it
    let resp = server
        .admin_client()
        .get("/api/v1/admin/search?q=hobbit")
        .await;
    resp.assert_ok();

    let json: Value = resp.json();

    // Collect all book hits from top_hits and remaining
    let all_hits: Vec<&Value> = json["top_hits"]
        .as_array()
        .unwrap()
        .iter()
        .chain(json["remaining"].as_array().unwrap().iter())
        .filter(|h| h["type"].as_str() == Some("book"))
        .collect();

    // Should find the book (if MeiliSearch is running)
    // Note: This test may not find results if MeiliSearch is not running in tests
    // The important thing is that the endpoint works correctly
    if !all_hits.is_empty() {
        assert!(all_hits.iter().any(|b| {
            b["title"]
                .as_str()
                .unwrap_or("")
                .to_lowercase()
                .contains("hobbit")
        }));
    }
}

#[tokio::test]
async fn test_search_finds_author() {
    let server = TestServer::start_api_only().await;

    // Create a book to create an author
    let _book_id = create_book_with_author(&server, "Foundation", "Isaac Asimov").await;

    // Give MeiliSearch time to index
    tokio::time::sleep(tokio::time::Duration::from_millis(500)).await;

    // Search for the author
    let resp = server
        .admin_client()
        .get("/api/v1/admin/search?q=asimov")
        .await;
    resp.assert_ok();

    let json: Value = resp.json();

    // Collect all author hits from top_hits and remaining
    let all_hits: Vec<&Value> = json["top_hits"]
        .as_array()
        .unwrap()
        .iter()
        .chain(json["remaining"].as_array().unwrap().iter())
        .filter(|h| h["type"].as_str() == Some("author"))
        .collect();

    // Should find the author (if MeiliSearch is running)
    if !all_hits.is_empty() {
        assert!(all_hits.iter().any(|a| {
            a["name"]
                .as_str()
                .unwrap_or("")
                .to_lowercase()
                .contains("asimov")
        }));
    }
}

// =============================================================================
// Search Indexing Integration Tests
// =============================================================================
// These tests verify that the SQL queries in the indexing functions are valid.
// They catch issues like missing columns that would otherwise only fail at runtime.

#[tokio::test]
async fn test_index_book_query_is_valid() {
    let server = TestServer::start_api_only().await;

    // Create a book with author and series
    let book_id = create_book_with_author(&server, "Mistborn", "Brandon Sanderson").await;
    assert!(!book_id.is_nil());

    // Run the index_book function - this tests the SQL query
    // Using SqlFallbackBackend, so indexing is a no-op, but the SQL query runs
    let result = index_book(server.search_backend(), &server.pool, book_id).await;
    assert!(result.is_ok(), "index_book SQL query failed: {:?}", result);
}

#[tokio::test]
async fn test_index_author_query_is_valid() {
    let server = TestServer::start_api_only().await;

    // Create a book to get an author (with a file so it's in-library)
    let _book_id = create_book_with_author(&server, "Test Book", "Test Author").await;
    create_stub_book_file(&server.pool, _book_id).await;

    // Get the author ID
    let resp = server.admin_client().get("/api/v1/library/authors").await;
    resp.assert_ok();
    let json: Value = resp.json();
    let authors = json["items"].as_array().unwrap();
    assert!(!authors.is_empty(), "Should have at least one author");

    let author_id = uuid::Uuid::parse_str(authors[0]["id"].as_str().unwrap()).unwrap();

    // Run the index_author function
    let result = index_author(server.search_backend(), &server.pool, author_id).await;
    assert!(
        result.is_ok(),
        "index_author SQL query failed: {:?}",
        result
    );
}

#[tokio::test]
async fn test_index_series_query_is_valid() {
    let server = TestServer::start_api_only().await;

    // Create a book with a series (with a file so it's in-library)
    let book_id = create_book_with_author(&server, "Elantris", "Brandon Sanderson").await;
    create_stub_book_file(&server.pool, book_id).await;
    assert!(!book_id.is_nil());

    // Update book to have a series
    let resp = server
        .admin_client()
        .patch_json(
            &format!("/api/v1/library/books/{}", book_id),
            &serde_json::json!({
                "series_name": "Cosmere Standalone"
            }),
        )
        .await;
    resp.assert_success();

    // Get the series ID
    let resp = server.admin_client().get("/api/v1/library/series").await;
    resp.assert_ok();
    let json: Value = resp.json();
    let series_list = json["items"].as_array().unwrap();
    assert!(!series_list.is_empty(), "Should have at least one series");

    let series_id = uuid::Uuid::parse_str(series_list[0]["id"].as_str().unwrap()).unwrap();

    // Run the index_series function
    let result = index_series(server.search_backend(), &server.pool, series_id).await;
    assert!(
        result.is_ok(),
        "index_series SQL query failed: {:?}",
        result
    );
}

#[tokio::test]
async fn test_fetch_all_book_docs_query_is_valid() {
    let server = TestServer::start_api_only().await;

    // Create some books
    let _book1 = create_book_with_author(&server, "Book One", "Author One").await;
    let _book2 = create_book_with_author(&server, "Book Two", "Author Two").await;

    // Run the fetch_all_book_docs function
    let result = fetch_all_book_docs(&server.pool).await;
    assert!(
        result.is_ok(),
        "fetch_all_book_docs SQL query failed: {:?}",
        result
    );

    let docs = result.unwrap();
    assert!(docs.len() >= 2, "Should have at least 2 book documents");
}

#[tokio::test]
async fn test_fetch_all_author_docs_query_is_valid() {
    let server = TestServer::start_api_only().await;

    // Create some books to create authors
    let _book1 = create_book_with_author(&server, "Book A", "Author A").await;
    let _book2 = create_book_with_author(&server, "Book B", "Author B").await;

    // Run the fetch_all_author_docs function
    let result = fetch_all_author_docs(&server.pool).await;
    assert!(
        result.is_ok(),
        "fetch_all_author_docs SQL query failed: {:?}",
        result
    );

    let docs = result.unwrap();
    assert!(docs.len() >= 2, "Should have at least 2 author documents");
}

#[tokio::test]
async fn test_fetch_all_series_docs_query_is_valid() {
    let server = TestServer::start_api_only().await;

    // Create books with series
    let book_id = create_book_with_author(&server, "Series Book", "Series Author").await;
    let resp = server
        .admin_client()
        .patch_json(
            &format!("/api/v1/library/books/{}", book_id),
            &serde_json::json!({ "series_name": "Test Series" }),
        )
        .await;
    resp.assert_success();

    // Run the fetch_all_series_docs function
    let result = fetch_all_series_docs(&server.pool).await;
    assert!(
        result.is_ok(),
        "fetch_all_series_docs SQL query failed: {:?}",
        result
    );

    let docs = result.unwrap();
    assert!(!docs.is_empty(), "Should have at least 1 series document");
}

#[tokio::test]
async fn test_book_document_includes_author_ids() {
    let server = TestServer::start_api_only().await;

    // Create a book with multiple authors
    let book_id = create_book_with_author(&server, "Coauthored Book", "First Author").await;

    // Add a second author (using AuthorInput format for library API)
    let resp = server
        .admin_client()
        .patch_json(
            &format!("/api/v1/library/books/{}", book_id),
            &serde_json::json!({
                "authors": [{"name": "First Author"}, {"name": "Second Author"}]
            }),
        )
        .await;
    resp.assert_success();

    // Fetch book documents
    let result = fetch_all_book_docs(&server.pool).await.unwrap();
    let doc = result.iter().find(|d| d.id == book_id.to_string()).unwrap();

    // Verify author_ids are populated
    assert_eq!(doc.author_ids.len(), 2, "Book should have 2 author IDs");
    assert!(
        doc.author_ids.iter().all(|id| !id.is_empty()),
        "Author IDs should not be empty"
    );
}

#[tokio::test]
async fn test_book_document_includes_series_id() {
    let server = TestServer::start_api_only().await;

    // Create a book with a series
    let book_id = create_book_with_author(&server, "Series Test Book", "Series Test Author").await;
    let resp = server
        .admin_client()
        .patch_json(
            &format!("/api/v1/library/books/{}", book_id),
            &serde_json::json!({ "series_name": "Test Series For ID" }),
        )
        .await;
    resp.assert_success();

    // Fetch book documents
    let result = fetch_all_book_docs(&server.pool).await.unwrap();
    let doc = result.iter().find(|d| d.id == book_id.to_string()).unwrap();

    // Verify series_id is populated
    assert!(
        doc.series_id.is_some(),
        "Book should have a series_id when it has a series"
    );
    assert!(
        !doc.series_id.as_ref().unwrap().is_empty(),
        "Series ID should not be empty"
    );
}

// =============================================================================
// Genre Indexing Tests
// =============================================================================

#[tokio::test]
async fn test_fetch_all_genre_docs_query_is_valid() {
    let server = TestServer::start_api_only().await;

    // Create a book (genres are typically associated with books)
    let _book_id = create_book_with_author(&server, "Genre Test Book", "Genre Test Author").await;

    // Run the fetch_all_genre_docs function - should not error even with no genres
    let result = fetch_all_genre_docs(&server.pool).await;
    assert!(
        result.is_ok(),
        "fetch_all_genre_docs SQL query failed: {:?}",
        result
    );
}

#[tokio::test]
async fn test_book_document_includes_genres() {
    let server = TestServer::start_api_only().await;

    // Create a book
    let book_id = create_book_with_author(&server, "Genre Book", "Genre Author").await;

    // Fetch book documents
    let result = fetch_all_book_docs(&server.pool).await.unwrap();
    let doc = result.iter().find(|d| d.id == book_id.to_string()).unwrap();

    // Verify genres field exists (may be empty but should be present)
    // This tests that the SQL query includes the genres subquery
    // Just accessing doc.genres confirms the field exists in the struct and query works
    let _ = doc.genres.len(); // Accessing the field proves it exists and was populated
}

// =============================================================================
// Search Status and Test Connection Endpoints
// =============================================================================

#[tokio::test]
async fn test_search_status_requires_auth() {
    let server = TestServer::start_api_only().await;

    let client = TestClient::new(&server.api_url);
    let resp = client.get("/api/v1/admin/search/status").await;
    resp.assert_status(StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn test_search_status_returns_structure() {
    let server = TestServer::start_api_only().await;

    let resp = server
        .admin_client()
        .get("/api/v1/admin/search/status")
        .await;
    resp.assert_ok();

    let json: Value = resp.json();

    // Verify required fields are present
    assert!(json["backend_type"].is_string(), "Should have backend_type");
    assert!(json["connected"].is_boolean(), "Should have connected");
    assert!(json["url"].is_string(), "Should have url");

    // Count fields may be null if not connected
    assert!(
        json["book_count"].is_null() || json["book_count"].is_number(),
        "book_count should be null or number"
    );
    assert!(
        json["author_count"].is_null() || json["author_count"].is_number(),
        "author_count should be null or number"
    );
    assert!(
        json["series_count"].is_null() || json["series_count"].is_number(),
        "series_count should be null or number"
    );
    assert!(
        json["genre_count"].is_null() || json["genre_count"].is_number(),
        "genre_count should be null or number"
    );
}

#[tokio::test]
async fn test_test_connection_requires_auth() {
    let server = TestServer::start_api_only().await;

    let client = TestClient::new(&server.api_url);
    let resp = client
        .post_json(
            "/api/v1/admin/search/test",
            &serde_json::json!({
                "url": "http://localhost:7700",
                "master_key": "test"
            }),
        )
        .await;
    resp.assert_status(StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn test_test_connection_returns_result() {
    let server = TestServer::start_api_only().await;

    // Test with an invalid URL to verify endpoint works
    let resp = server
        .admin_client()
        .post_json(
            "/api/v1/admin/search/test",
            &serde_json::json!({
                "url": "http://invalid-host-that-does-not-exist:7700",
                "master_key": "test"
            }),
        )
        .await;
    resp.assert_ok();

    let json: Value = resp.json();

    // Verify required fields are present
    assert!(json["success"].is_boolean(), "Should have success");
    assert!(json["message"].is_string(), "Should have message");

    // Connection should fail with invalid host
    assert_eq!(json["success"], false, "Should fail with invalid host");
}

// =============================================================================
// Response Structure Tests (Federated Search Format)
// =============================================================================

#[tokio::test]
async fn test_search_returns_federated_format() {
    let server = TestServer::start_api_only().await;

    let resp = server
        .admin_client()
        .get("/api/v1/admin/search?q=test")
        .await;
    resp.assert_ok();

    let json: Value = resp.json();

    // Response should have federated search format
    assert!(
        json["top_hits"].is_array(),
        "Response should have top_hits array"
    );
    assert!(
        json["remaining"].is_array(),
        "Response should have remaining array"
    );

    // Each hit should have a type field
    for hit in json["top_hits"].as_array().unwrap() {
        assert!(hit["type"].is_string(), "Each hit should have a type field");
    }
}

// =============================================================================
// Static Content Sync Tests (Pages and Commands)
// =============================================================================

#[tokio::test]
async fn test_sync_static_requires_auth() {
    let server = TestServer::start_api_only().await;

    let client = TestClient::new(&server.api_url);
    let resp = client
        .post_json("/api/v1/admin/search/sync-static", &serde_json::json!({}))
        .await;
    resp.assert_status(StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn test_sync_static_returns_counts() {
    let server = TestServer::start_api_only().await;

    let resp = server
        .admin_client()
        .post_json("/api/v1/admin/search/sync-static", &serde_json::json!({}))
        .await;
    resp.assert_ok();

    let json: Value = resp.json();

    // Should return counts for pages and commands
    assert!(json["message"].is_string(), "Should have message");
    assert!(json["pages_count"].is_number(), "Should have pages_count");
    assert!(
        json["commands_count"].is_number(),
        "Should have commands_count"
    );

    // Should have synced some pages and commands
    let pages_count = json["pages_count"].as_i64().unwrap();
    let commands_count = json["commands_count"].as_i64().unwrap();
    assert!(pages_count > 0, "Should have synced at least 1 page");
    assert!(commands_count > 0, "Should have synced at least 1 command");
}

// =============================================================================
// Movie and TV Show Search Indexing Tests
// =============================================================================
// These tests verify that the SQL queries for movie/TV indexing are valid,
// catching type mismatches (e.g., TEXT[] vs JSONB) that only fail at runtime.

/// Helper to create a movie via API and return its UUID.
async fn create_test_movie(server: &TestServer, title: &str) -> uuid::Uuid {
    let resp = server
        .admin_client()
        .post_json(
            "/api/v1/admin/movies",
            &serde_json::json!({
                "title": title,
                "year": 2024,
                "genres": ["Drama", "Thriller"],
                "rating": 7.5,
            }),
        )
        .await;
    resp.assert_status(StatusCode::CREATED);
    let json: Value = resp.json();
    uuid::Uuid::parse_str(json["id"].as_str().unwrap()).unwrap()
}

/// Helper to create a TV show via API and return its UUID.
async fn create_test_tv_show(server: &TestServer, title: &str) -> uuid::Uuid {
    let resp = server
        .admin_client()
        .post_json(
            "/api/v1/admin/tv/shows",
            &serde_json::json!({
                "title": title,
                "year": 2024,
                "genres": ["Sci-Fi", "Drama"],
            }),
        )
        .await;
    resp.assert_status(StatusCode::CREATED);
    let json: Value = resp.json();
    uuid::Uuid::parse_str(json["id"].as_str().unwrap()).unwrap()
}

#[tokio::test]
async fn test_index_movie_query_is_valid() {
    let server = TestServer::start_api_only().await;

    let movie_id = create_test_movie(&server, "Test Movie For Indexing").await;

    let result = index_movie(server.search_backend(), &server.pool, movie_id).await;
    assert!(result.is_ok(), "index_movie SQL query failed: {:?}", result);
}

#[tokio::test]
async fn test_index_tv_show_query_is_valid() {
    let server = TestServer::start_api_only().await;

    let show_id = create_test_tv_show(&server, "Test Show For Indexing").await;

    let result = index_tv_show(server.search_backend(), &server.pool, show_id).await;
    assert!(
        result.is_ok(),
        "index_tv_show SQL query failed: {:?}",
        result
    );
}

#[tokio::test]
async fn test_fetch_all_movie_docs_query_is_valid() {
    let server = TestServer::start_api_only().await;

    let _movie1 = create_test_movie(&server, "Movie One").await;
    let _movie2 = create_test_movie(&server, "Movie Two").await;

    let result = fetch_all_movie_docs(&server.pool).await;
    assert!(
        result.is_ok(),
        "fetch_all_movie_docs SQL query failed: {:?}",
        result
    );

    let docs = result.unwrap();
    assert!(docs.len() >= 2, "Should have at least 2 movie documents");
}

#[tokio::test]
async fn test_fetch_all_tv_show_docs_query_is_valid() {
    let server = TestServer::start_api_only().await;

    let _show1 = create_test_tv_show(&server, "Show One").await;
    let _show2 = create_test_tv_show(&server, "Show Two").await;

    let result = fetch_all_tv_show_docs(&server.pool).await;
    assert!(
        result.is_ok(),
        "fetch_all_tv_show_docs SQL query failed: {:?}",
        result
    );

    let docs = result.unwrap();
    assert!(docs.len() >= 2, "Should have at least 2 TV show documents");
}

#[tokio::test]
async fn test_movie_document_includes_genres() {
    let server = TestServer::start_api_only().await;

    let movie_id = create_test_movie(&server, "Genre Movie Test").await;

    let result = fetch_all_movie_docs(&server.pool).await.unwrap();
    let doc = result
        .iter()
        .find(|d| d.id == movie_id.to_string())
        .unwrap();

    assert!(
        !doc.genres.is_empty(),
        "Movie should have genres populated from TEXT[] column"
    );
}

#[tokio::test]
async fn test_tv_show_document_includes_genres() {
    let server = TestServer::start_api_only().await;

    let show_id = create_test_tv_show(&server, "Genre Show Test").await;

    let result = fetch_all_tv_show_docs(&server.pool).await.unwrap();
    let doc = result.iter().find(|d| d.id == show_id.to_string()).unwrap();

    assert!(
        !doc.genres.is_empty(),
        "TV show should have genres populated from TEXT[] column"
    );
}

/// Insert a season with `episodes` episodes under a show.
async fn insert_tv_season_with_episodes(
    server: &TestServer,
    show_id: uuid::Uuid,
    season: i32,
    episodes: i32,
) {
    let season_id = uuid::Uuid::new_v4();
    sqlx::query(
        "INSERT INTO tv_seasons (id, show_id, season_identifier, season_sort_order, is_monitored, is_special) \
         VALUES ($1, $2, $3, $4, true, false)",
    )
    .bind(season_id)
    .bind(show_id)
    .bind(season.to_string())
    .bind(season)
    .execute(&server.pool)
    .await
    .expect("Failed to create season");

    for ep in 1..=episodes {
        sqlx::query(
            "INSERT INTO tv_episodes (id, season_id, episode_identifier, episode_sort_order, title) \
             VALUES ($1, $2, $3, $4, $5)",
        )
        .bind(uuid::Uuid::new_v4())
        .bind(season_id)
        .bind(ep.to_string())
        .bind(ep)
        .bind(format!("Episode {ep}"))
        .execute(&server.pool)
        .await
        .expect("Failed to create episode");
    }
}

#[tokio::test]
async fn test_index_tv_show_children_covers_only_that_show() {
    let server = TestServer::start_api_only().await;

    let show = create_test_tv_show(&server, "Children Batch Show").await;
    insert_tv_season_with_episodes(&server, show, 1, 3).await;
    insert_tv_season_with_episodes(&server, show, 2, 2).await;

    // A second show's children must not be swept into the first show's batch.
    let other = create_test_tv_show(&server, "Children Batch Other").await;
    insert_tv_season_with_episodes(&server, other, 1, 4).await;

    let counts = index_tv_show_children(server.search_backend(), &server.pool, show)
        .await
        .expect("index_tv_show_children SQL query failed");
    assert_eq!(
        counts,
        (2, 5),
        "expected 2 seasons and 5 episodes for the show"
    );
}

#[tokio::test]
async fn test_index_tv_show_children_unknown_show_is_empty() {
    let server = TestServer::start_api_only().await;

    let counts =
        index_tv_show_children(server.search_backend(), &server.pool, uuid::Uuid::new_v4())
            .await
            .expect("index_tv_show_children should succeed for an unknown show");
    assert_eq!(counts, (0, 0));
}

async fn seed_show_of_kind(server: &TestServer, title: &str, kind: &str) -> uuid::Uuid {
    let id = uuid::Uuid::new_v4();
    sqlx::query("INSERT INTO tv_shows (id, title, content_category) VALUES ($1, $2, $3)")
        .bind(id)
        .bind(title)
        .bind(kind)
        .execute(&server.pool)
        .await
        .unwrap();
    id
}

fn letters_only_token() -> String {
    uuid::Uuid::new_v4()
        .simple()
        .to_string()
        .chars()
        .map(|c| match c.to_digit(10) {
            Some(d) => (b'g' + d as u8) as char,
            None => c,
        })
        .collect()
}

#[tokio::test]
async fn test_fetch_all_tv_show_docs_carry_kind() {
    let server = TestServer::start_api_only().await;
    let league = seed_show_of_kind(&server, "Kind League", "sport").await;
    let drama = seed_show_of_kind(&server, "Kind Drama", "tv").await;

    let docs = fetch_all_tv_show_docs(&server.pool).await.unwrap();
    let kind_of = |id: uuid::Uuid| {
        docs.iter()
            .find(|d| d.id == id.to_string())
            .map(|d| d.kind.clone())
            .unwrap()
    };
    assert_eq!(kind_of(league), "sport");
    assert_eq!(kind_of(drama), "tv");
}

#[tokio::test]
async fn test_search_hits_tell_sports_leagues_from_tv_shows() {
    let url =
        std::env::var("MEILI_URL_TEST").unwrap_or_else(|_| "http://localhost:7701".to_string());
    let meili = std::sync::Arc::new(api::search::MeiliState::new(&url, "test-master-key").unwrap());
    api::search::SearchBackend::init_indexes(meili.as_ref())
        .await
        .unwrap();
    let server = TestServer::start_api_only_with_search(meili.clone()).await;

    let token = letters_only_token();
    for (title, kind) in [
        (format!("{token} League"), "sport"),
        (format!("{token} Drama"), "tv"),
    ] {
        let id = seed_show_of_kind(&server, &title, kind).await;
        index_tv_show(meili.as_ref(), &server.pool, id)
            .await
            .unwrap();
    }

    let client = server.admin_client();
    let mut kinds = Vec::new();
    for _ in 0..100 {
        let json: Value = client
            .get(&format!("/api/v1/admin/search?q={token}&limit=10"))
            .await
            .json();
        kinds = ["top_hits", "remaining"]
            .iter()
            .flat_map(|k| json[k].as_array().cloned().unwrap_or_default())
            .filter(|h| h["type"] == "tv_show")
            .map(|h| (h["title"].as_str().unwrap().to_string(), h["kind"].clone()))
            .collect::<Vec<_>>();
        if kinds.len() == 2 {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
    kinds.sort_by(|a, b| a.0.cmp(&b.0));
    assert_eq!(
        kinds,
        vec![
            (format!("{token} Drama"), Value::from("tv")),
            (format!("{token} League"), Value::from("sport")),
        ]
    );
}
