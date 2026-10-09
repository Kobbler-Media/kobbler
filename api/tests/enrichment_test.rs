//! Tests for per-entity enrichment job handlers and the media_root_enrich catch-up orchestrator.

use crate::common::*;
use api::jobs::worker::JobHandler;
use reqwest::StatusCode;
use serde_json::{Value, json};
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

// =============================================================================
// Test Helpers
// =============================================================================

/// Create a movie via the admin API and return its ID.
async fn create_movie(server: &TestServer, title: &str, tmdb_id: Option<&str>) -> String {
    let mut body = json!({ "title": title, "year": 2024 });
    if let Some(id) = tmdb_id {
        body["identifiers"] = json!({ "tmdb_id": id });
    }
    let resp = server
        .admin_client()
        .post_json("/api/v1/admin/movies", &body)
        .await;
    resp.assert_status(StatusCode::CREATED);
    resp.json::<Value>()["id"].as_str().unwrap().to_string()
}

/// Create a TV show via the admin API and return its ID.
async fn create_show(server: &TestServer, title: &str, tmdb_id: Option<&str>) -> String {
    let mut body = json!({ "title": title, "year": 2024 });
    if let Some(id) = tmdb_id {
        body["identifiers"] = json!({ "tmdb_id": id });
    }
    let resp = server
        .admin_client()
        .post_json("/api/v1/admin/tv/shows", &body)
        .await;
    resp.assert_status(StatusCode::CREATED);
    resp.json::<Value>()["id"].as_str().unwrap().to_string()
}

// =============================================================================
// Movie Enrichment
// =============================================================================

#[tokio::test]
async fn test_movie_enrich_missing_tmdb_id_returns_error() {
    let server = TestServer::start_api_only().await;
    let movie_id = create_movie(&server, "No TMDB Movie", None).await;
    let movie_uuid: Uuid = movie_id.parse().unwrap();

    let result =
        api::jobs::handlers::movie_enrich::enrich_movie(&server.pool, movie_uuid, false).await;

    assert!(result.is_err(), "Should fail when movie has no provider ID");
    let err = result.unwrap_err().to_string();
    assert!(
        err.contains("No enabled provider")
            || err.contains("no identifiers")
            || err.contains("not found"),
        "Error should mention missing provider: {err}"
    );
}

#[tokio::test]
async fn test_movie_refresh_metadata_requires_tmdb_id() {
    let server = TestServer::start_api_only().await;
    let movie_id = create_movie(&server, "No TMDB Movie", None).await;

    let resp = server
        .admin_client()
        .post_json(
            &format!("/api/v1/admin/movies/{movie_id}/refresh-metadata"),
            &json!({}),
        )
        .await;

    // Should fail because the movie has no TMDB identifier
    resp.assert_status(StatusCode::BAD_REQUEST);
    let body = resp.json::<Value>();
    assert!(body["error"].as_str().unwrap_or("").contains("TMDB"));
}

// =============================================================================
// TV Show Enrichment
// =============================================================================

#[tokio::test]
async fn test_tv_show_enrich_missing_tmdb_id_returns_error() {
    let server = TestServer::start_api_only().await;
    let show_id = create_show(&server, "No TMDB Show", None).await;
    let show_uuid: Uuid = show_id.parse().unwrap();

    let result =
        api::jobs::handlers::tv_show_enrich::enrich_tv_show(&server.pool, show_uuid, false).await;

    assert!(result.is_err(), "Should fail when show has no provider ID");
    let err = result.unwrap_err().to_string();
    assert!(
        err.contains("No enabled provider")
            || err.contains("no identifiers")
            || err.contains("not found"),
        "Error should mention missing provider: {err}"
    );
}

/// Seasons are committed as they are fetched and one season the provider will
/// not serve aborts the rest, so a failed enrichment still changed the library.
/// Telling search only on success is how a show ends up fully present in the UI
/// and absent from the index — which is what turned a request for one season
/// into a question about two others.
#[tokio::test]
async fn a_failed_enrichment_still_tells_search_what_it_wrote() {
    use nats_jobs::JobHandler;

    let server = TestServer::start_api_only().await;
    let show_id = create_show(&server, "No TMDB Show", None).await;
    let show_uuid: Uuid = show_id.parse().unwrap();
    let mut rx = server.library_events.subscribe();

    let handler = api::jobs::handlers::tv_show_enrich::TvShowEnrichHandler::new(
        server.pool.clone(),
        server.nats_publisher.clone(),
        "/tmp".to_string(),
        server.library_events.clone(),
        server.runtime_config.clone(),
    );
    let result = handler
        .execute(
            Uuid::now_v7(),
            serde_json::json!({ "show_id": show_uuid }),
            tokio_util::sync::CancellationToken::new(),
        )
        .await;
    assert!(result.is_err(), "the show has no provider id");

    let event = tokio::time::timeout(std::time::Duration::from_millis(500), rx.recv())
        .await
        .expect("a failed enrichment still has to say what it wrote")
        .expect("channel open");
    assert_eq!(event.kind(), "tv_show_updated");
}

