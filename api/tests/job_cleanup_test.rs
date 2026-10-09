use crate::common::*;

/// Helper to create a test context for jobs
fn test_context(description: &str) -> nats_jobs::JobContext {
    nats_jobs::JobContext::new(description)
}

/// The stale-pending cleanup SQL, mirroring JobCleanupHandler exactly.
const STALE_CLEANUP_SQL: &str = r#"
    UPDATE job_status
    SET status = 'failed',
        error = 'Job timed out - marked as stale',
        completed_at = NOW()
    WHERE status = 'pending'
      AND created_at < NOW() - INTERVAL '24 hours'
      AND job_type NOT IN (
        SELECT DISTINCT job_type FROM job_status
        WHERE status = 'running'
           OR (status = 'completed' AND completed_at > NOW() - INTERVAL '24 hours')
           OR (status = 'pending' AND created_at > NOW() - INTERVAL '4 hours')
      )
"#;

/// The old-job deletion SQL, mirroring JobCleanupHandler exactly.
const DELETION_SQL: &str = r#"
    DELETE FROM job_status
    WHERE status IN ('completed', 'failed')
      AND completed_at < NOW() - INTERVAL '90 days'
"#;

// =============================================================================
// Job Cleanup Tests
// =============================================================================

#[tokio::test]
async fn test_cleanup_skips_pending_jobs_with_active_type() {
    let server = TestServer::start_api_only().await;

    // Create old pending jobs of type "media_probe"
    let old_pending = api::jobs::enqueue(
        &server.pool,
        &server.nats_publisher,
        "media_probe",
        serde_json::json!({ "file_id": "aaaaaaaa-0000-0000-0000-000000000001" }),
        test_context("Old pending probe"),
    )
    .await
    .unwrap();

    // Age it past the 24-hour threshold
    sqlx::query("UPDATE job_status SET created_at = NOW() - INTERVAL '25 hours' WHERE id = $1")
        .bind(old_pending)
        .execute(&server.pool)
        .await
        .unwrap();

    // Create a running job of the SAME type — this should protect the pending one
    let running_job = api::jobs::enqueue(
        &server.pool,
        &server.nats_publisher,
        "media_probe",
        serde_json::json!({ "file_id": "aaaaaaaa-0000-0000-0000-000000000002" }),
        test_context("Running probe"),
    )
    .await
    .unwrap();
    sqlx::query("UPDATE job_status SET status = 'running' WHERE id = $1")
        .bind(running_job)
        .execute(&server.pool)
        .await
        .unwrap();

    // Run the cleanup query (same SQL as JobCleanupHandler)
    let result = sqlx::query(STALE_CLEANUP_SQL)
        .execute(&server.pool)
        .await
        .unwrap();

    assert_eq!(
        result.rows_affected(),
        0,
        "Should NOT clean up pending jobs when same type has a running job"
    );

    // Verify the pending job is still pending
    let row: (String,) = sqlx::query_as("SELECT status FROM job_status WHERE id = $1")
        .bind(old_pending)
        .fetch_one(&server.pool)
        .await
        .unwrap();
    assert_eq!(row.0, "pending");
}

#[tokio::test]
async fn test_cleanup_skips_pending_jobs_with_recent_completions() {
    let server = TestServer::start_api_only().await;

    // Create old pending jobs of type "media_probe"
    let old_pending = api::jobs::enqueue(
        &server.pool,
        &server.nats_publisher,
        "media_probe",
        serde_json::json!({ "file_id": "bbbbbbbb-0000-0000-0000-000000000001" }),
        test_context("Old pending probe"),
    )
    .await
    .unwrap();
    sqlx::query("UPDATE job_status SET created_at = NOW() - INTERVAL '25 hours' WHERE id = $1")
        .bind(old_pending)
        .execute(&server.pool)
        .await
        .unwrap();

    // Create a recently completed job of the SAME type — this should also protect
    let completed_job = api::jobs::enqueue(
        &server.pool,
        &server.nats_publisher,
        "media_probe",
        serde_json::json!({ "file_id": "bbbbbbbb-0000-0000-0000-000000000002" }),
        test_context("Recently completed probe"),
    )
    .await
    .unwrap();
    sqlx::query(
        "UPDATE job_status SET status = 'completed', completed_at = NOW() - INTERVAL '12 hours' WHERE id = $1",
    )
    .bind(completed_job)
    .execute(&server.pool)
    .await
    .unwrap();

    // Run cleanup
    let result = sqlx::query(STALE_CLEANUP_SQL)
        .execute(&server.pool)
        .await
        .unwrap();

    assert_eq!(
        result.rows_affected(),
        0,
        "Should NOT clean up pending jobs when same type has recent completions"
    );
}

