use crate::common::*;
use reqwest::StatusCode;
use serde_json::{Value, json};
use time::macros::date;
use uuid::Uuid;

async fn create_show(server: &TestServer, title: &str) -> Uuid {
    let resp = server
        .admin_client()
        .post_json(
            "/api/v1/admin/tv/shows",
            &json!({ "title": title, "year": 2024 }),
        )
        .await;
    resp.assert_status(StatusCode::CREATED);
    let json: Value = resp.json();
    json["id"].as_str().unwrap().parse().unwrap()
}

async fn create_season(pool: &sqlx::PgPool, show_id: Uuid, season_num: i32) -> Uuid {
    let id = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO tv_seasons (id, show_id, season_identifier, season_sort_order, is_monitored, is_special)
         VALUES ($1, $2, $3, $4, true, false)",
    )
    .bind(id)
    .bind(show_id)
    .bind(season_num.to_string())
    .bind(season_num)
    .execute(pool)
    .await
    .unwrap();
    id
}

async fn create_episode_with_air_date(
    pool: &sqlx::PgPool,
    season_id: Uuid,
    episode_num: i32,
    air_date: time::Date,
) -> Uuid {
    let id = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO tv_episodes (id, season_id, episode_identifier, episode_sort_order, title, air_date)
         VALUES ($1, $2, $3, $4, $5, $6)",
    )
    .bind(id)
    .bind(season_id)
    .bind(episode_num.to_string())
    .bind(episode_num)
    .bind(format!("Episode {episode_num}"))
    .bind(air_date)
    .execute(pool)
    .await
    .unwrap();
    id
}

async fn get_calendar(server: &TestServer, from: &str, to: &str) -> Value {
    let resp = server
        .admin_client()
        .get(&format!("/api/v1/admin/calendar?from={from}&to={to}"))
        .await;
    resp.assert_ok();
    resp.json()
}

#[tokio::test]
async fn test_calendar_sport_link_uses_events_route() {
    // Sport calendar items must link to /sports/events/{id} — the actual
    // SvelteKit route is web/src/routes/sports/events/[id]; using
    // /sports/episodes/{id} 404s on the frontend.
    let server = TestServer::start_api_only().await;
    let pool = &server.pool;

    let show_id = create_show(&server, "F1 2024").await;
    sqlx::query(
        "UPDATE tv_shows SET content_category = 'sport', is_monitored = TRUE WHERE id = $1",
    )
    .bind(show_id)
    .execute(pool)
    .await
    .unwrap();

    let season_id = create_season(pool, show_id, 1).await;
    let air = date!(2024 - 06 - 15);
    let ep = create_episode_with_air_date(pool, season_id, 1, air).await;

    let json = get_calendar(&server, "2024-06-01", "2024-07-01").await;
    let items = json["items"].as_array().unwrap();
    let sport = items
        .iter()
        .find(|i| i["id"] == ep.to_string())
        .expect("expected sport item in calendar");

    assert_eq!(sport["media_type"], "sport");
    assert_eq!(sport["link"], format!("/sports/events/{ep}"));
}

#[tokio::test]
async fn test_calendar_sport_subtitle_omits_tv_episode_format() {
    // Sport events must not show the TV-style "S{season}E{episode_id} - ..."
    // prefix — episode_identifier is an opaque event id (e.g. "evt-2403662")
    // that's noise to users. The subtitle should just be the event name.
    let server = TestServer::start_api_only().await;
    let pool = &server.pool;

    let show_id = create_show(&server, "SBK").await;
    sqlx::query(
        "UPDATE tv_shows SET content_category = 'sport', is_monitored = TRUE WHERE id = $1",
    )
    .bind(show_id)
    .execute(pool)
    .await
    .unwrap();

    let season_id = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO tv_seasons (id, show_id, season_identifier, season_sort_order, is_monitored, is_special)
         VALUES ($1, $2, '2026', 1, true, false)",
    )
    .bind(season_id)
    .bind(show_id)
    .execute(pool)
    .await
    .unwrap();

    let ep = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO tv_episodes (id, season_id, episode_identifier, episode_sort_order, title, air_date)
         VALUES ($1, $2, 'evt-2403662', 1, 'Motul Hungarian Round Race 1', $3)",
    )
    .bind(ep)
    .bind(season_id)
    .bind(date!(2026 - 05 - 02))
    .execute(pool)
    .await
    .unwrap();

    let json = get_calendar(&server, "2026-05-01", "2026-06-01").await;
    let items = json["items"].as_array().unwrap();
    let sport = items
        .iter()
        .find(|i| i["id"] == ep.to_string())
        .expect("expected sport item in calendar");

    let subtitle = sport["subtitle"].as_str().unwrap();
    // Matches dashboard's format_sport_label: "{year} {event title}",
    // with the show title prefix stripped if present.
    assert_eq!(subtitle, "2026 Motul Hungarian Round Race 1");
    assert!(
        !subtitle.contains("evt-"),
        "sport subtitle leaked event identifier: {subtitle}"
    );
    assert!(
        !subtitle.contains("E2026") && !subtitle.contains("S2026E"),
        "sport subtitle used TV-style SxxExx format: {subtitle}"
    );
}

