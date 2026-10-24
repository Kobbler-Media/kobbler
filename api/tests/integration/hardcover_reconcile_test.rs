//! Following Hardcover's merges and deletions (tracking::reconcile).
//!
//! Hardcover is played by a wiremock responder that answers the batched
//! `items: <table>(where: {id: {_in: $ids}})` lookup from a fixture: an id mapped to
//! `None` is canonical, to `Some(c)` is merged into `c`, and an absent id is deleted.

use std::collections::HashMap;

use crate::common::*;
use api::repositories::provider_reconcile::EntityKind;
use api::tracking::HardcoverAuthorProvider;
use api::tracking::reconcile::{self, Remote};
use serde_json::{Value, json};
use sqlx::PgPool;
use uuid::Uuid;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, Request, Respond, ResponseTemplate};

#[derive(Default, Clone)]
struct Hardcover {
    authors: HashMap<i64, Option<i64>>,
    series: HashMap<i64, Option<i64>>,
    books: HashMap<i64, Option<i64>>,
}

impl Hardcover {
    fn table(&self, query: &str) -> &HashMap<i64, Option<i64>> {
        if query.contains("authors") {
            &self.authors
        } else if query.contains("series") {
            &self.series
        } else {
            &self.books
        }
    }
}

impl Respond for Hardcover {
    fn respond(&self, request: &Request) -> ResponseTemplate {
        let body: Value = serde_json::from_slice(&request.body).unwrap();
        let query = body["query"].as_str().unwrap();
        let table = self.table(query);

        // `m0: series_by_pk(id: 7) { id canonical_id } m1: ...` -- the one-by-one
        // confirmation of ids a batch did not return.
        if query.contains("_by_pk(") {
            let mut data = serde_json::Map::new();
            for part in query.split(" m").skip(1) {
                let (alias, rest) = part.split_once(':').unwrap();
                let id: i64 = rest
                    .split("(id: ")
                    .nth(1)
                    .and_then(|r| r.split(')').next())
                    .unwrap()
                    .parse()
                    .unwrap();
                let item = table
                    .get(&id)
                    .map(|canonical| json!({ "id": id, "canonical_id": canonical }))
                    .unwrap_or(Value::Null);
                data.insert(format!("m{alias}"), item);
            }
            return ResponseTemplate::new(200).set_body_json(json!({ "data": data }));
        }

        let items: Vec<Value> = body["variables"]["ids"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|id| {
                let id = id.as_i64().unwrap();
                table
                    .get(&id)
                    .map(|canonical| json!({ "id": id, "canonical_id": canonical }))
            })
            .collect();
        ResponseTemplate::new(200).set_body_json(json!({ "data": { "items": items } }))
    }
}

async fn hardcover(fixture: Hardcover) -> (MockServer, HardcoverAuthorProvider) {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/graphql"))
        .respond_with(fixture)
        .mount(&server)
        .await;
    let provider = HardcoverAuthorProvider::with_base_url("test-key".into(), server.uri());
    (server, provider)
}

async fn insert_book(pool: &PgPool, title: &str, hardcover_id: i64) -> Uuid {
    let id = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO books (id, title, identifiers, want_status) VALUES ($1, $2, $3, 'pending')",
    )
    .bind(id)
    .bind(title)
    .bind(json!({ "hardcover": hardcover_id.to_string(), "hardcover_slug": "old-slug" }))
    .execute(pool)
    .await
    .unwrap();
    id
}

async fn insert_series(pool: &PgPool, name: &str, hardcover_id: i64) -> Uuid {
    sqlx::query_scalar(
        "INSERT INTO series (name, sort_key, identifiers) VALUES ($1, lower($1), $2) RETURNING id",
    )
    .bind(name)
    .bind(json!({ "hardcover": hardcover_id.to_string() }))
    .fetch_one(pool)
    .await
    .unwrap()
}

async fn add_to_series(pool: &PgPool, book_id: Uuid, series_id: Uuid) {
    sqlx::query("INSERT INTO book_series (book_id, series_id, is_primary) VALUES ($1, $2, true)")
        .bind(book_id)
        .bind(series_id)
        .execute(pool)
        .await
        .unwrap();
}

async fn exists(pool: &PgPool, table: &str, id: Uuid) -> bool {
    let sql = format!("SELECT EXISTS (SELECT 1 FROM {table} WHERE id = $1)");
    sqlx::query_scalar(sqlx::AssertSqlSafe(sql.as_str()))
        .bind(id)
        .fetch_one(pool)
        .await
        .unwrap()
}

