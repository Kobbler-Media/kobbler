use crate::common::*;
use api::jobs::MediaRootIoGate;
use api::jobs::handlers::{MediaAnalyzeBackfillHandler, MediaAnalyzeHandler};
use api::jobs::worker::JobHandler;
use api::repositories::movie_files;
use serde_json::{Value, json};
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

const MKV: &[u8] = include_bytes!("../fixtures/playback/interlaced.mkv");

/// Five 5x5 sprite sheets of 320x180 thumbnails, every one a 2.39 picture in a 16:9 frame.
fn write_letterboxed_trickplay(dir: &Path) {
    std::fs::create_dir_all(dir).unwrap();
    let mut vtt = String::from("WEBVTT\n\n");
    for sheet in 1..=5u32 {
        let img = image::GrayImage::from_fn(1600, 900, |x, y| {
            let y = y % 180;
            image::Luma([if (22..158).contains(&y) {
                60 + ((x + y) % 90) as u8
            } else {
                1
            }])
        });
        img.save(dir.join(format!("sprite_{sheet:03}.jpg")))
            .unwrap();
        for tile in 0..25u32 {
            let i = (sheet - 1) * 25 + tile;
            let stamp = |s: u32| format!("00:{:02}:{:02}.000", s / 60, s % 60);
            vtt.push_str(&format!(
                "{} --> {}\nsprite_{sheet:03}.jpg#xywh={},{},320,180\n\n",
                stamp(i * 5),
                stamp(i * 5 + 5),
                (tile % 5) * 320,
                (tile / 5) * 180
            ));
        }
    }
    std::fs::write(dir.join("thumbnails.vtt"), vtt).unwrap();
}

#[tokio::test]
async fn an_analysed_file_gets_only_the_missing_bar_check_and_its_session_reports_it() {
    let server = TestServer::start_api_only().await;
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
    movie_files::set_media_cache_version(&server.pool, file_id, 2)
        .await
        .unwrap();
    write_letterboxed_trickplay(&api::media_cache::trickplay_dir(
        &server.storage_root(),
        file_id,
    ));

    server
        .admin_client()
        .patch_json(
            "/api/v1/admin/config",
            &json!({ "enrichment_enabled": true }),
        )
        .await
        .assert_ok();
    let backfill = MediaAnalyzeBackfillHandler::new(
        server.pool.clone(),
        server.nats_publisher.clone(),
        server.runtime_config.clone(),
        Arc::new(AtomicBool::new(true)),
    );
    let walked = backfill
        .execute(Uuid::now_v7(), json!({}), CancellationToken::new())
        .await
        .unwrap();
    assert_eq!(
        walked["gaps_enqueued"], 1,
        "an analysed file missing the check is queued for it: {walked}"
    );

    let analyze = MediaAnalyzeHandler::new(
        server.pool.clone(),
        server.nats_publisher.clone(),
        server.runtime_config.clone(),
        Arc::new(tokio::sync::Semaphore::new(1)),
        kob_gpu_gate::GpuLock::new(),
        server.storage_root(),
        MediaRootIoGate::new(server.pool.clone()),
    );
    server.process_pending_jobs(&analyze).await;

    let again = backfill
        .execute(Uuid::now_v7(), json!({}), CancellationToken::new())
        .await
        .unwrap();
    assert_eq!(
        again["gaps_found"], 0,
        "a checked file is not queued again: {again}"
    );

    let session: Value = server
        .admin_client()
        .post_json(
            "/api/v1/admin/playback/sessions",
            &json!({
                "media_type": "movie",
                "file_id": file_id.to_string(),
                "capabilities": { "video_codecs": ["h264"], "audio_codecs": ["aac"], "containers": ["mkv", "matroska", "webm"] }
            }),
        )
        .await
        .json();
    let picture = &session["picture"];
    let shape = 1920.0 * picture["width"].as_f64().unwrap()
        / (1080.0 * picture["height"].as_f64().unwrap());
    assert!(
        (shape - 2.39).abs() < 0.08,
        "picture {picture} is {shape:.2}:1"
    );
}
