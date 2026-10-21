use crate::common::*;
use reqwest::StatusCode;
use serde_json::Value;

/// Helper: create a book with ALL_USERS entitlement.
async fn create_book_with_all_users_entitlement(server: &TestServer, title: &str) -> uuid::Uuid {
    let book_id = create_book(server, title).await;
    let resp = server
        .admin_client()
        .post_json(
            &format!("/api/v1/admin/books/{}/devices/all", book_id),
            &serde_json::json!({}),
        )
        .await;
    resp.assert_status(StatusCode::NO_CONTENT);
    book_id
}

/// Helper: get book devices response.
async fn get_book_devices(server: &TestServer, book_id: uuid::Uuid) -> Value {
    let resp = server
        .admin_client()
        .get(&format!("/api/v1/admin/books/{}/devices", book_id))
        .await;
    resp.assert_ok();
    resp.json()
}

#[tokio::test]
async fn test_remove_device_converts_all_users_to_per_device() {
    let server = TestServer::start_api_only().await;
    let user_id = create_user(&server, "ent_test@test.com").await;
    let (device_a, _) = create_device(&server, user_id).await;
    let (device_b, _) = create_device(&server, user_id).await;
    let (device_c, _) = create_device(&server, user_id).await;

    let book_id = create_book_with_all_users_entitlement(&server, "All Users Book").await;

    // Verify ALL_USERS is set
    let json = get_book_devices(&server, book_id).await;
    assert!(json["all_users_entitled"].as_bool().unwrap());

    // Remove device_a from the book
    let resp = server
        .admin_client()
        .delete(&format!(
            "/api/v1/admin/books/{}/devices/{}",
            book_id, device_a
        ))
        .await;
    resp.assert_status(StatusCode::NO_CONTENT);

    // After removal: ALL_USERS should be gone, device_b and device_c should have DEVICE entitlements
    let json = get_book_devices(&server, book_id).await;
    assert!(!json["all_users_entitled"].as_bool().unwrap());

    let devices = json["devices"].as_array().unwrap();
    let find_device = |id: uuid::Uuid| -> &Value {
        devices
            .iter()
            .find(|d| d["device_id"].as_str().unwrap() == id.to_string())
            .unwrap()
    };

    // device_b and device_c should be entitled
    assert!(find_device(device_b)["entitled"].as_bool().unwrap());
    assert!(find_device(device_c)["entitled"].as_bool().unwrap());

    // device_a should NOT be entitled (it was the one we removed)
    assert!(!find_device(device_a)["entitled"].as_bool().unwrap());
}

#[tokio::test]
async fn test_remove_device_normal_entitlement() {
    let server = TestServer::start_api_only().await;
    let user_id = create_user(&server, "ent_normal@test.com").await;
    let (device_id, _) = create_device(&server, user_id).await;

    let book_id = create_book(&server, "Device Entitled Book").await;

    // Add DEVICE entitlement
    let resp = server
        .admin_client()
        .post(&format!(
            "/api/v1/admin/books/{}/devices/{}",
            book_id, device_id
        ))
        .await;
    resp.assert_status(StatusCode::NO_CONTENT);

    // Verify entitled
    let json = get_book_devices(&server, book_id).await;
    let devices = json["devices"].as_array().unwrap();
    let dev = devices
        .iter()
        .find(|d| d["device_id"].as_str().unwrap() == device_id.to_string())
        .unwrap();
    assert!(dev["entitled"].as_bool().unwrap());

    // Remove it
    let resp = server
        .admin_client()
        .delete(&format!(
            "/api/v1/admin/books/{}/devices/{}",
            book_id, device_id
        ))
        .await;
    resp.assert_status(StatusCode::NO_CONTENT);

    // Should be marked as removed
    let json = get_book_devices(&server, book_id).await;
    let devices = json["devices"].as_array().unwrap();
    let dev = devices
        .iter()
        .find(|d| d["device_id"].as_str().unwrap() == device_id.to_string())
        .unwrap();
    assert!(dev["is_removed"].as_bool().unwrap());
}

#[tokio::test]
async fn test_remove_device_no_entitlement_returns_404() {
    let server = TestServer::start_api_only().await;
    let user_id = create_user(&server, "ent_404@test.com").await;
    let (device_id, _) = create_device(&server, user_id).await;

    let book_id = create_book(&server, "No Entitlement Book").await;

    // Try to remove without any entitlement
    let resp = server
        .admin_client()
        .delete(&format!(
            "/api/v1/admin/books/{}/devices/{}",
            book_id, device_id
        ))
        .await;
    resp.assert_status(StatusCode::NOT_FOUND);
}