async fn hardcover_id(pool: &PgPool, table: &str, id: Uuid) -> Option<String> {
    let sql = format!("SELECT identifiers->>'hardcover' FROM {table} WHERE id = $1");
    sqlx::query_scalar(sqlx::AssertSqlSafe(sql.as_str()))
        .bind(id)
        .fetch_one(pool)
        .await
        .unwrap()
}

async fn issue_kind(pool: &PgPool, id: Uuid) -> Option<String> {
    sqlx::query_scalar("SELECT kind FROM provider_link_issues WHERE entity_id = $1")
        .bind(id)
        .fetch_optional(pool)
        .await
        .unwrap()
}

/// The case that started this: a series Hardcover deleted, holding only discovered
/// books Hardcover merged into editions we already have. Series enrich failed on it
/// six times a week, forever.
#[tokio::test]
async fn a_deleted_series_of_merged_duplicates_is_removed() {
    let server = TestServer::start_api_only().await;
    let pool = &server.pool;

    let canonical = insert_book(pool, "A Clash of Kings", 945184).await;
    let duplicate = insert_book(pool, "Kralların Çarpışması: Kısım I", 2474756).await;
    let series = insert_series(pool, "A Song of Ice and Fire Split-Volume Edition", 279993).await;
    add_to_series(pool, duplicate, series).await;

    let (_mock, hc) = hardcover(Hardcover {
        books: HashMap::from([(945184, None), (2474756, Some(945184))]),
        ..Default::default()
    })
    .await;

    reconcile::run(pool, &server.nats_publisher, &hc)
        .await
        .unwrap();

    assert!(
        !exists(pool, "series", series).await,
        "deleted upstream, nothing of ours in it"
    );
    assert!(
        !exists(pool, "books", duplicate).await,
        "merged upstream, holds nothing"
    );
    assert!(exists(pool, "books", canonical).await);
}

#[tokio::test]
async fn a_deleted_book_with_a_file_is_unlinked_and_flagged_not_deleted() {
    let server = TestServer::start_api_only().await;
    let pool = &server.pool;

    let book = insert_book(pool, "Gone Upstream", 111).await;
    create_stub_book_file(pool, book).await;

    let (_mock, hc) = hardcover(Hardcover::default()).await;
    reconcile::run(pool, &server.nats_publisher, &hc)
        .await
        .unwrap();

    assert!(exists(pool, "books", book).await);
    assert_eq!(hardcover_id(pool, "books", book).await, None);
    assert_eq!(issue_kind(pool, book).await.as_deref(), Some("deleted"));
}

#[tokio::test]
async fn a_merged_book_holding_a_file_takes_over_an_empty_canonical_row() {
    let server = TestServer::start_api_only().await;
    let pool = &server.pool;

    let owned = insert_book(pool, "Owned Edition", 200).await;
    create_stub_book_file(pool, owned).await;
    let empty_canonical = insert_book(pool, "Canonical", 201).await;

    let (_mock, hc) = hardcover(Hardcover {
        books: HashMap::from([(200, Some(201)), (201, None)]),
        ..Default::default()
    })
    .await;
    reconcile::run(pool, &server.nats_publisher, &hc)
        .await
        .unwrap();

    assert!(!exists(pool, "books", empty_canonical).await);
    assert_eq!(
        hardcover_id(pool, "books", owned).await.as_deref(),
        Some("201")
    );
}

#[tokio::test]
async fn two_books_holding_files_are_flagged_not_merged() {
    let server = TestServer::start_api_only().await;
    let pool = &server.pool;

    let a = insert_book(pool, "Edition A", 300).await;
    let b = insert_book(pool, "Edition B", 301).await;
    create_stub_book_file(pool, a).await;
    create_stub_book_file(pool, b).await;

    let (_mock, hc) = hardcover(Hardcover {
        books: HashMap::from([(300, Some(301)), (301, None)]),
        ..Default::default()
    })
    .await;
    let report = reconcile::run(pool, &server.nats_publisher, &hc)
        .await
        .unwrap();

    assert!(exists(pool, "books", a).await && exists(pool, "books", b).await);
    assert_eq!(hardcover_id(pool, "books", a).await.as_deref(), Some("300"));
    assert_eq!(issue_kind(pool, a).await.as_deref(), Some("merge_conflict"));
    assert_eq!(report.conflicts, 1);
}

