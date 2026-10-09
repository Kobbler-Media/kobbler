use crate::common::*;
use reqwest::StatusCode;
use serde_json::{Value, json};
use uuid::Uuid;

async fn insert_league(pool: &sqlx::PgPool) -> Uuid {
    let id = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO tv_shows (id, title, content_category, identifiers) VALUES ($1, 'Tally League', 'sport', '{}')",
    )
    .bind(id)
    .execute(pool)
    .await
    .unwrap();
    id
}

async fn insert_season(pool: &sqlx::PgPool, league_id: Uuid, year: i32) -> Uuid {
    let id = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO tv_seasons (id, show_id, season_identifier, season_sort_order, is_monitored, is_special)
         VALUES ($1, $2, $3, $4, true, false)",
    )
    .bind(id)
    .bind(league_id)
    .bind(year.to_string())
    .bind(year)
    .execute(pool)
    .await
    .unwrap();
    id
}

async fn insert_event(pool: &sqlx::PgPool, season_id: Uuid, n: i32, want_status: &str) -> Uuid {
    let id = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO tv_episodes (id, season_id, episode_identifier, episode_sort_order, title, want_status)
         VALUES ($1, $2, $3, $4, $5, $6)",
    )
    .bind(id)
    .bind(season_id)
    .bind(n.to_string())
    .bind(n)
    .bind(format!("Event {n}"))
    .bind(want_status)
    .execute(pool)
    .await
    .unwrap();
    id
}

async fn attach_file(pool: &sqlx::PgPool, episode_id: Uuid) {
    let file_id = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO tv_episode_files (id, file_path, file_size, format, available)
         VALUES ($1, $2, 100, 'mkv', true)",
    )
    .bind(file_id)
    .bind(format!("/tmp/sports-tally-{file_id}.mkv"))
    .execute(pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO tv_episode_file_episodes (episode_file_id, episode_id) VALUES ($1, $2)",
    )
    .bind(file_id)
    .bind(episode_id)
    .execute(pool)
    .await
    .unwrap();
}

async fn mark_watched(server: &TestServer, episode_id: Uuid) {
    server
        .admin_client()
        .put_json(
            &format!("/api/v1/admin/tv/episodes/{episode_id}/watch-state"),
            &json!({ "watched": true }),
        )
        .await
        .assert_ok();
}

async fn get_json(server: &TestServer, path: &str) -> Value {
    let resp = server.admin_client().get(path).await;
    resp.assert_status(StatusCode::OK);
    resp.json()
}

struct Fixture {
    league: Uuid,
    season: Uuid,
    other_season: Uuid,
    watched_event: Uuid,
}

/// Season 2025: one event in the library and watched, one wanted, one untouched.
/// Season 2024: one wanted event, so league totals must span seasons.
async fn fixture(server: &TestServer) -> Fixture {
    let pool = &server.pool;
    let league = insert_league(pool).await;
    let season = insert_season(pool, league, 2025).await;
    let other_season = insert_season(pool, league, 2024).await;

    let watched_event = insert_event(pool, season, 1, "imported").await;
    attach_file(pool, watched_event).await;
    mark_watched(server, watched_event).await;
    insert_event(pool, season, 2, "wanted").await;
    insert_event(pool, season, 3, "none").await;
    insert_event(pool, other_season, 1, "wanted").await;

    Fixture {
        league,
        season,
        other_season,
        watched_event,
    }
}

#[tokio::test]
async fn test_season_list_reports_library_and_watched_counts() {
    let server = TestServer::start_api_only().await;
    let f = fixture(&server).await;

    let body = get_json(
        &server,
        &format!("/api/v1/admin/sports/leagues/{}/seasons", f.league),
    )
    .await;
    let items = body["items"].as_array().unwrap();

    let season = items
        .iter()
        .find(|s| s["id"] == f.season.to_string())
        .unwrap();
    assert_eq!(season["actual_episode_count"], 3);
    assert_eq!(season["in_library_event_count"], 1);
    assert_eq!(season["wanted_event_count"], 1);
    assert_eq!(season["watched_event_count"], 1);

    let other = items
        .iter()
        .find(|s| s["id"] == f.other_season.to_string())
        .unwrap();
    assert_eq!(other["in_library_event_count"], 0);
    assert_eq!(other["wanted_event_count"], 1);
    assert_eq!(other["watched_event_count"], 0);
}

#[tokio::test]
async fn test_league_detail_reports_totals_across_all_seasons() {
    let server = TestServer::start_api_only().await;
    let f = fixture(&server).await;

    let league = get_json(
        &server,
        &format!("/api/v1/admin/sports/leagues/{}", f.league),
    )
    .await;
    assert_eq!(league["event_count"], 4);
    assert_eq!(league["in_library_event_count"], 1);
    assert_eq!(league["wanted_event_count"], 2);
    assert_eq!(league["watched_event_count"], 1);
}

#[tokio::test]
async fn test_season_detail_carries_viewer_watch_state() {
    let server = TestServer::start_api_only().await;
    let f = fixture(&server).await;

    let season = get_json(
        &server,
        &format!("/api/v1/admin/sports/seasons/{}", f.season),
    )
    .await;
    for event in season["episodes"].as_array().unwrap() {
        let expected = event["id"] == f.watched_event.to_string();
        assert_eq!(event["watched"], expected, "event {}", event["id"]);
    }
}

#[tokio::test]
async fn test_watch_counts_are_per_user() {
    let server = TestServer::start_api_only().await;
    let f = fixture(&server).await;

    let other_user = create_user_client(&server, "sports-tally-viewer@test.local", "viewer").await;
    let resp = other_user
        .get(&format!(
            "/api/v1/admin/sports/leagues/{}/seasons",
            f.league
        ))
        .await;
    resp.assert_ok();
    let body: Value = resp.json();
    for season in body["items"].as_array().unwrap() {
        assert_eq!(season["watched_event_count"], 0);
    }
}
