use crate::common::*;
use reqwest::StatusCode;
use serde_json::{Value, json};
use tempfile::TempDir;

/// Helper: create a media root and return its id.
async fn create_book_root(server: &TestServer) -> (String, TempDir) {
    let root_dir = TempDir::new().unwrap();
    let resp = server
        .admin_client()
        .post_json(
            "/api/v1/admin/media-roots",
            &json!({
                "path": root_dir.path().to_string_lossy(),
                "name": "Test Books",
                "media_type": "book",
                "auto_add": false,
            }),
        )
        .await;
    resp.assert_status(StatusCode::CREATED);
    let id = resp.json::<Value>()["id"].as_str().unwrap().to_string();
    (id, root_dir)
}

// =============================================================================
// Orphaned Books Filter Tests
// =============================================================================

#[tokio::test]
async fn test_orphaned_filter_returns_only_orphaned_books() {
    let server = TestServer::start_api_only().await;
    let (root_id, _root_dir) = create_book_root(&server).await;

    // Create two books — one orphaned, one with a media root
    let orphan_id = create_book(&server, "Orphan Book").await;
    let rooted_id = create_book(&server, "Rooted Book").await;

    // Set media_root_id on one book directly
    sqlx::query("UPDATE books SET media_root_id = $1 WHERE id = $2")
        .bind(uuid::Uuid::parse_str(&root_id).unwrap())
        .bind(rooted_id)
        .execute(&server.pool)
        .await
        .unwrap();

    // Filter orphaned=true should return only the orphan
    let resp = server
        .admin_client()
        .get("/api/v1/library/books?orphaned=true")
        .await;
    resp.assert_ok();
    let json: Value = resp.json();
    let items = json["items"].as_array().unwrap();
    assert_eq!(items.len(), 1);
    assert_eq!(items[0]["id"].as_str().unwrap(), orphan_id.to_string());
    assert!(items[0]["media_root_id"].is_null());

    // Filter orphaned=false should return only the rooted book
    let resp = server
        .admin_client()
        .get("/api/v1/library/books?orphaned=false")
        .await;
    resp.assert_ok();
    let json: Value = resp.json();
    let items = json["items"].as_array().unwrap();
    assert_eq!(items.len(), 1);
    assert_eq!(items[0]["id"].as_str().unwrap(), rooted_id.to_string());
    assert_eq!(items[0]["media_root_id"].as_str().unwrap(), root_id);
}

#[tokio::test]
async fn test_orphaned_filter_no_filter_returns_all() {
    let server = TestServer::start_api_only().await;
    let (root_id, _root_dir) = create_book_root(&server).await;

    let _orphan_id = create_book(&server, "Orphan Book").await;
    let rooted_id = create_book(&server, "Rooted Book").await;

    sqlx::query("UPDATE books SET media_root_id = $1 WHERE id = $2")
        .bind(uuid::Uuid::parse_str(&root_id).unwrap())
        .bind(rooted_id)
        .execute(&server.pool)
        .await
        .unwrap();

    // No orphaned filter — should return all books
    let resp = server.admin_client().get("/api/v1/library/books").await;
    resp.assert_ok();
    let json: Value = resp.json();
    let items = json["items"].as_array().unwrap();
    assert_eq!(items.len(), 2);
}

#[tokio::test]
async fn test_orphaned_filter_count_matches() {
    let server = TestServer::start_api_only().await;
    let (root_id, _root_dir) = create_book_root(&server).await;

    create_book(&server, "Orphan 1").await;
    create_book(&server, "Orphan 2").await;
    let rooted_id = create_book(&server, "Rooted Book").await;

    sqlx::query("UPDATE books SET media_root_id = $1 WHERE id = $2")
        .bind(uuid::Uuid::parse_str(&root_id).unwrap())
        .bind(rooted_id)
        .execute(&server.pool)
        .await
        .unwrap();

    let resp = server
        .admin_client()
        .get("/api/v1/library/books?orphaned=true")
        .await;
    resp.assert_ok();
    let json: Value = resp.json();
    assert_eq!(json["total_count"].as_i64().unwrap(), 2);
}

// =============================================================================
// Move to Media Root Tests
// =============================================================================

#[tokio::test]
async fn test_move_book_no_files_sets_media_root() {
    let server = TestServer::start_api_only().await;
    let (root_id, _root_dir) = create_book_root(&server).await;
    let book_id = create_book(&server, "Fileless Book").await;

    let resp = server
        .admin_client()
        .post_json(
            &format!("/api/v1/admin/books/{}/move-to-media-root", book_id),
            &json!({ "media_root_id": root_id }),
        )
        .await;
    resp.assert_ok();

    let json: Value = resp.json();
    assert_eq!(json["moved_files"].as_i64().unwrap(), 0);
    assert_eq!(json["skipped"].as_i64().unwrap(), 0);
    assert!(json["errors"].as_array().unwrap().is_empty());

    // Verify media_root_id was set
    let row =
        sqlx::query_as::<_, (Option<uuid::Uuid>,)>("SELECT media_root_id FROM books WHERE id = $1")
            .bind(book_id)
            .fetch_one(&server.pool)
            .await
            .unwrap();
    assert_eq!(row.0.unwrap().to_string(), root_id);
}

