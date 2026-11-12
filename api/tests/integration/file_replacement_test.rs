use crate::common::*;
use api::repositories::{movie_files, tv_episode_files};
use serde_json::json;
use sqlx::Row;
use uuid::Uuid;

/// Create a show → season → episode hierarchy. Returns (show_id, season_id, episode_id).
async fn create_episode_hierarchy(pool: &sqlx::PgPool, suffix: &str) -> (Uuid, Uuid, Uuid) {
    let show_id = Uuid::now_v7();
    sqlx::query("INSERT INTO tv_shows (id, title) VALUES ($1, $2)")
        .bind(show_id)
        .bind(format!("Test Show {suffix}"))
        .execute(pool)
        .await
        .unwrap();

    let season_id = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO tv_seasons (id, show_id, season_identifier, season_sort_order, is_monitored, is_special) \
         VALUES ($1, $2, '1', 1, true, false)",
    )
    .bind(season_id)
    .bind(show_id)
    .execute(pool)
    .await
    .unwrap();

    let ep_id = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO tv_episodes (id, season_id, episode_identifier, episode_sort_order, title) \
         VALUES ($1, $2, '1', 1, $3)",
    )
    .bind(ep_id)
    .bind(season_id)
    .bind(format!("Episode {suffix}"))
    .execute(pool)
    .await
    .unwrap();

    (show_id, season_id, ep_id)
}

/// Create a minimal episode file. Returns the file ID.
async fn create_ep_file(pool: &sqlx::PgPool, path: &str) -> Uuid {
    tv_episode_files::create(
        pool,
        tv_episode_files::EpisodeFileInput {
            file_path: path.to_string(),
            file_size: Some(1024),
            file_fingerprint: None,
            format: "mkv".to_string(),
            quality: None,
            languages: None,
            subtitles: None,
            source_type: Some("test".to_string()),
            source_metadata: None,
            video_codec: None,
            video_width: None,
            video_height: None,
            video_bitrate_kbps: None,
            framerate: None,
            bit_depth: None,
            is_hdr: false,
            is_interlaced: false,
            hdr_format: None,
            duration_seconds: None,
            container: None,
            overall_bitrate_kbps: None,
        },
    )
    .await
    .unwrap()
}

/// Build an episode file input for the given path and size.
fn ep_file_input(path: &str, file_size: Option<i64>) -> tv_episode_files::EpisodeFileInput {
    tv_episode_files::EpisodeFileInput {
        file_path: path.to_string(),
        file_size,
        file_fingerprint: None,
        format: "mkv".to_string(),
        quality: None,
        languages: None,
        subtitles: None,
        source_type: Some("test".to_string()),
        source_metadata: None,
        video_codec: Some("h264".to_string()),
        video_width: None,
        video_height: None,
        video_bitrate_kbps: None,
        framerate: None,
        bit_depth: None,
        is_hdr: false,
        is_interlaced: false,
        hdr_format: None,
        duration_seconds: Some(1800.0),
        container: None,
        overall_bitrate_kbps: None,
    }
}

// =============================================================================
// Episode file replacement
// =============================================================================

#[tokio::test]
async fn test_episode_file_replaced_on_new_ingest() {
    let server = TestServer::start_api_only().await;
    let pool = &server.pool;

    let (_show_id, _season_id, ep_id) = create_episode_hierarchy(pool, "replace").await;

    // Create first file and link to episode
    let file_a = create_ep_file(pool, "/test/ep1-old.mkv").await;
    let replaced = tv_episode_files::link_episodes(pool, file_a, &[ep_id])
        .await
        .unwrap();
    assert!(replaced.is_empty(), "No files to replace on first link");

    // Create second file and link to same episode — should replace file_a
    let file_b = create_ep_file(pool, "/test/ep1-new.mp4").await;
    let replaced = tv_episode_files::link_episodes(pool, file_b, &[ep_id])
        .await
        .unwrap();
    assert_eq!(replaced, vec!["/test/ep1-old.mkv"]);

    // Verify file_a no longer exists in DB
    let file_a_exists: bool =
        sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM tv_episode_files WHERE id = $1)")
            .bind(file_a)
            .fetch_one(pool)
            .await
            .unwrap();
    assert!(!file_a_exists, "Old file should be deleted from DB");

    // Verify file_b is linked to the episode
    let ep_ids = tv_episode_files::get_episode_ids(pool, file_b)
        .await
        .unwrap();
    assert_eq!(ep_ids, vec![ep_id]);
}

