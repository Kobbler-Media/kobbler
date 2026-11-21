//! `POST /api/v1/admin/discover/watchlist` — putting a title that is not in the
//! library yet on the caller's watchlist, without asking for any of it to be
//! downloaded.

use crate::common::*;
use crate::dashboard_test::watchlist_ids;
use reqwest::StatusCode;
use serde_json::{Value, json};
use uuid::Uuid;

const PATH: &str = "/api/v1/admin/discover/watchlist";

fn show_body() -> Value {
    json!({
        "media_type": "tv",
        "provider": "tmdb",
        "provider_id": "7101",
        "title": "Later Show",
        "year": 2025,
        "identifiers": { "tmdb_id": "7101" },
        "poster_url": "https://image.tmdb.org/t/p/w500/later.jpg",
    })
}

fn movie_body() -> Value {
    json!({
        "media_type": "movie",
        "provider": "tmdb",
        "provider_id": "7202",
        "title": "Later Movie",
        "year": 2024,
        "identifiers": { "tmdb_id": "7202" },
    })
}

async fn add(client: &TestClient, body: &Value) -> (String, Uuid) {
    let resp = client.post_json(PATH, body).await;
    resp.assert_ok();
    let json: Value = resp.json();
    (
        json["media_type"].as_str().unwrap().to_string(),
        json["media_id"].as_str().unwrap().parse().unwrap(),
    )
}

#[tokio::test]
async fn a_discover_show_lands_on_the_watchlist_without_wanting_anything() {
    let server = TestServer::start_api_only().await;

    let (media_type, show_id) = add(&server.admin_client(), &show_body()).await;
    assert_eq!(media_type, "show");
    assert_eq!(watchlist_ids(&server).await, vec![show_id.to_string()]);

    let (monitored, mode): (bool, String) =
        sqlx::query_as("SELECT is_monitored, auto_want_mode FROM tv_shows WHERE id = $1")
            .bind(show_id)
            .fetch_one(&server.pool)
            .await
            .unwrap();
    assert!(
        !monitored,
        "a show kept to stream later must not be monitored"
    );
    assert_eq!(mode, "none");

    let wanted: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM tv_episodes te JOIN tv_seasons ts ON ts.id = te.season_id
         WHERE ts.show_id = $1 AND te.want_status <> 'none'",
    )
    .bind(show_id)
    .fetch_one(&server.pool)
    .await
    .unwrap();
    assert_eq!(wanted, 0);
}

#[tokio::test]
async fn a_discover_movie_lands_on_the_watchlist_without_wanting_it() {
    let server = TestServer::start_api_only().await;

    let (media_type, movie_id) = add(&server.admin_client(), &movie_body()).await;
    assert_eq!(media_type, "movie");
    assert_eq!(watchlist_ids(&server).await, vec![movie_id.to_string()]);

    let want: String = sqlx::query_scalar("SELECT want_status FROM movies WHERE id = $1")
        .bind(movie_id)
        .fetch_one(&server.pool)
        .await
        .unwrap();
    assert_eq!(want, "none");
}

#[tokio::test]
async fn adding_twice_reuses_the_library_row_and_leaves_its_settings_alone() {
    let server = TestServer::start_api_only().await;
    let client = server.admin_client();

    let (_, first) = add(&client, &show_body()).await;
    sqlx::query(
        "UPDATE tv_shows SET is_monitored = TRUE, auto_want_mode = 'new_only' WHERE id = $1",
    )
    .bind(first)
    .execute(&server.pool)
    .await
    .unwrap();
    let (_, second) = add(&client, &show_body()).await;
    assert_eq!(first, second);

    let (monitored, mode): (bool, String) =
        sqlx::query_as("SELECT is_monitored, auto_want_mode FROM tv_shows WHERE id = $1")
            .bind(first)
            .fetch_one(&server.pool)
            .await
            .unwrap();
    assert!(monitored);
    assert_eq!(mode, "new_only");
    assert_eq!(watchlist_ids(&server).await, vec![first.to_string()]);
}

#[tokio::test]
async fn rejects_unknown_media_types_and_viewers() {
    let server = TestServer::start_api_only().await;

    let mut book = show_body();
    book["media_type"] = json!("book");
    server
        .admin_client()
        .post_json(PATH, &book)
        .await
        .assert_status(StatusCode::BAD_REQUEST);

    let viewer = create_user_client(&server, "viewer@test.com", "viewer").await;
    viewer
        .post_json(PATH, &show_body())
        .await
        .assert_status(StatusCode::FORBIDDEN);
}
