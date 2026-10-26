//! The daily book want check re-sends every wanted book through `book_want`,
//! so each book and format has exactly one Kobload pattern, keyed the same
//! way every time, and patterns left under another key are removed.

use crate::common::*;
use api::jobs::handlers::{BookWantCheckHandler, BookWantHandler};
use serde_json::{Value, json};
use uuid::Uuid;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

async fn book(pool: &sqlx::PgPool, title: &str, want_format: &str, want_status: &str) -> Uuid {
    let id = Uuid::now_v7();
    sqlx::query("INSERT INTO books (id, title, want_status, want_format) VALUES ($1, $2, $3, $4)")
        .bind(id)
        .bind(title)
        .bind(want_status)
        .bind(want_format)
        .execute(pool)
        .await
        .unwrap();
    id
}

fn pattern(source_id: &str) -> Value {
    json!({
        "id": Uuid::now_v7(), "source_system": "kobbler", "source_id": source_id,
        "callback_url": "http://kobbler", "patterns": [], "match_mode": "fuzzy",
        "status": "active",
        "created_at": "2026-10-01T00:00:00Z", "updated_at": "2026-10-01T00:00:00Z",
    })
}

async fn rig(patterns: Vec<Value>) -> (TestServer, MockServer) {
    let kobload = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/v1/watch"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(json!({ "items": patterns, "next_cursor": null })),
        )
        .mount(&kobload)
        .await;
    let server = TestServer::start_api_only().await;
    configure_kobload_at(&server, &kobload.uri()).await;
    (server, kobload)
}

fn check(server: &TestServer) -> BookWantCheckHandler {
    BookWantCheckHandler::new(
        server.pool.clone(),
        server.runtime_config.clone(),
        server.nats_publisher.clone(),
    )
}

async fn pending_book_wants(server: &TestServer) -> Vec<Uuid> {
    sqlx::query_scalar(
        "SELECT (payload->>'book_id')::UUID FROM job_status
         WHERE job_type = 'book_want' AND status = 'pending' ORDER BY created_at",
    )
    .fetch_all(&server.pool)
    .await
    .unwrap()
}

#[tokio::test]
async fn wanted_books_are_resent_with_their_alternate_titles() {
    let (server, _kobload) = rig(Vec::new()).await;
    let wanted = book(&server.pool, "The Final Empire", "ebook", "wanted").await;
    let in_library = book(&server.pool, "Owned Book", "ebook", "wanted").await;
    sqlx::query(
        "INSERT INTO book_files (id, book_id, file_path, format, size_bytes)
         VALUES ($1, $2, '/library/owned.epub', 'epub', 1)",
    )
    .bind(Uuid::now_v7())
    .bind(in_library)
    .execute(&server.pool)
    .await
    .unwrap();
    book(&server.pool, "Unwanted Book", "ebook", "none").await;

    process(&server, &check(&server)).await;
    assert_eq!(
        pending_book_wants(&server).await,
        vec![wanted],
        "only the wanted book"
    );

    let tip = stream_tip(&server).await;
    process(&server, &book_want(&server)).await;
    let sent = want_requests_since(&server, tip, &wanted.to_string()).await;
    let want = &sent.last().expect("book_want sent nothing")["media_want"];
    assert_eq!(want["alternate_titles"], json!(["Final Empire"]), "{want}");
}

#[tokio::test]
async fn a_second_check_adds_no_job_and_each_format_keeps_one_key() {
    let (server, _kobload) = rig(Vec::new()).await;
    let both = book(&server.pool, "Elantris", "both", "wanted").await;

    process(&server, &check(&server)).await;
    process(&server, &check(&server)).await;
    assert_eq!(
        pending_book_wants(&server).await,
        vec![both],
        "a duplicate job was queued"
    );
    process(&server, &book_want(&server)).await;

    let mut keys = Vec::new();
    for _ in 0..2 {
        let tip = stream_tip(&server).await;
        enqueue_book_want(&server, both).await;
        process(&server, &book_want(&server)).await;
        let mut sent: Vec<(String, String)> = sent_since(&server, tip, both)
            .await
            .into_iter()
            .map(|r| {
                (
                    r["source_id"].as_str().unwrap().to_string(),
                    r["media_want"]["kind"].as_str().unwrap().to_string(),
                )
            })
            .collect();
        sent.sort();
        keys.push(sent);
    }
    assert_eq!(
        keys[0],
        vec![
            (format!("{both}:audiobook"), "audiobook".to_string()),
            (format!("{both}:ebook"), "book".to_string()),
        ]
    );
    assert_eq!(keys[0], keys[1], "the keys moved between runs");
}

