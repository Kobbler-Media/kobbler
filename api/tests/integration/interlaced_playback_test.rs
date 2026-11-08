//! A client whose decoder cannot show field-coded video says `supports_interlaced: false`
//! (the Pixel 10's H.264 decoder emits no frames for MBAFF). Every source must then send
//! it a deinterlaced transcode; a client that says nothing keeps the old behaviour.

use crate::common::debrid_rig::*;
use crate::common::debrid_upstream::UpstreamPlan;
use crate::common::iptv_rig::iptv_channel;
use crate::common::*;
use api::repositories::{audio_tracks, iptv_recordings, movie_files};
use serde_json::{Value, json};
use std::time::Duration;
use uuid::Uuid;
use wiremock::MockServer;

const MKV: &[u8] = include_bytes!("../fixtures/playback/interlaced.mkv");
const TS: &[u8] = include_bytes!("../fixtures/playback/interlaced.ts");
const DURATION: f64 = 4.02;
const SOURCE_FPS: &str = "25/1";
const BWDIF_FPS: &str = "50/1";

fn caps(supports_interlaced: Option<bool>) -> Value {
    let mut caps = json!({
        "video_codecs": ["h264"],
        "audio_codecs": ["aac"],
        "containers": ["mp4", "mkv", "matroska", "webm", "ts", "mpegts"]
    });
    if let Some(s) = supports_interlaced {
        caps["supports_interlaced"] = json!(s);
    }
    caps
}

/// (frame rate, field order) of the first video stream.
fn video_of(bytes: &[u8]) -> (String, String) {
    let file = tempfile::NamedTempFile::with_suffix(".mp4").unwrap();
    std::fs::write(file.path(), bytes).unwrap();
    let out = std::process::Command::new("ffprobe")
        .args([
            "-v",
            "error",
            "-select_streams",
            "v:0",
            "-show_entries",
            "stream=r_frame_rate,field_order",
            "-of",
            "default=nw=1",
        ])
        .arg(file.path())
        .output()
        .expect("ffprobe");
    let text = String::from_utf8_lossy(&out.stdout).into_owned();
    let field = |k: &str| {
        text.lines()
            .find_map(|l| l.strip_prefix(&format!("{k}=")))
            .unwrap_or_default()
            .to_string()
    };
    (field("r_frame_rate"), field("field_order"))
}

/// Init segment + first media segment of an fMP4 HLS playlist, joined.
async fn first_segment(server: &TestServer, playlist_url: &str) -> Vec<u8> {
    first_segment_of(server, playlist_url, "segments/").await
}

