use crate::common::debrid_rig::*;
use crate::common::debrid_upstream::UpstreamPlan;
use crate::common::*;
use api::jobs::MediaRootIoGate;
use api::jobs::handlers::{KeyframeBackfillHandler, MediaExtractHandler};
use api::jobs::worker::JobHandler;
use api::repositories::{movie_files, subtitle_tracks};
use serde_json::{Value, json};
use std::path::Path;
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, OnceLock};
use std::time::Duration;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

const ENGLISH: &[&str] = &[
    "Where were you last night? I waited for hours.",
    "I told you, the car broke down on the motorway.",
    "You always have an excuse ready, don't you?",
    "Then tell me the truth for once.",
];
const FRENCH: &[&str] = &[
    "Où étais-tu hier soir ? Je t'ai attendu des heures.",
    "Je te l'ai dit, la voiture est tombée en panne.",
    "Tu as toujours une excuse toute prête, n'est-ce pas ?",
    "Alors dis-moi la vérité pour une fois.",
];

fn srt(lines: &[&str], every_secs: u32, count: u32) -> String {
    let stamp = |s: u32| format!("00:{:02}:{:02},000", s / 60, s % 60);
    (0..count)
        .map(|i| {
            format!(
                "{}\n{} --> {}\n{}\n\n",
                i + 1,
                stamp(i * every_secs),
                stamp(i * every_secs + 2),
                lines[i as usize % lines.len()]
            )
        })
        .collect()
}

/// Untagged English, French mis-tagged as Spanish, and a sparse untagged English track.
fn source_mkv() -> &'static [u8] {
    static SOURCE: OnceLock<Vec<u8>> = OnceLock::new();
    SOURCE.get_or_init(|| {
        let dir = tempfile::tempdir().unwrap();
        let subs = [srt(ENGLISH, 5, 110), srt(FRENCH, 5, 110), srt(ENGLISH, 130, 4)];
        let mut args: Vec<String> = "-v error -y -f lavfi -i testsrc=s=64x64:r=1:d=1".split_whitespace().map(String::from).collect();
        for (i, body) in subs.iter().enumerate() {
            let path = dir.path().join(format!("{i}.srt"));
            std::fs::write(&path, body).unwrap();
            args.extend(["-i".into(), path.to_string_lossy().into_owned()]);
        }
        args.extend("-map 0 -map 1 -map 2 -map 3 -c:v libx264 -preset ultrafast -c:s srt -metadata:s:s:1 language=spa".split_whitespace().map(String::from));
        let out = dir.path().join("out.mkv");
        let status = std::process::Command::new("ffmpeg").args(&args).arg(&out).status().expect("ffmpeg");
        assert!(status.success());
        std::fs::read(out).unwrap()
    })
}

