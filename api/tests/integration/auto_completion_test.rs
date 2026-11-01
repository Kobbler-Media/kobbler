use crate::common::*;
use reqwest::StatusCode;
use serde_json::{Value, json};
use uuid::Uuid;

// =============================================================================
// Local Helpers
// =============================================================================

async fn create_movie(server: &TestServer, title: &str) -> String {
    let resp = server
        .admin_client()
        .post_json(
            "/api/v1/admin/movies",
            &json!({ "title": title, "year": 2024 }),
        )
        .await;
    resp.assert_status(StatusCode::CREATED);
    resp.json::<Value>()["id"].as_str().unwrap().to_string()
}

async fn create_show(server: &TestServer, title: &str) -> String {
    let resp = server
        .admin_client()
        .post_json(
            "/api/v1/admin/tv/shows",
            &json!({ "title": title, "year": 2024 }),
        )
        .await;
    resp.assert_status(StatusCode::CREATED);
    resp.json::<Value>()["id"].as_str().unwrap().to_string()
}

async fn create_season(server: &TestServer, show_id: &str, season_num: i32) -> String {
    let id = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO tv_seasons (id, show_id, season_identifier, season_sort_order, is_monitored, is_special) \
         VALUES ($1, $2::uuid, $3, $4, true, false)",
    )
    .bind(id)
    .bind(Uuid::parse_str(show_id).unwrap())
    .bind(season_num.to_string())
    .bind(season_num)
    .execute(&server.pool)
    .await
    .unwrap();
    id.to_string()
}

async fn create_episode(
    server: &TestServer,
    season_id: &str,
    episode_num: i32,
    title: &str,
) -> String {
    let id = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO tv_episodes (id, season_id, episode_identifier, episode_sort_order, title) \
         VALUES ($1, $2::uuid, $3, $4, $5)",
    )
    .bind(id)
    .bind(Uuid::parse_str(season_id).unwrap())
    .bind(episode_num.to_string())
    .bind(episode_num)
    .bind(title)
    .execute(&server.pool)
    .await
    .unwrap();
    id.to_string()
}

async fn insert_movie_file_with_duration(
    server: &TestServer,
    movie_id: &str,
    duration: f64,
) -> Uuid {
    let file_id = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO movie_files (id, movie_id, file_path, format, available, duration_seconds) \
         VALUES ($1, $2::uuid, '/test/movie-' || $1::text || '.mkv', 'MKV', true, $3)",
    )
    .bind(file_id)
    .bind(Uuid::parse_str(movie_id).unwrap())
    .bind(duration)
    .execute(&server.pool)
    .await
    .unwrap();
    file_id
}

async fn insert_episode_file_with_duration(
    server: &TestServer,
    episode_id: &str,
    duration: f64,
) -> Uuid {
    let file_id = Uuid::now_v7();
    let ep_uuid = Uuid::parse_str(episode_id).unwrap();
    sqlx::query(
        "INSERT INTO tv_episode_files (id, file_path, format, available, duration_seconds) \
         VALUES ($1, '/test/episode-' || $1::text || '.mkv', 'MKV', true, $2)",
    )
    .bind(file_id)
    .bind(duration)
    .execute(&server.pool)
    .await
    .unwrap();

    sqlx::query(
        "INSERT INTO tv_episode_file_episodes (episode_file_id, episode_id) VALUES ($1, $2)",
    )
    .bind(file_id)
    .bind(ep_uuid)
    .execute(&server.pool)
    .await
    .unwrap();
    file_id
}

const EPISODE_CREDITS: &str = "INSERT INTO episode_skip_segments \
     (episode_file_id, segment_type, start_seconds, end_seconds) VALUES ($1, 'credits', $2, $3)";
const MOVIE_CREDITS: &str = "INSERT INTO movie_skip_segments \
     (movie_file_id, segment_type, start_seconds, end_seconds) VALUES ($1, 'credits', $2, $3)";

