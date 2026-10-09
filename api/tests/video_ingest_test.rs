//! Video ingest regression tests.
//!
//! Tests that video ingest callbacks are correctly routed and that
//! book ingest still works after the ingest refactor.

use crate::common::*;
use reqwest::StatusCode;
use serde_json::json;
use uuid::Uuid;

// =============================================================================
// Movie Ingest
// =============================================================================

#[tokio::test]
async fn test_movie_ingest_callback_missing_movie_id() {
    let server = TestServer::start_api_only().await;

    // Send a movie ingest callback without movie_id in metadata
    let resp = server
        .api_client()
        .post_json(
            &format!("/api/v1/ingest/{}", server.ingest_token),
            &json!({
                "download_id": Uuid::now_v7(),
                "status": "completed",
                "metadata": {
                    "source_type": "movie"
                },
                "files": [
                    {"path": "Movie.2024.mkv", "size": 4096, "extension": "mkv"}
                ]
            }),
        )
        .await;

    // Endpoint returns 202 and enqueues the job; validation happens in the handler
    resp.assert_success();

    server.process_pending_ingest_jobs().await;

    // Job should have run and recorded the validation error
    let job: (serde_json::Value,) = sqlx::query_as(
        "SELECT COALESCE(result, '{}'::jsonb) FROM job_status WHERE job_type = 'kobload_ingest' ORDER BY created_at DESC LIMIT 1",
    )
    .fetch_one(&server.pool)
    .await
    .expect("job should exist");
    assert_eq!(
        job.0.get("status").and_then(|s| s.as_str()),
        Some("failed"),
        "Missing movie_id should cause job failure"
    );
}

#[tokio::test]
async fn test_tv_episode_ingest_callback_missing_episode_id() {
    let server = TestServer::start_api_only().await;

    let resp = server
        .api_client()
        .post_json(
            &format!("/api/v1/ingest/{}", server.ingest_token),
            &json!({
                "download_id": Uuid::now_v7(),
                "status": "completed",
                "metadata": {
                    "source_type": "tv_episode"
                },
                "files": [
                    {"path": "Show.S01E01.mkv", "size": 2048, "extension": "mkv"}
                ]
            }),
        )
        .await;

    // Endpoint returns 202; validation happens in the handler
    resp.assert_success();

    server.process_pending_ingest_jobs().await;

    let job: (serde_json::Value,) = sqlx::query_as(
        "SELECT COALESCE(result, '{}'::jsonb) FROM job_status WHERE job_type = 'kobload_ingest' ORDER BY created_at DESC LIMIT 1",
    )
    .fetch_one(&server.pool)
    .await
    .expect("job should exist");
    assert_eq!(
        job.0.get("status").and_then(|s| s.as_str()),
        Some("failed"),
        "Missing episode_id should cause job failure"
    );
}

// =============================================================================
// Non-completed Status (acknowledges without processing)
// =============================================================================

#[tokio::test]
async fn test_movie_ingest_noncompleted_status_acks() {
    let server = TestServer::start_api_only().await;

    let resp = server
        .api_client()
        .post_json(
            &format!("/api/v1/ingest/{}", server.ingest_token),
            &json!({
                "download_id": Uuid::now_v7(),
                "status": "downloading",
                "metadata": {
                    "source_type": "movie",
                    "movie_id": Uuid::now_v7()
                },
                "files": []
            }),
        )
        .await;

    // Non-completed status should be acknowledged
    assert!(
        resp.status.is_success(),
        "Non-completed status should be acknowledged, got {}",
        resp.status
    );
}

// =============================================================================
// Book Ingest Regression
// =============================================================================

#[tokio::test]
async fn test_book_ingest_still_works() {
    let server = TestServer::start_api_only().await;

    // Default source_type should be "book" (backwards-compatible)
    let resp = server
        .api_client()
        .post_json(
            &format!("/api/v1/ingest/{}", server.ingest_token),
            &json!({
                "download_id": Uuid::now_v7(),
                "status": "completed",
                "metadata": {},
                "files": [
                    {"path": "test-book.epub", "size": 1024, "extension": "epub"}
                ]
            }),
        )
        .await;

    // Book ingest should work (may fail for other reasons like no matching book,
    // but should NOT return 401 or routing errors)
    assert_ne!(
        resp.status,
        StatusCode::UNAUTHORIZED,
        "Book ingest should still accept valid token"
    );
    assert_ne!(
        resp.status,
        StatusCode::NOT_FOUND,
        "Book ingest route should exist"
    );
}

// =============================================================================
// Movie Ingest — Happy Path (no Kobload server, but validates routing + metadata extraction)
// =============================================================================

#[tokio::test]
async fn test_movie_ingest_no_video_files_returns_error() {
    let server = TestServer::start_api_only().await;
    let movie_id = Uuid::now_v7();

    // Create the movie in DB
    sqlx::query("INSERT INTO movies (id, title, year) VALUES ($1, 'Test Movie', 2024)")
        .bind(movie_id)
        .execute(&server.pool)
        .await
        .unwrap();

    // Send callback with non-video files
    let resp = server
        .api_client()
        .post_json(
            &format!("/api/v1/ingest/{}", server.ingest_token),
            &json!({
                "download_id": Uuid::now_v7(),
                "status": "completed",
                "metadata": {
                    "source_type": "movie",
                    "movie_id": movie_id
                },
                "files": [
                    {"path": "readme.txt", "size": 100, "extension": "txt"}
                ]
            }),
        )
        .await;

    // Endpoint returns 202; validation happens in the handler
    resp.assert_success();

    server.process_pending_ingest_jobs().await;

    let job: (serde_json::Value,) = sqlx::query_as(
        "SELECT COALESCE(result, '{}'::jsonb) FROM job_status WHERE job_type = 'kobload_ingest' ORDER BY created_at DESC LIMIT 1",
    )
    .fetch_one(&server.pool)
    .await
    .expect("job should exist");
    assert_eq!(
        job.0.get("status").and_then(|s| s.as_str()),
        Some("failed"),
        "No video files should cause job failure"
    );
}

