//! Tests for database cleanup orphan deletion queries.

use crate::common::*;
use api::repositories::{movies, tv_episodes, tv_seasons, tv_shows};
use serde_json::{Value, json};
use uuid::Uuid;

// =============================================================================
// Test Helpers
// =============================================================================

/// Create a movie via the API, set want_status='none', backdate updated_at.
/// Returns the movie UUID.
async fn create_unwanted_movie(server: &TestServer, title: &str) -> Uuid {
    let resp = server
        .admin_client()
        .post_json(
            "/api/v1/admin/movies",
            &json!({
                "title": title,
                "year": 2024,
                "identifiers": { "tmdb_id": Uuid::now_v7().to_string() }
            }),
        )
        .await;
    resp.assert_status(reqwest::StatusCode::CREATED);
    let id: Uuid = resp.json::<Value>()["id"]
        .as_str()
        .unwrap()
        .parse()
        .unwrap();

    // Set want_status='none' and backdate past 30-minute grace period
    sqlx::query(
        "UPDATE movies SET want_status = 'none', updated_at = NOW() - INTERVAL '1 hour' WHERE id = $1",
    )
    .bind(id)
    .execute(&server.pool)
    .await
    .unwrap();

    id
}

/// Create a TV show/season/episode chain. Sets show auto_want_mode='none',
/// season auto_want_mode='none', episode want_status='none', backdates
/// episode updated_at, and sets season is_monitored=false / show is_monitored=false.
/// Returns (show_id, season_id, episode_id).
async fn create_unwanted_tv_chain(server: &TestServer, title: &str) -> (Uuid, Uuid, Uuid) {
    let resp = server
        .admin_client()
        .post_json(
            "/api/v1/admin/tv/shows",
            &json!({
                "title": title,
                "year": 2024,
                "identifiers": { "tmdb_id": Uuid::now_v7().to_string() }
            }),
        )
        .await;
    resp.assert_status(reqwest::StatusCode::CREATED);
    let show_id: Uuid = resp.json::<Value>()["id"]
        .as_str()
        .unwrap()
        .parse()
        .unwrap();

    let season_id = Uuid::now_v7();
    sqlx::query(
        r#"
        INSERT INTO tv_seasons (id, show_id, season_identifier, season_sort_order, is_monitored, is_special, auto_want_mode)
        VALUES ($1, $2, '1', 1, false, false, 'none')
        "#,
    )
    .bind(season_id)
    .bind(show_id)
    .execute(&server.pool)
    .await
    .unwrap();

    let episode_id = Uuid::now_v7();
    sqlx::query(
        r#"
        INSERT INTO tv_episodes (id, season_id, episode_identifier, episode_sort_order, title, want_status, updated_at)
        VALUES ($1, $2, '1', 1, 'Episode 1', 'none', NOW() - INTERVAL '1 hour')
        "#,
    )
    .bind(episode_id)
    .bind(season_id)
    .execute(&server.pool)
    .await
    .unwrap();

    // Set show to unwanted and unmonitored
    sqlx::query("UPDATE tv_shows SET auto_want_mode = 'none', is_monitored = false WHERE id = $1")
        .bind(show_id)
        .execute(&server.pool)
        .await
        .unwrap();

    (show_id, season_id, episode_id)
}

/// Assert a movie exists in the database.
async fn movie_exists(pool: &sqlx::PgPool, id: Uuid) -> bool {
    sqlx::query_scalar::<_, bool>("SELECT EXISTS(SELECT 1 FROM movies WHERE id = $1)")
        .bind(id)
        .fetch_one(pool)
        .await
        .unwrap()
}

/// Assert a TV show exists in the database.
async fn show_exists(pool: &sqlx::PgPool, id: Uuid) -> bool {
    sqlx::query_scalar::<_, bool>("SELECT EXISTS(SELECT 1 FROM tv_shows WHERE id = $1)")
        .bind(id)
        .fetch_one(pool)
        .await
        .unwrap()
}

/// Assert a TV season exists in the database.
async fn season_exists(pool: &sqlx::PgPool, id: Uuid) -> bool {
    sqlx::query_scalar::<_, bool>("SELECT EXISTS(SELECT 1 FROM tv_seasons WHERE id = $1)")
        .bind(id)
        .fetch_one(pool)
        .await
        .unwrap()
}

/// Assert a TV episode exists in the database.
async fn episode_exists(pool: &sqlx::PgPool, id: Uuid) -> bool {
    sqlx::query_scalar::<_, bool>("SELECT EXISTS(SELECT 1 FROM tv_episodes WHERE id = $1)")
        .bind(id)
        .fetch_one(pool)
        .await
        .unwrap()
}

// =============================================================================
// Tests
// =============================================================================

/// Movie with all files unavailable and want_status='none' should be deleted.
#[tokio::test]
async fn test_movie_unavailable_files_deleted() {
    let server = TestServer::start_api_only().await;
    let movie_id = create_unwanted_movie(&server, "Unavailable Movie").await;

    // Create a movie file and mark it unavailable
    let file_id = create_stub_movie_file(&server.pool, movie_id).await;
    sqlx::query("UPDATE movie_files SET available = false WHERE id = $1")
        .bind(file_id)
        .execute(&server.pool)
        .await
        .unwrap();

    let deleted = movies::delete_orphaned_movies(&server.pool).await.unwrap();
    assert!(
        deleted.contains(&movie_id),
        "Movie with only unavailable files should be deleted"
    );
    assert!(!movie_exists(&server.pool, movie_id).await);
}

