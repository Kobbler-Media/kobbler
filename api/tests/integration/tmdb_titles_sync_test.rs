use std::io::Write;

use crate::common::*;
use api::jobs::handlers::TmdbTitlesSyncHandler;
use api::jobs::worker::JobHandler;
use api::tmdb_titles::export::ExportLimits;
use flate2::Compression;
use flate2::write::GzEncoder;
use serde_json::{Value, json};
use time::{Date, Duration, OffsetDateTime};
use tokio_util::sync::CancellationToken;
use uuid::Uuid;
use wiremock::matchers::{method, path, query_param};
use wiremock::{Mock, MockServer, ResponseTemplate};

const LIMITS: ExportLimits = ExportLimits { movie: 2, tv: 1 };

fn gz(lines: &[&str]) -> Vec<u8> {
    let mut encoder = GzEncoder::new(Vec::new(), Compression::default());
    for line in lines {
        writeln!(encoder, "{line}").unwrap();
    }
    encoder.finish().unwrap()
}

fn today() -> Date {
    OffsetDateTime::now_utc().date()
}

fn export_path(prefix: &str, date: Date) -> String {
    format!(
        "/{prefix}_{:02}_{:02}_{}.json.gz",
        u8::from(date.month()),
        date.day(),
        date.year()
    )
}

async fn serve_export(mock: &MockServer, prefix: &str, date: Date, lines: &[&str]) {
    Mock::given(method("GET"))
        .and(path(export_path(prefix, date)))
        .respond_with(ResponseTemplate::new(200).set_body_bytes(gz(lines)))
        .expect(1)
        .mount(mock)
        .await;
}

const MOVIES: &[&str] = &[
    r#"{"adult":false,"id":1,"original_title":"Parasite Hunters","popularity":5.0,"video":false}"#,
    r#"{"adult":true,"id":2,"original_title":"Adult Thing","popularity":99.0,"video":false}"#,
    r#"{"adult":false,"id":3,"original_title":"Making Of","popularity":50.0,"video":true}"#,
    r#"{"adult":false,"id":4,"original_title":"Blue Lagoon","popularity":3.0,"video":false}"#,
    "not json",
    r#"{"adult":false,"id":6,"original_title":"기생충","popularity":40.0,"video":false}"#,
];

const SHOWS: &[&str] = &[
    r#"{"id":10,"original_name":"Bluey","popularity":30.0}"#,
    r#"{"id":11,"original_name":"Late Talk","popularity":2.0}"#,
];

async fn set_tmdb(server: &TestServer, config: Value, enabled: bool) {
    sqlx::query(
        "UPDATE video_metadata_providers SET config = $1, enabled = $2 WHERE provider_type = 'tmdb'",
    )
    .bind(config)
    .bind(enabled)
    .execute(&server.pool)
    .await
    .unwrap();
}

async fn run(server: &TestServer, exports: &MockServer) -> Value {
    TmdbTitlesSyncHandler::new(server.pool.clone(), exports.uri(), LIMITS)
        .execute(Uuid::now_v7(), json!({}), CancellationToken::new())
        .await
        .unwrap()
}

async fn titles(server: &TestServer) -> Vec<(String, i32, String, Option<String>)> {
    sqlx::query_as(
        "SELECT media_type, tmdb_id, original_title, title FROM tmdb_titles
         ORDER BY media_type, popularity DESC",
    )
    .fetch_all(&server.pool)
    .await
    .unwrap()
}

async fn export_date(server: &TestServer, media_type: &str) -> Option<Date> {
    sqlx::query_scalar("SELECT export_date FROM tmdb_title_syncs WHERE media_type = $1")
        .bind(media_type)
        .fetch_optional(&server.pool)
        .await
        .unwrap()
        .flatten()
}

fn row(
    media: &str,
    id: i32,
    original: &str,
    title: Option<&str>,
) -> (String, i32, String, Option<String>) {
    (media.into(), id, original.into(), title.map(Into::into))
}

#[tokio::test]
async fn export_keeps_the_most_popular_watchable_titles_once_a_day() {
    let server = TestServer::start_api_only().await;
    let exports = MockServer::start().await;
    serve_export(&exports, "movie_ids", today(), MOVIES).await;
    serve_export(&exports, "tv_series_ids", today(), SHOWS).await;

    run(&server, &exports).await;
    run(&server, &exports).await;

    assert_eq!(
        titles(&server).await,
        vec![
            row("movie", 6, "기생충", None),
            row("movie", 1, "Parasite Hunters", None),
            row("tv", 10, "Bluey", None),
        ]
    );
    assert_eq!(export_date(&server, "movie").await, Some(today()));
    assert_eq!(export_date(&server, "tv").await, Some(today()));
}

