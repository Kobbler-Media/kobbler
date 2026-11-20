//! DLNA renderers neither play HLS nor heartbeat a session: they take one MPEG-TS
//! response, SRT sidecars, and keep the session alive only by fetching from it.

use crate::common::*;
use api::repositories::movie_files;
use reqwest::StatusCode;
use serde_json::{Value, json};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};
use uuid::Uuid;

const MINUTE: &[u8] = include_bytes!("../fixtures/playback/minute.mkv");

const ASS: &str = "[Script Info]\nScriptType: v4.00+\n\n[V4+ Styles]\nFormat: Name, Fontname, Fontsize\nStyle: Default,Arial,20\n\n[Events]\nFormat: Layer, Start, End, Style, Name, MarginL, MarginR, MarginV, Effect, Text\nDialogue: 0,0:00:40.00,0:00:42.50,Default,,0,0,0,,{\\i1}Later{\\i0}, line\\Nsecond\nDialogue: 0,0:00:02.00,0:00:04.00,Default,,0,0,0,,Gone\nDialogue: 0,0:00:09.00,0:00:11.25,Default,,0,0,0,,{\\an8}Straddles\n";

const VTT: &str = "WEBVTT\n\n00:00:02.000 --> 00:00:04.000\nGone\n\n00:00:09.000 --> 00:00:11.250 line:90%\n<v Bob>Straddles</v>\n\n00:00:40.000 --> 00:00:42.500\nLater &amp; on\n";

fn hls_caps() -> Value {
    json!({ "video_codecs": ["h264"], "audio_codecs": ["aac"], "containers": ["mp4"] })
}

fn direct_caps() -> Value {
    json!({ "video_codecs": ["h264"], "audio_codecs": ["aac"], "containers": ["mkv", "matroska", "webm"] })
}

async fn seed_file(server: &TestServer, path: &Path) -> Uuid {
    let movie = server
        .admin_client()
        .post_json(
            "/api/v1/admin/movies",
            &json!({ "title": "Renderer", "year": 2024 }),
        )
        .await;
    movie.assert_status(StatusCode::CREATED);
    let movie_id: Uuid = movie.json::<Value>()["id"]
        .as_str()
        .unwrap()
        .parse()
        .unwrap();
    movie_files::create(
        &server.pool,
        movie_files::MovieFileInput {
            movie_id,
            file_path: path.to_string_lossy().into_owned(),
            file_size: Some(std::fs::metadata(path).unwrap().len() as i64),
            format: "mkv".to_string(),
            ..Default::default()
        },
    )
    .await
    .unwrap()
}

async fn seed_minute(server: &TestServer) -> Uuid {
    let path = Path::new(&server.storage_root()).join(format!("{}.mkv", Uuid::new_v4()));
    std::fs::write(&path, MINUTE).unwrap();
    seed_file(server, &path).await
}

