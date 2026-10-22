//! Duplicate services with one guide are shown as one channel, and a tune that cannot
//! lock falls back to the stack's other members.

use std::collections::BTreeSet;
use std::time::Duration;

use crate::common::*;
use api::livetv::tuner_source::{self, OpenError, SourceCandidate};
use api::repositories::{livetv_channels, livetv_guide, livetv_tuners};
use reqwest::StatusCode;
use serde_json::{Value, json};
use time::OffsetDateTime;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

async fn make_tuner(pool: &sqlx::PgPool) -> Uuid {
    livetv_tuners::upsert(
        pool,
        livetv_tuners::CreateTuner {
            device_id: "STACK01".to_string(),
            friendly_name: "Test Quatro".to_string(),
            model_number: None,
            firmware_version: None,
            base_url: "http://127.0.0.1:1".to_string(),
            device_auth: None,
            tuner_count: 4,
        },
    )
    .await
    .unwrap()
    .id
}

struct Svc {
    lcn: i32,
    name: &'static str,
    hd: bool,
    codec: &'static str,
    quality: i32,
}

async fn make_channel(pool: &sqlx::PgPool, tuner_id: Uuid, s: &Svc) -> Uuid {
    livetv_channels::upsert_scan(
        pool,
        livetv_channels::UpsertScanChannel {
            tuner_id,
            lcn: Some(s.lcn),
            scan_name: Some(s.name.to_string()),
            frequency: 177_500_000 + s.lcn as i64,
            program_number: 1000 + s.lcn,
            onid: None,
            tsid: None,
            modulation: None,
            video_codec: Some(s.codec.to_string()),
            audio_codec: Some("AC3".to_string()),
            hd: s.hd,
            signal_strength: Some(90),
            signal_quality: Some(s.quality),
            initial_kind: "tv".to_string(),
            initial_enabled: true,
        },
    )
    .await
    .unwrap()
    .0
    .id
}

