//! A debrid stream cast to a DLNA renderer takes the same one-response MPEG-TS delivery a
//! library session does, whether the release streams in or is already on disk.

use crate::common::debrid_rig::*;
use crate::common::debrid_upstream::{UpstreamPlan, WholeBody};
use crate::common::*;
use reqwest::StatusCode;
use serde_json::{Value, json};
use std::time::Duration;
use uuid::Uuid;

const PREWARM: &str = "/api/v1/admin/stream/prewarm";
const MINUTE: &[u8] = include_bytes!("../fixtures/playback/minute.mkv");

fn hls_caps() -> Value {
    json!({ "video_codecs": ["h264"], "audio_codecs": ["aac"], "containers": ["mp4"] })
}

/// Takes the container, so a streaming release starts out as direct play, but not the
/// video codec, so the probe turns it into a transcode.
fn mkv_without_h264_caps() -> Value {
    json!({ "video_codecs": ["hevc"], "audio_codecs": ["aac"], "containers": ["mkv", "matroska"] })
}

struct Case {
    what: &'static str,
    caps: Value,
    delivery: Option<&'static str>,
    progressive: bool,
}

fn cases() -> Vec<Case> {
    vec![
        Case {
            what: "progressive transcode",
            caps: hls_caps(),
            delivery: Some("progressive"),
            progressive: true,
        },
        Case {
            what: "default delivery",
            caps: hls_caps(),
            delivery: None,
            progressive: false,
        },
        Case {
            what: "explicit hls",
            caps: hls_caps(),
            delivery: Some("hls"),
            progressive: false,
        },
        Case {
            what: "direct play",
            caps: direct_play_caps(),
            delivery: Some("progressive"),
            progressive: false,
        },
    ]
}

fn body(rig: &Rig, info_hash: &str, movie: Uuid, caps: Value, delivery: Option<&str>) -> Value {
    let mut body = rig.resolve_body(info_hash, movie, caps);
    if let Some(d) = delivery {
        body["delivery"] = json!(d);
    }
    body
}

fn expected_url(session: &Value) -> Value {
    json!(format!(
        "/api/v1/admin/playback/sessions/{}/stream.ts",
        session_id_of(session)
    ))
}

fn assert_progressive(what: &str, session: &Value, progressive: bool) {
    if progressive {
        assert_ne!(session["method"], "direct_play", "{what}: {session}");
        assert_eq!(
            session["progressive_url"],
            expected_url(session),
            "{what}: {session}"
        );
    } else {
        assert!(session["progressive_url"].is_null(), "{what}: {session}");
    }
}

/// Reads only the head: a held release's transcode does not reach its end.
async fn assert_serves_mpegts(rig: &Rig, what: &str, session: &Value) {
    use futures::StreamExt;
    let resp = reqwest::Client::new()
        .get(format!(
            "{}{}",
            rig.server.api_url,
            session["progressive_url"].as_str().unwrap()
        ))
        .bearer_auth(&rig.server.admin_token)
        .send()
        .await
        .expect("stream.ts request");
    assert_eq!(resp.status(), StatusCode::OK, "{what}");
    assert_eq!(resp.headers()["content-type"], "video/mp2t", "{what}");
    let mut bytes = Vec::new();
    let mut stream = resp.bytes_stream();
    while bytes.len() < 188 * 3
        && let Some(chunk) = tokio::time::timeout(Duration::from_secs(20), stream.next())
            .await
            .unwrap_or_else(|_| panic!("{what}: no TS bytes"))
    {
        bytes.extend_from_slice(&chunk.expect("body chunk"));
    }
    assert!(bytes.len() >= 188 * 3, "{what}: {} bytes", bytes.len());
    for packet in [0, 188, 376] {
        assert_eq!(bytes[packet], 0x47, "{what}: TS sync byte at {packet}");
    }
}

/// A minute-long release whose download stalls half way and whose ranged reads crawl, so
/// its session is still transcoding when a repeat resolve looks for it.
async fn held_rig() -> Rig {
    let mut plan = UpstreamPlan::new(MINUTE.to_vec());
    plan.whole = WholeBody::Stall {
        first: MINUTE.len() / 2,
    };
    plan.range_delay = Duration::from_secs(3);
    Rig::start(plan, "film.mkv").await
}

/// Probed, and a transcode attached and running when the decision needs one: before
/// then a repeat resolve takes the session for dead and opens another.
async fn wait_live(rig: &Rig, session_id: Uuid) {
    use api::streaming::decision::PlaybackMethod;
    let sessions = &rig.server.app_state.playback_sessions;
    wait_until(
        Duration::from_secs(30),
        "the session never went live",
        || {
            sessions.get_mut(&session_id).is_some_and(|mut s| {
                !s.file_path.as_os_str().is_empty()
                    && (s.decision.method == PlaybackMethod::DirectPlay
                        || s.transcode.as_mut().is_some_and(|t| t.is_running()))
            })
        },
    )
    .await;
}

