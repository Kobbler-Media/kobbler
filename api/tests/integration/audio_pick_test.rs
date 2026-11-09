use crate::common::*;
use api::repositories::{audio_tracks, movie_files};
use serde_json::{Value, json};
use uuid::Uuid;

const MKV: &[u8] = include_bytes!("../fixtures/playback/interlaced.mkv");

async fn seed_race(server: &TestServer) -> Uuid {
    let movie = server
        .admin_client()
        .post_json(
            "/api/v1/admin/movies",
            &json!({ "title": "Race", "year": 2026 }),
        )
        .await;
    let movie_id: Uuid = movie.json::<Value>()["id"]
        .as_str()
        .unwrap()
        .parse()
        .unwrap();
    let path = std::path::Path::new(&server.storage_root()).join(format!("{movie_id}.mkv"));
    std::fs::write(&path, MKV).unwrap();
    let file_id = movie_files::create(
        &server.pool,
        movie_files::MovieFileInput {
            movie_id,
            file_path: path.to_string_lossy().into_owned(),
            file_size: Some(MKV.len() as i64),
            format: "mkv".to_string(),
            container: Some("matroska,webm".to_string()),
            video_codec: Some("h264".to_string()),
            video_width: Some(64),
            video_height: Some(64),
            duration_seconds: Some(4.02),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    for (track_index, language, title) in
        [(1, Some("eng"), "Commentary"), (2, None, "Natural/Ambient")]
    {
        audio_tracks::create(
            &server.pool,
            &audio_tracks::AudioTrackInput {
                movie_file_id: Some(file_id),
                episode_file_id: None,
                track_index,
                codec: "aac".to_string(),
                channels: 2,
                bitrate_kbps: None,
                sample_rate: Some(48000),
                language: language.map(str::to_string),
                title: Some(title.to_string()),
                is_default: true,
            },
        )
        .await
        .unwrap();
    }
    file_id
}

#[tokio::test]
async fn a_session_picks_commentary_over_ambient_unless_the_viewer_chose_otherwise() {
    let server = TestServer::start_api_only().await;
    let file_id = seed_race(&server).await;
    let cases: [(Option<&str>, i64); 3] = [
        (None, 1),
        (Some("Natural/Ambient"), 2),
        (Some("A track this release does not have"), 1),
    ];
    for (title, expected) in cases {
        let resp = server
            .admin_client()
            .post_json(
                "/api/v1/admin/playback/sessions",
                &json!({
                    "media_type": "movie",
                    "file_id": file_id.to_string(),
                    "preferred_audio_language": "english",
                    "preferred_audio_title": title,
                    "capabilities": {
                        "video_codecs": ["h264"],
                        "audio_codecs": ["aac"],
                        "containers": ["mkv", "matroska", "webm"]
                    }
                }),
            )
            .await;
        resp.assert_ok();
        let session: Value = resp.json();
        assert_eq!(
            session["selected_audio_index"].as_i64(),
            Some(expected),
            "title {title:?}: {session}"
        );
        assert_eq!(
            session["audio_tracks"][0]["language_name"], "English",
            "{session}"
        );
        assert!(
            session["audio_tracks"][1]["language_name"].is_null(),
            "{session}"
        );
    }
}
