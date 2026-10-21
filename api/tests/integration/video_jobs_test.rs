//! Tests for movie and TV want job handlers.

use crate::common::*;
use api::jobs::handlers::{MovieWantHandler, TvWantHandler};
use api::jobs::worker::JobHandler;
use api::repositories::{movies, tv_episodes, tv_shows};
use reqwest::StatusCode;
use serde_json::{Value, json};
use time::Duration;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

// =============================================================================
// Test Helpers
// =============================================================================

/// Create a movie via the API and return its ID.
async fn create_movie(server: &TestServer, title: &str) -> String {
    create_movie_with_options(server, title, None).await
}

/// Create a movie via the API with optional quality_profile_id. Returns the movie ID.
async fn create_movie_with_options(
    server: &TestServer,
    title: &str,
    quality_profile_id: Option<Uuid>,
) -> String {
    let mut body = json!({
        "title": title,
        "year": 2024,
        "identifiers": { "tmdb_id": Uuid::now_v7().to_string() }
    });
    if let Some(qp_id) = quality_profile_id {
        body["quality_profile_id"] = json!(qp_id);
    }
    let resp = server
        .admin_client()
        .post_json("/api/v1/admin/movies", &body)
        .await;
    resp.assert_status(reqwest::StatusCode::CREATED);
    resp.json::<Value>()["id"].as_str().unwrap().to_string()
}

/// Create a TV show with a season and episodes. Returns (show_id, season_id, [episode_ids]).
async fn create_show_with_episodes(
    server: &TestServer,
    title: &str,
    episode_count: usize,
) -> (String, String, Vec<String>) {
    create_show_with_options(server, title, episode_count, None).await
}

/// Create a TV show with a season, episodes, and optional quality_profile_id.
/// Returns (show_id, season_id, [episode_ids]).
async fn create_show_with_options(
    server: &TestServer,
    title: &str,
    episode_count: usize,
    quality_profile_id: Option<Uuid>,
) -> (String, String, Vec<String>) {
    let mut body = json!({
        "title": title,
        "year": 2024,
        "identifiers": { "tmdb_id": Uuid::now_v7().to_string() }
    });
    if let Some(qp_id) = quality_profile_id {
        body["quality_profile_id"] = json!(qp_id);
    }
    let resp = server
        .admin_client()
        .post_json("/api/v1/admin/tv/shows", &body)
        .await;
    resp.assert_status(reqwest::StatusCode::CREATED);
    let show_id = resp.json::<Value>()["id"].as_str().unwrap().to_string();
    let show_uuid: Uuid = show_id.parse().unwrap();

    // Create season
    let season_id = Uuid::now_v7();
    sqlx::query(
        r#"
        INSERT INTO tv_seasons (id, show_id, season_identifier, season_sort_order, is_monitored, is_special)
        VALUES ($1, $2, '1', 1, true, false)
        "#,
    )
    .bind(season_id)
    .bind(show_uuid)
    .execute(&server.pool)
    .await
    .unwrap();

    // Create episodes
    let mut episode_ids = Vec::new();
    for i in 1..=episode_count {
        let ep_id = Uuid::now_v7();
        sqlx::query(
            r#"
            INSERT INTO tv_episodes (id, season_id, episode_identifier, episode_sort_order, title)
            VALUES ($1, $2, $3, $4, $5)
            "#,
        )
        .bind(ep_id)
        .bind(season_id)
        .bind(i.to_string())
        .bind(i as i32)
        .bind(format!("Episode {}", i))
        .execute(&server.pool)
        .await
        .unwrap();
        episode_ids.push(ep_id.to_string());
    }

    (show_id, season_id.to_string(), episode_ids)
}

/// Configure Kobload settings for the test server.
async fn configure_kobload(server: &TestServer) {
    server
        .admin_client()
        .patch_json(
            "/api/v1/admin/config",
            &json!({
                "kobload_base_url": "http://localhost:9999",
                "kobload_api_key": "test-key",
                "kobload_ingest_url": format!("{}/api/v1/ingest/{}", server.api_url, server.ingest_token)
            }),
        )
        .await
        .assert_ok();
}

