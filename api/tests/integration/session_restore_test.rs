//! A restart empties the in-memory session map; a player holding a session's URLs has to
//! find the session there again, playing on from what it already had.

use crate::common::*;
use api::jobs::handlers::PlaybackSessionExpiryHandler;
use api::jobs::worker::JobHandler;
use api::repositories::movie_files;
use reqwest::StatusCode;
use serde_json::{Value, json};
use std::time::Duration;
use uuid::Uuid;

const FIXTURE: &[u8] = include_bytes!("../fixtures/playback/minute.mkv");

fn hls_caps() -> Value {
    json!({ "video_codecs": ["h264"], "audio_codecs": ["aac"], "containers": ["mp4"] })
}

fn direct_caps() -> Value {
    json!({ "video_codecs": ["h264"], "audio_codecs": ["aac"], "containers": ["mkv", "matroska", "webm"] })
}

async fn seed_file(server: &TestServer) -> Uuid {
    let movie = server
        .admin_client()
        .post_json(
            "/api/v1/admin/movies",
            &json!({ "title": "Restart", "year": 2024 }),
        )
        .await;
    movie.assert_status(StatusCode::CREATED);
    let movie_id: Uuid = movie.json::<Value>()["id"]
        .as_str()
        .unwrap()
        .parse()
        .unwrap();
    let path = std::path::Path::new(&server.storage_root()).join("minute.mkv");
    std::fs::write(&path, FIXTURE).unwrap();
    movie_files::create(
        &server.pool,
        movie_files::MovieFileInput {
            movie_id,
            file_path: path.to_string_lossy().into_owned(),
            file_size: Some(FIXTURE.len() as i64),
            format: "mkv".to_string(),
            ..Default::default()
        },
    )
    .await
    .unwrap()
}

async fn open(server: &TestServer, file_id: Uuid, extra: Value) -> Value {
    let mut body = json!({ "media_type": "movie", "file_id": file_id.to_string() });
    for (k, v) in extra.as_object().unwrap() {
        body[k] = v.clone();
    }
    let resp = server
        .admin_client()
        .post_json("/api/v1/admin/playback/sessions", &body)
        .await;
    resp.assert_ok();
    resp.json()
}

fn base(session_id: &str) -> String {
    format!("/api/v1/admin/playback/sessions/{session_id}")
}

/// What a restart leaves behind: the rows and the segments on disk, nothing in memory.
async fn restart(server: &TestServer) {
    api::streaming::persist::flush(&server.app_state).await;
    let ids: Vec<Uuid> = server
        .app_state
        .playback_sessions
        .iter()
        .map(|e| *e.key())
        .collect();
    for id in ids {
        if let Some((_, mut session)) = server.app_state.playback_sessions.remove(&id) {
            session.kill_all_transcodes().await;
        }
    }
}