async fn seed_guide(pool: &sqlx::PgPool, channel_id: Uuid, titles: &[&str]) {
    let now = OffsetDateTime::now_utc();
    for (i, title) in titles.iter().enumerate() {
        let start = now + time::Duration::minutes(i as i64 * 60 - 10);
        livetv_guide::upsert(
            pool,
            livetv_guide::UpsertGuideEntry {
                channel_id,
                start_time: start,
                end_time: start + time::Duration::minutes(60),
                title: title.to_string(),
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
    }
}

struct Lineup {
    hd7: Uuid,
    hd7_weak: Uuid,
    sd7: Uuid,
    nine: Uuid,
    short: Uuid,
    short_twin: Uuid,
}

/// 60 and 6 are 7HD Gippsland (60 the cleaner signal), 61 the MPEG-2 SD copy; 9 has its
/// own guide; 31 and 32 share a two-entry placeholder guide that must not stack them.
async fn lineup(server: &TestServer) -> Lineup {
    let pool = &server.pool;
    let tuner = make_tuner(pool).await;
    let seven = ["News", "Sunrise", "The Chase", "Home and Away", "Movie"];
    let svc = |lcn, name, hd, codec, quality| Svc {
        lcn,
        name,
        hd,
        codec,
        quality,
    };
    let hd7 = make_channel(pool, tuner, &svc(60, "7HD Gippsland", true, "H264", 95)).await;
    let hd7_weak = make_channel(pool, tuner, &svc(6, "7HD Gippsland", true, "H264", 40)).await;
    let sd7 = make_channel(pool, tuner, &svc(61, "7 Gippsland", false, "MPEG2", 100)).await;
    let nine = make_channel(pool, tuner, &svc(9, "9HD", true, "H264", 90)).await;
    let short = make_channel(pool, tuner, &svc(31, "Short", true, "H264", 90)).await;
    let short_twin = make_channel(pool, tuner, &svc(32, "Short Twin", true, "H264", 90)).await;
    for id in [hd7, hd7_weak, sd7] {
        seed_guide(pool, id, &seven).await;
    }
    seed_guide(pool, nine, &["A", "B", "C", "D"]).await;
    seed_guide(pool, short, &["Placeholder", "Placeholder"]).await;
    seed_guide(pool, short_twin, &["Placeholder", "Placeholder"]).await;
    sqlx::query("UPDATE livetv_channels SET favourite = TRUE WHERE id = $1")
        .bind(sd7)
        .execute(pool)
        .await
        .unwrap();
    Lineup {
        hd7,
        hd7_weak,
        sd7,
        nine,
        short,
        short_twin,
    }
}

fn ids_of(channels: &[Value]) -> BTreeSet<String> {
    channels
        .iter()
        .map(|c| c["id"].as_str().unwrap().to_string())
        .collect()
}

fn find(channels: &[Value], id: Uuid) -> &Value {
    channels
        .iter()
        .find(|c| c["id"] == id.to_string())
        .unwrap_or_else(|| panic!("{id} missing"))
}

fn set(ids: &[Uuid]) -> BTreeSet<String> {
    ids.iter().map(|i| i.to_string()).collect()
}

#[tokio::test]
async fn stacked_channel_list_shows_one_row_per_guide() {
    let server = TestServer::start_api_only().await;
    let l = lineup(&server).await;
    let admin = server.admin_client();

    let raw: Value = admin.get("/api/v1/admin/livetv/channels").await.json();
    assert_eq!(raw["total"], 6);
    let raw_items = raw["items"].as_array().unwrap();
    assert_eq!(find(raw_items, l.hd7)["alternate_ids"], json!([]));

    let stacked: Value = admin
        .get("/api/v1/admin/livetv/channels?stacked=true")
        .await
        .json();
    assert_eq!(stacked["total"], 4);
    let items = stacked["items"].as_array().unwrap();
    assert_eq!(ids_of(items), set(&[l.hd7, l.nine, l.short, l.short_twin]));
    let primary = find(items, l.hd7);
    assert_eq!(
        primary["alternate_ids"],
        json!([l.hd7_weak.to_string(), l.sd7.to_string()])
    );
    assert_eq!(primary["favourite"], true);
    assert_eq!(find(items, l.nine)["alternate_ids"], json!([]));
    assert_eq!(find(items, l.nine)["favourite"], false);

    let paged: Value = admin
        .get("/api/v1/admin/livetv/channels?stacked=true&limit=2&offset=2")
        .await
        .json();
    assert_eq!(paged["total"], 4);
    assert_eq!(paged["items"].as_array().unwrap().len(), 2);
}

#[tokio::test]
async fn stacked_guide_now_and_on_now_drop_the_duplicates() {
    let server = TestServer::start_api_only().await;
    let l = lineup(&server).await;
    let admin = server.admin_client();
    let expected = set(&[l.hd7, l.nine, l.short, l.short_twin]);

    let guide: Value = admin
        .get("/api/v1/admin/livetv/guide?stacked=true")
        .await
        .json();
    let channels: Vec<Value> = guide["channels"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| c["channel"].clone())
        .collect();
    assert_eq!(ids_of(&channels), expected);
    assert_eq!(
        find(&channels, l.hd7)["alternate_ids"]
            .as_array()
            .unwrap()
            .len(),
        2
    );

    let raw_guide: Value = admin.get("/api/v1/admin/livetv/guide").await.json();
    assert_eq!(raw_guide["channels"].as_array().unwrap().len(), 6);

    let now: Value = admin
        .get("/api/v1/admin/livetv/guide/now?stacked=true")
        .await
        .json();
    let channels: Vec<Value> = now["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| c["channel"].clone())
        .collect();
    assert_eq!(ids_of(&channels), expected);

    let admin_id: Uuid = sqlx::query_scalar("SELECT id FROM users WHERE role = 'admin' LIMIT 1")
        .fetch_one(&server.pool)
        .await
        .unwrap();
    for channel in [l.hd7, l.sd7, l.nine] {
        api::repositories::livetv_affinity::upsert(
            &server.pool,
            admin_id,
            api::repositories::livetv_affinity::UpsertAffinity {
                series_id: None,
                channel_id: Some(channel),
                genre: None,
                weekday: None,
                hour: None,
                watch_count: 10,
                total_secs: 36_000,
                mean_completion: 0.9,
                last_watched_at: Some(OffsetDateTime::now_utc()),
            },
        )
        .await
        .unwrap();
    }
    let on_now: Value = admin
        .get("/api/v1/admin/livetv/on-now?stacked=true")
        .await
        .json();
    let seen: BTreeSet<String> = on_now["groups"]
        .as_array()
        .unwrap()
        .iter()
        .flat_map(|g| g["items"].as_array().unwrap().clone())
        .map(|i| i["channel"]["id"].as_str().unwrap().to_string())
        .collect();
    assert!(seen.contains(&l.hd7.to_string()));
    assert!(seen.contains(&l.nine.to_string()));
    assert!(!seen.contains(&l.sd7.to_string()));
    assert!(!seen.contains(&l.hd7_weak.to_string()));
}

/// A 503 that takes its time, a tuner that answers but never sends, then one that works.
#[tokio::test]
async fn a_tune_falls_through_to_the_first_member_that_sends_a_byte() {
    let mock = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/auto/v60"))
        .respond_with(
            ResponseTemplate::new(503)
                .insert_header("X-HDHomeRun-Error", "807 No Video Data")
                .set_delay(Duration::from_millis(100)),
        )
        .mount(&mock)
        .await;
    Mock::given(method("GET"))
        .and(path("/auto/v6"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_bytes(vec![0x47u8; 188])
                .set_delay(Duration::from_secs(5)),
        )
        .mount(&mock)
        .await;
    Mock::given(method("GET"))
        .and(path("/auto/v61"))
        .respond_with(ResponseTemplate::new(200).set_body_bytes(vec![0x47u8; 188]))
        .mount(&mock)
        .await;
    let candidate = |lcn: i32, frequency: i64| SourceCandidate {
        channel_id: Uuid::new_v4(),
        name: format!("LCN {lcn}"),
        url: format!("{}/auto/v{lcn}", mock.uri()),
        frequency,
    };
    let candidates = [
        candidate(60, 564_500_000),
        candidate(6, 571_500_000),
        candidate(61, 177_500_000),
    ];

    let opened = tuner_source::open_first(
        &api::http::new_client(),
        &candidates,
        Duration::from_millis(500),
        &CancellationToken::new(),
    )
    .await
    .unwrap_or_else(|_| panic!("the SD member should have tuned"));
    assert_eq!(opened.index, 2);
    assert_eq!(opened.first[0], 0x47);

    let errors = match tuner_source::open_first(
        &api::http::new_client(),
        &candidates[..2],
        Duration::from_millis(500),
        &CancellationToken::new(),
    )
    .await
    {
        Ok(_) => panic!("neither member can tune"),
        Err(errors) => errors,
    };
    assert_eq!(
        errors,
        vec![
            OpenError::Status {
                status: 503,
                tuner_error: Some("807 No Video Data".to_string())
            },
            OpenError::NoFirstByte
        ]
    );
    assert_eq!(
        tuner_source::failure_message("7HD Gippsland", &errors),
        "7HD Gippsland has no signal right now (the tuner couldn't lock onto it)"
    );

    // 61 would tune, but it shares 60's mux and that mux has just said it has no signal.
    let same_mux = [candidate(60, 564_500_000), candidate(61, 564_500_000)];
    let errors = match tuner_source::open_first(
        &api::http::new_client(),
        &same_mux,
        Duration::from_millis(500),
        &CancellationToken::new(),
    )
    .await
    {
        Ok(_) => panic!("a member on a dead mux was tried"),
        Err(errors) => errors,
    };
    assert_eq!(errors.len(), 1);
}

#[test]
fn failure_messages_name_the_cause() {
    let all_busy = [OpenError::Status {
        status: 503,
        tuner_error: Some("805 All Tuners In Use".to_string()),
    }];
    assert_eq!(
        tuner_source::failure_message("9HD", &all_busy),
        "All tuners are in use, so 9HD couldn't be tuned"
    );
    let down = [OpenError::Unreachable("connection refused".to_string())];
    assert_eq!(
        tuner_source::failure_message("9HD", &down),
        "The tuner didn't answer when tuning 9HD"
    );
}

/// The device behind this tuner refuses every connection, so the tune fails before a
/// byte and the viewer has to be told why rather than left on a spinner.
#[tokio::test]
async fn a_tune_that_never_starts_says_why() {
    let server = TestServer::start_api_only().await;
    let l = lineup(&server).await;
    let admin = server.admin_client();
    let resp = admin
        .post_json(
            "/api/v1/admin/livetv/sessions",
            &json!({ "channel_id": l.nine, "capabilities": {
                "video_codecs": ["h264", "mpeg2video"],
                "audio_codecs": ["ac3", "aac"],
                "containers": ["mpegts", "mp4"]
            } }),
        )
        .await;
    resp.assert_ok();
    let session: Value = resp.json();
    let sid = session["session_id"].as_str().unwrap().to_string();

    let mut status = Value::Null;
    for _ in 0..100 {
        status = admin
            .get(&format!("/api/v1/admin/livetv/sessions/{sid}/buffer"))
            .await
            .json();
        if status["tune_error"].is_string() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert_eq!(
        status["tune_error"],
        "The tuner didn't answer when tuning 9HD"
    );

    let playlist = TestClient::new(&server.api_url)
        .get(&format!("/api/v1/admin/livetv/sessions/{sid}/stream.m3u8"))
        .await;
    assert_eq!(playlist.status, StatusCode::BAD_GATEWAY);
    assert_eq!(
        playlist.json::<Value>()["error"],
        "The tuner didn't answer when tuning 9HD"
    );

    let direct = TestClient::new(&server.api_url)
        .get(&format!(
            "/api/v1/admin/livetv/channels/{}/stream.ts?session={sid}",
            l.nine
        ))
        .await;
    assert_eq!(direct.status, StatusCode::BAD_GATEWAY);
    assert_eq!(
        direct.json::<Value>()["error"],
        "The tuner didn't answer when tuning 9HD"
    );
}

#[tokio::test]
async fn stacked_lists_need_contributor() {
    let server = TestServer::start_api_only().await;
    let viewer = create_user_client(&server, "viewer@test.com", "viewer").await;
    let resp = viewer
        .get("/api/v1/admin/livetv/channels?stacked=true")
        .await;
    assert_eq!(resp.status, StatusCode::FORBIDDEN);
}