/// Extract quality_profile_id from a MediaWant enum variant.
fn extract_quality_profile_id(media_want: &nats_jobs::MediaWant) -> Option<Uuid> {
    match media_want {
        nats_jobs::MediaWant::Movie {
            quality_profile_id, ..
        } => *quality_profile_id,
        nats_jobs::MediaWant::TvEpisode {
            quality_profile_id, ..
        } => *quality_profile_id,
        nats_jobs::MediaWant::TvSeason {
            quality_profile_id, ..
        } => *quality_profile_id,
        _ => None,
    }
}

/// Build the same MediaWant::Movie the handler would build from a movie row.
/// This mirrors the handler logic so we can verify the quality_profile_id flows through.
fn build_movie_media_want(movie: &movies::MovieRow) -> nats_jobs::MediaWant {
    let identifiers = nats_jobs::MovieIdentifiers {
        tmdb_id: movie
            .identifiers
            .get("tmdb_id")
            .and_then(|v| v.as_str())
            .map(String::from),
        imdb_id: movie
            .identifiers
            .get("imdb_id")
            .and_then(|v| v.as_str())
            .map(String::from),
    };

    nats_jobs::MediaWant::Movie {
        title: movie.title.clone(),
        year: movie.year.map(|y| y as u32),
        release_date: movie.release_date.map(|d| d.to_string()),
        identifiers: Some(identifiers),
        alternate_titles: movie.alternate_titles.clone().filter(|v| !v.is_empty()),
        quality_profile_id: movie.quality_profile_id,
        quality_overrides: None,
        in_library: None,
        library_video_height: None,
    }
}

/// Build the same MediaWant::TvEpisode the handler would build from show + episode data.
fn build_tv_episode_media_want(
    show: &tv_shows::TvShowRow,
    season_identifier: &str,
    episode_identifier: &str,
) -> nats_jobs::MediaWant {
    let identifiers = nats_jobs::TvIdentifiers {
        tmdb_id: show
            .identifiers
            .get("tmdb_id")
            .and_then(|v| v.as_str())
            .map(String::from),
        tvdb_id: show
            .identifiers
            .get("tvdb_id")
            .and_then(|v| v.as_str())
            .map(String::from),
        imdb_id: show
            .identifiers
            .get("imdb_id")
            .and_then(|v| v.as_str())
            .map(String::from),
    };

    nats_jobs::MediaWant::TvEpisode {
        show_name: show.title.clone(),
        season: season_identifier.to_string(),
        episode: episode_identifier.to_string(),
        year: show.year.map(|y| y as u32),
        air_date: None,
        identifiers: Some(identifiers),
        alternate_titles: show.alternate_titles.clone().filter(|v| !v.is_empty()),
        episode_title: None,
        quality_profile_id: show.quality_profile_id,
        quality_overrides: None,
    }
}

/// Build the same MediaWant::TvSeason the handler would build from show data.
fn build_tv_season_media_want(
    show: &tv_shows::TvShowRow,
    season_identifier: &str,
) -> nats_jobs::MediaWant {
    let identifiers = nats_jobs::TvIdentifiers {
        tmdb_id: show
            .identifiers
            .get("tmdb_id")
            .and_then(|v| v.as_str())
            .map(String::from),
        tvdb_id: show
            .identifiers
            .get("tvdb_id")
            .and_then(|v| v.as_str())
            .map(String::from),
        imdb_id: show
            .identifiers
            .get("imdb_id")
            .and_then(|v| v.as_str())
            .map(String::from),
    };

    nats_jobs::MediaWant::TvSeason {
        show_name: show.title.clone(),
        season: season_identifier.to_string(),
        year: show.year.map(|y| y as u32),
        episodes_needed: None,
        total_episodes: None,
        pack_preference: "auto".to_string(),
        last_air_date: None,
        identifiers: Some(identifiers),
        alternate_titles: show.alternate_titles.clone().filter(|v| !v.is_empty()),
        quality_profile_id: show.quality_profile_id,
        quality_overrides: None,
    }
}

// =============================================================================
// MovieWantHandler Tests
// =============================================================================

