//! Tests verifying NATS queue subscription and message processing.
//!
//! These tests ensure that each service's NATS worker picks up messages
//! from the correct subjects and routes them to the registered handlers.
//! Unlike the handler tests (which call execute() directly), these tests
//! publish messages to NATS and verify the worker processes them.
//!
//! Each test uses a unique service name to avoid durable consumer conflicts
//! when tests run in parallel.

use crate::common::*;
use api::jobs::handlers::BookWantHandler;
use nats_jobs::{JobStatusDb, WorkerConfig};
use serde_json::{Value, json};
use tokio::sync::watch;
use uuid::Uuid;

/// Helper: poll job_status table until the job reaches a terminal state.
async fn wait_for_job_terminal(
    pool: &sqlx::PgPool,
    job_id: Uuid,
    timeout_ms: u64,
) -> (String, Option<Value>) {
    use std::time::Instant;
    use tokio::time::Duration;

    let start = Instant::now();
    let timeout = Duration::from_millis(timeout_ms);

    while start.elapsed() < timeout {
        let row: Option<(String, Option<Value>)> =
            sqlx::query_as("SELECT status, result FROM job_status WHERE id = $1")
                .bind(job_id)
                .fetch_optional(pool)
                .await
                .expect("query job_status");

        if let Some((status, result)) = row
            && (status == "completed" || status == "failed")
        {
            return (status, result);
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }

    panic!(
        "Job {} did not reach terminal state within {}ms",
        job_id, timeout_ms
    );
}

/// Adapter matching the pattern in api/src/nats.rs for registering kobbler handlers.
struct KobblerHandlerAdapter<H: api::jobs::worker::JobHandler>(H);

impl<H: api::jobs::worker::JobHandler + 'static> KobblerHandlerAdapter<H> {
    fn new(handler: H) -> Self {
        Self(handler)
    }
}

#[async_trait::async_trait]
impl<H: api::jobs::worker::JobHandler + 'static> nats_jobs::JobHandler
    for KobblerHandlerAdapter<H>
{
    fn job_type(&self) -> &'static str {
        self.0.job_type()
    }

    fn max_concurrent(&self) -> usize {
        self.0.max_concurrent()
    }

    async fn execute(
        &self,
        job_id: Uuid,
        payload: Value,
        cancel: tokio_util::sync::CancellationToken,
    ) -> Result<Value, nats_jobs::JobError> {
        self.0.execute(job_id, payload, cancel).await.map_err(|e| {
            if e.is_retryable() {
                nats_jobs::JobError::retryable(e.to_string())
            } else {
                nats_jobs::JobError::permanent(e.to_string())
            }
        })
    }
}

/// Generate a unique service name for each test to avoid durable consumer collisions.
fn unique_service() -> String {
    format!("kobbler-test-{}", Uuid::now_v7())
}

// =============================================================================
// Tests
// =============================================================================

