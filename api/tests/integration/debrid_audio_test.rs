use crate::common::debrid_rig::*;
use crate::common::debrid_upstream::UpstreamPlan;
use crate::common::*;
use reqwest::StatusCode;
use serde_json::{Value, json};
use std::time::Duration;
use uuid::Uuid;

const ENG_STREAM: i64 = 2;

async fn fixture_rig() -> Rig {
    Rig::start(UpstreamPlan::new(ITA_ENG_FIXTURE.to_vec()), "film.mkv").await
}

async fn preferred_from_status(rig: &Rig, session_id: Uuid) -> Value {
    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    loop {
        let status: Value = session_status(&rig.server, session_id).await.json();
        let preferred = &status["preferred_audio_stream_index"];
        if !preferred.is_null() {
            return preferred.clone();
        }
        assert!(
            std::time::Instant::now() < deadline,
            "status never reported a preferred audio track: {status}"
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

fn audio_languages_of(path: &std::path::Path) -> Vec<String> {
    let out = std::process::Command::new("ffprobe")
        .args([
            "-v",
            "error",
            "-select_streams",
            "a",
            "-show_entries",
            "stream_tags=language",
            "-of",
            "csv=p=0",
        ])
        .arg(path)
        .output()
        .expect("ffprobe");
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .map(str::to_string)
        .collect()
}

#[tokio::test]
async fn searches_ask_kobload_for_the_preferred_audio_language() {
    let rig = fixture_rig().await;
    rig.mount_search(json!([])).await;
    rig.set_config(json!({ "preferred_audio_language": "Italian" }))
        .await;
    let movie = rig.movie().await;
    let cases = [
        (json!({ "movie_id": movie }), json!(["italian"])),
        (
            json!({ "title": "Feed Movie", "media_type": "movie", "tmdb_id": 77 }),
            json!(["italian"]),
        ),
        (
            json!({ "movie_id": movie, "preferred_audio_language": "ger" }),
            json!(["ger"]),
        ),
    ];
    for (i, (body, expected)) in cases.iter().enumerate() {
        rig.server
            .admin_client()
            .post_json("/api/v1/admin/stream/search", body)
            .await
            .assert_ok();
        let searches = rig.kobload_requests("/stream/search").await;
        assert_eq!(searches[i]["audio_languages"], *expected, "{body}");
    }
}

/// The release lists Italian first and flags it default; the viewer wants English.
#[tokio::test]
async fn a_debrid_session_names_the_preferred_audio_track() {
    let rig = fixture_rig().await;
    let movie = rig.movie().await;
    let body = rig.resolve_body("audiohash", movie, direct_play_caps());

    let first = session_id_of(&rig.resolve_with(&body).await);
    assert_eq!(preferred_from_status(&rig, first).await, ENG_STREAM);

    rig.wait_for_complete("audiohash").await;
    rig.server
        .admin_client()
        .delete(&format!("/api/v1/admin/stream/{first}"))
        .await
        .assert_status(StatusCode::NO_CONTENT);
    let local = rig.resolve_with(&body).await;
    assert_eq!(local["preferred_audio_stream_index"], ENG_STREAM, "{local}");
    assert_eq!(local["selected_audio_index"], ENG_STREAM, "{local}");
    assert_audio_named(&local["audio_tracks"]);
}

fn assert_audio_named(tracks: &Value) {
    let names: Vec<&str> = tracks
        .as_array()
        .unwrap_or_else(|| panic!("{tracks}"))
        .iter()
        .map(|t| {
            t["language_name"]
                .as_str()
                .unwrap_or_else(|| panic!("{tracks}"))
        })
        .collect();
    assert_eq!(names, ["Italian", "English"], "{tracks}");
}

#[tokio::test]
async fn the_remux_carries_the_preferred_audio_track_not_the_default() {
    let rig = fixture_rig().await;
    let movie = rig.movie().await;
    let caps = json!({
        "video_codecs": ["h264"],
        "audio_codecs": ["aac"],
        "containers": ["mp4"]
    });
    let session = session_id_of(
        &rig.resolve_with(&rig.resolve_body("remuxhash", movie, caps))
            .await,
    );
    assert_eq!(preferred_from_status(&rig, session).await, ENG_STREAM);

    let played = play(&rig.server, session, "bytes=0-").await;
    assert!(played.status.is_success(), "{}", played.status);
    let out = tempfile::NamedTempFile::with_suffix(".mp4").unwrap();
    std::fs::write(out.path(), &played.body).unwrap();

    assert_eq!(audio_languages_of(out.path()), vec!["eng".to_string()]);
}

/// The web player cannot pick a track out of a raw MKV, so it asks once for a stream that
/// carries the preferred one.
#[tokio::test]
async fn switch_audio_turns_a_debrid_direct_play_into_a_stream_with_that_track() {
    let rig = fixture_rig().await;
    let movie = rig.movie().await;
    let session = session_id_of(
        &rig.resolve_with(&rig.resolve_body("switchhash", movie, direct_play_caps()))
            .await,
    );
    rig.wait_for_complete("switchhash").await;
    assert_eq!(preferred_from_status(&rig, session).await, ENG_STREAM);

    let resp = rig
        .server
        .admin_client()
        .post_json(
            &format!("/api/v1/admin/playback/sessions/{session}/switch-audio"),
            &json!({ "audio_stream_index": ENG_STREAM }),
        )
        .await;
    resp.assert_ok();
    let switched: Value = resp.json();
    assert_ne!(switched["method"], "direct_play", "{switched}");
    assert!(switched["playlist_url"].is_string(), "{switched}");
    assert_eq!(switched["selected_audio_index"], ENG_STREAM, "{switched}");
}

async fn probe_event(rig: &Rig, session_id: Uuid) -> Value {
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
    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    while std::time::Instant::now() < deadline {
        if let Some(event) = collected
            .lines()
            .filter_map(|l| l.strip_prefix("data:"))
            .filter_map(|d| serde_json::from_str::<Value>(d.trim()).ok())
            .find(|e| e["type"] == "probe_complete")
        {
            return event;
        }
        match tokio::time::timeout(Duration::from_secs(5), futures::StreamExt::next(&mut body))
            .await
        {
            Ok(Some(Ok(chunk))) => collected.push_str(&String::from_utf8_lossy(&chunk)),
            _ => break,
        }
    }
    panic!("no probe_complete event: {collected}");
}

#[tokio::test]
async fn the_probe_event_names_the_preferred_audio_track() {
    let rig = fixture_rig().await;
    let movie = rig.movie().await;
    let session = session_id_of(
        &rig.resolve_with(&rig.resolve_body("ssehash", movie, direct_play_caps()))
            .await,
    );

    let event = probe_event(&rig, session).await;
    assert_eq!(event["preferred_audio_stream_index"], ENG_STREAM, "{event}");
    assert_audio_named(&event["audio_tracks"]);
}