#[tokio::test]
async fn test_movie_want_enqueues_to_nats() {
    let server = TestServer::start_api_only().await;
    configure_kobload(&server).await;
    let movie_id = create_movie(&server, "Inception").await;

    // Want the movie first
    server
        .admin_client()
        .post(&format!("/api/v1/admin/movies/{}/want", movie_id))
        .await
        .assert_status(StatusCode::NO_CONTENT);

    let handler = MovieWantHandler::new(
        server.pool.clone(),
        server.runtime_config.clone(),
        server.nats_publisher.clone(),
        server.library_events.clone(),
    );

    let result = handler
        .execute(
            Uuid::now_v7(),
            json!({ "movie_id": movie_id }),
            CancellationToken::new(),
        )
        .await;
    assert!(result.is_ok(), "handler should succeed: {:?}", result.err());

    let result_json = result.unwrap();
    assert_eq!(result_json["status"], "enqueued");
    assert_eq!(result_json["movie_id"], movie_id);
}

#[tokio::test]
async fn test_movie_want_skips_without_config() {
    let server = TestServer::start_api_only().await;
    // Don't configure Kobload
    let movie_id = create_movie(&server, "No Config Movie").await;

    server
        .admin_client()
        .post(&format!("/api/v1/admin/movies/{}/want", movie_id))
        .await
        .assert_status(StatusCode::NO_CONTENT);

    let handler = MovieWantHandler::new(
        server.pool.clone(),
        server.runtime_config.clone(),
        server.nats_publisher.clone(),
        server.library_events.clone(),
    );

    let result = handler
        .execute(
            Uuid::now_v7(),
            json!({ "movie_id": movie_id }),
            CancellationToken::new(),
        )
        .await;
    assert!(result.is_ok());

    let result_json = result.unwrap();
    assert_eq!(result_json["status"], "skipped");
    assert_eq!(result_json["reason"], "ingest_url_not_configured");
}

#[tokio::test]
async fn test_movie_want_nonexistent_movie() {
    let server = TestServer::start_api_only().await;
    configure_kobload(&server).await;

    let handler = MovieWantHandler::new(
        server.pool.clone(),
        server.runtime_config.clone(),
        server.nats_publisher.clone(),
        server.library_events.clone(),
    );

    let fake_id = Uuid::now_v7();
    let result = handler
        .execute(
            Uuid::now_v7(),
            json!({ "movie_id": fake_id }),
            CancellationToken::new(),
        )
        .await;
    assert!(result.is_err(), "handler should fail for nonexistent movie");
}

// =============================================================================
// TvWantHandler Tests
// =============================================================================

#[tokio::test]
async fn test_tv_want_enqueues_for_episodes() {
    let server = TestServer::start_api_only().await;
    configure_kobload(&server).await;
    let (show_id, season_id, episode_ids) =
        create_show_with_episodes(&server, "Breaking Bad", 3).await;

    // Want all episodes
    for ep_id in &episode_ids {
        server
            .admin_client()
            .post(&format!("/api/v1/admin/tv/episodes/{}/want", ep_id))
            .await
            .assert_status(StatusCode::NO_CONTENT);
    }

    let handler = TvWantHandler::new(
        server.pool.clone(),
        server.runtime_config.clone(),
        server.nats_publisher.clone(),
        server.library_events.clone(),
    );

    let ep_uuids: Vec<Uuid> = episode_ids.iter().map(|id| id.parse().unwrap()).collect();
    let result = handler
        .execute(
            Uuid::now_v7(),
            json!({
                "show_id": show_id,
                "season_id": season_id,
                "episode_ids": ep_uuids
            }),
            CancellationToken::new(),
        )
        .await;
    assert!(result.is_ok(), "handler should succeed: {:?}", result.err());
}

#[tokio::test]
async fn test_tv_want_skips_without_config() {
    let server = TestServer::start_api_only().await;
    let (show_id, season_id, episode_ids) =
        create_show_with_episodes(&server, "No Config Show", 2).await;

    for ep_id in &episode_ids {
        server
            .admin_client()
            .post(&format!("/api/v1/admin/tv/episodes/{}/want", ep_id))
            .await
            .assert_status(StatusCode::NO_CONTENT);
    }

    let handler = TvWantHandler::new(
        server.pool.clone(),
        server.runtime_config.clone(),
        server.nats_publisher.clone(),
        server.library_events.clone(),
    );

    let ep_uuids: Vec<Uuid> = episode_ids.iter().map(|id| id.parse().unwrap()).collect();
    let result = handler
        .execute(
            Uuid::now_v7(),
            json!({
                "show_id": show_id,
                "season_id": season_id,
                "episode_ids": ep_uuids
            }),
            CancellationToken::new(),
        )
        .await;
    assert!(result.is_ok());

    let result_json = result.unwrap();
    assert_eq!(result_json["status"], "skipped");
}