/// Verify that a book_want job published to NATS is picked up by the worker
/// and processed through the BookWantHandler. This tests the full
/// publish → consume → execute → status-update loop.
#[tokio::test]
async fn test_nats_worker_processes_book_want_job() {
    let server = TestServer::start_api_only().await;
    let service = unique_service();

    // Configure Kobload settings so the handler doesn't skip
    let resp = server
        .admin_client()
        .patch_json(
            "/api/v1/admin/config",
            &json!({
                "kobload_base_url": "http://localhost:9999",
                "kobload_api_key": "test-key",
                "kobload_ingest_url": format!("{}/api/v1/ingest/{}", server.api_url, server.ingest_token)
            }),
        )
        .await;
    resp.assert_ok();

    // Create a wanted book
    let create_resp = server
        .admin_client()
        .post_json(
            "/api/v1/admin/wanted",
            &json!({
                "title": "The Way of Kings",
                "author": "Brandon Sanderson",
            }),
        )
        .await;
    create_resp.assert_ok();
    let wanted_id = create_resp.json::<Value>()["id"]
        .as_str()
        .unwrap()
        .to_string();

    // Start a minimal NATS worker with just the BookWantHandler
    let nats_url =
        std::env::var("NATS_URL_TEST").unwrap_or_else(|_| "nats://localhost:4223".to_string());
    let nats_config = nats_jobs::NatsConfig::new(&nats_url, &service);
    let nats_client = nats_jobs::NatsClient::connect(&nats_config)
        .await
        .expect("connect to test NATS");

    let job_db = JobStatusDb::new(server.pool.clone());
    let config = WorkerConfig::for_service(&service);

    let handler = BookWantHandler::new(
        server.pool.clone(),
        server.runtime_config.clone(),
        server.nats_publisher.clone(),
        server.library_events.clone(),
    );

    let mut worker = nats_jobs::JobWorker::new(nats_client, config).with_job_db(job_db);
    worker.register_handler(KobblerHandlerAdapter::new(handler));

    // Create shutdown channel
    let (shutdown_tx, shutdown_rx) = watch::channel(false);

    // Spawn the worker
    tokio::spawn(async move {
        let _ = worker.run(shutdown_rx).await;
    });

    // Give the worker a moment to set up consumers
    tokio::time::sleep(tokio::time::Duration::from_millis(500)).await;

    // Publish a book_want job via the publisher (targeting our unique service)
    let job_id = server
        .nats_publisher
        .enqueue_job(&service, "book_want", json!({ "book_id": wanted_id }))
        .await
        .expect("enqueue job");

    // Wait for the job to complete
    let (status, result) = wait_for_job_terminal(&server.pool, job_id, 10_000).await;

    assert_eq!(status, "completed", "job should complete successfully");
    let result = result.expect("result should exist");
    assert_eq!(
        result["status"], "enqueued",
        "handler should enqueue to kobload"
    );
    assert_eq!(result["book_id"], wanted_id);

    // Verify the book's want_status was updated
    let want_status: String = sqlx::query_scalar("SELECT want_status FROM books WHERE id = $1")
        .bind(Uuid::parse_str(&wanted_id).unwrap())
        .fetch_one(&server.pool)
        .await
        .expect("fetch want_status");
    assert_eq!(want_status, "searching");

    // Shut down the worker
    let _ = shutdown_tx.send(true);
}

/// Verify that a job published to the wrong service is NOT picked up by the worker.
/// The worker for service X should not process jobs targeted at service Y.
#[tokio::test]
async fn test_nats_worker_ignores_other_service_jobs() {
    let server = TestServer::start_api_only().await;
    let service = unique_service();

    let nats_url =
        std::env::var("NATS_URL_TEST").unwrap_or_else(|_| "nats://localhost:4223".to_string());
    let nats_config = nats_jobs::NatsConfig::new(&nats_url, &service);
    let nats_client = nats_jobs::NatsClient::connect(&nats_config)
        .await
        .expect("connect to test NATS");

    let job_db = JobStatusDb::new(server.pool.clone());
    let config = WorkerConfig::for_service(&service);

    let handler = BookWantHandler::new(
        server.pool.clone(),
        server.runtime_config.clone(),
        server.nats_publisher.clone(),
        server.library_events.clone(),
    );

    let mut worker = nats_jobs::JobWorker::new(nats_client, config).with_job_db(job_db);
    worker.register_handler(KobblerHandlerAdapter::new(handler));

    let (shutdown_tx, shutdown_rx) = watch::channel(false);

    tokio::spawn(async move {
        let _ = worker.run(shutdown_rx).await;
    });

    tokio::time::sleep(tokio::time::Duration::from_millis(500)).await;

    // Publish a job to a DIFFERENT service — worker should NOT pick it up.
    let job_id = server
        .nats_publisher
        .enqueue_job("other-service", "book_want", json!({ "book_id": "fake" }))
        .await
        .expect("enqueue job");

    // Wait briefly — the job should NOT be processed.
    tokio::time::sleep(tokio::time::Duration::from_millis(1500)).await;

    let row: Option<(String,)> = sqlx::query_as("SELECT status FROM job_status WHERE id = $1")
        .bind(job_id)
        .fetch_optional(&server.pool)
        .await
        .expect("query job_status");

    // The job should either not exist in our DB (worker never saw it)
    // or still be pending (never picked up).
    match row {
        None => {} // Expected — worker has no consumer for "other-service"
        Some((status,)) => {
            assert_ne!(
                status, "completed",
                "Job for other-service should NOT be completed by our worker"
            );
        }
    }

    let _ = shutdown_tx.send(true);
}