#[tokio::test]
async fn export_falls_back_to_yesterday_until_today_is_published() {
    let server = TestServer::start_api_only().await;
    let exports = MockServer::start().await;
    let yesterday = today() - Duration::days(1);
    for prefix in ["movie_ids", "tv_series_ids"] {
        Mock::given(method("GET"))
            .and(path(export_path(prefix, today())))
            .respond_with(ResponseTemplate::new(403))
            .mount(&exports)
            .await;
    }
    serve_export(&exports, "movie_ids", yesterday, MOVIES).await;
    serve_export(&exports, "tv_series_ids", yesterday, SHOWS).await;

    run(&server, &exports).await;
    run(&server, &exports).await;

    assert_eq!(titles(&server).await.len(), 3);
    assert_eq!(export_date(&server, "movie").await, Some(yesterday));
    assert_eq!(export_date(&server, "tv").await, Some(yesterday));
}

#[tokio::test]
async fn a_newer_export_replaces_the_set_and_keeps_english_titles() {
    let server = TestServer::start_api_only().await;
    let exports = MockServer::start().await;
    sqlx::query(
        "INSERT INTO tmdb_titles (media_type, tmdb_id, original_title, title, popularity) VALUES
           ('movie', 6, '기생충', 'Parasite', 1.0),
           ('movie', 99, 'Gone Now', NULL, 80.0),
           ('tv', 10, 'Bluey', NULL, 1.0)",
    )
    .execute(&server.pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO tmdb_title_syncs (media_type, export_date) VALUES ('movie', $1), ('tv', $1)",
    )
    .bind(today() - Duration::days(2))
    .execute(&server.pool)
    .await
    .unwrap();
    serve_export(&exports, "movie_ids", today(), MOVIES).await;
    serve_export(&exports, "tv_series_ids", today(), SHOWS).await;

    run(&server, &exports).await;

    assert_eq!(
        titles(&server).await,
        vec![
            row("movie", 6, "기생충", Some("Parasite")),
            row("movie", 1, "Parasite Hunters", None),
            row("tv", 10, "Bluey", None),
        ]
    );
}

#[tokio::test]
async fn nothing_is_downloaded_while_tmdb_is_disabled() {
    let server = TestServer::start_api_only().await;
    let exports = MockServer::start().await;
    set_tmdb(&server, json!({}), false).await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(200))
        .expect(0)
        .mount(&exports)
        .await;

    run(&server, &exports).await;

    assert!(titles(&server).await.is_empty());
}

#[tokio::test]
async fn english_titles_come_from_bounded_weekly_discover_sweeps() {
    let server = TestServer::start_api_only().await;
    let exports = MockServer::start().await;
    let tmdb = MockServer::start().await;
    set_tmdb(
        &server,
        json!({ "api_key": "test-tmdb-key", "base_url": tmdb.uri() }),
        true,
    )
    .await;
    serve_export(&exports, "movie_ids", today(), MOVIES).await;
    serve_export(&exports, "tv_series_ids", today(), SHOWS).await;
    Mock::given(method("GET"))
        .and(path("/discover/movie"))
        .and(query_param("with_original_language", "ko|ja|zh|cn|hi|ta|te|ml|kn|bn|mr|th|ru|uk|ar|fa|he|el|ka|hy|ur|pa|gu|my|km|bg|sr|mk|kk|mn"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "page": 1, "total_pages": 1,
            "results": [{ "id": 6, "title": "Parasite", "original_title": "기생충", "popularity": 41.0 }]
        })))
        .expect(1)
        .mount(&tmdb)
        .await;
    Mock::given(method("GET"))
        .and(path("/discover/movie"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "page": 1, "total_pages": 1,
            "results": [
                { "id": 1, "title": "Parasite Hunters", "original_title": "Parasite Hunters", "popularity": 5.0 },
                { "id": 77, "title": "Brand New", "original_title": "Ganz Neu", "popularity": 9.0 }
            ]
        })))
        .expect(1)
        .mount(&tmdb)
        .await;
    Mock::given(method("GET"))
        .and(path("/discover/tv"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "page": 1, "total_pages": 1,
            "results": [{ "id": 10, "name": "Bluey", "original_name": "Bluey", "popularity": 30.0 }]
        })))
        .expect(2)
        .mount(&tmdb)
        .await;

    run(&server, &exports).await;
    run(&server, &exports).await;

    assert_eq!(
        titles(&server).await,
        vec![
            row("movie", 6, "기생충", Some("Parasite")),
            row("movie", 77, "Ganz Neu", Some("Brand New")),
            row("movie", 1, "Parasite Hunters", None),
            row("tv", 10, "Bluey", None),
        ]
    );
}