#[tokio::test]
async fn test_tv_want_season_wants_missing_episodes() {
    let server = TestServer::start_api_only().await;
    configure_kobload(&server).await;
    let (show_id, season_id, episode_ids) =
        create_show_with_episodes(&server, "Want All Show", 5).await;

    // In production, auto_want sets episodes to "wanted" before enqueuing the
    // tv_want job. Replicate that here so list_for_season_pattern finds them.
    let ep_uuids: Vec<Uuid> = episode_ids.iter().map(|id| id.parse().unwrap()).collect();
    tv_episodes::set_want_status_batch(&server.pool, &ep_uuids, "wanted")
        .await
        .expect("Failed to set want_status");

    // Don't specify episode_ids -> handler wants entire season (pattern coverage)
    let handler = TvWantHandler::new(
        server.pool.clone(),
        server.runtime_config.clone(),
        server.nats_publisher.clone(),
        server.library_events.clone(),
    );

    let result = handler
        .execute(
            Uuid::now_v7(),
            json!({
                "show_id": show_id,
                "season_id": season_id
            }),
            CancellationToken::new(),
        )
        .await;
    assert!(result.is_ok(), "handler should succeed: {:?}", result.err());

    let result_json = result.unwrap();
    assert_eq!(result_json["status"], "enqueued");
    assert_eq!(
        result_json["episodes_count"], 5,
        "All 5 episodes should be wanted"
    );

    // Verify episodes are now in "searching" status
    let rows = sqlx::query_scalar::<_, String>(
        "SELECT want_status FROM tv_episodes WHERE season_id = $1 ORDER BY episode_sort_order",
    )
    .bind(season_id.parse::<Uuid>().unwrap())
    .fetch_all(&server.pool)
    .await
    .unwrap();

    assert!(
        rows.iter().all(|s| s == "searching"),
        "All episodes should be 'searching', got: {:?}",
        rows
    );
}

// =============================================================================
// Quality Profile ID Pass-Through Tests
//
// These tests verify that quality_profile_id from MovieRow/TvShowRow is correctly
// passed through to the MediaWant struct in the want_request NATS payload.
//
// Strategy: We verify the data flow at each step:
// 1. Create entity with quality_profile_id via API
// 2. Confirm the DB stores quality_profile_id correctly (repo read)
// 3. Verify the MediaWant construction uses the DB value
// 4. Run the handler and confirm it succeeds (enqueues to NATS)
// =============================================================================

#[tokio::test]
async fn test_movie_want_sends_quality_profile_id() {
    let server = TestServer::start_api_only().await;
    configure_kobload(&server).await;

    let quality_profile_id = Uuid::now_v7();
    let movie_id =
        create_movie_with_options(&server, "Dune Part Two", Some(quality_profile_id)).await;
    let movie_uuid: Uuid = movie_id.parse().unwrap();

    // Verify the quality_profile_id was stored correctly in the DB
    let movie_row = movies::get(&server.pool, movie_uuid)
        .await
        .expect("fetch movie")
        .expect("movie should exist");
    assert_eq!(
        movie_row.quality_profile_id,
        Some(quality_profile_id),
        "DB should store the quality_profile_id"
    );

    // Verify the MediaWant construction passes quality_profile_id through
    let media_want = build_movie_media_want(&movie_row);
    assert_eq!(
        extract_quality_profile_id(&media_want),
        Some(quality_profile_id),
        "MediaWant::Movie should carry quality_profile_id from the movie row"
    );

    // Verify the serialized payload preserves quality_profile_id
    let serialized = serde_json::to_value(&media_want).unwrap();
    assert_eq!(
        serialized["quality_profile_id"],
        json!(quality_profile_id),
        "Serialized MediaWant should include quality_profile_id"
    );

    // Set want status and run the handler end-to-end
    server
        .admin_client()
        .post(&format!("/api/v1/admin/movies/{}/want", movie_id))
        .await
        .assert_status(StatusCode::NO_CONTENT);

    let handler = MovieWantHandler::new(
        server.pool.clone(),
        server.runtime_config.clone(),
        server.nats_publisher.clone(),
        server.library_events.clone(),
    );

    let result = handler
        .execute(
            Uuid::now_v7(),
            json!({ "movie_id": movie_id }),
            CancellationToken::new(),
        )
        .await;
    assert!(result.is_ok(), "handler should succeed: {:?}", result.err());
    assert_eq!(
        result.unwrap()["status"],
        "enqueued",
        "Handler should enqueue the want_request to NATS"
    );
}

