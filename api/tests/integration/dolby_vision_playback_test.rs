//! Dolby Vision profile 5 has no backward-compatible base layer: played or copied as
//! it is, the picture comes out green and purple. Only a client that says it renders
//! Dolby Vision is sent the stream untouched; everyone else gets a tone-mapped encode.

use crate::common::debrid_rig::*;
use crate::common::debrid_upstream::UpstreamPlan;
use crate::common::*;
use api::jobs::handlers::OfflineExportHandler;
use api::repositories::{movie_files, tv_episode_files};
use api::streaming::offline_export::export_file;
use serde_json::{Value, json};
use std::path::{Path, PathBuf};
use std::time::Duration;
use uuid::Uuid;

const DV_P5: &[u8] = include_bytes!("../fixtures/playback/dolby_vision_p5.mp4");

fn caps(supports_dolby_vision: Option<bool>) -> Value {
    let mut caps = json!({
        "video_codecs": ["h264", "hevc"],
        "audio_codecs": ["aac"],
        "containers": ["mp4"]
    });
    if let Some(s) = supports_dolby_vision {
        caps["supports_dolby_vision"] = json!(s);
    }
    caps
}

fn color_tags_of(bytes: &[u8]) -> String {
    let file = tempfile::NamedTempFile::with_suffix(".mp4").unwrap();
    std::fs::write(file.path(), bytes).unwrap();
    let out = std::process::Command::new("ffprobe")
        .args(["-v", "error", "-select_streams", "v:0"])
        .args(["-show_entries", "stream=color_primaries,color_transfer"])
        .args(["-of", "csv=p=0"])
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

fn stream_playlist(session_id: impl std::fmt::Display) -> String {
    format!("/api/v1/admin/playback/sessions/{session_id}/stream.m3u8?g=0")
}

#[derive(Clone, Copy, Debug)]
enum Media {
    Movie,
    Episode,
}

impl Media {
    fn as_str(self) -> &'static str {
        match self {
            Media::Movie => "movie",
            Media::Episode => "episode",
        }
    }
}

async fn create_movie(server: &TestServer) -> Uuid {
    let movie = server
        .admin_client()
        .post_json(
            "/api/v1/admin/movies",
            &json!({ "title": "Below", "year": 2026 }),
        )
        .await;
    movie.json::<Value>()["id"]
        .as_str()
        .unwrap()
        .parse()
        .unwrap()
}

async fn seed_file(server: &TestServer, media: Media, probed: bool) -> (Uuid, PathBuf) {
    let path = Path::new(&server.storage_root()).join(format!("{}.mp4", Uuid::new_v4()));
    std::fs::write(&path, DV_P5).unwrap();
    let file_path = path.to_string_lossy().into_owned();
    let file_id = match media {
        Media::Movie => movie_files::create(
            &server.pool,
            movie_files::MovieFileInput {
                movie_id: create_movie(server).await,
                file_path,
                file_size: Some(DV_P5.len() as i64),
                format: "mp4".to_string(),
                ..Default::default()
            },
        )
        .await
        .unwrap(),
        Media::Episode => tv_episode_files::create(
            &server.pool,
            tv_episode_files::EpisodeFileInput {
                file_path,
                file_size: Some(DV_P5.len() as i64),
                file_fingerprint: None,
                format: "mp4".to_string(),
                quality: None,
                languages: None,
                subtitles: None,
                source_type: None,
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
        .unwrap(),
    };
    if probed {
        api::ffmpeg::persist::probe_and_persist(&server.pool, file_id, &path, media.as_str())
            .await
            .unwrap()
            .expect("probe");
    }
    (file_id, path)
}

async fn open_session(server: &TestServer, media: Media, file_id: Uuid, caps: Value) -> Value {
    let resp = server
        .admin_client()
        .post_json(
            "/api/v1/admin/playback/sessions",
            &json!({ "media_type": media.as_str(), "file_id": file_id.to_string(), "capabilities": caps }),
        )
        .await;
    resp.assert_ok();
    resp.json()
}

#[tokio::test]
async fn probing_records_the_dolby_vision_profile_and_base_layer() {
    let server = TestServer::start_api_only().await;
    let (movie_file, _) = seed_file(&server, Media::Movie, true).await;
    let movie = movie_files::get(&server.pool, movie_file)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        (movie.dv_profile, movie.dv_bl_compat_id, movie.is_hdr),
        (Some(5), Some(0), true)
    );
    assert_eq!(movie.hdr_format.as_deref(), Some("Dolby Vision P5"));

    let (episode_file, _) = seed_file(&server, Media::Episode, true).await;
    let episode = tv_episode_files::get(&server.pool, episode_file)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        (episode.dv_profile, episode.dv_bl_compat_id, episode.is_hdr),
        (Some(5), Some(0), true)
    );
    assert_eq!(episode.hdr_format.as_deref(), Some("Dolby Vision P5"));
}