async fn insert_credits(
    server: &TestServer,
    sql: &'static str,
    file_id: Uuid,
    start: f64,
    end: f64,
) {
    sqlx::query(sql)
        .bind(file_id)
        .bind(start)
        .bind(end)
        .execute(&server.pool)
        .await
        .unwrap();
}

// =============================================================================
// Movie Auto-Completion Tests
// =============================================================================

#[tokio::test]
async fn test_movie_auto_complete_at_90_percent() {
    let server = TestServer::start_api_only().await;
    let id = create_movie(&server, "Auto Complete Movie").await;
    insert_movie_file_with_duration(&server, &id, 1000.0).await;

    // Position at 91% → should auto-mark as watched
    server
        .admin_client()
        .patch_json(
            &format!("/api/v1/admin/movies/{id}/watch-state/position"),
            &json!({ "position_seconds": 910 }),
        )
        .await
        .assert_ok();

    let ws: Value = server
        .admin_client()
        .get(&format!("/api/v1/admin/movies/{id}/watch-state"))
        .await
        .json();
    assert_eq!(
        ws["watched"], true,
        "movie should be auto-marked as watched at 91%"
    );
    assert!(ws["watched_at"].is_string(), "watched_at should be set");
}

#[tokio::test]
async fn test_movie_no_auto_complete_below_90_percent() {
    let server = TestServer::start_api_only().await;
    let id = create_movie(&server, "No Auto Complete Movie").await;
    insert_movie_file_with_duration(&server, &id, 1000.0).await;

    // Position at 89% → should NOT auto-mark
    server
        .admin_client()
        .patch_json(
            &format!("/api/v1/admin/movies/{id}/watch-state/position"),
            &json!({ "position_seconds": 890 }),
        )
        .await
        .assert_ok();

    let ws: Value = server
        .admin_client()
        .get(&format!("/api/v1/admin/movies/{id}/watch-state"))
        .await
        .json();
    assert_eq!(
        ws["watched"], false,
        "movie should NOT be auto-marked at 89%"
    );
}

// =============================================================================
// TV Episode Auto-Completion Tests
// =============================================================================

#[tokio::test]
async fn test_episode_auto_complete_at_90_percent() {
    let server = TestServer::start_api_only().await;
    let show_id = create_show(&server, "Auto Complete Show").await;
    let season_id = create_season(&server, &show_id, 1).await;
    let ep_id = create_episode(&server, &season_id, 1, "Pilot").await;
    insert_episode_file_with_duration(&server, &ep_id, 2400.0).await;

    // Position at 91% → should auto-mark
    server
        .admin_client()
        .patch_json(
            &format!("/api/v1/admin/tv/episodes/{ep_id}/watch-state/position"),
            &json!({ "position_seconds": 2184 }),
        )
        .await
        .assert_ok();

    let ws: Value = server
        .admin_client()
        .get(&format!("/api/v1/admin/tv/episodes/{ep_id}/watch-state"))
        .await
        .json();
    assert_eq!(
        ws["watched"], true,
        "episode should be auto-marked as watched at 91%"
    );
    assert!(ws["watched_at"].is_string(), "watched_at should be set");
}

#[tokio::test]
async fn test_episode_no_auto_complete_below_90_percent() {
    let server = TestServer::start_api_only().await;
    let show_id = create_show(&server, "No Auto Complete Show").await;
    let season_id = create_season(&server, &show_id, 1).await;
    let ep_id = create_episode(&server, &season_id, 1, "Pilot").await;
    insert_episode_file_with_duration(&server, &ep_id, 2400.0).await;

    // Position at 89% → should NOT auto-mark
    server
        .admin_client()
        .patch_json(
            &format!("/api/v1/admin/tv/episodes/{ep_id}/watch-state/position"),
            &json!({ "position_seconds": 2136 }),
        )
        .await
        .assert_ok();

    let ws: Value = server
        .admin_client()
        .get(&format!("/api/v1/admin/tv/episodes/{ep_id}/watch-state"))
        .await
        .json();
    assert_eq!(
        ws["watched"], false,
        "episode should NOT be auto-marked at 89%"
    );
}

// =============================================================================
// Credits-Aware Completion
// =============================================================================

