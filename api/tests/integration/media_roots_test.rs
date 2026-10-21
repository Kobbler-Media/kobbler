use crate::common::*;
use api::repositories::{media_root_files, media_root_files::MediaRootFileInput};
use reqwest::StatusCode;
use serde_json::{Value, json};
use tempfile::TempDir;
use uuid::Uuid;

/// Helper: create a media root and return its id.
async fn create_root(server: &TestServer, media_type: &str) -> (String, TempDir) {
    create_root_with_opts(server, media_type, true).await
}

/// Helper: create a media root with explicit auto_add setting.
async fn create_root_with_opts(
    server: &TestServer,
    media_type: &str,
    auto_add: bool,
) -> (String, TempDir) {
    let root_dir = TempDir::new().unwrap();
    let resp = server
        .admin_client()
        .post_json(
            "/api/v1/admin/media-roots",
            &json!({
                "path": root_dir.path().to_string_lossy(),
                "name": format!("Test {}", media_type),
                "media_type": media_type,
                "auto_add": auto_add,
            }),
        )
        .await;
    resp.assert_status(StatusCode::CREATED);
    let id = resp.json::<Value>()["id"].as_str().unwrap().to_string();
    (id, root_dir)
}

// =============================================================================
// Media Root CRUD Tests
// =============================================================================

#[tokio::test]
async fn test_list_media_roots_empty() {
    let server = TestServer::start_api_only().await;

    let resp = server.admin_client().get("/api/v1/admin/media-roots").await;
    resp.assert_ok();

    let json: Vec<Value> = resp.json();
    assert!(json.is_empty());
}

#[tokio::test]
async fn test_create_media_root() {
    let server = TestServer::start_api_only().await;
    let root_dir = TempDir::new().unwrap();

    let resp = server
        .admin_client()
        .post_json(
            "/api/v1/admin/media-roots",
            &json!({
                "path": root_dir.path().to_string_lossy(),
                "name": "My TV Shows",
                "media_type": "tv",
                "auto_add": true
            }),
        )
        .await;
    resp.assert_status(StatusCode::CREATED);

    let json: Value = resp.json();
    assert_eq!(json["name"], "My TV Shows");
    assert_eq!(json["media_type"], "tv");
    assert_eq!(json["auto_add"], true);
    assert_eq!(json["enabled"], true);
    assert_eq!(json["scan_status"], "idle");
    assert_eq!(json["write_sidecars"], true);
    assert_eq!(json["write_images"], true);
    assert!(json["id"].as_str().is_some());
}

#[tokio::test]
async fn test_create_media_root_movie_selective() {
    let server = TestServer::start_api_only().await;
    let root_dir = TempDir::new().unwrap();

    let resp = server
        .admin_client()
        .post_json(
            "/api/v1/admin/media-roots",
            &json!({
                "path": root_dir.path().to_string_lossy(),
                "media_type": "movie",
                "auto_add": false
            }),
        )
        .await;
    resp.assert_status(StatusCode::CREATED);

    let json: Value = resp.json();
    assert_eq!(json["media_type"], "movie");
    assert_eq!(json["auto_add"], false);
}

#[tokio::test]
async fn test_create_media_root_with_write_flags() {
    let server = TestServer::start_api_only().await;
    let root_dir = TempDir::new().unwrap();

    let resp = server
        .admin_client()
        .post_json(
            "/api/v1/admin/media-roots",
            &json!({
                "path": root_dir.path().to_string_lossy(),
                "name": "TV With Metadata",
                "media_type": "tv",
                "write_sidecars": true,
                "write_images": true
            }),
        )
        .await;
    resp.assert_status(StatusCode::CREATED);

    let json: Value = resp.json();
    assert_eq!(json["write_sidecars"], true);
    assert_eq!(json["write_images"], true);

    // Verify persisted via GET
    let id = json["id"].as_str().unwrap();
    let resp = server
        .admin_client()
        .get(&format!("/api/v1/admin/media-roots/{}", id))
        .await;
    resp.assert_ok();
    let json: Value = resp.json();
    assert_eq!(json["write_sidecars"], true);
    assert_eq!(json["write_images"], true);
}

#[tokio::test]
async fn test_get_media_root() {
    let server = TestServer::start_api_only().await;
    let (id, _dir) = create_root(&server, "tv").await;

    let resp = server
        .admin_client()
        .get(&format!("/api/v1/admin/media-roots/{}", id))
        .await;
    resp.assert_ok();

    let json: Value = resp.json();
    assert_eq!(json["name"], "Test tv");
    assert!(json["stats"].is_object());
}

#[tokio::test]
async fn test_update_media_root() {
    let server = TestServer::start_api_only().await;
    let (id, _dir) = create_root(&server, "tv").await;

    let resp = server
        .admin_client()
        .put_json(
            &format!("/api/v1/admin/media-roots/{}", id),
            &json!({
                "name": "New Name",
                "auto_add": false
            }),
        )
        .await;
    resp.assert_ok();

    let json: Value = resp.json();
    assert_eq!(json["name"], "New Name");
    assert_eq!(json["auto_add"], false);
}

#[tokio::test]
async fn test_delete_media_root() {
    let server = TestServer::start_api_only().await;
    let (id, _dir) = create_root(&server, "tv").await;

    // Delete now enqueues a background job
    let resp = server
        .admin_client()
        .delete(&format!("/api/v1/admin/media-roots/{}", id))
        .await;
    resp.assert_ok();

    let json: Value = resp.json();
    assert!(json["job_id"].as_str().is_some(), "should return a job_id");
}

