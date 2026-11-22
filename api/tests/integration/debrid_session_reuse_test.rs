//! A release resolved again shares the session already open for it whenever that
//! session plays what this request would be given, and only then.

use crate::common::debrid_rig::*;
use crate::common::debrid_upstream::UpstreamPlan;
use crate::common::*;
use serde_json::{Value, json};
use std::time::Duration;
use uuid::Uuid;

async fn on_disk(video: &str, audio: &str) -> (Rig, Uuid) {
    let rig = Rig::start(UpstreamPlan::new(encoded_mkv(video, audio)), "film.mkv").await;
    let movie = rig.movie().await;
    rig.put_on_disk("reusehash", movie).await;
    (rig, movie)
}

fn with_audio(mut body: Value, language: &str) -> Value {
    body["preferred_audio_language"] = json!(language);
    body
}

#[tokio::test]
async fn a_transcode_not_yet_attached_or_already_finished_is_shared() {
    let (rig, movie) = on_disk("libx265", "aac").await;
    let body = rig.resolve_body("reusehash", movie, mkv_h264_aac_caps());
    let first = rig.resolve_with(&body).await;
    assert_eq!(first["method"], "video_transcode", "{first}");
    let id = session_id_of(&first);

    let sessions = &rig.server.app_state.playback_sessions;
    wait_until(Duration::from_secs(20), "transcode never finished", || {
        sessions
            .get_mut(&id)
            .and_then(|mut s| s.transcode.as_mut().map(|t| !t.is_running()))
            .unwrap_or(false)
    })
    .await;
    let again = rig.resolve_with(&body).await;
    assert_eq!(session_id_of(&again), id, "a finished transcode: {again}");

    sessions.get_mut(&id).unwrap().transcode = None;
    let again = rig.resolve_with(&body).await;
    assert_eq!(
        session_id_of(&again),
        id,
        "a transcode still to start: {again}"
    );
}

#[tokio::test]
async fn a_device_the_open_session_does_not_suit_gets_its_own() {
    let (rig, movie) = on_disk("libx265", "aac").await;
    let phone = rig.resolve_body("reusehash", movie, direct_play_caps());
    let receiver = rig.resolve_body("reusehash", movie, mkv_h264_aac_caps());

    let watching = rig.resolve_with(&phone).await;
    assert_eq!(watching["method"], "direct_play", "{watching}");
    let cast = rig.resolve_with(&receiver).await;
    assert_eq!(cast["method"], "video_transcode", "{cast}");
    let cast_id = session_id_of(&cast);
    assert_ne!(cast_id, session_id_of(&watching));
    let phone_again = rig.resolve_with(&phone).await;
    assert_eq!(
        session_id_of(&phone_again),
        session_id_of(&watching),
        "the phone's own session is still open: {phone_again}"
    );

    end_playback(&rig.server, session_id_of(&watching)).await;
    let again = rig.resolve_with(&receiver).await;
    assert_eq!(
        session_id_of(&again),
        cast_id,
        "the session that ended took the other's place with it: {again}"
    );
}

#[tokio::test]
async fn a_listener_of_another_language_gets_their_own() {
    let rig = Rig::start(UpstreamPlan::new(ITA_ENG_FIXTURE.to_vec()), "film.mkv").await;
    let movie = rig.movie().await;
    rig.put_on_disk("reusehash", movie).await;
    let body = rig.resolve_body("reusehash", movie, direct_play_caps());

    let italian = rig.resolve_with(&with_audio(body.clone(), "ita")).await;
    let english = rig.resolve_with(&with_audio(body.clone(), "eng")).await;
    assert_ne!(
        session_id_of(&italian),
        session_id_of(&english),
        "{italian}\n{english}"
    );
    assert_ne!(
        italian["selected_audio_index"],
        english["selected_audio_index"]
    );
    let english_again = rig.resolve_with(&with_audio(body, "eng")).await;
    assert_eq!(session_id_of(&english_again), session_id_of(&english));
}
