use crate::common::*;
use api::repositories::{audio_tracks, movie_files, subtitle_tracks};
use serde_json::{Value, json};
use uuid::Uuid;

const MKV: &[u8] = include_bytes!("../fixtures/playback/interlaced.mkv");
pub(crate) const SUB_STREAM: i32 = 2;

pub(crate) async fn seed_file(
    server: &TestServer,
    audio_language: Option<&str>,
    file_name: &str,
) -> Uuid {
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
    let path = std::path::Path::new(&server.storage_root()).join(format!("{movie_id}-{file_name}"));
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
    audio_tracks::create(
        &server.pool,
        &audio_tracks::AudioTrackInput {
            movie_file_id: Some(file_id),
            episode_file_id: None,
            track_index: 1,
            codec: "aac".to_string(),
            channels: 2,
            bitrate_kbps: None,
            sample_rate: Some(48000),
            language: audio_language.map(str::to_string),
            title: None,
            is_default: true,
        },
    )
    .await
    .unwrap();
    subtitle_tracks::create(
        &server.pool,
        &subtitle_tracks::SubtitleTrackInput {
            movie_file_id: Some(file_id),
            episode_file_id: None,
            track_index: SUB_STREAM,
            codec: "subrip".to_string(),
            language: Some("eng".to_string()),
            title: Some("English Subtitles".to_string()),
            is_forced: false,
            is_default: false,
            is_hearing_impaired: false,
        },
    )
    .await
    .unwrap();
    file_id
}

#[tokio::test]
async fn subtitles_switch_on_by_language_only_for_audio_the_viewer_does_not_speak() {
    let server = TestServer::start_api_only().await;
    let cases: [(Option<&str>, bool, Option<i32>); 4] = [
        (None, true, None),
        (Some("eng"), true, None),
        (Some("jpn"), true, Some(SUB_STREAM)),
        (Some("jpn"), false, None),
    ];
    for (audio_language, subtitles_enabled, expected) in cases {
        let file_id = seed_file(&server, audio_language, "film.mkv").await;
        let resp = server
            .admin_client()
            .post_json(
                "/api/v1/admin/playback/sessions",
                &json!({
                    "media_type": "movie",
                    "file_id": file_id.to_string(),
                    "preferred_audio_language": "english",
                    "preferred_subtitle_language": "english",
                    "subtitles_enabled": subtitles_enabled,
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
            session["selected_subtitle_index"].as_i64(),
            expected.map(i64::from),
            "audio {audio_language:?}, subtitles_enabled {subtitles_enabled}: {session}"
        );
    }
}

#[tokio::test]
async fn a_forced_track_switches_on_only_in_a_language_the_viewer_reads() {
    let server = TestServer::start_api_only().await;
    let cases: [(Option<&str>, Option<i32>); 4] = [
        (Some("cze"), None),
        (Some("eng"), Some(SUB_STREAM + 1)),
        (Some("en"), Some(SUB_STREAM + 1)),
        (None, Some(SUB_STREAM + 1)),
    ];
    for (forced_language, expected) in cases {
        let file_id = seed_file(&server, Some("eng"), "film.mkv").await;
        subtitle_tracks::create(
            &server.pool,
            &subtitle_tracks::SubtitleTrackInput {
                movie_file_id: Some(file_id),
                episode_file_id: None,
                track_index: SUB_STREAM + 1,
                codec: "subrip".to_string(),
                language: forced_language.map(str::to_string),
                title: Some("Forced".to_string()),
                is_forced: true,
                is_default: false,
                is_hearing_impaired: false,
            },
        )
        .await
        .unwrap();
        let resp = server
            .admin_client()
            .post_json(
                "/api/v1/admin/playback/sessions",
                &json!({
                    "media_type": "movie",
                    "file_id": file_id.to_string(),
                    "preferred_audio_language": "english",
                    "preferred_subtitle_language": "english",
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
            session["selected_subtitle_index"].as_i64(),
            expected.map(i64::from),
            "forced track in {forced_language:?}: {session}"
        );
    }
}