/// A show added from Discover is asked for before it has any episodes: creation enqueues
/// enrichment and returns an id, and the want lands on a show with no seasons at all. The
/// want is persisted, but nothing re-read it when the episodes turned up minutes later —
/// the hourly monitor sweep was the first thing to notice, so a show asked for at midnight
/// collected nothing until one, and an unmonitored one never collected anything.
#[tokio::test]
async fn a_want_set_before_the_episodes_exist_is_applied_when_they_arrive() {
    let server = TestServer::start_api_only().await;
    let show_id = create_show(&server, "Late Arrival", None).await;
    let show_uuid: Uuid = show_id.parse().unwrap();

    server
        .admin_client()
        .post_json(
            &format!("/api/v1/admin/tv/shows/{show_id}/monitor"),
            &json!({ "is_monitored": true, "auto_want_mode": "all" }),
        )
        .await
        .assert_success();

    let season_id = create_season(&server, &show_id, 1).await;
    let episode_id = create_episode(&server, &season_id, 1, "Pilot").await;

    let wanted = api::jobs::handlers::auto_want::apply_show_auto_want(
        &server.pool,
        &server.nats_publisher,
        show_uuid,
    )
    .await
    .expect("applying the show's standing want");

    assert_eq!(
        1, wanted,
        "the episode that arrived after the want was left alone"
    );
    let resp = server
        .admin_client()
        .get(&format!("/api/v1/admin/tv/episodes/{episode_id}"))
        .await;
    resp.assert_ok();
    assert_eq!(resp.json::<Value>()["want_status"], "wanted");
}

/// A show nobody asked for is not wanted just because its episodes arrived.
#[tokio::test]
async fn episodes_arriving_for_an_unwanted_show_are_left_alone() {
    let server = TestServer::start_api_only().await;
    let show_id = create_show(&server, "Just Browsing", None).await;
    let season_id = create_season(&server, &show_id, 1).await;
    let episode_id = create_episode(&server, &season_id, 1, "Pilot").await;

    let wanted = api::jobs::handlers::auto_want::apply_show_auto_want(
        &server.pool,
        &server.nats_publisher,
        show_id.parse().unwrap(),
    )
    .await
    .expect("applying the show's standing want");

    assert_eq!(0, wanted);
    let resp = server
        .admin_client()
        .get(&format!("/api/v1/admin/tv/episodes/{episode_id}"))
        .await;
    resp.assert_ok();
    assert_eq!(resp.json::<Value>()["want_status"], "none");
}

/// Create a season directly in the DB, as enrichment does. Returns the season ID.
async fn create_season(server: &TestServer, show_id: &str, season_num: i32) -> String {
    let id = Uuid::now_v7();
    sqlx::query(
        r#"
        INSERT INTO tv_seasons (id, show_id, season_identifier, season_sort_order, is_monitored, is_special)
        VALUES ($1, $2::uuid, $3, $4, true, false)
        "#,
    )
    .bind(id)
    .bind(Uuid::parse_str(show_id).unwrap())
    .bind(season_num.to_string())
    .bind(season_num)
    .execute(&server.pool)
    .await
    .expect("Failed to create season");
    id.to_string()
}

/// Create an episode directly in the DB, as enrichment does. Returns the episode ID.
async fn create_episode(
    server: &TestServer,
    season_id: &str,
    episode_num: i32,
    title: &str,
) -> String {
    let id = Uuid::now_v7();
    sqlx::query(
        r#"
        INSERT INTO tv_episodes (id, season_id, episode_identifier, episode_sort_order, title)
        VALUES ($1, $2::uuid, $3, $4, $5)
        "#,
    )
    .bind(id)
    .bind(Uuid::parse_str(season_id).unwrap())
    .bind(episode_num.to_string())
    .bind(episode_num)
    .bind(title)
    .execute(&server.pool)
    .await
    .expect("Failed to create episode");
    id.to_string()
}

#[tokio::test]
async fn test_tv_show_refresh_metadata_requires_tmdb_id() {
    let server = TestServer::start_api_only().await;
    let show_id = create_show(&server, "No TMDB Show", None).await;

    let resp = server
        .admin_client()
        .post_json(
            &format!("/api/v1/admin/tv/shows/{show_id}/refresh-metadata"),
            &json!({}),
        )
        .await;

    // Should fail because the show has no TMDB identifier
    resp.assert_status(StatusCode::BAD_REQUEST);
    let body = resp.json::<Value>();
    assert!(body["error"].as_str().unwrap_or("").contains("TMDB"));
}

// =============================================================================
// Media Root Enrich Catch-Up Orchestrator
// =============================================================================

