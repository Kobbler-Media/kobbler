use crate::common::*;
use api::livetv::live_session;
use api::livetv::tuner_lease::{self, HolderKind, LeaseRequest, TunerLeases};
use api::repositories::{livetv_channels, livetv_tuners};
use reqwest::StatusCode;
use serde_json::{Value, json};
use time::OffsetDateTime;
use uuid::Uuid;

// =============================================================================
// Fixtures
// =============================================================================

async fn make_tuner(pool: &sqlx::PgPool, device_id: &str, tuners: i32) -> Uuid {
    livetv_tuners::upsert(
        pool,
        livetv_tuners::CreateTuner {
            device_id: device_id.to_string(),
            friendly_name: "Test Quatro".to_string(),
            model_number: Some("HDHR5-4DT".to_string()),
            firmware_version: Some("20260326".to_string()),
            // Unroutable on purpose: nothing on this path may block on the device.
            base_url: "http://127.0.0.1:1".to_string(),
            device_auth: Some("test-auth".to_string()),
            tuner_count: tuners,
        },
    )
    .await
    .unwrap()
    .id
}

async fn make_channel(pool: &sqlx::PgPool, tuner_id: Uuid, lcn: i32, hd: bool) -> Uuid {
    livetv_channels::upsert_scan(
        pool,
        livetv_channels::UpsertScanChannel {
            tuner_id,
            lcn: Some(lcn),
            scan_name: Some(format!("Channel {lcn}")),
            frequency: 177_500_000 + lcn as i64,
            program_number: 1000 + lcn,
            onid: Some(12931),
            tsid: Some(1793),
            modulation: Some("t7dvbt".to_string()),
            video_codec: Some(if hd { "H264" } else { "MPEG2" }.to_string()),
            audio_codec: Some(if hd { "AC3" } else { "MPEG" }.to_string()),
            hd,
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

fn browser_caps() -> Value {
    json!({
        "video_codecs": ["h264"],
        "audio_codecs": ["aac"],
        "containers": ["mp4"]
    })
}

fn exoplayer_caps() -> Value {
    json!({
        "video_codecs": ["h264", "mpeg2video"],
        "audio_codecs": ["ac3", "mp2", "aac"],
        "containers": ["mpegts", "mp4"]
    })
}

async fn open_session(client: &TestClient, channel_id: Uuid, caps: Value) -> Value {
    let resp = client
        .post_json(
            "/api/v1/admin/livetv/sessions",
            &json!({ "channel_id": channel_id, "capabilities": caps }),
        )
        .await;
    resp.assert_ok();
    resp.json()
}

fn lease_req(tuner_id: Uuid, channel_id: Uuid, key: &str, kind: HolderKind) -> LeaseRequest {
    LeaseRequest {
        tuner_id,
        channel_id,
        share_key: key.to_string(),
        capacity: 4,
        holder_kind: kind,
        session_id: None,
        recording_id: None,
        user_id: None,
    }
}

// =============================================================================
// Lease registry — the four hard slots
// =============================================================================

#[tokio::test]
async fn two_holders_of_one_share_key_share_a_single_slot() {
    let leases = TunerLeases::detached();
    let tuner = Uuid::new_v4();
    let channel = Uuid::new_v4();
    let a = leases
        .acquire(lease_req(tuner, channel, "t:a", HolderKind::Viewer))
        .unwrap();
    let b = leases
        .acquire(lease_req(tuner, channel, "t:a", HolderKind::Viewer))
        .unwrap();
    assert_eq!(a.id(), b.id());
    assert_eq!(leases.in_use(tuner), 1);
}

#[tokio::test]
async fn a_fifth_stream_is_refused_not_degraded() {
    let leases = TunerLeases::detached();
    let tuner = Uuid::new_v4();
    let mut held = Vec::new();
    for i in 0..4 {
        held.push(
            leases
                .acquire(lease_req(
                    tuner,
                    Uuid::new_v4(),
                    &format!("t:{i}"),
                    HolderKind::Viewer,
                ))
                .unwrap(),
        );
    }
    assert_eq!(leases.in_use(tuner), 4);
    let err = leases
        .acquire(lease_req(tuner, Uuid::new_v4(), "t:5", HolderKind::Viewer))
        .unwrap_err();
    assert!(err.to_string().contains("4 tuners"), "{err}");
    assert_eq!(leases.in_use(tuner), 4);
}

#[tokio::test]
async fn a_waiting_acquire_reaps_the_linger_immediately() {
    let leases = TunerLeases::detached();
    let tuner = Uuid::new_v4();
    let mut held = Vec::new();
    for i in 0..4 {
        held.push(
            leases
                .acquire(lease_req(
                    tuner,
                    Uuid::new_v4(),
                    &format!("t:{i}"),
                    HolderKind::Viewer,
                ))
                .unwrap(),
        );
    }
    // Channel change: the old lease lingers so flicking back is instant.
    drop(held.pop().unwrap());
    assert_eq!(leases.in_use(tuner), 4, "linger keeps the slot warm");
    let fresh = leases
        .acquire(lease_req(
            tuner,
            Uuid::new_v4(),
            "t:new",
            HolderKind::Viewer,
        ))
        .unwrap();
    assert_eq!(leases.in_use(tuner), 4);
    drop(fresh);
}

#[tokio::test]
async fn a_recording_outranks_a_viewer() {
    let leases = TunerLeases::detached();
    let tuner = Uuid::new_v4();
    let mut held = Vec::new();
    for i in 0..4 {
        held.push(
            leases
                .acquire(lease_req(
                    tuner,
                    Uuid::new_v4(),
                    &format!("t:{i}"),
                    HolderKind::Viewer,
                ))
                .unwrap(),
        );
    }
    let recording_id = Uuid::new_v4();
    let mut req = lease_req(tuner, Uuid::new_v4(), "t:rec", HolderKind::Recording);
    req.recording_id = Some(recording_id);
    let rec = leases.acquire(req).unwrap();
    assert_eq!(leases.in_use(tuner), 4);
    let evicted: Vec<_> = held.iter().filter(|t| t.is_evicted()).collect();
    assert_eq!(evicted.len(), 1, "exactly one viewer loses its slot");
    assert_eq!(evicted[0].flags.preempted_by(), Some(recording_id));
    drop(rec);
}

#[tokio::test]
async fn a_recording_on_the_same_channel_is_not_a_conflict() {
    let leases = TunerLeases::detached();
    let tuner = Uuid::new_v4();
    let channel = Uuid::new_v4();
    let viewer = leases
        .acquire(lease_req(tuner, channel, "t:same", HolderKind::Viewer))
        .unwrap();
    let mut req = lease_req(tuner, channel, "t:same", HolderKind::Recording);
    req.recording_id = Some(Uuid::new_v4());
    let rec = leases.acquire(req).unwrap();
    assert_eq!(leases.in_use(tuner), 1, "the buffer just keeps growing");
    assert!(!viewer.is_evicted());
    assert_eq!(rec.info.holder_kind.as_str(), "recording");
}

// =============================================================================
// Scan-time facts feed the decision engine — no probe
// =============================================================================

#[tokio::test]
async fn stream_facts_come_off_the_channel_row() {
    let server = TestServer::start_api_only().await;
    let tuner_id = make_tuner(&server.pool, "FACTS001", 4).await;
    let hd = make_channel(&server.pool, tuner_id, 7, true).await;
    let sd = make_channel(&server.pool, tuner_id, 1, false).await;

    let hd_row = livetv_channels::get_by_id(&server.pool, hd)
        .await
        .unwrap()
        .unwrap();
    let facts = live_session::stream_facts(&hd_row);
    assert_eq!(facts.video_codec, "h264");
    assert_eq!(facts.audio_codec, "ac3");
    assert_eq!((facts.width, facts.height), (1920, 1080));
    assert!(facts.interlaced, "Australian DVB-T is field-coded");

    let sd_row = livetv_channels::get_by_id(&server.pool, sd)
        .await
        .unwrap()
        .unwrap();
    let facts = live_session::stream_facts(&sd_row);
    assert_eq!(facts.video_codec, "mpeg2video");
    assert_eq!(facts.audio_codec, "mp2");
    assert_eq!((facts.width, facts.height), (720, 576));
}

#[test]
fn the_stream_url_is_the_devices_own_auto_path() {
    assert_eq!(
        live_session::tuner_stream_url("http://192.0.2.10", 7),
        "http://192.0.2.10:5004/auto/v7"
    );
    assert_eq!(
        live_session::tuner_stream_url("192.0.2.10:80/", 22),
        "http://192.0.2.10:5004/auto/v22"
    );
}

// =============================================================================
// Sessions
// =============================================================================

#[tokio::test]
async fn creating_a_session_does_not_probe_the_stream() {
    let server = TestServer::start_api_only().await;
    let client = server.admin_client();
    let tuner_id = make_tuner(&server.pool, "SESS0001", 4).await;
    let channel = make_channel(&server.pool, tuner_id, 7, true).await;

    let started = std::time::Instant::now();
    let body = open_session(&client, channel, browser_caps()).await;
    let elapsed = started.elapsed();

    // The device is unroutable here, so anything that waited on it would hang.
    // A probe would also burn up to 2s of a 1.2-1.8s budget.
    assert!(
        elapsed < std::time::Duration::from_millis(1500),
        "session creation took {elapsed:?}"
    );
    assert_eq!(body["method"], "transcode");
    assert!(body["playlist_url"].is_string());
    assert!(body["direct_url"].is_null());
    assert_eq!(body["video_codec"], "h264");
    assert_eq!(body["audio_codec"], "ac3");
    assert_eq!(body["hd"], true);
    assert_eq!(body["interlaced"], true);
    assert_eq!(body["is_live"], true);
    assert!(
        body["thumbnails_url"]
            .as_str()
            .is_some_and(|u| u.ends_with("/thumbnails.vtt")),
        "the scrubber needs somewhere to fetch buffer thumbnails from"
    );
    assert_eq!(body["duration_seconds"], 0.0);
    assert_eq!(body["tuners_in_use"], 1);
    assert_eq!(body["tuner_count"], 4);
    assert_eq!(body["lease"]["holder_kind"], "viewer");
    assert!(body["buffer_start_time"].is_string());
}

/// The trickplay pass needs a few closed segments before it emits anything, so the
/// first minute of a tune has no VTT. The client polls, so this has to be an ordinary
/// 404 rather than an error that kills the poll.
#[tokio::test]
async fn buffer_thumbnails_are_missing_until_the_pass_has_run() {
    let server = TestServer::start_api_only().await;
    let client = server.admin_client();
    let tuner_id = make_tuner(&server.pool, "THUMB001", 4).await;
    let channel = make_channel(&server.pool, tuner_id, 7, true).await;

    let body = open_session(&client, channel, exoplayer_caps()).await;
    let url = body["thumbnails_url"].as_str().unwrap();

    client.get(url).await.assert_status(StatusCode::NOT_FOUND);
}

/// Both clients resolve a sprite ref the same way: strip `.vtt` off the VTT URL and
/// treat the stem as a directory. A served cue that does not land on the sprite route
/// under that rule is a silent 404 the scrubber never reports.
#[tokio::test]
async fn a_served_sprite_ref_resolves_onto_the_sprite_route() {
    let server = TestServer::start_api_only().await;
    let client = server.admin_client();
    let tuner_id = make_tuner(&server.pool, "THUMB002", 4).await;
    let channel = make_channel(&server.pool, tuner_id, 7, true).await;

    let body = open_session(&client, channel, exoplayer_caps()).await;
    let sid = body["session_id"].as_str().unwrap();
    let vtt_url = body["thumbnails_url"].as_str().unwrap().to_string();

    let dir = live_session::buffer_dir_of(&server.app_state, sid)
        .expect("the tune owns a buffer dir")
        .join("trickplay");
    tokio::fs::create_dir_all(&dir).await.unwrap();
    tokio::fs::write(
        dir.join("thumbnails.vtt"),
        "WEBVTT\n\n00:00:00.000 --> 00:00:05.000\nsprite_001.jpg#xywh=0,0,320,180\n\n",
    )
    .await
    .unwrap();
    tokio::fs::write(dir.join("sprite_001.jpg"), b"jpeg")
        .await
        .unwrap();

    let resp = client.get(&vtt_url).await;
    resp.assert_ok();
    let vtt = resp.text();
    let cue = vtt
        .lines()
        .find(|l| l.contains("sprite_"))
        .expect("the served vtt names a sprite");
    let sprite_ref = cue.split('#').next().unwrap();

    let base = vtt_url.trim_end_matches(".vtt");
    client
        .get(&format!("{base}/{sprite_ref}"))
        .await
        .assert_ok();
}

#[tokio::test]
async fn a_client_that_reads_mpegts_gets_the_raw_transport_stream() {
    let server = TestServer::start_api_only().await;
    let client = server.admin_client();
    let tuner_id = make_tuner(&server.pool, "DIRECT01", 4).await;
    let channel = make_channel(&server.pool, tuner_id, 7, true).await;

    let body = open_session(&client, channel, exoplayer_caps()).await;
    assert_eq!(body["method"], "direct");
    let direct = body["direct_url"].as_str().unwrap();
    assert!(direct.contains(&format!("/livetv/channels/{channel}/stream.ts")));
    assert!(direct.contains("session="));

    // The buffer runs for a direct tune too, so the playlist comes with it — the
    // raw stream is the live edge and the playlist is everything behind it. Sending
    // only the direct URL left Restart and the rewind step permanently disabled,
    // because the client had nothing to seek in.
    let playlist = body["playlist_url"].as_str().unwrap();
    assert!(playlist.ends_with("/stream.m3u8"), "got {playlist}");

    // The Tensor VPU decodes the sequence header of a 1080i broadcast fine and then
    // errors on every field picture, so a DirectPlay client has to know to keep the
    // stream off its hardware decoder. Nothing else in the payload tells it.
    assert_eq!(body["interlaced"], true);
    assert_eq!(body["video_codec"], "h264");
}

#[tokio::test]
async fn a_client_whose_decoder_cannot_show_fields_is_sent_the_deinterlaced_encode() {
    let server = TestServer::start_api_only().await;
    let client = server.admin_client();
    let tuner_id = make_tuner(&server.pool, "FIELDS01", 4).await;
    let channel = make_channel(&server.pool, tuner_id, 7, true).await;

    let mut caps = exoplayer_caps();
    caps["supports_interlaced"] = json!(false);
    let body = open_session(&client, channel, caps).await;
    assert_eq!(body["method"], "transcode");
    assert!(body["playlist_url"].is_string());
    assert!(body["direct_url"].is_null());
}

/// One physical tune serves every consumer of a channel: two DirectPlay clients,
/// a browser and a recording all share one lease and one buffer, and the buffer
/// exists for the direct path too — which is what makes pause, Restart and a
/// retroactive Record work on kobapp.
#[tokio::test]
async fn direct_viewers_share_one_tune_with_each_other_a_browser_and_a_recording() {
    let server = TestServer::start_api_only().await;
    let client = server.admin_client();
    let tuner_id = make_tuner(&server.pool, "DIRECT02", 1).await;
    let channel = make_channel(&server.pool, tuner_id, 7, true).await;

    let first = open_session(&client, channel, exoplayer_caps()).await;
    let second = open_session(&client, channel, exoplayer_caps()).await;
    assert_eq!(first["method"], "direct");
    assert_eq!(second["method"], "direct");
    assert_eq!(
        second["tuners_in_use"], 1,
        "two direct viewers are one physical tune"
    );

    // The one remaining slot is not taken by a browser on the same channel either.
    let web = open_session(&client, channel, browser_caps()).await;
    assert_eq!(web["method"], "transcode");
    assert_eq!(web["tuners_in_use"], 1);

    let sid = first["session_id"].as_str().unwrap();
    let buffer: Value = client
        .get(&format!("/api/v1/admin/livetv/sessions/{sid}/buffer"))
        .await
        .assert_ok()
        .json();
    assert!(
        buffer["start_time"].is_string(),
        "a direct session has a truthful buffer window"
    );
    assert_eq!(buffer["can_jump_to_live"], true);

    // Recording the channel somebody is already watching is not a conflict.
    let recording: Value = client
        .post_json(
            &format!("/api/v1/admin/livetv/sessions/{sid}/record"),
            &json!({ "mode": "open_ended" }),
        )
        .await
        .assert_ok()
        .json();
    assert_eq!(recording["status"], "recording");

    let leases: Value = client.get("/api/v1/admin/livetv/leases").await.json();
    assert_eq!(leases["in_use"], 1, "still one tuner");
    assert_eq!(leases["items"][0]["holder_kind"], "recording");
}

#[tokio::test]
async fn two_sessions_on_one_channel_share_a_lease_and_a_fifth_channel_is_refused() {
    let server = TestServer::start_api_only().await;
    let client = server.admin_client();
    let tuner_id = make_tuner(&server.pool, "LEASE001", 4).await;
    let channels: Vec<Uuid> = {
        let mut v = Vec::new();
        for lcn in [7, 9, 10, 2, 3] {
            v.push(make_channel(&server.pool, tuner_id, lcn, true).await);
        }
        v
    };

    open_session(&client, channels[0], browser_caps()).await;
    let second = open_session(&client, channels[0], browser_caps()).await;
    assert_eq!(second["tuners_in_use"], 1, "same channel, same tune");

    let leases: Value = client.get("/api/v1/admin/livetv/leases").await.json();
    assert_eq!(leases["in_use"], 1);
    assert_eq!(leases["total"], 4);
    assert_eq!(leases["items"][0]["channel_name"], "Channel 7");

    for ch in &channels[1..4] {
        open_session(&client, *ch, browser_caps()).await;
    }
    let leases: Value = client.get("/api/v1/admin/livetv/leases").await.json();
    assert_eq!(leases["in_use"], 4);

    let resp = client
        .post_json(
            "/api/v1/admin/livetv/sessions",
            &json!({ "channel_id": channels[4], "capabilities": browser_caps() }),
        )
        .await;
    resp.assert_status(StatusCode::BAD_REQUEST);
    let err: Value = resp.json();
    assert!(
        err["error"].as_str().unwrap().contains("4 tuners"),
        "{err:?}"
    );
}

#[tokio::test]
async fn killing_a_session_frees_its_slot() {
    let server = TestServer::start_api_only().await;
    let client = server.admin_client();
    let tuner_id = make_tuner(&server.pool, "KILL0001", 2).await;
    let a = make_channel(&server.pool, tuner_id, 7, true).await;
    let b = make_channel(&server.pool, tuner_id, 9, true).await;
    let c = make_channel(&server.pool, tuner_id, 10, true).await;

    let s1 = open_session(&client, a, browser_caps()).await;
    open_session(&client, b, browser_caps()).await;

    let resp = client
        .post_json(
            "/api/v1/admin/livetv/sessions",
            &json!({ "channel_id": c, "capabilities": browser_caps() }),
        )
        .await;
    resp.assert_status(StatusCode::BAD_REQUEST);

    let sid = s1["session_id"].as_str().unwrap();
    client
        .delete(&format!("/api/v1/admin/livetv/sessions/{sid}"))
        .await
        .assert_status(StatusCode::NO_CONTENT);

    open_session(&client, c, browser_caps()).await;

    client
        .delete(&format!("/api/v1/admin/livetv/sessions/{sid}"))
        .await
        .assert_status(StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn a_channel_change_and_a_session_end_each_write_a_watch_event() {
    let server = TestServer::start_api_only().await;
    let client = server.admin_client();
    let tuner_id = make_tuner(&server.pool, "WATCH001", 4).await;
    let a = make_channel(&server.pool, tuner_id, 7, true).await;
    let b = make_channel(&server.pool, tuner_id, 9, true).await;

    let s = open_session(&client, a, browser_caps()).await;
    let sid = s["session_id"].as_str().unwrap();

    let resp = client
        .post_json(
            &format!("/api/v1/admin/livetv/sessions/{sid}/channel"),
            &json!({ "channel_id": b, "capabilities": browser_caps() }),
        )
        .await;
    resp.assert_ok();
    let switched: Value = resp.json();
    assert_eq!(switched["channel_id"], b.to_string());
    assert_eq!(switched["session_id"], sid);

    assert_eq!(
        api::repositories::livetv_watch_events::count_for_channel(&server.pool, a)
            .await
            .unwrap(),
        1
    );

    client
        .delete(&format!("/api/v1/admin/livetv/sessions/{sid}"))
        .await
        .assert_status(StatusCode::NO_CONTENT);
    assert_eq!(
        api::repositories::livetv_watch_events::count_for_channel(&server.pool, b)
            .await
            .unwrap(),
        1
    );
}

async fn switch(client: &TestClient, sid: &str, channel_id: Uuid) -> Value {
    let resp = client
        .post_json(
            &format!("/api/v1/admin/livetv/sessions/{sid}/channel"),
            &json!({ "channel_id": channel_id, "capabilities": browser_caps() }),
        )
        .await;
    resp.assert_ok();
    resp.json()
}

/// A flick back to the channel just left revives that channel's lingering lease, and the
/// buffer directory is named after the lease — so the returning tune landed on the very
/// directory the outgoing tune's discard was still deleting in the background, and the
/// segmenter being killed could drop a stray segment into it.
#[tokio::test]
async fn a_flick_back_does_not_land_on_the_buffer_being_discarded() {
    let server = TestServer::start_api_only().await;
    let client = server.admin_client();
    let tuner_id = make_tuner(&server.pool, "FLICKBAK", 2).await;
    let a = make_channel(&server.pool, tuner_id, 7, true).await;
    let b = make_channel(&server.pool, tuner_id, 9, true).await;

    let s = open_session(&client, a, browser_caps()).await;
    let sid = s["session_id"].as_str().unwrap();
    let first = live_session::buffer_dir_of(&server.app_state, sid).expect("a buffer directory");

    switch(&client, sid, b).await;
    switch(&client, sid, a).await;

    let back = live_session::buffer_dir_of(&server.app_state, sid).expect("a buffer directory");
    assert_ne!(
        first, back,
        "the flick back reused the buffer directory being discarded"
    );
}

/// The switch tunes the new channel before letting the old one go, so the tuner behind
/// the picture the viewer is still watching stays up until there is something to cut to.
/// On a device with nothing spare that overlap is impossible, and the old tune is the
/// only free slot there is — so it has to go first rather than the switch failing.
#[tokio::test]
async fn a_switch_with_every_tuner_busy_lets_the_old_channel_go_first() {
    let server = TestServer::start_api_only().await;
    let client = server.admin_client();
    let tuner_id = make_tuner(&server.pool, "SWAPFULL", 2).await;
    let a = make_channel(&server.pool, tuner_id, 7, true).await;
    let b = make_channel(&server.pool, tuner_id, 9, true).await;
    let c = make_channel(&server.pool, tuner_id, 10, true).await;
    let d = make_channel(&server.pool, tuner_id, 11, true).await;

    let s = open_session(&client, a, browser_caps()).await;
    let sid = s["session_id"].as_str().unwrap();
    open_session(&client, b, browser_caps()).await;

    let switched = switch(&client, sid, c).await;
    assert_eq!(switched["channel_id"], c.to_string());

    // And the slot the old channel was holding is genuinely gone, not merely overlapped:
    // c and b are the two tunes, so a third channel is still refused.
    client
        .post_json(
            "/api/v1/admin/livetv/sessions",
            &json!({ "channel_id": d, "capabilities": browser_caps() }),
        )
        .await
        .assert_status(StatusCode::BAD_REQUEST);
}

/// The overlap must not leave the outgoing tune behind. With a slot to spare the new
/// channel is tuned first, and a tune that is never detached is a tuner lost per press.
#[tokio::test]
async fn a_switch_with_a_slot_to_spare_still_gives_the_old_tuner_back() {
    let server = TestServer::start_api_only().await;
    let client = server.admin_client();
    let tuner_id = make_tuner(&server.pool, "SWAPFREE", 2).await;
    let a = make_channel(&server.pool, tuner_id, 7, true).await;
    let b = make_channel(&server.pool, tuner_id, 9, true).await;
    let c = make_channel(&server.pool, tuner_id, 10, true).await;

    let s = open_session(&client, a, browser_caps()).await;
    let sid = s["session_id"].as_str().unwrap();

    switch(&client, sid, b).await;

    // Two slots, one of them now on b: a second channel still fits. The tune the switch
    // left behind lingers so a flick back is instant, but a lingering lease gives way to
    // anything waiting — one that is still held does not.
    open_session(&client, c, browser_caps()).await;
}

#[tokio::test]
async fn heartbeat_and_buffer_report_the_retention_cap() {
    let server = TestServer::start_api_only().await;
    let client = server.admin_client();
    let tuner_id = make_tuner(&server.pool, "BUFF0001", 4).await;
    let channel = make_channel(&server.pool, tuner_id, 7, true).await;
    let s = open_session(&client, channel, browser_caps()).await;
    let sid = s["session_id"].as_str().unwrap();

    let resp = client
        .post_json(
            &format!("/api/v1/admin/livetv/sessions/{sid}/heartbeat"),
            &json!({ "position_seconds": 12.5, "source": "buffer" }),
        )
        .await;
    resp.assert_ok();
    assert_eq!(resp.json::<Value>()["ok"], true);

    let resp = client
        .get(&format!("/api/v1/admin/livetv/sessions/{sid}/buffer"))
        .await;
    resp.assert_ok();
    let body: Value = resp.json();
    assert_eq!(body["cap_seconds"], 3 * 3600);
    assert_eq!(body["preempted"], false);
    assert_eq!(body["can_jump_to_live"], true);
    assert!(body["start_time"].is_string());

    client
        .post_json(
            &format!("/api/v1/admin/livetv/sessions/{}/heartbeat", Uuid::new_v4()),
            &json!({}),
        )
        .await
        .assert_status(StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn record_promotes_the_existing_buffer_retroactively() {
    let server = TestServer::start_api_only().await;
    let client = server.admin_client();
    let tuner_id = make_tuner(&server.pool, "REC00001", 4).await;
    let channel = make_channel(&server.pool, tuner_id, 7, true).await;
    let s = open_session(&client, channel, browser_caps()).await;
    let sid = s["session_id"].as_str().unwrap();

    let resp = client
        .post_json(
            &format!("/api/v1/admin/livetv/sessions/{sid}/record"),
            &json!({ "mode": "open_ended" }),
        )
        .await;
    resp.assert_ok();
    let rec: Value = resp.json();
    assert_eq!(rec["status"], "recording");
    assert_eq!(rec["channel_id"], channel.to_string());
    assert_eq!(rec["container"], "hls");
    assert_eq!(rec["is_in_progress"], true);
    assert_eq!(rec["playable_from_start"], true);
    // Retroactive: the recording starts where the buffer starts, not now.
    assert_eq!(rec["actual_start"], rec["scheduled_start"]);
    assert_eq!(rec["scheduled_start"], s["buffer_start_time"]);
    assert_eq!(rec["provenance"]["lcn"], 7);

    let session = api::repositories::livetv_sessions::get_by_session_id(&server.pool, sid)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        session.recording_id.map(|id| id.to_string()),
        rec["id"].as_str().map(str::to_string)
    );
    assert_eq!(session.buffer_dir, rec["output_dir"].as_str().unwrap());

    // The lease is a recording's now, so it stops being evictable.
    let leases: Value = client.get("/api/v1/admin/livetv/leases").await.json();
    assert_eq!(leases["items"][0]["holder_kind"], "recording");

    // Pressing it twice returns the same recording rather than making a second.
    let again: Value = client
        .post_json(
            &format!("/api/v1/admin/livetv/sessions/{sid}/record"),
            &json!({ "mode": "open_ended" }),
        )
        .await
        .json();
    assert_eq!(again["id"], rec["id"]);
}

/// The in-player Record button sends the session's `now` guide entry, which is read once
/// at tune-in. Forty minutes into a watch that entry has finished, and scheduling a
/// recording to end at its end time ends it before it starts.
#[tokio::test]
async fn recording_a_programme_that_already_ended_does_not_schedule_the_end_in_the_past() {
    let server = TestServer::start_api_only().await;
    let client = server.admin_client();
    let tuner_id = make_tuner(&server.pool, "RECSTALE", 4).await;
    let channel = make_channel(&server.pool, tuner_id, 7, true).await;

    let now = OffsetDateTime::now_utc();
    let stale = api::repositories::livetv_guide::upsert(
        &server.pool,
        api::repositories::livetv_guide::UpsertGuideEntry {
            channel_id: channel,
            start_time: now - time::Duration::minutes(90),
            end_time: now - time::Duration::minutes(30),
            title: "The programme that was on when we tuned in".to_string(),
            episode_number: None,
            episode_title: None,
            synopsis: None,
            original_airdate: None,
            series_id: None,
            genres: vec![],
            image_url: None,
            poster_url: None,
            is_first: false,
            episode_key: None,
            episode_key_kind: None,
        },
    )
    .await
    .unwrap();

    let s = open_session(&client, channel, browser_caps()).await;
    let sid = s["session_id"].as_str().unwrap();

    let resp = client
        .post_json(
            &format!("/api/v1/admin/livetv/sessions/{sid}/record"),
            &json!({ "mode": "programme", "guide_id": stale }),
        )
        .await;
    resp.assert_ok();
    let rec: Value = resp.json();

    let scheduled_end = OffsetDateTime::parse(
        rec["scheduled_end"].as_str().unwrap(),
        &time::format_description::well_known::Rfc3339,
    )
    .unwrap();
    assert!(
        scheduled_end > now,
        "a recording that starts already finished captures nothing; scheduled_end was {scheduled_end}"
    );
}

#[tokio::test]
async fn record_rejects_an_unknown_mode_and_an_unknown_session() {
    let server = TestServer::start_api_only().await;
    let client = server.admin_client();
    let tuner_id = make_tuner(&server.pool, "REC00002", 4).await;
    let channel = make_channel(&server.pool, tuner_id, 7, true).await;
    let s = open_session(&client, channel, browser_caps()).await;
    let sid = s["session_id"].as_str().unwrap();

    client
        .post_json(
            &format!("/api/v1/admin/livetv/sessions/{sid}/record"),
            &json!({ "mode": "whenever" }),
        )
        .await
        .assert_status(StatusCode::BAD_REQUEST);

    client
        .post_json(
            &format!("/api/v1/admin/livetv/sessions/{}/record", Uuid::new_v4()),
            &json!({ "mode": "open_ended" }),
        )
        .await
        .assert_status(StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn a_channel_the_device_cannot_tune_is_a_400_and_an_unknown_channel_a_404() {
    let server = TestServer::start_api_only().await;
    let client = server.admin_client();
    let tuner_id = make_tuner(&server.pool, "TUNE0001", 4).await;
    let untunable = livetv_channels::upsert_scan(
        &server.pool,
        livetv_channels::UpsertScanChannel {
            tuner_id,
            lcn: None,
            scan_name: Some("No LCN".to_string()),
            frequency: 191_625_000,
            program_number: 4242,
            onid: None,
            tsid: None,
            modulation: None,
            video_codec: None,
            audio_codec: None,
            hd: false,
            signal_strength: None,
            signal_quality: None,
            initial_kind: "tv".to_string(),
            initial_enabled: false,
        },
    )
    .await
    .unwrap()
    .0
    .id;

    client
        .post_json(
            "/api/v1/admin/livetv/sessions",
            &json!({ "channel_id": untunable, "capabilities": browser_caps() }),
        )
        .await
        .assert_status(StatusCode::BAD_REQUEST);

    client
        .post_json(
            "/api/v1/admin/livetv/sessions",
            &json!({ "channel_id": Uuid::new_v4(), "capabilities": browser_caps() }),
        )
        .await
        .assert_status(StatusCode::NOT_FOUND);
}

// =============================================================================
// The segment directory IS the buffer
// =============================================================================

#[tokio::test]
async fn the_served_playlist_drops_reaped_segments_and_rebases_the_sequence() {
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path();
    std::fs::write(p.join("init.mp4"), b"init").unwrap();
    for i in 2..=4 {
        std::fs::write(p.join(format!("seg_{i:05}.m4s")), b"x").unwrap();
    }
    std::fs::write(
        p.join("stream.m3u8"),
        concat!(
            "#EXTM3U\n",
            "#EXT-X-VERSION:7\n",
            "#EXT-X-TARGETDURATION:1\n",
            "#EXT-X-MEDIA-SEQUENCE:0\n",
            "#EXT-X-PLAYLIST-TYPE:EVENT\n",
            "#EXT-X-MAP:URI=\"init.mp4\"\n",
            "#EXTINF:1.000000,\nseg_00000.m4s\n",
            "#EXTINF:1.000000,\nseg_00001.m4s\n",
            "#EXTINF:1.000000,\nseg_00002.m4s\n",
            "#EXTINF:1.000000,\nseg_00003.m4s\n",
            "#EXTINF:1.000000,\nseg_00004.m4s\n",
        ),
    )
    .unwrap();

    let (playlist, status) = live_session::read_buffer(p).await.unwrap();
    assert_eq!(status.segments, 3);
    assert!((status.duration_seconds - 3.0).abs() < 0.001);
    assert_eq!(status.first_segment_index, 2);
    assert!(playlist.contains("#EXT-X-MEDIA-SEQUENCE:2"));
    assert!(playlist.contains("#EXT-X-MAP:URI=\"segments/init.mp4\""));
    assert!(playlist.contains("segments/seg_00002.m4s"));
    assert!(!playlist.contains("seg_00000.m4s"));
    // EVENT forbids removing from the front, and retention does exactly that.
    assert!(!playlist.contains("EXT-X-PLAYLIST-TYPE"));
    assert!(!playlist.contains("ENDLIST"));
}

/// ffmpeg's live DVR playlist is not legal HLS on its own: it declares
/// `TARGETDURATION:0`, opens with a `DISCONTINUITY` that precedes every
/// segment, and orders `MEDIA-SEQUENCE` after `EXT-X-MAP`. Browsers tolerate
/// all three; a strict player does not.
#[tokio::test]
async fn the_served_playlist_is_legal_hls() {
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path();
    std::fs::write(p.join("init.mp4"), b"init").unwrap();
    for i in 0..=2 {
        std::fs::write(p.join(format!("seg_{i:05}.m4s")), b"x").unwrap();
    }
    std::fs::write(
        p.join("stream.m3u8"),
        concat!(
            "#EXTM3U\n",
            "#EXT-X-VERSION:7\n",
            "#EXT-X-TARGETDURATION:0\n",
            "#EXT-X-INDEPENDENT-SEGMENTS\n",
            "#EXT-X-MAP:URI=\"init.mp4\"\n",
            "#EXT-X-MEDIA-SEQUENCE:0\n",
            "#EXT-X-DISCONTINUITY\n",
            "#EXTINF:1.041667,\nseg_00000.m4s\n",
            "#EXTINF:2.500000,\nseg_00001.m4s\n",
            "#EXTINF:1.000000,\nseg_00002.m4s\n",
        ),
    )
    .unwrap();

    let (playlist, status) = live_session::read_buffer(p).await.unwrap();
    assert_eq!(status.segments, 3);

    let lines: Vec<&str> = playlist.lines().collect();
    let target = lines
        .iter()
        .find_map(|l| l.strip_prefix("#EXT-X-TARGETDURATION:"))
        .expect("a target duration")
        .parse::<u64>()
        .unwrap();
    assert!(
        target >= 3,
        "TARGETDURATION {target} is below the 2.5s EXTINF"
    );

    let first_segment = lines
        .iter()
        .position(|l| !l.starts_with('#') && !l.trim().is_empty())
        .expect("a segment");
    assert!(
        !lines[..first_segment].contains(&"#EXT-X-DISCONTINUITY"),
        "a discontinuity before the first segment is meaningless: {playlist}"
    );

    let seq = lines
        .iter()
        .position(|l| l.starts_with("#EXT-X-MEDIA-SEQUENCE:"))
        .expect("a media sequence");
    let map = lines
        .iter()
        .position(|l| l.starts_with("#EXT-X-MAP:"))
        .expect("a map");
    assert!(
        seq < map,
        "MEDIA-SEQUENCE must precede EXT-X-MAP: {playlist}"
    );
    assert!(playlist.contains("#EXT-X-MAP:URI=\"segments/init.mp4\""));
    assert!(playlist.contains("#EXT-X-INDEPENDENT-SEGMENTS"));
}

#[tokio::test]
async fn retention_reaps_only_segments_past_the_cap() {
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path();
    std::fs::write(p.join("seg_00000.m4s"), b"old").unwrap();
    std::fs::write(p.join("seg_00001.m4s"), b"new").unwrap();
    std::fs::write(p.join("stream.m3u8"), b"#EXTM3U\n").unwrap();

    let old = std::time::SystemTime::now() - std::time::Duration::from_secs(7200);
    let f = std::fs::File::options()
        .write(true)
        .open(p.join("seg_00000.m4s"))
        .unwrap();
    f.set_modified(old).unwrap();

    let removed = live_session::enforce_retention(p, std::time::Duration::from_secs(3600)).await;
    assert_eq!(removed, 1);
    assert!(!p.join("seg_00000.m4s").exists());
    assert!(p.join("seg_00001.m4s").exists());
    assert!(
        p.join("stream.m3u8").exists(),
        "the playlist is not a segment"
    );
}

// =============================================================================
// Restart
// =============================================================================

#[tokio::test]
async fn the_boot_reconciler_releases_orphans_and_deletes_stray_buffers() {
    let server = TestServer::start_api_only().await;
    let tuner_id = make_tuner(&server.pool, "BOOT0001", 4).await;
    let channel = make_channel(&server.pool, tuner_id, 7, true).await;

    let root = tempfile::tempdir().unwrap();
    let kept = root.path().join("kept");
    let stray = root.path().join("stray");
    std::fs::create_dir_all(&kept).unwrap();
    std::fs::create_dir_all(&stray).unwrap();

    let lease_id = Uuid::now_v7();
    api::repositories::livetv_leases::create(
        &server.pool,
        api::repositories::livetv_leases::CreateLease {
            id: lease_id,
            tuner_id,
            channel_id: channel,
            tuner_index: Some(0),
            holder_kind: "viewer".to_string(),
            session_id: Some("orphan-session".to_string()),
            recording_id: None,
            user_id: None,
            buffer_dir: Some(kept.to_string_lossy().to_string()),
        },
    )
    .await
    .unwrap();
    api::repositories::livetv_sessions::create(
        &server.pool,
        api::repositories::livetv_sessions::CreateSession {
            session_id: "orphan-session".to_string(),
            channel_id: channel,
            lease_id: Some(lease_id),
            user_id: None,
            mode: "transcode".to_string(),
            buffer_dir: kept.to_string_lossy().to_string(),
        },
    )
    .await
    .unwrap();

    let report = tuner_lease::reconcile_on_boot(&server.pool, root.path())
        .await
        .unwrap();
    assert_eq!(report.released_leases, 1);
    assert_eq!(report.ended_sessions, 1);
    assert_eq!(report.removed_buffer_dirs, 1);
    assert!(kept.exists(), "a dir with a session row survives");
    assert!(!stray.exists(), "a dir with no session row is disposable");

    assert!(
        api::repositories::livetv_leases::list_active(&server.pool)
            .await
            .unwrap()
            .is_empty()
    );
}

#[tokio::test]
async fn the_sweeper_keeps_live_recorded_and_in_flight_buffers_only() {
    let server = TestServer::start_api_only().await;
    let tuner_id = make_tuner(&server.pool, "SWEEP001", 4).await;
    let channel = make_channel(&server.pool, tuner_id, 7, true).await;

    let root = tempfile::tempdir().unwrap();
    let mut dirs = std::collections::HashMap::new();
    for name in ["active", "recorded", "in_flight", "stray"] {
        let p = root.path().join(name);
        std::fs::create_dir_all(&p).unwrap();
        dirs.insert(name, p);
    }

    api::repositories::livetv_sessions::create(
        &server.pool,
        api::repositories::livetv_sessions::CreateSession {
            session_id: "sweep-active".to_string(),
            channel_id: channel,
            lease_id: None,
            user_id: None,
            mode: "transcode".to_string(),
            buffer_dir: dirs["active"].to_string_lossy().to_string(),
        },
    )
    .await
    .unwrap();

    api::repositories::livetv_sessions::create(
        &server.pool,
        api::repositories::livetv_sessions::CreateSession {
            session_id: "sweep-recorded".to_string(),
            channel_id: channel,
            lease_id: None,
            user_id: None,
            mode: "transcode".to_string(),
            buffer_dir: dirs["recorded"].to_string_lossy().to_string(),
        },
    )
    .await
    .unwrap();
    let recording = api::repositories::livetv_recordings::create(
        &server.pool,
        api::repositories::livetv_recordings::CreateRecording {
            channel_id: channel,
            guide_id: None,
            series_rule_id: None,
            title: "Promoted".to_string(),
            episode_number: None,
            episode_title: None,
            episode_key: None,
            series_id: None,
            synopsis: None,
            scheduled_start: OffsetDateTime::now_utc(),
            scheduled_end: OffsetDateTime::now_utc(),
            actual_start: None,
            status: "recording".to_string(),
            output_dir: Some(dirs["recorded"].to_string_lossy().to_string()),
            container: Some("hls".to_string()),
            video_codec: None,
            audio_codec: None,
            padding_before_secs: 0,
            padding_after_secs: 0,
            image_url: None,
        },
    )
    .await
    .unwrap();
    api::repositories::livetv_sessions::set_recording(&server.pool, "sweep-recorded", recording.id)
        .await
        .unwrap();
    api::repositories::livetv_sessions::end(&server.pool, "sweep-recorded")
        .await
        .unwrap();

    let keep: std::collections::HashSet<std::path::PathBuf> =
        [dirs["in_flight"].clone()].into_iter().collect();
    let removed = tuner_lease::sweep_orphan_buffers(&server.pool, root.path(), &keep).await;

    assert_eq!(removed, 1);
    assert!(dirs["active"].exists(), "a live session's buffer stays");
    assert!(
        dirs["recorded"].exists(),
        "a promoted buffer IS the recording"
    );
    assert!(
        dirs["in_flight"].exists(),
        "a tune mid-setup is not an orphan"
    );
    assert!(!dirs["stray"].exists());
}

// =============================================================================
// Auth
// =============================================================================

#[tokio::test]
async fn playback_is_per_user_but_leases_and_recording_are_not() {
    let server = TestServer::start_api_only().await;
    let tuner_id = make_tuner(&server.pool, "AUTHP001", 4).await;
    let channel = make_channel(&server.pool, tuner_id, 7, true).await;

    let anon = TestClient::new(&server.api_url);
    anon.post_json(
        "/api/v1/admin/livetv/sessions",
        &json!({ "channel_id": channel, "capabilities": browser_caps() }),
    )
    .await
    .assert_status(StatusCode::UNAUTHORIZED);

    let viewer = create_user_client(&server, "livetv-play-viewer@example.com", "viewer").await;
    viewer
        .get("/api/v1/admin/livetv/leases")
        .await
        .assert_status(StatusCode::FORBIDDEN);

    // Watching is a per-user surface: a viewer may open a session.
    let s = open_session(&viewer, channel, browser_caps()).await;
    let sid = s["session_id"].as_str().unwrap();

    // Valid body: the guard runs inside the handler, so a bad body would 422 first.
    viewer
        .post_json(
            &format!("/api/v1/admin/livetv/sessions/{sid}/record"),
            &json!({ "mode": "open_ended" }),
        )
        .await
        .assert_status(StatusCode::FORBIDDEN);

    let contributor =
        create_user_client(&server, "livetv-play-contrib@example.com", "contributor").await;
    contributor
        .get("/api/v1/admin/livetv/leases")
        .await
        .assert_ok();
}

// =============================================================================
// Real device. Opt-in: LIVETV_DEVICE_URL=http://192.0.2.10 cargo test -- --ignored
// =============================================================================

#[tokio::test]
#[ignore]
async fn asap_playback_against_a_real_hdhomerun() {
    let Ok(base_url) = std::env::var("LIVETV_DEVICE_URL") else {
        panic!("set LIVETV_DEVICE_URL to run this test");
    };
    let lcn: i32 = std::env::var("LIVETV_LCN")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(7);

    let loadavg = std::fs::read_to_string("/proc/loadavg").unwrap_or_default();
    println!("load average: {}", loadavg.trim());

    let server = TestServer::start_api_only().await;
    let client = server.admin_client();
    let resp = client
        .post_json(
            "/api/v1/admin/livetv/tuners",
            &json!({ "base_url": base_url }),
        )
        .await;
    resp.assert_status(StatusCode::CREATED);
    let tuner: Value = resp.json();
    let tuner_id = Uuid::parse_str(tuner["id"].as_str().unwrap()).unwrap();

    let entries = api::livetv::hdhomerun::lineup(&base_url).await.unwrap();
    let entry = entries
        .iter()
        .find(|e| e.lcn() == Some(lcn))
        .expect("requested LCN is in the lineup");
    let channel = make_channel(&server.pool, tuner_id, lcn, entry.is_hd()).await;

    // 1. Raw TS: TTFB plus one GOP and nothing else.
    {
        let candidate = api::livetv::tuner_source::SourceCandidate {
            channel_id: channel,
            name: format!("LCN {lcn}"),
            url: live_session::tuner_stream_url(&base_url, lcn),
            frequency: entry.frequency,
        };
        let t0 = std::time::Instant::now();
        let opened = api::livetv::tuner_source::open_first(
            &api::http::new_client(),
            &[candidate],
            api::livetv::tuner_source::FIRST_BYTE_DEADLINE,
            &tokio_util::sync::CancellationToken::new(),
        )
        .await
        .unwrap_or_else(|e| panic!("tune failed: {e:?}"));
        println!(
            "RAW /auto/v{lcn}: first {} bytes at {:?}",
            opened.first.len(),
            t0.elapsed()
        );
        drop(opened);
    }

    // 2. Transcoded buffer: POST /sessions -> first playable segment on disk.
    let t0 = std::time::Instant::now();
    let s = open_session(&client, channel, browser_caps()).await;
    let post_returned = t0.elapsed();
    let sid = s["session_id"].as_str().unwrap().to_string();
    let lease_id = s["lease"]["id"].as_str().unwrap().to_string();
    let buffer_dir = std::path::PathBuf::from(server.storage_root())
        .join("cache")
        .join("livetv")
        .join(&lease_id);

    let mut first_segment = None;
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
    while std::time::Instant::now() < deadline {
        if buffer_dir.join("seg_00000.m4s").exists() && buffer_dir.join("init.mp4").exists() {
            first_segment = Some(t0.elapsed());
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(5)).await;
    }

    let t_playlist = std::time::Instant::now();
    let resp = client
        .get(&format!("/api/v1/admin/livetv/sessions/{sid}/stream.m3u8"))
        .await;
    let playlist_at = t0.elapsed();
    resp.assert_ok();
    let playlist = resp.text();

    println!("POST /sessions returned in {post_returned:?}");
    println!("first playable segment on disk at {first_segment:?} after POST /sessions");
    println!(
        "GET stream.m3u8 served at {playlist_at:?} (route took {:?})",
        t_playlist.elapsed()
    );
    println!("--- playlist ---\n{playlist}");
    assert!(playlist.contains("segments/seg_00000.m4s"));
    assert!(
        post_returned < std::time::Duration::from_millis(300),
        "POST /sessions must not wait on the device: {post_returned:?}"
    );

    // 3. DirectPlay: the client reads the tune's byte fan-out, so first bytes
    //    must still arrive at tuner TTFB even though a segmenter now runs
    //    alongside on the same connection.
    {
        let t0 = std::time::Instant::now();
        let d = open_session(&client, channel, exoplayer_caps()).await;
        let direct_post = t0.elapsed();
        assert_eq!(d["method"], "direct");
        let dsid = d["session_id"].as_str().unwrap();
        let t1 = std::time::Instant::now();
        let resp = reqwest::Client::new()
            .get(format!(
                "{}/api/v1/admin/livetv/channels/{channel}/stream.ts?session={dsid}",
                server.api_url
            ))
            .bearer_auth(&server.admin_token)
            .send()
            .await
            .unwrap();
        let headers_at = t1.elapsed();
        use futures_util::StreamExt;
        let mut body = resp.bytes_stream();
        let first = body.next().await.unwrap().unwrap();
        println!(
            "DIRECT: POST /sessions {direct_post:?}, stream.ts headers {headers_at:?}, \
             first {} bytes at {:?} after the GET",
            first.len(),
            t1.elapsed()
        );
        let dbuf: Value = client
            .get(&format!("/api/v1/admin/livetv/sessions/{dsid}/buffer"))
            .await
            .json();
        println!("direct session buffer: {dbuf}");
        drop(body);
        client
            .delete(&format!("/api/v1/admin/livetv/sessions/{dsid}"))
            .await
            .assert_status(StatusCode::NO_CONTENT);
    }

    // Let the buffer grow, then report what the retention/timeshift surface sees.
    tokio::time::sleep(std::time::Duration::from_secs(8)).await;
    let buffer: Value = client
        .get(&format!("/api/v1/admin/livetv/sessions/{sid}/buffer"))
        .await
        .json();
    println!("buffer after 8s: {buffer}");
    if let Ok(dest) = std::env::var("LIVETV_KEEP_BUFFER") {
        let _ = std::fs::create_dir_all(&dest);
        for entry in std::fs::read_dir(&buffer_dir).unwrap().flatten() {
            let _ = std::fs::copy(
                entry.path(),
                std::path::Path::new(&dest).join(entry.file_name()),
            );
        }
        println!("buffer copied to {dest}");
    }

    client
        .delete(&format!("/api/v1/admin/livetv/sessions/{sid}"))
        .await
        .assert_status(StatusCode::NO_CONTENT);
    client
        .delete(&format!("/api/v1/admin/livetv/tuners/{tuner_id}"))
        .await
        .assert_status(StatusCode::NO_CONTENT);
}

// =============================================================================
// Auth boundary — what a player may fetch without a header
// =============================================================================

// libVLC cannot send an Authorization header, so anything it fetches directly
// has to be reachable without one or every byte detours through the client's
// localhost proxy. The session id is the credential: it lives only in the live
// tune map, so an unknown one is a miss rather than a traversal.
#[tokio::test]
async fn livetv_session_playback_routes_are_public() {
    let server = TestServer::start_api_only().await;
    let no_auth = server.api_client();
    let sid = Uuid::new_v4();

    for path in [
        format!("/api/v1/admin/livetv/sessions/{sid}/stream.m3u8"),
        format!("/api/v1/admin/livetv/sessions/{sid}/segments/init.mp4"),
        format!("/api/v1/admin/livetv/sessions/{sid}/segments/seg_00000.m4s"),
        format!("/api/v1/admin/livetv/sessions/{sid}/thumbnails.vtt"),
        format!("/api/v1/admin/livetv/sessions/{sid}/thumbnails/sprite_0.jpg"),
    ] {
        no_auth
            .get(&path)
            .await
            .assert_status(StatusCode::NOT_FOUND);
    }
}

// Unlike a session id, a channel id is listed in the guide, and tuning one from
// cold takes one of the four hardware tuners. A caller that cannot name a live
// session is asking for exactly that, so it still has to prove who it is —
// a bogus session id must not be a way around the check.
#[tokio::test]
async fn livetv_channel_stream_without_a_session_requires_auth() {
    let server = TestServer::start_api_only().await;
    let channel = Uuid::new_v4();
    let no_auth = server.api_client();

    for path in [
        format!("/api/v1/admin/livetv/channels/{channel}/stream.ts"),
        format!(
            "/api/v1/admin/livetv/channels/{channel}/stream.ts?session={}",
            Uuid::new_v4()
        ),
    ] {
        no_auth
            .get(&path)
            .await
            .assert_status(StatusCode::UNAUTHORIZED);
    }
}