#[tokio::test]
async fn test_multi_episode_file_not_replaced() {
    let server = TestServer::start_api_only().await;
    let pool = &server.pool;

    let show_id = Uuid::now_v7();
    sqlx::query("INSERT INTO tv_shows (id, title) VALUES ($1, 'Multi-Ep Test')")
        .bind(show_id)
        .execute(pool)
        .await
        .unwrap();

    let season_id = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO tv_seasons (id, show_id, season_identifier, season_sort_order, is_monitored, is_special) \
         VALUES ($1, $2, '1', 1, true, false)",
    )
    .bind(season_id)
    .bind(show_id)
    .execute(pool)
    .await
    .unwrap();

    let ep1 = Uuid::now_v7();
    let ep2 = Uuid::now_v7();
    for (ep_id, num) in [(ep1, 1), (ep2, 2)] {
        sqlx::query(
            "INSERT INTO tv_episodes (id, season_id, episode_identifier, episode_sort_order, title) \
             VALUES ($1, $2, $3, $4, $5)",
        )
        .bind(ep_id)
        .bind(season_id)
        .bind(num.to_string())
        .bind(num)
        .bind(format!("Episode {num}"))
        .execute(pool)
        .await
        .unwrap();
    }

    // Create a multi-episode file (linked to both ep1 and ep2)
    let multi_file = create_ep_file(pool, "/test/ep1+2.mkv").await;
    tv_episode_files::link_episodes(pool, multi_file, &[ep1, ep2])
        .await
        .unwrap();

    // Create a new file for ep1 only — should NOT replace the multi-episode file
    let new_file = create_ep_file(pool, "/test/ep1-single.mkv").await;
    let replaced = tv_episode_files::link_episodes(pool, new_file, &[ep1])
        .await
        .unwrap();
    assert!(
        replaced.is_empty(),
        "Multi-episode file should NOT be replaced"
    );

    // Verify multi-episode file still exists
    let multi_exists: bool =
        sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM tv_episode_files WHERE id = $1)")
            .bind(multi_file)
            .fetch_one(pool)
            .await
            .unwrap();
    assert!(multi_exists, "Multi-episode file should still exist");

    // Verify it's still linked to ep2
    let multi_eps = tv_episode_files::get_episode_ids(pool, multi_file)
        .await
        .unwrap();
    assert!(
        multi_eps.contains(&ep2),
        "Multi-file should still serve ep2"
    );
}

// =============================================================================
// Keyframe cache invalidation on content change
// =============================================================================

/// When a file at the same path is replaced with different content (file_size
/// changes), the cached keyframe table from the old copy is stale and must be
/// cleared, otherwise resume-seek maps to a wrong segment and the player hangs.
#[tokio::test]
async fn test_keyframes_cleared_when_file_size_changes() {
    let server = TestServer::start_api_only().await;
    let pool = &server.pool;

    let path = "/test/ep-keyframe-replace.mkv";

    // Initial import + cached keyframes from the first (bad) copy.
    let file_id = tv_episode_files::create_or_update(pool, ep_file_input(path, Some(1000)))
        .await
        .unwrap();
    tv_episode_files::update_keyframes(pool, file_id, &json!([0.0, 6.0, 12.0, 18.0]))
        .await
        .unwrap();

    // Re-import at the same path with changed content (new copy, different size).
    let same_id = tv_episode_files::create_or_update(pool, ep_file_input(path, Some(2000)))
        .await
        .unwrap();
    assert_eq!(
        same_id, file_id,
        "Upsert on file_path keeps the same row id"
    );

    let kf = tv_episode_files::get(pool, file_id)
        .await
        .unwrap()
        .unwrap()
        .keyframes;
    assert!(
        kf.is_none(),
        "Keyframes must be cleared when file_size changes, got {kf:?}"
    );
}

