use crate::common::*;
use reqwest::StatusCode;
use serde_json::{Value, json};
use uuid::Uuid;

// =============================================================================
// Helpers
// =============================================================================

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

async fn create_episode(pool: &sqlx::PgPool, season_id: Uuid, episode_num: i32) -> Uuid {
    let id = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO tv_episodes (id, season_id, episode_identifier, episode_sort_order, title)
         VALUES ($1, $2, $3, $4, $5)",
    )
    .bind(id)
    .bind(season_id)
    .bind(episode_num.to_string())
    .bind(episode_num)
    .bind(format!("Episode {episode_num}"))
    .execute(pool)
    .await
    .unwrap();
    id
}

async fn get_episode_watch_state(server: &TestServer, episode_id: Uuid) -> Value {
    let resp = server
        .admin_client()
        .get(&format!(
            "/api/v1/admin/tv/episodes/{episode_id}/watch-state"
        ))
        .await;
    resp.assert_ok();
    resp.json()
}

// =============================================================================
// Tests — Show-level bulk watch state
// =============================================================================

#[tokio::test]
async fn test_bulk_mark_show_watched() {
    let server = TestServer::start_api_only().await;
    let pool = &server.pool;

    let show_id = create_show(&server, "Bulk Watch Show").await;
    let s1 = create_season(pool, show_id, 1).await;
    let s2 = create_season(pool, show_id, 2).await;
    let ep1 = create_episode(pool, s1, 1).await;
    let ep2 = create_episode(pool, s1, 2).await;
    let ep3 = create_episode(pool, s2, 1).await;

    // Mark all as watched
    let resp = server
        .admin_client()
        .put_json(
            &format!("/api/v1/admin/tv/shows/{show_id}/watch-state"),
            &json!({ "watched": true }),
        )
        .await;
    resp.assert_ok();
    let body: Value = resp.json();
    assert_eq!(body["updated_count"], 3);

    // Verify all episodes are watched
    for ep_id in [ep1, ep2, ep3] {
        let ws = get_episode_watch_state(&server, ep_id).await;
        assert_eq!(ws["watched"], true, "Episode {ep_id} should be watched");
        assert!(ws["watched_at"].is_string(), "watched_at should be set");
    }
}

#[tokio::test]
async fn test_bulk_mark_show_unwatched() {
    let server = TestServer::start_api_only().await;
    let pool = &server.pool;

    let show_id = create_show(&server, "Bulk Unwatch Show").await;
    let s1 = create_season(pool, show_id, 1).await;
    let ep1 = create_episode(pool, s1, 1).await;
    let ep2 = create_episode(pool, s1, 2).await;

    // First mark as watched
    server
        .admin_client()
        .put_json(
            &format!("/api/v1/admin/tv/shows/{show_id}/watch-state"),
            &json!({ "watched": true }),
        )
        .await
        .assert_ok();

    // Now mark as unwatched
    let resp = server
        .admin_client()
        .put_json(
            &format!("/api/v1/admin/tv/shows/{show_id}/watch-state"),
            &json!({ "watched": false }),
        )
        .await;
    resp.assert_ok();
    let body: Value = resp.json();
    assert_eq!(body["updated_count"], 2);

    // Verify all episodes are unwatched
    for ep_id in [ep1, ep2] {
        let ws = get_episode_watch_state(&server, ep_id).await;
        assert_eq!(ws["watched"], false, "Episode {ep_id} should be unwatched");
        assert!(
            ws["watched_at"].is_null(),
            "watched_at should be null for {ep_id}"
        );
    }
}

#[tokio::test]
async fn test_bulk_mark_show_watched_clears_position() {
    let server = TestServer::start_api_only().await;
    let pool = &server.pool;

    let show_id = create_show(&server, "Position Clear Show").await;
    let s1 = create_season(pool, show_id, 1).await;
    let ep1 = create_episode(pool, s1, 1).await;

    // Set a position on the episode
    server
        .admin_client()
        .put_json(
            &format!("/api/v1/admin/tv/episodes/{ep1}/watch-state"),
            &json!({ "watched": false, "position_seconds": 300 }),
        )
        .await
        .assert_ok();

    // Bulk mark watched
    server
        .admin_client()
        .put_json(
            &format!("/api/v1/admin/tv/shows/{show_id}/watch-state"),
            &json!({ "watched": true }),
        )
        .await
        .assert_ok();

    // Position should be cleared
    let ws = get_episode_watch_state(&server, ep1).await;
    assert_eq!(ws["watched"], true);
    assert!(
        ws["position_seconds"].is_null(),
        "position_seconds should be cleared"
    );
}

// =============================================================================
// Tests — Season-level bulk watch state
// =============================================================================