async fn seed(server: &TestServer) -> Uuid {
    let movie = server
        .admin_client()
        .post_json(
            "/api/v1/admin/movies",
            &json!({ "title": "Subs", "year": 2024 }),
        )
        .await;
    let movie_id: Uuid = movie.json::<Value>()["id"]
        .as_str()
        .unwrap()
        .parse()
        .unwrap();
    let path = Path::new(&server.storage_root()).join(format!("{movie_id}.mkv"));
    std::fs::write(&path, source_mkv()).unwrap();
    let file_id = movie_files::create(
        &server.pool,
        movie_files::MovieFileInput {
            movie_id,
            file_path: path.to_string_lossy().into_owned(),
            format: "mkv".to_string(),
            container: Some("matroska,webm".to_string()),
            video_codec: Some("h264".to_string()),
            duration_seconds: Some(600.0),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    // Keyframes already found: the job that reads the subtitles must leave them be.
    movie_files::update_keyframes(&server.pool, file_id, &json!([0.0]))
        .await
        .unwrap();
    for (track_index, language) in [(1, None), (2, Some("spa")), (3, None)] {
        subtitle_tracks::create(
            &server.pool,
            &subtitle_tracks::SubtitleTrackInput {
                movie_file_id: Some(file_id),
                episode_file_id: None,
                track_index,
                codec: "subrip".to_string(),
                language: language.map(str::to_string),
                title: None,
                is_forced: false,
                is_default: false,
                is_hearing_impaired: false,
            },
        )
        .await
        .unwrap();
    }
    file_id
}

async fn backfill(server: &TestServer) -> Value {
    KeyframeBackfillHandler::new(
        server.pool.clone(),
        server.nats_publisher.clone(),
        Arc::new(AtomicBool::new(true)),
    )
    .execute(Uuid::now_v7(), json!({}), CancellationToken::new())
    .await
    .unwrap()
}

#[tokio::test]
async fn a_file_extracted_before_its_text_was_read_gets_only_that_part() {
    let server = TestServer::start_api_only().await;
    let file_id = seed(&server).await;

    let first = backfill(&server).await;
    assert_eq!(first["unchecked_subtitles_found"], 1, "{first}");
    server
        .process_pending_jobs(&MediaExtractHandler::new(
            server.pool.clone(),
            server.storage_root(),
            server.runtime_config.clone(),
            Arc::new(tokio::sync::Semaphore::new(1)),
            MediaRootIoGate::new(server.pool.clone()),
        ))
        .await;

    let tracks = subtitle_tracks::list_by_movie_file(&server.pool, file_id)
        .await
        .unwrap();
    let by_index = |i: i32| tracks.iter().find(|t| t.track_index == i).unwrap();
    assert_eq!(by_index(1).language.as_deref(), Some("eng"));
    assert!(!by_index(1).is_forced);
    assert_eq!(
        by_index(2).language.as_deref(),
        Some("spa"),
        "a real tag is never overridden"
    );
    assert!(by_index(3).is_forced);
    let keyframes = movie_files::get(&server.pool, file_id)
        .await
        .unwrap()
        .unwrap()
        .keyframes;
    assert_eq!(
        keyframes,
        Some(json!([0.0])),
        "keyframes it already had were not redone"
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
    let sent = |i: i64| {
        session["subtitles"]
            .as_array()
            .unwrap()
            .iter()
            .find(|t| t["stream_index"] == i)
            .cloned()
            .unwrap()
    };
    assert_eq!(sent(1)["language"], "eng", "{session}");
    assert_eq!(sent(1)["language_name"], "English", "{session}");
    assert_eq!(sent(2)["language_name"], "Spanish", "{session}");
    assert_eq!(sent(3)["is_forced"], true, "{session}");
    assert_eq!(sent(1)["is_forced"], false, "{session}");

    let again = backfill(&server).await;
    assert_eq!(
        again["unchecked_subtitles_found"], 0,
        "a checked file is not queued again: {again}"
    );
}

async fn subtitles_event(rig: &Rig, session_id: Uuid) -> Value {
    let resp = reqwest::Client::new()
        .get(format!(
            "{}/api/v1/admin/stream/{session_id}/status",
            rig.server.api_url
        ))
        .bearer_auth(&rig.server.admin_token)
        .send()
        .await
        .expect("SSE request");
    let mut body = resp.bytes_stream();
    let mut collected = String::new();
    let deadline = std::time::Instant::now() + Duration::from_secs(30);
    while std::time::Instant::now() < deadline {
        if let Some(event) = collected
            .lines()
            .filter_map(|l| l.strip_prefix("data:"))
            .filter_map(|d| serde_json::from_str::<Value>(d.trim()).ok())
            .find(|e| e["type"] == "subtitles")
        {
            return event;
        }
        match tokio::time::timeout(Duration::from_secs(10), futures::StreamExt::next(&mut body))
            .await
        {
            Ok(Some(Ok(chunk))) => collected.push_str(&String::from_utf8_lossy(&chunk)),
            _ => break,
        }
    }
    panic!("no subtitles event: {collected}");
}

fn assert_named(event: &Value) {
    let subs = event["subtitles"].as_array().unwrap();
    let by_index = |i: i64| {
        subs.iter()
            .find(|s| s["stream_index"] == i)
            .cloned()
            .unwrap_or_else(|| panic!("{event}"))
    };
    assert_eq!(by_index(1)["language"], "eng", "{event}");
    assert_eq!(by_index(2)["language"], "spa", "{event}");
    assert_eq!(by_index(1)["language_name"], "English", "{event}");
    assert_eq!(by_index(3)["is_forced"], true, "{event}");
    assert_eq!(by_index(1)["is_forced"], false, "{event}");
}

#[tokio::test]
async fn a_debrid_stream_names_its_subtitle_tracks_from_the_head_of_the_download() {
    let rig = Rig::start(UpstreamPlan::new(source_mkv().to_vec()), "subs.mkv").await;
    let movie = rig.movie().await;
    let session = session_id_of(
        &rig.resolve_with(&rig.resolve_body("subhash", movie, direct_play_caps()))
            .await,
    );

    assert_named(&subtitles_event(&rig, session).await);
}

#[tokio::test]
async fn a_debrid_release_replayed_from_the_cache_names_its_subtitle_tracks() {
    let rig = Rig::start(UpstreamPlan::new(source_mkv().to_vec()), "subs.mkv").await;
    let movie = rig.movie().await;
    let body = rig.resolve_body("replayhash", movie, direct_play_caps());
    let first = session_id_of(&rig.resolve_with(&body).await);
    rig.wait_for_complete("replayhash").await;
    rig.server
        .admin_client()
        .delete(&format!("/api/v1/admin/stream/{first}"))
        .await
        .assert_status(reqwest::StatusCode::NO_CONTENT);

    let replay = session_id_of(&rig.resolve_with(&body).await);

    assert_named(&subtitles_event(&rig, replay).await);
}
