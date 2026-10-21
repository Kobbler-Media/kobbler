use crate::common::*;
use reqwest::StatusCode;
use serde_json::Value;

#[tokio::test]
async fn test_backup_requires_auth() {
    let server = TestServer::start_api_only().await;

    // Try without auth
    let client = TestClient::new(&server.api_url);
    let resp = client.get("/api/v1/admin/backup").await;
    assert_eq!(resp.status, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn test_backup_download() {
    let server = TestServer::start_api_only().await;

    // Download backup
    let resp = server.admin_client().get("/api/v1/admin/backup").await;
    resp.assert_ok();

    // Check response headers
    let content_type = resp
        .headers
        .get("content-type")
        .expect("Missing Content-Type header");
    assert_eq!(content_type, "application/gzip");

    let content_disposition = resp
        .headers
        .get("content-disposition")
        .expect("Missing Content-Disposition header");
    let disposition = content_disposition.to_str().unwrap();
    assert!(disposition.contains("attachment"));
    assert!(disposition.contains("kobbler-backup-"));
    assert!(disposition.contains(".tar.gz"));

    // Check that we got some data (gzip starts with magic bytes 0x1f 0x8b)
    let bytes = resp.bytes();
    assert!(bytes.len() > 100, "Backup file too small");
    assert_eq!(bytes[0..2], [0x1f, 0x8b], "File should be gzip compressed");
}

#[tokio::test]
async fn test_backup_with_data() {
    let server = TestServer::start_api_only().await;

    // Create some data first
    let _book_id = create_book(&server, "Test Book for Backup").await;

    // Download backup
    let resp = server.admin_client().get("/api/v1/admin/backup").await;
    resp.assert_ok();

    // Verify the backup contains data (will be larger than empty backup)
    let bytes = resp.bytes();
    assert!(bytes.len() > 500, "Backup with data should be larger");
}

#[tokio::test]
async fn test_restore_requires_auth() {
    let server = TestServer::start_api_only().await;

    // Try without auth
    let client = TestClient::new(&server.api_url);
    let resp = client
        .post_bytes(
            "/api/v1/admin/restore",
            vec![0, 1, 2, 3],
            "application/gzip",
        )
        .await;
    assert_eq!(resp.status, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn test_restore_rejects_empty_body() {
    let server = TestServer::start_api_only().await;

    let resp = server
        .admin_client()
        .post_bytes("/api/v1/admin/restore", vec![], "application/gzip")
        .await;
    assert_eq!(resp.status, StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn test_restore_rejects_invalid_archive() {
    let server = TestServer::start_api_only().await;

    // Send garbage data that isn't a valid tar.gz
    let resp = server
        .admin_client()
        .post_bytes(
            "/api/v1/admin/restore",
            b"not a valid tar.gz file".to_vec(),
            "application/gzip",
        )
        .await;
    assert_eq!(resp.status, StatusCode::INTERNAL_SERVER_ERROR);
}

#[tokio::test]
async fn test_backup_restore_roundtrip() {
    let server = TestServer::start_api_only().await;

    // Create a book first
    let _book_id = create_book(&server, "Roundtrip Test Book").await;

    // Create a user
    create_user(&server, "backuptest@example.com").await;

    // Download backup
    let backup_resp = server.admin_client().get("/api/v1/admin/backup").await;
    backup_resp.assert_ok();
    let backup_data = backup_resp.bytes().to_vec();

    // Restore from backup
    let restore_resp = server
        .admin_client()
        .post_bytes("/api/v1/admin/restore", backup_data, "application/gzip")
        .await;
    restore_resp.assert_ok();

    let result: Value = restore_resp.json();
    assert_eq!(result["success"], true);
    assert!(result["stats"]["tables_restored"].as_i64().unwrap() > 0);
    // Books count should be >= 1 (the one we created)
    assert!(result["stats"]["books_restored"].as_i64().unwrap() >= 1);

    // Verify the user still exists after restore
    let users_resp = server.admin_client().get("/api/v1/admin/users").await;
    users_resp.assert_ok();
    let users: Vec<Value> = users_resp.json();
    assert!(users.iter().any(|u| u["email"] == "backuptest@example.com"));
}

#[tokio::test]
async fn test_backup_with_proxy_blobs_option() {
    let server = TestServer::start_api_only().await;

    // Download backup without proxy blobs (default)
    let resp_without = server.admin_client().get("/api/v1/admin/backup").await;
    resp_without.assert_ok();
    let size_without = resp_without.bytes().len();

    // Download backup with proxy blobs
    let resp_with = server
        .admin_client()
        .get("/api/v1/admin/backup?include_proxy_blobs=true")
        .await;
    resp_with.assert_ok();
    let size_with = resp_with.bytes().len();

    // Both should be valid gzip files
    assert_eq!(resp_without.bytes()[0..2], [0x1f, 0x8b]);
    assert_eq!(resp_with.bytes()[0..2], [0x1f, 0x8b]);

    // Sizes should be similar when there are no proxy blobs
    // (they're equal or very close since we haven't generated any proxy logs)
    assert!(
        (size_with as i64 - size_without as i64).abs() < 1000,
        "Size difference should be minimal when no proxy blobs exist"
    );
}