#[tokio::test]
async fn test_tv_episode_ingest_no_video_files_returns_error() {
    let server = TestServer::start_api_only().await;

    let show_id = Uuid::now_v7();
    sqlx::query("INSERT INTO tv_shows (id, title, year) VALUES ($1, 'Test Show', 2024)")
        .bind(show_id)
        .execute(&server.pool)
        .await
        .unwrap();

    let season_id = Uuid::now_v7();
    sqlx::query("INSERT INTO tv_seasons (id, show_id, season_identifier, season_sort_order, is_monitored, is_special) VALUES ($1, $2, '1', 1, true, false)")
        .bind(season_id)
        .bind(show_id)
        .execute(&server.pool)
        .await
        .unwrap();

    let ep_id = Uuid::now_v7();
    sqlx::query("INSERT INTO tv_episodes (id, season_id, episode_identifier, episode_sort_order, title) VALUES ($1, $2, '1', 1, 'Pilot')")
        .bind(ep_id)
        .bind(season_id)
        .execute(&server.pool)
        .await
        .unwrap();

    let resp = server
        .api_client()
        .post_json(
            &format!("/api/v1/ingest/{}", server.ingest_token),
            &json!({
                "download_id": Uuid::now_v7(),
                "status": "completed",
                "metadata": {
                    "source_type": "tv_episode",
                    "show_id": show_id,
                    "season_id": season_id,
                    "episode_id": ep_id
                },
                "files": [
                    {"path": "nfo_file.nfo", "size": 200, "extension": "nfo"}
                ]
            }),
        )
        .await;

    // Endpoint returns 202; validation happens in the handler
    resp.assert_success();

    server.process_pending_ingest_jobs().await;

    let job: (serde_json::Value,) = sqlx::query_as(
        "SELECT COALESCE(result, '{}'::jsonb) FROM job_status WHERE job_type = 'kobload_ingest' ORDER BY created_at DESC LIMIT 1",
    )
    .fetch_one(&server.pool)
    .await
    .expect("job should exist");
    assert_eq!(
        job.0.get("status").and_then(|s| s.as_str()),
        Some("failed"),
        "No video files should cause job failure"
    );
}

#[tokio::test]
async fn test_tv_season_ingest_no_video_files_returns_error() {
    let server = TestServer::start_api_only().await;

    let show_id = Uuid::now_v7();
    sqlx::query("INSERT INTO tv_shows (id, title, year) VALUES ($1, 'Pack Show', 2024)")
        .bind(show_id)
        .execute(&server.pool)
        .await
        .unwrap();

    let season_id = Uuid::now_v7();
    sqlx::query("INSERT INTO tv_seasons (id, show_id, season_identifier, season_sort_order, is_monitored, is_special) VALUES ($1, $2, '1', 1, true, false)")
        .bind(season_id)
        .bind(show_id)
        .execute(&server.pool)
        .await
        .unwrap();

    let ep1_id = Uuid::now_v7();
    let ep2_id = Uuid::now_v7();
    for (ep_id, num) in [(ep1_id, 1), (ep2_id, 2)] {
        sqlx::query("INSERT INTO tv_episodes (id, season_id, episode_identifier, episode_sort_order, title) VALUES ($1, $2, $3, $4, $5)")
            .bind(ep_id)
            .bind(season_id)
            .bind(num.to_string())
            .bind(num)
            .bind(format!("Episode {}", num))
            .execute(&server.pool)
            .await
            .unwrap();
    }

    let resp = server
        .api_client()
        .post_json(
            &format!("/api/v1/ingest/{}", server.ingest_token),
            &json!({
                "download_id": Uuid::now_v7(),
                "status": "completed",
                "metadata": {
                    "source_type": "tv_season",
                    "show_id": show_id,
                    "season_id": season_id,
                    "episode_map": {
                        "1": ep1_id,
                        "2": ep2_id
                    }
                },
                "files": [
                    {"path": "sample.txt", "size": 50, "extension": "txt"}
                ]
            }),
        )
        .await;

    // Endpoint returns 202; validation happens in the handler
    resp.assert_success();

    server.process_pending_ingest_jobs().await;

    let job: (serde_json::Value,) = sqlx::query_as(
        "SELECT COALESCE(result, '{}'::jsonb) FROM job_status WHERE job_type = 'kobload_ingest' ORDER BY created_at DESC LIMIT 1",
    )
    .fetch_one(&server.pool)
    .await
    .expect("job should exist");
    assert_eq!(
        job.0.get("status").and_then(|s| s.as_str()),
        Some("failed"),
        "No video files should cause job failure"
    );
}

// =============================================================================
// Auth
// =============================================================================

#[tokio::test]
async fn test_ingest_requires_valid_token() {
    let server = TestServer::start_api_only().await;

    let resp = server
        .api_client()
        .post_json(
            "/api/v1/ingest/invalid-token",
            &json!({
                "download_id": Uuid::now_v7(),
                "status": "completed",
                "metadata": {"source_type": "movie"},
                "files": []
            }),
        )
        .await;

    assert_eq!(resp.status, StatusCode::UNAUTHORIZED);
}