#[tokio::test]
async fn a_merged_series_folds_its_books_into_the_survivor() {
    let server = TestServer::start_api_only().await;
    let pool = &server.pool;

    let survivor = insert_series(pool, "Cradle", 400).await;
    let duplicate = insert_series(pool, "Cradle (dup)", 401).await;
    let book = insert_book(pool, "Unsouled", 402).await;
    create_stub_book_file(pool, book).await;
    add_to_series(pool, book, duplicate).await;

    let (_mock, hc) = hardcover(Hardcover {
        series: HashMap::from([(400, None), (401, Some(400))]),
        books: HashMap::from([(402, None)]),
        ..Default::default()
    })
    .await;
    reconcile::run(pool, &server.nats_publisher, &hc)
        .await
        .unwrap();

    assert!(!exists(pool, "series", duplicate).await);
    let in_survivor: bool = sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM book_series WHERE book_id = $1 AND series_id = $2)",
    )
    .bind(book)
    .bind(survivor)
    .fetch_one(pool)
    .await
    .unwrap();
    assert!(in_survivor);
}

#[tokio::test]
async fn a_merged_series_with_no_local_survivor_is_repointed() {
    let server = TestServer::start_api_only().await;
    let pool = &server.pool;

    let series = insert_series(pool, "Old Id", 500).await;
    let (_mock, hc) = hardcover(Hardcover {
        series: HashMap::from([(500, Some(501)), (501, None)]),
        ..Default::default()
    })
    .await;

    let remote = reconcile::reconcile_entity(
        pool,
        &server.nats_publisher,
        &hc,
        EntityKind::Series,
        series,
        500,
    )
    .await
    .unwrap();

    assert_eq!(remote, Remote::MergedInto(501));
    assert_eq!(
        hardcover_id(pool, "series", series).await.as_deref(),
        Some("501")
    );
}

#[tokio::test]
async fn a_hardcover_error_changes_nothing() {
    let server = TestServer::start_api_only().await;
    let pool = &server.pool;
    let book = insert_book(pool, "Still Here", 600).await;

    let mock = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(500))
        .mount(&mock)
        .await;
    let hc = HardcoverAuthorProvider::with_base_url("test-key".into(), mock.uri());

    assert!(
        reconcile::run(pool, &server.nats_publisher, &hc)
            .await
            .is_err()
    );
    assert!(
        exists(pool, "books", book).await,
        "an error must never read as a deletion"
    );
}

#[tokio::test]
async fn flagged_links_are_listed_with_their_names() {
    let server = TestServer::start_api_only().await;
    let pool = &server.pool;

    let book = insert_book(pool, "Gone Upstream", 700).await;
    create_stub_book_file(pool, book).await;
    let (_mock, hc) = hardcover(Hardcover::default()).await;
    reconcile::run(pool, &server.nats_publisher, &hc)
        .await
        .unwrap();

    let resp = server
        .admin_client()
        .get("/api/v1/library/provider-link-issues")
        .await;
    resp.assert_status(reqwest::StatusCode::OK);
    let rows: Vec<Value> = resp.json();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["entity_id"], json!(book));
    assert_eq!(rows[0]["name"], json!("Gone Upstream"));
    assert_eq!(rows[0]["kind"], json!("deleted"));
    assert_eq!(rows[0]["external_id"], json!("700"));
}

#[tokio::test]
async fn flagged_links_need_auth() {
    let server = TestServer::start_api_only().await;
    server
        .api_client()
        .get("/api/v1/library/provider-link-issues")
        .await
        .assert_status(reqwest::StatusCode::UNAUTHORIZED);
}

/// Hardcover caps rows per query; a lookup that loses rows must not read as a mass
/// deletion. Past the threshold, deletions are withheld.
#[tokio::test]
async fn implausibly_many_deletions_are_withheld() {
    let server = TestServer::start_api_only().await;
    let pool = &server.pool;

    let mut ids = Vec::new();
    for n in 0..30 {
        ids.push(insert_book(pool, &format!("Book {n}"), 10_000 + n).await);
    }

    let (_mock, hc) = hardcover(Hardcover::default()).await;
    let report = reconcile::run(pool, &server.nats_publisher, &hc)
        .await
        .unwrap();

    assert_eq!(report.deletions_withheld, 30, "{report:?}");
    for id in ids {
        assert!(exists(pool, "books", id).await);
    }
}