async fn finished_playlist(server: &TestServer, session_id: &str) -> String {
    let client = server.api_client();
    for _ in 0..200 {
        let resp = client
            .get(&format!("{}/stream.m3u8?g=0", base(session_id)))
            .await;
        if resp.status == StatusCode::OK && resp.text().contains("#EXT-X-ENDLIST") {
            return resp.text();
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    panic!("playlist never finished");
}

fn segment_uris(playlist: &str) -> Vec<&str> {
    playlist
        .lines()
        .filter(|l| l.starts_with("segments/"))
        .collect()
}

async fn stored_progress(server: &TestServer, session_id: &str) -> Value {
    let id: Uuid = session_id.parse().unwrap();
    api::streaming::persist::flush(&server.app_state).await;
    api::repositories::playback_sessions::get(&server.pool, id)
        .await
        .unwrap()
        .expect("row")
        .progress
        .expect("progress")
}

#[tokio::test]
async fn a_transcoded_session_keeps_its_playlist_and_segments_across_a_restart() {
    let server = TestServer::start_api_only().await;
    let file_id = seed_file(&server).await;
    let session = open(&server, file_id, json!({ "capabilities": hls_caps() })).await;
    let id = session["session_id"].as_str().unwrap();
    assert_ne!(session["method"], "direct_play");

    let before = finished_playlist(&server, id).await;
    restart(&server).await;

    let client = server.api_client();
    let after = client.get(&format!("{}/stream.m3u8?g=0", base(id))).await;
    after.assert_ok();
    assert_eq!(
        after.text(),
        before,
        "the player's playlist must not change under it"
    );
    for uri in segment_uris(&before) {
        client.get(&format!("{}/{uri}", base(id))).await.assert_ok();
    }
    client
        .get(&format!("{}/heartbeat", base(id)))
        .await
        .assert_ok();
    client
        .get(&format!("{}/status", base(id)))
        .await
        .assert_ok();
}

#[tokio::test]
async fn a_restart_after_a_seek_resumes_the_playlist_the_seek_made() {
    let server = TestServer::start_api_only().await;
    let file_id = seed_file(&server).await;
    let session = open(
        &server,
        file_id,
        json!({ "capabilities": hls_caps(), "start_time": 30.0 }),
    )
    .await;
    let id = session["session_id"].as_str().unwrap();
    finished_playlist(&server, id).await;

    server
        .admin_client()
        .post_json(
            &format!("{}/seek", base(id)),
            &json!({ "time_seconds": 6.0 }),
        )
        .await
        .assert_ok();
    let before = finished_playlist(&server, id).await;
    let uris = segment_uris(&before);
    assert!(uris[0].contains("gen_1/"), "{before}");

    // gen_1 starts at the seek's segment, so its third file is two segments past it.
    server
        .api_client()
        .get(&format!("{}/{}", base(id), uris[2]))
        .await
        .assert_ok();
    let progress = stored_progress(&server, id).await;
    assert_eq!(progress["playback_start_segment"], 1);
    assert_eq!(
        progress["highest_served_segment"], 3,
        "the playhead is an absolute segment, not the file's index within its generation"
    );

    restart(&server).await;
    let after = server
        .api_client()
        .get(&format!("{}/stream.m3u8?g=0", base(id)))
        .await;
    after.assert_ok();
    assert_eq!(after.text(), before);
}

#[tokio::test]
async fn a_direct_played_session_serves_ranges_across_a_restart() {
    let server = TestServer::start_api_only().await;
    let file_id = seed_file(&server).await;
    let session = open(&server, file_id, json!({ "capabilities": direct_caps() })).await;
    assert_eq!(session["method"], "direct_play");
    let id = session["session_id"].as_str().unwrap();

    restart(&server).await;

    let resp = reqwest::Client::new()
        .get(format!("{}{}/direct", server.api_url, base(id)))
        .header("Range", "bytes=0-99")
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::PARTIAL_CONTENT);
    assert_eq!(resp.bytes().await.unwrap().as_ref(), &FIXTURE[..100]);
}

#[tokio::test]
async fn an_ended_session_is_gone_for_good() {
    let server = TestServer::start_api_only().await;
    let file_id = seed_file(&server).await;

    // Ended while loaded, and ended after a restart without being asked for in between.
    for restart_first in [false, true] {
        let session = open(&server, file_id, json!({ "capabilities": hls_caps() })).await;
        let id = session["session_id"].as_str().unwrap();
        finished_playlist(&server, id).await;
        if restart_first {
            restart(&server).await;
        }

        let dir = std::path::Path::new(&server.storage_root())
            .join("cache/transcode")
            .join(id);
        assert!(dir.exists());
        server
            .admin_client()
            .delete(&base(id))
            .await
            .assert_status(StatusCode::NO_CONTENT);
        restart(&server).await;
        server
            .api_client()
            .get(&format!("{}/status", base(id)))
            .await
            .assert_status(StatusCode::NOT_FOUND);
        let row = api::repositories::playback_sessions::get(&server.pool, id.parse().unwrap())
            .await
            .unwrap();
        assert!(row.is_none(), "restart_first={restart_first}");
    }
}

#[tokio::test]
async fn an_orphaned_session_expires_after_the_idle_limit() {
    let server = TestServer::start_api_only().await;
    let file_id = seed_file(&server).await;
    let idle = open(&server, file_id, json!({ "capabilities": hls_caps() })).await;
    let recent = open(&server, file_id, json!({ "capabilities": hls_caps() })).await;
    let idle_id = idle["session_id"].as_str().unwrap();
    let recent_id = recent["session_id"].as_str().unwrap();
    finished_playlist(&server, idle_id).await;
    restart(&server).await;

    sqlx::query(
        "UPDATE playback_sessions SET last_activity_at = NOW() - INTERVAL '5 hours' WHERE id = $1",
    )
    .bind(Uuid::parse_str(idle_id).unwrap())
    .execute(&server.pool)
    .await
    .unwrap();
    PlaybackSessionExpiryHandler::new(server.pool.clone())
        .execute(Uuid::new_v4(), json!({}), Default::default())
        .await
        .unwrap();

    let client = server.api_client();
    client
        .get(&format!("{}/status", base(idle_id)))
        .await
        .assert_status(StatusCode::NOT_FOUND);
    assert!(
        !std::path::Path::new(&server.storage_root())
            .join("cache/transcode")
            .join(idle_id)
            .exists()
    );
    client
        .get(&format!("{}/status", base(recent_id)))
        .await
        .assert_ok();
}

#[tokio::test]
async fn an_unknown_session_is_still_not_found() {
    let server = TestServer::start_api_only().await;
    server
        .api_client()
        .get(&format!(
            "{}/stream.m3u8",
            base(&Uuid::new_v4().to_string())
        ))
        .await
        .assert_status(StatusCode::NOT_FOUND);
}
