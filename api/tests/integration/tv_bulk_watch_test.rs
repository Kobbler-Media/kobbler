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
        "INSERT INTO tv_episodes (id, season_id, episode_identifier, episode_sort_order, title, air_date)
         VALUES ($1, $2, $3, $4, $5, '2020-01-01')",
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

async fn put_watch_state(server: &TestServer, path: &str, body: Value) -> Value {
    let resp = server.admin_client().put_json(path, &body).await;
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

/// Rewatch: unwatching a show (or its only season) starts it over — no half-watched
/// episode left to resume, and Continue points back at S1E1.
#[tokio::test]
async fn bulk_unwatch_starts_the_show_over() {
    let server = TestServer::start_api_only().await;
    let pool = &server.pool;

    for scope in ["show", "season"] {
        let show_id = create_show(&server, &format!("Rewatch by {scope}")).await;
        let s1 = create_season(pool, show_id, 1).await;
        let mut episodes = Vec::new();
        for n in 1..=4 {
            episodes.push(create_episode(pool, s1, n).await);
        }
        for ep in &episodes[..2] {
            put_watch_state(
                &server,
                &format!("/api/v1/admin/tv/episodes/{ep}/watch-state"),
                json!({ "watched": true }),
            )
            .await;
        }
        put_watch_state(
            &server,
            &format!("/api/v1/admin/tv/episodes/{}/watch-state", episodes[2]),
            json!({ "watched": false, "position_seconds": 300 }),
        )
        .await;

        let path = match scope {
            "show" => format!("/api/v1/admin/tv/shows/{show_id}/watch-state"),
            _ => format!("/api/v1/admin/tv/seasons/{s1}/watch-state"),
        };
        let body = put_watch_state(&server, &path, json!({ "watched": false })).await;
        assert_eq!(
            body["updated_count"], 2,
            "{scope}: only watched episodes sync"
        );

        for ep in &episodes {
            let ws = get_episode_watch_state(&server, *ep).await;
            assert!(
                ws.is_null() || (ws["watched"] == false && ws["position_seconds"].is_null()),
                "{scope}: episode {ep} should be fresh, got {ws}"
            );
        }

        let detail = server
            .admin_client()
            .get(&format!("/api/v1/admin/tv/shows/{show_id}"))
            .await;
        detail.assert_ok();
        let cont = &detail.json::<Value>()["continue_episode"];
        assert_eq!(
            cont["episode_id"],
            json!(episodes[0]),
            "{scope}: continue should restart at S1E1, got {cont}"
        );
        assert_eq!(cont["position_seconds"], 0, "{scope}");
    }
}

// =============================================================================
// Tests — everything before an episode
// =============================================================================

#[tokio::test]
async fn earlier_episodes_are_marked_watched_up_to_but_not_including_the_one_named() {
    let server = TestServer::start_api_only().await;
    let pool = &server.pool;

    let show_id = create_show(&server, "Round By Round").await;
    let specials = create_season(pool, show_id, 0).await;
    sqlx::query("UPDATE tv_seasons SET is_special = true WHERE id = $1")
        .bind(specials)
        .execute(pool)
        .await
        .unwrap();
    let special = create_episode(pool, specials, 1).await;
    let s1 = create_season(pool, show_id, 1).await;
    let s2 = create_season(pool, show_id, 2).await;
    let earlier = [
        create_episode(pool, s1, 1).await,
        create_episode(pool, s1, 2).await,
        create_episode(pool, s2, 1).await,
    ];
    let named = create_episode(pool, s2, 2).await;
    let later = create_episode(pool, s2, 3).await;

    let body = put_watch_state(
        &server,
        &format!("/api/v1/admin/tv/episodes/{named}/watch-state/earlier"),
        json!({ "watched": true }),
    )
    .await;
    assert_eq!(body["updated_count"], 3, "{body}");

    for ep in earlier {
        assert_eq!(
            get_episode_watch_state(&server, ep).await["watched"],
            true,
            "{ep}"
        );
    }
    for ep in [named, later, special] {
        assert_ne!(
            get_episode_watch_state(&server, ep).await["watched"],
            true,
            "{ep}"
        );
    }
}

#[tokio::test]
async fn earlier_episodes_of_an_unknown_episode_is_not_found() {
    let server = TestServer::start_api_only().await;
    let resp = server
        .admin_client()
        .put_json(
            &format!(
                "/api/v1/admin/tv/episodes/{}/watch-state/earlier",
                Uuid::now_v7()
            ),
            &json!({ "watched": true }),
        )
        .await;
    resp.assert_status(StatusCode::NOT_FOUND);
}
