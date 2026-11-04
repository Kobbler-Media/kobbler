//! What a cast receiver needs from a session that a browser does not.

use crate::common::*;
use api::repositories::movie_files;
use serde_json::{Value, json};
use std::time::Duration;
use uuid::Uuid;

const SRT: &str =
    "1\n00:00:02,000 --> 00:00:03,000\nEarly\n\n2\n00:00:15,000 --> 00:00:17,500\nLate\n";

fn encode_mkv(subtitles: bool) -> Vec<u8> {
    let srt = tempfile::NamedTempFile::with_suffix(".srt").unwrap();
    std::fs::write(srt.path(), SRT).unwrap();
    let file = tempfile::NamedTempFile::with_suffix(".mkv").unwrap();
    let mut cmd = std::process::Command::new("ffmpeg");
    cmd.args(
        "-v error -y -f lavfi -i testsrc=s=128x96:r=25 -f lavfi -i sine=r=48000".split_whitespace(),
    );
    if subtitles {
        cmd.arg("-i").arg(srt.path());
        cmd.args("-map 0 -map 1 -map 2 -c:s srt".split_whitespace());
    }
    cmd.args(
        "-t 20 -c:v libx264 -preset ultrafast -g 25 -c:a aac -b:a 32k -ac 1".split_whitespace(),
    );
    let status = cmd.arg(file.path()).status().expect("ffmpeg");
    assert!(status.success());
    std::fs::read(file.path()).unwrap()
}

async fn seed_movie_file(server: &TestServer, subtitles: bool) -> Uuid {
    let movie = server
        .admin_client()
        .post_json(
            "/api/v1/admin/movies",
            &json!({ "title": "Tall", "year": 2024 }),
        )
        .await;
    let movie_id: Uuid = movie.json::<Value>()["id"]
        .as_str()
        .unwrap()
        .parse()
        .unwrap();
    let bytes = encode_mkv(subtitles);
    let path = std::path::Path::new(&server.storage_root()).join(format!("{movie_id}.mkv"));
    std::fs::write(&path, &bytes).unwrap();
    movie_files::create(
        &server.pool,
        movie_files::MovieFileInput {
            movie_id,
            file_path: path.to_string_lossy().into_owned(),
            file_size: Some(bytes.len() as i64),
            format: "mkv".to_string(),
            ..Default::default()
        },
    )
    .await
    .unwrap()
}