struct CreditsCase {
    name: &'static str,
    credits_start: Option<f64>,
    position: i32,
    watched: bool,
}

/// A 3311 s file whose 90% mark (2980 s) falls after the credits start, the way
/// Carrie 1x01's does: name cards at 2918 s, the crawl not until 3131 s.
const CREDITS_CASES: &[CreditsCase] = &[
    CreditsCase {
        name: "inside the credits, short of 90%",
        credits_start: Some(2918.0),
        position: 2948,
        watched: true,
    },
    CreditsCase {
        name: "just before the credits",
        credits_start: Some(2918.0),
        position: 2900,
        watched: false,
    },
    CreditsCase {
        name: "credits after the 90% mark",
        credits_start: Some(3131.0),
        position: 2990,
        watched: true,
    },
    CreditsCase {
        name: "no credits detected",
        credits_start: None,
        position: 2948,
        watched: false,
    },
    CreditsCase {
        name: "implausibly early credits",
        credits_start: Some(600.0),
        position: 700,
        watched: false,
    },
];
const CREDITS_CASE_DURATION: f64 = 3311.0;

#[tokio::test]
async fn test_episode_completes_at_credits_start() {
    let server = TestServer::start_api_only().await;
    let show_id = create_show(&server, "Credits Show").await;
    let season_id = create_season(&server, &show_id, 1).await;
    for (n, case) in CREDITS_CASES.iter().enumerate() {
        let ep_id = create_episode(&server, &season_id, n as i32 + 1, case.name).await;
        let file_id =
            insert_episode_file_with_duration(&server, &ep_id, CREDITS_CASE_DURATION).await;
        if let Some(start) = case.credits_start {
            insert_credits(
                &server,
                EPISODE_CREDITS,
                file_id,
                start,
                CREDITS_CASE_DURATION,
            )
            .await;
        }
        let saved: Value = server
            .admin_client()
            .patch_json(
                &format!("/api/v1/admin/tv/episodes/{ep_id}/watch-state/position"),
                &json!({ "position_seconds": case.position, "client_id": "phone" }),
            )
            .await
            .json();
        assert_eq!(
            saved["watched"], case.watched,
            "episode save response: {}",
            case.name
        );

        let ws: Value = server
            .admin_client()
            .get(&format!("/api/v1/admin/tv/episodes/{ep_id}/watch-state"))
            .await
            .json();
        assert_eq!(ws["watched"], case.watched, "episode: {}", case.name);

        let handoff: Value = server
            .admin_client()
            .get(&format!(
                "/api/v1/admin/playback/handoff?media_type=episode&media_id={ep_id}"
            ))
            .await
            .json();
        assert_eq!(
            handoff["latest"]["watched"], case.watched,
            "episode handoff: {}",
            case.name
        );
    }
}

#[tokio::test]
async fn test_movie_completes_at_credits_start() {
    let server = TestServer::start_api_only().await;
    for case in CREDITS_CASES {
        let id = create_movie(&server, case.name).await;
        let file_id = insert_movie_file_with_duration(&server, &id, CREDITS_CASE_DURATION).await;
        if let Some(start) = case.credits_start {
            insert_credits(
                &server,
                MOVIE_CREDITS,
                file_id,
                start,
                CREDITS_CASE_DURATION,
            )
            .await;
        }
        let saved: Value = server
            .admin_client()
            .patch_json(
                &format!("/api/v1/admin/movies/{id}/watch-state/position"),
                &json!({ "position_seconds": case.position, "client_id": "phone" }),
            )
            .await
            .json();
        assert_eq!(
            saved["watched"], case.watched,
            "movie save response: {}",
            case.name
        );

        let ws: Value = server
            .admin_client()
            .get(&format!("/api/v1/admin/movies/{id}/watch-state"))
            .await
            .json();
        assert_eq!(ws["watched"], case.watched, "movie: {}", case.name);

        let handoff: Value = server
            .admin_client()
            .get(&format!(
                "/api/v1/admin/playback/handoff?media_type=movie&media_id={id}"
            ))
            .await
            .json();
        assert_eq!(
            handoff["latest"]["watched"], case.watched,
            "movie handoff: {}",
            case.name
        );
    }
}

