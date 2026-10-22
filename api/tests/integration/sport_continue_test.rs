//! Sports resume on the newest event, not the oldest unwatched one: a league is followed
//! live, so last season's races are never "next".

use crate::common::*;
use reqwest::StatusCode;
use serde_json::{Value, json};
use uuid::Uuid;

struct League {
    show_id: Uuid,
    events: Vec<Uuid>,
}

/// Two seasons of two events each, aired oldest to newest, plus one future event in the
/// newest season that is not available yet.
async fn sport_league(server: &TestServer) -> League {
    let pool = &server.pool;
    let resp = server
        .admin_client()
        .post_json(
            "/api/v1/admin/tv/shows",
            &json!({ "title": "Test League", "year": 2025 }),
        )
        .await;
    resp.assert_status(StatusCode::CREATED);
    let show_id: Uuid = resp.json::<Value>()["id"]
        .as_str()
        .unwrap()
        .parse()
        .unwrap();
    sqlx::query("UPDATE tv_shows SET content_category = 'sport' WHERE id = $1")
        .bind(show_id)
        .execute(pool)
        .await
        .unwrap();

    let mut events = Vec::new();
    for (season, days_ago) in [(1, [400, 380]), (2, [30, 10])] {
        let season_id = Uuid::now_v7();
        sqlx::query(
            "INSERT INTO tv_seasons (id, show_id, season_identifier, season_sort_order, is_monitored, is_special)
             VALUES ($1, $2, $3, $4, true, false)",
        )
        .bind(season_id)
        .bind(show_id)
        .bind(season.to_string())
        .bind(season)
        .execute(pool)
        .await
        .unwrap();
        for (i, ago) in days_ago.into_iter().chain([-20]).enumerate() {
            if season == 1 && ago < 0 {
                continue;
            }
            let id = Uuid::now_v7();
            sqlx::query(
                "INSERT INTO tv_episodes (id, season_id, episode_identifier, episode_sort_order, title, air_date)
                 VALUES ($1, $2, $3, $4, $5, CURRENT_DATE - $6::int)",
            )
            .bind(id)
            .bind(season_id)
            .bind((i + 1).to_string())
            .bind(i as i32 + 1)
            .bind(format!("Event S{season}E{}", i + 1))
            .bind(ago)
            .execute(pool)
            .await
            .unwrap();
            if ago > 0 {
                create_stub_episode_file(pool, id).await;
            }
            events.push(id);
        }
    }
    League { show_id, events }
}

async fn put_state(server: &TestServer, episode_id: Uuid, body: Value) {
    server
        .admin_client()
        .put_json(
            &format!("/api/v1/admin/tv/episodes/{episode_id}/watch-state"),
            &body,
        )
        .await
        .assert_ok();
}

async fn continue_episode(server: &TestServer, show_id: Uuid) -> Option<String> {
    let resp = server
        .admin_client()
        .get(&format!("/api/v1/admin/tv/shows/{show_id}"))
        .await;
    resp.assert_ok();
    let json: Value = resp.json();
    assert_eq!(json["content_category"], "sport");
    json["continue_episode"]["episode_id"]
        .as_str()
        .map(String::from)
}

async fn watch_next_card(server: &TestServer) -> Option<Value> {
    let resp = server.admin_client().get("/api/v1/admin/discover").await;
    resp.assert_ok();
    let json: Value = resp.json();
    json["watch_next"]
        .as_array()
        .unwrap()
        .iter()
        .find(|i| i["title"] == "Test League")
        .cloned()
}

#[tokio::test]
async fn no_history_starts_on_the_newest_available_event() {
    let server = TestServer::start_api_only().await;
    let league = sport_league(&server).await;
    let newest_available = league.events[3];

    assert_eq!(
        continue_episode(&server, league.show_id).await,
        Some(newest_available.to_string())
    );
}

#[tokio::test]
async fn after_an_old_event_the_newest_event_is_next_not_the_one_after_it() {
    let server = TestServer::start_api_only().await;
    let league = sport_league(&server).await;
    put_state(&server, league.events[0], json!({ "watched": true })).await;

    assert_eq!(
        continue_episode(&server, league.show_id).await,
        Some(league.events[3].to_string())
    );
    let card = watch_next_card(&server)
        .await
        .expect("newest event should be on Watch Next");
    assert_eq!(card["media_id"], league.events[3].to_string());
}

#[tokio::test]
async fn watching_the_newest_event_leaves_nothing_behind_it() {
    let server = TestServer::start_api_only().await;
    let league = sport_league(&server).await;
    put_state(&server, league.events[3], json!({ "watched": true })).await;

    assert_eq!(continue_episode(&server, league.show_id).await, None);
    assert!(watch_next_card(&server).await.is_none());
}

#[tokio::test]
async fn watching_an_old_event_after_the_newest_does_not_bring_older_ones_back() {
    let server = TestServer::start_api_only().await;
    let league = sport_league(&server).await;
    put_state(&server, league.events[3], json!({ "watched": true })).await;
    put_state(&server, league.events[0], json!({ "watched": true })).await;

    assert_eq!(continue_episode(&server, league.show_id).await, None);
    assert!(watch_next_card(&server).await.is_none());
}

#[tokio::test]
async fn an_event_in_progress_is_resumed() {
    let server = TestServer::start_api_only().await;
    let league = sport_league(&server).await;
    put_state(&server, league.events[3], json!({ "watched": true })).await;
    put_state(
        &server,
        league.events[1],
        json!({ "watched": false, "position_seconds": 600 }),
    )
    .await;

    assert_eq!(
        continue_episode(&server, league.show_id).await,
        Some(league.events[1].to_string())
    );
    let card = watch_next_card(&server)
        .await
        .expect("in-progress event should be on Watch Next");
    assert_eq!(card["media_id"], league.events[1].to_string());
    assert_eq!(card["position_seconds"], 600);
}

#[tokio::test]
async fn watchlisted_league_without_history_offers_the_newest_event() {
    let server = TestServer::start_api_only().await;
    let league = sport_league(&server).await;
    server
        .admin_client()
        .post_json(
            "/api/v1/admin/watchlist",
            &json!({ "media_type": "show", "media_id": league.show_id }),
        )
        .await
        .assert_status(StatusCode::NO_CONTENT);

    let card = watch_next_card(&server)
        .await
        .expect("watchlisted league should be on Watch Next");
    assert_eq!(card["media_id"], league.events[3].to_string());
}