#[tokio::test]
async fn a_library_file_is_tone_mapped_unless_the_client_renders_dolby_vision() {
    let server = TestServer::start_api_only().await;
    for (media, probed) in [
        (Media::Movie, true),
        (Media::Movie, false),
        (Media::Episode, true),
        (Media::Episode, false),
    ] {
        let what = format!("{media:?} probed={probed}");
        let (file_id, _) = seed_file(&server, media, probed).await;
        let session = open_session(&server, media, file_id, caps(Some(true))).await;
        assert_eq!(session["method"], "direct_play", "{what}: {session}");

        for supports in [None, Some(false)] {
            let (file_id, _) = seed_file(&server, media, probed).await;
            let session = open_session(&server, media, file_id, caps(supports)).await;
            assert_eq!(
                session["method"], "video_transcode",
                "{what} {supports:?}: {session}"
            );
            let id = session["session_id"].as_str().unwrap();
            let segment = first_segment(&server, &stream_playlist(id)).await;
            assert_eq!(color_tags_of(&segment), "bt709,bt709", "{what}");
        }
    }
}

async fn stored_method(rig: &Rig, session_id: Uuid) -> String {
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
    let session = server
        .app_state
        .playback_sessions
        .get(&session_id)
        .expect("session");
    serde_json::to_value(session.decision.method)
        .unwrap()
        .as_str()
        .unwrap()
        .to_string()
}

#[tokio::test]
async fn a_debrid_stream_is_tone_mapped_unless_the_client_renders_dolby_vision() {
    let rig = Rig::start(UpstreamPlan::new(DV_P5.to_vec()), "below.mp4").await;
    let movie = rig.movie().await;
    let renders = session_id_of(
        &rig.resolve_with(&rig.resolve_body("dvyes", movie, caps(Some(true))))
            .await,
    );
    assert_eq!(stored_method(&rig, renders).await, "direct_play");

    let movie = rig.movie().await;
    let body = rig.resolve_body("dvno", movie, caps(None));
    let remote = session_id_of(&rig.resolve_with(&body).await);
    assert_eq!(stored_method(&rig, remote).await, "video_transcode");
    let segment = first_segment(&rig.server, &stream_playlist(remote)).await;
    assert_eq!(color_tags_of(&segment), "bt709,bt709", "debrid remote");

    rig.wait_for_complete("dvno").await;
    rig.server
        .admin_client()
        .delete(&format!("/api/v1/admin/stream/{remote}"))
        .await;
    let local = session_id_of(&rig.resolve_with(&body).await);
    assert_eq!(stored_method(&rig, local).await, "video_transcode");
}

#[tokio::test]
async fn an_offline_export_of_dolby_vision_is_a_tone_mapped_copy_never_the_original() {
    let server = TestServer::start_api_only().await;
    let (file_id, _) = seed_file(&server, Media::Movie, true).await;
    let movie_id = movie_files::get(&server.pool, file_id)
        .await
        .unwrap()
        .unwrap()
        .movie_id;
    let resp = server
        .admin_client()
        .post_json(
            "/api/v1/admin/offline/exports",
            &json!({ "media_type": "movie", "media_id": movie_id.to_string(), "quality": "1080p" }),
        )
        .await;
    resp.assert_ok();
    let export: Value = resp.json();
    assert_ne!(export, json!({ "state": "original" }));
    server
        .process_pending_jobs(&OfflineExportHandler::new(server.app_state.clone()))
        .await;
    let id: Uuid = export["export_id"].as_str().unwrap().parse().unwrap();
    let bytes = std::fs::read(export_file(&server.storage_root(), id)).unwrap();
    assert_eq!(color_tags_of(&bytes), "bt709,bt709");
}
