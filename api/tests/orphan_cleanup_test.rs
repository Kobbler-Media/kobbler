use crate::common::*;
use api::jobs::worker::JobHandler;
use std::path::PathBuf;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

/// Create a directory for the given UUID under a parent directory.
async fn create_uuid_dir(parent: &str, id: Uuid) -> PathBuf {
    let dir = PathBuf::from(parent).join(id.to_string());
    tokio::fs::create_dir_all(&dir).await.unwrap();
    // Write a dummy file so we can verify bytes_freed accounting
    tokio::fs::write(dir.join("dummy.dat"), b"test data")
        .await
        .unwrap();
    dir
}

// =============================================================================
// Thumbnail orphan cleanup
// =============================================================================

#[tokio::test]
async fn test_thumbnail_orphan_cleanup() {
    let server = TestServer::start_api_only().await;
    let storage_root = server.storage_root();
    let thumb_base = PathBuf::from(&storage_root).join("thumbnails");

    // Create a movie with a file so we have a valid file ID
    let movie_id = Uuid::now_v7();
    let valid_file_id = Uuid::now_v7();
    sqlx::query("INSERT INTO movies (id, title) VALUES ($1, 'Orphan Test Movie')")
        .bind(movie_id)
        .execute(&server.pool)
        .await
        .unwrap();
    sqlx::query(
        "INSERT INTO movie_files (id, movie_id, file_path, format, available) \
         VALUES ($1, $2, '/test/movie.mkv', 'MKV', true)",
    )
    .bind(valid_file_id)
    .bind(movie_id)
    .execute(&server.pool)
    .await
    .unwrap();

    // Create thumbnail dirs: one valid, one orphan
    let valid_dir = create_uuid_dir(thumb_base.to_str().unwrap(), valid_file_id).await;
    let orphan_id = Uuid::now_v7();
    let orphan_dir = create_uuid_dir(thumb_base.to_str().unwrap(), orphan_id).await;

    assert!(valid_dir.exists());
    assert!(orphan_dir.exists());

    // Run orphan cleanup
    let handler = api::jobs::handlers::OrphanCleanupHandler::new(
        server.pool.clone(),
        storage_root.clone(),
        server.runtime_config.clone(),
    );
    let result = handler
        .execute(
            Uuid::now_v7(),
            serde_json::json!({}),
            CancellationToken::new(),
        )
        .await
        .unwrap();

    // Orphan directory should be gone, valid should remain
    assert!(
        !orphan_dir.exists(),
        "Orphan thumbnail dir should be deleted"
    );
    assert!(
        valid_dir.exists(),
        "Valid thumbnail dir should be preserved"
    );

    // Check result reports deletion
    let total_deleted = result["total_deleted"].as_u64().unwrap();
    assert!(total_deleted >= 1, "Should report at least 1 deletion");
    let total_bytes = result["total_bytes_freed"].as_u64().unwrap();
    assert!(total_bytes > 0, "Should report bytes freed");
}

// =============================================================================
// Subtitle cache orphan cleanup
// =============================================================================

#[tokio::test]
async fn test_subtitle_cache_orphan_cleanup() {
    let server = TestServer::start_api_only().await;
    let storage_root = server.storage_root();
    let sub_base = PathBuf::from(&storage_root).join("cache").join("subtitles");

    // Create a movie file for a valid ID
    let movie_id = Uuid::now_v7();
    let valid_file_id = Uuid::now_v7();
    sqlx::query("INSERT INTO movies (id, title) VALUES ($1, 'Sub Test Movie')")
        .bind(movie_id)
        .execute(&server.pool)
        .await
        .unwrap();
    sqlx::query(
        "INSERT INTO movie_files (id, movie_id, file_path, format, available) \
         VALUES ($1, $2, '/test/sub_movie.mkv', 'MKV', true)",
    )
    .bind(valid_file_id)
    .bind(movie_id)
    .execute(&server.pool)
    .await
    .unwrap();

    // Create subtitle cache dirs: one valid, one orphan
    let valid_dir = create_uuid_dir(sub_base.to_str().unwrap(), valid_file_id).await;
    let orphan_id = Uuid::now_v7();
    let orphan_dir = create_uuid_dir(sub_base.to_str().unwrap(), orphan_id).await;

    let handler = api::jobs::handlers::OrphanCleanupHandler::new(
        server.pool.clone(),
        storage_root.clone(),
        server.runtime_config.clone(),
    );
    handler
        .execute(
            Uuid::now_v7(),
            serde_json::json!({}),
            CancellationToken::new(),
        )
        .await
        .unwrap();

    assert!(
        !orphan_dir.exists(),
        "Orphan subtitle dir should be deleted"
    );
    assert!(valid_dir.exists(), "Valid subtitle dir should be preserved");
}

// =============================================================================
// Stale transcode directory cleanup
// =============================================================================

#[tokio::test]
async fn test_stale_transcode_cleanup() {
    let server = TestServer::start_api_only().await;
    let storage_root = server.storage_root();
    let tc_base = PathBuf::from(&storage_root).join("cache").join("transcode");

    // Create a "stale" session dir (>24h old)
    let stale_id = Uuid::now_v7();
    let stale_dir = create_uuid_dir(tc_base.to_str().unwrap(), stale_id).await;
    // Backdate the mtime to 25 hours ago
    let old_time = filetime::FileTime::from_system_time(
        std::time::SystemTime::now() - std::time::Duration::from_secs(25 * 3600),
    );
    filetime::set_file_mtime(&stale_dir, old_time).unwrap();

    // Create a "fresh" session dir (recent)
    let fresh_id = Uuid::now_v7();
    let fresh_dir = create_uuid_dir(tc_base.to_str().unwrap(), fresh_id).await;

    let handler = api::jobs::handlers::OrphanCleanupHandler::new(
        server.pool.clone(),
        storage_root.clone(),
        server.runtime_config.clone(),
    );
    handler
        .execute(
            Uuid::now_v7(),
            serde_json::json!({}),
            CancellationToken::new(),
        )
        .await
        .unwrap();

    assert!(!stale_dir.exists(), "Stale transcode dir should be deleted");
    assert!(
        fresh_dir.exists(),
        "Fresh transcode dir should be preserved"
    );
}