/// A pattern under a key the book's format no longer uses (the bare id that
/// held only the last of a "both" book's two wants) is removed. Patterns that
/// are not a wanted book's are left alone.
#[tokio::test]
async fn patterns_under_a_stale_key_are_removed() {
    let season = Uuid::now_v7();
    let both = Uuid::now_v7();
    let (server, _kobload) = rig(vec![
        pattern(&both.to_string()),
        pattern(&format!("{both}:ebook")),
        pattern(&season.to_string()),
    ])
    .await;
    sqlx::query(
        "INSERT INTO books (id, title, want_status, want_format) VALUES ($1, 'Warbreaker', 'wanted', 'both')",
    )
    .bind(both)
    .execute(&server.pool)
    .await
    .unwrap();

    let tip = stream_tip(&server).await;
    process(&server, &check(&server)).await;
    let deleted: Vec<String> = deletes_since(&server, tip, both).await;
    assert_eq!(deleted, vec![both.to_string()]);
}

async fn process(server: &TestServer, handler: &dyn api::jobs::worker::JobHandler) {
    if handler.job_type() == "book_want_check" {
        handler
            .execute(
                Uuid::now_v7(),
                json!({}),
                tokio_util::sync::CancellationToken::new(),
            )
            .await
            .unwrap();
    } else {
        server.process_pending_jobs(handler).await;
    }
}

fn book_want(server: &TestServer) -> BookWantHandler {
    BookWantHandler::new(
        server.pool.clone(),
        server.runtime_config.clone(),
        server.nats_publisher.clone(),
        server.library_events.clone(),
    )
}

async fn enqueue_book_want(server: &TestServer, book_id: Uuid) {
    api::jobs::enqueue(
        &server.pool,
        &server.nats_publisher,
        "book_want",
        json!({ "book_id": book_id }),
        nats_jobs::JobContext::for_entity("book", book_id, "test".to_string()),
    )
    .await
    .unwrap();
}

/// The JOBS stream is shared by every test in the run, so only this book's.
async fn jobs_for_book(server: &TestServer, tip: u64, job_type: &str, book: Uuid) -> Vec<Value> {
    kobload_jobs_of_type_since(server, tip, job_type)
        .await
        .into_iter()
        .filter(|job| {
            job["source_id"]
                .as_str()
                .is_some_and(|id| id.starts_with(&book.to_string()))
        })
        .collect()
}

async fn sent_since(server: &TestServer, tip: u64, book: Uuid) -> Vec<Value> {
    jobs_for_book(server, tip, "want_request", book).await
}

async fn deletes_since(server: &TestServer, tip: u64, book: Uuid) -> Vec<String> {
    jobs_for_book(server, tip, "watch_pattern_delete", book)
        .await
        .into_iter()
        .map(|d| d["source_id"].as_str().unwrap().to_string())
        .collect()
}

#[tokio::test]
async fn an_acquired_book_confirms_each_of_its_keys() {
    let (server, kobload) = rig(Vec::new()).await;
    Mock::given(method("POST"))
        .and(wiremock::matchers::path_regex("^/api/v1/want/"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "status": "confirmed" })))
        .mount(&kobload)
        .await;
    let both = book(&server.pool, "Mistborn", "both", "wanted").await;

    api::wanted::on_book_acquired(&server.runtime_config, &server.library_events, both).await;

    let confirmed: Vec<String> = kobload
        .received_requests()
        .await
        .unwrap()
        .iter()
        .filter(|r| r.url.path().starts_with("/api/v1/want/"))
        .map(|r| urlencoding::decode(r.url.path()).unwrap().into_owned())
        .collect();
    for key in [format!("{both}:ebook"), format!("{both}:audiobook")] {
        assert!(
            confirmed.iter().any(|p| p.contains(&key)),
            "{key} was not confirmed: {confirmed:?}"
        );
    }
}

#[tokio::test]
async fn undecided_and_unwanted_books_are_never_sent() {
    let (server, _kobload) = rig(Vec::new()).await;
    let wanted = book(&server.pool, "The Final Empire", "ebook", "wanted").await;
    let undecided = [
        book(&server.pool, "Discovered Novel", "ebook", "pending").await,
        book(&server.pool, "Skipped Novel", "ebook", "skipped").await,
        book(&server.pool, "Unwanted Novel", "ebook", "none").await,
    ];

    process(&server, &check(&server)).await;
    assert_eq!(pending_book_wants(&server).await, vec![wanted]);

    let tip = stream_tip(&server).await;
    for id in undecided {
        enqueue_book_want(&server, id).await;
    }
    process(&server, &book_want(&server)).await;
    for id in undecided {
        assert!(sent_since(&server, tip, id).await.is_empty());
    }
    let statuses: Vec<String> =
        sqlx::query_scalar("SELECT want_status FROM books WHERE id = ANY($1) ORDER BY title")
            .bind(undecided.to_vec())
            .fetch_all(&server.pool)
            .await
            .unwrap();
    assert_eq!(statuses, vec!["pending", "skipped", "none"]);
}
