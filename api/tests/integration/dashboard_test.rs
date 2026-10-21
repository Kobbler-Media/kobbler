use crate::common::*;
use reqwest::StatusCode;
use serde_json::{Value, json};
use time::macros::date;
use uuid::Uuid;

// =============================================================================
// Helpers
// =============================================================================

async fn create_movie(server: &TestServer, title: &str) -> Uuid {
    let resp = server
        .admin_client()
        .post_json(
            "/api/v1/admin/movies",
            &json!({ "title": title, "year": 2024 }),
        )
        .await;
    resp.assert_status(StatusCode::CREATED);
    let json: Value = resp.json();
    json["id"].as_str().unwrap().parse().unwrap()
}

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
    create_episode_with_air_date(pool, season_id, episode_num, None).await
}

async fn create_episode_with_air_date(
    pool: &sqlx::PgPool,
    season_id: Uuid,
    episode_num: i32,
    air_date: Option<time::Date>,
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

async fn set_position(server: &TestServer, episode_id: Uuid, position: i32) {
    server
        .admin_client()
        .put_json(
            &format!("/api/v1/admin/tv/episodes/{episode_id}/watch-state"),
            &json!({ "watched": false, "position_seconds": position }),
        )
        .await
        .assert_ok();
}

async fn get_dashboard(server: &TestServer) -> Value {
    let resp = server.admin_client().get("/api/v1/admin/dashboard").await;
    resp.assert_ok();
    resp.json()
}

async fn get_discover(server: &TestServer) -> Value {
    let resp = server.admin_client().get("/api/v1/admin/discover").await;
    resp.assert_ok();
    resp.json()
}

// =============================================================================
// Tests
// =============================================================================

#[tokio::test]
async fn test_dashboard_empty() {
    let server = TestServer::start_api_only().await;
    let json = get_dashboard(&server).await;

    assert!(json["read_next"].as_array().unwrap().is_empty());
    // Old field should not exist
    assert!(json.get("in_progress").is_none());
    // watch_next moved to discover endpoint
    assert!(json.get("watch_next").is_none());

    let discover = get_discover(&server).await;
    assert!(discover["watch_next"].as_array().unwrap().is_empty());
}

#[tokio::test]
async fn test_watch_next_up_next_episode() {
    let server = TestServer::start_api_only().await;
    let pool = &server.pool;

    // Create show with 3 episodes, all in library
    let show_id = create_show(&server, "Breaking Bad").await;
    let season_id = create_season(pool, show_id, 1).await;
    let ep1 = create_episode(pool, season_id, 1).await;
    let ep2 = create_episode(pool, season_id, 2).await;
    let _ep3 = create_episode(pool, season_id, 3).await;

    create_stub_episode_file(pool, ep1).await;
    create_stub_episode_file(pool, ep2).await;
    create_stub_episode_file(pool, _ep3).await;

    // Watch episode 1
    mark_watched(&server, ep1).await;

    let json = get_discover(&server).await;
    let watch_next = json["watch_next"].as_array().unwrap();

    // Episode 2 should be up next
    assert_eq!(watch_next.len(), 1);
    assert_eq!(watch_next[0]["media_id"], ep2.to_string());
    assert_eq!(watch_next[0]["media_type"], "episode");
    // Up-next items have no progress
    assert!(watch_next[0]["progress_percent"].is_null());
    assert!(watch_next[0]["position_seconds"].is_null());
}

#[tokio::test]
async fn test_watch_next_fully_watched_show_excluded() {
    let server = TestServer::start_api_only().await;
    let pool = &server.pool;

    let show_id = create_show(&server, "Miniseries").await;
    let season_id = create_season(pool, show_id, 1).await;
    let ep1 = create_episode(pool, season_id, 1).await;
    let ep2 = create_episode(pool, season_id, 2).await;

    create_stub_episode_file(pool, ep1).await;
    create_stub_episode_file(pool, ep2).await;

    // Watch both episodes
    mark_watched(&server, ep1).await;
    mark_watched(&server, ep2).await;

    let json = get_discover(&server).await;
    let watch_next = json["watch_next"].as_array().unwrap();
    assert!(watch_next.is_empty());
}

#[tokio::test]
async fn test_watch_next_in_progress_movie() {
    let server = TestServer::start_api_only().await;
    let pool = &server.pool;

    let movie_id = create_movie(&server, "Inception").await;
    create_stub_movie_file(pool, movie_id).await;

    // Set position at 120 seconds (>= 60 threshold)
    server
        .admin_client()
        .put_json(
            &format!("/api/v1/admin/movies/{movie_id}/watch-state"),
            &json!({ "watched": false, "position_seconds": 120 }),
        )
        .await
        .assert_ok();

    let json = get_discover(&server).await;
    let watch_next = json["watch_next"].as_array().unwrap();
    assert_eq!(watch_next.len(), 1);
    assert_eq!(watch_next[0]["media_type"], "movie");
    assert_eq!(watch_next[0]["media_id"], movie_id.to_string());
    assert_eq!(watch_next[0]["position_seconds"], 120);
}

#[tokio::test]
async fn test_watch_next_in_progress_episode_not_duplicated_with_up_next() {
    let server = TestServer::start_api_only().await;
    let pool = &server.pool;

    let show_id = create_show(&server, "The Wire").await;
    let season_id = create_season(pool, show_id, 1).await;
    let ep1 = create_episode(pool, season_id, 1).await;
    let ep2 = create_episode(pool, season_id, 2).await;
    let _ep3 = create_episode(pool, season_id, 3).await;

    create_stub_episode_file(pool, ep1).await;
    create_stub_episode_file(pool, ep2).await;
    create_stub_episode_file(pool, _ep3).await;

    // Watch ep1, start ep2 (in-progress at 120s)
    mark_watched(&server, ep1).await;
    set_position(&server, ep2, 120).await;

    let json = get_discover(&server).await;
    let watch_next = json["watch_next"].as_array().unwrap();

    // Deduped: one entry per show, showing the in-progress episode (ep2)
    assert_eq!(watch_next.len(), 1);
    assert_eq!(watch_next[0]["media_id"], ep2.to_string());
    assert_eq!(watch_next[0]["position_seconds"], 120);
}

#[tokio::test]
async fn test_watch_next_no_watch_history_excluded() {
    let server = TestServer::start_api_only().await;
    let pool = &server.pool;

    // Show exists with episodes in library but no watch history
    let show_id = create_show(&server, "Unwatched Show").await;
    let season_id = create_season(pool, show_id, 1).await;
    let ep1 = create_episode(pool, season_id, 1).await;
    create_stub_episode_file(pool, ep1).await;

    let json = get_discover(&server).await;
    let watch_next = json["watch_next"].as_array().unwrap();
    assert!(watch_next.is_empty());
}

#[tokio::test]
async fn test_watch_next_episode_not_in_library_skipped() {
    let server = TestServer::start_api_only().await;
    let pool = &server.pool;

    let show_id = create_show(&server, "Partial Library").await;
    let season_id = create_season(pool, show_id, 1).await;
    let ep1 = create_episode(pool, season_id, 1).await;
    let _ep2 = create_episode(pool, season_id, 2).await; // NOT in library
    let ep3 = create_episode(pool, season_id, 3).await;

    create_stub_episode_file(pool, ep1).await;
    // ep2 has no file
    create_stub_episode_file(pool, ep3).await;

    mark_watched(&server, ep1).await;

    let json = get_discover(&server).await;
    let watch_next = json["watch_next"].as_array().unwrap();

    // Should skip ep2 (not in library) and show ep3
    assert_eq!(watch_next.len(), 1);
    assert_eq!(watch_next[0]["media_id"], ep3.to_string());
}

#[tokio::test]
async fn test_watch_next_14_day_expiry() {
    let server = TestServer::start_api_only().await;
    let pool = &server.pool;

    let show_id = create_show(&server, "Stale Show").await;
    let season_id = create_season(pool, show_id, 1).await;
    let ep1 = create_episode(pool, season_id, 1).await;
    let ep2 = create_episode(pool, season_id, 2).await;

    create_stub_episode_file(pool, ep1).await;
    create_stub_episode_file(pool, ep2).await;

    // Watch ep1, then backdate the watch state to 15 days ago
    mark_watched(&server, ep1).await;
    sqlx::query("UPDATE tv_watch_states SET updated_at = NOW() - INTERVAL '15 days'")
        .execute(pool)
        .await
        .unwrap();

    let json = get_discover(&server).await;
    let watch_next = json["watch_next"].as_array().unwrap();

    // Show should be expired (no activity in 14 days)
    let has_show = watch_next
        .iter()
        .any(|i| i["media_type"] == "episode" && i["title"] == "Stale Show");
    assert!(
        !has_show,
        "Show with no activity in 14 days should not appear"
    );
}

#[tokio::test]
async fn test_watch_next_new_season_exception() {
    let server = TestServer::start_api_only().await;
    let pool = &server.pool;

    let show_id = create_show(&server, "Revival Show").await;
    let s1 = create_season(pool, show_id, 1).await;
    let s1e1 = create_episode(pool, s1, 1).await;
    let s1e2 = create_episode(pool, s1, 2).await;
    create_stub_episode_file(pool, s1e1).await;
    create_stub_episode_file(pool, s1e2).await;

    // Watch all of season 1
    mark_watched(&server, s1e1).await;
    mark_watched(&server, s1e2).await;

    // Backdate activity to 30 days ago (well past 14-day window)
    sqlx::query("UPDATE tv_watch_states SET updated_at = NOW() - INTERVAL '30 days'")
        .execute(pool)
        .await
        .unwrap();

    // New season appears with episodes in library
    let s2 = create_season(pool, show_id, 2).await;
    let s2e1 = create_episode(pool, s2, 1).await;
    create_stub_episode_file(pool, s2e1).await;

    let json = get_discover(&server).await;
    let watch_next = json["watch_next"].as_array().unwrap();

    // New-season exception: should appear despite being > 14 days
    assert_eq!(watch_next.len(), 1);
    assert_eq!(watch_next[0]["media_id"], s2e1.to_string());
    assert_eq!(watch_next[0]["subtitle"], "S02E01 - Episode 1");
}

#[tokio::test]
async fn test_watch_next_new_season_exception_not_triggered_mid_season() {
    let server = TestServer::start_api_only().await;
    let pool = &server.pool;

    let show_id = create_show(&server, "Abandoned Show").await;
    let s1 = create_season(pool, show_id, 1).await;
    let s1e1 = create_episode(pool, s1, 1).await;
    let s1e2 = create_episode(pool, s1, 2).await; // unwatched - user didn't finish season
    create_stub_episode_file(pool, s1e1).await;
    create_stub_episode_file(pool, s1e2).await;

    // Only watch ep1 (NOT the last episode of the season)
    mark_watched(&server, s1e1).await;

    // Backdate activity
    sqlx::query("UPDATE tv_watch_states SET updated_at = NOW() - INTERVAL '30 days'")
        .execute(pool)
        .await
        .unwrap();

    // New season appears
    let s2 = create_season(pool, show_id, 2).await;
    let s2e1 = create_episode(pool, s2, 1).await;
    create_stub_episode_file(pool, s2e1).await;

    let json = get_discover(&server).await;
    let watch_next = json["watch_next"].as_array().unwrap();

    // NOT a new-season exception: user didn't watch last ep of S01, and > 14 days
    let has_show = watch_next
        .iter()
        .any(|i| i["media_type"] == "episode" && i["title"] == "Abandoned Show");
    assert!(
        !has_show,
        "Show abandoned mid-season should not appear after 14 days"
    );
}

#[tokio::test]
async fn test_watch_next_dedup_across_seasons() {
    let server = TestServer::start_api_only().await;
    let pool = &server.pool;

    let show_id = create_show(&server, "Multi Season").await;
    let s1 = create_season(pool, show_id, 1).await;
    let s1e1 = create_episode(pool, s1, 1).await;
    create_stub_episode_file(pool, s1e1).await;

    let s3 = create_season(pool, show_id, 3).await;
    let s3e4 = create_episode(pool, s3, 4).await;
    create_stub_episode_file(pool, s3e4).await;

    // Watch s1e1
    mark_watched(&server, s1e1).await;

    let json = get_discover(&server).await;
    let watch_next = json["watch_next"].as_array().unwrap();

    // Should see exactly one entry for the show (not two)
    let show_entries: Vec<&Value> = watch_next
        .iter()
        .filter(|i| i["title"] == "Multi Season")
        .collect();
    assert_eq!(show_entries.len(), 1, "Show should appear exactly once");
}

#[tokio::test]
async fn test_watch_next_new_season_exception_expires_after_30_days() {
    let server = TestServer::start_api_only().await;
    let pool = &server.pool;

    let show_id = create_show(&server, "Stale Revival").await;
    let s1 = create_season(pool, show_id, 1).await;
    let s1e1 = create_episode(pool, s1, 1).await;
    let s1e2 = create_episode(pool, s1, 2).await;
    create_stub_episode_file(pool, s1e1).await;
    create_stub_episode_file(pool, s1e2).await;

    // Watch all of season 1
    mark_watched(&server, s1e1).await;
    mark_watched(&server, s1e2).await;

    // Backdate watch activity to 60 days ago
    sqlx::query("UPDATE tv_watch_states SET updated_at = NOW() - INTERVAL '60 days'")
        .execute(pool)
        .await
        .unwrap();

    // New season exists but was added > 30 days ago (stale Trakt import scenario)
    let s2 = create_season(pool, show_id, 2).await;
    let s2e1 = create_episode(pool, s2, 1).await;
    create_stub_episode_file(pool, s2e1).await;

    // Backdate the episode air_date and the file created_at to 45 days ago
    sqlx::query("UPDATE tv_episodes SET air_date = CURRENT_DATE - 45 WHERE id = $1")
        .bind(s2e1)
        .execute(pool)
        .await
        .unwrap();
    sqlx::query(
        "UPDATE tv_episode_files SET created_at = NOW() - INTERVAL '45 days'
         WHERE id IN (SELECT episode_file_id FROM tv_episode_file_episodes WHERE episode_id = $1)",
    )
    .bind(s2e1)
    .execute(pool)
    .await
    .unwrap();

    let json = get_discover(&server).await;
    let watch_next = json["watch_next"].as_array().unwrap();

    // New-season exception should NOT fire: content is older than 30 days
    let has_show = watch_next
        .iter()
        .any(|i| i["media_type"] == "episode" && i["title"] == "Stale Revival");
    assert!(
        !has_show,
        "Stale new season (>30 days old) should not appear"
    );
}

// =============================================================================
// Debrid watch-next tests
// =============================================================================

#[tokio::test]
async fn test_watch_next_debrid_watched_aired_episode() {
    let server = TestServer::start_api_only().await;
    let pool = &server.pool;

    // Show with 3 aired episodes, NO local files (debrid-only)
    let show_id = create_show(&server, "Debrid Show").await;
    let season_id = create_season(pool, show_id, 1).await;
    let ep1 = create_episode_with_air_date(pool, season_id, 1, Some(date!(2024 - 01 - 01))).await;
    let ep2 = create_episode_with_air_date(pool, season_id, 2, Some(date!(2024 - 01 - 08))).await;
    let _ep3 = create_episode_with_air_date(pool, season_id, 3, Some(date!(2024 - 01 - 15))).await;

    // Mark ep1 as watched (simulating debrid watch completion)
    mark_watched(&server, ep1).await;

    let json = get_discover(&server).await;
    let watch_next = json["watch_next"].as_array().unwrap();

    // Episode 2 should be up next even with no local files (it has aired)
    assert_eq!(watch_next.len(), 1);
    assert_eq!(watch_next[0]["media_id"], ep2.to_string());
    assert_eq!(watch_next[0]["media_type"], "episode");
}

async fn set_episode_want_status(pool: &sqlx::PgPool, episode_id: Uuid, status: &str) {
    sqlx::query("UPDATE tv_episodes SET want_status = $1 WHERE id = $2")
        .bind(status)
        .bind(episode_id)
        .execute(pool)
        .await
        .unwrap();
}

#[tokio::test]
async fn test_watch_next_wanted_episode_without_file_hidden() {
    // A wanted episode whose air date has passed but which has no file yet
    // must NOT appear in watch-next — kobload will surface it once the grab lands.
    let server = TestServer::start_api_only().await;
    let pool = &server.pool;

    let show_id = create_show(&server, "Wanted Show").await;
    let season_id = create_season(pool, show_id, 1).await;
    let ep1 = create_episode_with_air_date(pool, season_id, 1, Some(date!(2024 - 01 - 01))).await;
    let ep2 = create_episode_with_air_date(pool, season_id, 2, Some(date!(2024 - 01 - 08))).await;

    create_stub_episode_file(pool, ep1).await;
    // ep2: no file, but wanted
    set_episode_want_status(pool, ep2, "wanted").await;

    mark_watched(&server, ep1).await;

    let json = get_discover(&server).await;
    let watch_next = json["watch_next"].as_array().unwrap();

    let has_show = watch_next.iter().any(|i| i["title"] == "Wanted Show");
    assert!(
        !has_show,
        "Wanted episode without a file must not appear — air_date is not a reliable availability signal for wanted episodes"
    );
}

#[tokio::test]
async fn test_watch_next_debrid_recently_aired_hidden() {
    // A debrid-only episode whose air_date is today should NOT appear yet —
    // we wait ~24h for release lag.
    let server = TestServer::start_api_only().await;
    let pool = &server.pool;

    let show_id = create_show(&server, "Fresh Air Show").await;
    let season_id = create_season(pool, show_id, 1).await;
    let ep1 = create_episode_with_air_date(pool, season_id, 1, Some(date!(2024 - 01 - 01))).await;
    let ep2 = create_episode(pool, season_id, 2).await;

    // Set ep2 air_date to today (just aired, < 24h ago)
    sqlx::query("UPDATE tv_episodes SET air_date = CURRENT_DATE WHERE id = $1")
        .bind(ep2)
        .execute(pool)
        .await
        .unwrap();

    mark_watched(&server, ep1).await;

    let json = get_discover(&server).await;
    let watch_next = json["watch_next"].as_array().unwrap();

    let has_show = watch_next.iter().any(|i| i["title"] == "Fresh Air Show");
    assert!(
        !has_show,
        "Debrid episode aired within last 24h should not yet appear"
    );
}

#[tokio::test]
async fn test_watch_next_debrid_aired_yesterday_shown() {
    // A debrid-only episode whose air_date was yesterday SHOULD appear —
    // 24h fuzz has elapsed.
    let server = TestServer::start_api_only().await;
    let pool = &server.pool;

    let show_id = create_show(&server, "Yesterday Show").await;
    let season_id = create_season(pool, show_id, 1).await;
    let ep1 = create_episode_with_air_date(pool, season_id, 1, Some(date!(2024 - 01 - 01))).await;
    let ep2 = create_episode(pool, season_id, 2).await;

    sqlx::query("UPDATE tv_episodes SET air_date = CURRENT_DATE - 1 WHERE id = $1")
        .bind(ep2)
        .execute(pool)
        .await
        .unwrap();

    mark_watched(&server, ep1).await;

    let json = get_discover(&server).await;
    let watch_next = json["watch_next"].as_array().unwrap();

    assert_eq!(watch_next.len(), 1);
    assert_eq!(watch_next[0]["media_id"], ep2.to_string());
}

#[tokio::test]
async fn test_watch_next_includes_sports_show() {
    // Sports shows should appear in What to Watch alongside tv shows,
    // with the "event" media_type and /sports/events/ link.
    let server = TestServer::start_api_only().await;
    let pool = &server.pool;

    let show_id = create_show(&server, "F1 2024").await;
    sqlx::query("UPDATE tv_shows SET content_category = 'sport' WHERE id = $1")
        .bind(show_id)
        .execute(pool)
        .await
        .unwrap();

    let season_id = create_season(pool, show_id, 1).await;
    let ep1 = create_episode(pool, season_id, 1).await;
    let ep2 = create_episode(pool, season_id, 2).await;
    create_stub_episode_file(pool, ep1).await;
    create_stub_episode_file(pool, ep2).await;

    mark_watched(&server, ep1).await;

    let json = get_discover(&server).await;
    let watch_next = json["watch_next"].as_array().unwrap();

    assert_eq!(watch_next.len(), 1);
    assert_eq!(watch_next[0]["media_id"], ep2.to_string());
    assert_eq!(watch_next[0]["media_type"], "event");
    assert_eq!(watch_next[0]["link"], format!("/sports/events/{}", ep2));
}

#[tokio::test]
async fn test_watch_next_debrid_unaired_excluded() {
    let server = TestServer::start_api_only().await;
    let pool = &server.pool;

    // Show with ep1 aired, ep2 unaired, NO local files
    let show_id = create_show(&server, "Unaired Show").await;
    let season_id = create_season(pool, show_id, 1).await;
    let ep1 = create_episode_with_air_date(pool, season_id, 1, Some(date!(2024 - 01 - 01))).await;
    let _ep2 = create_episode_with_air_date(pool, season_id, 2, Some(date!(2099 - 12 - 31))).await;

    mark_watched(&server, ep1).await;

    let json = get_discover(&server).await;
    let watch_next = json["watch_next"].as_array().unwrap();

    // ep2 hasn't aired and has no file — should not appear
    let has_show = watch_next.iter().any(|i| i["title"] == "Unaired Show");
    assert!(
        !has_show,
        "Unaired episode without file should not appear in watch-next"
    );
}

// =============================================================================
// Watchlist ("want to watch")
// =============================================================================

async fn add_watchlist(server: &TestServer, media_type: &str, media_id: Uuid) {
    server
        .admin_client()
        .post_json(
            "/api/v1/admin/watchlist",
            &json!({ "media_type": media_type, "media_id": media_id }),
        )
        .await
        .assert_status(StatusCode::NO_CONTENT);
}

async fn remove_watchlist(server: &TestServer, media_type: &str, media_id: Uuid) {
    server
        .admin_client()
        .delete(&format!("/api/v1/admin/watchlist/{media_type}/{media_id}"))
        .await
        .assert_status(StatusCode::NO_CONTENT);
}

async fn watchlist_ids(server: &TestServer) -> Vec<String> {
    let resp = server.admin_client().get("/api/v1/admin/watchlist").await;
    resp.assert_ok();
    let json: Value = resp.json();
    json.as_array()
        .unwrap()
        .iter()
        .map(|e| e["media_id"].as_str().unwrap().to_string())
        .collect()
}

/// The gap this feature exists to close: a show with no watch history could
/// never reach Up Next, because the up-next branch keys off `last_watched`.
#[tokio::test]
async fn test_watchlist_unstarted_show_appears_in_watch_next() {
    let server = TestServer::start_api_only().await;
    let pool = &server.pool;

    let show_id = create_show(&server, "Never Started").await;
    let season_id = create_season(pool, show_id, 1).await;
    let ep1 = create_episode(pool, season_id, 1).await;
    let _ep2 = create_episode(pool, season_id, 2).await;
    create_stub_episode_file(pool, ep1).await;

    // Without the watchlist entry there is nothing to show.
    let json = get_discover(&server).await;
    assert!(json["watch_next"].as_array().unwrap().is_empty());

    add_watchlist(&server, "show", show_id).await;

    let json = get_discover(&server).await;
    let watch_next = json["watch_next"].as_array().unwrap();
    assert_eq!(watch_next.len(), 1);
    // The card points at the episode she would start on.
    assert_eq!(watch_next[0]["media_id"], ep1.to_string());
    assert_eq!(watch_next[0]["media_type"], "episode");
    assert_eq!(watch_next[0]["title"], "Never Started");
    assert!(watch_next[0]["progress_percent"].is_null());
}

/// A watchlisted show that has been started must not produce two cards: the
/// up-next branch already covers it.
#[tokio::test]
async fn test_watchlist_started_show_not_duplicated() {
    let server = TestServer::start_api_only().await;
    let pool = &server.pool;

    let show_id = create_show(&server, "Already Going").await;
    let season_id = create_season(pool, show_id, 1).await;
    let ep1 = create_episode(pool, season_id, 1).await;
    let ep2 = create_episode(pool, season_id, 2).await;
    create_stub_episode_file(pool, ep1).await;
    create_stub_episode_file(pool, ep2).await;

    add_watchlist(&server, "show", show_id).await;
    mark_watched(&server, ep1).await;

    let json = get_discover(&server).await;
    let watch_next = json["watch_next"].as_array().unwrap();
    assert_eq!(watch_next.len(), 1);
    assert_eq!(watch_next[0]["media_id"], ep2.to_string());
}

/// Watching something satisfies the intent — the DB trigger drops the row, so
/// the stored list matches what the UI button claims.
#[tokio::test]
async fn test_watchlist_dropped_once_started() {
    let server = TestServer::start_api_only().await;
    let pool = &server.pool;

    let show_id = create_show(&server, "Drop Me").await;
    let season_id = create_season(pool, show_id, 1).await;
    let ep1 = create_episode(pool, season_id, 1).await;
    create_stub_episode_file(pool, ep1).await;

    add_watchlist(&server, "show", show_id).await;
    assert_eq!(watchlist_ids(&server).await, vec![show_id.to_string()]);

    mark_watched(&server, ep1).await;
    assert!(watchlist_ids(&server).await.is_empty());

    let in_watchlist = server
        .admin_client()
        .get(&format!("/api/v1/admin/tv/shows/{show_id}"))
        .await
        .json::<Value>()["in_watchlist"]
        .as_bool()
        .unwrap();
    assert!(!in_watchlist);
}

/// The same trigger fires on a position update crossing the 60s mark, not just
/// on an explicit "mark watched".
#[tokio::test]
async fn test_watchlist_dropped_on_position_past_threshold() {
    let server = TestServer::start_api_only().await;
    let pool = &server.pool;

    let show_id = create_show(&server, "Partially Started").await;
    let season_id = create_season(pool, show_id, 1).await;
    let ep1 = create_episode(pool, season_id, 1).await;
    create_stub_episode_file(pool, ep1).await;

    add_watchlist(&server, "show", show_id).await;

    // Below the threshold the intent still stands.
    set_position(&server, ep1, 30).await;
    assert_eq!(watchlist_ids(&server).await, vec![show_id.to_string()]);

    set_position(&server, ep1, 120).await;
    assert!(watchlist_ids(&server).await.is_empty());
}

#[tokio::test]
async fn test_watchlist_movie_round_trip() {
    let server = TestServer::start_api_only().await;

    let movie_id = create_movie(&server, "On The List").await;
    add_watchlist(&server, "movie", movie_id).await;

    let json = get_discover(&server).await;
    let watch_next = json["watch_next"].as_array().unwrap();
    assert_eq!(watch_next.len(), 1);
    assert_eq!(watch_next[0]["media_id"], movie_id.to_string());
    assert_eq!(watch_next[0]["media_type"], "movie");

    let in_watchlist = server
        .admin_client()
        .get(&format!("/api/v1/admin/movies/{movie_id}"))
        .await
        .json::<Value>()["in_watchlist"]
        .as_bool()
        .unwrap();
    assert!(in_watchlist);

    remove_watchlist(&server, "movie", movie_id).await;
    assert!(watchlist_ids(&server).await.is_empty());
    let json = get_discover(&server).await;
    assert!(json["watch_next"].as_array().unwrap().is_empty());
}

#[tokio::test]
async fn test_watchlist_is_per_user() {
    let server = TestServer::start_api_only().await;

    let movie_id = create_movie(&server, "Mine Only").await;
    add_watchlist(&server, "movie", movie_id).await;

    // A second user sees their own (empty) list, not the admin's.
    let other = create_user_client(&server, "watchlist-viewer@test.local", "viewer").await;
    let resp = other.get("/api/v1/admin/watchlist").await;
    resp.assert_ok();
    assert!(resp.json::<Value>().as_array().unwrap().is_empty());
}

#[tokio::test]
async fn test_watchlist_rejects_unknown_media() {
    let server = TestServer::start_api_only().await;

    server
        .admin_client()
        .post_json(
            "/api/v1/admin/watchlist",
            &json!({ "media_type": "show", "media_id": Uuid::now_v7() }),
        )
        .await
        .assert_status(StatusCode::NOT_FOUND);

    let movie_id = create_movie(&server, "Real Movie").await;
    server
        .admin_client()
        .post_json(
            "/api/v1/admin/watchlist",
            &json!({ "media_type": "banana", "media_id": movie_id }),
        )
        .await
        .assert_status(StatusCode::BAD_REQUEST);
}

/// Nothing cascades a polymorphic row, so the delete paths clear it by hand.
#[tokio::test]
async fn test_watchlist_cleared_when_media_deleted() {
    let server = TestServer::start_api_only().await;

    let movie_id = create_movie(&server, "Doomed").await;
    add_watchlist(&server, "movie", movie_id).await;

    server
        .admin_client()
        .delete(&format!("/api/v1/admin/movies/{movie_id}"))
        .await
        .assert_status(StatusCode::NO_CONTENT);

    let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM watchlist_items WHERE media_id = $1")
        .bind(movie_id)
        .fetch_one(&server.pool)
        .await
        .unwrap();
    assert_eq!(count, 0);
}

// =============================================================================
// Recently added books / audiobooks
// =============================================================================

async fn backdate_book(server: &TestServer, book_id: Uuid, days: i32) {
    sqlx::query("UPDATE books SET created_at = NOW() - make_interval(days => $2) WHERE id = $1")
        .bind(book_id)
        .bind(days)
        .execute(&server.pool)
        .await
        .unwrap();
}

async fn backdate_audiobook_file(server: &TestServer, book_id: Uuid, days: i32) {
    sqlx::query(
        "UPDATE audiobook_files SET added_at = NOW() - make_interval(days => $2) WHERE book_id = $1",
    )
    .bind(book_id)
    .bind(days)
    .execute(&server.pool)
    .await
    .unwrap();
}

fn ids(items: &Value) -> Vec<String> {
    items
        .as_array()
        .unwrap()
        .iter()
        .map(|b| b["id"].as_str().unwrap().to_string())
        .collect()
}

#[tokio::test]
async fn test_recent_audiobooks_excludes_ebook_only_books() {
    let server = TestServer::start_api_only().await;

    let ebook = create_book(&server, "Ebook Only").await;
    add_ebook_file(&server, ebook).await;
    let audiobook = create_book(&server, "Audiobook").await;
    add_audiobook_file(&server, audiobook).await;

    let json = get_dashboard(&server).await;

    let recent_books = ids(&json["recent_books"]);
    assert!(recent_books.contains(&ebook.to_string()));
    assert!(recent_books.contains(&audiobook.to_string()));

    let recent_audiobooks = ids(&json["recent_audiobooks"]);
    assert_eq!(recent_audiobooks, vec![audiobook.to_string()]);
}

#[tokio::test]
async fn test_recent_audiobooks_ordered_by_audio_added_at() {
    let server = TestServer::start_api_only().await;

    // Long-shelved ebook that just gained an audiobook: recent as an audiobook,
    // stale as a book.
    let late_audio = create_book(&server, "Late Audio").await;
    add_ebook_file(&server, late_audio).await;
    add_audiobook_file(&server, late_audio).await;
    backdate_book(&server, late_audio, 730).await;

    let older_audio = create_book(&server, "Older Audio").await;
    add_audiobook_file(&server, older_audio).await;
    backdate_audiobook_file(&server, older_audio, 1).await;

    let json = get_dashboard(&server).await;

    assert_eq!(
        ids(&json["recent_audiobooks"]),
        vec![late_audio.to_string(), older_audio.to_string()]
    );
    assert_eq!(
        ids(&json["recent_books"]),
        vec![older_audio.to_string(), late_audio.to_string()]
    );
}