#[tokio::test]
async fn test_save_without_a_file_has_no_watched_verdict() {
    let server = TestServer::start_api_only().await;
    let show_id = create_show(&server, "Streamed Show").await;
    let season_id = create_season(&server, &show_id, 1).await;
    let ep_id = create_episode(&server, &season_id, 1, "Pilot").await;
    let movie_id = create_movie(&server, "Streamed Movie").await;
    for path in [
        format!("/api/v1/admin/tv/episodes/{ep_id}/watch-state/position"),
        format!("/api/v1/admin/movies/{movie_id}/watch-state/position"),
    ] {
        let saved: Value = server
            .admin_client()
            .patch_json(&path, &json!({ "position_seconds": 3000 }))
            .await
            .json();
        assert!(saved["watched"].is_null(), "{path}: {saved}");
    }
}

#[tokio::test]
async fn test_episode_seeking_back_before_credits_unwatches() {
    let server = TestServer::start_api_only().await;
    let show_id = create_show(&server, "Rewind Show").await;
    let season_id = create_season(&server, &show_id, 1).await;
    let ep_id = create_episode(&server, &season_id, 1, "Pilot").await;
    let file_id = insert_episode_file_with_duration(&server, &ep_id, CREDITS_CASE_DURATION).await;
    insert_credits(
        &server,
        EPISODE_CREDITS,
        file_id,
        2918.0,
        CREDITS_CASE_DURATION,
    )
    .await;

    for (position, watched) in [(2948, true), (2948 - 100, false)] {
        server
            .admin_client()
            .patch_json(
                &format!("/api/v1/admin/tv/episodes/{ep_id}/watch-state/position"),
                &json!({ "position_seconds": position }),
            )
            .await
            .assert_ok();
        let ws: Value = server
            .admin_client()
            .get(&format!("/api/v1/admin/tv/episodes/{ep_id}/watch-state"))
            .await
            .json();
        assert_eq!(ws["watched"], watched, "at {position}s");
    }
}

// =============================================================================
// Book Auto-Completion Tests
// =============================================================================

#[tokio::test]
async fn test_book_auto_complete_at_95_percent() {
    let server = TestServer::start_api_only().await;
    let book_id = create_book(&server, "Almost Done Book").await;

    // Set progress to 96% (no explicit status) → should auto-set to "finished"
    server
        .admin_client()
        .put_json(
            &format!("/api/v1/admin/books/{book_id}/reading"),
            &json!({ "progress_percent": 96.0 }),
        )
        .await
        .assert_status(StatusCode::NO_CONTENT);

    let json: Value = server
        .admin_client()
        .get(&format!("/api/v1/admin/books/{book_id}/reading"))
        .await
        .json();
    assert_eq!(
        json["status"], "finished",
        "book should be auto-finished at 96%"
    );
}

#[tokio::test]
async fn test_book_no_auto_complete_below_95_percent() {
    let server = TestServer::start_api_only().await;
    let book_id = create_book(&server, "Still Reading Book").await;

    // Set progress to 94% → should NOT auto-finish
    server
        .admin_client()
        .put_json(
            &format!("/api/v1/admin/books/{book_id}/reading"),
            &json!({ "progress_percent": 94.0 }),
        )
        .await
        .assert_status(StatusCode::NO_CONTENT);

    let json: Value = server
        .admin_client()
        .get(&format!("/api/v1/admin/books/{book_id}/reading"))
        .await
        .json();
    assert_ne!(
        json["status"].as_str().unwrap_or(""),
        "finished",
        "book should NOT be auto-finished at 94%"
    );
}