fn height_of(bytes: &[u8]) -> String {
    let file = tempfile::NamedTempFile::with_suffix(".mp4").unwrap();
    std::fs::write(file.path(), bytes).unwrap();
    let out = std::process::Command::new("ffprobe")
        .args([
            "-v",
            "error",
            "-select_streams",
            "v:0",
            "-show_entries",
            "stream=height",
            "-of",
            "default=nw=1:nk=1",
        ])
        .arg(file.path())
        .output()
        .expect("ffprobe");
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

async fn first_segment(server: &TestServer, playlist_url: &str) -> Vec<u8> {
    let client = server.api_client();
    let dir = &playlist_url[..playlist_url.rfind('/').unwrap() + 1];
    let deadline = std::time::Instant::now() + Duration::from_secs(30);
    let playlist = loop {
        let resp = client.get(playlist_url).await;
        let text = resp.text();
        if resp.status.is_success() && text.lines().any(|l| l.starts_with("segments/")) {
            break text;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "no segment in {playlist_url}: {} {text}",
            resp.status
        );
        tokio::time::sleep(Duration::from_millis(100)).await;
    };
    let init = playlist
        .lines()
        .find_map(|l| l.split("URI=\"").nth(1))
        .and_then(|rest| rest.split('"').next())
        .unwrap_or_else(|| panic!("no EXT-X-MAP in {playlist}"));
    let seg = playlist
        .lines()
        .find(|l| l.starts_with("segments/"))
        .unwrap();
    let mut bytes = Vec::new();
    for uri in [init, seg] {
        let resp = client.get(&format!("{dir}{uri}")).await;
        resp.assert_ok();
        bytes.extend_from_slice(resp.bytes());
    }
    bytes
}

/// A 1080p Chromecast decodes HEVC but not 4K HEVC, and says so with `max_height`. A
/// taller file must reach it as a transcode scaled to fit, on the first transcode and on
/// every one a seek restarts.
#[tokio::test]
async fn a_file_taller_than_the_client_can_decode_is_scaled_to_fit() {
    let server = TestServer::start_api_only().await;
    let file_id = seed_movie_file(&server, false).await;
    let caps = |max_height: Option<u32>| {
        let mut caps = json!({
            "video_codecs": ["h264"],
            "audio_codecs": ["aac"],
            "containers": ["mp4", "mkv", "matroska", "webm"]
        });
        if let Some(h) = max_height {
            caps["max_height"] = json!(h);
        }
        caps
    };
    let open = |caps: Value| {
        let server = &server;
        async move {
            let resp = server
                .admin_client()
                .post_json(
                    "/api/v1/admin/playback/sessions",
                    &json!({ "media_type": "movie", "file_id": file_id.to_string(), "capabilities": caps }),
                )
                .await;
            resp.assert_ok();
            resp.json::<Value>()
        }
    };

    for max_height in [None, Some(96)] {
        let session = open(caps(max_height)).await;
        assert_eq!(
            session["method"], "direct_play",
            "{max_height:?}: {session}"
        );
    }

    let session = open(caps(Some(48))).await;
    assert_eq!(session["method"], "video_transcode", "{session}");
    let id = session["session_id"].as_str().unwrap();
    let base = format!("/api/v1/admin/playback/sessions/{id}");

    let master = server
        .api_client()
        .get(&format!("{base}/master.m3u8"))
        .await
        .text();
    assert!(
        !master.contains("RESOLUTION=128x96"),
        "the playlist offers a picture the client cannot decode: {master}"
    );
    assert_eq!(
        height_of(&first_segment(&server, &format!("{base}/stream.m3u8?g=0")).await),
        "48"
    );

    let seek = server
        .admin_client()
        .post_json(&format!("{base}/seek"), &json!({ "time_seconds": 12.0 }))
        .await;
    seek.assert_ok();
    let generation = seek.json::<Value>()["seek_generation"].as_u64().unwrap();
    assert_eq!(
        height_of(&first_segment(&server, &format!("{base}/stream.m3u8?g={generation}")).await),
        "48",
        "the transcode a seek restarted is back at the source height"
    );
}

/// A transcode made from part way in plays from its own zero, and the receiver's text
/// tracks run on that clock.
#[tokio::test]
async fn subtitles_can_be_fetched_on_a_timeline_that_starts_part_way_in() {
    let server = TestServer::start_api_only().await;
    let file_id = seed_movie_file(&server, true).await;
    let resp = server
        .admin_client()
        .post_json(
            "/api/v1/admin/playback/sessions",
            &json!({
                "media_type": "movie",
                "file_id": file_id.to_string(),
                "capabilities": { "video_codecs": ["h264"], "audio_codecs": ["aac"], "containers": ["mp4"] }
            }),
        )
        .await;
    resp.assert_ok();
    let session: Value = resp.json();
    let url = session["subtitles"][0]["url"]
        .as_str()
        .unwrap_or_else(|| panic!("no subtitle track: {session}"))
        .to_string();
    assert!(url.ends_with(".vtt"), "{url}");

    let fetch = |query: &'static str| {
        let server = &server;
        let url = url.clone();
        async move {
            let resp = server.api_client().get(&format!("{url}{query}")).await;
            resp.assert_ok();
            resp.text()
        }
    };
    let whole = fetch("").await;
    assert!(whole.contains("00:00:02.000 --> 00:00:03.000"), "{whole}");
    let shifted = fetch("?offset=12").await;
    assert!(!shifted.contains("Early"), "{shifted}");
    assert!(
        shifted.contains("00:00:03.000 --> 00:00:05.500\nLate"),
        "{shifted}"
    );
}
