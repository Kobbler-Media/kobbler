use crate::common::*;
use api::livetv::recorder;
use api::livetv::series_rules;
use api::repositories::{livetv_channels, livetv_guide, livetv_recordings, livetv_series_rules};
use reqwest::StatusCode;
use serde_json::{Value, json};
use time::OffsetDateTime;
use uuid::Uuid;

// =============================================================================
// Fixtures
// =============================================================================

async fn make_tuner(pool: &sqlx::PgPool, device_id: &str, tuners: i32) -> Uuid {
    api::repositories::livetv_tuners::upsert(
        pool,
        api::repositories::livetv_tuners::CreateTuner {
            device_id: device_id.to_string(),
            friendly_name: "Test Quatro".to_string(),
            model_number: Some("HDHR5-4DT".to_string()),
            firmware_version: Some("20260326".to_string()),
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
            scan_name: Some(format!("CH{lcn}")),
            frequency: 226_500_000 + i64::from(lcn),
            program_number: lcn,
            onid: Some(12931),
            tsid: Some(1793),
            modulation: Some("auto".to_string()),
            video_codec: Some("MPEG4".to_string()),
            audio_codec: Some("AC3".to_string()),
            hd,
            signal_strength: Some(80),
            signal_quality: Some(90),
            initial_kind: "tv".to_string(),
            initial_enabled: true,
        },
    )
    .await
    .unwrap()
    .0
    .id
}

#[allow(clippy::too_many_arguments)]
async fn make_guide(
    pool: &sqlx::PgPool,
    channel_id: Uuid,
    series_id: &str,
    episode: &str,
    start_mins: i64,
    length_mins: i64,
    genres: &[&str],
) -> Uuid {
    let now = OffsetDateTime::now_utc();
    livetv_guide::upsert(
        pool,
        livetv_guide::UpsertGuideEntry {
            channel_id,
            start_time: now + time::Duration::minutes(start_mins),
            end_time: now + time::Duration::minutes(start_mins + length_mins),
            title: "Sample Programme".to_string(),
            episode_number: Some(episode.to_string()),
            episode_title: None,
            synopsis: Some("A synopsis".to_string()),
            original_airdate: None,
            series_id: Some(series_id.to_string()),
            genres: genres.iter().map(|g| g.to_string()).collect(),
            image_url: None,
            poster_url: None,
            is_first: false,
            episode_key: Some(format!("{series_id}|E:{episode}")),
            episode_key_kind: Some("episode".to_string()),
        },
    )
    .await
    .unwrap()
}

fn rule_body(series_id: &str) -> Value {
    json!({
        "series_id": series_id,
        "title": "Sample Programme",
        "enabled": true,
        "channel_ids": [],
        "weekdays": [],
        "window_start": null,
        "window_end": null,
        "new_only": false,
        "min_season": null,
        "keep_count": null,
        "keep_until": "space",
        "padding_before_secs": 0,
        "padding_after_secs": 0,
        "on_clash": "move",
        "priority": 0
    })
}

async fn write_part(dir: &std::path::Path, part: &str, segments: &[&str]) {
    let part_dir = dir.join(part);
    tokio::fs::create_dir_all(&part_dir).await.unwrap();
    tokio::fs::write(part_dir.join("init.mp4"), b"init")
        .await
        .unwrap();
    let mut playlist = String::from(
        "#EXTM3U\n#EXT-X-VERSION:7\n#EXT-X-TARGETDURATION:4\n#EXT-X-PLAYLIST-TYPE:EVENT\n#EXT-X-MAP:URI=\"init.mp4\"\n",
    );
    for seg in segments {
        tokio::fs::write(part_dir.join(seg), b"data").await.unwrap();
        playlist.push_str("#EXTINF:4.000000,\n");
        playlist.push_str(seg);
        playlist.push('\n');
    }
    playlist.push_str("#EXT-X-ENDLIST\n");
    tokio::fs::write(part_dir.join("stream.m3u8"), playlist)
        .await
        .unwrap();
}

// =============================================================================
// Decision 13 — restart resilience
// =============================================================================

/// A reboot mid-window is just another mid-stream failure: the next capture
/// appends a part and every earlier segment stays in the playlist.
#[tokio::test]
async fn a_restart_mid_window_appends_a_part_instead_of_restarting() {
    let root = tempfile::tempdir().unwrap();
    let dir = root.path().join("rec");

    write_part(&dir, "part_000", &["seg_00000.m4s", "seg_00001.m4s"]).await;
    let next = recorder::next_part_dir(&dir).await;
    assert_eq!(next.file_name().unwrap(), "part_001");

    write_part(&dir, "part_001", &["seg_00000.m4s"]).await;
    assert_eq!(
        recorder::next_part_dir(&dir).await.file_name().unwrap(),
        "part_002"
    );

    let (playlist, status) = recorder::read_playlist(&dir, true, "segments/")
        .await
        .expect("a stitched playlist");
    assert_eq!(status.parts, 2);
    assert_eq!(status.segments, 3, "the first part's segments survive");
    assert_eq!(status.duration_seconds, 12.0);
    assert_eq!(
        playlist.matches("#EXT-X-DISCONTINUITY").count(),
        1,
        "one discontinuity between the parts, none before the first"
    );
    assert_eq!(playlist.matches("#EXT-X-MAP:URI=").count(), 2);
    assert!(playlist.contains("segments/part_000/seg_00000.m4s"));
    assert!(playlist.contains("segments/part_001/seg_00000.m4s"));
    assert!(playlist.ends_with("#EXT-X-ENDLIST\n"));

    let (live, _) = recorder::read_playlist(&dir, false, "segments/")
        .await
        .unwrap();
    assert!(
        !live.contains("#EXT-X-ENDLIST"),
        "an in-progress recording is still growing"
    );
}