#[tokio::test]
async fn test_cleanup_kills_truly_stale_pending_jobs() {
    let server = TestServer::start_api_only().await;

    // Create old pending jobs of type "stale_type" with no activity
    let stale_job = api::jobs::enqueue(
        &server.pool,
        &server.nats_publisher,
        "stale_type",
        serde_json::json!({}),
        test_context("Stale pending job"),
    )
    .await
    .unwrap();
    sqlx::query("UPDATE job_status SET created_at = NOW() - INTERVAL '25 hours' WHERE id = $1")
        .bind(stale_job)
        .execute(&server.pool)
        .await
        .unwrap();

    // No running or recently-completed jobs of this type exist

    // Run cleanup
    let result = sqlx::query(STALE_CLEANUP_SQL)
        .execute(&server.pool)
        .await
        .unwrap();

    assert_eq!(
        result.rows_affected(),
        1,
        "Should clean up stale pending jobs with no active processing"
    );

    // Verify it was marked failed
    let row: (String,) = sqlx::query_as("SELECT status FROM job_status WHERE id = $1")
        .bind(stale_job)
        .fetch_one(&server.pool)
        .await
        .unwrap();
    assert_eq!(row.0, "failed");
}

#[tokio::test]
async fn test_cleanup_deletes_old_completed_jobs() {
    let server = TestServer::start_api_only().await;

    // Create a completed job older than 90 days
    let old_job = api::jobs::enqueue(
        &server.pool,
        &server.nats_publisher,
        "old_type",
        serde_json::json!({}),
        test_context("Old completed job"),
    )
    .await
    .unwrap();
    sqlx::query(
        "UPDATE job_status SET status = 'completed', completed_at = NOW() - INTERVAL '100 days' WHERE id = $1",
    )
    .bind(old_job)
    .execute(&server.pool)
    .await
    .unwrap();

    // Create a recent completed job (should survive)
    let recent_job = api::jobs::enqueue(
        &server.pool,
        &server.nats_publisher,
        "recent_type",
        serde_json::json!({}),
        test_context("Recent completed job"),
    )
    .await
    .unwrap();
    sqlx::query(
        "UPDATE job_status SET status = 'completed', completed_at = NOW() - INTERVAL '30 days' WHERE id = $1",
    )
    .bind(recent_job)
    .execute(&server.pool)
    .await
    .unwrap();

    // Run the deletion query (same as JobCleanupHandler)
    let deleted = sqlx::query(DELETION_SQL)
        .execute(&server.pool)
        .await
        .unwrap()
        .rows_affected();

    assert_eq!(
        deleted, 1,
        "Should delete old completed job but not recent one"
    );

    // Verify recent job still exists
    let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM job_status WHERE id = $1")
        .bind(recent_job)
        .fetch_one(&server.pool)
        .await
        .unwrap();
    assert_eq!(count, 1);
}

// =============================================================================
// Orphaned Running Job Cleanup Tests
// =============================================================================

#[tokio::test]
async fn test_reset_orphaned_running_jobs_marks_stale_as_failed() {
    let server = TestServer::start_api_only().await;
    let db = nats_jobs::JobStatusDb::new(server.pool.clone());

    // Create a running job and backdate its updated_at past the threshold
    let stale_running = api::jobs::enqueue(
        &server.pool,
        &server.nats_publisher,
        "media_probe",
        serde_json::json!({}),
        test_context("Stale running job"),
    )
    .await
    .unwrap();
    sqlx::query(
        "UPDATE job_status SET status = 'running', started_at = NOW() - INTERVAL '1 hour', updated_at = NOW() - INTERVAL '1 hour' WHERE id = $1",
    )
    .bind(stale_running)
    .execute(&server.pool)
    .await
    .unwrap();

    let orphaned = db.reset_orphaned_running_jobs(30).await.unwrap();
    assert_eq!(orphaned, 1, "Should reset one stale running job");

    let row: (String, Option<String>) =
        sqlx::query_as("SELECT status, error FROM job_status WHERE id = $1")
            .bind(stale_running)
            .fetch_one(&server.pool)
            .await
            .unwrap();
    assert_eq!(row.0, "failed");
    assert!(
        row.1.as_deref().unwrap().contains("Orphaned"),
        "Error should mention orphaned"
    );
}

