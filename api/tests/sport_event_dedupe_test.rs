//! Sport-event de-duplication (the duplicate-download root cause) + the re-want guard.
//!
//! C: `sport_event_dedupe` merges two `tv_episodes` rows that represent one real session
//!    (two TheSportsDB event IDs) into a single natural-key row, preserving library files.
//! D: an already-imported sport event must not be re-wanted — `list_active_watch_statuses`
//!    (the tv_monitor reconciler's source) excludes episodes that have library files.

use crate::common::*;
use api::jobs::handlers::SportEventDedupeHandler;
use api::jobs::worker::JobHandler;
use api::repositories::tv_episodes;
use serde_json::json;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

async fn create_sport_show(pool: &sqlx::PgPool) -> (Uuid, Uuid) {
    let show_id = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO tv_shows (id, title, content_category, identifiers) VALUES ($1, $2, 'sport', '{}')",
    )
    .bind(show_id)
    .bind("Moto3")
    .execute(pool)
    .await
    .unwrap();

    let season_id = Uuid::now_v7();
    sqlx::query(
        r#"INSERT INTO tv_seasons (id, show_id, season_identifier, season_sort_order, is_monitored, is_special, auto_want_mode)
           VALUES ($1, $2, '2026', 1, false, false, 'none')"#,
    )
    .bind(season_id)
    .bind(show_id)
    .execute(pool)
    .await
    .unwrap();

    (show_id, season_id)
}

#[allow(clippy::too_many_arguments)]
async fn insert_episode(
    pool: &sqlx::PgPool,
    season_id: Uuid,
    identifier: &str,
    sort_order: i32,
    title: &str,
    provider_id: &str,
    want_status: &str,
) -> Uuid {
    let id = Uuid::now_v7();
    sqlx::query(
        r#"INSERT INTO tv_episodes
           (id, season_id, episode_identifier, episode_sort_order, title, want_status, identifiers)
           VALUES ($1, $2, $3, $4, $5, $6, $7)"#,
    )
    .bind(id)
    .bind(season_id)
    .bind(identifier)
    .bind(sort_order)
    .bind(title)
    .bind(want_status)
    .bind(json!({ "thesportsdb_id": provider_id }))
    .execute(pool)
    .await
    .unwrap();
    id
}

async fn season_episode_count(pool: &sqlx::PgPool, season_id: Uuid) -> i64 {
    sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM tv_episodes WHERE season_id = $1")
        .bind(season_id)
        .fetch_one(pool)
        .await
        .unwrap()
}

async fn episode_exists(pool: &sqlx::PgPool, id: Uuid) -> bool {
    sqlx::query_scalar::<_, bool>("SELECT EXISTS(SELECT 1 FROM tv_episodes WHERE id = $1)")
        .bind(id)
        .fetch_one(pool)
        .await
        .unwrap()
}

#[tokio::test]
async fn dedupe_merges_duplicate_sport_events() {
    let server = TestServer::start_api_only().await;
    let (_show_id, season_id) = create_sport_show(&server.pool).await;

    // Same real session (Hungary FP1, round 5) under two TheSportsDB event IDs.
    let title = "Hungary Free Practice 1 Moto3";
    let loser = insert_episode(
        &server.pool,
        season_id,
        "evt-ev-567724",
        5000,
        title,
        "ev-567724",
        "searching",
    )
    .await;
    let survivor = insert_episode(
        &server.pool,
        season_id,
        "evt-2417416",
        5001,
        title,
        "2417416",
        "wanted",
    )
    .await;
    // The survivor candidate has the library file.
    let file_id = create_stub_episode_file(&server.pool, survivor).await;

    SportEventDedupeHandler::new(server.pool.clone())
        .execute(Uuid::now_v7(), json!({}), CancellationToken::new())
        .await
        .expect("dedupe job");

    // One row remains, carrying the natural-key identifier; the file-bearing row survived.
    assert_eq!(season_episode_count(&server.pool, season_id).await, 1);
    assert!(episode_exists(&server.pool, survivor).await);
    assert!(!episode_exists(&server.pool, loser).await);

    let surviving_id: String =
        sqlx::query_scalar::<_, String>("SELECT episode_identifier FROM tv_episodes WHERE id = $1")
            .bind(survivor)
            .fetch_one(&server.pool)
            .await
            .unwrap();
    assert_eq!(surviving_id, "evt-r5-free-practice-1");

    // The library file is still linked to the survivor.
    let file_linked: bool = sqlx::query_scalar::<_, bool>(
        "SELECT EXISTS(SELECT 1 FROM tv_episode_file_episodes WHERE episode_id = $1 AND episode_file_id = $2)",
    )
    .bind(survivor)
    .bind(file_id)
    .fetch_one(&server.pool)
    .await
    .unwrap();
    assert!(file_linked, "survivor must keep its library file");
}