#[tokio::test]
async fn test_book_explicit_status_overrides_auto_complete() {
    let server = TestServer::start_api_only().await;
    let book_id = create_book(&server, "Override Book").await;

    // Set status to "reading" explicitly along with 96% progress
    server
        .admin_client()
        .put_json(
            &format!("/api/v1/admin/books/{book_id}/reading"),
            &json!({ "status": "reading", "progress_percent": 96.0 }),
        )
        .await
        .assert_status(StatusCode::NO_CONTENT);

    let json: Value = server
        .admin_client()
        .get(&format!("/api/v1/admin/books/{book_id}/reading"))
        .await
        .json();
    assert_eq!(
        json["status"], "reading",
        "explicit 'reading' status should override auto-completion"
    );
}

// =============================================================================
// Manual Toggle After Auto-Completion
// =============================================================================

#[tokio::test]
async fn test_movie_unwatched_after_auto_complete() {
    let server = TestServer::start_api_only().await;
    let id = create_movie(&server, "Re-watch Movie").await;
    insert_movie_file_with_duration(&server, &id, 1000.0).await;

    // Auto-complete by watching to 91%
    server
        .admin_client()
        .patch_json(
            &format!("/api/v1/admin/movies/{id}/watch-state/position"),
            &json!({ "position_seconds": 910 }),
        )
        .await
        .assert_ok();

    // Manually mark as unwatched
    server
        .admin_client()
        .put_json(
            &format!("/api/v1/admin/movies/{id}/watch-state"),
            &json!({ "watched": false }),
        )
        .await
        .assert_ok();

    let ws: Value = server
        .admin_client()
        .get(&format!("/api/v1/admin/movies/{id}/watch-state"))
        .await
        .json();
    assert_eq!(
        ws["watched"], false,
        "manual unwatched should override auto-completion"
    );
}

#[tokio::test]
async fn test_book_unfinish_after_auto_complete() {
    let server = TestServer::start_api_only().await;
    let book_id = create_book(&server, "Re-read Book").await;

    // Auto-complete by setting 96% progress
    server
        .admin_client()
        .put_json(
            &format!("/api/v1/admin/books/{book_id}/reading"),
            &json!({ "progress_percent": 96.0 }),
        )
        .await
        .assert_status(StatusCode::NO_CONTENT);

    // Manually set back to reading
    server
        .admin_client()
        .put_json(
            &format!("/api/v1/admin/books/{book_id}/reading"),
            &json!({ "status": "reading" }),
        )
        .await
        .assert_status(StatusCode::NO_CONTENT);

    let json: Value = server
        .admin_client()
        .get(&format!("/api/v1/admin/books/{book_id}/reading"))
        .await
        .json();
    assert_eq!(
        json["status"], "reading",
        "manual 'reading' should override auto-finish"
    );
}

// =============================================================================
// Dashboard Start Threshold Tests
// =============================================================================

#[tokio::test]
async fn test_dashboard_movie_start_threshold() {
    let server = TestServer::start_api_only().await;
    let id = create_movie(&server, "Short Watch Movie").await;

    // Save position at 30 seconds (below 60s threshold)
    server
        .admin_client()
        .patch_json(
            &format!("/api/v1/admin/movies/{id}/watch-state/position"),
            &json!({ "position_seconds": 30 }),
        )
        .await
        .assert_ok();

    // Discover should NOT include this movie in watch_next
    let dash: Value = server
        .admin_client()
        .get("/api/v1/admin/discover")
        .await
        .json();
    let in_progress = dash["watch_next"].as_array().unwrap();
    assert!(
        !in_progress
            .iter()
            .any(|item| item["media_id"].as_str() == Some(&id)),
        "movie with 30s position should NOT appear in watch_next"
    );

    // Update to 60 seconds (at threshold)
    server
        .admin_client()
        .patch_json(
            &format!("/api/v1/admin/movies/{id}/watch-state/position"),
            &json!({ "position_seconds": 60 }),
        )
        .await
        .assert_ok();

    // Discover SHOULD include this movie now
    let dash: Value = server
        .admin_client()
        .get("/api/v1/admin/discover")
        .await
        .json();
    let in_progress = dash["watch_next"].as_array().unwrap();
    assert!(
        in_progress
            .iter()
            .any(|item| item["media_id"].as_str() == Some(&id)),
        "movie with 60s position should appear in watch_next"
    );
}
