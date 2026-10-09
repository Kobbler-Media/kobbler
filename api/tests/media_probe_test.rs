use crate::common::*;
use uuid::Uuid;

/// Insert a movie file with no video_codec (simulates a newly discovered file).
/// Also creates the required media_roots + media_root_files rows so the JOIN
/// in list_needing_probe/list_needing_extract finds the file.
async fn insert_unprobed_movie_file(server: &TestServer) -> Uuid {
    let movie_id = Uuid::now_v7();
    sqlx::query("INSERT INTO movies (id, title, year) VALUES ($1, 'Test Movie', 2024)")
        .bind(movie_id)
        .execute(&server.pool)
        .await
        .unwrap();

    let file_path = format!("/test/movie_{}.avi", Uuid::now_v7());

    let root_id = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO media_roots (id, path, media_type) VALUES ($1, $2, 'movie') \
         ON CONFLICT (path) DO UPDATE SET id = media_roots.id RETURNING id",
    )
    .bind(root_id)
    .bind("/test")
    .execute(&server.pool)
    .await
    .unwrap();

    let file_id = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO movie_files (id, movie_id, file_path, format, available) \
         VALUES ($1, $2, $3, 'AVI', true)",
    )
    .bind(file_id)
    .bind(movie_id)
    .bind(&file_path)
    .execute(&server.pool)
    .await
    .unwrap();

    sqlx::query(
        "INSERT INTO media_root_files (id, media_root_id, file_path, file_name, available) \
         VALUES ($1, $2, $3, 'fake.avi', true)",
    )
    .bind(Uuid::now_v7())
    .bind(root_id)
    .bind(&file_path)
    .execute(&server.pool)
    .await
    .unwrap();

    file_id
}

/// Insert an episode file with no video_codec (simulates a newly discovered file).
/// Also creates the required media_roots + media_root_files rows so the JOIN
/// in list_needing_probe/list_needing_extract finds the file.
async fn insert_unprobed_episode_file(server: &TestServer) -> Uuid {
    let show_id = Uuid::now_v7();
    sqlx::query("INSERT INTO tv_shows (id, title, year) VALUES ($1, 'Test Show', 2024)")
        .bind(show_id)
        .execute(&server.pool)
        .await
        .unwrap();

    let season_id = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO tv_seasons (id, show_id, season_identifier, season_sort_order, is_monitored, is_special) \
         VALUES ($1, $2, '1', 1, true, false)",
    )
    .bind(season_id)
    .bind(show_id)
    .execute(&server.pool)
    .await
    .unwrap();

    let episode_id = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO tv_episodes (id, season_id, episode_identifier, episode_sort_order, title) \
         VALUES ($1, $2, '1', 1, 'Test Episode')",
    )
    .bind(episode_id)
    .bind(season_id)
    .execute(&server.pool)
    .await
    .unwrap();

    let file_path = format!("/test/episode_{}.avi", Uuid::now_v7());

    let root_id = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO media_roots (id, path, media_type) VALUES ($1, $2, 'tv') \
         ON CONFLICT (path) DO UPDATE SET id = media_roots.id RETURNING id",
    )
    .bind(root_id)
    .bind("/test-tv")
    .execute(&server.pool)
    .await
    .unwrap();

    let file_id = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO tv_episode_files (id, file_path, format, available) \
         VALUES ($1, $2, 'AVI', true)",
    )
    .bind(file_id)
    .bind(&file_path)
    .execute(&server.pool)
    .await
    .unwrap();

    sqlx::query(
        "INSERT INTO media_root_files (id, media_root_id, file_path, file_name, available) \
         VALUES ($1, $2, $3, 'fake.avi', true)",
    )
    .bind(Uuid::now_v7())
    .bind(root_id)
    .bind(&file_path)
    .execute(&server.pool)
    .await
    .unwrap();

    sqlx::query(
        "INSERT INTO tv_episode_file_episodes (episode_file_id, episode_id) VALUES ($1, $2)",
    )
    .bind(file_id)
    .bind(episode_id)
    .execute(&server.pool)
    .await
    .unwrap();

    file_id
}

