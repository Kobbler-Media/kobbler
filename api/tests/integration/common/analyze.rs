use super::TestServer;
use api::jobs::MediaRootIoGate;
use api::jobs::handlers::{MediaAnalyzeBackfillHandler, MediaAnalyzeHandler};
use api::repositories::movie_files;
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