#[tokio::test]
async fn test_bulk_mark_season_watched() {
    let server = TestServer::start_api_only().await;
    let pool = &server.pool;

    let show_id = create_show(&server, "Season Watch Show").await;
    let s1 = create_season(pool, show_id, 1).await;
    let s2 = create_season(pool, show_id, 2).await;
    let ep1 = create_episode(pool, s1, 1).await;
    let ep2 = create_episode(pool, s1, 2).await;
    let ep3 = create_episode(pool, s2, 1).await;

    // Mark only season 1 as watched
    let resp = server
        .admin_client()
        .put_json(
            &format!("/api/v1/admin/tv/seasons/{s1}/watch-state"),
            &json!({ "watched": true }),
        )
        .await;
    resp.assert_ok();
    let body: Value = resp.json();
    assert_eq!(body["updated_count"], 2);

    // Season 1 episodes should be watched
    for ep_id in [ep1, ep2] {
        let ws = get_episode_watch_state(&server, ep_id).await;
        assert_eq!(ws["watched"], true, "S1 episode {ep_id} should be watched");
    }

    // Season 2 episode should NOT be watched (no watch state row exists)
    let ws = get_episode_watch_state(&server, ep3).await;
    assert!(
        ws.is_null() || ws["watched"] == false,
        "S2 episode should not be watched"
    );
}

#[tokio::test]
async fn test_bulk_mark_season_unwatched() {
    let server = TestServer::start_api_only().await;
    let pool = &server.pool;

    let show_id = create_show(&server, "Season Unwatch Show").await;
    let s1 = create_season(pool, show_id, 1).await;
    let s2 = create_season(pool, show_id, 2).await;
    let ep1 = create_episode(pool, s1, 1).await;
    let ep2 = create_episode(pool, s2, 1).await;

    // Mark both seasons as watched
    for sid in [s1, s2] {
        server
            .admin_client()
            .put_json(
                &format!("/api/v1/admin/tv/seasons/{sid}/watch-state"),
                &json!({ "watched": true }),
            )
            .await
            .assert_ok();
    }

    // Unwatch only season 1
    let resp = server
        .admin_client()
        .put_json(
            &format!("/api/v1/admin/tv/seasons/{s1}/watch-state"),
            &json!({ "watched": false }),
        )
        .await;
    resp.assert_ok();
    let body: Value = resp.json();
    assert_eq!(body["updated_count"], 1);

    // Season 1 episode unwatched
    let ws = get_episode_watch_state(&server, ep1).await;
    assert_eq!(ws["watched"], false);

    // Season 2 episode still watched
    let ws = get_episode_watch_state(&server, ep2).await;
    assert_eq!(ws["watched"], true);
}

// =============================================================================
// Tests — Idempotency and edge cases
// =============================================================================

#[tokio::test]
async fn test_bulk_watch_idempotent() {
    let server = TestServer::start_api_only().await;
    let pool = &server.pool;

    let show_id = create_show(&server, "Idempotent Show").await;
    let s1 = create_season(pool, show_id, 1).await;
    let _ep1 = create_episode(pool, s1, 1).await;

    // Mark watched twice
    for _ in 0..2 {
        let resp = server
            .admin_client()
            .put_json(
                &format!("/api/v1/admin/tv/shows/{show_id}/watch-state"),
                &json!({ "watched": true }),
            )
            .await;
        resp.assert_ok();
        let body: Value = resp.json();
        assert_eq!(body["updated_count"], 1);
    }
}

#[tokio::test]
async fn test_bulk_unwatch_no_watched_episodes() {
    let server = TestServer::start_api_only().await;
    let pool = &server.pool;

    let show_id = create_show(&server, "No Watch Show").await;
    let s1 = create_season(pool, show_id, 1).await;
    let _ep1 = create_episode(pool, s1, 1).await;

    // Unwatch when nothing is watched
    let resp = server
        .admin_client()
        .put_json(
            &format!("/api/v1/admin/tv/shows/{show_id}/watch-state"),
            &json!({ "watched": false }),
        )
        .await;
    resp.assert_ok();
    let body: Value = resp.json();
    assert_eq!(body["updated_count"], 0);
}

#[tokio::test]
async fn test_bulk_watch_nonexistent_show() {
    let server = TestServer::start_api_only().await;
    let fake_id = Uuid::now_v7();

    let resp = server
        .admin_client()
        .put_json(
            &format!("/api/v1/admin/tv/shows/{fake_id}/watch-state"),
            &json!({ "watched": true }),
        )
        .await;
    resp.assert_status(StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn test_bulk_watch_nonexistent_season() {
    let server = TestServer::start_api_only().await;
    let fake_id = Uuid::now_v7();

    let resp = server
        .admin_client()
        .put_json(
            &format!("/api/v1/admin/tv/seasons/{fake_id}/watch-state"),
            &json!({ "watched": true }),
        )
        .await;
    resp.assert_status(StatusCode::NOT_FOUND);
}