// =============================================================================
// mark_unreadable prevents re-probe loop
// =============================================================================

#[tokio::test]
async fn test_unprobed_movie_file_appears_in_needing_probe() {
    let server = TestServer::start_api_only().await;
    let file_id = insert_unprobed_movie_file(&server).await;

    let needing_probe = api::repositories::movie_files::list_needing_probe(&server.pool)
        .await
        .unwrap();
    assert!(
        needing_probe.contains(&file_id),
        "File with NULL video_codec should appear in list_needing_probe"
    );
}

#[tokio::test]
async fn test_mark_unreadable_movie_excludes_from_probe_and_extract() {
    let server = TestServer::start_api_only().await;
    let file_id = insert_unprobed_movie_file(&server).await;

    // Mark as unreadable (simulating ffprobe failure)
    let updated = api::repositories::movie_files::mark_unreadable(&server.pool, file_id)
        .await
        .unwrap();
    assert!(updated, "mark_unreadable should update exactly one row");

    // Should no longer appear in list_needing_probe (WHERE video_codec IS NULL)
    let needing_probe = api::repositories::movie_files::list_needing_probe(&server.pool)
        .await
        .unwrap();
    assert!(
        !needing_probe.contains(&file_id),
        "Unreadable file must not appear in list_needing_probe"
    );

    // Should not appear in list_needing_extract either
    let needing_extract = api::repositories::movie_files::list_needing_extract(&server.pool)
        .await
        .unwrap();
    assert!(
        !needing_extract.contains(&file_id),
        "Unreadable file must not appear in list_needing_extract"
    );
}

#[tokio::test]
async fn test_mark_unreadable_episode_excludes_from_probe_and_extract() {
    let server = TestServer::start_api_only().await;
    let file_id = insert_unprobed_episode_file(&server).await;

    // Verify it starts in needing_probe
    let needing_probe = api::repositories::tv_episode_files::list_needing_probe(&server.pool)
        .await
        .unwrap();
    assert!(
        needing_probe.contains(&file_id),
        "File with NULL video_codec should appear in list_needing_probe"
    );

    // Mark as unreadable
    let updated = api::repositories::tv_episode_files::mark_unreadable(&server.pool, file_id)
        .await
        .unwrap();
    assert!(updated);

    // Should no longer appear in either list
    let needing_probe = api::repositories::tv_episode_files::list_needing_probe(&server.pool)
        .await
        .unwrap();
    assert!(
        !needing_probe.contains(&file_id),
        "Unreadable file must not appear in list_needing_probe"
    );

    let needing_extract = api::repositories::tv_episode_files::list_needing_extract(&server.pool)
        .await
        .unwrap();
    assert!(
        !needing_extract.contains(&file_id),
        "Unreadable file must not appear in list_needing_extract"
    );
}

#[tokio::test]
async fn test_mark_unreadable_does_not_affect_normal_probed_files() {
    let server = TestServer::start_api_only().await;
    let file_id = insert_unprobed_movie_file(&server).await;

    // Simulate a successful probe by setting a real codec
    sqlx::query("UPDATE movie_files SET video_codec = 'h264' WHERE id = $1")
        .bind(file_id)
        .execute(&server.pool)
        .await
        .unwrap();

    // Should not be in needing_probe
    let needing_probe = api::repositories::movie_files::list_needing_probe(&server.pool)
        .await
        .unwrap();
    assert!(!needing_probe.contains(&file_id));

    // Should be in needing_extract (has codec but no keyframes)
    let needing_extract = api::repositories::movie_files::list_needing_extract(&server.pool)
        .await
        .unwrap();
    assert!(
        needing_extract.contains(&file_id),
        "Successfully probed file should still appear in list_needing_extract"
    );
}