/// Re-importing the identical file (same size) must NOT wipe cached keyframes —
/// metadata-poor rescans should not cost us an expensive keyframe re-extract.
#[tokio::test]
async fn test_keyframes_preserved_when_file_size_unchanged() {
    let server = TestServer::start_api_only().await;
    let pool = &server.pool;

    let path = "/test/ep-keyframe-stable.mkv";

    let file_id = tv_episode_files::create_or_update(pool, ep_file_input(path, Some(1000)))
        .await
        .unwrap();
    tv_episode_files::update_keyframes(pool, file_id, &json!([0.0, 6.0, 12.0]))
        .await
        .unwrap();

    tv_episode_files::create_or_update(pool, ep_file_input(path, Some(1000)))
        .await
        .unwrap();

    let kf = tv_episode_files::get(pool, file_id)
        .await
        .unwrap()
        .unwrap()
        .keyframes;
    assert_eq!(
        kf,
        Some(json!([0.0, 6.0, 12.0])),
        "Keyframes must survive a same-size re-import"
    );
}

/// A content change must invalidate ALL probe/analysis caches keyed by the row,
/// not just keyframes: duration (taken from the new import), lufs, corrupt_ranges,
/// and media_cache_version (→ 0 so media_analyze regenerates trickplay/skip data).
#[tokio::test]
async fn test_content_change_invalidates_all_caches() {
    let server = TestServer::start_api_only().await;
    let pool = &server.pool;

    let path = "/test/ep-cache-invalidate.mkv";

    let file_id = tv_episode_files::create_or_update(pool, ep_file_input(path, Some(1000)))
        .await
        .unwrap();
    // Seed the analysis-derived caches as if the first copy had been fully processed.
    sqlx::query(
        "UPDATE tv_episode_files \
         SET lufs = -18.0, downmix_peak_db = -2.0, corrupt_ranges = '[[1.0,2.0]]'::jsonb, \
             media_cache_version = 7 \
         WHERE id = $1",
    )
    .bind(file_id)
    .execute(pool)
    .await
    .unwrap();

    // Replace with different content (changed size); the new import carries no duration.
    let mut replaced = ep_file_input(path, Some(2000));
    replaced.duration_seconds = None;
    tv_episode_files::create_or_update(pool, replaced)
        .await
        .unwrap();

    let row = sqlx::query(
        "SELECT duration_seconds, lufs, downmix_peak_db, corrupt_ranges, media_cache_version \
         FROM tv_episode_files WHERE id = $1",
    )
    .bind(file_id)
    .fetch_one(pool)
    .await
    .unwrap();

    let duration: Option<f64> = row.get("duration_seconds");
    let lufs: Option<f64> = row.get("lufs");
    let corrupt: serde_json::Value = row.get("corrupt_ranges");
    let cache_version: i32 = row.get("media_cache_version");

    assert!(
        duration.is_none(),
        "duration must take the new import's value (None → reprobe)"
    );
    assert!(lufs.is_none(), "lufs must be cleared on content change");
    let peak: Option<f64> = row.get("downmix_peak_db");
    assert!(
        peak.is_none(),
        "downmix peak must be cleared on content change"
    );
    assert_eq!(corrupt, json!([]), "corrupt_ranges must reset to empty");
    assert_eq!(
        cache_version, -1,
        "media_cache_version must be the -1 invalidation sentinel (not 0, which is legacy)"
    );
}

