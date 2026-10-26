use std::time::Duration as StdDuration;

use crate::common::*;
use api::hardcover_titles::{OffPeak, SyncPlan, TitleLimits};
use api::jobs::handlers::HardcoverTitlesSyncHandler;
use api::jobs::worker::JobHandler;
use api::runtime_config::RuntimeConfigUpdate;
use serde_json::{Value, json};
use time::{Duration, OffsetDateTime, Time};
use tokio_util::sync::CancellationToken;
use uuid::Uuid;
use wiremock::matchers::{body_partial_json, header, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

const ALL_DAY: OffPeak = OffPeak {
    from: Time::MIDNIGHT,
    to: Time::MAX,
};

const PLAN: SyncPlan = SyncPlan {
    limits: TitleLimits { total: 4, page: 2 },
    off_peak: ALL_DAY,
    page_gap: StdDuration::ZERO,
    busy_poll: StdDuration::from_millis(50),
};

fn book(id: i64, title: &str, users: i64, contributions: Value) -> Value {
    json!({ "id": id, "title": title, "users_count": users, "contributions": contributions })
}

async fn serve_page(mock: &MockServer, offset: u32, books: Vec<Value>) {
    Mock::given(method("POST"))
        .and(path("/v1/graphql"))
        .and(header("authorization", "Bearer hc-key"))
        .and(body_partial_json(
            json!({ "variables": { "offset": offset, "limit": 2 } }),
        ))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(json!({ "data": { "books": books } })),
        )
        .expect(1)
        .mount(mock)
        .await;
}

async fn configure(server: &TestServer, config: Value, enabled: bool) {
    sqlx::query(
        "UPDATE metadata_providers SET config = $1, enabled = $2 WHERE provider_type = 'hardcover'",
    )
    .bind(config)
    .bind(enabled)
    .execute(&server.pool)
    .await
    .unwrap();
}

async fn enable(server: &TestServer) {
    server
        .runtime_config
        .update(RuntimeConfigUpdate {
            book_title_suggestions_enabled: Some(true),
            ..Default::default()
        })
        .await
        .unwrap();
}

async fn configure_mock(server: &TestServer, mock: &MockServer) {
    enable(server).await;
    configure_key(server, mock).await;
}

async fn configure_key(server: &TestServer, mock: &MockServer) {
    configure(
        server,
        json!({ "api_key": "hc-key", "base_url": mock.uri() }),
        true,
    )
    .await;
}

fn handler(server: &TestServer, plan: SyncPlan) -> HardcoverTitlesSyncHandler {
    HardcoverTitlesSyncHandler::new(server.pool.clone(), server.runtime_config.clone(), plan)
}

async fn run_with(server: &TestServer, plan: SyncPlan) -> Result<Value, api::jobs::JobError> {
    handler(server, plan)
        .execute(Uuid::now_v7(), json!({}), CancellationToken::new())
        .await
}

async fn run(server: &TestServer) -> Result<Value, api::jobs::JobError> {
    run_with(server, PLAN).await
}

async fn titles(server: &TestServer) -> Vec<(i32, String, Option<String>, i32)> {
    sqlx::query_as(
        "SELECT hardcover_id, title, author, users_count FROM hardcover_titles
         ORDER BY users_count DESC",
    )
    .fetch_all(&server.pool)
    .await
    .unwrap()
}

fn row(
    id: i32,
    title: &str,
    author: Option<&str>,
    users: i32,
) -> (i32, String, Option<String>, i32) {
    (id, title.into(), author.map(Into::into), users)
}

async fn seed_stale(server: &TestServer) {
    sqlx::query(
        "INSERT INTO hardcover_titles (hardcover_id, title, author, users_count)
         VALUES (99, 'Forgotten', NULL, 1)",
    )
    .execute(&server.pool)
    .await
    .unwrap();
}

#[tokio::test]
async fn sync_pages_the_most_read_books_with_their_primary_author_once_a_week() {
    let server = TestServer::start_api_only().await;
    let mock = MockServer::start().await;
    configure_mock(&server, &mock).await;
    seed_stale(&server).await;
    serve_page(
        &mock,
        0,
        vec![
            book(
                1,
                "1984",
                900,
                json!([
                    { "contribution": "Cover Artist", "author": { "name": "Shepard Fairey" } },
                    { "contribution": null, "author": { "name": "George Orwell" } },
                ]),
            ),
            book(
                2,
                " Leaf Storm ",
                500,
                json!([
                    { "contribution": "Translator", "author": { "name": "Gregory Rabassa" } },
                    { "contribution": "Author", "author": { "name": "Gabriel García Márquez" } },
                ]),
            ),
        ],
    )
    .await;
    serve_page(
        &mock,
        2,
        vec![
            book(3, "  ", 400, json!([])),
            book(
                4,
                "Anthology",
                300,
                json!([{ "contribution": "Editor", "author": { "name": "Ed" } }]),
            ),
        ],
    )
    .await;

    run(&server).await.unwrap();
    run(&server).await.unwrap();

    assert_eq!(
        titles(&server).await,
        vec![
            row(1, "1984", Some("George Orwell"), 900),
            row(2, "Leaf Storm", Some("Gabriel García Márquez"), 500),
            row(4, "Anthology", None, 300),
        ]
    );
}