#[tokio::test]
async fn test_media_root_enrich_queries_unenriched_movies() {
    let server = TestServer::start_api_only().await;

    // Create a media root for movies
    let root_resp = server
        .admin_client()
        .post_json(
            "/api/v1/admin/media-roots",
            &json!({
                "name": "Test Movie Root",
                "path": "/tmp/test-movies",
                "media_type": "movie"
            }),
        )
        .await;
    root_resp.assert_status(StatusCode::CREATED);
    let root_id: String = root_resp.json::<Value>()["id"]
        .as_str()
        .unwrap()
        .to_string();
    let root_uuid: Uuid = root_id.parse().unwrap();

    // Insert a movie that has tmdb_id but no description (unenriched)
    let movie_id = Uuid::now_v7();
    sqlx::query(
        r#"INSERT INTO movies (id, media_root_id, title, year, identifiers)
           VALUES ($1, $2, 'Unenriched Movie', 2024, '{"tmdb_id": "12345"}'::jsonb)"#,
    )
    .bind(movie_id)
    .bind(root_uuid)
    .execute(&server.pool)
    .await
    .unwrap();

    // Insert an enriched movie (has description — should not be queued)
    let enriched_id = Uuid::now_v7();
    sqlx::query(
        r#"INSERT INTO movies (id, media_root_id, title, year, description, identifiers)
           VALUES ($1, $2, 'Enriched Movie', 2024, 'A great film.', '{"tmdb_id": "67890"}'::jsonb)"#,
    )
    .bind(enriched_id)
    .bind(root_uuid)
    .execute(&server.pool)
    .await
    .unwrap();

    // Use the handler to check which movies would be enqueued
    let handler = api::jobs::handlers::MediaRootEnrichHandler::new(
        server.pool.clone(),
        server.nats_publisher.clone(),
        "/tmp".to_string(),
    );

    let result = handler
        .execute(
            Uuid::now_v7(),
            json!({ "media_root_id": root_id }),
            CancellationToken::new(),
        )
        .await;

    assert!(result.is_ok(), "Handler should succeed");
    let val = result.unwrap();
    // At least the unenriched movie should have been queued
    assert!(
        val["enqueued"].as_u64().unwrap_or(0) >= 1,
        "Should enqueue at least 1 unenriched movie, got: {val}"
    );
}

#[tokio::test]
async fn test_media_root_enrich_queries_unenriched_tv_shows() {
    let server = TestServer::start_api_only().await;

    // Create a media root for TV
    let root_resp = server
        .admin_client()
        .post_json(
            "/api/v1/admin/media-roots",
            &json!({
                "name": "Test TV Root",
                "path": "/tmp/test-tv",
                "media_type": "tv"
            }),
        )
        .await;
    root_resp.assert_status(StatusCode::CREATED);
    let root_id: String = root_resp.json::<Value>()["id"]
        .as_str()
        .unwrap()
        .to_string();
    let root_uuid: Uuid = root_id.parse().unwrap();

    // Insert an unenriched show (has tmdb_id, no description)
    let show_id = Uuid::now_v7();
    sqlx::query(
        r#"INSERT INTO tv_shows (id, media_root_id, title, year, identifiers)
           VALUES ($1, $2, 'Unenriched Show', 2024, '{"tmdb_id": "11111"}'::jsonb)"#,
    )
    .bind(show_id)
    .bind(root_uuid)
    .execute(&server.pool)
    .await
    .unwrap();

    // Insert an enriched show
    let enriched_id = Uuid::now_v7();
    sqlx::query(
        r#"INSERT INTO tv_shows (id, media_root_id, title, year, description, identifiers)
           VALUES ($1, $2, 'Enriched Show', 2024, 'A great series.', '{"tmdb_id": "22222"}'::jsonb)"#,
    )
    .bind(enriched_id)
    .bind(root_uuid)
    .execute(&server.pool)
    .await
    .unwrap();

    let handler = api::jobs::handlers::MediaRootEnrichHandler::new(
        server.pool.clone(),
        server.nats_publisher.clone(),
        "/tmp".to_string(),
    );

    let result = handler
        .execute(
            Uuid::now_v7(),
            json!({ "media_root_id": root_id }),
            CancellationToken::new(),
        )
        .await;

    assert!(result.is_ok(), "Handler should succeed");
    let val = result.unwrap();
    assert!(
        val["enqueued"].as_u64().unwrap_or(0) >= 1,
        "Should enqueue at least 1 unenriched show, got: {val}"
    );
}

// =============================================================================
// SSE Event Integration
// =============================================================================

#[tokio::test]
async fn test_movie_updated_event_broadcast() {
    let server = TestServer::start_api_only().await;
    let movie_id = Uuid::now_v7();

    let mut rx = server.library_events.subscribe();

    // Broadcast a movie_updated event
    let _ = server
        .library_events
        .send(api::LibraryEvent::movie_updated(movie_id));

    let event = tokio::time::timeout(std::time::Duration::from_millis(500), rx.recv())
        .await
        .expect("Should receive event within timeout")
        .expect("Channel should not be closed");

    assert_eq!(event.kind(), "movie_updated");
}

#[tokio::test]
async fn test_tv_show_updated_event_broadcast() {
    let server = TestServer::start_api_only().await;
    let show_id = Uuid::now_v7();

    let mut rx = server.library_events.subscribe();

    // Broadcast a tv_show_updated event
    let _ = server
        .library_events
        .send(api::LibraryEvent::tv_show_updated(show_id));

    let event = tokio::time::timeout(std::time::Duration::from_millis(500), rx.recv())
        .await
        .expect("Should receive event within timeout")
        .expect("Channel should not be closed");

    assert_eq!(event.kind(), "tv_show_updated");
}
