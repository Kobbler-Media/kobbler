use crate::common::*;
use reqwest::StatusCode;
use serde_json::{Value, json};
use sqlx::Row;
use uuid::Uuid;

// =============================================================================
// Helpers
// =============================================================================

async fn create_series(server: &TestServer, name: &str) -> Uuid {
    let id = Uuid::now_v7();
    let sort_key = name.to_lowercase();
    sqlx::query("INSERT INTO series (id, name, sort_key, total_book_count) VALUES ($1, $2, $3, 0)")
        .bind(id)
        .bind(name)
        .bind(&sort_key)
        .execute(&server.pool)
        .await
        .unwrap();
    id
}

async fn create_series_with_count(server: &TestServer, name: &str, count: i32) -> Uuid {
    let id = Uuid::now_v7();
    let sort_key = name.to_lowercase();
    sqlx::query(
        "INSERT INTO series (id, name, sort_key, total_book_count) VALUES ($1, $2, $3, $4)",
    )
    .bind(id)
    .bind(name)
    .bind(&sort_key)
    .bind(count)
    .execute(&server.pool)
    .await
    .unwrap();
    id
}

async fn add_book_to_series(
    server: &TestServer,
    book_id: Uuid,
    series_id: Uuid,
    index: Option<f64>,
    is_primary: bool,
) {
    // Demote existing primary if setting new one
    if is_primary {
        sqlx::query(
            "UPDATE book_series SET is_primary = FALSE WHERE book_id = $1 AND is_primary = TRUE",
        )
        .bind(book_id)
        .execute(&server.pool)
        .await
        .unwrap();
    }
    sqlx::query(
        r#"
        INSERT INTO book_series (book_id, series_id, series_index, is_primary)
        VALUES ($1, $2, $3, $4)
        ON CONFLICT (book_id, series_id) DO UPDATE SET
            series_index = EXCLUDED.series_index,
            is_primary = EXCLUDED.is_primary
        "#,
    )
    .bind(book_id)
    .bind(series_id)
    .bind(index)
    .bind(is_primary)
    .execute(&server.pool)
    .await
    .unwrap();
}

async fn get_book_series_entries(
    server: &TestServer,
    book_id: Uuid,
) -> Vec<(Uuid, Option<f64>, bool)> {
    sqlx::query(
        "SELECT series_id, series_index, is_primary FROM book_series WHERE book_id = $1 ORDER BY is_primary DESC, series_index",
    )
    .bind(book_id)
    .fetch_all(&server.pool)
    .await
    .unwrap()
    .into_iter()
    .map(|r| (r.get("series_id"), r.get("series_index"), r.get("is_primary")))
    .collect()
}

// =============================================================================
// Multi-Series Membership Tests
// =============================================================================

#[tokio::test]
async fn test_book_in_multiple_series() {
    let server = TestServer::start_api_only().await;
    let book_id = create_book(&server, "Into the Wild").await;

    let warriors = create_series(&server, "Warriors").await;
    let prophecies = create_series(&server, "Warriors: The Prophecies Begin").await;

    add_book_to_series(&server, book_id, warriors, Some(1.0), true).await;
    add_book_to_series(&server, book_id, prophecies, Some(1.0), false).await;

    let entries = get_book_series_entries(&server, book_id).await;
    assert_eq!(entries.len(), 2, "Book should be in two series");
    assert!(entries[0].2, "First entry should be primary");
    assert!(!entries[1].2, "Second entry should not be primary");
}

#[tokio::test]
async fn test_book_detail_shows_primary_series() {
    let server = TestServer::start_api_only().await;
    let book_id = create_book(&server, "Into the Wild").await;

    let warriors = create_series(&server, "Warriors").await;
    let prophecies = create_series(&server, "Warriors: The Prophecies Begin").await;

    add_book_to_series(&server, book_id, warriors, Some(1.0), false).await;
    add_book_to_series(&server, book_id, prophecies, Some(1.0), true).await;

    let resp = server
        .admin_client()
        .get(&format!("/api/v1/library/books/{}", book_id))
        .await;
    resp.assert_ok();
    let json: Value = resp.json();

    // Primary series should be "Warriors: The Prophecies Begin"
    assert_eq!(
        json["series_name"].as_str().unwrap(),
        "Warriors: The Prophecies Begin"
    );
    assert_eq!(json["series_id"].as_str().unwrap(), prophecies.to_string());
}

