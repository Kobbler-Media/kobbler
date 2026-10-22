//! A tune lives exactly as long as it can deliver: a source that gives up or a buffer
//! that cannot be kept going takes the tune and its tuner with it, and nothing joins a
//! dead one.
//!
//! The device is faked on port 5004 of a loopback alias, where the real one serves its
//! streams, one alias per test so they can run side by side. `clip.ts` is six seconds of
//! testsrc2 + sine as H.264/MP2 in MPEG-TS, served twice in a row for a timestamp jump.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use axum::extract::State;
use axum::response::IntoResponse;
use bytes::Bytes;
use futures_util::StreamExt;
use reqwest::StatusCode;
use serde_json::{Value, json};
use uuid::Uuid;

use crate::common::*;
use api::livetv::live_session;
use api::repositories::{livetv_channels, livetv_tuners};

const CLIP: &[u8] = include_bytes!("../fixtures/livetv/clip.ts");

#[derive(Clone, Copy, PartialEq)]
enum Signal {
    None,
    Clip,
    ClipThenJumpBack,
}

async fn serve(State(signal): State<Arc<Mutex<Signal>>>) -> axum::response::Response {
    let signal = *signal.lock().unwrap();
    if signal == Signal::None {
        // The real device takes ~10 s to give up on a mux.
        tokio::time::sleep(Duration::from_millis(500)).await;
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            [("X-HDHomeRun-Error", "807 No Video Data")],
        )
            .into_response();
    }
    let first = futures_util::stream::iter(
        CLIP.chunks(8192)
            .map(|c| Ok::<_, std::io::Error>(Bytes::copy_from_slice(c)))
            .collect::<Vec<_>>(),
    );
    let again = futures_util::stream::iter(
        CLIP.chunks(8192)
            .map(Bytes::copy_from_slice)
            .collect::<Vec<_>>(),
    )
    .then(|c| async move {
        tokio::time::sleep(Duration::from_millis(230)).await;
        Ok::<_, std::io::Error>(c)
    });
    let body = if signal == Signal::ClipThenJumpBack {
        first
            .chain(again)
            .chain(futures_util::stream::pending())
            .boxed()
    } else {
        first.chain(futures_util::stream::pending()).boxed()
    };
    axum::body::Body::from_stream(body).into_response()
}

async fn fake_device(ip: &str, signal: Signal) -> Arc<Mutex<Signal>> {
    let state = Arc::new(Mutex::new(signal));
    let listener = tokio::net::TcpListener::bind((ip, 5004)).await.unwrap();
    let app = axum::Router::new()
        .route("/auto/{channel}", axum::routing::get(serve))
        .with_state(state.clone());
    tokio::spawn(async move { axum::serve(listener, app).await });
    state
}

async fn channel_on(server: &TestServer, ip: &str) -> Uuid {
    let tuner = livetv_tuners::upsert(
        &server.pool,
        livetv_tuners::CreateTuner {
            device_id: format!("FAKE-{ip}"),
            friendly_name: "Fake".to_string(),
            model_number: None,
            firmware_version: None,
            base_url: format!("http://{ip}"),
            device_auth: None,
            tuner_count: 2,
        },
    )
    .await
    .unwrap()
    .id;
    livetv_channels::upsert_scan(
        &server.pool,
        livetv_channels::UpsertScanChannel {
            tuner_id: tuner,
            lcn: Some(7),
            scan_name: Some("7 Melbourne".to_string()),
            frequency: 177_500_000,
            program_number: 1793,
            onid: None,
            tsid: None,
            modulation: None,
            video_codec: Some("H264".to_string()),
            audio_codec: Some("MPEG".to_string()),
            hd: false,
            signal_strength: Some(100),
            signal_quality: Some(100),
            initial_kind: "tv".to_string(),
            initial_enabled: true,
        },
    )
    .await
    .unwrap()
    .0
    .id
}

async fn open(client: &TestClient, channel: Uuid) -> String {
    let resp = client
        .post_json(
            "/api/v1/admin/livetv/sessions",
            &json!({ "channel_id": channel, "capabilities": {
                "video_codecs": ["h264"], "audio_codecs": ["aac"], "containers": ["mp4"]
            } }),
        )
        .await;
    resp.assert_ok();
    resp.json::<Value>()["session_id"]
        .as_str()
        .unwrap()
        .to_string()
}