#[tokio::test]
async fn dedupe_is_idempotent() {
    let server = TestServer::start_api_only().await;
    let (_show_id, season_id) = create_sport_show(&server.pool).await;
    insert_episode(
        &server.pool,
        season_id,
        "evt-r5-race",
        5003,
        "Hungary Grand Prix Moto3",
        "p1",
        "wanted",
    )
    .await;

    for _ in 0..2 {
        SportEventDedupeHandler::new(server.pool.clone())
            .execute(Uuid::now_v7(), json!({}), CancellationToken::new())
            .await
            .expect("dedupe job");
    }
    // Already-canonical row is untouched.
    assert_eq!(season_episode_count(&server.pool, season_id).await, 1);
}

#[tokio::test]
async fn imported_event_excluded_from_stale_watch_reconcile() {
    // The re-want bug: the tv_monitor reconciler reset imported sport events back to
    // 'wanted'. Its source query must exclude episodes that already have library files.
    let server = TestServer::start_api_only().await;
    let (_show_id, season_id) = create_sport_show(&server.pool).await;

    let imported = insert_episode(
        &server.pool,
        season_id,
        "evt-r5-race",
        5003,
        "Hungary Grand Prix Moto3",
        "p1",
        "searching",
    )
    .await;
    create_stub_episode_file(&server.pool, imported).await; // now in library

    let still_searching = insert_episode(
        &server.pool,
        season_id,
        "evt-r6-race",
        6003,
        "Italy Grand Prix Moto3",
        "p2",
        "searching",
    )
    .await;

    let active = tv_episodes::list_active_watch_statuses(&server.pool)
        .await
        .unwrap();
    let ids: Vec<Uuid> = active.iter().map(|e| e.id).collect();

    assert!(
        !ids.contains(&imported),
        "an episode with library files must not be a stale-watch reset candidate"
    );
    assert!(
        ids.contains(&still_searching),
        "a searching episode without files is still reconciled normally"
    );
}

/// "Unwant All" on a league takes back every outstanding event want. Sports
/// leagues are `tv_shows` rows, so this shares the TV clearing logic — the test
/// is here to pin the sports routes to it.
#[tokio::test]
async fn test_unwant_all_missing_for_league() {
    let server = TestServer::start_api_only().await;
    let (league_id, season_id) = create_sport_show(&server.pool).await;
    let e1 = insert_episode(&server.pool, season_id, "1", 1, "Race 1", "p1", "wanted").await;
    let e2 = insert_episode(&server.pool, season_id, "2", 2, "Race 2", "p2", "searching").await;

    let resp = server
        .admin_client()
        .delete(&format!(
            "/api/v1/admin/sports/leagues/{}/want-all-missing",
            league_id
        ))
        .await;
    resp.assert_ok();
    assert_eq!(resp.json::<serde_json::Value>()["unwanted_count"], 2);

    for id in [e1, e2] {
        let status: String =
            sqlx::query_scalar("SELECT want_status FROM tv_episodes WHERE id = $1")
                .bind(id)
                .fetch_one(&server.pool)
                .await
                .unwrap();
        assert_eq!(status, "none");
    }
}

/// A season unwant has to switch a refilling auto-want off, or `sports_monitor`
/// re-wants everything on its next pass.
#[tokio::test]
async fn test_unwant_sports_season_disables_refilling_auto_want() {
    let server = TestServer::start_api_only().await;
    let (_league_id, season_id) = create_sport_show(&server.pool).await;
    insert_episode(&server.pool, season_id, "1", 1, "Race 1", "p1", "wanted").await;

    sqlx::query("UPDATE tv_seasons SET auto_want_mode = 'all' WHERE id = $1")
        .bind(season_id)
        .execute(&server.pool)
        .await
        .unwrap();

    let resp = server
        .admin_client()
        .delete(&format!("/api/v1/admin/sports/seasons/{}/want", season_id))
        .await;
    resp.assert_ok();
    let json: serde_json::Value = resp.json();
    assert_eq!(json["unwanted_count"], 1);
    assert_eq!(json["auto_want_disabled"], true);

    let mode: String = sqlx::query_scalar("SELECT auto_want_mode FROM tv_seasons WHERE id = $1")
        .bind(season_id)
        .fetch_one(&server.pool)
        .await
        .unwrap();
    assert_eq!(mode, "none");
}