/// Movie with at least one available file should NOT be deleted.
#[tokio::test]
async fn test_movie_available_files_kept() {
    let server = TestServer::start_api_only().await;
    let movie_id = create_unwanted_movie(&server, "Available Movie").await;

    // Create a movie file that is available (default)
    create_stub_movie_file(&server.pool, movie_id).await;

    let deleted = movies::delete_orphaned_movies(&server.pool).await.unwrap();
    assert!(
        !deleted.contains(&movie_id),
        "Movie with available files should NOT be deleted"
    );
    assert!(movie_exists(&server.pool, movie_id).await);
}

/// TV show/season/episode chain with unavailable files and unwanted status
/// should cascade-delete: episodes first, then seasons, then shows.
#[tokio::test]
async fn test_tv_cascade_cleanup_unavailable_files() {
    let server = TestServer::start_api_only().await;
    let (show_id, season_id, episode_id) =
        create_unwanted_tv_chain(&server, "Unavailable Show").await;

    // Create an episode file and mark it unavailable
    let file_id = create_stub_episode_file(&server.pool, episode_id).await;
    sqlx::query("UPDATE tv_episode_files SET available = false WHERE id = $1")
        .bind(file_id)
        .execute(&server.pool)
        .await
        .unwrap();

    // Phase 1: delete orphaned episodes
    let deleted_eps = tv_episodes::delete_orphaned_episodes(&server.pool)
        .await
        .unwrap();
    assert!(
        deleted_eps.contains(&episode_id),
        "Episode with only unavailable files should be deleted"
    );
    assert!(!episode_exists(&server.pool, episode_id).await);

    // Phase 2: delete orphaned seasons (no episodes remain)
    let deleted_seasons = tv_seasons::delete_orphaned(&server.pool).await.unwrap();
    assert!(
        deleted_seasons.contains(&season_id),
        "Season with no episodes and auto_want_mode='none' should be deleted"
    );
    assert!(!season_exists(&server.pool, season_id).await);

    // Phase 3: delete orphaned shows (no seasons remain)
    let deleted_shows = tv_shows::delete_orphaned(&server.pool).await.unwrap();
    assert!(
        deleted_shows.contains(&show_id),
        "Show with no seasons and auto_want_mode='none' should be deleted"
    );
    assert!(!show_exists(&server.pool, show_id).await);
}

/// Movie with streamed_at set should NOT be deleted even with no files.
#[tokio::test]
async fn test_debrid_movie_streamed_at_protected() {
    let server = TestServer::start_api_only().await;
    let movie_id = create_unwanted_movie(&server, "Streamed Movie").await;

    // Set streamed_at (debrid streaming happened)
    sqlx::query("UPDATE movies SET streamed_at = NOW() WHERE id = $1")
        .bind(movie_id)
        .execute(&server.pool)
        .await
        .unwrap();

    let deleted = movies::delete_orphaned_movies(&server.pool).await.unwrap();
    assert!(
        !deleted.contains(&movie_id),
        "Movie with streamed_at set should NOT be deleted"
    );
    assert!(movie_exists(&server.pool, movie_id).await);
}

/// Episode with a debrid_stream_sessions row should NOT be deleted even with no files.
#[tokio::test]
async fn test_debrid_episode_stream_session_protected() {
    let server = TestServer::start_api_only().await;
    let (_, _, episode_id) = create_unwanted_tv_chain(&server, "Debrid Episode Show").await;

    // Create a user for the debrid session FK
    let user_id = create_user(&server, "debrid@test.com").await;

    // Insert a debrid stream session for this episode
    sqlx::query(
        r#"
        INSERT INTO debrid_stream_sessions (id, user_id, media_type, media_id, release_title, debrid_provider, filename, filesize)
        VALUES ($1, $2, 'episode', $3, 'Test.S01E01.720p', 'realdebrid', 'test.mkv', 1024)
        "#,
    )
    .bind(Uuid::now_v7())
    .bind(user_id)
    .bind(episode_id)
    .execute(&server.pool)
    .await
    .unwrap();

    let deleted = tv_episodes::delete_orphaned_episodes(&server.pool)
        .await
        .unwrap();
    assert!(
        !deleted.contains(&episode_id),
        "Episode with debrid stream session should NOT be deleted"
    );
    assert!(episode_exists(&server.pool, episode_id).await);
}

/// Show with streamed_at set should NOT be deleted even with no seasons.
#[tokio::test]
async fn test_debrid_show_streamed_at_protected() {
    let server = TestServer::start_api_only().await;
    let resp = server
        .admin_client()
        .post_json(
            "/api/v1/admin/tv/shows",
            &json!({
                "title": "Streamed Show",
                "year": 2024,
                "identifiers": { "tmdb_id": Uuid::now_v7().to_string() }
            }),
        )
        .await;
    resp.assert_status(reqwest::StatusCode::CREATED);
    let show_id: Uuid = resp.json::<Value>()["id"]
        .as_str()
        .unwrap()
        .parse()
        .unwrap();

    // Set to unwanted/unmonitored but with streamed_at
    sqlx::query(
        "UPDATE tv_shows SET auto_want_mode = 'none', is_monitored = false, streamed_at = NOW() WHERE id = $1",
    )
    .bind(show_id)
    .execute(&server.pool)
    .await
    .unwrap();

    // Delete any auto-created seasons so the show would otherwise be orphaned
    sqlx::query("DELETE FROM tv_seasons WHERE show_id = $1")
        .bind(show_id)
        .execute(&server.pool)
        .await
        .unwrap();

    let deleted = tv_shows::delete_orphaned(&server.pool).await.unwrap();
    assert!(
        !deleted.contains(&show_id),
        "Show with streamed_at set should NOT be deleted"
    );
    assert!(show_exists(&server.pool, show_id).await);
}
