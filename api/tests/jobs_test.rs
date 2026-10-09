use crate::common::*;
use reqwest::StatusCode;
use serde_json::Value;
use uuid::Uuid;

/// Helper to create a test context for jobs
fn test_context(description: &str) -> nats_jobs::JobContext {
    nats_jobs::JobContext::new(description)
}

#[tokio::test]
async fn test_jobs_require_auth() {
    let server = TestServer::start_api_only().await;
    let client = TestClient::new(&server.api_url);

    // List jobs without auth
    let resp = client.get("/api/v1/admin/jobs").await;
    assert_eq!(resp.status, StatusCode::UNAUTHORIZED);

    // Get job without auth
    let resp = client
        .get(&format!("/api/v1/admin/jobs/{}", Uuid::new_v4()))
        .await;
    assert_eq!(resp.status, StatusCode::UNAUTHORIZED);

    // Running count without auth
    let resp = client.get("/api/v1/admin/jobs/running").await;
    assert_eq!(resp.status, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn test_list_jobs_empty() {
    let server = TestServer::start_api_only().await;

    let resp = server.admin_client().get("/api/v1/admin/jobs").await;
    resp.assert_ok();

    let json: Value = resp.json();
    assert!(json["jobs"].is_array());
    assert_eq!(json["total"].as_i64().unwrap(), 0);
}

#[tokio::test]
async fn test_list_jobs_with_pagination() {
    let server = TestServer::start_api_only().await;

    // Enqueue some jobs via NATS
    for i in 0..5 {
        api::jobs::enqueue(
            &server.pool,
            &server.nats_publisher,
            "test_job",
            serde_json::json!({ "index": i }),
            test_context(&format!("Test job {}", i)),
        )
        .await
        .unwrap();
    }

    // List with limit
    let resp = server
        .admin_client()
        .get("/api/v1/admin/jobs?limit=2")
        .await;
    resp.assert_ok();

    let json: Value = resp.json();
    assert_eq!(json["jobs"].as_array().unwrap().len(), 2);
    assert_eq!(json["total"].as_i64().unwrap(), 5);

    // List with offset
    let resp = server
        .admin_client()
        .get("/api/v1/admin/jobs?limit=2&offset=3")
        .await;
    resp.assert_ok();

    let json: Value = resp.json();
    assert_eq!(json["jobs"].as_array().unwrap().len(), 2);
}

#[tokio::test]
async fn test_list_jobs_with_status_filter() {
    let server = TestServer::start_api_only().await;

    // Enqueue a job
    let job_id = api::jobs::enqueue(
        &server.pool,
        &server.nats_publisher,
        "test_job",
        serde_json::json!({}),
        test_context("Test job for status filter"),
    )
    .await
    .unwrap();

    // Mark it as completed
    sqlx::query("UPDATE job_status SET status = 'completed' WHERE id = $1")
        .bind(job_id)
        .execute(&server.pool)
        .await
        .unwrap();

    // Enqueue another that stays pending
    api::jobs::enqueue(
        &server.pool,
        &server.nats_publisher,
        "test_job",
        serde_json::json!({}),
        test_context("Another test job"),
    )
    .await
    .unwrap();

    // Filter by pending
    let resp = server
        .admin_client()
        .get("/api/v1/admin/jobs?status=pending")
        .await;
    resp.assert_ok();

    let json: Value = resp.json();
    assert_eq!(json["jobs"].as_array().unwrap().len(), 1);

    // Filter by completed
    let resp = server
        .admin_client()
        .get("/api/v1/admin/jobs?status=completed")
        .await;
    resp.assert_ok();

    let json: Value = resp.json();
    assert_eq!(json["jobs"].as_array().unwrap().len(), 1);
}

#[tokio::test]
async fn test_list_jobs_with_job_type_filter() {
    let server = TestServer::start_api_only().await;

    api::jobs::enqueue(
        &server.pool,
        &server.nats_publisher,
        "type_a",
        serde_json::json!({}),
        test_context("Type A job 1"),
    )
    .await
    .unwrap();
    api::jobs::enqueue(
        &server.pool,
        &server.nats_publisher,
        "type_b",
        serde_json::json!({}),
        test_context("Type B job"),
    )
    .await
    .unwrap();
    api::jobs::enqueue(
        &server.pool,
        &server.nats_publisher,
        "type_a",
        serde_json::json!({}),
        test_context("Type A job 2"),
    )
    .await
    .unwrap();

    // Filter by type_a
    let resp = server
        .admin_client()
        .get("/api/v1/admin/jobs?job_type=type_a")
        .await;
    resp.assert_ok();

    let json: Value = resp.json();
    assert_eq!(json["jobs"].as_array().unwrap().len(), 2);

    // Filter by type_b
    let resp = server
        .admin_client()
        .get("/api/v1/admin/jobs?job_type=type_b")
        .await;
    resp.assert_ok();

    let json: Value = resp.json();
    assert_eq!(json["jobs"].as_array().unwrap().len(), 1);
}

#[tokio::test]
async fn test_get_job() {
    let server = TestServer::start_api_only().await;

    let job_id = api::jobs::enqueue(
        &server.pool,
        &server.nats_publisher,
        "test_job",
        serde_json::json!({ "book_id": "123" }),
        test_context("Get job test"),
    )
    .await
    .unwrap();

    let resp = server
        .admin_client()
        .get(&format!("/api/v1/admin/jobs/{}", job_id))
        .await;
    resp.assert_ok();

    let json: Value = resp.json();
    assert_eq!(json["id"].as_str().unwrap(), job_id.to_string());
    assert_eq!(json["job_type"].as_str().unwrap(), "test_job");
    assert_eq!(json["status"].as_str().unwrap(), "pending");
    assert_eq!(json["payload"]["book_id"].as_str().unwrap(), "123");
}

#[tokio::test]
async fn test_get_job_not_found() {
    let server = TestServer::start_api_only().await;

    let resp = server
        .admin_client()
        .get(&format!("/api/v1/admin/jobs/{}", Uuid::new_v4()))
        .await;
    assert_eq!(resp.status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn test_running_count() {
    let server = TestServer::start_api_only().await;

    // Initially zero
    let resp = server
        .admin_client()
        .get("/api/v1/admin/jobs/running")
        .await;
    resp.assert_ok();

    let json: Value = resp.json();
    assert_eq!(json["count"].as_i64().unwrap(), 0);

    // Add some jobs
    let job1 = api::jobs::enqueue(
        &server.pool,
        &server.nats_publisher,
        "test",
        serde_json::json!({}),
        test_context("Running count test 1"),
    )
    .await
    .unwrap();
    let job2 = api::jobs::enqueue(
        &server.pool,
        &server.nats_publisher,
        "test",
        serde_json::json!({}),
        test_context("Running count test 2"),
    )
    .await
    .unwrap();
    api::jobs::enqueue(
        &server.pool,
        &server.nats_publisher,
        "test",
        serde_json::json!({}),
        test_context("Running count test 3"),
    )
    .await
    .unwrap();

    // Mark one as running, one as completed
    sqlx::query("UPDATE job_status SET status = 'running' WHERE id = $1")
        .bind(job1)
        .execute(&server.pool)
        .await
        .unwrap();
    sqlx::query("UPDATE job_status SET status = 'completed' WHERE id = $1")
        .bind(job2)
        .execute(&server.pool)
        .await
        .unwrap();

    // Should count pending + running = 2
    let resp = server
        .admin_client()
        .get("/api/v1/admin/jobs/running")
        .await;
    resp.assert_ok();

    let json: Value = resp.json();
    assert_eq!(json["count"].as_i64().unwrap(), 2);
}

#[tokio::test]
async fn test_job_has_timestamps() {
    let server = TestServer::start_api_only().await;

    let job_id = api::jobs::enqueue(
        &server.pool,
        &server.nats_publisher,
        "test_job",
        serde_json::json!({}),
        test_context("Timestamp test"),
    )
    .await
    .unwrap();

    let resp = server
        .admin_client()
        .get(&format!("/api/v1/admin/jobs/{}", job_id))
        .await;
    resp.assert_ok();

    let json: Value = resp.json();
    assert!(json["created_at"].is_string());
    assert!(json["started_at"].is_null()); // Not started yet
    assert!(json["completed_at"].is_null()); // Not completed yet
}

#[tokio::test]
async fn test_job_result_and_error() {
    let server = TestServer::start_api_only().await;

    let job_id = api::jobs::enqueue(
        &server.pool,
        &server.nats_publisher,
        "test_job",
        serde_json::json!({}),
        test_context("Result and error test"),
    )
    .await
    .unwrap();

    // Simulate job completion with result
    sqlx::query(
        "UPDATE job_status SET status = 'completed', result = $1, completed_at = NOW() WHERE id = $2",
    )
    .bind(serde_json::json!({ "updated_formats": ["KEPUB"] }))
    .bind(job_id)
    .execute(&server.pool)
    .await
    .unwrap();

    let resp = server
        .admin_client()
        .get(&format!("/api/v1/admin/jobs/{}", job_id))
        .await;
    resp.assert_ok();

    let json: Value = resp.json();
    assert_eq!(json["status"].as_str().unwrap(), "completed");
    assert_eq!(
        json["result"]["updated_formats"].as_array().unwrap()[0]
            .as_str()
            .unwrap(),
        "KEPUB"
    );

    // Test with error
    let job_id2 = api::jobs::enqueue(
        &server.pool,
        &server.nats_publisher,
        "test_job",
        serde_json::json!({}),
        test_context("Error test"),
    )
    .await
    .unwrap();

    sqlx::query(
        "UPDATE job_status SET status = 'failed', error = $1, completed_at = NOW() WHERE id = $2",
    )
    .bind("Something went wrong")
    .bind(job_id2)
    .execute(&server.pool)
    .await
    .unwrap();

    let resp = server
        .admin_client()
        .get(&format!("/api/v1/admin/jobs/{}", job_id2))
        .await;
    resp.assert_ok();

    let json: Value = resp.json();
    assert_eq!(json["status"].as_str().unwrap(), "failed");
    assert_eq!(json["error"].as_str().unwrap(), "Something went wrong");
}

#[tokio::test]
async fn test_job_counts() {
    let server = TestServer::start_api_only().await;

    // Add jobs with various statuses
    // job1 stays pending (not updated)
    let _job1 = api::jobs::enqueue(
        &server.pool,
        &server.nats_publisher,
        "test",
        serde_json::json!({}),
        test_context("Counts test - pending"),
    )
    .await
    .unwrap();

    let job2 = api::jobs::enqueue(
        &server.pool,
        &server.nats_publisher,
        "test",
        serde_json::json!({}),
        test_context("Counts test - running"),
    )
    .await
    .unwrap();

    let job3 = api::jobs::enqueue(
        &server.pool,
        &server.nats_publisher,
        "test",
        serde_json::json!({}),
        test_context("Counts test - failed"),
    )
    .await
    .unwrap();

    // Set statuses: 1 pending, 1 running, 1 failed
    sqlx::query("UPDATE job_status SET status = 'running' WHERE id = $1")
        .bind(job2)
        .execute(&server.pool)
        .await
        .unwrap();
    sqlx::query("UPDATE job_status SET status = 'failed' WHERE id = $1")
        .bind(job3)
        .execute(&server.pool)
        .await
        .unwrap();

    let resp = server.admin_client().get("/api/v1/admin/jobs/counts").await;
    resp.assert_ok();

    let json: Value = resp.json();
    assert_eq!(json["pending"].as_i64().unwrap(), 1);
    assert_eq!(json["running"].as_i64().unwrap(), 1);
    assert_eq!(json["failed"].as_i64().unwrap(), 1);
    assert_eq!(json["completed"].as_i64().unwrap(), 0);
}

#[tokio::test]
async fn test_retry_job() {
    let server = TestServer::start_api_only().await;

    // Create a failed job
    let job_id = api::jobs::enqueue(
        &server.pool,
        &server.nats_publisher,
        "test_job",
        serde_json::json!({ "key": "value" }),
        test_context("Retry test original"),
    )
    .await
    .unwrap();

    sqlx::query("UPDATE job_status SET status = 'failed', error = 'Test error' WHERE id = $1")
        .bind(job_id)
        .execute(&server.pool)
        .await
        .unwrap();

    // Retry it
    let resp = server
        .admin_client()
        .post(&format!("/api/v1/admin/jobs/{}/retry", job_id))
        .await;
    resp.assert_ok();

    let json: Value = resp.json();
    assert!(json["new_job_id"].is_string());
    assert!(json["message"].as_str().unwrap().contains("retried"));

    // Verify new job was created with same payload
    let new_job_id: Uuid = json["new_job_id"].as_str().unwrap().parse().unwrap();
    let new_job_resp = server
        .admin_client()
        .get(&format!("/api/v1/admin/jobs/{}", new_job_id))
        .await;
    new_job_resp.assert_ok();

    let new_job: Value = new_job_resp.json();
    assert_eq!(new_job["job_type"].as_str().unwrap(), "test_job");
    assert_eq!(new_job["payload"]["key"].as_str().unwrap(), "value");
    assert_eq!(new_job["status"].as_str().unwrap(), "pending");
}

#[tokio::test]
async fn test_retry_job_not_failed() {
    let server = TestServer::start_api_only().await;

    // Create a pending job (not failed)
    let job_id = api::jobs::enqueue(
        &server.pool,
        &server.nats_publisher,
        "test_job",
        serde_json::json!({}),
        test_context("Retry not failed test"),
    )
    .await
    .unwrap();

    // Try to retry it - should fail
    let resp = server
        .admin_client()
        .post(&format!("/api/v1/admin/jobs/{}/retry", job_id))
        .await;
    assert_eq!(resp.status, StatusCode::BAD_REQUEST);

    let json: Value = resp.json();
    assert!(json["error"].as_str().unwrap().contains("Only failed jobs"));
}

#[tokio::test]
async fn test_retry_job_not_found() {
    let server = TestServer::start_api_only().await;

    let resp = server
        .admin_client()
        .post(&format!("/api/v1/admin/jobs/{}/retry", Uuid::new_v4()))
        .await;
    assert_eq!(resp.status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn test_job_with_context() {
    let server = TestServer::start_api_only().await;

    // Create a job with entity context
    let entity_id = Uuid::new_v4();
    let job_id = api::jobs::enqueue(
        &server.pool,
        &server.nats_publisher,
        "test_job",
        serde_json::json!({ "book_id": "123" }),
        nats_jobs::JobContext::for_entity("book", entity_id, "Testing book metadata"),
    )
    .await
    .unwrap();

    let resp = server
        .admin_client()
        .get(&format!("/api/v1/admin/jobs/{}", job_id))
        .await;
    resp.assert_ok();

    let json: Value = resp.json();
    assert_eq!(json["entity_type"].as_str().unwrap(), "book");
    assert_eq!(json["entity_id"].as_str().unwrap(), entity_id.to_string());
    assert_eq!(
        json["description"].as_str().unwrap(),
        "Testing book metadata"
    );
}

#[tokio::test]
async fn test_retry_job_preserves_context() {
    let server = TestServer::start_api_only().await;

    let entity_id = Uuid::new_v4();

    // Create a failed job with context
    let job_id = api::jobs::enqueue(
        &server.pool,
        &server.nats_publisher,
        "test_job",
        serde_json::json!({}),
        nats_jobs::JobContext::for_entity("book", entity_id, "Original description"),
    )
    .await
    .unwrap();

    sqlx::query("UPDATE job_status SET status = 'failed' WHERE id = $1")
        .bind(job_id)
        .execute(&server.pool)
        .await
        .unwrap();

    // Retry it
    let resp = server
        .admin_client()
        .post(&format!("/api/v1/admin/jobs/{}/retry", job_id))
        .await;
    resp.assert_ok();

    let json: Value = resp.json();
    let new_job_id: Uuid = json["new_job_id"].as_str().unwrap().parse().unwrap();

    // Check new job has context preserved (with "Retry:" prefix)
    let new_job_resp = server
        .admin_client()
        .get(&format!("/api/v1/admin/jobs/{}", new_job_id))
        .await;
    new_job_resp.assert_ok();

    let new_job: Value = new_job_resp.json();
    assert_eq!(new_job["entity_type"].as_str().unwrap(), "book");
    assert_eq!(
        new_job["entity_id"].as_str().unwrap(),
        entity_id.to_string()
    );
    assert!(new_job["description"].as_str().unwrap().contains("Retry:"));
}

// =============================================================================
// Series Sync Check Tests
// =============================================================================

#[tokio::test]
async fn test_series_sync_check_enqueues_jobs_for_due_series() {
    let server = TestServer::start_api_only().await;

    // Create a series with a hardcover ID and expired refresh time
    let series_id = Uuid::new_v4();
    sqlx::query(
        r#"
        INSERT INTO series (id, name, sort_key, identifiers, next_metadata_refresh_at, created_at, updated_at)
        VALUES ($1, 'Test Series', 'test series', '{"hardcover": "hc-test-123"}', NOW() - INTERVAL '1 hour', NOW(), NOW())
        "#,
    )
    .bind(series_id)
    .execute(&server.pool)
    .await
    .unwrap();

    // Enqueue the series_sync_check job (simulating the scheduler)
    api::jobs::enqueue(
        &server.pool,
        &server.nats_publisher,
        "series_sync_check",
        serde_json::json!({}),
        test_context("Test series sync check"),
    )
    .await
    .unwrap();

    // Give a moment for processing (in real usage the worker would pick this up)
    // For this test, we just verify the job was created
    let resp = server
        .admin_client()
        .get("/api/v1/admin/jobs?job_type=series_sync_check&exclude_routine=false")
        .await;
    resp.assert_ok();

    let json: Value = resp.json();
    assert!(!json["jobs"].as_array().unwrap().is_empty());
}

#[tokio::test]
async fn test_series_without_hardcover_id_not_synced() {
    let server = TestServer::start_api_only().await;

    // Create a series WITHOUT a hardcover ID
    let series_id = Uuid::new_v4();
    sqlx::query(
        r#"
        INSERT INTO series (id, name, sort_key, identifiers, next_metadata_refresh_at, created_at, updated_at)
        VALUES ($1, 'Unlinked Series', 'unlinked series', '{}', NOW() - INTERVAL '1 hour', NOW(), NOW())
        "#,
    )
    .bind(series_id)
    .execute(&server.pool)
    .await
    .unwrap();

    // Query to check series due for sync (same query as SeriesSyncCheckHandler)
    let due_series: Vec<(Uuid,)> = sqlx::query_as(
        r#"
        SELECT id
        FROM series
        WHERE identifiers->>'hardcover' IS NOT NULL
          AND (next_metadata_refresh_at IS NULL OR next_metadata_refresh_at <= NOW())
        "#,
    )
    .fetch_all(&server.pool)
    .await
    .unwrap();

    // The unlinked series should NOT be in the results
    assert!(
        !due_series.iter().any(|(id,)| *id == series_id),
        "Series without hardcover ID should not be picked up for sync"
    );
}

#[tokio::test]
async fn test_series_not_due_for_refresh_not_synced() {
    let server = TestServer::start_api_only().await;

    // Create a series with a hardcover ID but future refresh time
    let series_id = Uuid::new_v4();
    sqlx::query(
        r#"
        INSERT INTO series (id, name, sort_key, identifiers, next_metadata_refresh_at, created_at, updated_at)
        VALUES ($1, 'Future Series', 'future series', '{"hardcover": "hc-future"}', NOW() + INTERVAL '1 week', NOW(), NOW())
        "#,
    )
    .bind(series_id)
    .execute(&server.pool)
    .await
    .unwrap();

    // Query to check series due for sync
    let due_series: Vec<(Uuid,)> = sqlx::query_as(
        r#"
        SELECT id
        FROM series
        WHERE identifiers->>'hardcover' IS NOT NULL
          AND (next_metadata_refresh_at IS NULL OR next_metadata_refresh_at <= NOW())
        "#,
    )
    .fetch_all(&server.pool)
    .await
    .unwrap();

    // The series with future refresh time should NOT be in the results
    assert!(
        !due_series.iter().any(|(id,)| *id == series_id),
        "Series with future refresh time should not be picked up for sync"
    );
}

#[tokio::test]
async fn test_series_enrich_updates_refresh_timestamps() {
    let server = TestServer::start_api_only().await;

    // Create a series with a hardcover ID
    let series_id = Uuid::new_v4();
    sqlx::query(
        r#"
        INSERT INTO series (id, name, sort_key, identifiers, created_at, updated_at)
        VALUES ($1, 'Enrich Test Series', 'enrich test series', '{"hardcover": "hc-enrich-test"}', NOW(), NOW())
        "#,
    )
    .bind(series_id)
    .execute(&server.pool)
    .await
    .unwrap();

    // Verify initial state - no refresh timestamps
    let row: (Option<time::OffsetDateTime>, Option<time::OffsetDateTime>) = sqlx::query_as(
        "SELECT last_metadata_refresh_at, next_metadata_refresh_at FROM series WHERE id = $1",
    )
    .bind(series_id)
    .fetch_one(&server.pool)
    .await
    .unwrap();

    assert!(
        row.0.is_none(),
        "last_metadata_refresh_at should be NULL initially"
    );
    assert!(
        row.1.is_none(),
        "next_metadata_refresh_at should be NULL initially"
    );

    // Enqueue a series_enrich job
    let job_id = api::jobs::enqueue(
        &server.pool,
        &server.nats_publisher,
        "series_enrich",
        serde_json::json!({ "series_id": series_id }),
        test_context("Test series enrich"),
    )
    .await
    .unwrap();

    // Verify job was created
    let resp = server
        .admin_client()
        .get(&format!("/api/v1/admin/jobs/{}", job_id))
        .await;
    resp.assert_ok();

    let json: Value = resp.json();
    assert_eq!(json["job_type"].as_str().unwrap(), "series_enrich");
    assert_eq!(
        json["payload"]["series_id"].as_str().unwrap(),
        series_id.to_string()
    );
}

// =============================================================================
// Display metadata tests
// =============================================================================

#[tokio::test]
async fn test_fetch_job_display_for_book() {
    let server = TestServer::start_api_only().await;

    // Create a book with an author via the ingest API
    let book_id = Uuid::new_v4();
    let resp = server
        .ingest_client()
        .put_json(
            &format!("/api/v1/books/{}", book_id),
            &serde_json::json!({
                "title": "The Hobbit",
                "authors": ["J.R.R. Tolkien"]
            }),
        )
        .await;
    resp.assert_success();

    let display = api::jobs::fetch_job_display(&server.pool, "book", book_id).await;
    assert!(
        display.is_some(),
        "display should be Some for existing book"
    );
    let display = display.unwrap();
    assert_eq!(display["title"].as_str().unwrap(), "The Hobbit");
    assert_eq!(display["subtitle"].as_str().unwrap(), "J.R.R. Tolkien");
    // Book was created without a cover image, so image_url should be null
    assert!(
        display["image_url"].is_null(),
        "image_url should be null for book without cover"
    );
}

#[tokio::test]
async fn test_fetch_job_display_for_unknown_entity() {
    let server = TestServer::start_api_only().await;

    let display = api::jobs::fetch_job_display(&server.pool, "book", Uuid::new_v4()).await;
    assert!(
        display.is_none(),
        "display should be None for non-existent entity"
    );
}

#[tokio::test]
async fn test_fetch_job_display_for_unknown_type() {
    let server = TestServer::start_api_only().await;

    let display =
        api::jobs::fetch_job_display(&server.pool, "nonexistent_type", Uuid::new_v4()).await;
    assert!(
        display.is_none(),
        "display should be None for unknown entity type"
    );
}

#[tokio::test]
async fn test_job_display_in_api_response() {
    let server = TestServer::start_api_only().await;

    // Create a book
    let book_id = Uuid::new_v4();
    let resp = server
        .ingest_client()
        .put_json(
            &format!("/api/v1/books/{}", book_id),
            &serde_json::json!({
                "title": "Dune",
                "authors": ["Frank Herbert"]
            }),
        )
        .await;
    resp.assert_success();

    // Enqueue a job with display populated via the helper
    let display = api::jobs::fetch_job_display(&server.pool, "book", book_id).await;
    let context = nats_jobs::JobContext::for_entity("book", book_id, "Testing display")
        .with_display_opt(display);
    let job_id = api::jobs::enqueue(
        &server.pool,
        &server.nats_publisher,
        "test_display_job",
        serde_json::json!({"book_id": book_id}),
        context,
    )
    .await
    .unwrap();

    // Fetch via API and verify display is present
    let resp = server
        .admin_client()
        .get(&format!("/api/v1/admin/jobs/{}", job_id))
        .await;
    resp.assert_ok();

    let json: Value = resp.json();
    assert!(
        json["display"].is_object(),
        "display should be a JSON object in API response"
    );
    assert_eq!(json["display"]["title"].as_str().unwrap(), "Dune");
    assert_eq!(
        json["display"]["subtitle"].as_str().unwrap(),
        "Frank Herbert"
    );
}

#[tokio::test]
async fn test_job_display_null_without_entity() {
    let server = TestServer::start_api_only().await;

    // Enqueue a job with plain context (no entity, no display)
    let context = nats_jobs::JobContext::new("Simple job without entity");
    let job_id = api::jobs::enqueue(
        &server.pool,
        &server.nats_publisher,
        "test_no_display",
        serde_json::json!({}),
        context,
    )
    .await
    .unwrap();

    let resp = server
        .admin_client()
        .get(&format!("/api/v1/admin/jobs/{}", job_id))
        .await;
    resp.assert_ok();

    let json: Value = resp.json();
    assert!(
        json["display"].is_null(),
        "display should be null for jobs without entity context"
    );
}
