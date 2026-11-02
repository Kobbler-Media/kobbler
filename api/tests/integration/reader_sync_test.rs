use crate::common::*;
use reqwest::StatusCode;
use serde_json::{Value, json};

/// Helper: get the admin user ID from the database.
/// Helper: enable hardcover sync for the admin user.
async fn enable_hardcover_for_admin(server: &TestServer) {
    let user_id = get_admin_user_id(server).await;
    let config = serde_json::json!({"api_key": "test-key"});
    api::repositories::user_sync_providers::upsert(
        &server.pool,
        user_id,
        "hardcover",
        true,
        &config,
    )
    .await
    .expect("Failed to enable hardcover");
}

/// Helper: get sync job action payloads for a book.
async fn get_sync_actions(server: &TestServer, book_id: uuid::Uuid) -> Vec<Value> {
    let rows: Vec<(Value,)> = sqlx::query_as(
        "SELECT payload->'action' FROM job_status WHERE job_type = 'sync_provider' AND payload->>'book_id' = $1 ORDER BY created_at"
    )
    .bind(book_id.to_string())
    .fetch_all(&server.pool)
    .await
    .unwrap_or_default();

    rows.into_iter().map(|(v,)| v).collect()
}

/// Helper: get action type strings from sync jobs.
fn action_types(actions: &[Value]) -> Vec<String> {
    actions
        .iter()
        .map(|v| {
            v.get("type")
                .and_then(|t| t.as_str())
                .unwrap_or("unknown")
                .to_string()
        })
        .collect()
}

#[tokio::test]
async fn test_reader_closing_queues_progress_sync() {
    let server = TestServer::start_api_only().await;
    let book_id = create_book(&server, "Sync Test Book").await;
    upload_epub(&server, book_id, test_epub_bytes()).await;

    // Enable hardcover for the admin user
    enable_hardcover_for_admin(&server).await;

    // Save progress with closing = true (first save: ReadyToRead → Reading)
    server
        .admin_client()
        .post_json(
            &format!("/api/v1/admin/books/{}/reader-progress", book_id),
            &json!({
                "progress_percent": 25.0,
                "cfi": "epubcfi(/6/10)",
                "closing": true
            }),
        )
        .await
        .assert_status(StatusCode::NO_CONTENT);

    // Should have: Progress + Started (both triggered by closing save)
    let actions = get_sync_actions(&server, book_id).await;
    let types = action_types(&actions);
    assert!(
        types.contains(&"progress".to_string()),
        "Closing should queue progress sync, got: {:?}",
        types
    );
    assert!(
        types.contains(&"started".to_string()),
        "Should queue started sync on first read, got: {:?}",
        types
    );

    // Verify progress action has started_at field
    let progress_action = actions
        .iter()
        .find(|a| a.get("type").and_then(|t| t.as_str()) == Some("progress"))
        .expect("Should have progress action");
    assert!(
        progress_action.get("started_at").is_some(),
        "Progress action should include started_at, got: {:?}",
        progress_action
    );

    // Verify no reading_time actions
    assert!(
        !types.contains(&"reading_time".to_string()),
        "Should NOT queue reading_time (removed), got: {:?}",
        types
    );
}

#[tokio::test]
async fn test_reader_no_progress_sync_without_closing() {
    let server = TestServer::start_api_only().await;
    let book_id = create_book(&server, "No Close Book").await;
    upload_epub(&server, book_id, test_epub_bytes()).await;
    enable_hardcover_for_admin(&server).await;

    // First save to set status to Reading (triggers Started sync)
    server
        .admin_client()
        .post_json(
            &format!("/api/v1/admin/books/{}/reader-progress", book_id),
            &json!({ "progress_percent": 10.0 }),
        )
        .await
        .assert_status(StatusCode::NO_CONTENT);

    // Save without closing — should NOT sync progress
    server
        .admin_client()
        .post_json(
            &format!("/api/v1/admin/books/{}/reader-progress", book_id),
            &json!({
                "progress_percent": 50.0,
                "minutes_since_last_save": 10
            }),
        )
        .await
        .assert_status(StatusCode::NO_CONTENT);

    let actions = get_sync_actions(&server, book_id).await;
    let types = action_types(&actions);
    // Only Started should be present (from first save's status transition)
    assert!(
        !types.contains(&"progress".to_string()),
        "Should NOT queue progress without closing, got: {:?}",
        types
    );
    assert!(
        !types.contains(&"reading_time".to_string()),
        "Should NOT queue reading_time (removed), got: {:?}",
        types
    );
}

#[tokio::test]
async fn test_reader_status_transition_to_finished() {
    let server = TestServer::start_api_only().await;
    let book_id = create_book(&server, "Finish Book").await;
    upload_epub(&server, book_id, test_epub_bytes()).await;
    enable_hardcover_for_admin(&server).await;

    // First save: sets status to Reading
    server
        .admin_client()
        .post_json(
            &format!("/api/v1/admin/books/{}/reader-progress", book_id),
            &json!({ "progress_percent": 50.0 }),
        )
        .await
        .assert_status(StatusCode::NO_CONTENT);

    // Second save: 100% → Finished
    server
        .admin_client()
        .post_json(
            &format!("/api/v1/admin/books/{}/reader-progress", book_id),
            &json!({ "progress_percent": 100.0 }),
        )
        .await
        .assert_status(StatusCode::NO_CONTENT);

    let actions = get_sync_actions(&server, book_id).await;
    let types = action_types(&actions);
    assert!(
        types.contains(&"finished".to_string()),
        "Should queue finished sync at 100%, got: {:?}",
        actions
    );
}