/// A promoted live buffer has no parts at all, and reads as one flat part.
#[tokio::test]
async fn a_promoted_buffer_reads_as_a_single_flat_part() {
    let root = tempfile::tempdir().unwrap();
    write_part(root.path(), ".", &["seg_00000.m4s"]).await;

    let (playlist, status) = recorder::read_playlist(root.path(), false, "segments/")
        .await
        .unwrap();
    assert_eq!(status.parts, 1);
    assert_eq!(status.segments, 1);
    assert!(playlist.contains("segments/seg_00000.m4s"));
    assert!(!playlist.contains("#EXT-X-DISCONTINUITY"));
}

#[tokio::test]
async fn boot_resumes_an_open_recording_and_closes_a_passed_one() {
    let server = TestServer::start_api_only().await;
    let tuner = make_tuner(&server.pool, "RESUME01", 4).await;
    let channel = make_channel(&server.pool, tuner, 7, true).await;
    let now = OffsetDateTime::now_utc();

    let open = livetv_recordings::create(
        &server.pool,
        livetv_recordings::CreateRecording {
            channel_id: channel,
            guide_id: None,
            series_rule_id: None,
            title: "Still going".to_string(),
            episode_number: None,
            episode_title: None,
            episode_key: None,
            series_id: None,
            synopsis: None,
            scheduled_start: now - time::Duration::minutes(10),
            scheduled_end: now + time::Duration::minutes(30),
            actual_start: Some(now - time::Duration::minutes(10)),
            status: "recording".to_string(),
            output_dir: Some("/tmp/does-not-matter".to_string()),
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

    let past = livetv_recordings::create(
        &server.pool,
        livetv_recordings::CreateRecording {
            channel_id: channel,
            guide_id: None,
            series_rule_id: None,
            title: "Window closed".to_string(),
            episode_number: None,
            episode_title: None,
            episode_key: None,
            series_id: None,
            synopsis: None,
            scheduled_start: now - time::Duration::hours(3),
            scheduled_end: now - time::Duration::hours(2),
            actual_start: Some(now - time::Duration::hours(3)),
            status: "recording".to_string(),
            output_dir: Some("/tmp/does-not-matter".to_string()),
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

    let missed = livetv_recordings::create(
        &server.pool,
        livetv_recordings::CreateRecording {
            channel_id: channel,
            guide_id: None,
            series_rule_id: None,
            title: "Never started".to_string(),
            episode_number: None,
            episode_title: None,
            episode_key: None,
            series_id: None,
            synopsis: None,
            scheduled_start: now - time::Duration::hours(5),
            scheduled_end: now - time::Duration::hours(4),
            actual_start: None,
            status: "scheduled".to_string(),
            output_dir: None,
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

    let report = recorder::reconcile_on_boot(&server.pool, &server.nats_publisher)
        .await
        .unwrap();
    assert_eq!(report.resumed, 1);
    assert_eq!(report.closed, 1);
    assert_eq!(report.missed, 1);

    let open = livetv_recordings::get_by_id(&server.pool, open.id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(open.status, "recording", "still inside its window");

    // Boot ages the claim stamp so the resume job can take the row straight away.
    assert!(
        livetv_recordings::try_claim_for_resume(&server.pool, open.id, recorder::RESUME_STALE_SECS)
            .await
            .unwrap(),
        "a resumed recording is immediately claimable"
    );
    // ...and only once, so a redelivery cannot append in parallel.
    assert!(
        !livetv_recordings::try_claim_for_resume(
            &server.pool,
            open.id,
            recorder::RESUME_STALE_SECS
        )
        .await
        .unwrap()
    );

    let past = livetv_recordings::get_by_id(&server.pool, past.id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(past.status, "completed");
    let missed = livetv_recordings::get_by_id(&server.pool, missed.id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(missed.status, "failed");
    assert!(missed.error.unwrap().contains("server was down"));
}

/// Failure to write to disk is the one case that aborts loudly rather than
/// retrying — another attempt would write nothing either.
#[tokio::test]
async fn a_capture_that_cannot_write_to_disk_aborts_loudly() {
    let root = tempfile::tempdir().unwrap();
    let blocked = root.path().join("nope");
    tokio::fs::write(&blocked, b"a file where a directory should be")
        .await
        .unwrap();

    let err = recorder::capture_part(
        "http://127.0.0.1:1/auto/v7",
        &blocked.join("part_000"),
        std::time::Duration::from_secs(1),
        tokio_util::sync::CancellationToken::new(),
    )
    .await
    .expect_err("a capture with nowhere to write must fail");
    assert!(
        matches!(err, recorder::CaptureError::Disk(_)),
        "got {err:?}, which would be retried"
    );
}

/// A source that will not open is retried, not aborted.
#[tokio::test]
async fn an_unreachable_tuner_is_a_retryable_source_failure() {
    let root = tempfile::tempdir().unwrap();
    let err = recorder::capture_part(
        "http://127.0.0.1:1/auto/v7",
        &root.path().join("part_000"),
        std::time::Duration::from_secs(2),
        tokio_util::sync::CancellationToken::new(),
    )
    .await
    .expect_err("an unreachable device cannot capture");
    assert!(matches!(err, recorder::CaptureError::Source(_)), "{err:?}");
}

// =============================================================================
// Recordings API
// =============================================================================

/// Every field here was typed wrong on at least one client, and a JSON number
/// read as a string is a hard decode failure on kobapp rather than a bad render.
/// The wire types are the contract, so they are asserted rather than inferred.
#[tokio::test]
async fn the_recording_payload_keeps_its_json_types() {
    let server = TestServer::start_api_only().await;
    let client = server.admin_client();
    let tuner = make_tuner(&server.pool, "RECTYPE1", 4).await;
    let channel = make_channel(&server.pool, tuner, 7, true).await;
    let guide = make_guide(
        &server.pool,
        channel,
        "SERIES1",
        "S01E01",
        60,
        30,
        &["Drama"],
    )
    .await;

    let rec: Value = client
        .post_json(
            "/api/v1/admin/livetv/recordings",
            &json!({ "guide_id": guide }),
        )
        .await
        .assert_status(StatusCode::CREATED)
        .json();

    assert!(rec["provenance"]["lcn"].is_number(), "provenance.lcn");
    assert!(rec["seconds_on_disk"].is_number(), "seconds_on_disk");
    assert!(rec["ad_posture"].is_string(), "ad_posture");
    assert!(rec["ad_skip_disabled"].is_boolean(), "ad_skip_disabled");
    // Not started yet, so these are the nulls a client has to tolerate.
    assert!(rec["duration_seconds"].is_null(), "duration_seconds");
    assert!(rec["size_bytes"].is_null(), "size_bytes");
    assert!(rec["tuner_index"].is_null(), "tuner_index");

    let channels: Value = client
        .get("/api/v1/admin/livetv/channels")
        .await
        .assert_ok()
        .json();
    assert!(channels["items"][0]["lcn"].is_number(), "channel lcn");

    let leases: Value = client
        .get("/api/v1/admin/livetv/leases")
        .await
        .assert_ok()
        .json();
    assert!(leases["in_use"].is_number(), "leases in_use");
}

/// Guide rows are pruned 24 h after they air, so the recording has to keep its own
/// copy of the artwork or every finished recording loses its cover.
#[tokio::test]
async fn a_recording_keeps_the_artwork_it_was_scheduled_from() {
    let server = TestServer::start_api_only().await;
    let client = server.admin_client();
    let tuner = make_tuner(&server.pool, "RECART01", 4).await;
    let channel = make_channel(&server.pool, tuner, 7, true).await;
    let now = OffsetDateTime::now_utc();
    let still = "https://img.hdhomerun.com/titles/C33677068ENP2IK.jpg";
    let guide = livetv_guide::upsert(
        &server.pool,
        livetv_guide::UpsertGuideEntry {
            channel_id: channel,
            start_time: now + time::Duration::minutes(60),
            end_time: now + time::Duration::minutes(90),
            title: "Sample Programme".to_string(),
            episode_number: Some("S01E01".to_string()),
            episode_title: None,
            synopsis: None,
            original_airdate: None,
            series_id: Some("SERIESART".to_string()),
            genres: vec![],
            image_url: Some(still.to_string()),
            poster_url: None,
            is_first: false,
            episode_key: Some("SERIESART|E:S01E01".to_string()),
            episode_key_kind: Some("episode".to_string()),
        },
    )
    .await
    .unwrap();

    let rec: Value = client
        .post_json(
            "/api/v1/admin/livetv/recordings",
            &json!({ "guide_id": guide }),
        )
        .await
        .assert_status(StatusCode::CREATED)
        .json();
    assert_eq!(rec["image_url"].as_str(), Some(still));

    sqlx::query("DELETE FROM livetv_guide WHERE id = $1")
        .bind(guide)
        .execute(&server.pool)
        .await
        .unwrap();

    let listed: Value = client
        .get("/api/v1/admin/livetv/recordings")
        .await
        .assert_ok()
        .json();
    assert_eq!(listed["items"][0]["image_url"].as_str(), Some(still));
}

#[tokio::test]
async fn a_recording_is_scheduled_from_a_guide_entry_then_stopped_and_deleted() {
    let server = TestServer::start_api_only().await;
    let client = server.admin_client();
    let tuner = make_tuner(&server.pool, "RECAPI01", 4).await;
    let channel = make_channel(&server.pool, tuner, 7, true).await;
    let guide = make_guide(
        &server.pool,
        channel,
        "SERIES1",
        "S01E01",
        60,
        30,
        &["Drama"],
    )
    .await;

    let created: Value = client
        .post_json(
            "/api/v1/admin/livetv/recordings",
            &json!({ "guide_id": guide }),
        )
        .await
        .assert_status(StatusCode::CREATED)
        .json();
    assert_eq!(created["status"], "scheduled");
    assert_eq!(created["channel_name"], "CH7");
    assert_eq!(created["provenance"]["lcn"], 7);
    assert_eq!(created["padding_before_secs"], 120);
    assert!(created["playable_from_start"].as_bool().unwrap());
    let id = created["id"].as_str().unwrap().to_string();

    // Asking twice for the same airing returns the same row, never a duplicate.
    let again: Value = client
        .post_json(
            "/api/v1/admin/livetv/recordings",
            &json!({ "guide_id": guide }),
        )
        .await
        .assert_ok()
        .json();
    assert_eq!(again["id"], created["id"]);

    let listed: Value = client
        .get("/api/v1/admin/livetv/recordings?status=scheduled")
        .await
        .assert_ok()
        .json();
    assert_eq!(listed["total"], 1);
    assert_eq!(listed["items"][0]["id"], created["id"]);

    let stopped: Value = client
        .post(&format!("/api/v1/admin/livetv/recordings/{id}/stop"))
        .await
        .assert_ok()
        .json();
    assert_eq!(stopped["status"], "cancelled");

    client
        .delete(&format!("/api/v1/admin/livetv/recordings/{id}"))
        .await
        .assert_status(StatusCode::NO_CONTENT);
    client
        .get(&format!("/api/v1/admin/livetv/recordings/{id}"))
        .await
        .assert_status(StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn a_manual_recording_needs_a_channel_and_a_sane_window() {
    let server = TestServer::start_api_only().await;
    let client = server.admin_client();
    let tuner = make_tuner(&server.pool, "RECAPI02", 4).await;
    let channel = make_channel(&server.pool, tuner, 7, true).await;
    let now = OffsetDateTime::now_utc();
    let fmt = time::format_description::well_known::Rfc3339;
    let start = (now + time::Duration::minutes(10)).format(&fmt).unwrap();
    let end = (now + time::Duration::minutes(40)).format(&fmt).unwrap();

    client
        .post_json("/api/v1/admin/livetv/recordings", &json!({}))
        .await
        .assert_status(StatusCode::BAD_REQUEST);

    client
        .post_json(
            "/api/v1/admin/livetv/recordings",
            &json!({ "channel_id": channel, "start_time": end, "end_time": start, "title": "Backwards" }),
        )
        .await
        .assert_status(StatusCode::BAD_REQUEST);

    client
        .post_json(
            "/api/v1/admin/livetv/recordings",
            &json!({ "channel_id": Uuid::new_v4(), "start_time": start, "end_time": end, "title": "Nowhere" }),
        )
        .await
        .assert_status(StatusCode::NOT_FOUND);

    let ok: Value = client
        .post_json(
            "/api/v1/admin/livetv/recordings",
            &json!({ "channel_id": channel, "start_time": start, "end_time": end, "title": "Manual" }),
        )
        .await
        .assert_status(StatusCode::CREATED)
        .json();
    assert_eq!(ok["title"], "Manual");
    assert!(ok["guide_id"].is_null());
}

/// The settings endpoint is admin-only, so a contributor's player can only learn
/// its posture from the payload it already fetches.
#[tokio::test]
async fn ad_posture_is_served_to_any_contributor_already_resolved() {
    let server = TestServer::start_api_only().await;
    let tuner = make_tuner(&server.pool, "RECAPI03", 4).await;
    let channel = make_channel(&server.pool, tuner, 7, true).await;
    let drama = make_guide(
        &server.pool,
        channel,
        "SERIES1",
        "S01E01",
        60,
        30,
        &["Drama"],
    )
    .await;
    let news = make_guide(
        &server.pool,
        channel,
        "SERIES2",
        "S01E01",
        120,
        30,
        &["News"],
    )
    .await;

    let contributor =
        create_user_client(&server, "livetv-rec-contrib@example.com", "contributor").await;
    let drama_rec: Value = contributor
        .post_json(
            "/api/v1/admin/livetv/recordings",
            &json!({ "guide_id": drama }),
        )
        .await
        .assert_status(StatusCode::CREATED)
        .json();
    assert_eq!(
        drama_rec["ad_posture"], "shade",
        "the owner default reaches a contributor, not a client-side guess"
    );
    assert_eq!(drama_rec["ad_skip_disabled"], false);

    let news_rec: Value = contributor
        .post_json(
            "/api/v1/admin/livetv/recordings",
            &json!({ "guide_id": news }),
        )
        .await
        .assert_status(StatusCode::CREATED)
        .json();
    assert_eq!(
        news_rec["ad_skip_disabled"], true,
        "detection collapses on news, and the server says so"
    );
}

#[tokio::test]
async fn recordings_and_series_rules_need_a_contributor() {
    let server = TestServer::start_api_only().await;
    let tuner = make_tuner(&server.pool, "RECAUTH1", 4).await;
    let channel = make_channel(&server.pool, tuner, 7, true).await;
    let guide = make_guide(
        &server.pool,
        channel,
        "SERIES1",
        "S01E01",
        60,
        30,
        &["Drama"],
    )
    .await;

    let viewer = create_user_client(&server, "livetv-rec-viewer@example.com", "viewer").await;
    let contributor =
        create_user_client(&server, "livetv-rule-contrib@example.com", "contributor").await;

    let anon = TestClient::new(&server.api_url);
    anon.get("/api/v1/admin/livetv/recordings")
        .await
        .assert_status(StatusCode::UNAUTHORIZED);

    viewer
        .get("/api/v1/admin/livetv/recordings")
        .await
        .assert_status(StatusCode::FORBIDDEN);
    // Valid bodies: the guard runs inside the handler, so a bad body would 422.
    viewer
        .post_json(
            "/api/v1/admin/livetv/recordings",
            &json!({ "guide_id": guide }),
        )
        .await
        .assert_status(StatusCode::FORBIDDEN);
    viewer
        .post_json(
            "/api/v1/admin/livetv/series-rules/preview",
            &rule_body("SERIES1"),
        )
        .await
        .assert_status(StatusCode::FORBIDDEN);

    contributor
        .get("/api/v1/admin/livetv/recordings")
        .await
        .assert_ok();
    contributor
        .get("/api/v1/admin/livetv/series-rules")
        .await
        .assert_ok();
    contributor
        .post_json(
            "/api/v1/admin/livetv/series-rules/preview",
            &rule_body("SERIES1"),
        )
        .await
        .assert_ok();
    // Scheduling is a household action, not device configuration: a contributor
    // creating a rule through the chatbot and through the UI must agree.
    let created: Value = contributor
        .post_json("/api/v1/admin/livetv/series-rules", &rule_body("SERIES1"))
        .await
        .json();
    let rule_id = created["id"].as_str().expect("a rule id");
    contributor
        .patch_json(&format!("/api/v1/admin/livetv/series-rules/{rule_id}"), &{
            let mut body = rule_body("SERIES1");
            body["enabled"] = json!(false);
            body
        })
        .await
        .assert_ok();
    viewer
        .patch_json(
            &format!("/api/v1/admin/livetv/series-rules/{rule_id}"),
            &rule_body("SERIES1"),
        )
        .await
        .assert_status(StatusCode::FORBIDDEN);
    viewer
        .delete(&format!("/api/v1/admin/livetv/series-rules/{rule_id}"))
        .await
        .assert_status(StatusCode::FORBIDDEN);
    contributor
        .delete(&format!("/api/v1/admin/livetv/series-rules/{rule_id}"))
        .await
        .assert_status(StatusCode::NO_CONTENT);
}

// =============================================================================
// Series rules
// =============================================================================

#[tokio::test]
async fn a_series_rule_round_trips_and_validates() {
    let server = TestServer::start_api_only().await;
    let client = server.admin_client();

    let mut body = rule_body("SERIES1");
    body["window_start"] = json!("19:30");
    body["window_end"] = json!("21:00:00");
    body["weekdays"] = json!([3, 1, 1]);

    let created: Value = client
        .post_json("/api/v1/admin/livetv/series-rules", &body)
        .await
        .assert_status(StatusCode::CREATED)
        .json();
    assert_eq!(created["window_start"], "19:30");
    assert_eq!(created["window_end"], "21:00");
    assert_eq!(created["weekdays"], json!([1, 3]), "sorted and deduped");
    assert_eq!(created["queued_count"], 0);
    let id = created["id"].as_str().unwrap().to_string();

    let listed: Value = client
        .get("/api/v1/admin/livetv/series-rules")
        .await
        .assert_ok()
        .json();
    assert_eq!(listed["items"].as_array().unwrap().len(), 1);

    let mut patched = body.clone();
    patched["title"] = json!("Renamed");
    patched["on_clash"] = json!("drop");
    let updated: Value = client
        .patch_json(&format!("/api/v1/admin/livetv/series-rules/{id}"), &patched)
        .await
        .assert_ok()
        .json();
    assert_eq!(updated["title"], "Renamed");
    assert_eq!(updated["on_clash"], "drop");

    let mut bad = body.clone();
    bad["on_clash"] = json!("shrug");
    client
        .post_json("/api/v1/admin/livetv/series-rules", &bad)
        .await
        .assert_status(StatusCode::BAD_REQUEST);

    let mut bad_day = body.clone();
    bad_day["weekdays"] = json!([9]);
    client
        .post_json("/api/v1/admin/livetv/series-rules", &bad_day)
        .await
        .assert_status(StatusCode::BAD_REQUEST);

    client
        .delete(&format!("/api/v1/admin/livetv/series-rules/{id}"))
        .await
        .assert_status(StatusCode::NO_CONTENT);
    client
        .delete(&format!("/api/v1/admin/livetv/series-rules/{id}"))
        .await
        .assert_status(StatusCode::NOT_FOUND);
}

/// The preview is rendered live as the rule is edited, so it must show every
/// airing with a reason and must not write anything.
#[tokio::test]
async fn preview_clusters_a_drifting_simulcast_and_writes_nothing() {
    let server = TestServer::start_api_only().await;
    let client = server.admin_client();
    let tuner = make_tuner(&server.pool, "PREVIEW1", 4).await;
    let sd = make_channel(&server.pool, tuner, 5, false).await;
    let hd = make_channel(&server.pool, tuner, 50, true).await;
    let other = make_channel(&server.pool, tuner, 14, false).await;

    // Two feeds of one airing, drifting by two minutes, plus a later repeat.
    make_guide(&server.pool, sd, "SERIES1", "S03E07", 60, 60, &["Drama"]).await;
    make_guide(&server.pool, hd, "SERIES1", "S03E07", 62, 60, &["Drama"]).await;
    make_guide(
        &server.pool,
        other,
        "SERIES1",
        "S03E07",
        300,
        60,
        &["Drama"],
    )
    .await;

    let preview: Value = client
        .post_json(
            "/api/v1/admin/livetv/series-rules/preview",
            &rule_body("SERIES1"),
        )
        .await
        .assert_ok()
        .json();

    let clusters = preview["clusters"].as_array().unwrap();
    assert_eq!(clusters.len(), 2, "a drift of two minutes is not a repeat");
    assert_eq!(clusters[0]["members"].as_array().unwrap().len(), 2);
    assert_eq!(preview["would_record"], 1);
    assert_eq!(preview["suppressed"], 2);

    let winner = clusters[0]["members"]
        .as_array()
        .unwrap()
        .iter()
        .find(|m| m["action"] == "record")
        .unwrap();
    assert_eq!(winner["hd"], true, "HD beats SD inside the cluster");
    assert_eq!(winner["lcn"], 50);
    assert_eq!(
        clusters[1]["members"][0]["reason"],
        "repeat of an earlier airing"
    );

    let (_, total) = livetv_recordings::list_filtered(
        &server.pool,
        &livetv_recordings::RecordingFilters::default(),
        50,
        0,
    )
    .await
    .unwrap();
    assert_eq!(total, 0, "preview must have no side effects");
}

#[tokio::test]
async fn a_title_only_key_is_never_auto_recorded_by_a_rule() {
    let server = TestServer::start_api_only().await;
    let client = server.admin_client();
    let tuner = make_tuner(&server.pool, "TITLEONLY", 4).await;
    let channel = make_channel(&server.pool, tuner, 7, true).await;

    let now = OffsetDateTime::now_utc();
    for start in [60i64, 120, 180] {
        livetv_guide::upsert(
            &server.pool,
            livetv_guide::UpsertGuideEntry {
                channel_id: channel,
                start_time: now + time::Duration::minutes(start),
                end_time: now + time::Duration::minutes(start + 30),
                title: "Home Shopping".to_string(),
                episode_number: None,
                episode_title: None,
                synopsis: None,
                original_airdate: None,
                series_id: Some("SHOP1".to_string()),
                genres: vec![],
                image_url: None,
                poster_url: None,
                is_first: false,
                episode_key: Some("SHOP1|X".to_string()),
                episode_key_kind: Some("title".to_string()),
            },
        )
        .await
        .unwrap();
    }

    let preview: Value = client
        .post_json(
            "/api/v1/admin/livetv/series-rules/preview",
            &rule_body("SHOP1"),
        )
        .await
        .assert_ok()
        .json();
    assert_eq!(preview["would_record"], 0);
    assert!(
        preview["clusters"][0]["members"][0]["reason"]
            .as_str()
            .unwrap()
            .contains("no episode identity")
    );

    client
        .post_json("/api/v1/admin/livetv/series-rules", &rule_body("SHOP1"))
        .await
        .assert_status(StatusCode::CREATED);
    let (_, total) = livetv_recordings::list_filtered(
        &server.pool,
        &livetv_recordings::RecordingFilters::default(),
        50,
        0,
    )
    .await
    .unwrap();
    assert_eq!(total, 0, "45 airings of Home Shopping, zero recordings");
}

/// Decision 6.6: the guide usually airs it again, so a clash moves rather than
/// dropping.
#[tokio::test]
async fn a_clash_moves_to_a_later_airing_instead_of_dropping() {
    let server = TestServer::start_api_only().await;
    let tuner = make_tuner(&server.pool, "CLASH001", 1).await;
    let a = make_channel(&server.pool, tuner, 7, true).await;
    let b = make_channel(&server.pool, tuner, 9, true).await;

    let first = make_guide(&server.pool, a, "SERIES1", "S01E01", 60, 60, &["Drama"]).await;
    let later = make_guide(&server.pool, a, "SERIES1", "S01E01", 300, 60, &["Drama"]).await;

    // The one tuner is already committed to something else at the first airing.
    let now = OffsetDateTime::now_utc();
    livetv_recordings::create(
        &server.pool,
        livetv_recordings::CreateRecording {
            channel_id: b,
            guide_id: None,
            series_rule_id: None,
            title: "Occupier".to_string(),
            episode_number: None,
            episode_title: None,
            episode_key: None,
            series_id: None,
            synopsis: None,
            scheduled_start: now + time::Duration::minutes(50),
            scheduled_end: now + time::Duration::minutes(130),
            actual_start: None,
            status: "scheduled".to_string(),
            output_dir: None,
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

    livetv_series_rules::create(
        &server.pool,
        livetv_series_rules::CreateSeriesRule {
            series_id: "SERIES1".to_string(),
            title: "Sample Programme".to_string(),
            enabled: true,
            channel_ids: vec![],
            weekdays: vec![],
            window_start: None,
            window_end: None,
            new_only: false,
            min_season: None,
            keep_count: None,
            keep_until: "space".to_string(),
            keep_until_date: None,
            padding_before_secs: 0,
            padding_after_secs: 0,
            on_clash: "move".to_string(),
            priority: 0,
        },
    )
    .await
    .unwrap();

    let stats = series_rules::expand_all(&server.pool).await.unwrap();
    assert_eq!(stats.moved, 1);
    assert_eq!(stats.dropped, 0);

    let moved = livetv_recordings::get_by_guide_id(&server.pool, first)
        .await
        .unwrap();
    assert!(moved.is_none(), "the clashing airing is no longer booked");

    let rescheduled = livetv_recordings::get_by_guide_id(&server.pool, later)
        .await
        .unwrap()
        .expect("the later airing is booked instead");
    assert_eq!(rescheduled.status, "scheduled");

    let (all, _) = livetv_recordings::list_filtered(
        &server.pool,
        &livetv_recordings::RecordingFilters::default(),
        50,
        0,
    )
    .await
    .unwrap();
    let moved_row = all
        .iter()
        .find(|r| r.status == "moved")
        .expect("the clash leaves a moved row naming where it went");
    assert_eq!(moved_row.guide_id, Some(first));
    assert_eq!(moved_row.moved_to_guide_id, Some(later));
}

#[tokio::test]
async fn with_no_later_airing_a_drop_rule_drops_and_a_move_rule_has_nowhere_to_go() {
    let server = TestServer::start_api_only().await;
    let tuner = make_tuner(&server.pool, "CLASH002", 1).await;
    let a = make_channel(&server.pool, tuner, 7, true).await;
    let b = make_channel(&server.pool, tuner, 9, true).await;
    make_guide(&server.pool, a, "SERIES1", "S01E01", 60, 60, &["Drama"]).await;

    let now = OffsetDateTime::now_utc();
    livetv_recordings::create(
        &server.pool,
        livetv_recordings::CreateRecording {
            channel_id: b,
            guide_id: None,
            series_rule_id: None,
            title: "Occupier".to_string(),
            episode_number: None,
            episode_title: None,
            episode_key: None,
            series_id: None,
            synopsis: None,
            scheduled_start: now + time::Duration::minutes(50),
            scheduled_end: now + time::Duration::minutes(130),
            actual_start: None,
            status: "scheduled".to_string(),
            output_dir: None,
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

    livetv_series_rules::create(
        &server.pool,
        livetv_series_rules::CreateSeriesRule {
            series_id: "SERIES1".to_string(),
            title: "Sample Programme".to_string(),
            enabled: true,
            channel_ids: vec![],
            weekdays: vec![],
            window_start: None,
            window_end: None,
            new_only: false,
            min_season: None,
            keep_count: None,
            keep_until: "space".to_string(),
            keep_until_date: None,
            padding_before_secs: 0,
            padding_after_secs: 0,
            on_clash: "drop".to_string(),
            priority: 0,
        },
    )
    .await
    .unwrap();

    let stats = series_rules::expand_all(&server.pool).await.unwrap();
    assert_eq!(stats.dropped, 1);
    assert_eq!(stats.moved, 0);
}

/// The 48 h ceiling means the expander cannot schedule once: it re-runs on every
/// guide refresh and must be idempotent when it does.
#[tokio::test]
async fn re_expanding_after_a_guide_refresh_does_not_double_book() {
    let server = TestServer::start_api_only().await;
    let tuner = make_tuner(&server.pool, "EXPAND01", 4).await;
    let sd = make_channel(&server.pool, tuner, 5, false).await;
    let hd = make_channel(&server.pool, tuner, 50, true).await;
    make_guide(&server.pool, sd, "SERIES1", "S01E01", 60, 60, &["Drama"]).await;
    make_guide(&server.pool, hd, "SERIES1", "S01E01", 62, 60, &["Drama"]).await;

    livetv_series_rules::create(
        &server.pool,
        livetv_series_rules::CreateSeriesRule {
            series_id: "SERIES1".to_string(),
            title: "Sample Programme".to_string(),
            enabled: true,
            channel_ids: vec![],
            weekdays: vec![],
            window_start: None,
            window_end: None,
            new_only: true,
            min_season: None,
            keep_count: None,
            keep_until: "space".to_string(),
            keep_until_date: None,
            padding_before_secs: 0,
            padding_after_secs: 0,
            on_clash: "move".to_string(),
            priority: 0,
        },
    )
    .await
    .unwrap();

    let first = series_rules::expand_all(&server.pool).await.unwrap();
    assert_eq!(first.scheduled, 1);
    let second = series_rules::expand_all(&server.pool).await.unwrap();
    assert_eq!(second.scheduled, 0, "the key is already claimed");

    let (rows, total) = livetv_recordings::list_filtered(
        &server.pool,
        &livetv_recordings::RecordingFilters::default(),
        50,
        0,
    )
    .await
    .unwrap();
    assert_eq!(
        total, 1,
        "one recording for two simulcast feeds, twice over"
    );
    assert!(rows[0].series_rule_id.is_some());
    assert_eq!(rows[0].channel_id, hd, "HD beats SD");
}

// =============================================================================
// Real device. Opt-in: LIVETV_DEVICE_URL=http://192.0.2.10 cargo test -- --ignored
// =============================================================================

/// The capture primitive against the real antenna: a pure remux into HLS parts,
/// then a second capture that appends rather than restarting.
#[tokio::test]
#[ignore]
async fn capture_writes_hls_parts_and_a_resume_appends_against_a_real_hdhomerun() {
    let Ok(base_url) = std::env::var("LIVETV_DEVICE_URL") else {
        panic!("set LIVETV_DEVICE_URL to run this test");
    };
    let lcn: i32 = std::env::var("LIVETV_LCN")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(7);
    println!(
        "load average: {}",
        std::fs::read_to_string("/proc/loadavg")
            .unwrap_or_default()
            .trim()
    );

    let url = api::livetv::live_session::tuner_stream_url(&base_url, lcn);
    let root = tempfile::tempdir().unwrap();
    let dir = root.path().join("recording");

    let t0 = std::time::Instant::now();
    let first = recorder::capture_part(
        &url,
        &dir.join("part_000"),
        std::time::Duration::from_secs(10),
        tokio_util::sync::CancellationToken::new(),
    )
    .await
    .expect("the real tuner captures");
    println!(
        "part_000: {} segments in {:?}",
        first.segments,
        t0.elapsed()
    );
    assert!(first.segments > 0, "a capture must write segments");

    let (_, after_first) = recorder::read_playlist(&dir, true, "segments/")
        .await
        .expect("a playlist after the first part");
    assert_eq!(after_first.parts, 1);

    let next = recorder::next_part_dir(&dir).await;
    assert_eq!(next.file_name().unwrap(), "part_001");
    let second = recorder::capture_part(
        &url,
        &next,
        std::time::Duration::from_secs(15),
        tokio_util::sync::CancellationToken::new(),
    )
    .await
    .expect("a resume captures");
    assert!(second.segments > 0);

    let (playlist, status) = recorder::read_playlist(&dir, true, "segments/")
        .await
        .expect("a stitched playlist");
    println!(
        "stitched: {} parts, {} segments, {:.2}s",
        status.parts, status.segments, status.duration_seconds
    );
    assert_eq!(status.parts, 2);
    assert_eq!(status.segments, first.segments + second.segments);
    assert_eq!(playlist.matches("#EXT-X-DISCONTINUITY").count(), 1);
    // Each part discards its final unclosed segment, so up to SEGMENT_SECONDS of
    // wall-clock capture is lost per part and an absolute threshold is unsound.
    // "Resume appends" means the stitched span exceeds the first part's own span.
    assert!(
        status.duration_seconds > after_first.duration_seconds,
        "stitched span {:.2}s must exceed the first part alone {:.2}s",
        status.duration_seconds,
        after_first.duration_seconds
    );
    assert!(recorder::dir_size(&dir).await > 0);
}

// =============================================================================
// Recording root fallback chain
// =============================================================================

#[tokio::test]
async fn recording_root_falls_back_to_the_iptv_path() {
    let server = TestServer::start_api_only().await;
    let client = server.admin_client();
    let storage_root = server.storage_root();

    let runtime = server.runtime_config.get().await;
    assert_eq!(
        recorder::recording_root(&server.pool, &server.app_state.config, &runtime)
            .await
            .unwrap(),
        std::path::PathBuf::from(&storage_root).join("livetv"),
        "nothing configured — the storage root default"
    );

    client
        .patch_json(
            "/api/v1/admin/iptv/settings",
            &json!({ "recording_path": "/mnt/hdd/recordings" }),
        )
        .await
        .assert_status(StatusCode::NO_CONTENT);

    let runtime = server.runtime_config.get().await;
    assert_eq!(
        recorder::recording_root(&server.pool, &server.app_state.config, &runtime)
            .await
            .unwrap(),
        std::path::PathBuf::from("/mnt/hdd/recordings"),
        "live tv unset — inherit the IPTV recordings dir"
    );

    server
        .runtime_config
        .update(api::runtime_config::RuntimeConfigUpdate {
            livetv_recording_path: Some("/mnt/hdd/livetv".to_string()),
            ..Default::default()
        })
        .await
        .expect("set livetv_recording_path");

    let runtime = server.runtime_config.get().await;
    assert_eq!(
        recorder::recording_root(&server.pool, &server.app_state.config, &runtime)
            .await
            .unwrap(),
        std::path::PathBuf::from("/mnt/hdd/livetv"),
        "an explicit live tv path outranks the IPTV one"
    );
}
