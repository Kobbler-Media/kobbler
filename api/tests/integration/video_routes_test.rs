use crate::common::*;
use reqwest::StatusCode;
use serde_json::{Value, json};

// =============================================================================
// Test Helpers
// =============================================================================

/// Create a movie via the admin API. Returns the movie ID.
async fn create_movie(server: &TestServer, title: &str) -> String {
    let resp = server
        .admin_client()
        .post_json(
            "/api/v1/admin/movies",
            &json!({
                "title": title,
                "year": 2024
            }),
        )
        .await;
    resp.assert_status(StatusCode::CREATED);
    let json: Value = resp.json();
    json["id"].as_str().unwrap().to_string()
}

/// Create a TV show via the admin API. Returns the show ID.
async fn create_show(server: &TestServer, title: &str) -> String {
    let resp = server
        .admin_client()
        .post_json(
            "/api/v1/admin/tv/shows",
            &json!({
                "title": title,
                "year": 2024
            }),
        )
        .await;
    resp.assert_status(StatusCode::CREATED);
    let json: Value = resp.json();
    json["id"].as_str().unwrap().to_string()
}

/// Create a season directly in the DB. Returns the season ID.
async fn create_season(server: &TestServer, show_id: &str, season_num: i32) -> String {
    let id = uuid::Uuid::now_v7();
    sqlx::query(
        r#"
        INSERT INTO tv_seasons (id, show_id, season_identifier, season_sort_order, is_monitored, is_special)
        VALUES ($1, $2::uuid, $3, $4, true, false)
        "#,
    )
    .bind(id)
    .bind(uuid::Uuid::parse_str(show_id).unwrap())
    .bind(season_num.to_string())
    .bind(season_num)
    .execute(&server.pool)
    .await
    .expect("Failed to create season");
    id.to_string()
}

/// Create an episode directly in the DB. Returns the episode ID.
async fn create_episode(
    server: &TestServer,
    season_id: &str,
    episode_num: i32,
    title: &str,
) -> String {
    let id = uuid::Uuid::now_v7();
    sqlx::query(
        r#"
        INSERT INTO tv_episodes (id, season_id, episode_identifier, episode_sort_order, title)
        VALUES ($1, $2::uuid, $3, $4, $5)
        "#,
    )
    .bind(id)
    .bind(uuid::Uuid::parse_str(season_id).unwrap())
    .bind(episode_num.to_string())
    .bind(episode_num)
    .bind(title)
    .execute(&server.pool)
    .await
    .expect("Failed to create episode");
    id.to_string()
}

// =============================================================================
// Movie CRUD Tests
// =============================================================================

#[tokio::test]
async fn test_list_movies_empty() {
    let server = TestServer::start_api_only().await;
    let resp = server.admin_client().get("/api/v1/admin/movies").await;
    resp.assert_ok();
    let json: Value = resp.json();
    assert!(json["items"].as_array().unwrap().is_empty());
    assert!(json["next_cursor"].is_null());
    assert!(json["prev_cursor"].is_null());
}

#[tokio::test]
async fn test_create_and_get_movie() {
    let server = TestServer::start_api_only().await;
    let id = create_movie(&server, "The Matrix").await;

    let resp = server
        .admin_client()
        .get(&format!("/api/v1/admin/movies/{}", id))
        .await;
    resp.assert_ok();
    let json: Value = resp.json();
    assert_eq!(json["title"], "The Matrix");
    assert_eq!(json["year"], 2024);
    assert_eq!(json["want_status"], "none");
}

#[tokio::test]
async fn test_list_movies_with_filters() {
    let server = TestServer::start_api_only().await;
    create_movie(&server, "Movie A").await;
    let id_b = create_movie(&server, "Movie B").await;

    // Want movie B
    server
        .admin_client()
        .post(&format!("/api/v1/admin/movies/{}/want", id_b))
        .await
        .assert_status(StatusCode::NO_CONTENT);

    // Filter by want_status=wanted
    let resp = server
        .admin_client()
        .get("/api/v1/admin/movies?want_status=wanted")
        .await;
    resp.assert_ok();
    let json: Value = resp.json();
    let items = json["items"].as_array().unwrap();
    assert_eq!(items.len(), 1);
    assert_eq!(items[0]["title"], "Movie B");
}

#[tokio::test]
async fn test_update_movie() {
    let server = TestServer::start_api_only().await;
    let id = create_movie(&server, "Old Title").await;

    server
        .admin_client()
        .patch_json(
            &format!("/api/v1/admin/movies/{}", id),
            &json!({ "title": "New Title", "year": 2025 }),
        )
        .await
        .assert_status(StatusCode::NO_CONTENT);

    let resp = server
        .admin_client()
        .get(&format!("/api/v1/admin/movies/{}", id))
        .await;
    resp.assert_ok();
    let json: Value = resp.json();
    assert_eq!(json["title"], "New Title");
    assert_eq!(json["year"], 2025);
}