/// The scan path passes file_size: None and supplies the content fingerprint
/// instead, so a replace there must be detected via fingerprint, not size.
#[tokio::test]
async fn test_caches_invalidated_when_fingerprint_changes() {
    let server = TestServer::start_api_only().await;
    let pool = &server.pool;

    let path = "/test/ep-fingerprint-replace.mkv";
    let with_fp = |fp: &str| {
        let mut input = ep_file_input(path, None); // scan path: no file_size
        input.file_fingerprint = Some(fp.to_string());
        input
    };

    let file_id = tv_episode_files::create_or_update(pool, with_fp("fp-old"))
        .await
        .unwrap();
    tv_episode_files::update_keyframes(pool, file_id, &json!([0.0, 6.0]))
        .await
        .unwrap();
    sqlx::query("UPDATE tv_episode_files SET media_cache_version = 7 WHERE id = $1")
        .bind(file_id)
        .execute(pool)
        .await
        .unwrap();

    // Same content (same fingerprint) → caches preserved.
    tv_episode_files::create_or_update(pool, with_fp("fp-old"))
        .await
        .unwrap();
    let row = tv_episode_files::get(pool, file_id).await.unwrap().unwrap();
    assert_eq!(
        row.keyframes,
        Some(json!([0.0, 6.0])),
        "Same fingerprint must preserve keyframes"
    );

    // Different content (new fingerprint, file_size still None) → caches invalidated.
    tv_episode_files::create_or_update(pool, with_fp("fp-new"))
        .await
        .unwrap();
    let row = tv_episode_files::get(pool, file_id).await.unwrap().unwrap();
    assert!(
        row.keyframes.is_none(),
        "Changed fingerprint must clear keyframes even when file_size is NULL"
    );

    let cache_version: i32 =
        sqlx::query_scalar("SELECT media_cache_version FROM tv_episode_files WHERE id = $1")
            .bind(file_id)
            .fetch_one(pool)
            .await
            .unwrap();
    assert_eq!(
        cache_version, -1,
        "Changed fingerprint must invalidate media_cache_version"
    );
}

/// Same-size re-import must preserve the analysis caches (no needless recompute).
#[tokio::test]
async fn test_same_size_reimport_preserves_analysis_caches() {
    let server = TestServer::start_api_only().await;
    let pool = &server.pool;

    let path = "/test/ep-cache-stable.mkv";

    let file_id = tv_episode_files::create_or_update(pool, ep_file_input(path, Some(1000)))
        .await
        .unwrap();
    sqlx::query(
        "UPDATE tv_episode_files \
         SET lufs = -18.0, downmix_peak_db = -2.0, corrupt_ranges = '[[1.0,2.0]]'::jsonb, \
             media_cache_version = 7 \
         WHERE id = $1",
    )
    .bind(file_id)
    .execute(pool)
    .await
    .unwrap();

    tv_episode_files::create_or_update(pool, ep_file_input(path, Some(1000)))
        .await
        .unwrap();

    let row = sqlx::query(
        "SELECT lufs, downmix_peak_db, corrupt_ranges, media_cache_version \
         FROM tv_episode_files WHERE id = $1",
    )
    .bind(file_id)
    .fetch_one(pool)
    .await
    .unwrap();
    let lufs: Option<f64> = row.get("lufs");
    let corrupt: serde_json::Value = row.get("corrupt_ranges");
    let cache_version: i32 = row.get("media_cache_version");

    assert_eq!(lufs, Some(-18.0), "lufs must survive same-size re-import");
    let peak: Option<f64> = row.get("downmix_peak_db");
    assert_eq!(
        peak,
        Some(-2.0),
        "downmix peak must survive same-size re-import"
    );
    assert_eq!(
        corrupt,
        json!([[1.0, 2.0]]),
        "corrupt_ranges must survive same-size re-import"
    );
    assert_eq!(
        cache_version, 7,
        "media_cache_version must survive same-size re-import"
    );
}