#[tokio::test]
async fn test_reset_orphaned_running_jobs_skips_recent() {
    let server = TestServer::start_api_only().await;
    let db = nats_jobs::JobStatusDb::new(server.pool.clone());

    // Create a running job with a recent updated_at (still actively processing)
    let active_running = api::jobs::enqueue(
        &server.pool,
        &server.nats_publisher,
        "media_probe",
        serde_json::json!({}),
        test_context("Active running job"),
    )
    .await
    .unwrap();
    sqlx::query("UPDATE job_status SET status = 'running', started_at = NOW() WHERE id = $1")
        .bind(active_running)
        .execute(&server.pool)
        .await
        .unwrap();

    let orphaned = db.reset_orphaned_running_jobs(30).await.unwrap();
    assert_eq!(orphaned, 0, "Should NOT reset recently-updated running job");

    let row: (String,) = sqlx::query_as("SELECT status FROM job_status WHERE id = $1")
        .bind(active_running)
        .fetch_one(&server.pool)
        .await
        .unwrap();
    assert_eq!(row.0, "running");
}

// =============================================================================
// Startup Reset Tests
// =============================================================================

#[tokio::test]
async fn test_startup_reset_running_to_failed() {
    let server = TestServer::start_api_only().await;

    // Create jobs in various states
    let running_job = api::jobs::enqueue(
        &server.pool,
        &server.nats_publisher,
        "media_probe",
        serde_json::json!({}),
        test_context("Orphaned running job"),
    )
    .await
    .unwrap();
    sqlx::query("UPDATE job_status SET status = 'running', started_at = NOW() WHERE id = $1")
        .bind(running_job)
        .execute(&server.pool)
        .await
        .unwrap();

    let pending_job = api::jobs::enqueue(
        &server.pool,
        &server.nats_publisher,
        "media_probe",
        serde_json::json!({}),
        test_context("Already pending job"),
    )
    .await
    .unwrap();

    let completed_job = api::jobs::enqueue(
        &server.pool,
        &server.nats_publisher,
        "media_probe",
        serde_json::json!({}),
        test_context("Completed job"),
    )
    .await
    .unwrap();
    sqlx::query("UPDATE job_status SET status = 'completed', completed_at = NOW() WHERE id = $1")
        .bind(completed_job)
        .execute(&server.pool)
        .await
        .unwrap();

    // Run the startup reset (same SQL as nats.rs)
    let count: i64 = sqlx::query_scalar(
        r#"
        WITH updated AS (
            UPDATE job_status
            SET status = 'failed',
                error = 'Orphaned on startup — worker restarted',
                completed_at = NOW()
            WHERE status = 'running'
            RETURNING 1
        )
        SELECT COUNT(*) as "count!" FROM updated
        "#,
    )
    .fetch_one(&server.pool)
    .await
    .unwrap();

    assert_eq!(count, 1, "Should reset exactly one running job");

    // Verify the running job is now failed with error message
    let row: (String, Option<String>) =
        sqlx::query_as("SELECT status, error FROM job_status WHERE id = $1")
            .bind(running_job)
            .fetch_one(&server.pool)
            .await
            .unwrap();
    assert_eq!(row.0, "failed");
    assert_eq!(
        row.1.as_deref(),
        Some("Orphaned on startup — worker restarted")
    );

    // Verify pending job is still pending
    let row: (String,) = sqlx::query_as("SELECT status FROM job_status WHERE id = $1")
        .bind(pending_job)
        .fetch_one(&server.pool)
        .await
        .unwrap();
    assert_eq!(row.0, "pending");

    // Verify completed job is still completed
    let row: (String,) = sqlx::query_as("SELECT status FROM job_status WHERE id = $1")
        .bind(completed_job)
        .fetch_one(&server.pool)
        .await
        .unwrap();
    assert_eq!(row.0, "completed");
}