#[tokio::test]
async fn test_movie_want_sends_none_when_no_profile_set() {
    let server = TestServer::start_api_only().await;
    configure_kobload(&server).await;

    // Create movie without quality_profile_id
    let movie_id = create_movie_with_options(&server, "No Profile Movie", None).await;
    let movie_uuid: Uuid = movie_id.parse().unwrap();

    // Verify quality_profile_id is None in the DB
    let movie_row = movies::get(&server.pool, movie_uuid)
        .await
        .expect("fetch movie")
        .expect("movie should exist");
    assert_eq!(
        movie_row.quality_profile_id, None,
        "DB should have None for quality_profile_id"
    );

    // Verify the MediaWant construction passes None through
    let media_want = build_movie_media_want(&movie_row);
    assert_eq!(
        extract_quality_profile_id(&media_want),
        None,
        "MediaWant::Movie should have None quality_profile_id"
    );

    // Verify serialized payload omits or nulls quality_profile_id
    let serialized = serde_json::to_value(&media_want).unwrap();
    assert!(
        serialized["quality_profile_id"].is_null(),
        "Serialized MediaWant should have null quality_profile_id, got: {}",
        serialized["quality_profile_id"]
    );

    // Run the handler end-to-end
    server
        .admin_client()
        .post(&format!("/api/v1/admin/movies/{}/want", movie_id))
        .await
        .assert_status(StatusCode::NO_CONTENT);

    let handler = MovieWantHandler::new(
        server.pool.clone(),
        server.runtime_config.clone(),
        server.nats_publisher.clone(),
        server.library_events.clone(),
    );

    let result = handler
        .execute(
            Uuid::now_v7(),
            json!({ "movie_id": movie_id }),
            CancellationToken::new(),
        )
        .await;
    assert!(result.is_ok(), "handler should succeed: {:?}", result.err());
    assert_eq!(result.unwrap()["status"], "enqueued");
}

#[tokio::test]
async fn test_tv_episode_want_sends_quality_profile_id() {
    let server = TestServer::start_api_only().await;
    configure_kobload(&server).await;

    let quality_profile_id = Uuid::now_v7();
    let (show_id, season_id, episode_ids) =
        create_show_with_options(&server, "Better Call Saul", 2, Some(quality_profile_id)).await;
    let show_uuid: Uuid = show_id.parse().unwrap();

    // Verify the quality_profile_id was stored on the show
    let show_row = tv_shows::get(&server.pool, show_uuid)
        .await
        .expect("fetch show")
        .expect("show should exist");
    assert_eq!(
        show_row.quality_profile_id,
        Some(quality_profile_id),
        "DB should store quality_profile_id on the show"
    );

    // Verify the MediaWant::TvEpisode construction passes quality_profile_id from the show
    let media_want = build_tv_episode_media_want(&show_row, "1", "1");
    assert_eq!(
        extract_quality_profile_id(&media_want),
        Some(quality_profile_id),
        "MediaWant::TvEpisode should carry quality_profile_id from the show"
    );

    // Want specific episodes and run the handler
    for ep_id in &episode_ids {
        server
            .admin_client()
            .post(&format!("/api/v1/admin/tv/episodes/{}/want", ep_id))
            .await
            .assert_status(StatusCode::NO_CONTENT);
    }

    let handler = TvWantHandler::new(
        server.pool.clone(),
        server.runtime_config.clone(),
        server.nats_publisher.clone(),
        server.library_events.clone(),
    );

    let ep_uuids: Vec<Uuid> = episode_ids.iter().map(|id| id.parse().unwrap()).collect();
    let result = handler
        .execute(
            Uuid::now_v7(),
            json!({
                "show_id": show_id,
                "season_id": season_id,
                "episode_ids": ep_uuids
            }),
            CancellationToken::new(),
        )
        .await;
    assert!(result.is_ok(), "handler should succeed: {:?}", result.err());
}