#[tokio::test]
async fn test_book_appears_in_both_series_listings() {
    let server = TestServer::start_api_only().await;
    let book_id = create_book(&server, "Into the Wild").await;
    create_stub_book_file(&server.pool, book_id).await;

    let warriors = create_series(&server, "Warriors").await;
    let prophecies = create_series(&server, "Warriors: The Prophecies Begin").await;

    add_book_to_series(&server, book_id, warriors, Some(1.0), true).await;
    add_book_to_series(&server, book_id, prophecies, Some(1.0), false).await;

    // Book should appear in Warriors series
    let resp = server
        .admin_client()
        .get(&format!("/api/v1/library/series/{}/books", warriors))
        .await;
    resp.assert_ok();
    let json: Value = resp.json();
    let books = json.as_array().unwrap();
    assert_eq!(books.len(), 1);
    assert_eq!(books[0]["title"].as_str().unwrap(), "Into the Wild");

    // Book should also appear in Prophecies series
    let resp = server
        .admin_client()
        .get(&format!("/api/v1/library/series/{}/books", prophecies))
        .await;
    resp.assert_ok();
    let json: Value = resp.json();
    let books = json.as_array().unwrap();
    assert_eq!(books.len(), 1);
    assert_eq!(books[0]["title"].as_str().unwrap(), "Into the Wild");
}

// =============================================================================
// Per-Series Index Ordering
// =============================================================================

#[tokio::test]
async fn test_list_by_series_uses_per_series_index() {
    let server = TestServer::start_api_only().await;

    let series_a = create_series(&server, "Series A").await;
    let series_b = create_series(&server, "Series B").await;

    let book1 = create_book(&server, "Book One").await;
    create_stub_book_file(&server.pool, book1).await;
    let book2 = create_book(&server, "Book Two").await;
    create_stub_book_file(&server.pool, book2).await;

    // In Series A: Book One is #1, Book Two is #2
    add_book_to_series(&server, book1, series_a, Some(1.0), true).await;
    add_book_to_series(&server, book2, series_a, Some(2.0), true).await;

    // In Series B: Book Two is #1, Book One is #3 (reversed/different numbering)
    add_book_to_series(&server, book2, series_b, Some(1.0), false).await;
    add_book_to_series(&server, book1, series_b, Some(3.0), false).await;

    // Series A should order: Book One (1), Book Two (2)
    let resp = server
        .admin_client()
        .get(&format!("/api/v1/library/series/{}/books", series_a))
        .await;
    resp.assert_ok();
    let books: Vec<Value> = resp.json();
    assert_eq!(books[0]["title"].as_str().unwrap(), "Book One");
    assert_eq!(books[1]["title"].as_str().unwrap(), "Book Two");

    // Series B should order: Book Two (1), Book One (3)
    let resp = server
        .admin_client()
        .get(&format!("/api/v1/library/series/{}/books", series_b))
        .await;
    resp.assert_ok();
    let books: Vec<Value> = resp.json();
    assert_eq!(books[0]["title"].as_str().unwrap(), "Book Two");
    assert_eq!(books[1]["title"].as_str().unwrap(), "Book One");
}

// =============================================================================
// Auto-Select Primary (Most Specific Series)
// =============================================================================