/// The init and first media segment whose URI contains [needle].
async fn first_segment_of(server: &TestServer, playlist_url: &str, needle: &str) -> Vec<u8> {
    let client = server.api_client();
    let dir = &playlist_url[..playlist_url.rfind('/').unwrap() + 1];
    let deadline = std::time::Instant::now() + Duration::from_secs(30);
    let playlist = loop {
        let resp = client.get(playlist_url).await;
        let text = resp.text();
        if resp.status.is_success()
            && text
                .lines()
                .any(|l| l.starts_with("segments/") && l.contains(needle))
        {
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
        .find(|l| l.starts_with("segments/") && l.contains(needle))
        .unwrap();
    let mut bytes = Vec::new();
    for uri in [init, seg] {
        let resp = client.get(&format!("{dir}{uri}")).await;
        resp.assert_ok();
        bytes.extend_from_slice(resp.bytes());
    }
    bytes
}

fn assert_deinterlaced(bytes: &[u8], what: &str) {
    let (fps, field_order) = video_of(bytes);
    assert_eq!(fps, BWDIF_FPS, "{what}: bwdif send_field doubles the rate");
    assert_eq!(field_order, "progressive", "{what}");
}

// =============================================================================
// Library files
// =============================================================================

#[derive(Clone, Copy, Debug)]
enum Seed {
    Probed,
    Unprobed,
}

async fn seed_movie_file(server: &TestServer, seed: Seed) -> Uuid {
    let movie = server
        .admin_client()
        .post_json(
            "/api/v1/admin/movies",
            &json!({ "title": "Fields", "year": 2024 }),
        )
        .await;
    let movie_id: Uuid = movie.json::<Value>()["id"]
        .as_str()
        .unwrap()
        .parse()
        .unwrap();
    let path = std::path::Path::new(&server.storage_root()).join(format!("{movie_id}.mkv"));
    std::fs::write(&path, MKV).unwrap();
    let mut input = movie_files::MovieFileInput {
        movie_id,
        file_path: path.to_string_lossy().into_owned(),
        file_size: Some(MKV.len() as i64),
        format: "mkv".to_string(),
        ..Default::default()
    };
    if let Seed::Probed = seed {
        input.container = Some("matroska,webm".to_string());
        input.video_codec = Some("h264".to_string());
        input.video_width = Some(64);
        input.video_height = Some(64);
        input.duration_seconds = Some(DURATION);
        input.is_interlaced = true;
    }
    let file_id = movie_files::create(&server.pool, input).await.unwrap();
    if let Seed::Probed = seed {
        audio_tracks::create(
            &server.pool,
            &audio_tracks::AudioTrackInput {
                movie_file_id: Some(file_id),
                episode_file_id: None,
                track_index: 1,
                codec: "aac".to_string(),
                channels: 1,
                bitrate_kbps: None,
                sample_rate: Some(48000),
                language: None,
                title: None,
                is_default: true,
            },
        )
        .await
        .unwrap();
    }
    file_id
}

async fn open_session(server: &TestServer, media_type: &str, file_id: Uuid, caps: Value) -> Value {
    let resp = server
        .admin_client()
        .post_json(
            "/api/v1/admin/playback/sessions",
            &json!({ "media_type": media_type, "file_id": file_id.to_string(), "capabilities": caps }),
        )
        .await;
    resp.assert_ok();
    resp.json()
}

fn stream_playlist(session_id: impl std::fmt::Display) -> String {
    format!("/api/v1/admin/playback/sessions/{session_id}/stream.m3u8?g=0")
}

async fn assert_session_deinterlaces(server: &TestServer, session: &Value, what: &str) {
    assert_eq!(session["method"], "video_transcode", "{what}: {session}");
    let id = session["session_id"].as_str().unwrap();
    assert_deinterlaced(&first_segment(server, &stream_playlist(id)).await, what);
}

#[tokio::test]
async fn a_library_file_is_deinterlaced_only_for_a_client_that_cannot_show_fields() {
    let server = TestServer::start_api_only().await;
    for seed in [Seed::Probed, Seed::Unprobed] {
        for supports in [None, Some(true)] {
            let file_id = seed_movie_file(&server, seed).await;
            let session = open_session(&server, "movie", file_id, caps(supports)).await;
            assert_eq!(
                session["method"], "direct_play",
                "{seed:?} {supports:?}: {session}"
            );
        }
        let file_id = seed_movie_file(&server, seed).await;
        let session = open_session(&server, "movie", file_id, caps(Some(false))).await;
        assert_session_deinterlaces(&server, &session, &format!("{seed:?}")).await;
    }
}

fn encode_mkv(args: &str) -> Vec<u8> {
    let file = tempfile::NamedTempFile::with_suffix(".mkv").unwrap();
    let status = std::process::Command::new("ffmpeg")
        .args(args.split_whitespace())
        .arg(file.path())
        .status()
        .expect("ffmpeg");
    assert!(status.success());
    std::fs::read(file.path()).unwrap()
}

/// Several segments long, unlike the fixture, so a seek can land somewhere not yet encoded.
fn long_interlaced_mkv() -> Vec<u8> {
    encode_mkv(
        "-v error -y -f lavfi -i testsrc=s=64x64:r=25 -f lavfi -i sine=r=48000 -t 20 \
         -c:v libx264 -preset ultrafast -g 25 -flags +ildct+ilme -x264opts tff=1 \
         -c:a aac -b:a 32k -ac 1",
    )
}

/// A film carried in a 1080i service: progressive frames under a field-coded sequence
/// header, so the container and picture timing both say progressive.
fn fake_interlaced_mkv() -> Vec<u8> {
    encode_mkv(
        "-v error -y -f lavfi -i testsrc=s=64x64:r=25 -f lavfi -i sine=r=48000 -t 4 \
         -c:v libx264 -preset ultrafast -x264-params fake-interlaced=1:pic-struct=1 \
         -c:a aac -b:a 32k -ac 1",
    )
}

#[tokio::test]
async fn progressive_frames_under_a_field_coded_header_are_still_deinterlaced() {
    let server = TestServer::start_api_only().await;
    for supports in [None, Some(false)] {
        let file_id = seed_movie_file(&server, Seed::Unprobed).await;
        let path = movie_files::get(&server.pool, file_id)
            .await
            .unwrap()
            .unwrap()
            .file_path;
        std::fs::write(path, fake_interlaced_mkv()).unwrap();
        let session = open_session(&server, "movie", file_id, caps(supports)).await;
        match supports {
            None => assert_eq!(session["method"], "direct_play", "{session}"),
            _ => assert_session_deinterlaces(&server, &session, "fake interlaced").await,
        }
    }
}

/// A seek outside what is encoded, an audio switch or a decode-error fallback starts the
/// encode again; the new generation has to deinterlace as the first did.
#[tokio::test]
async fn a_restarted_encode_is_still_deinterlaced() {
    let server = TestServer::start_api_only().await;
    let file_id = seed_movie_file(&server, Seed::Unprobed).await;
    let path = movie_files::get(&server.pool, file_id)
        .await
        .unwrap()
        .unwrap()
        .file_path;
    std::fs::write(path, long_interlaced_mkv()).unwrap();
    let resp = server
        .admin_client()
        .post_json(
            "/api/v1/admin/playback/sessions",
            &json!({
                "media_type": "movie",
                "file_id": file_id.to_string(),
                "capabilities": caps(Some(false)),
                "start_time": 12.0,
            }),
        )
        .await;
    resp.assert_ok();
    let session: Value = resp.json();
    assert_session_deinterlaces(&server, &session, "first run").await;
    let id = session["session_id"].as_str().unwrap();
    let seek = server
        .admin_client()
        .post_json(
            &format!("/api/v1/admin/playback/sessions/{id}/seek"),
            &json!({ "time_seconds": 1.0 }),
        )
        .await;
    seek.assert_ok();
    assert_eq!(
        seek.json::<Value>()["from_cache"],
        false,
        "the seek must restart the encode"
    );
    assert_deinterlaced(
        &first_segment_of(&server, &stream_playlist(id), "gen_1/").await,
        "after a seek",
    );
}

async fn open_recording(server: &TestServer, channel_id: Uuid, supports: Option<bool>) -> Value {
    let start = time::OffsetDateTime::now_utc() - time::Duration::hours(1);
    let rec = iptv_recordings::create(
        &server.pool,
        iptv_recordings::CreateRecording {
            channel_id,
            epg_id: None,
            title: "Fields".into(),
            start_time: start,
            end_time: start + time::Duration::minutes(30),
            pre_padding_secs: Some(0),
            post_padding_secs: Some(0),
        },
    )
    .await
    .unwrap();
    let path = std::path::Path::new(&server.storage_root()).join(format!("{}.mkv", rec.id));
    std::fs::write(&path, MKV).unwrap();
    iptv_recordings::update_status(
        &server.pool,
        rec.id,
        "completed",
        Some(&path.to_string_lossy()),
        Some(MKV.len() as i64),
        None,
    )
    .await
    .unwrap();
    iptv_recordings::update_media_metadata(
        &server.pool,
        rec.id,
        Some(DURATION),
        Some("h264"),
        Some("matroska,webm"),
    )
    .await
    .unwrap();
    open_session(server, "recording", rec.id, caps(supports)).await
}

#[tokio::test]
async fn a_recording_is_deinterlaced_for_a_client_that_cannot_show_fields() {
    let server = TestServer::start_api_only().await;
    let channel_id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO iptv_channels (id, name, normalized_name, enabled) VALUES ($1, 'Rec', 'rec', true)",
    )
    .bind(channel_id)
    .execute(&server.pool)
    .await
    .unwrap();

    let unsaid = open_recording(&server, channel_id, None).await;
    assert_ne!(unsaid["method"], "video_transcode", "{unsaid}");
    let cannot = open_recording(&server, channel_id, Some(false)).await;
    assert_session_deinterlaces(&server, &cannot, "recording").await;
}

// =============================================================================
// Debrid
// =============================================================================

async fn wait_for_probe(rig: &Rig, session_id: Uuid) {
    let server = &rig.server;
    wait_until_async(
        Duration::from_secs(10),
        "probe never finished",
        || async move {
            let status: Value = session_status(server, session_id).await.json();
            status["state"] != "preparing"
        },
    )
    .await;
}

fn stored_decision(rig: &Rig, session_id: Uuid) -> (String, bool) {
    let session = rig
        .server
        .app_state
        .playback_sessions
        .get(&session_id)
        .expect("session");
    (
        serde_json::to_value(session.decision.method)
            .unwrap()
            .as_str()
            .unwrap()
            .to_string(),
        session.decision.deinterlace,
    )
}

/// Opens [hash] for a client that cannot show fields, over the live download and then from
/// the finished cache, and returns the first segment each served.
async fn debrid_transcodes(rig: &Rig, hash: &str) -> [Vec<u8>; 2] {
    let movie = rig.movie().await;
    let body = rig.resolve_body(hash, movie, caps(Some(false)));
    let remote = session_id_of(&rig.resolve_with(&body).await);
    wait_for_probe(rig, remote).await;
    assert_eq!(
        stored_decision(rig, remote),
        ("video_transcode".to_string(), true)
    );
    let remote_segment = first_segment(&rig.server, &stream_playlist(remote)).await;

    rig.wait_for_complete(hash).await;
    rig.server
        .admin_client()
        .delete(&format!("/api/v1/admin/stream/{remote}"))
        .await;
    let local = rig.resolve_with(&body).await;
    assert_eq!(local["method"], "video_transcode", "{local}");
    let local = session_id_of(&local);
    assert_eq!(
        stored_decision(rig, local),
        ("video_transcode".to_string(), true)
    );
    [
        remote_segment,
        first_segment(&rig.server, &stream_playlist(local)).await,
    ]
}

#[tokio::test]
async fn a_debrid_stream_is_deinterlaced_only_for_a_client_that_cannot_show_fields() {
    let rig = Rig::start(UpstreamPlan::new(MKV.to_vec()), "film.mkv").await;
    for (hash, supports) in [("fieldsnone", None), ("fieldstrue", Some(true))] {
        let movie = rig.movie().await;
        let session = session_id_of(
            &rig.resolve_with(&rig.resolve_body(hash, movie, caps(supports)))
                .await,
        );
        wait_for_probe(&rig, session).await;
        assert_eq!(
            stored_decision(&rig, session),
            ("direct_play".to_string(), false),
            "{supports:?}"
        );
    }

    let [remote, local] = debrid_transcodes(&rig, "fieldsfalse").await;
    assert_deinterlaced(&remote, "debrid remote");
    assert_deinterlaced(&local, "debrid local cache");
}

/// A trailer, a sample or an extra encodes faster than the launchers' crash check looks:
/// an encoder that has already finished cleanly is done, not dead.
#[tokio::test]
async fn a_debrid_file_that_finishes_encoding_at_once_still_plays() {
    let rig = Rig::start(UpstreamPlan::new(MKV.to_vec()), "trailer.mkv").await;
    debrid_transcodes(&rig, "shortclip").await;
}

// =============================================================================
// IPTV
// =============================================================================

async fn open_iptv(server: &TestServer, supports: Option<bool>) -> Value {
    let provider = MockServer::start().await;
    let channel_id = iptv_channel(server, &provider, TS).await;
    let resp = server
        .admin_client()
        .post_json(
            "/api/v1/admin/iptv/sessions",
            &json!({ "channel_id": channel_id, "capabilities": caps(supports) }),
        )
        .await;
    resp.assert_ok();
    let body: Value = resp.json();
    if body["method"] == "video_transcode" {
        let playlist = body["playlist_url"].as_str().unwrap();
        assert_deinterlaced(&first_segment(server, playlist).await, "iptv");
    }
    body
}

#[tokio::test]
async fn an_iptv_channel_is_deinterlaced_only_for_a_client_that_cannot_show_fields() {
    let server = TestServer::start_api_only().await;
    for supports in [None, Some(true)] {
        let body = open_iptv(&server, supports).await;
        assert_eq!(body["method"], "direct_play", "{supports:?}: {body}");
    }
    let body = open_iptv(&server, Some(false)).await;
    assert_eq!(body["method"], "video_transcode", "{body}");
}

#[test]
fn the_fixture_is_field_coded_at_the_source_rate() {
    assert_eq!(video_of(MKV).0, SOURCE_FPS);
    assert_ne!(video_of(MKV).1, "progressive");
}