#[tokio::test]
async fn sync_stops_at_the_last_short_page_and_resyncs_after_a_week() {
    let server = TestServer::start_api_only().await;
    let mock = MockServer::start().await;
    configure_mock(&server, &mock).await;
    serve_page(&mock, 0, vec![book(1, "Dune", 50, json!([]))]).await;
    sqlx::query("INSERT INTO hardcover_title_syncs (synced_at) VALUES ($1)")
        .bind(OffsetDateTime::now_utc() - Duration::days(8))
        .execute(&server.pool)
        .await
        .unwrap();

    run(&server).await.unwrap();

    assert_eq!(titles(&server).await, vec![row(1, "Dune", None, 50)]);
}

#[tokio::test]
async fn a_failed_page_leaves_the_previous_titles_in_place_and_waits_a_day_to_retry() {
    let server = TestServer::start_api_only().await;
    let mock = MockServer::start().await;
    configure_mock(&server, &mock).await;
    seed_stale(&server).await;
    serve_page(
        &mock,
        0,
        vec![
            book(1, "Dune", 50, json!([])),
            book(2, "Emma", 40, json!([])),
        ],
    )
    .await;
    Mock::given(method("POST"))
        .and(body_partial_json(json!({ "variables": { "offset": 2 } })))
        .respond_with(ResponseTemplate::new(500))
        .mount(&mock)
        .await;

    assert!(run(&server).await.is_err());
    assert_eq!(titles(&server).await, vec![row(99, "Forgotten", None, 1)]);

    assert_eq!(run(&server).await.unwrap()["status"], "backoff");
    assert_eq!(mock.received_requests().await.unwrap().len(), 2);
}

#[tokio::test]
async fn sync_skips_without_an_enabled_hardcover_key() {
    let server = TestServer::start_api_only().await;
    let mock = MockServer::start().await;
    enable(&server).await;
    for (config, enabled) in [
        (json!({ "base_url": mock.uri() }), true),
        (json!({ "api_key": "", "base_url": mock.uri() }), true),
        (
            json!({ "api_key": "hc-key", "base_url": mock.uri() }),
            false,
        ),
    ] {
        configure(&server, config, enabled).await;
        assert_eq!(run(&server).await.unwrap()["status"], "skipped");
    }
    assert!(mock.received_requests().await.unwrap().is_empty());
}

#[tokio::test]
async fn sync_is_off_by_default_so_hardcover_is_never_asked() {
    let server = TestServer::start_api_only().await;
    let mock = MockServer::start().await;
    configure_key(&server, &mock).await;

    let result = run(&server).await.unwrap();

    assert_eq!(result["status"], "skipped");
    assert_eq!(result["reason"], "disabled");
    assert!(mock.received_requests().await.unwrap().is_empty());
    assert!(titles(&server).await.is_empty());
}

#[tokio::test]
async fn sync_waits_for_its_off_peak_window() {
    let server = TestServer::start_api_only().await;
    let mock = MockServer::start().await;
    configure_mock(&server, &mock).await;
    let now = api::notifications::prefs::Zone::ServerLocal.local_time(OffsetDateTime::now_utc());
    let plan = SyncPlan {
        off_peak: OffPeak {
            from: now + Duration::hours(3),
            to: now + Duration::hours(4),
        },
        ..PLAN
    };

    assert_eq!(
        run_with(&server, plan).await.unwrap()["status"],
        "off_peak_wait"
    );
    assert!(mock.received_requests().await.unwrap().is_empty());
}

#[tokio::test]
async fn sync_yields_to_running_book_provider_jobs() {
    let server = TestServer::start_api_only().await;
    let mock = MockServer::start().await;
    configure_mock(&server, &mock).await;
    serve_page(&mock, 0, vec![book(1, "Dune", 50, json!([]))]).await;
    let enrich = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO job_status (id, job_type, status) VALUES ($1, 'book_enrich', 'running')",
    )
    .bind(enrich)
    .execute(&server.pool)
    .await
    .unwrap();

    let sync = tokio::spawn({
        let handler = handler(&server, PLAN);
        async move {
            handler
                .execute(Uuid::now_v7(), json!({}), CancellationToken::new())
                .await
        }
    });
    tokio::time::sleep(StdDuration::from_millis(400)).await;
    assert!(mock.received_requests().await.unwrap().is_empty());

    sqlx::query("UPDATE job_status SET status = 'completed' WHERE id = $1")
        .bind(enrich)
        .execute(&server.pool)
        .await
        .unwrap();
    sync.await.unwrap().unwrap();
    assert_eq!(titles(&server).await, vec![row(1, "Dune", None, 50)]);
}

#[tokio::test]
async fn a_rate_limited_page_backs_off_every_hardcover_client() {
    let server = TestServer::start_api_only().await;
    let mock = MockServer::start().await;
    configure_mock(&server, &mock).await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(429).insert_header("retry-after", "1"))
        .mount(&mock)
        .await;

    assert!(run(&server).await.is_err());
    assert!(api::http::provider_cooldown().is_cooling_down("hardcover"));
}
