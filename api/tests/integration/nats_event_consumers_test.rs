//! The durable EVENTS consumers kobbler runs for Kobload, driven over the real test NATS.
//!
//! Each consumer has a fixed durable name, so each test owns exactly one consumer and
//! no two tests may drive the same one: they would split its messages between them.

use std::time::Duration;

use api::LibraryEvent;
use nats_jobs::NatsClient;
use serde_json::{Value, json};
use tokio::sync::broadcast;
use tokio::task::JoinHandle;
use uuid::Uuid;

use crate::common::*;

const WAIT: Duration = Duration::from_secs(20);

struct Running(JoinHandle<()>);

impl Drop for Running {
    fn drop(&mut self) {
        self.0.abort();
    }
}

fn nats(server: &TestServer) -> NatsClient {
    server.app_state.nats.client.clone()
}

async fn publish_raw(nats: &NatsClient, subject: &str, body: &'static [u8]) {
    nats.jetstream()
        .publish(subject.to_string(), body.into())
        .await
        .unwrap()
        .await
        .unwrap();
}

async fn publish(server: &TestServer, event_type: &str, payload: Value) {
    server
        .nats_publisher
        .publish_event(event_type, payload)
        .await
        .unwrap();
}

async fn next_matching(
    rx: &mut broadcast::Receiver<LibraryEvent>,
    what: &str,
    mut pred: impl FnMut(&LibraryEvent) -> bool,
) {
    tokio::time::timeout(WAIT, async {
        loop {
            match rx.recv().await {
                Ok(event) if pred(&event) => return,
                Ok(_) | Err(broadcast::error::RecvError::Lagged(_)) => {}
                Err(broadcast::error::RecvError::Closed) => panic!("library bus closed"),
            }
        }
    })
    .await
    .unwrap_or_else(|_| panic!("never saw {what}"));
}

async fn consumer_info(
    nats: &NatsClient,
    durable: &str,
) -> Option<async_nats::jetstream::consumer::Info> {
    let stream = nats.jetstream().get_stream("EVENTS").await.ok()?;
    let mut consumer = stream
        .get_consumer::<async_nats::jetstream::consumer::pull::Config>(durable)
        .await
        .ok()?;
    consumer.info().await.ok().cloned()
}

async fn wait_for_consumer(nats: &NatsClient, durable: &str) {
    poll_until(
        WAIT,
        &format!("consumer {durable} never appeared"),
        || async { consumer_info(nats, durable).await.is_some() },
    )
    .await;
}

/// Everything delivered has been acked and nothing is left to deliver, so nothing will
/// come round again after ack_wait.
async fn wait_all_acked(nats: &NatsClient, durable: &str) {
    poll_until(
        WAIT,
        &format!("{durable} left messages unacked"),
        || async {
            consumer_info(nats, durable)
                .await
                .is_some_and(|i| i.num_ack_pending == 0 && i.num_pending == 0)
        },
    )
    .await;
}