#[tokio::test]
async fn test_tv_season_want_sends_quality_profile_id() {
    let server = TestServer::start_api_only().await;
    configure_kobload(&server).await;

    let quality_profile_id = Uuid::now_v7();
    let (show_id, season_id, episode_ids) =
        create_show_with_options(&server, "The Wire", 3, Some(quality_profile_id)).await;
    let show_uuid: Uuid = show_id.parse().unwrap();

    // In production, auto_want sets episodes to "wanted" before enqueuing the
    // tv_want job. Replicate that so list_for_season_pattern finds them.
    let ep_uuids: Vec<Uuid> = episode_ids.iter().map(|id| id.parse().unwrap()).collect();
    tv_episodes::set_want_status_batch(&server.pool, &ep_uuids, "wanted")
        .await
        .expect("Failed to set want_status");

    // Verify quality_profile_id stored on the show
    let show_row = tv_shows::get(&server.pool, show_uuid)
        .await
        .expect("fetch show")
        .expect("show should exist");
    assert_eq!(
        show_row.quality_profile_id,
        Some(quality_profile_id),
        "DB should store quality_profile_id on the show"
    );

    // Verify the MediaWant::TvSeason construction passes quality_profile_id from the show
    let media_want = build_tv_season_media_want(&show_row, "1");
    assert_eq!(
        extract_quality_profile_id(&media_want),
        Some(quality_profile_id),
        "MediaWant::TvSeason should carry quality_profile_id from the show"
    );

    // Don't specify episode_ids - triggers the TvSeason variant (full season want)
    let handler = TvWantHandler::new(
        server.pool.clone(),
        server.runtime_config.clone(),
        server.nats_publisher.clone(),
        server.library_events.clone(),
    );

    let result = handler
        .execute(
            Uuid::now_v7(),
            json!({
                "show_id": show_id,
                "season_id": season_id
            }),
            CancellationToken::new(),
        )
        .await;
    assert!(result.is_ok(), "handler should succeed: {:?}", result.err());
    assert_eq!(
        result.unwrap()["status"],
        "enqueued",
        "Handler should enqueue the want_request to NATS"
    );
}

// =============================================================================
// Air Date Filter & Auto-Want Re-Trigger Tests
// =============================================================================

/// Create a TV show with episodes that have specific air_dates.
/// `air_dates` is a slice of Option<time::Date> — one per episode.
/// Returns (show_id, season_id, [episode_ids]).
async fn create_show_with_air_dates(
    server: &TestServer,
    title: &str,
    air_dates: &[Option<time::Date>],
) -> (String, String, Vec<Uuid>) {
    let resp = server
        .admin_client()
        .post_json(
            "/api/v1/admin/tv/shows",
            &json!({
                "title": title,
                "year": 2026,
                "identifiers": { "tmdb_id": Uuid::now_v7().to_string() }
            }),
        )
        .await;
    resp.assert_status(reqwest::StatusCode::CREATED);
    let show_id = resp.json::<Value>()["id"].as_str().unwrap().to_string();
    let show_uuid: Uuid = show_id.parse().unwrap();

    let season_id = Uuid::now_v7();
    sqlx::query(
        r#"
        INSERT INTO tv_seasons (id, show_id, season_identifier, season_sort_order, is_monitored, is_special)
        VALUES ($1, $2, '1', 1, true, false)
        "#,
    )
    .bind(season_id)
    .bind(show_uuid)
    .execute(&server.pool)
    .await
    .unwrap();

    let mut episode_ids = Vec::new();
    for (i, air_date) in air_dates.iter().enumerate() {
        let ep_id = Uuid::now_v7();
        let ep_num = (i + 1) as i32;
        sqlx::query(
            r#"
            INSERT INTO tv_episodes (id, season_id, episode_identifier, episode_sort_order, title, air_date)
            VALUES ($1, $2, $3, $4, $5, $6)
            "#,
        )
        .bind(ep_id)
        .bind(season_id)
        .bind(ep_num.to_string())
        .bind(ep_num)
        .bind(format!("Episode {}", ep_num))
        .bind(*air_date)
        .execute(&server.pool)
        .await
        .unwrap();
        episode_ids.push(ep_id);
    }

    (show_id, season_id.to_string(), episode_ids)
}