#[tokio::test]
async fn test_calendar_sport_subtitle_falls_back_to_season_when_no_title() {
    let server = TestServer::start_api_only().await;
    let pool = &server.pool;

    let show_id = create_show(&server, "F1").await;
    sqlx::query(
        "UPDATE tv_shows SET content_category = 'sport', is_monitored = TRUE WHERE id = $1",
    )
    .bind(show_id)
    .execute(pool)
    .await
    .unwrap();

    let season_id = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO tv_seasons (id, show_id, season_identifier, season_sort_order, is_monitored, is_special)
         VALUES ($1, $2, '2026', 1, true, false)",
    )
    .bind(season_id)
    .bind(show_id)
    .execute(pool)
    .await
    .unwrap();

    let ep = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO tv_episodes (id, season_id, episode_identifier, episode_sort_order, air_date)
         VALUES ($1, $2, 'evt-99', 1, $3)",
    )
    .bind(ep)
    .bind(season_id)
    .bind(date!(2026 - 05 - 02))
    .execute(pool)
    .await
    .unwrap();

    let json = get_calendar(&server, "2026-05-01", "2026-06-01").await;
    let items = json["items"].as_array().unwrap();
    let sport = items
        .iter()
        .find(|i| i["id"] == ep.to_string())
        .expect("expected sport item in calendar");

    assert_eq!(sport["subtitle"].as_str().unwrap(), "2026");
}

#[tokio::test]
async fn test_calendar_tv_link_uses_episodes_route() {
    let server = TestServer::start_api_only().await;
    let pool = &server.pool;

    let show_id = create_show(&server, "Breaking Bad").await;
    sqlx::query("UPDATE tv_shows SET is_monitored = TRUE WHERE id = $1")
        .bind(show_id)
        .execute(pool)
        .await
        .unwrap();

    let season_id = create_season(pool, show_id, 1).await;
    let air = date!(2024 - 06 - 15);
    let ep = create_episode_with_air_date(pool, season_id, 1, air).await;

    let json = get_calendar(&server, "2024-06-01", "2024-07-01").await;
    let items = json["items"].as_array().unwrap();
    let tv = items
        .iter()
        .find(|i| i["id"] == ep.to_string())
        .expect("expected tv item in calendar");

    assert_eq!(tv["media_type"], "tv");
    assert_eq!(tv["link"], format!("/tv/episodes/{ep}"));
}

#[tokio::test]
async fn test_calendar_book_link_uses_books_route() {
    // Book calendar items must link to /library/books/{id}; the route
    // /library/{id} doesn't exist on the frontend.
    let server = TestServer::start_api_only().await;
    let pool = &server.pool;

    let book_id = create_book(&server, "Wandering Inn 11").await;
    let release = date!(2024 - 06 - 15);
    sqlx::query("UPDATE books SET release_date = $1, want_status = 'wanted' WHERE id = $2")
        .bind(release)
        .bind(book_id)
        .execute(pool)
        .await
        .unwrap();

    let json = get_calendar(&server, "2024-06-01", "2024-07-01").await;
    let items = json["items"].as_array().unwrap();
    let book = items
        .iter()
        .find(|i| i["id"] == book_id.to_string())
        .expect("expected book item in calendar");

    assert_eq!(book["media_type"], "book");
    assert_eq!(book["link"], format!("/library/books/{book_id}"));
}
