use super::TestServer;
use api::jobs::MediaRootIoGate;
use api::jobs::handlers::{
    DownmixPeakFillHandler, MediaAnalyzeBackfillHandler, MediaAnalyzeHandler,
};
use api::repositories::{movie_files, tv_episode_files};
use serde_json::{Value, json};
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use uuid::Uuid;

const MKV: &[u8] = include_bytes!("../../fixtures/playback/interlaced.mkv");

/// A movie whose file is the 4 s interlaced fixture, at `cache_version`. Returns its file id.
pub async fn seed_analyzable_movie(server: &TestServer, cache_version: i32) -> Uuid {
    let movie = server
        .admin_client()
        .post_json(
            "/api/v1/admin/movies",
            &json!({ "title": "Scope", "year": 2024 }),
        )
        .await;
    let movie_id: Uuid = movie.json::<Value>()["id"]
        .as_str()
        .unwrap()
        .parse()
        .unwrap();
    let path = Path::new(&server.storage_root()).join(format!("{movie_id}.mkv"));
    std::fs::write(&path, MKV).unwrap();
    let file_id = movie_files::create(
        &server.pool,
        movie_files::MovieFileInput {
            movie_id,
            file_path: path.to_string_lossy().into_owned(),
            format: "mkv".to_string(),
            container: Some("matroska,webm".to_string()),
            video_codec: Some("h264".to_string()),
            video_width: Some(1920),
            video_height: Some(1080),
            duration_seconds: Some(4.02),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    movie_files::set_media_cache_version(&server.pool, file_id, cache_version)
        .await
        .unwrap();
    file_id
}

/// An episode whose file is the same fixture, at `cache_version`. Returns its file id.
pub async fn seed_analyzable_episode(server: &TestServer, cache_version: i32) -> Uuid {
    let pool = &server.pool;
    let (show_id, season_id, episode_id) = (Uuid::now_v7(), Uuid::now_v7(), Uuid::now_v7());
    sqlx::query("INSERT INTO tv_shows (id, title) VALUES ($1, 'Scope Show')")
        .bind(show_id)
        .execute(pool)
        .await
        .unwrap();
    sqlx::query(
        "INSERT INTO tv_seasons (id, show_id, season_identifier, season_sort_order, is_monitored, is_special) \
         VALUES ($1, $2, '1', 1, true, false)",
    )
    .bind(season_id)
    .bind(show_id)
    .execute(pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO tv_episodes (id, season_id, episode_identifier, episode_sort_order, title) \
         VALUES ($1, $2, '1', 1, 'Pilot')",
    )
    .bind(episode_id)
    .bind(season_id)
    .execute(pool)
    .await
    .unwrap();
    let path = Path::new(&server.storage_root()).join(format!("{episode_id}.mkv"));
    std::fs::write(&path, MKV).unwrap();
    let file_id = tv_episode_files::create(
        pool,
        tv_episode_files::EpisodeFileInput {
            file_path: path.to_string_lossy().into_owned(),
            file_size: Some(MKV.len() as i64),
            file_fingerprint: None,
            format: "mkv".to_string(),
            quality: None,
            languages: None,
            subtitles: None,
            source_type: Some("test".to_string()),
            source_metadata: None,
            video_codec: Some("h264".to_string()),
            video_width: Some(1920),
            video_height: Some(1080),
            video_bitrate_kbps: None,
            framerate: None,
            bit_depth: None,
            is_hdr: false,
            is_interlaced: false,
            hdr_format: None,
            duration_seconds: Some(4.02),
            container: Some("matroska,webm".to_string()),
            overall_bitrate_kbps: None,
        },
    )
    .await
    .unwrap();
    tv_episode_files::link_episodes(pool, file_id, &[episode_id])
        .await
        .unwrap();
    tv_episode_files::set_media_cache_version(pool, file_id, cache_version)
        .await
        .unwrap();
    file_id
}

pub fn analyze_backfill(server: &TestServer) -> MediaAnalyzeBackfillHandler {
    MediaAnalyzeBackfillHandler::new(
        server.pool.clone(),
        server.nats_publisher.clone(),
        server.runtime_config.clone(),
        Arc::new(AtomicBool::new(true)),
    )
}

pub fn analyzer(server: &TestServer) -> MediaAnalyzeHandler {
    MediaAnalyzeHandler::new(
        server.pool.clone(),
        server.nats_publisher.clone(),
        server.runtime_config.clone(),
        Arc::new(tokio::sync::Semaphore::new(1)),
        kob_gpu_gate::GpuLock::new(),
        server.storage_root(),
        MediaRootIoGate::new(server.pool.clone()),
    )
}

pub fn peak_filler(server: &TestServer) -> DownmixPeakFillHandler {
    DownmixPeakFillHandler::new(
        server.pool.clone(),
        server.storage_root(),
        MediaRootIoGate::new(server.pool.clone()),
    )
}
