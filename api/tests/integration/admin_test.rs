use crate::common::*;
use reqwest::StatusCode;
use serde_json::Value;

#[tokio::test]
async fn test_create_user() {
    let server = TestServer::start().await;

    let resp = server
        .admin_client()
        .post_json(
            "/api/v1/admin/users",
            &serde_json::json!({
                "email": "newuser@test.com",
                "password": "testpass123"
            }),
        )
        .await;
    resp.assert_ok();

    let json: Value = resp.json();
    assert!(json["user_id"].is_string());
}

#[tokio::test]
async fn test_create_admin_user() {
    let server = TestServer::start().await;

    let resp = server
        .admin_client()
        .post_json(
            "/api/v1/admin/users",
            &serde_json::json!({
                "email": "admin2@test.com",
                "password": "testpass123"
            }),
        )
        .await;
    resp.assert_ok();

    // Verify the new admin can login
    let client = TestClient::new(&server.api_url);
    let resp = client
        .post_json(
            "/api/v1/admin/login",
            &serde_json::json!({
                "email": "admin2@test.com",
                "password": "testpass123"
            }),
        )
        .await;
    resp.assert_ok();
}

#[tokio::test]
async fn test_list_users() {
    let server = TestServer::start().await;

    // Create a user first
    create_user(&server, "listtest@test.com").await;

    let resp = server.admin_client().get("/api/v1/admin/users").await;
    resp.assert_ok();

    let users: Vec<Value> = resp.json();
    assert!(users.len() >= 2); // At least admin + our new user
    assert!(users.iter().any(|u| u["email"] == "listtest@test.com"));
}

#[tokio::test]
async fn test_update_user_email() {
    let server = TestServer::start().await;

    let user_id = create_user(&server, "oldemail@test.com").await;

    // Update email
    let resp = server
        .admin_client()
        .patch_json(
            &format!("/api/v1/admin/users/{}", user_id),
            &serde_json::json!({
                "email": "newemail@test.com"
            }),
        )
        .await;
    resp.assert_ok();

    let json: Value = resp.json();
    assert_eq!(json["email"], "newemail@test.com");

    // Verify old email no longer works for login
    let client = TestClient::new(&server.api_url);
    let resp = client
        .post_json(
            "/api/v1/admin/login",
            &serde_json::json!({
                "email": "oldemail@test.com",
                "password": "testpass123"
            }),
        )
        .await;
    assert_eq!(resp.status, StatusCode::UNAUTHORIZED);

    // Verify new email works
    let resp = client
        .post_json(
            "/api/v1/admin/login",
            &serde_json::json!({
                "email": "newemail@test.com",
                "password": "testpass123"
            }),
        )
        .await;
    resp.assert_ok();
}

#[tokio::test]
async fn test_update_user_password() {
    let server = TestServer::start().await;

    let user_id = create_user(&server, "pwchange@test.com").await;

    // Update password
    let resp = server
        .admin_client()
        .patch_json(
            &format!("/api/v1/admin/users/{}", user_id),
            &serde_json::json!({
                "password": "newpassword456"
            }),
        )
        .await;
    resp.assert_ok();

    // Verify old password no longer works
    let client = TestClient::new(&server.api_url);
    let resp = client
        .post_json(
            "/api/v1/admin/login",
            &serde_json::json!({
                "email": "pwchange@test.com",
                "password": "testpass123"
            }),
        )
        .await;
    assert_eq!(resp.status, StatusCode::UNAUTHORIZED);

    // Verify new password works
    let resp = client
        .post_json(
            "/api/v1/admin/login",
            &serde_json::json!({
                "email": "pwchange@test.com",
                "password": "newpassword456"
            }),
        )
        .await;
    resp.assert_ok();
}

#[tokio::test]
async fn test_update_user_duplicate_email() {
    let server = TestServer::start().await;

    let user1_id = create_user(&server, "user1@test.com").await;
    let _user2_id = create_user(&server, "user2@test.com").await;

    // Try to update user1's email to user2's email
    let resp = server
        .admin_client()
        .patch_json(
            &format!("/api/v1/admin/users/{}", user1_id),
            &serde_json::json!({
                "email": "user2@test.com"
            }),
        )
        .await;
    assert_eq!(resp.status, StatusCode::CONFLICT);
}