async fn fixture_rig() -> Rig {
    Rig::start(UpstreamPlan::new(ITA_ENG_FIXTURE.to_vec()), "film.mkv").await
}

#[tokio::test]
async fn a_streaming_release_answers_the_delivery_it_was_asked_for() {
    let rig = held_rig().await;
    for (n, case) in cases().into_iter().enumerate() {
        let hash = format!("remotehash{n}");
        let req = body(&rig, &hash, rig.movie().await, case.caps, case.delivery);
        let session = rig.resolve_with(&req).await;
        assert_progressive(case.what, &session, case.progressive);
        let id = session_id_of(&session);
        wait_live(&rig, id).await;
        let probed = rig.resolve_with(&req).await;
        assert_eq!(session_id_of(&probed), id, "{}", case.what);
        assert_progressive(case.what, &probed, case.progressive);
        if case.progressive {
            assert_serves_mpegts(&rig, case.what, &session).await;
        }
    }
}

#[tokio::test]
async fn a_release_on_disk_answers_the_delivery_it_was_asked_for() {
    let rig = fixture_rig().await;
    for (n, case) in cases().into_iter().enumerate() {
        let hash = format!("localhash{n}");
        let movie = rig.movie().await;
        rig.put_on_disk(&hash, movie).await;
        let resolves = rig.kobload_requests("/stream/resolve").await.len();

        let session = rig
            .resolve_with(&body(&rig, &hash, movie, case.caps, case.delivery))
            .await;
        assert_eq!(
            rig.kobload_requests("/stream/resolve").await.len(),
            resolves,
            "{}: played from disk without kobload",
            case.what
        );
        assert_progressive(case.what, &session, case.progressive);
        if case.progressive {
            assert_serves_mpegts(&rig, case.what, &session).await;
        }
    }
}

#[tokio::test]
async fn a_session_the_probe_turns_into_a_transcode_gains_its_progressive_url() {
    let rig = held_rig().await;
    let req = body(
        &rig,
        "probedhash",
        rig.movie().await,
        mkv_without_h264_caps(),
        Some("progressive"),
    );
    let session = rig.resolve_with(&req).await;
    assert_eq!(session["method"], "direct_play", "{session}");
    assert!(session["progressive_url"].is_null(), "{session}");
    wait_live(&rig, session_id_of(&session)).await;
    let again = rig.resolve_with(&req).await;
    assert_progressive("after the probe", &again, true);
    assert_serves_mpegts(&rig, "after the probe", &again).await;
}

async fn wait_ready(rig: &Rig, body: &Value) -> Uuid {
    let deadline = std::time::Instant::now() + Duration::from_secs(15);
    loop {
        let resp = rig.server.admin_client().post_json(PREWARM, body).await;
        resp.assert_ok();
        let reply: Value = resp.json();
        if reply["state"] == "ready" {
            return reply["prewarm_id"].as_str().unwrap().parse().unwrap();
        }
        assert!(std::time::Instant::now() < deadline, "never ready: {reply}");
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

#[tokio::test]
async fn a_claimed_prewarm_answers_the_delivery_it_was_asked_for() {
    let rig = fixture_rig().await;
    rig.mount_search(json!([rig.option("warmhash")])).await;
    let movie = rig.movie().await;
    let prewarm = json!({ "media_type": "movie", "media_id": movie, "capabilities": hls_caps() });
    wait_ready(&rig, &prewarm).await;

    let resolved = rig
        .resolve_with(&body(
            &rig,
            "warmhash",
            movie,
            hls_caps(),
            Some("progressive"),
        ))
        .await;
    assert_eq!(rig.kobload_requests("/stream/resolve").await.len(), 1);
    assert_progressive("claimed by a resolve", &resolved, true);
    assert_serves_mpegts(&rig, "claimed by a resolve", &resolved).await;

    let rig = fixture_rig().await;
    rig.mount_search(json!([rig.option("claimhash")])).await;
    let mut prewarm =
        json!({ "media_type": "movie", "media_id": rig.movie().await, "capabilities": hls_caps() });
    prewarm["delivery"] = json!("progressive");
    let id = wait_ready(&rig, &prewarm).await;
    let resp = rig
        .server
        .admin_client()
        .post_json(&format!("{PREWARM}/{id}/claim"), &json!({}))
        .await;
    resp.assert_status(StatusCode::OK);
    assert_progressive("claimed by id", &resp.json(), true);
}