#[tokio::test]
async fn test_move_book_with_file() {
    let server = TestServer::start_api_only().await;
    let (root_id, root_dir) = create_book_root(&server).await;
    let book_id = create_book(&server, "Moving Book").await;

    // Create a temp source directory and file
    let source_dir = TempDir::new().unwrap();
    let source_path = source_dir.path().join("test.epub");
    std::fs::write(&source_path, b"fake epub content").unwrap();

    // Insert a book_file record
    let file_id = uuid::Uuid::new_v4();
    sqlx::query(
        "INSERT INTO book_files (id, book_id, file_path, format, size_bytes, content_revision) VALUES ($1, $2, $3, 'EPUB', 100, gen_random_uuid())",
    )
    .bind(file_id)
    .bind(book_id)
    .bind(source_path.to_string_lossy().as_ref())
    .execute(&server.pool)
    .await
    .unwrap();

    let resp = server
        .admin_client()
        .post_json(
            &format!("/api/v1/admin/books/{}/move-to-media-root", book_id),
            &json!({ "media_root_id": root_id }),
        )
        .await;
    resp.assert_ok();

    let json: Value = resp.json();
    assert_eq!(json["moved_files"].as_i64().unwrap(), 1);
    assert!(json["errors"].as_array().unwrap().is_empty());

    // Source file should no longer exist
    assert!(!source_path.exists());

    // DB path should be updated to under the root dir
    let row = sqlx::query_as::<_, (String,)>("SELECT file_path FROM book_files WHERE id = $1")
        .bind(file_id)
        .fetch_one(&server.pool)
        .await
        .unwrap();
    assert!(
        row.0
            .starts_with(&root_dir.path().to_string_lossy().to_string())
    );

    // Target file should exist on disk
    assert!(
        std::path::Path::new(&row.0).exists(),
        "moved file should exist at target path"
    );

    // media_root_id should be set
    let row =
        sqlx::query_as::<_, (Option<uuid::Uuid>,)>("SELECT media_root_id FROM books WHERE id = $1")
            .bind(book_id)
            .fetch_one(&server.pool)
            .await
            .unwrap();
    assert_eq!(row.0.unwrap().to_string(), root_id);
}

#[tokio::test]
async fn test_move_book_cleans_up_empty_dirs() {
    let server = TestServer::start_api_only().await;
    let (root_id, _root_dir) = create_book_root(&server).await;
    let book_id = create_book(&server, "Dir Cleanup Book").await;

    // Create nested source dir: /tmp/xxx/author/title/book.epub
    let source_base = TempDir::new().unwrap();
    let nested_dir = source_base.path().join("author").join("title");
    std::fs::create_dir_all(&nested_dir).unwrap();
    let source_path = nested_dir.join("book.epub");
    std::fs::write(&source_path, b"fake epub").unwrap();

    let file_id = uuid::Uuid::new_v4();
    sqlx::query(
        "INSERT INTO book_files (id, book_id, file_path, format, size_bytes, content_revision) VALUES ($1, $2, $3, 'EPUB', 100, gen_random_uuid())",
    )
    .bind(file_id)
    .bind(book_id)
    .bind(source_path.to_string_lossy().as_ref())
    .execute(&server.pool)
    .await
    .unwrap();

    let resp = server
        .admin_client()
        .post_json(
            &format!("/api/v1/admin/books/{}/move-to-media-root", book_id),
            &json!({ "media_root_id": root_id }),
        )
        .await;
    resp.assert_ok();

    // The nested empty dirs should be cleaned up
    assert!(!nested_dir.exists(), "nested dir should be removed");
    assert!(
        !source_base.path().join("author").exists(),
        "author dir should be removed"
    );
}