#[tokio::test]
async fn test_auto_select_primary_picks_most_specific() {
    let server = TestServer::start_api_only().await;
    let book_id = create_book(&server, "Into the Wild").await;

    // Warriors has 80 books, Prophecies has 6
    let warriors = create_series_with_count(&server, "Warriors", 80).await;
    let prophecies = create_series_with_count(&server, "Warriors: The Prophecies Begin", 6).await;

    // Add both without setting primary
    sqlx::query(
        "INSERT INTO book_series (book_id, series_id, series_index, is_primary) VALUES ($1, $2, 1.0, FALSE)",
    )
    .bind(book_id)
    .bind(warriors)
    .execute(&server.pool)
    .await
    .unwrap();

    sqlx::query(
        "INSERT INTO book_series (book_id, series_id, series_index, is_primary) VALUES ($1, $2, 1.0, FALSE)",
    )
    .bind(book_id)
    .bind(prophecies)
    .execute(&server.pool)
    .await
    .unwrap();

    // Call auto_select_primary
    api::repositories::book_series::auto_select_primary(&server.pool, book_id, false)
        .await
        .unwrap();

    let entries = get_book_series_entries(&server, book_id).await;
    let primary = entries.iter().find(|e| e.2).unwrap();
    assert_eq!(
        primary.0, prophecies,
        "Primary should be the more specific series (lower total_book_count)"
    );
}

#[tokio::test]
async fn test_auto_select_primary_noop_if_primary_exists() {
    let server = TestServer::start_api_only().await;
    let book_id = create_book(&server, "Test Book").await;

    let series_a = create_series_with_count(&server, "Series A", 100).await;
    let series_b = create_series_with_count(&server, "Series B", 5).await;

    // Set A as primary even though B is more specific
    add_book_to_series(&server, book_id, series_a, Some(1.0), true).await;
    add_book_to_series(&server, book_id, series_b, Some(1.0), false).await;

    // auto_select with force=false should NOT change primary
    api::repositories::book_series::auto_select_primary(&server.pool, book_id, false)
        .await
        .unwrap();

    let entries = get_book_series_entries(&server, book_id).await;
    let primary = entries.iter().find(|e| e.2).unwrap();
    assert_eq!(primary.0, series_a, "Primary should remain Series A");
}

#[tokio::test]
async fn test_auto_select_primary_force_overrides() {
    let server = TestServer::start_api_only().await;
    let book_id = create_book(&server, "Test Book").await;

    let series_a = create_series_with_count(&server, "Series A", 100).await;
    let series_b = create_series_with_count(&server, "Series B", 5).await;

    add_book_to_series(&server, book_id, series_a, Some(1.0), true).await;
    add_book_to_series(&server, book_id, series_b, Some(1.0), false).await;

    // force=true should re-select based on specificity
    api::repositories::book_series::auto_select_primary(&server.pool, book_id, true)
        .await
        .unwrap();

    let entries = get_book_series_entries(&server, book_id).await;
    let primary = entries.iter().find(|e| e.2).unwrap();
    assert_eq!(
        primary.0, series_b,
        "Primary should now be Series B (more specific)"
    );
}

// =============================================================================
// Set Primary Endpoint
// =============================================================================

#[tokio::test]
async fn test_set_primary_series_endpoint() {
    let server = TestServer::start_api_only().await;
    let book_id = create_book(&server, "Test Book").await;

    let series_a = create_series(&server, "Series A").await;
    let series_b = create_series(&server, "Series B").await;

    add_book_to_series(&server, book_id, series_a, Some(1.0), true).await;
    add_book_to_series(&server, book_id, series_b, Some(2.0), false).await;

    // Switch primary to series_b via API
    let resp = server
        .admin_client()
        .put_json(
            &format!("/api/v1/library/books/{}/series-primary", book_id),
            &json!({ "series_id": series_b.to_string() }),
        )
        .await;
    resp.assert_status(StatusCode::NO_CONTENT);

    // Verify primary changed
    let entries = get_book_series_entries(&server, book_id).await;
    let primary = entries.iter().find(|e| e.2).unwrap();
    assert_eq!(primary.0, series_b);
}

// =============================================================================
// Series Merge with Conflicts
// =============================================================================