// =============================================================================
// Movie file replacement
// =============================================================================

#[tokio::test]
async fn test_movie_file_replaced_on_new_ingest() {
    let server = TestServer::start_api_only().await;
    let pool = &server.pool;

    // Create movie via raw SQL
    let movie_id = Uuid::now_v7();
    sqlx::query("INSERT INTO movies (id, title) VALUES ($1, 'Replace Test Movie')")
        .bind(movie_id)
        .execute(pool)
        .await
        .unwrap();

    // Create first file
    let file_a = movie_files::create(
        pool,
        movie_files::MovieFileInput {
            movie_id,
            file_path: "/test/movie-old.mkv".to_string(),
            file_size: Some(1000),
            format: "mkv".to_string(),
            ..Default::default()
        },
    )
    .await
    .unwrap();

    // Create second file
    let file_b = movie_files::create(
        pool,
        movie_files::MovieFileInput {
            movie_id,
            file_path: "/test/movie-new.mp4".to_string(),
            file_size: Some(2000),
            format: "mp4".to_string(),
            ..Default::default()
        },
    )
    .await
    .unwrap();

    // Replace old files
    let replaced = movie_files::delete_other_files_for_movie(pool, movie_id, file_b)
        .await
        .unwrap();
    assert_eq!(replaced, vec!["/test/movie-old.mkv"]);

    // Verify file_a deleted, file_b kept
    let file_a_exists: bool =
        sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM movie_files WHERE id = $1)")
            .bind(file_a)
            .fetch_one(pool)
            .await
            .unwrap();
    assert!(!file_a_exists, "Old movie file should be deleted");

    let file_b_exists: bool =
        sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM movie_files WHERE id = $1)")
            .bind(file_b)
            .fetch_one(pool)
            .await
            .unwrap();
    assert!(file_b_exists, "New movie file should still exist");
}

/// Same content-change invalidation as the episode path, for movies.
#[tokio::test]
async fn test_movie_keyframes_cleared_when_file_size_changes() {
    let server = TestServer::start_api_only().await;
    let pool = &server.pool;

    let movie_id = Uuid::now_v7();
    sqlx::query("INSERT INTO movies (id, title) VALUES ($1, 'Keyframe Replace Movie')")
        .bind(movie_id)
        .execute(pool)
        .await
        .unwrap();

    let path = "/test/movie-keyframe-replace.mkv";
    let input = |size| movie_files::MovieFileInput {
        movie_id,
        file_path: path.to_string(),
        file_size: Some(size),
        format: "mkv".to_string(),
        video_codec: Some("h264".to_string()),
        ..Default::default()
    };

    let file_id = movie_files::create_or_update(pool, input(1000))
        .await
        .unwrap();
    movie_files::update_keyframes(pool, file_id, &json!([0.0, 6.0, 12.0]))
        .await
        .unwrap();
    sqlx::query("UPDATE movie_files SET lufs = -18.0, downmix_peak_db = -2.0 WHERE id = $1")
        .bind(file_id)
        .execute(pool)
        .await
        .unwrap();

    let same_id = movie_files::create_or_update(pool, input(2000))
        .await
        .unwrap();
    assert_eq!(
        same_id, file_id,
        "Upsert on file_path keeps the same row id"
    );

    let kf = movie_files::get(pool, file_id)
        .await
        .unwrap()
        .unwrap()
        .keyframes;
    assert!(
        kf.is_none(),
        "Movie keyframes must be cleared on content change, got {kf:?}"
    );
    let loudness: (Option<f64>, Option<f64>) =
        sqlx::query_as("SELECT lufs, downmix_peak_db FROM movie_files WHERE id = $1")
            .bind(file_id)
            .fetch_one(pool)
            .await
            .unwrap();
    assert_eq!(
        loudness,
        (None, None),
        "Movie loudness must be cleared on content change"
    );
}