// =============================================================================
// Deploy Safety: Bulk Backlog Scenario
// =============================================================================

#[tokio::test]
async fn test_bulk_backlog_survives_cleanup() {
    let server = TestServer::start_api_only().await;

    // Simulate a bulk import: 20 pending media_probe jobs, some old
    let mut job_ids = Vec::new();
    for i in 0..20 {
        let job_id = api::jobs::enqueue(
            &server.pool,
            &server.nats_publisher,
            "media_probe",
            serde_json::json!({ "file_id": format!("cccccccc-0000-0000-0000-{:012}", i) }),
            test_context(&format!("Bulk probe {}", i)),
        )
        .await
        .unwrap();
        job_ids.push(job_id);
    }

    // Age all pending jobs past 24 hours (simulating a long queue)
    sqlx::query(
        "UPDATE job_status SET created_at = NOW() - INTERVAL '25 hours' WHERE job_type = 'media_probe'",
    )
    .execute(&server.pool)
    .await
    .unwrap();

    // Mark one as running (worker is processing the queue)
    sqlx::query("UPDATE job_status SET status = 'running' WHERE id = $1")
        .bind(job_ids[0])
        .execute(&server.pool)
        .await
        .unwrap();

    // Run cleanup — should NOT kill any pending jobs because the type has a running job
    let result = sqlx::query(STALE_CLEANUP_SQL)
        .execute(&server.pool)
        .await
        .unwrap();

    assert_eq!(
        result.rows_affected(),
        0,
        "Bulk backlog should survive cleanup when the type has active processing"
    );

    // All 19 pending jobs should still be pending
    let pending_count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM job_status WHERE job_type = 'media_probe' AND status = 'pending'",
    )
    .fetch_one(&server.pool)
    .await
    .unwrap();
    assert_eq!(pending_count, 19);
}

// =============================================================================
// Recent pending jobs protect the queue
// =============================================================================

#[tokio::test]
async fn test_cleanup_skips_when_recent_pending_jobs_exist() {
    let server = TestServer::start_api_only().await;

    // Create an old pending job (>24h) — would normally be cleaned up
    let old_pending = api::jobs::enqueue(
        &server.pool,
        &server.nats_publisher,
        "media_probe",
        serde_json::json!({ "file_id": "ffffffff-0000-0000-0000-000000000001" }),
        test_context("Old pending probe"),
    )
    .await
    .unwrap();
    sqlx::query("UPDATE job_status SET created_at = NOW() - INTERVAL '25 hours' WHERE id = $1")
        .bind(old_pending)
        .execute(&server.pool)
        .await
        .unwrap();

    // Create a recent pending job of the SAME type (< 4h old) — should protect entire type
    let _recent_pending = api::jobs::enqueue(
        &server.pool,
        &server.nats_publisher,
        "media_probe",
        serde_json::json!({ "file_id": "ffffffff-0000-0000-0000-000000000002" }),
        test_context("Recent pending probe"),
    )
    .await
    .unwrap();
    // This job is brand new (created_at = NOW()), so it's within the 4-hour window

    // No running or completed jobs — but the recent pending should protect the type
    let result = sqlx::query(STALE_CLEANUP_SQL)
        .execute(&server.pool)
        .await
        .unwrap();

    assert_eq!(
        result.rows_affected(),
        0,
        "Should NOT clean up old pending jobs when same type has recently-created pending jobs"
    );

    // Verify the old pending job is still pending
    let row: (String,) = sqlx::query_as("SELECT status FROM job_status WHERE id = $1")
        .bind(old_pending)
        .fetch_one(&server.pool)
        .await
        .unwrap();
    assert_eq!(row.0, "pending");
}

// =============================================================================
// enqueue_routine dedup behavior
// =============================================================================