#[tokio::test]
async fn test_merge_series_moves_books() {
    let server = TestServer::start_api_only().await;

    let source = create_series(&server, "Source Series").await;
    let target = create_series(&server, "Target Series").await;

    let book1 = create_book(&server, "Book 1").await;
    create_stub_book_file(&server.pool, book1).await;
    let book2 = create_book(&server, "Book 2").await;
    create_stub_book_file(&server.pool, book2).await;

    add_book_to_series(&server, book1, source, Some(1.0), true).await;
    add_book_to_series(&server, book2, source, Some(2.0), true).await;

    // Merge source into target
    let resp = server
        .admin_client()
        .post_json(
            "/api/v1/library/series/merge",
            &json!({ "source_id": source.to_string(), "target_id": target.to_string() }),
        )
        .await;
    resp.assert_status(StatusCode::NO_CONTENT);

    // Both books should now be in target series
    let resp = server
        .admin_client()
        .get(&format!("/api/v1/library/series/{}/books", target))
        .await;
    resp.assert_ok();
    let books: Vec<Value> = resp.json();
    assert_eq!(books.len(), 2);

    // Source series should have no books
    let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM book_series WHERE series_id = $1")
        .bind(source)
        .fetch_one(&server.pool)
        .await
        .unwrap();
    assert_eq!(count, 0);
}

#[tokio::test]
async fn test_merge_series_handles_conflicts() {
    let server = TestServer::start_api_only().await;

    let source = create_series(&server, "Source Series").await;
    let target = create_series(&server, "Target Series").await;

    let book_in_both = create_book(&server, "Shared Book").await;
    create_stub_book_file(&server.pool, book_in_both).await;
    let book_source_only = create_book(&server, "Source Only").await;
    create_stub_book_file(&server.pool, book_source_only).await;

    // Book in both series
    add_book_to_series(&server, book_in_both, source, Some(1.0), true).await;
    add_book_to_series(&server, book_in_both, target, Some(5.0), false).await;
    // Book only in source
    add_book_to_series(&server, book_source_only, source, Some(2.0), true).await;

    // Merge source into target
    let resp = server
        .admin_client()
        .post_json(
            "/api/v1/library/series/merge",
            &json!({ "source_id": source.to_string(), "target_id": target.to_string() }),
        )
        .await;
    resp.assert_status(StatusCode::NO_CONTENT);

    // Target should have both books
    let resp = server
        .admin_client()
        .get(&format!("/api/v1/library/series/{}/books", target))
        .await;
    resp.assert_ok();
    let books: Vec<Value> = resp.json();
    assert_eq!(books.len(), 2);

    // Shared book should keep its target series_index (5.0), not source (1.0)
    let shared_entry: (Option<f64>,) = sqlx::query_as(
        "SELECT series_index FROM book_series WHERE book_id = $1 AND series_id = $2",
    )
    .bind(book_in_both)
    .bind(target)
    .fetch_one(&server.pool)
    .await
    .unwrap();
    assert!(
        (shared_entry.0.unwrap() - 5.0).abs() < 0.001,
        "Shared book should keep target's index"
    );
}

// =============================================================================
// Orphan Detection
// =============================================================================

#[tokio::test]
async fn test_orphan_series_uses_junction_not_books_table() {
    let server = TestServer::start_api_only().await;

    let series_a = create_series(&server, "Has Junction Entry").await;
    let series_b = create_series(&server, "No Junction Entry").await;

    let book = create_book(&server, "Test Book").await;
    add_book_to_series(&server, book, series_a, Some(1.0), true).await;
    // series_b has no junction entries

    // Delete orphaned series
    let deleted = api::repositories::series::delete_orphaned(&server.pool)
        .await
        .unwrap();

    assert!(
        !deleted.contains(&series_a),
        "Series with junction entry should NOT be deleted"
    );
    assert!(
        deleted.contains(&series_b),
        "Series without junction entry should be deleted"
    );
}

// =============================================================================
// Wanted Books via Series
// =============================================================================