#[tokio::test]
async fn test_reading_time_accumulates_not_replaces() {
    let server = TestServer::start_api_only().await;
    let book_id = create_book(&server, "Accumulate Book").await;
    upload_epub(&server, book_id, test_epub_bytes()).await;

    // Save progress with 5 minutes
    server
        .admin_client()
        .post_json(
            &format!("/api/v1/admin/books/{}/reader-progress", book_id),
            &json!({ "progress_percent": 10.0, "minutes_since_last_save": 5 }),
        )
        .await
        .assert_status(StatusCode::NO_CONTENT);

    // Save again with 3 more minutes
    server
        .admin_client()
        .post_json(
            &format!("/api/v1/admin/books/{}/reader-progress", book_id),
            &json!({ "progress_percent": 20.0, "minutes_since_last_save": 3 }),
        )
        .await
        .assert_status(StatusCode::NO_CONTENT);

    // Check total reading time = 8 (not 3)
    let total: Option<i32> = sqlx::query_scalar(
        r#"
        SELECT stats.spent_reading_minutes
        FROM reading_states rs
        JOIN reading_statistics stats ON stats.reading_state_id = rs.id
        WHERE rs.book_id = $1
        ORDER BY rs.updated_at DESC
        LIMIT 1
        "#,
    )
    .bind(book_id)
    .fetch_optional(&server.pool)
    .await
    .expect("query failed");

    assert_eq!(total, Some(8), "Reading time should accumulate (5 + 3 = 8)");
}

#[tokio::test]
async fn test_provider_read_id_storage() {
    let server = TestServer::start_api_only().await;
    let book_id = create_book(&server, "Read ID Book").await;
    let user_id = get_admin_user_id(&server).await;

    // Initially no read ID stored
    let read_id = api::repositories::user_sync_providers::get_provider_read_id(
        &server.pool,
        user_id,
        book_id,
        "hardcover",
    )
    .await
    .expect("get_provider_read_id failed");
    assert!(read_id.is_none(), "Should start with no read ID");

    // Store a read ID
    api::repositories::user_sync_providers::set_provider_read_id(
        &server.pool,
        user_id,
        book_id,
        "hardcover",
        12345,
    )
    .await
    .expect("set_provider_read_id failed");

    // Verify it's stored
    let read_id = api::repositories::user_sync_providers::get_provider_read_id(
        &server.pool,
        user_id,
        book_id,
        "hardcover",
    )
    .await
    .expect("get_provider_read_id failed");
    assert_eq!(read_id, Some(12345), "Should store the read ID");

    // Clear it (as happens on finish)
    api::repositories::user_sync_providers::clear_provider_read_id(
        &server.pool,
        user_id,
        book_id,
        "hardcover",
    )
    .await
    .expect("clear_provider_read_id failed");

    let read_id = api::repositories::user_sync_providers::get_provider_read_id(
        &server.pool,
        user_id,
        book_id,
        "hardcover",
    )
    .await
    .expect("get_provider_read_id failed");
    assert!(read_id.is_none(), "Should be cleared after finish");
}

#[tokio::test]
async fn test_no_sync_without_hardcover() {
    let server = TestServer::start_api_only().await;
    let book_id = create_book(&server, "No Hardcover Book").await;
    upload_epub(&server, book_id, test_epub_bytes()).await;

    // Don't enable hardcover — save progress
    server
        .admin_client()
        .post_json(
            &format!("/api/v1/admin/books/{}/reader-progress", book_id),
            &json!({ "progress_percent": 50.0 }),
        )
        .await
        .assert_status(StatusCode::NO_CONTENT);

    let actions = get_sync_actions(&server, book_id).await;
    assert!(
        actions.is_empty(),
        "Should not queue sync jobs without hardcover enabled, got: {:?}",
        action_types(&actions)
    );
}

/// Helper: get sync job payloads for a book (full payloads, not just actions).
async fn get_sync_payloads(server: &TestServer, book_id: uuid::Uuid) -> Vec<Value> {
    let rows: Vec<(Value,)> = sqlx::query_as(
        "SELECT payload FROM job_status WHERE job_type = 'sync_provider' AND payload->>'book_id' = $1 ORDER BY created_at",
    )
    .bind(book_id.to_string())
    .fetch_all(&server.pool)
    .await
    .unwrap_or_default();

    rows.into_iter().map(|(v,)| v).collect()
}

#[tokio::test]
async fn test_sync_job_includes_user_id() {
    // This test verifies that sync jobs from the admin web reader API include user_id.
    // The admin API uses queue_sync_job_for_user() directly with the authenticated user.
    // See reading_state_sync_test.rs::kobo_reading_state_sync_job_includes_user_id
    // for the Kobo device path test (which was the buggy code path).
    let server = TestServer::start_api_only().await;
    let book_id = create_book(&server, "User ID Test Book").await;
    upload_epub(&server, book_id, test_epub_bytes()).await;

    let user_id = get_admin_user_id(&server).await;
    enable_hardcover_for_admin(&server).await;

    // Save progress with closing = true to trigger sync
    server
        .admin_client()
        .post_json(
            &format!("/api/v1/admin/books/{}/reader-progress", book_id),
            &json!({
                "progress_percent": 25.0,
                "closing": true
            }),
        )
        .await
        .assert_status(StatusCode::NO_CONTENT);

    // Check that all sync job payloads have user_id set
    let payloads = get_sync_payloads(&server, book_id).await;
    assert!(!payloads.is_empty(), "Should have queued sync jobs");

    for payload in &payloads {
        let payload_user_id = payload.get("user_id").and_then(|v| v.as_str());
        assert!(
            payload_user_id.is_some(),
            "Sync job payload must include user_id for read ID lookup, got: {:?}",
            payload
        );
        assert_eq!(
            payload_user_id.unwrap(),
            user_id.to_string(),
            "Sync job user_id should match the device owner"
        );
    }
}