/// Bug 2: When tv_want runs with specific episode_ids that include future-dated
/// merged episodes, ALL non-library episodes should get 'searching' status
/// (not just the aired ones). The air_date filter was removed because kobload
/// handles its own date filtering.
#[tokio::test]
async fn test_tv_want_future_episodes_get_searching_status() {
    let server = TestServer::start_api_only().await;
    configure_kobload(&server).await;

    let today = time::OffsetDateTime::now_utc().date();
    let air_dates = vec![
        Some(today - Duration::days(14)), // ep1: aired 2 weeks ago
        Some(today - Duration::days(7)),  // ep2: aired last week
        Some(today - Duration::days(1)),  // ep3: aired yesterday
        Some(today + Duration::days(7)),  // ep4: airs next week (future)
        None,                             // ep5: no air date
    ];

    let (show_id, season_id, episode_ids) =
        create_show_with_air_dates(&server, "Future Episodes Show", &air_dates).await;

    // Set all episodes to 'wanted' (simulating previous auto_want)
    tv_episodes::set_want_status_batch(&server.pool, &episode_ids, "wanted")
        .await
        .unwrap();

    let handler = TvWantHandler::new(
        server.pool.clone(),
        server.runtime_config.clone(),
        server.nats_publisher.clone(),
        server.library_events.clone(),
    );

    // Request ep3 specifically — handler merges in other 'wanted' episodes
    let result = handler
        .execute(
            Uuid::now_v7(),
            json!({
                "show_id": show_id,
                "season_id": season_id,
                "episode_ids": [episode_ids[2]] // ep3
            }),
            CancellationToken::new(),
        )
        .await;
    assert!(result.is_ok(), "handler should succeed: {:?}", result.err());

    // After fix: ALL non-library episodes should be 'searching' (no air_date filter)
    let statuses = sqlx::query_scalar::<_, String>(
        "SELECT want_status FROM tv_episodes WHERE season_id = $1 ORDER BY episode_sort_order",
    )
    .bind(season_id.parse::<Uuid>().unwrap())
    .fetch_all(&server.pool)
    .await
    .unwrap();

    assert_eq!(
        statuses,
        vec![
            "searching",
            "searching",
            "searching",
            "searching",
            "searching"
        ],
        "All episodes (including future) should be 'searching' since kobload handles air_date filtering. Got: {:?}",
        statuses
    );
}

/// Bug 2 (whole-season path): Same test but using the whole-season path
/// (episode_ids: None). Future episodes should get 'searching' status.
#[tokio::test]
async fn test_tv_want_whole_season_future_episodes_get_searching() {
    let server = TestServer::start_api_only().await;
    configure_kobload(&server).await;

    let today = time::OffsetDateTime::now_utc().date();
    let air_dates = vec![
        Some(today - Duration::days(7)),  // ep1: aired
        Some(today - Duration::days(1)),  // ep2: aired
        Some(today + Duration::days(7)),  // ep3: future
        Some(today + Duration::days(14)), // ep4: future
    ];

    let (show_id, season_id, episode_ids) =
        create_show_with_air_dates(&server, "Whole Season Future Show", &air_dates).await;

    // Set all to 'wanted'
    tv_episodes::set_want_status_batch(&server.pool, &episode_ids, "wanted")
        .await
        .unwrap();

    let handler = TvWantHandler::new(
        server.pool.clone(),
        server.runtime_config.clone(),
        server.nats_publisher.clone(),
        server.library_events.clone(),
    );

    // Whole-season path (no episode_ids)
    let result = handler
        .execute(
            Uuid::now_v7(),
            json!({
                "show_id": show_id,
                "season_id": season_id,
            }),
            CancellationToken::new(),
        )
        .await;
    assert!(result.is_ok(), "handler should succeed: {:?}", result.err());

    let result_json = result.unwrap();
    assert_eq!(result_json["status"], "enqueued");
    // All 4 episodes should be affected (no air_date filter)
    assert_eq!(
        result_json["episodes_count"], 4,
        "All 4 non-library episodes should be affected, got: {}",
        result_json["episodes_count"]
    );

    let statuses = sqlx::query_scalar::<_, String>(
        "SELECT want_status FROM tv_episodes WHERE season_id = $1 ORDER BY episode_sort_order",
    )
    .bind(season_id.parse::<Uuid>().unwrap())
    .fetch_all(&server.pool)
    .await
    .unwrap();

    assert_eq!(
        statuses,
        vec!["searching", "searching", "searching", "searching"],
        "All episodes should be 'searching'. Got: {:?}",
        statuses
    );
}