#[tokio::test]
async fn test_want_all_pending_uses_junction() {
    let server = TestServer::start_api_only().await;

    let series = create_series(&server, "Test Series").await;

    // Create two discovered books in the series
    let book1 = Uuid::now_v7();
    sqlx::query("INSERT INTO books (id, title, want_status) VALUES ($1, 'Book 1', 'pending')")
        .bind(book1)
        .execute(&server.pool)
        .await
        .unwrap();
    add_book_to_series(&server, book1, series, Some(1.0), true).await;

    let book2 = Uuid::now_v7();
    sqlx::query("INSERT INTO books (id, title, want_status) VALUES ($1, 'Book 2', 'pending')")
        .bind(book2)
        .execute(&server.pool)
        .await
        .unwrap();
    add_book_to_series(&server, book2, series, Some(2.0), true).await;

    // Want all pending
    let resp = server
        .admin_client()
        .post_json(
            &format!("/api/v1/library/series/{}/want-pending", series),
            &json!({}),
        )
        .await;
    resp.assert_ok();
    let json: Value = resp.json();
    assert_eq!(json["wanted_count"].as_i64().unwrap(), 2);
}

// =============================================================================
// Create Book with Series (end-to-end)
// =============================================================================

#[tokio::test]
async fn test_create_wanted_book_with_series_creates_junction_entry() {
    let server = TestServer::start_api_only().await;

    let resp = server
        .admin_client()
        .post_json(
            "/api/v1/admin/wanted",
            &json!({
                "title": "Test Book",
                "author": "Test Author",
                "series": "My Series",
                "series_index": 3.0
            }),
        )
        .await;
    resp.assert_ok();
    let json: Value = resp.json();
    let book_id: Uuid = json["id"].as_str().unwrap().parse().unwrap();

    // Verify junction entry exists
    let entries = get_book_series_entries(&server, book_id).await;
    assert_eq!(entries.len(), 1, "Should have one series entry");
    assert!(entries[0].2, "Entry should be primary");
    assert!(
        (entries[0].1.unwrap() - 3.0).abs() < 0.001,
        "Index should be 3.0"
    );
}

// =============================================================================
// Partial Unique Index Enforcement
// =============================================================================

#[tokio::test]
async fn test_only_one_primary_per_book() {
    let server = TestServer::start_api_only().await;
    let book_id = create_book(&server, "Test Book").await;

    let series_a = create_series(&server, "Series A").await;
    let series_b = create_series(&server, "Series B").await;

    // Add A as primary
    api::repositories::book_series::add(&server.pool, book_id, series_a, Some(1.0), true)
        .await
        .unwrap();
    // Add B as primary — should demote A
    api::repositories::book_series::add(&server.pool, book_id, series_b, Some(2.0), true)
        .await
        .unwrap();

    let entries = get_book_series_entries(&server, book_id).await;
    let primary_count = entries.iter().filter(|e| e.2).count();
    assert_eq!(primary_count, 1, "Should have exactly one primary");
    let primary = entries.iter().find(|e| e.2).unwrap();
    assert_eq!(primary.0, series_b, "B should be the primary");
}

// =============================================================================
// Remove Series Membership
// =============================================================================

#[tokio::test]
async fn test_remove_non_primary_membership() {
    let server = TestServer::start_api_only().await;
    let book_id = create_book(&server, "Test Book").await;

    let series_a = create_series(&server, "Series A").await;
    let series_b = create_series(&server, "Series B").await;

    add_book_to_series(&server, book_id, series_a, Some(1.0), true).await;
    add_book_to_series(&server, book_id, series_b, Some(1.0), false).await;

    // Remove non-primary
    api::repositories::book_series::remove(&server.pool, book_id, series_b)
        .await
        .unwrap();

    let entries = get_book_series_entries(&server, book_id).await;
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].0, series_a);
}

#[tokio::test]
async fn test_delete_series_cascades_junction_entries() {
    let server = TestServer::start_api_only().await;
    let book_id = create_book(&server, "Test Book").await;

    let series_a = create_series(&server, "Series A").await;
    let series_b = create_series(&server, "Series B").await;

    add_book_to_series(&server, book_id, series_a, Some(1.0), true).await;
    add_book_to_series(&server, book_id, series_b, Some(1.0), false).await;

    // Delete series A
    let resp = server
        .admin_client()
        .delete(&format!("/api/v1/library/series/{}", series_a))
        .await;
    resp.assert_status(StatusCode::NO_CONTENT);

    // Book should only be in series B now
    let entries = get_book_series_entries(&server, book_id).await;
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].0, series_b);
}