#[tokio::test]
async fn test_enqueue_routine_skips_duplicate_active_jobs() {
    let server = TestServer::start_api_only().await;

    // First enqueue should succeed
    let result1 = api::jobs::enqueue_routine(
        &server.pool,
        &server.nats_publisher,
        "media_extract",
        serde_json::json!({ "file_id": "dddddddd-0000-0000-0000-000000000001", "media_type": "movie" }),
        nats_jobs::JobContext::new("First extract"),
    )
    .await;
    assert!(result1.is_ok(), "First enqueue should succeed");

    // Check if enqueue_routine has dedup — enqueue same payload again
    let result2 = api::jobs::enqueue_routine(
        &server.pool,
        &server.nats_publisher,
        "media_extract",
        serde_json::json!({ "file_id": "dddddddd-0000-0000-0000-000000000001", "media_type": "movie" }),
        nats_jobs::JobContext::new("Duplicate extract"),
    )
    .await;

    // enqueue_routine doesn't dedup — both should succeed (documented behavior)
    assert!(
        result2.is_ok(),
        "Second enqueue should also succeed (no built-in dedup)"
    );

    // Verify both jobs exist
    let count: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM job_status WHERE job_type = 'media_extract'")
            .fetch_one(&server.pool)
            .await
            .unwrap();
    assert_eq!(
        count, 2,
        "Both jobs should exist — enqueue_routine has no dedup"
    );
}

// =============================================================================
// Cleanup + startup reset interaction
// =============================================================================

#[tokio::test]
async fn test_restart_then_cleanup_preserves_backlog() {
    let server = TestServer::start_api_only().await;

    // Simulate pre-restart state: some running, many pending
    let running = api::jobs::enqueue(
        &server.pool,
        &server.nats_publisher,
        "media_probe",
        serde_json::json!({}),
        test_context("Was running"),
    )
    .await
    .unwrap();
    sqlx::query("UPDATE job_status SET status = 'running', started_at = NOW() - INTERVAL '10 minutes' WHERE id = $1")
        .bind(running)
        .execute(&server.pool)
        .await
        .unwrap();

    for i in 0..5 {
        let job_id = api::jobs::enqueue(
            &server.pool,
            &server.nats_publisher,
            "media_probe",
            serde_json::json!({ "i": i }),
            test_context("Queued"),
        )
        .await
        .unwrap();
        sqlx::query("UPDATE job_status SET created_at = NOW() - INTERVAL '25 hours' WHERE id = $1")
            .bind(job_id)
            .execute(&server.pool)
            .await
            .unwrap();
    }

    // Step 1: Startup reset (running → failed)
    sqlx::query(
        "UPDATE job_status SET status = 'failed', error = 'Orphaned on startup — worker restarted', completed_at = NOW() WHERE status = 'running'",
    )
    .execute(&server.pool)
    .await
    .unwrap();

    // Step 2: Cleanup runs — 5 pending jobs are old (>24h), no running/recent-completed
    // (the formerly-running job is now failed, not pending)
    // After reset there are NO running jobs, so cleanup WOULD kill pending ones
    let result = sqlx::query(STALE_CLEANUP_SQL)
        .execute(&server.pool)
        .await
        .unwrap();

    // After restart, if no job starts processing within 24 hours, stale jobs get
    // cleaned up. The worker should pick them up well before then.
    // Only 5 (not 6) because the formerly-running job was marked failed, not pending.
    assert_eq!(
        result.rows_affected(),
        5,
        "After restart with no active processing, old pending jobs are cleaned up"
    );
}

// =============================================================================
// Job context preserved through enqueue_routine
// =============================================================================

#[tokio::test]
async fn test_enqueue_routine_stores_context() {
    let server = TestServer::start_api_only().await;

    let context = nats_jobs::JobContext::new("Extracting keyframes & subtitles");
    let job_id = api::jobs::enqueue_routine(
        &server.pool,
        &server.nats_publisher,
        "media_extract",
        serde_json::json!({ "file_id": "eeeeeeee-0000-0000-0000-000000000001", "media_type": "movie" }),
        context,
    )
    .await
    .unwrap();

    // Query DB directly since enqueue_routine creates scheduled jobs
    // which are hidden from the default jobs listing API
    let row: (String, bool) =
        sqlx::query_as("SELECT description, scheduled FROM job_status WHERE id = $1")
            .bind(job_id)
            .fetch_one(&server.pool)
            .await
            .unwrap();

    assert_eq!(row.0, "Extracting keyframes & subtitles");
    assert!(row.1, "enqueue_routine should mark jobs as scheduled");
}