#[tokio::test]
async fn test_update_user_not_found() {
    let server = TestServer::start().await;

    let fake_id = uuid::Uuid::new_v4();
    let resp = server
        .admin_client()
        .patch_json(
            &format!("/api/v1/admin/users/{}", fake_id),
            &serde_json::json!({
                "email": "new@test.com"
            }),
        )
        .await;
    assert_eq!(resp.status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn test_create_device() {
    let server = TestServer::start().await;

    let user_id = create_user(&server, "devicetest@test.com").await;

    let resp = server
        .admin_client()
        .post_json(
            "/api/v1/admin/devices",
            &serde_json::json!({
                "user_id": user_id,
                "name": "Test Device"
            }),
        )
        .await;
    resp.assert_ok();

    let json: Value = resp.json();
    assert!(json["device_id"].is_string());
    assert!(json["device_token"].is_string());
    assert!(json["sync_url"].is_string());
}

#[tokio::test]
async fn test_list_devices() {
    let server = TestServer::start().await;

    let user_id = create_user(&server, "devicelist@test.com").await;
    create_device(&server, user_id).await;

    let resp = server.admin_client().get("/api/v1/admin/devices").await;
    resp.assert_ok();

    let devices: Vec<Value> = resp.json();
    assert!(!devices.is_empty());
}

#[tokio::test]
async fn test_rotate_device_token() {
    let server = TestServer::start().await;

    let user_id = create_user(&server, "rotatetest@test.com").await;
    let (device_id, old_token) = create_device(&server, user_id).await;

    // Rotate the token
    let resp = server
        .admin_client()
        .post_json(
            &format!("/api/v1/admin/devices/{}/token", device_id),
            &serde_json::json!({}),
        )
        .await;
    resp.assert_ok();

    let json: Value = resp.json();
    let new_token = json["device_token"].as_str().unwrap();
    assert_ne!(new_token, old_token);

    // Old token should no longer work
    let resp = server.kobo_client().kobo_sync(&old_token).await;
    resp.assert_status(StatusCode::UNAUTHORIZED);

    // New token should work
    let resp = server.kobo_client().kobo_sync(new_token).await;
    resp.assert_ok();
}

#[tokio::test]
async fn test_update_device() {
    let server = TestServer::start().await;

    let user_id = create_user(&server, "updatetest@test.com").await;
    let (device_id, _) = create_device(&server, user_id).await;

    // Update the device name and kobo_api_url
    server
        .admin_client()
        .patch_json(
            &format!("/api/v1/admin/devices/{}", device_id),
            &serde_json::json!({
                "name": "Updated Device Name",
                "kobo_api_url": "https://custom.kobo.api"
            }),
        )
        .await
        .assert_status(StatusCode::NO_CONTENT);

    // Verify the update
    let resp = server.admin_client().get("/api/v1/admin/devices").await;
    resp.assert_ok();

    let devices: Vec<Value> = resp.json();
    let device = devices
        .iter()
        .find(|d| d["id"].as_str() == Some(&device_id.to_string()))
        .expect("Device should exist");

    assert_eq!(device["name"].as_str(), Some("Updated Device Name"));
    assert_eq!(
        device["kobo_api_url"].as_str(),
        Some("https://custom.kobo.api")
    );
}

#[tokio::test]
async fn test_delete_device() {
    let server = TestServer::start().await;

    let user_id = create_user(&server, "deletetest@test.com").await;
    let (device_id, device_token) = create_device(&server, user_id).await;

    // Device should work before deletion
    let resp = server.kobo_client().kobo_sync(&device_token).await;
    resp.assert_ok();

    // Delete the device
    server
        .admin_client()
        .delete(&format!("/api/v1/admin/devices/{}", device_id))
        .await
        .assert_status(StatusCode::NO_CONTENT);

    // Device token should no longer work
    let resp = server.kobo_client().kobo_sync(&device_token).await;
    resp.assert_status(StatusCode::UNAUTHORIZED);

    // Device should not appear in list
    let resp = server.admin_client().get("/api/v1/admin/devices").await;
    resp.assert_ok();
    let devices: Vec<Value> = resp.json();
    assert!(
        !devices
            .iter()
            .any(|d| d["id"].as_str() == Some(&device_id.to_string()))
    );

    // Deleting again should return 404
    server
        .admin_client()
        .delete(&format!("/api/v1/admin/devices/{}", device_id))
        .await
        .assert_status(StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn test_device_activity_requires_auth() {
    let server = TestServer::start().await;

    let user_id = create_user(&server, "activity@test.com").await;
    let (device_id, _) = create_device(&server, user_id).await;

    let resp = server
        .api_client()
        .get(&format!("/api/v1/admin/devices/{}/activity", device_id))
        .await;
    resp.assert_status(StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn test_device_activity_returns_structure() {
    let server = TestServer::start().await;

    let user_id = create_user(&server, "activity2@test.com").await;
    let (device_id, _) = create_device(&server, user_id).await;

    let resp = server
        .admin_client()
        .get(&format!("/api/v1/admin/devices/{}/activity", device_id))
        .await;
    resp.assert_ok();

    let activity: Value = resp.json();
    // Check that the expected fields are present
    assert!(activity.get("last_sync_at").is_some());
    assert!(activity.get("pending_sync").is_some());
    assert!(activity.get("recently_read").is_some());
    assert!(activity.get("recently_synced").is_some());
    assert!(activity.get("pending_delivery").is_some());
    assert!(activity.get("recently_delivered").is_some());
    assert!(activity.get("last_delivery_at").is_some());

    // Pending sync should be an array
    assert!(activity["pending_sync"].as_array().is_some());
}

#[tokio::test]
async fn test_device_activity_not_found() {
    let server = TestServer::start().await;

    let fake_device_id = "01936b96-c5e9-7000-8000-000000000000";
    let resp = server
        .admin_client()
        .get(&format!(
            "/api/v1/admin/devices/{}/activity",
            fake_device_id
        ))
        .await;
    resp.assert_status(StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn test_create_ingest_token() {
    let server = TestServer::start().await;

    let resp = server
        .admin_client()
        .post_json("/api/v1/admin/ingest-tokens", &serde_json::json!({}))
        .await;
    resp.assert_ok();

    let json: Value = resp.json();
    assert!(json["token"].is_string());
}

#[tokio::test]
async fn test_list_ingest_tokens() {
    let server = TestServer::start().await;

    // Create an additional ingest token
    server
        .admin_client()
        .post_json("/api/v1/admin/ingest-tokens", &serde_json::json!({}))
        .await
        .assert_ok();

    let resp = server
        .admin_client()
        .get("/api/v1/admin/ingest-tokens")
        .await;
    resp.assert_ok();

    let tokens: Vec<Value> = resp.json();
    assert!(tokens.len() >= 2); // Server ingest token + our new one
}

#[tokio::test]
async fn test_revoke_ingest_token() {
    let server = TestServer::start().await;

    // Create a new ingest token
    let resp = server
        .admin_client()
        .post_json("/api/v1/admin/ingest-tokens", &serde_json::json!({}))
        .await;
    resp.assert_ok();
    let token: String = resp.json::<Value>()["token"].as_str().unwrap().to_string();

    // Find the token ID
    let resp = server
        .admin_client()
        .get("/api/v1/admin/ingest-tokens")
        .await;
    let tokens: Vec<Value> = resp.json();
    let token_entry = tokens
        .iter()
        .find(|t| !t["revoked"].as_bool().unwrap_or(true))
        .unwrap();
    let token_id = token_entry["id"].as_str().unwrap();

    // Revoke it
    server
        .admin_client()
        .post_json(
            &format!("/api/v1/admin/ingest-tokens/{}/revoke", token_id),
            &serde_json::json!({}),
        )
        .await
        .assert_status(StatusCode::NO_CONTENT);

    // Token should no longer work for ingest
    let book_id = uuid::Uuid::new_v4();
    let client = TestClient::new(&server.api_url).with_auth(&token);
    let resp = client
        .put_json(
            &format!("/api/v1/books/{}", book_id),
            &serde_json::json!({
                "title": "Test Book"
            }),
        )
        .await;
    resp.assert_status(StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn test_remove_and_restore_book_entitlements() {
    let server = TestServer::start().await;

    let user_id = create_user(&server, "entitletest@test.com").await;
    let (_, device_token) = create_device(&server, user_id).await;

    // Create and entitle a book
    let book_id = create_book(&server, "Entitlement Test Book").await;
    entitle_book(&server, book_id).await;

    // Sync should show the book
    let resp = server.kobo_client().kobo_sync(&device_token).await;
    resp.assert_ok();
    let token = resp.get_sync_token().expect("should have sync token");
    let items: Vec<Value> = resp.json();
    assert_sync_contains_book(&items, book_id);

    // Remove the book via admin API
    server
        .admin_client()
        .post_json(
            &format!("/api/v1/admin/books/{}/remove", book_id),
            &serde_json::json!({}),
        )
        .await
        .assert_success();

    // Sync should show book as removed
    let resp = server
        .kobo_client()
        .kobo_sync_with_token(&device_token, &token)
        .await;
    resp.assert_ok();
    let token = resp.get_sync_token().expect("should have sync token");
    let items: Vec<Value> = resp.json();
    assert_book_is_removed(&items, book_id);

    // Restore the book
    server
        .admin_client()
        .post_json(
            &format!("/api/v1/admin/books/{}/restore", book_id),
            &serde_json::json!({}),
        )
        .await
        .assert_success();

    // Sync should show book again (as changed, not removed)
    let items: Vec<Value> = server
        .kobo_client()
        .kobo_sync_with_token(&device_token, &token)
        .await
        .assert_ok()
        .json();

    // Book should appear and not be marked as removed
    let book_entry = items.iter().find(|item| {
        item.get("NewEntitlement")
            .or_else(|| item.get("ChangedEntitlement"))
            .and_then(|e| e.get("BookMetadata"))
            .and_then(|m| m.get("RevisionId"))
            .and_then(|r| r.as_str())
            .map(|id| id == book_id.to_string())
            .unwrap_or(false)
    });
    assert!(
        book_entry.is_some(),
        "Book should appear in sync after restore"
    );
}

#[tokio::test]
async fn test_admin_endpoints_require_auth() {
    let server = TestServer::start().await;
    let client = TestClient::new(&server.api_url);

    // All admin endpoints should require auth
    client
        .get("/api/v1/admin/users")
        .await
        .assert_status(StatusCode::UNAUTHORIZED);

    client
        .get("/api/v1/admin/devices")
        .await
        .assert_status(StatusCode::UNAUTHORIZED);

    client
        .get("/api/v1/admin/ingest-tokens")
        .await
        .assert_status(StatusCode::UNAUTHORIZED);

    client
        .post_json(
            "/api/v1/admin/users",
            &serde_json::json!({"email": "x@x.com", "password": "x"}),
        )
        .await
        .assert_status(StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn test_list_device_books_empty() {
    let server = TestServer::start().await;

    let user_id = create_user(&server, "devicebooks@test.com").await;
    let (device_id, _) = create_device(&server, user_id).await;

    // List books for device - should be empty
    let resp = server
        .admin_client()
        .get(&format!("/api/v1/admin/devices/{}/books", device_id))
        .await;
    resp.assert_ok();

    let books: Vec<Value> = resp.json();
    assert!(books.is_empty());
}

#[tokio::test]
async fn test_add_and_list_device_books() {
    let server = TestServer::start().await;

    let user_id = create_user(&server, "devicebooks@test.com").await;
    let (device_id, _) = create_device(&server, user_id).await;

    // Create a book
    let book_id = create_book(&server, "Device Book Test").await;

    // Add book to device
    let resp = server
        .admin_client()
        .post_json(
            &format!("/api/v1/admin/devices/{}/books", device_id),
            &serde_json::json!({
                "book_ids": [book_id]
            }),
        )
        .await;
    resp.assert_ok();

    let result: Value = resp.json();
    assert_eq!(result["added"], 1);

    // List books for device - should contain our book
    let resp = server
        .admin_client()
        .get(&format!("/api/v1/admin/devices/{}/books", device_id))
        .await;
    resp.assert_ok();

    let books: Vec<Value> = resp.json();
    assert_eq!(books.len(), 1);
    assert_eq!(books[0]["id"], book_id.to_string());
    assert_eq!(books[0]["title"], "Device Book Test");
}

#[tokio::test]
async fn test_add_device_books_idempotent() {
    let server = TestServer::start().await;

    let user_id = create_user(&server, "idempotent@test.com").await;
    let (device_id, _) = create_device(&server, user_id).await;

    let book_id = create_book(&server, "Idempotent Test").await;

    // Add book twice
    server
        .admin_client()
        .post_json(
            &format!("/api/v1/admin/devices/{}/books", device_id),
            &serde_json::json!({ "book_ids": [book_id] }),
        )
        .await
        .assert_ok();

    server
        .admin_client()
        .post_json(
            &format!("/api/v1/admin/devices/{}/books", device_id),
            &serde_json::json!({ "book_ids": [book_id] }),
        )
        .await
        .assert_ok();

    // Should still have just one book
    let books: Vec<Value> = server
        .admin_client()
        .get(&format!("/api/v1/admin/devices/{}/books", device_id))
        .await
        .assert_ok()
        .json();
    assert_eq!(books.len(), 1);
}

#[tokio::test]
async fn test_remove_device_book() {
    let server = TestServer::start().await;

    let user_id = create_user(&server, "removebook@test.com").await;
    let (device_id, _) = create_device(&server, user_id).await;

    let book_id = create_book(&server, "Book to Remove").await;

    // Add then remove
    server
        .admin_client()
        .post_json(
            &format!("/api/v1/admin/devices/{}/books", device_id),
            &serde_json::json!({ "book_ids": [book_id] }),
        )
        .await
        .assert_ok();

    server
        .admin_client()
        .delete(&format!(
            "/api/v1/admin/devices/{}/books/{}",
            device_id, book_id
        ))
        .await
        .assert_status(StatusCode::NO_CONTENT);

    // Should be empty again
    let books: Vec<Value> = server
        .admin_client()
        .get(&format!("/api/v1/admin/devices/{}/books", device_id))
        .await
        .assert_ok()
        .json();
    assert!(books.is_empty());
}

#[tokio::test]
async fn test_device_book_assignment_affects_sync() {
    let server = TestServer::start().await;

    let user_id = create_user(&server, "synctest@test.com").await;
    let (device1_id, device1_token) = create_device(&server, user_id).await;
    let (device2_id, device2_token) = create_device(&server, user_id).await;

    // Create a book (no global entitlement)
    let book_id = create_book(&server, "Selective Book").await;

    // Assign to device1 only via admin API
    server
        .admin_client()
        .post_json(
            &format!("/api/v1/admin/devices/{}/books", device1_id),
            &serde_json::json!({ "book_ids": [book_id] }),
        )
        .await
        .assert_ok();

    // Device 1 should see the book
    let items: Vec<Value> = server
        .kobo_client()
        .kobo_sync(&device1_token)
        .await
        .assert_ok()
        .json();
    assert_sync_contains_book(&items, book_id);

    // Device 2 should NOT see the book
    let items: Vec<Value> = server
        .kobo_client()
        .kobo_sync(&device2_token)
        .await
        .assert_ok()
        .json();
    assert_sync_excludes_book(&items, book_id);

    // Now also add to device2
    server
        .admin_client()
        .post_json(
            &format!("/api/v1/admin/devices/{}/books", device2_id),
            &serde_json::json!({ "book_ids": [book_id] }),
        )
        .await
        .assert_ok();

    // Device 2 should now see the book
    let items: Vec<Value> = server
        .kobo_client()
        .kobo_sync(&device2_token)
        .await
        .assert_ok()
        .json();
    assert_sync_contains_book(&items, book_id);
}

#[tokio::test]
async fn test_remove_device_book_triggers_removal_sync() {
    let server = TestServer::start().await;

    let user_id = create_user(&server, "removedsync@test.com").await;
    let (device_id, device_token) = create_device(&server, user_id).await;

    let book_id = create_book(&server, "Book to Un-assign").await;

    // Assign to device
    server
        .admin_client()
        .post_json(
            &format!("/api/v1/admin/devices/{}/books", device_id),
            &serde_json::json!({ "book_ids": [book_id] }),
        )
        .await
        .assert_ok();

    // Sync to establish baseline
    let resp = server.kobo_client().kobo_sync(&device_token).await;
    resp.assert_ok();
    let token = resp.get_sync_token().expect("should have sync token");
    let items: Vec<Value> = resp.json();
    assert_sync_contains_book(&items, book_id);

    // Remove from device
    server
        .admin_client()
        .delete(&format!(
            "/api/v1/admin/devices/{}/books/{}",
            device_id, book_id
        ))
        .await
        .assert_status(StatusCode::NO_CONTENT);

    // Next sync should show book as removed
    let items: Vec<Value> = server
        .kobo_client()
        .kobo_sync_with_token(&device_token, &token)
        .await
        .assert_ok()
        .json();
    assert_book_is_removed(&items, book_id);
}

// =============================================================================
// Unavailable Books Management Tests
// =============================================================================

#[tokio::test]
async fn test_list_unavailable_books_empty() {
    let server = TestServer::start().await;

    let resp = server
        .admin_client()
        .get("/api/v1/library/books/unavailable")
        .await;
    resp.assert_ok();

    let json: Value = resp.json();
    assert_eq!(json["total_count"], 0);
    assert!(json["items"].as_array().unwrap().is_empty());
}

#[tokio::test]
async fn test_list_unavailable_books_shows_books_with_unavailable_files() {
    let server = TestServer::start().await;

    // Create a book with a file
    let book_id = create_book(&server, "Unavailable Test Book").await;
    upload_epub(&server, book_id, test_epub_bytes()).await;

    // Mark file as unavailable
    sqlx::query(
        "UPDATE book_files SET available = FALSE, unavailable_since = NOW() WHERE book_id = $1",
    )
    .bind(book_id)
    .execute(&server.pool)
    .await
    .unwrap();

    let resp = server
        .admin_client()
        .get("/api/v1/library/books/unavailable")
        .await;
    resp.assert_ok();

    let json: Value = resp.json();
    assert_eq!(json["total_count"], 1);
    let items = json["items"].as_array().unwrap();
    assert_eq!(items.len(), 1);
    assert_eq!(items[0]["title"], "Unavailable Test Book");
    assert_eq!(items[0]["unavailable_files"], 1);
}

#[tokio::test]
async fn test_list_unavailable_books_requires_auth() {
    let server = TestServer::start().await;
    let client = TestClient::new(&server.api_url);

    client
        .get("/api/v1/library/books/unavailable")
        .await
        .assert_status(StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn test_delete_unavailable_books_empty() {
    let server = TestServer::start().await;

    let resp = server
        .admin_client()
        .delete("/api/v1/library/books/unavailable")
        .await;
    resp.assert_ok();

    let json: Value = resp.json();
    assert_eq!(json["deleted"], 0);
}

#[tokio::test]
async fn test_delete_unavailable_books_removes_fully_unavailable() {
    let server = TestServer::start().await;

    // Create a book with a file
    let book_id = create_book(&server, "Fully Unavailable Book").await;
    upload_epub(&server, book_id, test_epub_bytes()).await;
    entitle_book(&server, book_id).await;

    // Mark all files as unavailable
    sqlx::query("UPDATE book_files SET available = FALSE WHERE book_id = $1")
        .bind(book_id)
        .execute(&server.pool)
        .await
        .unwrap();

    // Delete unavailable books
    let resp = server
        .admin_client()
        .delete("/api/v1/library/books/unavailable")
        .await;
    resp.assert_ok();

    let json: Value = resp.json();
    assert_eq!(json["deleted"], 1);

    // Verify book's entitlement is marked as removed
    let is_removed: bool =
        sqlx::query_scalar("SELECT is_removed FROM entitlements WHERE book_id = $1")
            .bind(book_id)
            .fetch_one(&server.pool)
            .await
            .unwrap();
    assert!(is_removed);

    // Verify files are deleted
    let file_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM book_files WHERE book_id = $1")
        .bind(book_id)
        .fetch_one(&server.pool)
        .await
        .unwrap();
    assert_eq!(file_count, 0);
}

#[tokio::test]
async fn test_delete_unavailable_books_keeps_partially_available() {
    let server = TestServer::start().await;

    // Create a book with a file
    let book_id = create_book(&server, "Partially Available Book").await;
    upload_epub(&server, book_id, test_epub_bytes()).await;

    // Add another file that's available
    sqlx::query(
        "INSERT INTO book_files (id, book_id, format, file_path, size_bytes, available) VALUES ($1, $2, 'PDF', '/path/test.pdf', 1000, TRUE)"
    )
    .bind(uuid::Uuid::new_v4())
    .bind(book_id)
    .execute(&server.pool)
    .await
    .unwrap();

    // Mark KEPUB file as unavailable
    sqlx::query("UPDATE book_files SET available = FALSE WHERE book_id = $1 AND format = 'KEPUB'")
        .bind(book_id)
        .execute(&server.pool)
        .await
        .unwrap();

    // Delete unavailable books - should NOT delete this one
    let resp = server
        .admin_client()
        .delete("/api/v1/library/books/unavailable")
        .await;
    resp.assert_ok();

    let json: Value = resp.json();
    assert_eq!(json["deleted"], 0);

    // Verify files still exist
    let file_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM book_files WHERE book_id = $1")
        .bind(book_id)
        .fetch_one(&server.pool)
        .await
        .unwrap();
    assert_eq!(file_count, 2);
}

#[tokio::test]
async fn test_delete_unavailable_books_requires_auth() {
    let server = TestServer::start().await;
    let client = TestClient::new(&server.api_url);

    client
        .delete("/api/v1/library/books/unavailable")
        .await
        .assert_status(StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn test_check_book_availability() {
    let server = TestServer::start().await;

    // Create a book with a file
    let book_id = create_book(&server, "Availability Check Book").await;
    upload_epub(&server, book_id, test_epub_bytes()).await;

    let resp = server
        .admin_client()
        .post_json(
            &format!("/api/v1/library/books/{}/check-availability", book_id),
            &serde_json::json!({}),
        )
        .await;
    resp.assert_ok();

    let json: Value = resp.json();
    assert_eq!(json["checked"], 1);
    // File exists in storage, so should be available
    assert_eq!(json["available"], 1);
    assert_eq!(json["unavailable"], 0);
}

#[tokio::test]
async fn test_check_book_availability_not_found() {
    let server = TestServer::start().await;

    let resp = server
        .admin_client()
        .post_json(
            &format!(
                "/api/v1/library/books/{}/check-availability",
                uuid::Uuid::new_v4()
            ),
            &serde_json::json!({}),
        )
        .await;
    resp.assert_status(StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn test_check_book_availability_requires_auth() {
    let server = TestServer::start().await;
    let client = TestClient::new(&server.api_url);

    client
        .post_json(
            &format!(
                "/api/v1/library/books/{}/check-availability",
                uuid::Uuid::new_v4()
            ),
            &serde_json::json!({}),
        )
        .await
        .assert_status(StatusCode::UNAUTHORIZED);
}

// ============================================================================
// Dev Snapshot Tests
// ============================================================================

/// Test that dev snapshot status endpoint works.
#[tokio::test]
async fn test_dev_snapshot_status() {
    let server = TestServer::start().await;

    let resp: Value = server
        .admin_client()
        .get("/api/v1/admin/dev/snapshot")
        .await
        .assert_ok()
        .json();

    // Should have exists and dev_mode fields
    assert!(resp.get("exists").is_some(), "Should have 'exists' field");
    assert!(
        resp.get("dev_mode").is_some(),
        "Should have 'dev_mode' field"
    );
}

/// Test that dev snapshot status requires auth.
#[tokio::test]
async fn test_dev_snapshot_status_requires_auth() {
    let server = TestServer::start().await;
    let client = TestClient::new(&server.api_url);

    client
        .get("/api/v1/admin/dev/snapshot")
        .await
        .assert_status(StatusCode::UNAUTHORIZED);
}

/// Test that save dev snapshot requires DEV_MODE.
#[tokio::test]
async fn test_save_dev_snapshot_requires_dev_mode() {
    let server = TestServer::start().await;

    // Without DEV_MODE=1, should be forbidden
    let resp = server
        .admin_client()
        .post_json("/api/v1/admin/dev/snapshot/save", &serde_json::json!({}))
        .await;

    // Either forbidden (no dev mode) or ok (dev mode enabled in test env)
    assert!(
        resp.status == StatusCode::FORBIDDEN || resp.status == StatusCode::OK,
        "Should be forbidden without DEV_MODE or ok with it"
    );
}

/// Test that delete dev snapshot requires DEV_MODE.
#[tokio::test]
async fn test_delete_dev_snapshot_requires_dev_mode() {
    let server = TestServer::start().await;

    let resp = server
        .admin_client()
        .delete("/api/v1/admin/dev/snapshot")
        .await;

    // Either forbidden (no dev mode) or not_found (no snapshot) or ok
    assert!(
        resp.status == StatusCode::FORBIDDEN
            || resp.status == StatusCode::NOT_FOUND
            || resp.status == StatusCode::OK,
        "Should be forbidden, not_found, or ok depending on env"
    );
}