async fn eventually<F, Fut>(what: &str, secs: u64, mut check: F)
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = bool>,
{
    let deadline = std::time::Instant::now() + Duration::from_secs(secs);
    while std::time::Instant::now() < deadline {
        if check().await {
            return;
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
    panic!("timed out waiting for {what}");
}

/// Held means a lease with no expiry: a tuner nobody can take.
async fn held_leases(client: &TestClient) -> usize {
    let leases: Value = client.get("/api/v1/admin/livetv/leases").await.json();
    leases["items"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|l| l["expires_at"].is_null())
        .count()
}

async fn segments(client: &TestClient, sid: &str) -> i64 {
    let resp = client
        .get(&format!("/api/v1/admin/livetv/sessions/{sid}/buffer"))
        .await;
    if resp.status != StatusCode::OK {
        return -1;
    }
    resp.json::<Value>()["segments"].as_i64().unwrap_or(0)
}

#[tokio::test]
async fn a_tune_whose_source_gives_up_lets_go_of_the_tuner_and_is_never_rejoined() {
    let server = TestServer::start_api_only().await;
    let ip = "127.0.0.21";
    let signal = fake_device(ip, Signal::None).await;
    let channel = channel_on(&server, ip).await;
    let admin = server.admin_client();

    let dead = open(&admin, channel).await;
    eventually("the dead tune to free its tuner", 15, || async {
        held_leases(&admin).await == 0
    })
    .await;
    let status = admin
        .get(&format!("/api/v1/admin/livetv/sessions/{dead}/buffer"))
        .await;
    assert_eq!(status.status, StatusCode::BAD_GATEWAY);
    assert_eq!(
        status.json::<Value>()["error"],
        "7 Melbourne has no signal right now (the tuner couldn't lock onto it)"
    );
    let beat = admin
        .post_json(
            &format!("/api/v1/admin/livetv/sessions/{dead}/heartbeat"),
            &json!({}),
        )
        .await;
    assert_eq!(beat.status, StatusCode::BAD_GATEWAY);

    *signal.lock().unwrap() = Signal::Clip;
    let fresh = open(&admin, channel).await;
    eventually("a fresh tune to fill its buffer", 30, || async {
        segments(&admin, &fresh).await > 0
    })
    .await;
    assert_eq!(held_leases(&admin).await, 1);
}

#[tokio::test]
async fn the_buffer_survives_the_broadcast_timestamps_jumping_back() {
    let server = TestServer::start_api_only().await;
    let ip = "127.0.0.22";
    fake_device(ip, Signal::ClipThenJumpBack).await;
    let channel = channel_on(&server, ip).await;
    let admin = server.admin_client();
    let sid = open(&admin, channel).await;

    let anon = TestClient::new(&server.api_url);
    eventually("segments from after the jump", 45, || async {
        let resp = anon
            .get(&format!("/api/v1/admin/livetv/sessions/{sid}/stream.m3u8"))
            .await;
        resp.status == StatusCode::OK && resp.text().contains("#EXT-X-DISCONTINUITY")
    })
    .await;
    let status: Value = admin
        .get(&format!("/api/v1/admin/livetv/sessions/{sid}/buffer"))
        .await
        .json();
    assert!(status["tune_error"].is_null(), "{status}");
    assert_eq!(held_leases(&admin).await, 1);
}

#[tokio::test]
async fn an_abandoned_session_is_reaped_and_a_tune_without_sessions_lets_go() {
    let server = TestServer::start_api_only().await;
    let ip = "127.0.0.23";
    fake_device(ip, Signal::Clip).await;
    let channel = channel_on(&server, ip).await;
    let admin = server.admin_client();

    let abandoned = open(&admin, channel).await;
    assert_eq!(held_leases(&admin).await, 1);
    live_session::sweep_sessions(&server.app_state, Duration::ZERO).await;
    let beat = admin
        .post_json(
            &format!("/api/v1/admin/livetv/sessions/{abandoned}/heartbeat"),
            &json!({}),
        )
        .await;
    assert_eq!(beat.status, StatusCode::NOT_FOUND);
    assert_eq!(held_leases(&admin).await, 0);

    let orphaned = open(&admin, channel).await;
    assert_eq!(held_leases(&admin).await, 1);
    server.app_state.livetv.sessions.remove(&orphaned);
    live_session::sweep_sessions(&server.app_state, Duration::from_secs(3600)).await;
    assert_eq!(held_leases(&admin).await, 0);
}