#[tokio::test]
async fn test_media_roots_requires_auth() {
    let server = TestServer::start_api_only().await;
    let client = TestClient::new(&server.api_url);

    client
        .get("/api/v1/admin/media-roots")
        .await
        .assert_status(StatusCode::UNAUTHORIZED);

    client
        .post_json(
            "/api/v1/admin/media-roots",
            &json!({"path": "/tmp", "media_type": "tv"}),
        )
        .await
        .assert_status(StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn test_trigger_scan() {
    let server = TestServer::start_api_only().await;
    let (id, _dir) = create_root(&server, "tv").await;

    let resp = server
        .admin_client()
        .post_json(
            &format!("/api/v1/admin/media-roots/{}/scan", id),
            &json!({}),
        )
        .await;
    resp.assert_ok();

    let json: Value = resp.json();
    assert!(json["job_id"].as_str().is_some());
}

#[tokio::test]
async fn test_list_files_empty() {
    let server = TestServer::start_api_only().await;
    let (id, _dir) = create_root(&server, "tv").await;

    let resp = server
        .admin_client()
        .get(&format!("/api/v1/admin/media-roots/{}/files", id))
        .await;
    resp.assert_ok();

    let json: Value = resp.json();
    assert_eq!(json["total_count"], 0);
    assert!(json["items"].as_array().unwrap().is_empty());
}

#[tokio::test]
async fn test_get_nonexistent_media_root() {
    let server = TestServer::start_api_only().await;

    let resp = server
        .admin_client()
        .get("/api/v1/admin/media-roots/00000000-0000-0000-0000-000000000000")
        .await;
    resp.assert_status(StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn test_trigger_reorganize() {
    let server = TestServer::start_api_only().await;
    let (id, _dir) = create_root(&server, "movie").await;

    let resp = server
        .admin_client()
        .post_json(
            &format!("/api/v1/admin/media-roots/{}/reorganize", id),
            &json!({"dry_run": true}),
        )
        .await;
    resp.assert_ok();

    let json: Value = resp.json();
    assert!(json["job_id"].as_str().is_some());
}

#[tokio::test]
async fn test_trigger_reorganize_include_unconfirmed() {
    let server = TestServer::start_api_only().await;
    let (id, _dir) = create_root(&server, "book").await;

    let resp = server
        .admin_client()
        .post_json(
            &format!("/api/v1/admin/media-roots/{}/reorganize", id),
            &json!({"dry_run": true, "include_unconfirmed": true}),
        )
        .await;
    resp.assert_ok();

    let json: Value = resp.json();
    assert!(json["job_id"].as_str().is_some());
}

// =============================================================================
// Book Media Root Tests
// =============================================================================

#[tokio::test]
async fn test_create_book_media_root() {
    let server = TestServer::start_api_only().await;
    let root_dir = TempDir::new().unwrap();

    let resp = server
        .admin_client()
        .post_json(
            "/api/v1/admin/media-roots",
            &json!({
                "path": root_dir.path().to_string_lossy(),
                "name": "My Books",
                "media_type": "book",
                "auto_add": true
            }),
        )
        .await;
    resp.assert_status(StatusCode::CREATED);

    let json: Value = resp.json();
    assert_eq!(json["name"], "My Books");
    assert_eq!(json["media_type"], "book");
    assert_eq!(json["auto_add"], true);
    assert_eq!(json["enabled"], true);
    assert_eq!(json["default_entitlement_scope"], "all_users");
}

#[tokio::test]
async fn test_create_book_media_root_custom_entitlement_scope() {
    let server = TestServer::start_api_only().await;
    let root_dir = TempDir::new().unwrap();

    let resp = server
        .admin_client()
        .post_json(
            "/api/v1/admin/media-roots",
            &json!({
                "path": root_dir.path().to_string_lossy(),
                "name": "Restricted Books",
                "media_type": "book",
                "default_entitlement_scope": "user"
            }),
        )
        .await;
    resp.assert_status(StatusCode::CREATED);

    let json: Value = resp.json();
    assert_eq!(json["default_entitlement_scope"], "user");
}

#[tokio::test]
async fn test_create_media_root_invalid_type() {
    let server = TestServer::start_api_only().await;
    let root_dir = TempDir::new().unwrap();

    let resp = server
        .admin_client()
        .post_json(
            "/api/v1/admin/media-roots",
            &json!({
                "path": root_dir.path().to_string_lossy(),
                "media_type": "podcast"
            }),
        )
        .await;
    resp.assert_status(StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn test_trigger_book_scan() {
    let server = TestServer::start_api_only().await;
    let (id, _dir) = create_root(&server, "book").await;

    let resp = server
        .admin_client()
        .post_json(
            &format!("/api/v1/admin/media-roots/{}/scan", id),
            &json!({}),
        )
        .await;
    resp.assert_ok();

    let json: Value = resp.json();
    assert!(json["job_id"].as_str().is_some());
}

// =============================================================================
// Default Media Root Tests
// =============================================================================

#[tokio::test]
async fn test_first_root_for_type_becomes_default() {
    let server = TestServer::start_api_only().await;
    let (id, _dir) = create_root(&server, "movie").await;

    let resp = server
        .admin_client()
        .get(&format!("/api/v1/admin/media-roots/{}", id))
        .await;
    resp.assert_ok();
    let json: Value = resp.json();
    assert_eq!(
        json["is_default"], true,
        "first root for type should auto-become default"
    );
}

#[tokio::test]
async fn test_second_root_same_type_not_default() {
    let server = TestServer::start_api_only().await;
    let (id1, _dir1) = create_root(&server, "tv").await;
    let (id2, _dir2) = create_root(&server, "tv").await;

    let resp1 = server
        .admin_client()
        .get(&format!("/api/v1/admin/media-roots/{}", id1))
        .await;
    resp1.assert_ok();
    assert_eq!(resp1.json::<Value>()["is_default"], true);

    let resp2 = server
        .admin_client()
        .get(&format!("/api/v1/admin/media-roots/{}", id2))
        .await;
    resp2.assert_ok();
    assert_eq!(
        resp2.json::<Value>()["is_default"],
        false,
        "second root for same type should NOT be default"
    );
}

#[tokio::test]
async fn test_first_root_per_type_each_default() {
    let server = TestServer::start_api_only().await;
    let (tv_id, _tv_dir) = create_root(&server, "tv").await;
    let (movie_id, _movie_dir) = create_root(&server, "movie").await;
    let (book_id, _book_dir) = create_root(&server, "book").await;

    // Each should be default for its type
    for (id, label) in [(&tv_id, "tv"), (&movie_id, "movie"), (&book_id, "book")] {
        let resp = server
            .admin_client()
            .get(&format!("/api/v1/admin/media-roots/{}", id))
            .await;
        resp.assert_ok();
        assert_eq!(
            resp.json::<Value>()["is_default"],
            true,
            "{} root should be default (first for its type)",
            label
        );
    }
}

#[tokio::test]
async fn test_set_default_endpoint() {
    let server = TestServer::start_api_only().await;
    let (id1, _dir1) = create_root(&server, "movie").await;
    let (id2, _dir2) = create_root(&server, "movie").await;

    // id1 is default initially
    let resp = server
        .admin_client()
        .get(&format!("/api/v1/admin/media-roots/{}", id1))
        .await;
    assert_eq!(resp.json::<Value>()["is_default"], true);

    // Set id2 as default
    let resp = server
        .admin_client()
        .post_json(
            &format!("/api/v1/admin/media-roots/{}/set-default", id2),
            &json!({}),
        )
        .await;
    resp.assert_ok();

    // Verify id2 is now default and id1 is not
    let resp1 = server
        .admin_client()
        .get(&format!("/api/v1/admin/media-roots/{}", id1))
        .await;
    assert_eq!(
        resp1.json::<Value>()["is_default"],
        false,
        "old default should be unset"
    );

    let resp2 = server
        .admin_client()
        .get(&format!("/api/v1/admin/media-roots/{}", id2))
        .await;
    assert_eq!(
        resp2.json::<Value>()["is_default"],
        true,
        "new default should be set"
    );
}

#[tokio::test]
async fn test_set_default_does_not_affect_other_types() {
    let server = TestServer::start_api_only().await;
    let (tv_id, _tv_dir) = create_root(&server, "tv").await;
    let (_movie_id1, _movie_dir1) = create_root(&server, "movie").await;
    let (movie_id2, _movie_dir2) = create_root(&server, "movie").await;

    // Set movie_id2 as default movie root
    server
        .admin_client()
        .post_json(
            &format!("/api/v1/admin/media-roots/{}/set-default", movie_id2),
            &json!({}),
        )
        .await
        .assert_ok();

    // TV default should be unaffected
    let resp = server
        .admin_client()
        .get(&format!("/api/v1/admin/media-roots/{}", tv_id))
        .await;
    assert_eq!(
        resp.json::<Value>()["is_default"],
        true,
        "TV default should be unaffected by movie default change"
    );
}

#[tokio::test]
async fn test_set_default_requires_auth() {
    let server = TestServer::start_api_only().await;
    let (id, _dir) = create_root(&server, "movie").await;

    let client = TestClient::new(&server.api_url);
    client
        .post_json(
            &format!("/api/v1/admin/media-roots/{}/set-default", id),
            &json!({}),
        )
        .await
        .assert_status(StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn test_set_default_nonexistent_root() {
    let server = TestServer::start_api_only().await;

    let resp = server
        .admin_client()
        .post_json(
            "/api/v1/admin/media-roots/00000000-0000-0000-0000-000000000000/set-default",
            &json!({}),
        )
        .await;
    resp.assert_status(StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn test_is_default_in_list_response() {
    let server = TestServer::start_api_only().await;
    let (_id1, _dir1) = create_root(&server, "movie").await;
    let (_id2, _dir2) = create_root(&server, "movie").await;

    let resp = server.admin_client().get("/api/v1/admin/media-roots").await;
    resp.assert_ok();

    let roots: Vec<Value> = resp.json();
    assert_eq!(roots.len(), 2);
    let defaults: Vec<&Value> = roots.iter().filter(|r| r["is_default"] == true).collect();
    assert_eq!(defaults.len(), 1, "exactly one default per type in list");
}

// =============================================================================
// Movie/TV Auto-assign Default Media Root Tests
// =============================================================================

#[tokio::test]
async fn test_movie_auto_assigns_default_media_root() {
    let server = TestServer::start_api_only().await;
    let (root_id, _dir) = create_root(&server, "movie").await;

    // Create movie without specifying media_root_id
    let resp = server
        .admin_client()
        .post_json(
            "/api/v1/admin/movies",
            &json!({
                "title": "Test Movie",
                "year": 2024
            }),
        )
        .await;
    resp.assert_status(StatusCode::CREATED);
    let movie: Value = resp.json();
    let movie_id = movie["id"].as_str().unwrap();

    // Get movie detail - should have media_root_id auto-assigned
    let resp = server
        .admin_client()
        .get(&format!("/api/v1/admin/movies/{}", movie_id))
        .await;
    resp.assert_ok();
    let detail: Value = resp.json();
    assert_eq!(
        detail["media_root_id"].as_str().unwrap(),
        root_id,
        "movie should auto-assign default media root"
    );
}

#[tokio::test]
async fn test_movie_explicit_media_root_overrides_default() {
    let server = TestServer::start_api_only().await;
    let (_root_id1, _dir1) = create_root(&server, "movie").await;
    let (root_id2, _dir2) = create_root(&server, "movie").await;

    // Create movie explicitly specifying the non-default root
    let resp = server
        .admin_client()
        .post_json(
            "/api/v1/admin/movies",
            &json!({
                "title": "Custom Root Movie",
                "year": 2024,
                "media_root_id": root_id2
            }),
        )
        .await;
    resp.assert_status(StatusCode::CREATED);
    let movie: Value = resp.json();
    let movie_id = movie["id"].as_str().unwrap();

    let resp = server
        .admin_client()
        .get(&format!("/api/v1/admin/movies/{}", movie_id))
        .await;
    resp.assert_ok();
    let detail: Value = resp.json();
    assert_eq!(
        detail["media_root_id"].as_str().unwrap(),
        root_id2,
        "explicit media_root_id should override default"
    );
}

#[tokio::test]
async fn test_movie_no_default_root_leaves_null() {
    let server = TestServer::start_api_only().await;

    // No media roots at all
    let resp = server
        .admin_client()
        .post_json(
            "/api/v1/admin/movies",
            &json!({
                "title": "No Root Movie",
                "year": 2024
            }),
        )
        .await;
    resp.assert_status(StatusCode::CREATED);
    let movie: Value = resp.json();
    let movie_id = movie["id"].as_str().unwrap();

    let resp = server
        .admin_client()
        .get(&format!("/api/v1/admin/movies/{}", movie_id))
        .await;
    resp.assert_ok();
    let detail: Value = resp.json();
    assert!(
        detail["media_root_id"].is_null(),
        "no default root means media_root_id stays null"
    );
}

#[tokio::test]
async fn test_tv_show_auto_assigns_default_media_root() {
    let server = TestServer::start_api_only().await;
    let (root_id, _dir) = create_root(&server, "tv").await;

    let resp = server
        .admin_client()
        .post_json(
            "/api/v1/admin/tv/shows",
            &json!({
                "title": "Test Show",
                "year": 2024
            }),
        )
        .await;
    resp.assert_status(StatusCode::CREATED);
    let show: Value = resp.json();
    let show_id = show["id"].as_str().unwrap();

    let resp = server
        .admin_client()
        .get(&format!("/api/v1/admin/tv/shows/{}", show_id))
        .await;
    resp.assert_ok();
    let detail: Value = resp.json();
    assert_eq!(
        detail["media_root_id"].as_str().unwrap(),
        root_id,
        "show should auto-assign default media root"
    );
}

#[tokio::test]
async fn test_movie_update_media_root_id() {
    let server = TestServer::start_api_only().await;
    let (_root_id1, _dir1) = create_root(&server, "movie").await;
    let (root_id2, _dir2) = create_root(&server, "movie").await;

    // Create movie (auto-assigns default)
    let resp = server
        .admin_client()
        .post_json(
            "/api/v1/admin/movies",
            &json!({ "title": "Movable Movie", "year": 2024 }),
        )
        .await;
    resp.assert_status(StatusCode::CREATED);
    let movie_id = resp.json::<Value>()["id"].as_str().unwrap().to_string();

    // Update to root_id2
    let resp = server
        .admin_client()
        .patch_json(
            &format!("/api/v1/admin/movies/{}", movie_id),
            &json!({ "media_root_id": root_id2 }),
        )
        .await;
    resp.assert_status(StatusCode::NO_CONTENT);

    let resp = server
        .admin_client()
        .get(&format!("/api/v1/admin/movies/{}", movie_id))
        .await;
    assert_eq!(
        resp.json::<Value>()["media_root_id"].as_str().unwrap(),
        root_id2
    );
}

// =============================================================================
// Bulk Resolve / Rematch Tests
// =============================================================================

/// Helper: insert a media root file in review status with a given candidate.
async fn insert_review_file(
    server: &TestServer,
    root_id: &str,
    file_path: &str,
    file_name: &str,
    candidates: &Value,
    interpretation: Option<&Value>,
) -> Uuid {
    let root_uuid: Uuid = root_id.parse().unwrap();
    let input = MediaRootFileInput {
        media_root_id: root_uuid,
        file_path: file_path.to_string(),
        file_name: file_name.to_string(),
        file_size: Some(1000),
        file_mtime: None,
        file_fingerprint: None,
    };
    let (file_id, _) = media_root_files::upsert_by_path(&server.pool, input)
        .await
        .unwrap();

    // Set interpretation if provided
    if let Some(interp) = interpretation {
        media_root_files::set_interpreted(&server.pool, file_id, interp)
            .await
            .unwrap();
    }

    // Set to review status with candidates
    media_root_files::set_review(&server.pool, file_id, candidates)
        .await
        .unwrap();

    file_id
}

#[tokio::test]
async fn test_bulk_resolve_empty_request() {
    let server = TestServer::start_api_only().await;

    let resp = server
        .admin_client()
        .post_json(
            "/api/v1/admin/media-roots/files/bulk-resolve",
            &json!({"file_ids": []}),
        )
        .await;
    resp.assert_status(StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn test_bulk_resolve_enqueues_job() {
    let server = TestServer::start_api_only().await;
    let (root_id, _dir) = create_root(&server, "tv").await;

    // Insert a file in pending status (not review)
    let root_uuid: Uuid = root_id.parse().unwrap();
    let input = MediaRootFileInput {
        media_root_id: root_uuid,
        file_path: "/tmp/show/S01E01.mkv".to_string(),
        file_name: "S01E01.mkv".to_string(),
        file_size: Some(1000),
        file_mtime: None,
        file_fingerprint: None,
    };
    let (file_id, _) = media_root_files::upsert_by_path(&server.pool, input)
        .await
        .unwrap();

    let resp = server
        .admin_client()
        .post_json(
            "/api/v1/admin/media-roots/files/bulk-resolve",
            &json!({"file_ids": [file_id.to_string()]}),
        )
        .await;
    resp.assert_status(StatusCode::ACCEPTED);

    let json: Value = resp.json();
    assert!(
        json["job_id"].is_string(),
        "Should return a job_id: {:?}",
        json
    );
}

#[tokio::test]
async fn test_bulk_resolve_requires_auth() {
    let server = TestServer::start_api_only().await;
    let client = TestClient::new(&server.api_url);

    client
        .post_json(
            "/api/v1/admin/media-roots/files/bulk-resolve",
            &json!({"file_ids": []}),
        )
        .await
        .assert_status(StatusCode::UNAUTHORIZED);
}

// =============================================================================
// Directory listing tests
// =============================================================================

#[tokio::test]
async fn test_list_directories_absolute_paths() {
    let server = TestServer::start_api_only().await;
    let (root_id, root_dir) = create_root(&server, "audiobook").await;
    let root_uuid: Uuid = root_id.parse().unwrap();
    let root_path = root_dir.path().to_string_lossy().to_string();

    // Insert files with absolute paths under the root directory.
    let files = [
        format!("{root_path}/Author A/Book 1/chapter1.mp3"),
        format!("{root_path}/Author A/Book 1/chapter2.mp3"),
        format!("{root_path}/Author A/Book 2/chapter1.mp3"),
        format!("{root_path}/Author B/Book 3/chapter1.mp3"),
    ];
    for file_path in &files {
        let file_name = file_path.rsplit('/').next().unwrap();
        let input = MediaRootFileInput {
            media_root_id: root_uuid,
            file_path: file_path.clone(),
            file_name: file_name.to_string(),
            file_size: Some(1000),
            file_mtime: None,
            file_fingerprint: None,
        };
        media_root_files::upsert_by_path(&server.pool, input)
            .await
            .unwrap();
    }

    // List top-level directories — should return "Author A" and "Author B".
    let resp = server
        .admin_client()
        .get(&format!("/api/v1/admin/media-roots/{root_id}/directories"))
        .await;
    resp.assert_ok();
    let dirs: Vec<Value> = resp.json();
    assert_eq!(dirs.len(), 2, "Expected 2 top-level dirs: {dirs:?}");
    assert_eq!(dirs[0]["name"], "Author A");
    assert_eq!(dirs[0]["file_count"], 3);
    assert_eq!(dirs[1]["name"], "Author B");
    assert_eq!(dirs[1]["file_count"], 1);

    // List subdirectories of "Author A" — should return "Book 1" and "Book 2".
    let resp = server
        .admin_client()
        .get(&format!(
            "/api/v1/admin/media-roots/{root_id}/directories?parent=Author A"
        ))
        .await;
    resp.assert_ok();
    let dirs: Vec<Value> = resp.json();
    assert_eq!(dirs.len(), 2, "Expected 2 sub-dirs: {dirs:?}");
    assert_eq!(dirs[0]["name"], "Book 1");
    assert_eq!(dirs[0]["file_count"], 2);
    assert_eq!(dirs[1]["name"], "Book 2");
    assert_eq!(dirs[1]["file_count"], 1);
}

#[tokio::test]
async fn test_list_directories_nonexistent_root() {
    let server = TestServer::start_api_only().await;
    let fake_id = Uuid::new_v4();
    let resp = server
        .admin_client()
        .get(&format!("/api/v1/admin/media-roots/{fake_id}/directories"))
        .await;
    resp.assert_status(StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn test_list_files_with_directory_filter() {
    let server = TestServer::start_api_only().await;
    let (root_id, root_dir) = create_root(&server, "audiobook").await;
    let root_uuid: Uuid = root_id.parse().unwrap();
    let root_path = root_dir.path().to_string_lossy().to_string();

    // Insert files: 2 directly under Author A, 1 in a subdirectory.
    let direct_file = format!("{root_path}/Author A/standalone.mp3");
    let nested_file = format!("{root_path}/Author A/Book 1/chapter1.mp3");
    for file_path in [&direct_file, &nested_file] {
        let file_name = file_path.rsplit('/').next().unwrap();
        let input = MediaRootFileInput {
            media_root_id: root_uuid,
            file_path: file_path.clone(),
            file_name: file_name.to_string(),
            file_size: Some(1000),
            file_mtime: None,
            file_fingerprint: None,
        };
        media_root_files::upsert_by_path(&server.pool, input)
            .await
            .unwrap();
    }

    // Filter to direct children of "Author A" only — should return 1 file.
    let resp = server
        .admin_client()
        .get(&format!(
            "/api/v1/admin/media-roots/{root_id}/files?lite=true&directory=Author A"
        ))
        .await;
    resp.assert_ok();
    let json: Value = resp.json();
    let items = json["items"].as_array().unwrap();
    assert_eq!(items.len(), 1, "Expected 1 direct child: {items:?}");
    assert!(
        items[0]["file_path"]
            .as_str()
            .unwrap()
            .ends_with("standalone.mp3"),
        "Expected standalone.mp3, got: {}",
        items[0]["file_path"]
    );
}

#[tokio::test]
async fn test_bulk_rematch_resets_files() {
    let server = TestServer::start_api_only().await;
    let (root_id, _dir) = create_root(&server, "tv").await;

    let candidates = json!([{
        "provider": "tmdb",
        "provider_id": "12345",
        "title": "Test Show",
        "year": 2024
    }]);
    let interp = json!({
        "show_name": "Test Show",
        "season": 1,
        "episode": 1,
        "episodes": [1]
    });

    let file_id = insert_review_file(
        &server,
        &root_id,
        "/tmp/show/S01E01.mkv",
        "S01E01.mkv",
        &candidates,
        Some(&interp),
    )
    .await;

    // Verify it's in review status
    let file = media_root_files::get_by_id(&server.pool, file_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(file.match_status, "review");

    // Bulk rematch
    let resp = server
        .admin_client()
        .post_json(
            "/api/v1/admin/media-roots/files/bulk-rematch",
            &json!({"file_ids": [file_id.to_string()]}),
        )
        .await;
    resp.assert_status(StatusCode::NO_CONTENT);

    // Verify it's been reset to pending
    let file = media_root_files::get_by_id(&server.pool, file_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(file.match_status, "pending");
}

#[tokio::test]
async fn test_bulk_rematch_requires_auth() {
    let server = TestServer::start_api_only().await;
    let client = TestClient::new(&server.api_url);

    client
        .post_json(
            "/api/v1/admin/media-roots/files/bulk-rematch",
            &json!({"file_ids": []}),
        )
        .await
        .assert_status(StatusCode::UNAUTHORIZED);
}

// =============================================================================
// Resolve / Add-to-Library Tests
// =============================================================================

/// Helper: insert a media root file with given status and candidates via SQL.
async fn insert_file_with_status(
    server: &TestServer,
    root_id: &str,
    status: &str,
    candidates: &Value,
) -> Uuid {
    let root_uuid: Uuid = root_id.parse().unwrap();
    let input = MediaRootFileInput {
        media_root_id: root_uuid,
        file_path: format!("/tmp/test-{}.epub", Uuid::now_v7()),
        file_name: "test.epub".to_string(),
        file_size: Some(1000),
        file_mtime: None,
        file_fingerprint: None,
    };
    let (file_id, _) = media_root_files::upsert_by_path(&server.pool, input)
        .await
        .unwrap();

    match status {
        "review" => media_root_files::set_review(&server.pool, file_id, candidates)
            .await
            .unwrap(),
        "ambiguous" => media_root_files::set_ambiguous(&server.pool, file_id, candidates)
            .await
            .unwrap(),
        _ => panic!("unsupported status: {}", status),
    }

    file_id
}

fn book_candidates() -> Value {
    json!([
        {
            "provider": "hardcover",
            "provider_id": "12345",
            "title": "Test Book",
            "authors": ["Test Author"],
            "confidence": 0.95
        },
        {
            "provider": "hardcover",
            "provider_id": "67890",
            "title": "Other Book",
            "authors": ["Other Author"],
            "confidence": 0.6
        }
    ])
}

#[tokio::test]
async fn test_resolve_ambiguous_auto_add_false_goes_to_review() {
    let server = TestServer::start_api_only().await;
    let (root_id, _dir) = create_root_with_opts(&server, "book", false).await;
    let file_id = insert_file_with_status(&server, &root_id, "ambiguous", &book_candidates()).await;

    let resp = server
        .admin_client()
        .post_json(
            &format!("/api/v1/admin/media-roots/files/{file_id}/match"),
            &json!({
                "provider": "hardcover",
                "provider_id": "12345",
                "title": "Test Book",
            }),
        )
        .await;
    resp.assert_ok();

    let body: Value = resp.json();
    assert_eq!(body["status"], "confirmed", "should confirm, not resolve");

    // File should now be in review, not added
    let file = media_root_files::get_by_id(&server.pool, file_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(file.match_status, "review");

    // First candidate should be the chosen one with resolved=true
    let candidates: Vec<Value> = serde_json::from_value(file.match_candidates.unwrap()).unwrap();
    assert_eq!(candidates[0]["provider_id"], "12345");
    assert_eq!(candidates[0]["resolved"], true);
}

#[tokio::test]
async fn test_resolve_review_updates_candidate() {
    let server = TestServer::start_api_only().await;
    let (root_id, _dir) = create_root_with_opts(&server, "book", false).await;
    let file_id = insert_file_with_status(&server, &root_id, "review", &book_candidates()).await;

    // Pick the second candidate
    let resp = server
        .admin_client()
        .post_json(
            &format!("/api/v1/admin/media-roots/files/{file_id}/match"),
            &json!({
                "provider": "hardcover",
                "provider_id": "67890",
                "title": "Other Book",
            }),
        )
        .await;
    resp.assert_ok();

    let body: Value = resp.json();
    assert_eq!(body["status"], "confirmed");

    // Still in review
    let file = media_root_files::get_by_id(&server.pool, file_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(file.match_status, "review");

    // First candidate should now be "Other Book" with resolved=true
    let candidates: Vec<Value> = serde_json::from_value(file.match_candidates.unwrap()).unwrap();
    assert_eq!(candidates[0]["provider_id"], "67890");
    assert_eq!(candidates[0]["resolved"], true);
    // Original first candidate should be second without resolved
    assert_eq!(candidates[1]["provider_id"], "12345");
}

#[tokio::test]
async fn test_add_to_library_from_review() {
    let server = TestServer::start_api_only().await;
    let (root_id, _dir) = create_root_with_opts(&server, "book", false).await;

    let candidates = json!([{
        "provider": "hardcover",
        "provider_id": "hc-99999",
        "title": "Library Test Book",
        "authors": ["Some Author"],
        "confidence": 0.95
    }]);
    let file_id = insert_file_with_status(&server, &root_id, "review", &candidates).await;

    let resp = server
        .admin_client()
        .post(&format!("/api/v1/admin/media-roots/files/{file_id}/add"))
        .await;
    resp.assert_ok();

    let body: Value = resp.json();
    assert_eq!(body["status"], "resolved");
    assert!(body["entity_type"].as_str().is_some());
    assert!(body["entity_id"].as_str().is_some());

    // File should now be added
    let file = media_root_files::get_by_id(&server.pool, file_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(file.match_status, "added");
}

#[tokio::test]
async fn test_add_to_library_rejects_non_review() {
    let server = TestServer::start_api_only().await;
    let (root_id, _dir) = create_root_with_opts(&server, "book", false).await;
    let file_id = insert_file_with_status(&server, &root_id, "ambiguous", &book_candidates()).await;

    let resp = server
        .admin_client()
        .post(&format!("/api/v1/admin/media-roots/files/{file_id}/add"))
        .await;
    resp.assert_status(StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn test_add_to_library_requires_auth() {
    let server = TestServer::start_api_only().await;
    let file_id = Uuid::now_v7();

    let client = TestClient::new(&server.api_url);
    client
        .post(&format!("/api/v1/admin/media-roots/files/{file_id}/add"))
        .await
        .assert_status(StatusCode::UNAUTHORIZED);
}

// =============================================================================
// Bulk Manual Resolve Tests
// =============================================================================

#[tokio::test]
async fn test_bulk_manual_resolve_enqueues_job() {
    let server = TestServer::start_api_only().await;
    let (root_id, _dir) = create_root_with_opts(&server, "tv", false).await;

    let interp = json!({
        "show_name": "Archer",
        "season": 1,
        "episode": 1,
        "episodes": [1]
    });
    let file_id = insert_review_file(
        &server,
        &root_id,
        "/tmp/archer/S01E01.mkv",
        "S01E01.mkv",
        &json!([]),
        Some(&interp),
    )
    .await;

    let resp = server
        .admin_client()
        .post_json(
            "/api/v1/admin/media-roots/files/bulk-manual-resolve",
            &json!({
                "file_ids": [file_id.to_string()],
                "provider": "tmdb",
                "provider_id": "110381",
                "title": "Archer",
                "year": 2009
            }),
        )
        .await;
    resp.assert_status(StatusCode::ACCEPTED);

    let json: Value = resp.json();
    assert!(json["job_id"].is_string(), "should return a job_id");
}

#[tokio::test]
async fn test_bulk_manual_resolve_empty_request() {
    let server = TestServer::start_api_only().await;

    let resp = server
        .admin_client()
        .post_json(
            "/api/v1/admin/media-roots/files/bulk-manual-resolve",
            &json!({
                "file_ids": [],
                "provider": "tmdb",
                "provider_id": "12345",
                "title": "Test",
            }),
        )
        .await;
    resp.assert_status(StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn test_bulk_manual_resolve_requires_auth() {
    let server = TestServer::start_api_only().await;
    let client = TestClient::new(&server.api_url);

    client
        .post_json(
            "/api/v1/admin/media-roots/files/bulk-manual-resolve",
            &json!({
                "file_ids": [],
                "provider": "tmdb",
                "provider_id": "12345",
                "title": "Test",
            }),
        )
        .await
        .assert_status(StatusCode::UNAUTHORIZED);
}

// =============================================================================
// Bulk Ignore Tests
// =============================================================================

#[tokio::test]
async fn test_bulk_ignore_returns_204() {
    let server = TestServer::start_api_only().await;
    let (root_id, _dir) = create_root(&server, "book").await;

    // Insert a file
    let root_uuid: Uuid = root_id.parse().unwrap();
    let input = MediaRootFileInput {
        media_root_id: root_uuid,
        file_path: format!("/tmp/test-{}.epub", Uuid::now_v7()),
        file_name: "test.epub".to_string(),
        file_size: Some(1000),
        file_mtime: None,
        file_fingerprint: None,
    };
    let (file_id, _) = media_root_files::upsert_by_path(&server.pool, input)
        .await
        .unwrap();

    let resp = server
        .admin_client()
        .post_json(
            "/api/v1/admin/media-roots/files/bulk-ignore",
            &json!({"file_ids": [file_id.to_string()]}),
        )
        .await;
    resp.assert_status(StatusCode::NO_CONTENT);

    // Verify the file is now ignored
    let file = media_root_files::get_by_id(&server.pool, file_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(file.match_status, "ignored");
}

#[tokio::test]
async fn test_bulk_ignore_empty_request() {
    let server = TestServer::start_api_only().await;

    let resp = server
        .admin_client()
        .post_json(
            "/api/v1/admin/media-roots/files/bulk-ignore",
            &json!({"file_ids": []}),
        )
        .await;
    resp.assert_status(StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn test_bulk_ignore_requires_auth() {
    let server = TestServer::start_api_only().await;
    let client = TestClient::new(&server.api_url);

    client
        .post_json(
            "/api/v1/admin/media-roots/files/bulk-ignore",
            &json!({"file_ids": []}),
        )
        .await
        .assert_status(StatusCode::UNAUTHORIZED);
}

// =============================================================================
// Entity Groups + Status Counts + Lite Projection Tests
// =============================================================================

/// Helper: insert a file with given status and interpretation.
async fn insert_interpreted_file(
    server: &TestServer,
    root_id: &str,
    file_path: &str,
    file_name: &str,
    interpretation: &Value,
    status: &str,
    candidates: Option<&Value>,
) -> Uuid {
    let root_uuid: Uuid = root_id.parse().unwrap();
    let input = MediaRootFileInput {
        media_root_id: root_uuid,
        file_path: file_path.to_string(),
        file_name: file_name.to_string(),
        file_size: Some(1000),
        file_mtime: None,
        file_fingerprint: None,
    };
    let (file_id, _) = media_root_files::upsert_by_path(&server.pool, input)
        .await
        .unwrap();

    media_root_files::set_interpreted(&server.pool, file_id, interpretation)
        .await
        .unwrap();

    let default_candidates =
        json!([{"provider":"test","provider_id":"1","title":"t","confidence":0.9}]);
    let cands = candidates.unwrap_or(&default_candidates);

    match status {
        "identified" => {} // already set by set_interpreted
        "review" => media_root_files::set_review(&server.pool, file_id, cands)
            .await
            .unwrap(),
        "ambiguous" => media_root_files::set_ambiguous(&server.pool, file_id, cands)
            .await
            .unwrap(),
        "unmatched" => media_root_files::set_unmatched(&server.pool, file_id)
            .await
            .unwrap(),
        "added" => media_root_files::set_added(&server.pool, file_id, "tv_episode", Uuid::now_v7())
            .await
            .unwrap(),
        "ignored" => media_root_files::set_ignored(&server.pool, file_id)
            .await
            .unwrap(),
        _ => panic!("unsupported status: {status}"),
    }

    file_id
}

#[tokio::test]
async fn test_entity_groups_tv() {
    let server = TestServer::start_api_only().await;
    let (root_id, _dir) = create_root(&server, "tv").await;

    let bb_cands = json!([{"provider":"tmdb","provider_id":"bb-1","title":"Breaking Bad","year":2008,"confidence":0.9}]);

    // Insert files for two shows
    for ep in 1..=3 {
        insert_interpreted_file(
            &server,
            &root_id,
            &format!("/tmp/tv/Breaking Bad/S01E{ep:02}.mkv"),
            &format!("S01E{ep:02}.mkv"),
            &json!({"show_name": "Breaking Bad", "show_year": 2008, "season": 1, "episodes": [ep]}),
            "review",
            Some(&bb_cands),
        )
        .await;
    }

    // "Added" file needs a real entity — use a tv_episode entity_id
    let wire_entity_id = Uuid::now_v7();
    let wire_show_id = Uuid::now_v7();
    let wire_season_id = Uuid::now_v7();
    // Create show, season, episode in DB
    sqlx::query("INSERT INTO tv_shows (id, title) VALUES ($1, $2)")
        .bind(wire_show_id)
        .bind("The Wire")
        .execute(&server.pool)
        .await
        .unwrap();
    sqlx::query(
        "INSERT INTO tv_seasons (id, show_id, season_identifier, season_sort_order) VALUES ($1, $2, '1', 1)",
    )
    .bind(wire_season_id)
    .bind(wire_show_id)
    .execute(&server.pool)
    .await
    .unwrap();
    sqlx::query("INSERT INTO tv_episodes (id, season_id, episode_identifier, episode_sort_order, title) VALUES ($1, $2, '1', 1, 'Ep1')")
        .bind(wire_entity_id)
        .bind(wire_season_id)
        .execute(&server.pool)
        .await
        .unwrap();

    insert_interpreted_file(
        &server,
        &root_id,
        "/tmp/tv/The Wire/S01E01.mkv",
        "S01E01.mkv",
        &json!({"show_name": "The Wire", "show_year": 2002, "season": 1, "episodes": [1]}),
        "review",
        Some(&json!([{"provider":"tmdb","provider_id":"wire-1","title":"The Wire","year":2002,"confidence":0.9}])),
    )
    .await;

    let resp = server
        .admin_client()
        .get(&format!(
            "/api/v1/admin/media-roots/{root_id}/entity-groups"
        ))
        .await;
    resp.assert_ok();
    let json: Value = resp.json();

    let groups = json["groups"].as_array().unwrap();
    assert_eq!(groups.len(), 2);

    // Groups are sorted alphabetically by label
    assert_eq!(groups[0]["label"], "Breaking Bad");
    assert_eq!(groups[0]["file_count"], 3);
    assert_eq!(groups[0]["review_count"], 3);

    assert_eq!(groups[1]["label"], "The Wire");
    assert_eq!(groups[1]["file_count"], 1);
    assert_eq!(groups[1]["review_count"], 1);

    assert_eq!(json["ungrouped_count"], 0);
}

#[tokio::test]
async fn test_entity_groups_tv_with_subgroups() {
    let server = TestServer::start_api_only().await;
    let (root_id, _dir) = create_root(&server, "tv").await;

    let show_cands =
        json!([{"provider":"tmdb","provider_id":"show-1","title":"Show","confidence":0.9}]);

    // Two seasons of one show — all review so they share the same candidate group
    for ep in 1..=2 {
        insert_interpreted_file(
            &server,
            &root_id,
            &format!("/tmp/tv/Show/S01E{ep:02}.mkv"),
            &format!("S01E{ep:02}.mkv"),
            &json!({"show_name": "Show", "season": 1, "episodes": [ep]}),
            "review",
            Some(&show_cands),
        )
        .await;
    }
    insert_interpreted_file(
        &server,
        &root_id,
        "/tmp/tv/Show/S02E01.mkv",
        "S02E01.mkv",
        &json!({"show_name": "Show", "season": 2, "episodes": [1]}),
        "review",
        Some(&show_cands),
    )
    .await;

    let resp = server
        .admin_client()
        .get(&format!(
            "/api/v1/admin/media-roots/{root_id}/entity-groups"
        ))
        .await;
    resp.assert_ok();
    let json: Value = resp.json();

    let groups = json["groups"].as_array().unwrap();
    assert_eq!(groups.len(), 1);
    assert_eq!(groups[0]["label"], "Show");
    assert_eq!(groups[0]["file_count"], 3);

    let subgroups = groups[0]["subgroups"].as_array().unwrap();
    assert_eq!(subgroups.len(), 2);
    assert_eq!(subgroups[0]["season"], 1);
    assert_eq!(subgroups[0]["file_count"], 2);
    assert_eq!(subgroups[1]["season"], 2);
    assert_eq!(subgroups[1]["file_count"], 1);
}

#[tokio::test]
async fn test_entity_groups_movie() {
    let server = TestServer::start_api_only().await;
    let (root_id, _dir) = create_root(&server, "movie").await;

    let movie_cands = json!([{"provider":"tmdb","provider_id":"inception-1","title":"Inception","year":2010,"confidence":0.9}]);

    insert_interpreted_file(
        &server,
        &root_id,
        "/tmp/movies/Inception.mkv",
        "Inception.mkv",
        &json!({"movie_title": "Inception", "movie_year": 2010}),
        "review",
        Some(&movie_cands),
    )
    .await;

    insert_interpreted_file(
        &server,
        &root_id,
        "/tmp/movies/Inception.srt",
        "Inception.srt",
        &json!({"movie_title": "Inception", "movie_year": 2010}),
        "review",
        Some(&movie_cands),
    )
    .await;

    let resp = server
        .admin_client()
        .get(&format!(
            "/api/v1/admin/media-roots/{root_id}/entity-groups"
        ))
        .await;
    resp.assert_ok();
    let json: Value = resp.json();

    let groups = json["groups"].as_array().unwrap();
    assert_eq!(groups.len(), 1);
    assert_eq!(groups[0]["label"], "Inception");
    assert_eq!(groups[0]["file_count"], 2);
    assert_eq!(groups[0]["review_count"], 2);
}

#[tokio::test]
async fn test_entity_groups_book_with_standalone() {
    let server = TestServer::start_api_only().await;
    let (root_id, _dir) = create_root(&server, "book").await;

    let lotr1_cands = json!([{"provider":"hardcover","provider_id":"lotr-1","title":"Fellowship","series_name":"Lord of the Rings","series_index":1,"confidence":0.9}]);
    let lotr2_cands = json!([{"provider":"hardcover","provider_id":"lotr-2","title":"Two Towers","series_name":"Lord of the Rings","series_index":2,"confidence":0.9}]);
    let standalone_cands = json!([{"provider":"hardcover","provider_id":"standalone-1","title":"A Standalone Book","confidence":0.9}]);

    // Books in a series — each book is its own match group now
    insert_interpreted_file(
        &server,
        &root_id,
        "/tmp/books/LOTR1.epub",
        "LOTR1.epub",
        &json!({"series": "Lord of the Rings", "title": "Fellowship", "series_index": 1}),
        "review",
        Some(&lotr1_cands),
    )
    .await;
    insert_interpreted_file(
        &server,
        &root_id,
        "/tmp/books/LOTR2.epub",
        "LOTR2.epub",
        &json!({"series": "Lord of the Rings", "title": "Two Towers", "series_index": 2}),
        "review",
        Some(&lotr2_cands),
    )
    .await;

    // Standalone book (no series)
    insert_interpreted_file(
        &server,
        &root_id,
        "/tmp/books/Standalone.epub",
        "Standalone.epub",
        &json!({"title": "A Standalone Book"}),
        "review",
        Some(&standalone_cands),
    )
    .await;

    let resp = server
        .admin_client()
        .get(&format!(
            "/api/v1/admin/media-roots/{root_id}/entity-groups"
        ))
        .await;
    resp.assert_ok();
    let json: Value = resp.json();

    let groups = json["groups"].as_array().unwrap();
    // Each book is its own group now (match-based, not series-based)
    assert_eq!(groups.len(), 3);
}

#[tokio::test]
async fn test_entity_groups_ungrouped() {
    let server = TestServer::start_api_only().await;
    let (root_id, _dir) = create_root(&server, "tv").await;

    // File with no interpretation at all
    let root_uuid: Uuid = root_id.parse().unwrap();
    let input = MediaRootFileInput {
        media_root_id: root_uuid,
        file_path: "/tmp/tv/random.mkv".to_string(),
        file_name: "random.mkv".to_string(),
        file_size: Some(1000),
        file_mtime: None,
        file_fingerprint: None,
    };
    media_root_files::upsert_by_path(&server.pool, input)
        .await
        .unwrap();

    let resp = server
        .admin_client()
        .get(&format!(
            "/api/v1/admin/media-roots/{root_id}/entity-groups"
        ))
        .await;
    resp.assert_ok();
    let json: Value = resp.json();

    assert_eq!(json["groups"].as_array().unwrap().len(), 0);
    assert_eq!(json["ungrouped_count"], 1);
}

#[tokio::test]
async fn test_status_counts() {
    let server = TestServer::start_api_only().await;
    let (root_id, _dir) = create_root(&server, "tv").await;

    insert_interpreted_file(
        &server,
        &root_id,
        "/tmp/tv/a.mkv",
        "a.mkv",
        &json!({"show_name": "A"}),
        "review",
        None,
    )
    .await;
    insert_interpreted_file(
        &server,
        &root_id,
        "/tmp/tv/b.mkv",
        "b.mkv",
        &json!({"show_name": "B"}),
        "review",
        None,
    )
    .await;
    insert_interpreted_file(
        &server,
        &root_id,
        "/tmp/tv/c.mkv",
        "c.mkv",
        &json!({"show_name": "C"}),
        "added",
        None,
    )
    .await;
    insert_interpreted_file(
        &server,
        &root_id,
        "/tmp/tv/d.mkv",
        "d.mkv",
        &json!({"show_name": "D"}),
        "ignored",
        None,
    )
    .await;

    let resp = server
        .admin_client()
        .get(&format!(
            "/api/v1/admin/media-roots/{root_id}/status-counts"
        ))
        .await;
    resp.assert_ok();
    let json: Value = resp.json();

    let counts = &json["counts"];
    assert_eq!(counts["review"], 2);
    assert_eq!(counts["added"], 1);
    assert_eq!(counts["ignored"], 1);
}

#[tokio::test]
async fn test_list_files_lite_projection() {
    let server = TestServer::start_api_only().await;
    let (root_id, _dir) = create_root(&server, "tv").await;

    insert_interpreted_file(
        &server,
        &root_id,
        "/tmp/tv/show/S01E01.mkv",
        "S01E01.mkv",
        &json!({"show_name": "Show", "season": 1, "episodes": [1]}),
        "review",
        None,
    )
    .await;

    // Normal (full) response should have match_candidates
    let resp = server
        .admin_client()
        .get(&format!(
            "/api/v1/admin/media-roots/{root_id}/files?limit=10"
        ))
        .await;
    resp.assert_ok();
    let json: Value = resp.json();
    assert!(json["items"][0].get("match_candidates").is_some());

    // Lite response should NOT have match_candidates
    let resp = server
        .admin_client()
        .get(&format!(
            "/api/v1/admin/media-roots/{root_id}/files?limit=10&lite=true"
        ))
        .await;
    resp.assert_ok();
    let json: Value = resp.json();
    assert!(json["items"][0].get("match_candidates").is_none());
    // But should still have basic fields
    assert!(json["items"][0].get("file_name").is_some());
    assert!(json["items"][0].get("match_status").is_some());
}

#[tokio::test]
async fn test_list_files_group_filtered() {
    let server = TestServer::start_api_only().await;
    let (root_id, _dir) = create_root(&server, "tv").await;

    let show_a_cands =
        json!([{"provider":"tmdb","provider_id":"show-a","title":"Show A","confidence":0.9}]);
    let show_b_cands =
        json!([{"provider":"tmdb","provider_id":"show-b","title":"Show B","confidence":0.9}]);

    // Insert files for two shows
    insert_interpreted_file(
        &server,
        &root_id,
        "/tmp/tv/ShowA/S01E01.mkv",
        "S01E01.mkv",
        &json!({"show_name": "Show A", "season": 1, "episodes": [1]}),
        "review",
        Some(&show_a_cands),
    )
    .await;
    insert_interpreted_file(
        &server,
        &root_id,
        "/tmp/tv/ShowA/S01E02.mkv",
        "S01E02.mkv",
        &json!({"show_name": "Show A", "season": 1, "episodes": [2]}),
        "review",
        Some(&show_a_cands),
    )
    .await;
    insert_interpreted_file(
        &server,
        &root_id,
        "/tmp/tv/ShowB/S01E01.mkv",
        "S01E01.mkv",
        &json!({"show_name": "Show B", "season": 1, "episodes": [1]}),
        "review",
        Some(&show_b_cands),
    )
    .await;

    // Filter by match-based group_key (provider:provider_id)
    let resp = server
        .admin_client()
        .get(&format!(
            "/api/v1/admin/media-roots/{root_id}/files?group_key=tmdb:show-a&limit=100"
        ))
        .await;
    resp.assert_ok();
    let json: Value = resp.json();
    assert_eq!(json["items"].as_array().unwrap().len(), 2);
    assert_eq!(json["total_count"], 2);

    // All returned files should be for "Show A"
    for item in json["items"].as_array().unwrap() {
        let interp = &item["interpretation"];
        assert_eq!(interp["show_name"], "Show A");
    }
}

#[tokio::test]
async fn test_list_files_group_filtered_with_season() {
    let server = TestServer::start_api_only().await;
    let (root_id, _dir) = create_root(&server, "tv").await;

    let show_cands =
        json!([{"provider":"tmdb","provider_id":"show-1","title":"Show","confidence":0.9}]);

    insert_interpreted_file(
        &server,
        &root_id,
        "/tmp/tv/Show/S01E01.mkv",
        "S01E01.mkv",
        &json!({"show_name": "Show", "season": 1, "episodes": [1]}),
        "review",
        Some(&show_cands),
    )
    .await;
    insert_interpreted_file(
        &server,
        &root_id,
        "/tmp/tv/Show/S02E01.mkv",
        "S02E01.mkv",
        &json!({"show_name": "Show", "season": 2, "episodes": [1]}),
        "review",
        Some(&show_cands),
    )
    .await;

    // Filter by match-based group_key + season
    let resp = server
        .admin_client()
        .get(&format!(
            "/api/v1/admin/media-roots/{root_id}/files?group_key=tmdb:show-1&season=1&limit=100"
        ))
        .await;
    resp.assert_ok();
    let json: Value = resp.json();
    assert_eq!(json["items"].as_array().unwrap().len(), 1);
    assert_eq!(json["total_count"], 1);
}

#[tokio::test]
async fn test_list_files_group_filtered_with_status() {
    let server = TestServer::start_api_only().await;
    let (root_id, _dir) = create_root(&server, "tv").await;

    let show_cands =
        json!([{"provider":"tmdb","provider_id":"show-1","title":"Show","confidence":0.9}]);

    insert_interpreted_file(
        &server,
        &root_id,
        "/tmp/tv/Show/S01E01.mkv",
        "S01E01.mkv",
        &json!({"show_name": "Show", "season": 1, "episodes": [1]}),
        "review",
        Some(&show_cands),
    )
    .await;
    insert_interpreted_file(
        &server,
        &root_id,
        "/tmp/tv/Show/S01E02.mkv",
        "S01E02.mkv",
        &json!({"show_name": "Show", "season": 1, "episodes": [2]}),
        "review",
        Some(&show_cands),
    )
    .await;

    // Filter by match-based group_key + status
    let resp = server
        .admin_client()
        .get(&format!(
            "/api/v1/admin/media-roots/{root_id}/files?group_key=tmdb:show-1&status=review&limit=100"
        ))
        .await;
    resp.assert_ok();
    let json: Value = resp.json();
    assert_eq!(json["items"].as_array().unwrap().len(), 2);
}

#[tokio::test]
async fn test_entity_groups_requires_auth() {
    let server = TestServer::start_api_only().await;
    let (root_id, _dir) = create_root(&server, "tv").await;

    let client = TestClient::new(&server.api_url);
    client
        .get(&format!(
            "/api/v1/admin/media-roots/{root_id}/entity-groups"
        ))
        .await
        .assert_status(StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn test_status_counts_requires_auth() {
    let server = TestServer::start_api_only().await;
    let (root_id, _dir) = create_root(&server, "tv").await;

    let client = TestClient::new(&server.api_url);
    client
        .get(&format!(
            "/api/v1/admin/media-roots/{root_id}/status-counts"
        ))
        .await
        .assert_status(StatusCode::UNAUTHORIZED);
}

// =============================================================================
// Manual Entity Linking Tests
// =============================================================================

/// Helper: insert a media root file (pending status).
async fn insert_pending_file(
    server: &TestServer,
    root_id: &str,
    file_path: &str,
    file_name: &str,
) -> Uuid {
    let root_uuid: Uuid = root_id.parse().unwrap();
    let input = MediaRootFileInput {
        media_root_id: root_uuid,
        file_path: file_path.to_string(),
        file_name: file_name.to_string(),
        file_size: Some(1000),
        file_mtime: None,
        file_fingerprint: None,
    };
    let (file_id, _) = media_root_files::upsert_by_path(&server.pool, input)
        .await
        .unwrap();
    file_id
}

#[tokio::test]
async fn test_link_file_to_existing_book() {
    let server = TestServer::start_api_only().await;
    let (root_id, _dir) = create_root(&server, "book").await;
    let book_id = create_book(&server, "Manual Link Book").await;

    let file_id =
        insert_pending_file(&server, &root_id, "/media/books/manual.epub", "manual.epub").await;

    let resp = server
        .admin_client()
        .post_json(
            &format!("/api/v1/admin/media-roots/files/{file_id}/link"),
            &json!({
                "entity_type": "book",
                "entity_id": book_id,
            }),
        )
        .await;
    resp.assert_ok();

    let body: Value = resp.json();
    assert_eq!(body["status"], "linked");
    assert_eq!(body["entity_type"], "book");
    assert_eq!(body["entity_id"], book_id.to_string());

    // Verify file is now added
    let file = media_root_files::get_by_id(&server.pool, file_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(file.match_status, "added");
    assert_eq!(file.entity_type.as_deref(), Some("book"));
    assert_eq!(file.entity_id, Some(book_id));
}

#[tokio::test]
async fn test_link_audiobook_file_to_existing_book() {
    let server = TestServer::start_api_only().await;
    let (root_id, _dir) = create_root(&server, "audiobook").await;
    let book_id = create_book(&server, "Audiobook Link Book").await;

    let file_id = insert_pending_file(
        &server,
        &root_id,
        "/media/audiobooks/story.m4b",
        "story.m4b",
    )
    .await;

    let resp = server
        .admin_client()
        .post_json(
            &format!("/api/v1/admin/media-roots/files/{file_id}/link"),
            &json!({
                "entity_type": "book",
                "entity_id": book_id,
            }),
        )
        .await;
    resp.assert_ok();

    let body: Value = resp.json();
    assert_eq!(body["status"], "linked");
    assert_eq!(body["entity_type"], "book");
}

#[tokio::test]
async fn test_link_file_to_existing_movie() {
    let server = TestServer::start_api_only().await;
    let (root_id, _dir) = create_root(&server, "movie").await;

    // Create a movie
    let resp = server
        .admin_client()
        .post_json(
            "/api/v1/admin/movies",
            &json!({
                "title": "Test Movie",
                "year": 2024,
                "identifiers": { "tmdb_id": Uuid::now_v7().to_string() }
            }),
        )
        .await;
    resp.assert_status(StatusCode::CREATED);
    let movie_id = resp.json::<Value>()["id"].as_str().unwrap().to_string();

    let file_id =
        insert_pending_file(&server, &root_id, "/media/movies/test.mkv", "test.mkv").await;

    let resp = server
        .admin_client()
        .post_json(
            &format!("/api/v1/admin/media-roots/files/{file_id}/link"),
            &json!({
                "entity_type": "movie",
                "entity_id": movie_id,
            }),
        )
        .await;
    resp.assert_ok();

    let body: Value = resp.json();
    assert_eq!(body["status"], "linked");
    assert_eq!(body["entity_type"], "movie");
}

#[tokio::test]
async fn test_link_file_to_existing_tv_episode() {
    let server = TestServer::start_api_only().await;
    let (root_id, _dir) = create_root(&server, "tv").await;

    // Create show
    let resp = server
        .admin_client()
        .post_json(
            "/api/v1/admin/tv/shows",
            &json!({
                "title": "Test Show",
                "year": 2024,
                "identifiers": { "tmdb_id": Uuid::now_v7().to_string() }
            }),
        )
        .await;
    resp.assert_status(StatusCode::CREATED);
    let show_id: Uuid = resp.json::<Value>()["id"]
        .as_str()
        .unwrap()
        .parse()
        .unwrap();

    // Create season + episode directly in DB
    let season_id = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO tv_seasons (id, show_id, season_identifier, season_sort_order, is_monitored, is_special) VALUES ($1, $2, '1', 1, true, false)",
    )
    .bind(season_id)
    .bind(show_id)
    .execute(&server.pool)
    .await
    .unwrap();

    let episode_id = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO tv_episodes (id, season_id, episode_identifier, episode_sort_order, title) VALUES ($1, $2, '1', 1, 'Pilot')",
    )
    .bind(episode_id)
    .bind(season_id)
    .execute(&server.pool)
    .await
    .unwrap();

    let file_id = insert_pending_file(
        &server,
        &root_id,
        "/media/tv/show.s01e01.mkv",
        "show.s01e01.mkv",
    )
    .await;

    let resp = server
        .admin_client()
        .post_json(
            &format!("/api/v1/admin/media-roots/files/{file_id}/link"),
            &json!({
                "entity_type": "tv_episode",
                "entity_id": episode_id,
            }),
        )
        .await;
    resp.assert_ok();

    let body: Value = resp.json();
    assert_eq!(body["status"], "linked");
    assert_eq!(body["entity_type"], "tv_episode");
}

#[tokio::test]
async fn test_link_file_nonexistent_entity() {
    let server = TestServer::start_api_only().await;
    let (root_id, _dir) = create_root(&server, "book").await;

    let file_id = insert_pending_file(
        &server,
        &root_id,
        "/media/books/missing.epub",
        "missing.epub",
    )
    .await;

    let resp = server
        .admin_client()
        .post_json(
            &format!("/api/v1/admin/media-roots/files/{file_id}/link"),
            &json!({
                "entity_type": "book",
                "entity_id": Uuid::now_v7(),
            }),
        )
        .await;
    resp.assert_status(StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn test_link_file_entity_type_mismatch() {
    let server = TestServer::start_api_only().await;
    let (root_id, _dir) = create_root(&server, "movie").await;
    let book_id = create_book(&server, "Wrong Type Book").await;

    let file_id =
        insert_pending_file(&server, &root_id, "/media/movies/wrong.mkv", "wrong.mkv").await;

    // Trying to link a book to a movie root should fail
    let resp = server
        .admin_client()
        .post_json(
            &format!("/api/v1/admin/media-roots/files/{file_id}/link"),
            &json!({
                "entity_type": "book",
                "entity_id": book_id,
            }),
        )
        .await;
    resp.assert_status(StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn test_bulk_link_files_to_entity() {
    let server = TestServer::start_api_only().await;
    let (root_id, _dir) = create_root(&server, "book").await;
    let book_id = create_book(&server, "Bulk Link Book").await;

    let file1 =
        insert_pending_file(&server, &root_id, "/media/books/part1.epub", "part1.epub").await;
    let file2 =
        insert_pending_file(&server, &root_id, "/media/books/part2.epub", "part2.epub").await;

    let resp = server
        .admin_client()
        .post_json(
            "/api/v1/admin/media-roots/files/bulk-link",
            &json!({
                "file_ids": [file1, file2],
                "entity_type": "book",
                "entity_id": book_id,
            }),
        )
        .await;
    resp.assert_ok();

    let body: Value = resp.json();
    assert_eq!(body["linked"], 2);

    // Verify both files are linked
    let f1 = media_root_files::get_by_id(&server.pool, file1)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(f1.match_status, "added");
    assert_eq!(f1.entity_id, Some(book_id));

    let f2 = media_root_files::get_by_id(&server.pool, file2)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(f2.match_status, "added");
    assert_eq!(f2.entity_id, Some(book_id));
}

#[tokio::test]
async fn test_link_file_requires_auth() {
    let server = TestServer::start_api_only().await;
    let (root_id, _dir) = create_root(&server, "book").await;

    let file_id =
        insert_pending_file(&server, &root_id, "/media/books/noauth.epub", "noauth.epub").await;

    let client = TestClient::new(&server.api_url);
    client
        .post_json(
            &format!("/api/v1/admin/media-roots/files/{file_id}/link"),
            &json!({
                "entity_type": "book",
                "entity_id": Uuid::now_v7(),
            }),
        )
        .await
        .assert_status(StatusCode::UNAUTHORIZED);
}