#[tokio::test]
async fn test_move_book_media_root_not_found() {
    let server = TestServer::start_api_only().await;
    let book_id = create_book(&server, "Test Book").await;

    let resp = server
        .admin_client()
        .post_json(
            &format!("/api/v1/admin/books/{}/move-to-media-root", book_id),
            &json!({ "media_root_id": uuid::Uuid::new_v4() }),
        )
        .await;
    resp.assert_status(StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn test_move_book_incompatible_media_type() {
    let server = TestServer::start_api_only().await;
    let book_id = create_book(&server, "Test Book").await;

    // Create a movie media root
    let root_dir = TempDir::new().unwrap();
    let resp = server
        .admin_client()
        .post_json(
            "/api/v1/admin/media-roots",
            &json!({
                "path": root_dir.path().to_string_lossy(),
                "name": "Movies",
                "media_type": "movie",
                "auto_add": false,
            }),
        )
        .await;
    resp.assert_status(StatusCode::CREATED);
    let root_id = resp.json::<Value>()["id"].as_str().unwrap().to_string();

    let resp = server
        .admin_client()
        .post_json(
            &format!("/api/v1/admin/books/{}/move-to-media-root", book_id),
            &json!({ "media_root_id": root_id }),
        )
        .await;
    resp.assert_status(StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn test_move_book_not_found() {
    let server = TestServer::start_api_only().await;
    let (root_id, _root_dir) = create_book_root(&server).await;

    let resp = server
        .admin_client()
        .post_json(
            &format!(
                "/api/v1/admin/books/{}/move-to-media-root",
                uuid::Uuid::new_v4()
            ),
            &json!({ "media_root_id": root_id }),
        )
        .await;
    resp.assert_status(StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn test_move_book_requires_auth() {
    let server = TestServer::start_api_only().await;

    let resp = server
        .api_client()
        .post_json(
            &format!(
                "/api/v1/admin/books/{}/move-to-media-root",
                uuid::Uuid::new_v4()
            ),
            &json!({ "media_root_id": uuid::Uuid::new_v4() }),
        )
        .await;
    resp.assert_status(StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn test_move_book_source_missing_reports_error() {
    let server = TestServer::start_api_only().await;
    let (root_id, _root_dir) = create_book_root(&server).await;
    let book_id = create_book(&server, "Missing File Book").await;

    // Insert a book_file with a nonexistent path
    let file_id = uuid::Uuid::new_v4();
    sqlx::query(
        "INSERT INTO book_files (id, book_id, file_path, format, size_bytes, content_revision) VALUES ($1, $2, '/nonexistent/path/book.epub', 'EPUB', 100, gen_random_uuid())",
    )
    .bind(file_id)
    .bind(book_id)
    .execute(&server.pool)
    .await
    .unwrap();

    let resp = server
        .admin_client()
        .post_json(
            &format!("/api/v1/admin/books/{}/move-to-media-root", book_id),
            &json!({ "media_root_id": root_id }),
        )
        .await;
    resp.assert_ok();

    let json: Value = resp.json();
    assert_eq!(json["moved_files"].as_i64().unwrap(), 0);
    let errors = json["errors"].as_array().unwrap();
    assert!(!errors.is_empty());
    assert!(errors[0].as_str().unwrap().contains("Source file missing"));
}

#[tokio::test]
async fn test_move_book_cleans_old_media_root_files_entry() {
    let server = TestServer::start_api_only().await;
    let (root_id, root_dir) = create_book_root(&server).await;
    let book_id = create_book(&server, "Cleanup MRF Book").await;
    let root_uuid = uuid::Uuid::parse_str(&root_id).unwrap();

    // Create source file
    let source_dir = TempDir::new().unwrap();
    let source_path = source_dir.path().join("old.epub");
    std::fs::write(&source_path, b"epub content").unwrap();
    let source_str = source_path.to_string_lossy().to_string();

    // Insert book_file
    let file_id = uuid::Uuid::new_v4();
    sqlx::query(
        "INSERT INTO book_files (id, book_id, file_path, format, size_bytes, content_revision) VALUES ($1, $2, $3, 'EPUB', 100, gen_random_uuid())",
    )
    .bind(file_id)
    .bind(book_id)
    .bind(&source_str)
    .execute(&server.pool)
    .await
    .unwrap();

    // Create an old media_root_files entry for the source path
    sqlx::query(
        "INSERT INTO media_root_files (id, media_root_id, file_path, file_name, match_status, entity_type, entity_id) VALUES ($1, $2, $3, 'old.epub', 'added', 'book', $4)",
    )
    .bind(uuid::Uuid::new_v4())
    .bind(root_uuid)
    .bind(&source_str)
    .bind(book_id)
    .execute(&server.pool)
    .await
    .unwrap();

    // Move the book
    let resp = server
        .admin_client()
        .post_json(
            &format!("/api/v1/admin/books/{}/move-to-media-root", book_id),
            &json!({ "media_root_id": root_id }),
        )
        .await;
    resp.assert_ok();

    // Old media_root_files entry should be deleted
    let count =
        sqlx::query_as::<_, (i64,)>("SELECT COUNT(*) FROM media_root_files WHERE file_path = $1")
            .bind(&source_str)
            .fetch_one(&server.pool)
            .await
            .unwrap();
    assert_eq!(count.0, 0, "old media_root_files entry should be deleted");

    // New media_root_files entry should exist under root_dir
    let count = sqlx::query_as::<_, (i64,)>(
        "SELECT COUNT(*) FROM media_root_files WHERE media_root_id = $1 AND file_path LIKE $2",
    )
    .bind(root_uuid)
    .bind(format!("{}%", root_dir.path().to_string_lossy()))
    .fetch_one(&server.pool)
    .await
    .unwrap();
    assert_eq!(count.0, 1, "new media_root_files entry should exist");
}