async fn poll_until<F, Fut>(timeout: Duration, what: &str, mut check: F)
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = bool>,
{
    let deadline = tokio::time::Instant::now() + timeout;
    while !check().await {
        assert!(tokio::time::Instant::now() < deadline, "{what}");
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

async fn assert_consumer_config(nats: &NatsClient, durable: &str, filter: &str) {
    let info = consumer_info(nats, durable).await.unwrap();
    assert_eq!(info.config.durable_name.as_deref(), Some(durable));
    assert_eq!(info.config.filter_subject, filter);
    assert_eq!(
        info.config.ack_policy,
        async_nats::jetstream::consumer::AckPolicy::Explicit
    );
    assert_eq!(
        info.config.deliver_policy,
        async_nats::jetstream::consumer::DeliverPolicy::All
    );
}

async fn wanted_book(server: &TestServer) -> Uuid {
    let id = Uuid::new_v4();
    sqlx::query("INSERT INTO books (id, title, want_status) VALUES ($1, 'Elantris', 'wanted')")
        .bind(id)
        .execute(&server.pool)
        .await
        .unwrap();
    id
}

fn watch_pattern_deleted(movie_id: Uuid) -> Value {
    json!({
        "source_system": "kobbler",
        "source_id": movie_id.to_string(),
        "metadata": {"source_type": "movie"},
    })
}

#[tokio::test]
async fn watch_pattern_events_survive_bad_input_and_a_lost_consumer() {
    const DURABLE: &str = "kobbler-watch-pattern-deleted";
    let server = TestServer::start_api_only().await;
    let nats = nats(&server);
    let mut rx = server.library_events.subscribe();
    let _running = Running(tokio::spawn(
        api::nats::subscribe_watch_pattern_deleted_events(
            nats.clone(),
            server.pool.clone(),
            server.library_events.clone(),
        ),
    ));

    publish_raw(&nats, "events.watch_pattern.deleted", b"not json").await;
    publish(
        &server,
        "watch_pattern.renamed",
        watch_pattern_deleted(Uuid::new_v4()),
    )
    .await;
    publish(&server, "watch_pattern.deleted", json!({"source_id": 7})).await;
    let movie = Uuid::new_v4();
    publish(
        &server,
        "watch_pattern.deleted",
        watch_pattern_deleted(movie),
    )
    .await;

    next_matching(
        &mut rx,
        "the reset after the bad input",
        |e| matches!(e, LibraryEvent::LibraryUpdated { book_id: Some(id) } if *id == movie),
    )
    .await;
    wait_all_acked(&nats, DURABLE).await;
    assert_consumer_config(&nats, DURABLE, "events.watch_pattern.>").await;

    nats.jetstream()
        .get_stream("EVENTS")
        .await
        .unwrap()
        .delete_consumer(DURABLE)
        .await
        .unwrap();
    let after = Uuid::new_v4();
    publish(
        &server,
        "watch_pattern.deleted",
        watch_pattern_deleted(after),
    )
    .await;

    next_matching(
        &mut rx,
        "an event after the consumer was deleted",
        |e| matches!(e, LibraryEvent::LibraryUpdated { book_id: Some(id) } if *id == after),
    )
    .await;
    wait_all_acked(&nats, DURABLE).await;
}

#[tokio::test]
async fn want_responses_are_acked_even_when_the_handler_fails() {
    const DURABLE: &str = "kobbler-want-response";
    let server = TestServer::start_api_only().await;
    let nats = nats(&server);
    let mut rx = server.library_events.subscribe();
    let _running = Running(tokio::spawn(api::nats::subscribe_want_response_events(
        nats.clone(),
        server.pool.clone(),
        server.library_events.clone(),
    )));

    publish_raw(&nats, "events.want.response.kobbler", b"{").await;
    publish(&server, "want.response.kobbler", json!({})).await;
    let movie = Uuid::new_v4();
    publish(
        &server,
        "want.response.kobbler",
        json!({
            "source_system": "kobbler",
            "source_id": movie.to_string(),
            "watch_pattern_id": Uuid::new_v4(),
            "status": "failed",
            "download_id": null,
            "results_found": 0,
            "best_score": 0.0,
            "metadata": {"source_type": "movie"},
        }),
    )
    .await;

    next_matching(
        &mut rx,
        "the movie update after the failures",
        |e| matches!(e, LibraryEvent::MovieUpdated { movie_id } if *movie_id == movie),
    )
    .await;
    wait_all_acked(&nats, DURABLE).await;
    assert_consumer_config(&nats, DURABLE, "events.want.response.kobbler").await;
}

#[tokio::test]
async fn download_events_dispatch_by_type_and_ack_everything() {
    const DURABLE: &str = "kobbler-download-events";
    let server = TestServer::start_api_only().await;
    let nats = nats(&server);
    let book = wanted_book(&server).await;
    let pattern = Uuid::new_v4();
    let mut rx = server.library_events.subscribe();
    let _running = Running(tokio::spawn(api::nats::subscribe_download_events(
        nats.clone(),
        server.pool.clone(),
        server.library_events.clone(),
    )));

    publish_raw(&nats, "events.download.completed", b"[]").await;
    publish(&server, "download.paused", json!({"download_id": pattern})).await;
    publish(
        &server,
        "download.completed",
        json!({"status": "completed"}),
    )
    .await;
    publish(
        &server,
        "download.watch_matched",
        json!({
            "watch_pattern_id": pattern,
            "source_system": "kobbler",
            "source_id": book.to_string(),
            "match_score": 1.0,
            "matched_pattern": "Elantris",
            "release_title": "Elantris.epub",
            "release_guid": "guid",
            "download_id": null,
            "action": "grabbed",
            "metadata": null,
        }),
    )
    .await;
    next_matching(&mut rx, "the watch match", |e| {
        matches!(e, LibraryEvent::WantedStatusUpdated { wanted_id, status, .. }
            if *wanted_id == book && status == "downloading")
    })
    .await;

    publish(
        &server,
        "download.completed",
        json!({
            "download_id": pattern,
            "status": "completed",
            "metadata": {},
            "files": [],
            "error": null,
        }),
    )
    .await;
    next_matching(&mut rx, "the completion", |e| {
        matches!(e, LibraryEvent::WantedStatusUpdated { wanted_id, status, .. }
            if *wanted_id == book && status == "completed")
    })
    .await;
    wait_all_acked(&nats, DURABLE).await;
    assert_consumer_config(&nats, DURABLE, "events.download.>").await;
}

async fn ingest_job(server: &TestServer, download_id: Uuid) -> Option<Uuid> {
    sqlx::query_scalar(
        "SELECT id FROM job_status WHERE job_type = 'kobload_ingest' AND entity_id = $1",
    )
    .bind(download_id)
    .fetch_optional(&server.pool)
    .await
    .unwrap()
}

#[tokio::test]
async fn ingest_ready_enqueues_once_per_idempotency_key() {
    const DURABLE: &str = "kobbler-ingest-ready";
    let server = TestServer::start_api_only().await;
    let nats = nats(&server);
    let book = wanted_book(&server).await;
    let seen_key = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO wanted_attempts (id, book_id, source, status, idempotency_key)
         VALUES ($1, $2, 'want', 'completed', $3)",
    )
    .bind(Uuid::new_v4())
    .bind(book)
    .bind(seen_key)
    .execute(&server.pool)
    .await
    .unwrap();
    let _running = Running(tokio::spawn(api::nats::subscribe_ingest_ready_events(
        nats.clone(),
        server.pool.clone(),
        server.nats_publisher.clone(),
    )));
    wait_for_consumer(&nats, DURABLE).await;

    publish_raw(&nats, "events.ingest.ready", b"nope").await;
    let duplicate = Uuid::new_v4();
    publish(
        &server,
        "ingest.ready",
        json!({"download_id": duplicate, "idempotency_key": seen_key}),
    )
    .await;
    let fresh = Uuid::new_v4();
    publish(
        &server,
        "ingest.ready",
        json!({"download_id": fresh, "idempotency_key": Uuid::new_v4()}),
    )
    .await;

    poll_until(WAIT, "the fresh ingest was never enqueued", || async {
        ingest_job(&server, fresh).await.is_some()
    })
    .await;
    wait_all_acked(&nats, DURABLE).await;
    assert_consumer_config(&nats, DURABLE, "events.ingest.ready").await;
    assert_eq!(
        ingest_job(&server, duplicate).await,
        None,
        "an already-processed idempotency key must not enqueue again"
    );

    let job_id = ingest_job(&server, fresh).await.unwrap();
    nats_jobs::purge_nats_job(&nats, "kobbler", "kobload_ingest", job_id).await;
}