/// The minute fixture with an ASS and a WebVTT track muxed in.
async fn seed_minute_with_text_tracks(server: &TestServer) -> Uuid {
    let dir = PathBuf::from(server.storage_root()).join(Uuid::new_v4().to_string());
    std::fs::create_dir_all(&dir).unwrap();
    let (src, ass, vtt, out) = (
        dir.join("src.mkv"),
        dir.join("subs.ass"),
        dir.join("subs.vtt"),
        dir.join("film.mkv"),
    );
    std::fs::write(&src, MINUTE).unwrap();
    std::fs::write(&ass, ASS).unwrap();
    std::fs::write(&vtt, VTT).unwrap();
    let status = std::process::Command::new("ffmpeg")
        .args(["-v", "error", "-y", "-i"])
        .arg(&src)
        .arg("-i")
        .arg(&ass)
        .arg("-i")
        .arg(&vtt)
        .args(["-map", "0", "-map", "1", "-map", "2", "-c", "copy"])
        .arg(&out)
        .status()
        .expect("ffmpeg");
    assert!(status.success());
    seed_file(server, &out).await
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

fn session_id(session: &Value) -> Uuid {
    session["session_id"].as_str().unwrap().parse().unwrap()
}

/// (format, codec names, duration) as ffprobe reads them.
fn probe(bytes: &[u8]) -> (String, Vec<String>, f64) {
    let file = tempfile::NamedTempFile::with_suffix(".ts").unwrap();
    std::fs::write(file.path(), bytes).unwrap();
    let out = std::process::Command::new("ffprobe")
        .args([
            "-v",
            "error",
            "-show_entries",
            "format=format_name,duration:stream=codec_name",
            "-of",
            "json",
        ])
        .arg(file.path())
        .output()
        .expect("ffprobe");
    let v: Value = serde_json::from_slice(&out.stdout).unwrap();
    let codecs = v["streams"]
        .as_array()
        .unwrap()
        .iter()
        .map(|s| s["codec_name"].as_str().unwrap().to_string())
        .collect();
    (
        v["format"]["format_name"].as_str().unwrap().to_string(),
        codecs,
        v["format"]["duration"]
            .as_str()
            .and_then(|d| d.parse().ok())
            .unwrap_or(0.0),
    )
}

#[tokio::test]
async fn progressive_delivery_is_one_mpegts_response_from_the_start_time() {
    let server = TestServer::start_api_only().await;
    let file_id = seed_minute(&server).await;
    let cases = [
        ("remux", json!({}), None, 60.0),
        ("transcode", json!({ "force_transcode": true }), None, 60.0),
        ("resumed", json!({}), Some(30.0), 30.0),
    ];
    for (what, extra, start, expected_duration) in cases {
        let mut req = json!({ "capabilities": hls_caps(), "delivery": "progressive" });
        for (k, v) in extra.as_object().unwrap() {
            req[k] = v.clone();
        }
        if let Some(st) = start {
            req["start_time"] = json!(st);
        }
        let session = open(&server, file_id, req).await;
        let id = session_id(&session);
        assert_eq!(
            session["progressive_url"],
            json!(format!("/api/v1/admin/playback/sessions/{id}/stream.ts")),
            "{what}: {session}"
        );

        let resp = server
            .api_client()
            .get(session["progressive_url"].as_str().unwrap())
            .await;
        resp.assert_ok();
        assert_eq!(resp.headers["content-type"], "video/mp2t", "{what}");
        assert!(
            !resp.headers.contains_key("accept-ranges"),
            "{what}: a live transcode cannot answer a range"
        );
        let bytes = resp.bytes();
        assert!(bytes.len() > 188 * 3, "{what}: {} bytes", bytes.len());
        for packet in [0, 188, 376] {
            assert_eq!(bytes[packet], 0x47, "{what}: TS sync byte at {packet}");
        }
        let (format, codecs, duration) = probe(bytes);
        assert_eq!(format, "mpegts", "{what}");
        assert_eq!(codecs, ["h264", "aac"], "{what}");
        assert!(
            (duration - expected_duration).abs() < 3.0,
            "{what}: the stream runs {duration}s, expected about {expected_duration}s"
        );
    }
}

#[tokio::test]
async fn delivery_defaults_to_hls_and_direct_play_stays_direct() {
    let server = TestServer::start_api_only().await;
    let file_id = seed_minute(&server).await;

    let hls = open(&server, file_id, json!({ "capabilities": hls_caps() })).await;
    assert!(hls["playlist_url"].is_string(), "{hls}");
    assert!(hls["progressive_url"].is_null(), "{hls}");

    let direct = open(
        &server,
        file_id,
        json!({ "capabilities": direct_caps(), "delivery": "progressive" }),
    )
    .await;
    assert!(direct["direct_url"].is_string(), "{direct}");
    assert!(direct["progressive_url"].is_null(), "{direct}");

    let resp = server
        .api_client()
        .get(&format!(
            "/api/v1/admin/playback/sessions/{}/stream.ts",
            session_id(&direct)
        ))
        .await;
    resp.assert_status(StatusCode::CONFLICT);
}

#[tokio::test]
async fn every_text_track_has_an_srt_shifted_by_the_offset() {
    let server = TestServer::start_api_only().await;
    let file_id = seed_minute_with_text_tracks(&server).await;
    let session = open(&server, file_id, json!({ "capabilities": hls_caps() })).await;
    let subs = session["subtitles"].as_array().unwrap();
    let codecs: Vec<&str> = subs.iter().map(|s| s["codec"].as_str().unwrap()).collect();
    assert_eq!(codecs, ["ass", "webvtt"], "{session}");

    let expected =
        "1\n00:00:00,000 --> 00:00:01,250\nStraddles\n\n2\n00:00:30,000 --> 00:00:32,500\n";
    for (sub, last_line) in subs.iter().zip(["Later, line\nsecond", "Later & on"]) {
        let srt_url = sub["srt_url"].as_str().expect("srt_url");
        assert_eq!(
            srt_url,
            format!(
                "/api/v1/admin/playback/sessions/{}/subtitles/{}.srt",
                session_id(&session),
                sub["track_id"].as_str().unwrap()
            )
        );
        let resp = server
            .api_client()
            .get(&format!("{srt_url}?offset=10"))
            .await;
        resp.assert_ok();
        assert!(
            resp.headers["content-type"]
                .to_str()
                .unwrap()
                .starts_with("application/x-subrip"),
            "{:?}",
            resp.headers
        );
        assert_eq!(
            resp.text(),
            format!("{expected}{last_line}\n"),
            "{}",
            sub["codec"]
        );

        let unshifted = server.api_client().get(srt_url).await;
        unshifted.assert_ok();
        assert!(
            unshifted
                .text()
                .starts_with("1\n00:00:02,000 --> 00:00:04,000\nGone\n\n"),
            "{}",
            unshifted.text()
        );
    }
}

fn backdate(server: &TestServer, id: Uuid) {
    let mut session = server.app_state.playback_sessions.get_mut(&id).unwrap();
    session.last_activity = Instant::now() - Duration::from_secs(3600);
}

fn recently_active(server: &TestServer, id: Uuid) -> bool {
    server
        .app_state
        .playback_sessions
        .get(&id)
        .is_some_and(|s| s.last_activity.elapsed() < Duration::from_secs(60))
}

#[tokio::test]
async fn renderer_fetches_count_as_session_activity() {
    let server = TestServer::start_api_only().await;
    let plain = seed_minute(&server).await;
    let with_subs = seed_minute_with_text_tracks(&server).await;

    let direct = open(&server, plain, json!({ "capabilities": direct_caps() })).await;
    let progressive = open(
        &server,
        plain,
        json!({ "capabilities": hls_caps(), "delivery": "progressive" }),
    )
    .await;
    let subtitled = open(&server, with_subs, json!({ "capabilities": hls_caps() })).await;

    let cases = [
        ("direct", &direct, direct["direct_url"].clone()),
        (
            "stream.ts",
            &progressive,
            progressive["progressive_url"].clone(),
        ),
        (
            "srt",
            &subtitled,
            subtitled["subtitles"][0]["srt_url"].clone(),
        ),
    ];
    for (what, session, url) in cases {
        let id = session_id(session);
        let url = url.as_str().unwrap_or_else(|| panic!("{what}: {session}"));
        backdate(&server, id);
        assert!(!recently_active(&server, id));
        server.api_client().get(url).await.assert_ok();
        assert!(recently_active(&server, id), "{what} did not touch");
    }
}