#[tokio::test]
async fn test_delete_movie() {
    let server = TestServer::start_api_only().await;
    let id = create_movie(&server, "Doomed Movie").await;

    server
        .admin_client()
        .delete(&format!("/api/v1/admin/movies/{}", id))
        .await
        .assert_status(StatusCode::NO_CONTENT);

    server
        .admin_client()
        .get(&format!("/api/v1/admin/movies/{}", id))
        .await
        .assert_status(StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn test_movie_not_found() {
    let server = TestServer::start_api_only().await;
    let fake_id = uuid::Uuid::new_v4();
    server
        .admin_client()
        .get(&format!("/api/v1/admin/movies/{}", fake_id))
        .await
        .assert_status(StatusCode::NOT_FOUND);
}

// =============================================================================
// Movie Want / Unwant
// =============================================================================

#[tokio::test]
async fn test_want_and_unwant_movie() {
    let server = TestServer::start_api_only().await;
    let id = create_movie(&server, "Wanted Movie").await;

    // Want
    server
        .admin_client()
        .post(&format!("/api/v1/admin/movies/{}/want", id))
        .await
        .assert_status(StatusCode::NO_CONTENT);

    let resp = server
        .admin_client()
        .get(&format!("/api/v1/admin/movies/{}", id))
        .await;
    resp.assert_ok();
    assert_eq!(resp.json::<Value>()["want_status"], "wanted");

    // Unwant
    server
        .admin_client()
        .delete(&format!("/api/v1/admin/movies/{}/want", id))
        .await
        .assert_status(StatusCode::NO_CONTENT);

    let resp = server
        .admin_client()
        .get(&format!("/api/v1/admin/movies/{}", id))
        .await;
    resp.assert_ok();
    assert_eq!(resp.json::<Value>()["want_status"], "none");
}

// =============================================================================
// Movie Watch State
// =============================================================================

#[tokio::test]
async fn test_movie_watch_state_crud() {
    let server = TestServer::start_api_only().await;
    let id = create_movie(&server, "Watch Me").await;

    // Get initial state (null/empty)
    let resp = server
        .admin_client()
        .get(&format!("/api/v1/admin/movies/{}/watch-state", id))
        .await;
    resp.assert_ok();

    // Set watched
    server
        .admin_client()
        .put_json(
            &format!("/api/v1/admin/movies/{}/watch-state", id),
            &json!({
                "watched": true,
                "watched_at": "2024-12-01T00:00:00Z"
            }),
        )
        .await
        .assert_ok();

    // Verify
    let resp = server
        .admin_client()
        .get(&format!("/api/v1/admin/movies/{}/watch-state", id))
        .await;
    resp.assert_ok();
    let json: Value = resp.json();
    assert_eq!(json["watched"], true);

    // Toggle unwatched
    server
        .admin_client()
        .put_json(
            &format!("/api/v1/admin/movies/{}/watch-state", id),
            &json!({ "watched": false }),
        )
        .await
        .assert_ok();

    let resp = server
        .admin_client()
        .get(&format!("/api/v1/admin/movies/{}/watch-state", id))
        .await;
    resp.assert_ok();
    assert_eq!(resp.json::<Value>()["watched"], false);
}

// =============================================================================
// Movie Files
// =============================================================================

#[tokio::test]
async fn test_movie_file_crud() {
    let server = TestServer::start_api_only().await;
    let id = create_movie(&server, "File Movie").await;

    // Add file
    let resp = server
        .admin_client()
        .post_json(
            &format!("/api/v1/admin/movies/{}/files", id),
            &json!({
                "file_path": "/media/movies/File Movie (2024)/File.Movie.2024.mkv",
                "format": "MKV",
                "file_size": 4294967296_i64,
            }),
        )
        .await;
    resp.assert_status(StatusCode::CREATED);
    let file_json: Value = resp.json();
    let file_id = file_json["id"].as_str().unwrap();

    // List files
    let resp = server
        .admin_client()
        .get(&format!("/api/v1/admin/movies/{}/files", id))
        .await;
    resp.assert_ok();
    let files: Value = resp.json();
    assert_eq!(files.as_array().unwrap().len(), 1);

    // Delete file
    server
        .admin_client()
        .delete(&format!("/api/v1/admin/movies/{}/files/{}", id, file_id))
        .await
        .assert_status(StatusCode::NO_CONTENT);

    let resp = server
        .admin_client()
        .get(&format!("/api/v1/admin/movies/{}/files", id))
        .await;
    resp.assert_ok();
    assert!(resp.json::<Value>().as_array().unwrap().is_empty());
}

// =============================================================================
// TV Show CRUD Tests
// =============================================================================

#[tokio::test]
async fn test_list_shows_empty() {
    let server = TestServer::start_api_only().await;
    let resp = server.admin_client().get("/api/v1/admin/tv/shows").await;
    resp.assert_ok();
    let json: Value = resp.json();
    assert!(json["items"].as_array().unwrap().is_empty());
    assert!(json["next_cursor"].is_null());
    assert!(json["prev_cursor"].is_null());
}

#[tokio::test]
async fn test_create_and_get_show() {
    let server = TestServer::start_api_only().await;
    let id = create_show(&server, "Breaking Bad").await;

    let resp = server
        .admin_client()
        .get(&format!("/api/v1/admin/tv/shows/{}", id))
        .await;
    resp.assert_ok();
    let json: Value = resp.json();
    assert_eq!(json["title"], "Breaking Bad");
    assert_eq!(json["is_monitored"], false);
    assert!(json["seasons"].as_array().unwrap().is_empty());
}

#[tokio::test]
async fn test_create_show_with_seasons_and_episodes() {
    let server = TestServer::start_api_only().await;
    let show_id = create_show(&server, "The Wire").await;
    let season_id = create_season(&server, &show_id, 1).await;
    create_episode(&server, &season_id, 1, "The Target").await;
    create_episode(&server, &season_id, 2, "The Detail").await;

    let resp = server
        .admin_client()
        .get(&format!("/api/v1/admin/tv/shows/{}", show_id))
        .await;
    resp.assert_ok();
    let json: Value = resp.json();
    let seasons = json["seasons"].as_array().unwrap();
    assert_eq!(seasons.len(), 1);
    let episodes = seasons[0]["episodes"].as_array().unwrap();
    assert_eq!(episodes.len(), 2);
    assert_eq!(episodes[0]["title"], "The Target");
}

#[tokio::test]
async fn test_update_show() {
    let server = TestServer::start_api_only().await;
    let id = create_show(&server, "Old Show").await;

    server
        .admin_client()
        .patch_json(
            &format!("/api/v1/admin/tv/shows/{}", id),
            &json!({ "title": "New Show", "status": "ended" }),
        )
        .await
        .assert_status(StatusCode::NO_CONTENT);

    let resp = server
        .admin_client()
        .get(&format!("/api/v1/admin/tv/shows/{}", id))
        .await;
    resp.assert_ok();
    let json: Value = resp.json();
    assert_eq!(json["title"], "New Show");
    assert_eq!(json["status"], "ended");
}

#[tokio::test]
async fn test_delete_show() {
    let server = TestServer::start_api_only().await;
    let id = create_show(&server, "Cancelled Show").await;

    server
        .admin_client()
        .delete(&format!("/api/v1/admin/tv/shows/{}", id))
        .await
        .assert_status(StatusCode::NO_CONTENT);

    server
        .admin_client()
        .get(&format!("/api/v1/admin/tv/shows/{}", id))
        .await
        .assert_status(StatusCode::NOT_FOUND);
}

// =============================================================================
// TV Monitor / Unmonitor
// =============================================================================

#[tokio::test]
async fn test_monitor_and_unmonitor_show() {
    let server = TestServer::start_api_only().await;
    let id = create_show(&server, "Monitor Me").await;

    // Monitor with auto_want
    server
        .admin_client()
        .post_json(
            &format!("/api/v1/admin/tv/shows/{}/monitor", id),
            &json!({ "is_monitored": true, "auto_want_mode": "new_only" }),
        )
        .await
        .assert_status(StatusCode::NO_CONTENT);

    let resp = server
        .admin_client()
        .get(&format!("/api/v1/admin/tv/shows/{}", id))
        .await;
    resp.assert_ok();
    let json: Value = resp.json();
    assert_eq!(json["is_monitored"], true);
    assert_eq!(json["auto_want_mode"], "new_only");

    // Unmonitor
    server
        .admin_client()
        .post_json(
            &format!("/api/v1/admin/tv/shows/{}/monitor", id),
            &json!({ "is_monitored": false }),
        )
        .await
        .assert_status(StatusCode::NO_CONTENT);

    let resp = server
        .admin_client()
        .get(&format!("/api/v1/admin/tv/shows/{}", id))
        .await;
    resp.assert_ok();
    assert_eq!(resp.json::<Value>()["is_monitored"], false);
}

// =============================================================================
// TV Want / Unwant Episodes
// =============================================================================

#[tokio::test]
async fn test_want_and_unwant_episode() {
    let server = TestServer::start_api_only().await;
    let show_id = create_show(&server, "Want Show").await;
    let season_id = create_season(&server, &show_id, 1).await;
    let ep_id = create_episode(&server, &season_id, 1, "Pilot").await;

    // Want
    server
        .admin_client()
        .post(&format!("/api/v1/admin/tv/episodes/{}/want", ep_id))
        .await
        .assert_status(StatusCode::NO_CONTENT);

    // Verify
    let resp = server
        .admin_client()
        .get(&format!("/api/v1/admin/tv/episodes/{}", ep_id))
        .await;
    resp.assert_ok();
    assert_eq!(resp.json::<Value>()["want_status"], "wanted");

    // Unwant
    server
        .admin_client()
        .delete(&format!("/api/v1/admin/tv/episodes/{}/want", ep_id))
        .await
        .assert_status(StatusCode::NO_CONTENT);

    let resp = server
        .admin_client()
        .get(&format!("/api/v1/admin/tv/episodes/{}", ep_id))
        .await;
    resp.assert_ok();
    assert_eq!(resp.json::<Value>()["want_status"], "none");
}

/// "Want Missing" on a season must want every missing episode, whatever the
/// season's auto-want mode is — auto-want modes gate the scheduler, not an
/// explicit user action.
#[tokio::test]
async fn test_want_season_wants_missing_episodes() {
    for auto_want_mode in ["inherit", "none", "new_only", "fill_gaps", "all"] {
        let server = TestServer::start_api_only().await;
        let show_id = create_show(&server, "Season Want Show").await;
        let season_id = create_season(&server, &show_id, 1).await;
        create_episode(&server, &season_id, 1, "Ep 1").await;
        create_episode(&server, &season_id, 2, "Ep 2").await;

        sqlx::query("UPDATE tv_seasons SET auto_want_mode = $1 WHERE id = $2::uuid")
            .bind(auto_want_mode)
            .bind(uuid::Uuid::parse_str(&season_id).unwrap())
            .execute(&server.pool)
            .await
            .expect("set season auto_want_mode");
        // Aired long ago: outside the new_only window, and with no episodes in
        // library there are no gaps to fill either.
        sqlx::query(
            "UPDATE tv_episodes SET air_date = DATE '2020-01-01' WHERE season_id = $1::uuid",
        )
        .bind(uuid::Uuid::parse_str(&season_id).unwrap())
        .execute(&server.pool)
        .await
        .expect("backdate episodes");

        server
            .admin_client()
            .post(&format!("/api/v1/admin/tv/seasons/{}/want", season_id))
            .await
            .assert_status(StatusCode::NO_CONTENT);

        let resp = server
            .admin_client()
            .get(&format!("/api/v1/admin/tv/shows/{}", show_id))
            .await;
        resp.assert_ok();
        let json: Value = resp.json();
        let episodes = &json["seasons"][0]["episodes"];
        assert_eq!(
            episodes[0]["want_status"], "wanted",
            "auto_want_mode={auto_want_mode}"
        );
        assert_eq!(
            episodes[1]["want_status"], "wanted",
            "auto_want_mode={auto_want_mode}"
        );
    }
}

/// "Unwant All" on a season must clear every outstanding want and pin the
/// season's auto-want off, otherwise tv_monitor puts the wants straight back.
#[tokio::test]
async fn test_unwant_season_clears_wants_and_disables_refilling_auto_want() {
    let server = TestServer::start_api_only().await;
    let show_id = create_show(&server, "Season Unwant Show").await;
    let season_id = create_season(&server, &show_id, 1).await;
    create_episode(&server, &season_id, 1, "Ep 1").await;
    create_episode(&server, &season_id, 2, "Ep 2").await;

    sqlx::query("UPDATE tv_seasons SET auto_want_mode = 'all' WHERE id = $1::uuid")
        .bind(uuid::Uuid::parse_str(&season_id).unwrap())
        .execute(&server.pool)
        .await
        .expect("set season auto_want_mode");

    server
        .admin_client()
        .post(&format!("/api/v1/admin/tv/seasons/{}/want", season_id))
        .await
        .assert_status(StatusCode::NO_CONTENT);

    let resp = server
        .admin_client()
        .delete(&format!("/api/v1/admin/tv/seasons/{}/want", season_id))
        .await;
    resp.assert_ok();
    let json: Value = resp.json();
    assert_eq!(json["unwanted_count"], 2);
    assert_eq!(json["auto_want_disabled"], true);

    let resp = server
        .admin_client()
        .get(&format!("/api/v1/admin/tv/seasons/{}", season_id))
        .await;
    resp.assert_ok();
    let json: Value = resp.json();
    assert_eq!(json["auto_want_mode"], "none");
    assert_eq!(json["episodes"][0]["want_status"], "none");
    assert_eq!(json["episodes"][1]["want_status"], "none");
}

/// A season unwant must not disturb the show's other seasons.
#[tokio::test]
async fn test_unwant_season_leaves_other_seasons_wanted() {
    let server = TestServer::start_api_only().await;
    let show_id = create_show(&server, "Two Season Show").await;
    let season1 = create_season(&server, &show_id, 1).await;
    let season2 = create_season(&server, &show_id, 2).await;
    create_episode(&server, &season1, 1, "S1E1").await;
    create_episode(&server, &season2, 1, "S2E1").await;

    server
        .admin_client()
        .post(&format!(
            "/api/v1/admin/tv/shows/{}/want-all-missing",
            show_id
        ))
        .await
        .assert_status(StatusCode::NO_CONTENT);

    server
        .admin_client()
        .delete(&format!("/api/v1/admin/tv/seasons/{}/want", season1))
        .await
        .assert_ok();

    let resp = server
        .admin_client()
        .get(&format!("/api/v1/admin/tv/seasons/{}", season2))
        .await;
    resp.assert_ok();
    assert_eq!(resp.json::<Value>()["episodes"][0]["want_status"], "wanted");
}

/// "Unwant All" on a show clears every season, but leaves episodes that already
/// have a file alone — their want_status records how they were acquired.
#[tokio::test]
async fn test_unwant_all_clears_show_but_spares_library_episodes() {
    let server = TestServer::start_api_only().await;
    let show_id = create_show(&server, "Show Unwant Show").await;
    let season1 = create_season(&server, &show_id, 1).await;
    let season2 = create_season(&server, &show_id, 2).await;
    let wanted_ep = create_episode(&server, &season1, 1, "Wanted").await;
    let library_ep = create_episode(&server, &season2, 1, "Already Here").await;

    server
        .admin_client()
        .post_json(
            &format!("/api/v1/admin/tv/episodes/{}/files", library_ep),
            &json!({
                "file_path": "/media/tv/Show Unwant Show/S02E01.mkv",
                "format": "MKV",
            }),
        )
        .await
        .assert_status(StatusCode::CREATED);
    sqlx::query("UPDATE tv_episodes SET want_status = 'imported' WHERE id = $1::uuid")
        .bind(uuid::Uuid::parse_str(&library_ep).unwrap())
        .execute(&server.pool)
        .await
        .expect("mark library episode imported");

    server
        .admin_client()
        .post(&format!("/api/v1/admin/tv/episodes/{}/want", wanted_ep))
        .await
        .assert_success();

    let resp = server
        .admin_client()
        .delete(&format!(
            "/api/v1/admin/tv/shows/{}/want-all-missing",
            show_id
        ))
        .await;
    resp.assert_ok();
    assert_eq!(resp.json::<Value>()["unwanted_count"], 1);

    let resp = server
        .admin_client()
        .get(&format!("/api/v1/admin/tv/episodes/{}", wanted_ep))
        .await;
    resp.assert_ok();
    assert_eq!(resp.json::<Value>()["want_status"], "none");

    let resp = server
        .admin_client()
        .get(&format!("/api/v1/admin/tv/episodes/{}", library_ep))
        .await;
    resp.assert_ok();
    assert_eq!(resp.json::<Value>()["want_status"], "imported");
}

#[tokio::test]
async fn test_unwant_all_missing_show_returns_not_found() {
    let server = TestServer::start_api_only().await;
    server
        .admin_client()
        .delete(&format!(
            "/api/v1/admin/tv/shows/{}/want-all-missing",
            uuid::Uuid::now_v7()
        ))
        .await
        .assert_status(StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn test_unwant_requires_auth() {
    let server = TestServer::start_api_only().await;
    let show_id = create_show(&server, "Auth Unwant Show").await;
    let season_id = create_season(&server, &show_id, 1).await;

    let client = TestClient::new(&server.api_url);
    let resp = client
        .delete(&format!(
            "/api/v1/admin/tv/shows/{}/want-all-missing",
            show_id
        ))
        .await;
    assert_eq!(resp.status, StatusCode::UNAUTHORIZED);

    let resp = client
        .delete(&format!("/api/v1/admin/tv/seasons/{}/want", season_id))
        .await;
    assert_eq!(resp.status, StatusCode::UNAUTHORIZED);
}

// =============================================================================
// TV Watch State
// =============================================================================

#[tokio::test]
async fn test_episode_watch_state() {
    let server = TestServer::start_api_only().await;
    let show_id = create_show(&server, "Watch Show").await;
    let season_id = create_season(&server, &show_id, 1).await;
    let ep_id = create_episode(&server, &season_id, 1, "Watch Ep").await;

    // Set watched
    server
        .admin_client()
        .put_json(
            &format!("/api/v1/admin/tv/episodes/{}/watch-state", ep_id),
            &json!({
                "watched": true,
                "watched_at": "2024-12-01T00:00:00Z"
            }),
        )
        .await
        .assert_ok();

    // Verify
    let resp = server
        .admin_client()
        .get(&format!("/api/v1/admin/tv/episodes/{}/watch-state", ep_id))
        .await;
    resp.assert_ok();
    assert_eq!(resp.json::<Value>()["watched"], true);
}

// =============================================================================
// TV Episode Files
// =============================================================================

#[tokio::test]
async fn test_episode_file_crud() {
    let server = TestServer::start_api_only().await;
    let show_id = create_show(&server, "File Show").await;
    let season_id = create_season(&server, &show_id, 1).await;
    let ep_id = create_episode(&server, &season_id, 1, "File Ep").await;

    // Add file
    let resp = server
        .admin_client()
        .post_json(
            &format!("/api/v1/admin/tv/episodes/{}/files", ep_id),
            &json!({
                "file_path": "/media/tv/File Show/S01E01.mkv",
                "format": "MKV",
            }),
        )
        .await;
    resp.assert_status(StatusCode::CREATED);
    let file_json: Value = resp.json();
    let file_id = file_json["id"].as_str().unwrap();

    // List files
    let resp = server
        .admin_client()
        .get(&format!("/api/v1/admin/tv/episodes/{}/files", ep_id))
        .await;
    resp.assert_ok();
    assert_eq!(resp.json::<Value>().as_array().unwrap().len(), 1);

    // Delete file
    server
        .admin_client()
        .delete(&format!(
            "/api/v1/admin/tv/episodes/{}/files/{}",
            ep_id, file_id
        ))
        .await
        .assert_status(StatusCode::NO_CONTENT);
}

// =============================================================================
// Video Metadata Provider Routes
// =============================================================================

#[tokio::test]
async fn test_list_video_metadata_providers() {
    let server = TestServer::start_api_only().await;
    let resp = server
        .admin_client()
        .get("/api/v1/admin/video-metadata/providers")
        .await;
    resp.assert_ok();
    let json: Value = resp.json();
    // Should have seed providers (tmdb at minimum)
    assert!(!json.as_array().unwrap().is_empty());
}

// =============================================================================
// Auth Required Tests
// =============================================================================

#[tokio::test]
async fn test_video_endpoints_require_auth() {
    let server = TestServer::start_api_only().await;
    let no_auth = server.api_client();

    let endpoints = [
        ("GET", "/api/v1/admin/movies"),
        ("POST", "/api/v1/admin/movies"),
        ("GET", "/api/v1/admin/tv/shows"),
        ("POST", "/api/v1/admin/tv/shows"),
        ("GET", "/api/v1/admin/video-metadata/providers"),
    ];

    for (method, path) in endpoints {
        let resp = match method {
            "GET" => no_auth.get(path).await,
            "POST" => no_auth.post_json(path, &json!({})).await,
            _ => unreachable!(),
        };
        assert_eq!(
            resp.status,
            StatusCode::UNAUTHORIZED,
            "{} {} should require auth but got {}",
            method,
            path,
            resp.status
        );
    }
}

// =============================================================================
// Position-Only Update Endpoints
// =============================================================================

#[tokio::test]
async fn test_movie_position_update() {
    let server = TestServer::start_api_only().await;
    let id = create_movie(&server, "Position Movie").await;

    // Save position (creates watch state if needed)
    let resp = server
        .admin_client()
        .patch_json(
            &format!("/api/v1/admin/movies/{}/watch-state/position", id),
            &json!({ "position_seconds": 120 }),
        )
        .await;
    resp.assert_ok();
    let body: Value = resp.json();
    assert!(body["id"].is_string());

    // Verify position is saved and watched is false
    let resp = server
        .admin_client()
        .get(&format!("/api/v1/admin/movies/{}/watch-state", id))
        .await;
    resp.assert_ok();
    let ws: Value = resp.json();
    assert_eq!(ws["position_seconds"], 120);
    assert_eq!(ws["watched"], false);

    // Update position again
    server
        .admin_client()
        .patch_json(
            &format!("/api/v1/admin/movies/{}/watch-state/position", id),
            &json!({ "position_seconds": 300 }),
        )
        .await
        .assert_ok();

    let resp = server
        .admin_client()
        .get(&format!("/api/v1/admin/movies/{}/watch-state", id))
        .await;
    resp.assert_ok();
    assert_eq!(resp.json::<Value>()["position_seconds"], 300);
}

#[tokio::test]
async fn test_movie_position_preserves_watched() {
    let server = TestServer::start_api_only().await;
    let id = create_movie(&server, "Rewatch Movie").await;

    // Mark as watched first
    server
        .admin_client()
        .put_json(
            &format!("/api/v1/admin/movies/{}/watch-state", id),
            &json!({
                "watched": true,
                "watched_at": "2024-12-01T00:00:00Z"
            }),
        )
        .await
        .assert_ok();

    // Save position — should NOT clobber watched status
    server
        .admin_client()
        .patch_json(
            &format!("/api/v1/admin/movies/{}/watch-state/position", id),
            &json!({ "position_seconds": 60 }),
        )
        .await
        .assert_ok();

    let resp = server
        .admin_client()
        .get(&format!("/api/v1/admin/movies/{}/watch-state", id))
        .await;
    resp.assert_ok();
    let ws: Value = resp.json();
    assert_eq!(
        ws["watched"], true,
        "position update should not reset watched"
    );
    assert_eq!(ws["position_seconds"], 60);
}

#[tokio::test]
async fn test_episode_position_update() {
    let server = TestServer::start_api_only().await;
    let show_id = create_show(&server, "Position Show").await;
    let season_id = create_season(&server, &show_id, 1).await;
    let ep_id = create_episode(&server, &season_id, 1, "Position Ep").await;

    // Save position
    let resp = server
        .admin_client()
        .patch_json(
            &format!("/api/v1/admin/tv/episodes/{}/watch-state/position", ep_id),
            &json!({ "position_seconds": 90 }),
        )
        .await;
    resp.assert_ok();

    // Verify
    let resp = server
        .admin_client()
        .get(&format!("/api/v1/admin/tv/episodes/{}/watch-state", ep_id))
        .await;
    resp.assert_ok();
    let ws: Value = resp.json();
    assert_eq!(ws["position_seconds"], 90);
    assert_eq!(ws["watched"], false);
}

// =============================================================================
// Dashboard In-Progress
// =============================================================================

#[tokio::test]
async fn test_dashboard_in_progress_movies() {
    let server = TestServer::start_api_only().await;
    let id = create_movie(&server, "In Progress Movie").await;

    // Save position
    server
        .admin_client()
        .patch_json(
            &format!("/api/v1/admin/movies/{}/watch-state/position", id),
            &json!({ "position_seconds": 200 }),
        )
        .await
        .assert_ok();

    // Check discover (watch_next moved from dashboard to discover)
    let resp = server.admin_client().get("/api/v1/admin/discover").await;
    resp.assert_ok();
    let dash: Value = resp.json();
    let in_progress = dash["watch_next"].as_array().unwrap();
    assert!(
        in_progress.iter().any(|item| {
            item["media_type"] == "movie"
                && item["media_id"] == id
                && item["title"] == "In Progress Movie"
                && item["position_seconds"] == 200
        }),
        "movie with saved position should appear in watch_next"
    );
}

#[tokio::test]
async fn test_dashboard_in_progress_excludes_watched() {
    let server = TestServer::start_api_only().await;
    let id = create_movie(&server, "Finished Movie").await;

    // Save position then mark as watched
    server
        .admin_client()
        .patch_json(
            &format!("/api/v1/admin/movies/{}/watch-state/position", id),
            &json!({ "position_seconds": 200 }),
        )
        .await
        .assert_ok();

    server
        .admin_client()
        .put_json(
            &format!("/api/v1/admin/movies/{}/watch-state", id),
            &json!({ "watched": true, "watched_at": "2024-12-01T00:00:00Z" }),
        )
        .await
        .assert_ok();

    // Should NOT appear in watch_next (now on discover endpoint)
    let resp = server.admin_client().get("/api/v1/admin/discover").await;
    resp.assert_ok();
    let dash: Value = resp.json();
    let in_progress = dash["watch_next"].as_array().unwrap();
    assert!(
        !in_progress.iter().any(|item| item["media_id"] == id),
        "watched movie should not appear in watch_next"
    );
}

// =============================================================================
// Continue Episode Tests
// =============================================================================

/// Mark an episode as in-library by creating a stub episode file.
async fn mark_episode_in_library(server: &TestServer, episode_id: &str, _path: &str) {
    create_stub_episode_file(&server.pool, uuid::Uuid::parse_str(episode_id).unwrap()).await;
}

/// Helper: get the show detail and return the continue_episode field.
async fn get_continue_episode(server: &TestServer, show_id: &str) -> Value {
    let resp = server
        .admin_client()
        .get(&format!("/api/v1/admin/tv/shows/{}", show_id))
        .await;
    resp.assert_ok();
    resp.json::<Value>()["continue_episode"].clone()
}

#[tokio::test]
async fn test_continue_episode_no_history_first_in_library() {
    let server = TestServer::start_api_only().await;
    let show_id = create_show(&server, "Continue Show").await;
    let s1 = create_season(&server, &show_id, 1).await;
    let ep1 = create_episode(&server, &s1, 1, "Ep 1").await;
    let ep2 = create_episode(&server, &s1, 2, "Ep 2").await;

    // No episodes in library → null
    let ce = get_continue_episode(&server, &show_id).await;
    assert!(ce.is_null(), "should be null when no episodes in library");

    // Mark ep2 in library (ep1 not in library)
    mark_episode_in_library(&server, &ep2, "/media/tv/Continue Show/S01E02.mkv").await;

    let ce = get_continue_episode(&server, &show_id).await;
    assert!(!ce.is_null(), "should return first in-library episode");
    assert_eq!(ce["season_idx"], 0);
    assert_eq!(ce["episode_idx"], 1); // ep2 is index 1 in season
    assert_eq!(ce["episode_id"], ep2);
    assert_eq!(ce["position_seconds"], 0);

    // Now mark ep1 in library too — should prefer ep1 (first in order)
    mark_episode_in_library(&server, &ep1, "/media/tv/Continue Show/S01E01.mkv").await;

    let ce = get_continue_episode(&server, &show_id).await;
    assert_eq!(ce["episode_idx"], 0);
    assert_eq!(ce["episode_id"], ep1);
}

#[tokio::test]
async fn test_continue_episode_in_progress() {
    let server = TestServer::start_api_only().await;
    let show_id = create_show(&server, "Progress Show").await;
    let s1 = create_season(&server, &show_id, 1).await;
    let ep1 = create_episode(&server, &s1, 1, "Ep 1").await;
    let ep2 = create_episode(&server, &s1, 2, "Ep 2").await;
    mark_episode_in_library(&server, &ep1, "/media/tv/Progress Show/S01E01.mkv").await;
    mark_episode_in_library(&server, &ep2, "/media/tv/Progress Show/S01E02.mkv").await;

    // Set position on ep1 (in progress)
    server
        .admin_client()
        .put_json(
            &format!("/api/v1/admin/tv/episodes/{}/watch-state", ep1),
            &json!({
                "watched": false,
                "position_seconds": 1234
            }),
        )
        .await
        .assert_ok();

    let ce = get_continue_episode(&server, &show_id).await;
    assert_eq!(ce["episode_id"], ep1);
    assert_eq!(ce["position_seconds"], 1234);
    assert_eq!(ce["season_idx"], 0);
    assert_eq!(ce["episode_idx"], 0);
}

#[tokio::test]
async fn test_continue_episode_completed_next_unwatched() {
    let server = TestServer::start_api_only().await;
    let show_id = create_show(&server, "Next Show").await;
    let s1 = create_season(&server, &show_id, 1).await;
    let ep1 = create_episode(&server, &s1, 1, "Ep 1").await;
    let ep2 = create_episode(&server, &s1, 2, "Ep 2").await;
    let ep3 = create_episode(&server, &s1, 3, "Ep 3").await;
    mark_episode_in_library(&server, &ep1, "/media/tv/Next Show/S01E01.mkv").await;
    mark_episode_in_library(&server, &ep2, "/media/tv/Next Show/S01E02.mkv").await;
    mark_episode_in_library(&server, &ep3, "/media/tv/Next Show/S01E03.mkv").await;

    // Mark ep1 as watched
    server
        .admin_client()
        .put_json(
            &format!("/api/v1/admin/tv/episodes/{}/watch-state", ep1),
            &json!({
                "watched": true,
                "position_seconds": 0
            }),
        )
        .await
        .assert_ok();

    // Should pick ep2 (next unwatched in-library)
    let ce = get_continue_episode(&server, &show_id).await;
    assert_eq!(ce["episode_id"], ep2);
    assert_eq!(ce["position_seconds"], 0);
    assert_eq!(ce["episode_idx"], 1);

    // Mark ep2 as watched too — should pick ep3
    server
        .admin_client()
        .put_json(
            &format!("/api/v1/admin/tv/episodes/{}/watch-state", ep2),
            &json!({
                "watched": true,
                "position_seconds": 0
            }),
        )
        .await
        .assert_ok();

    let ce = get_continue_episode(&server, &show_id).await;
    assert_eq!(ce["episode_id"], ep3);
    assert_eq!(ce["episode_idx"], 2);

    // Mark ep3 as watched — nothing left, should be null
    server
        .admin_client()
        .put_json(
            &format!("/api/v1/admin/tv/episodes/{}/watch-state", ep3),
            &json!({
                "watched": true,
                "position_seconds": 0
            }),
        )
        .await
        .assert_ok();

    let ce = get_continue_episode(&server, &show_id).await;
    assert!(ce.is_null(), "should be null when all episodes watched");
}

/// Give an episode a broadcast date in the past without giving it a file.
async fn set_episode_aired(server: &TestServer, episode_id: &str, days_ago: i64) {
    let date = time::OffsetDateTime::now_utc().date() - time::Duration::days(days_ago);
    sqlx::query("UPDATE tv_episodes SET air_date = $1 WHERE id = $2::uuid")
        .bind(date)
        .bind(uuid::Uuid::parse_str(episode_id).unwrap())
        .execute(&server.pool)
        .await
        .expect("Failed to set air_date");
}
async fn set_episode_want_status(server: &TestServer, episode_id: &str, status: &str) {
    sqlx::query("UPDATE tv_episodes SET want_status = $1 WHERE id = $2::uuid")
        .bind(status)
        .bind(uuid::Uuid::parse_str(episode_id).unwrap())
        .execute(&server.pool)
        .await
        .expect("Failed to set want_status");
}

async fn mark_watched(server: &TestServer, episode_id: &str) {
    server
        .admin_client()
        .put_json(
            &format!("/api/v1/admin/tv/episodes/{}/watch-state", episode_id),
            &json!({ "watched": true, "position_seconds": 0 }),
        )
        .await
        .assert_ok();
}

/// A wanted episode is a download in flight, not something to offer as a stream — air
/// dates run early often enough that "aired" alone pointed Resume at an episode nobody
/// could play.
#[tokio::test]
async fn test_continue_episode_skips_aired_but_wanted() {
    let server = TestServer::start_api_only().await;
    let show_id = create_show(&server, "Wanted Show").await;
    let s1 = create_season(&server, &show_id, 1).await;
    let ep1 = create_episode(&server, &s1, 1, "Ep 1").await;
    let ep2 = create_episode(&server, &s1, 2, "Ep 2").await;
    let ep3 = create_episode(&server, &s1, 3, "Ep 3").await;
    set_episode_aired(&server, &ep1, 21).await;
    set_episode_aired(&server, &ep2, 14).await;
    set_episode_aired(&server, &ep3, 7).await;
    mark_episode_in_library(&server, &ep1, "/media/tv/Wanted Show/S01E01.mkv").await;
    set_episode_want_status(&server, &ep2, "wanted").await;
    set_episode_want_status(&server, &ep3, "downloading").await;

    // No history: the first downloaded episode, never the first aired one.
    let ce = get_continue_episode(&server, &show_id).await;
    assert_eq!(ce["episode_id"], ep1);

    // ep2/ep3 are on their way, so there is nothing to continue to.
    mark_watched(&server, &ep1).await;
    let ce = get_continue_episode(&server, &show_id).await;
    assert!(ce.is_null(), "wanted episode must not be offered: {ce}");

    // Downloaded, and it is the target again — skipping the still-wanted ep2.
    mark_episode_in_library(&server, &ep3, "/media/tv/Wanted Show/S01E03.mkv").await;
    let ce = get_continue_episode(&server, &show_id).await;
    assert_eq!(ce["episode_id"], ep3);
    assert_eq!(ce["episode_idx"], 2);
}

/// The other half of the rule: a show being streamed rather than collected. Aired and
/// unwanted is offered with no file at all, which the client labels "Stream" rather
/// than "Resume" — from the very first episode, before any of it has been played.
#[tokio::test]
async fn test_continue_episode_offers_aired_unwanted_episodes() {
    let server = TestServer::start_api_only().await;
    let show_id = create_show(&server, "Streamed Show").await;
    let s1 = create_season(&server, &show_id, 1).await;
    let ep1 = create_episode(&server, &s1, 1, "Ep 1").await;
    let ep2 = create_episode(&server, &s1, 2, "Ep 2").await;
    let ep3 = create_episode(&server, &s1, 3, "Ep 3").await;
    set_episode_aired(&server, &ep1, 21).await;
    set_episode_aired(&server, &ep2, 14).await;
    set_episode_aired(&server, &ep3, 7).await;
    // Nothing in the library and nothing wanted — this show is streamed.

    // No history: the first aired episode, with no file behind it.
    let ce = get_continue_episode(&server, &show_id).await;
    assert_eq!(ce["episode_id"], ep1, "first aired episode: {ce}");
    assert_eq!(ce["episode_idx"], 0);

    // Having watched ep1, ep2 is the one to continue to.
    mark_watched(&server, &ep1).await;
    let ce = get_continue_episode(&server, &show_id).await;
    assert_eq!(ce["episode_id"], ep2);
    assert_eq!(ce["episode_idx"], 1);

    // Want ep2 and it drops out again — the download supersedes the stream.
    set_episode_want_status(&server, &ep2, "wanted").await;
    let ce = get_continue_episode(&server, &show_id).await;
    assert_eq!(
        ce["episode_id"], ep3,
        "should skip the wanted one, not stop: {ce}"
    );
}

/// An episode with no broadcast date is not a stream candidate — "unwanted" on its own
/// must not turn an unaired episode into an offer.
#[tokio::test]
async fn test_continue_episode_skips_unaired_episodes() {
    let server = TestServer::start_api_only().await;
    let show_id = create_show(&server, "Unaired Show").await;
    let s1 = create_season(&server, &show_id, 1).await;
    let ep1 = create_episode(&server, &s1, 1, "Ep 1").await;
    let ep2 = create_episode(&server, &s1, 2, "Ep 2").await;
    set_episode_aired(&server, &ep1, 7).await;
    // ep2 airs next week.
    let future = time::OffsetDateTime::now_utc().date() + time::Duration::days(7);
    sqlx::query("UPDATE tv_episodes SET air_date = $1 WHERE id = $2::uuid")
        .bind(future)
        .bind(uuid::Uuid::parse_str(&ep2).unwrap())
        .execute(&server.pool)
        .await
        .expect("Failed to set air_date");

    mark_watched(&server, &ep1).await;
    let ce = get_continue_episode(&server, &show_id).await;
    assert!(ce.is_null(), "unaired episode must not be offered: {ce}");
}

/// An episode left half-played is still where you were, file or no file.
#[tokio::test]
async fn test_continue_episode_in_progress_without_file() {
    let server = TestServer::start_api_only().await;
    let show_id = create_show(&server, "Half Streamed Show").await;
    let s1 = create_season(&server, &show_id, 1).await;
    let ep1 = create_episode(&server, &s1, 1, "Ep 1").await;
    set_episode_aired(&server, &ep1, 7).await;

    server
        .admin_client()
        .put_json(
            &format!("/api/v1/admin/tv/episodes/{}/watch-state", ep1),
            &json!({ "watched": false, "position_seconds": 900 }),
        )
        .await
        .assert_ok();

    let ce = get_continue_episode(&server, &show_id).await;
    assert_eq!(ce["episode_id"], ep1);
    assert_eq!(ce["position_seconds"], 900);
}
