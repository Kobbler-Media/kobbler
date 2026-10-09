use crate::common::*;
use api::repositories::{
    livetv_ad_breaks, livetv_affinity, livetv_channels, livetv_guide, livetv_recordings,
};
use reqwest::StatusCode;
use serde_json::{Value, json};
use time::OffsetDateTime;
use uuid::Uuid;

// =============================================================================
// Fixtures
// =============================================================================

async fn make_tuner(pool: &sqlx::PgPool, device_id: &str) -> Uuid {
    api::repositories::livetv_tuners::upsert(
        pool,
        api::repositories::livetv_tuners::CreateTuner {
            device_id: device_id.to_string(),
            friendly_name: "Test Quatro".to_string(),
            model_number: Some("HDHR5-4DT".to_string()),
            firmware_version: Some("20260326".to_string()),
            base_url: "http://127.0.0.1:1".to_string(),
            device_auth: Some("test-auth".to_string()),
            tuner_count: 4,
        },
    )
    .await
    .unwrap()
    .id
}

async fn make_channel(pool: &sqlx::PgPool, tuner_id: Uuid, lcn: i32) -> Uuid {
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
            hd: true,
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

/// On now, so `/on-now` has something to group.
async fn make_live_guide(pool: &sqlx::PgPool, channel_id: Uuid, series_id: &str) -> Uuid {
    let now = OffsetDateTime::now_utc();
    livetv_guide::upsert(
        pool,
        livetv_guide::UpsertGuideEntry {
            channel_id,
            start_time: now - time::Duration::minutes(10),
            end_time: now + time::Duration::minutes(20),
            title: "Hard Quiz".to_string(),
            episode_number: Some("S01E01".to_string()),
            episode_title: None,
            synopsis: Some("A synopsis".to_string()),
            original_airdate: None,
            series_id: Some(series_id.to_string()),
            genres: vec!["Comedy".to_string()],
            image_url: None,
            poster_url: None,
            is_first: false,
            episode_key: Some(format!("{series_id}|E:S01E01")),
            episode_key_kind: Some("episode".to_string()),
        },
    )
    .await
    .unwrap()
}

async fn make_recording(pool: &sqlx::PgPool, channel_id: Uuid) -> Uuid {
    let now = OffsetDateTime::now_utc();
    livetv_recordings::create(
        pool,
        livetv_recordings::CreateRecording {
            channel_id,
            guide_id: None,
            series_rule_id: None,
            title: "Hard Quiz".to_string(),
            episode_number: None,
            episode_title: None,
            episode_key: None,
            series_id: None,
            synopsis: None,
            scheduled_start: now - time::Duration::hours(1),
            scheduled_end: now,
            actual_start: Some(now - time::Duration::hours(1)),
            status: "completed".to_string(),
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
    .unwrap()
    .id
}

// =============================================================================
// Watch events
// =============================================================================

#[tokio::test]
async fn a_watch_event_is_accepted_from_any_authenticated_user() {
    let server = TestServer::start_api_only().await;
    let tuner = make_tuner(&server.pool, "WATCH001").await;
    let channel = make_channel(&server.pool, tuner, 7).await;
    let guide = make_live_guide(&server.pool, channel, "SERIESW").await;

    let viewer = create_user_client(&server, "livetv-watch-viewer@example.com", "viewer").await;
    let started = (OffsetDateTime::now_utc() - time::Duration::minutes(30))
        .format(&time::format_description::well_known::Rfc3339)
        .unwrap();

    // A viewer is not a contributor, and watch history is still theirs to write.
    viewer
        .post_json(
            "/api/v1/admin/livetv/watch-events",
            &json!({
                "channel_id": channel,
                "guide_id": guide,
                "started_at": started,
                "watched_secs": 1500,
                "programme_secs": 1800,
                "source": "live"
            }),
        )
        .await
        .assert_status(StatusCode::NO_CONTENT);

    let count = api::repositories::livetv_watch_events::count_for_channel(&server.pool, channel)
        .await
        .unwrap();
    assert_eq!(count, 1);
}

#[tokio::test]
async fn a_watch_event_rejects_a_bad_source_a_missing_channel_and_anonymous_callers() {
    let server = TestServer::start_api_only().await;
    let tuner = make_tuner(&server.pool, "WATCH002").await;
    let channel = make_channel(&server.pool, tuner, 9).await;
    let started = OffsetDateTime::now_utc()
        .format(&time::format_description::well_known::Rfc3339)
        .unwrap();
    let body = json!({
        "channel_id": channel,
        "started_at": started,
        "watched_secs": 60,
        "source": "live"
    });

    TestClient::new(&server.api_url)
        .post_json("/api/v1/admin/livetv/watch-events", &body)
        .await
        .assert_status(StatusCode::UNAUTHORIZED);

    let viewer = create_user_client(&server, "livetv-watch-bad@example.com", "viewer").await;
    viewer
        .post_json(
            "/api/v1/admin/livetv/watch-events",
            &json!({
                "channel_id": channel,
                "started_at": started,
                "watched_secs": 60,
                "source": "telepathy"
            }),
        )
        .await
        .assert_status(StatusCode::BAD_REQUEST);
    viewer
        .post_json(
            "/api/v1/admin/livetv/watch-events",
            &json!({
                "channel_id": Uuid::new_v4(),
                "started_at": started,
                "watched_secs": 60,
                "source": "live"
            }),
        )
        .await
        .assert_status(StatusCode::NOT_FOUND);
    viewer
        .post_json(
            "/api/v1/admin/livetv/watch-events",
            &json!({
                "channel_id": channel,
                "started_at": "not a timestamp",
                "watched_secs": 60,
                "source": "live"
            }),
        )
        .await
        .assert_status(StatusCode::BAD_REQUEST);
}

// =============================================================================
// On now
// =============================================================================

#[tokio::test]
async fn on_now_groups_what_the_household_actually_watches() {
    let server = TestServer::start_api_only().await;
    let tuner = make_tuner(&server.pool, "ONNOW001").await;
    let channel = make_channel(&server.pool, tuner, 7).await;
    make_live_guide(&server.pool, channel, "SERIESON").await;

    let user_id = create_user_with_role(&server, "livetv-onnow@example.com", "viewer").await;
    let client = create_user_client(&server, "livetv-onnow2@example.com", "viewer").await;
    let _ = client;
    let viewer = TestClient::new(&server.api_url);
    let resp = viewer
        .post_json(
            "/api/v1/admin/login",
            &json!({ "email": "livetv-onnow@example.com", "password": "testpass123" }),
        )
        .await;
    resp.assert_ok();
    let token = resp.json::<Value>()["token"].as_str().unwrap().to_string();
    let viewer = TestClient::new(&server.api_url).with_auth(&token);

    livetv_affinity::upsert(
        &server.pool,
        user_id,
        livetv_affinity::UpsertAffinity {
            series_id: Some("SERIESON".to_string()),
            channel_id: None,
            genre: None,
            weekday: None,
            hour: None,
            watch_count: 14,
            total_secs: 25_200,
            mean_completion: 0.95,
            last_watched_at: Some(OffsetDateTime::now_utc()),
        },
    )
    .await
    .unwrap();

    let body: Value = viewer.get("/api/v1/admin/livetv/on-now").await.json();
    let groups = body["groups"].as_array().expect("groups");
    let series = groups
        .iter()
        .find(|g| g["key"] == "series")
        .expect("a series group");
    let item = &series["items"][0];
    assert_eq!(item["guide"]["title"], "Hard Quiz");
    assert!(
        item["confidence"].as_f64().unwrap() > 0.0,
        "14 completed viewings must produce a confidence: {item}"
    );
    assert!(item["reason"].as_str().unwrap().contains("14"));
}

/// Below the history floor the dots are hidden entirely rather than faked.
#[tokio::test]
async fn on_now_reports_no_confidence_when_there_is_barely_any_history() {
    let server = TestServer::start_api_only().await;
    let tuner = make_tuner(&server.pool, "ONNOW002").await;
    let channel = make_channel(&server.pool, tuner, 7).await;
    make_live_guide(&server.pool, channel, "SERIESTHIN").await;

    let user_id = create_user_with_role(&server, "livetv-thin@example.com", "viewer").await;
    let resp = TestClient::new(&server.api_url)
        .post_json(
            "/api/v1/admin/login",
            &json!({ "email": "livetv-thin@example.com", "password": "testpass123" }),
        )
        .await;
    let token = resp.json::<Value>()["token"].as_str().unwrap().to_string();
    let viewer = TestClient::new(&server.api_url).with_auth(&token);

    livetv_affinity::upsert(
        &server.pool,
        user_id,
        livetv_affinity::UpsertAffinity {
            series_id: Some("SERIESTHIN".to_string()),
            channel_id: None,
            genre: None,
            weekday: None,
            hour: None,
            watch_count: 1,
            total_secs: 300,
            mean_completion: 0.2,
            last_watched_at: Some(OffsetDateTime::now_utc()),
        },
    )
    .await
    .unwrap();

    let body: Value = viewer.get("/api/v1/admin/livetv/on-now").await.json();
    let series = body["groups"]
        .as_array()
        .unwrap()
        .iter()
        .find(|g| g["key"] == "series")
        .expect("a series group");
    assert!(series["items"][0]["confidence"].is_null());
}

#[tokio::test]
async fn on_now_needs_a_logged_in_caller() {
    let server = TestServer::start_api_only().await;
    TestClient::new(&server.api_url)
        .get("/api/v1/admin/livetv/on-now")
        .await
        .assert_status(StatusCode::UNAUTHORIZED);
}

// =============================================================================
// Ad breaks
// =============================================================================

async fn seed_break(pool: &sqlx::PgPool, recording_id: Uuid) -> Uuid {
    livetv_ad_breaks::create(
        pool,
        livetv_ad_breaks::CreateAdBreak {
            recording_id,
            start_seconds: 164.2,
            end_seconds: 374.2,
            method: "combined".to_string(),
            confidence: 0.8,
            ads_known: 1,
            ads_total: 7,
        },
    )
    .await
    .unwrap()
    .id
}

#[tokio::test]
async fn ad_breaks_are_listed_corrected_and_deleted() {
    let server = TestServer::start_api_only().await;
    let tuner = make_tuner(&server.pool, "ADBRK001").await;
    let channel = make_channel(&server.pool, tuner, 7).await;
    let recording = make_recording(&server.pool, channel).await;
    let break_id = seed_break(&server.pool, recording).await;

    let contributor =
        create_user_client(&server, "livetv-ad-contrib@example.com", "contributor").await;

    let listed: Value = contributor
        .get(&format!(
            "/api/v1/admin/livetv/recordings/{recording}/ad-breaks"
        ))
        .await
        .json();
    assert_eq!(listed["items"].as_array().unwrap().len(), 1);
    assert_eq!(listed["items"][0]["method"], "combined");
    assert_eq!(listed["items"][0]["ads_total"], 7);

    // The recording payload carries the marks so the player needs one call.
    let recording_body: Value = contributor
        .get(&format!("/api/v1/admin/livetv/recordings/{recording}"))
        .await
        .json();
    assert_eq!(recording_body["ad_breaks"].as_array().unwrap().len(), 1);

    let corrected: Value = contributor
        .patch_json(
            &format!("/api/v1/admin/livetv/recordings/{recording}/ad-breaks/{break_id}"),
            &json!({ "start_seconds": 170.0, "end_seconds": 372.0 }),
        )
        .await
        .json();
    assert_eq!(corrected["start_seconds"], 170.0);
    assert!(!corrected["corrected_at"].is_null());

    let edl = contributor
        .get(&format!(
            "/api/v1/admin/livetv/recordings/{recording}/ad-breaks.edl"
        ))
        .await;
    edl.assert_ok();

    contributor
        .delete(&format!(
            "/api/v1/admin/livetv/recordings/{recording}/ad-breaks/{break_id}"
        ))
        .await
        .assert_status(StatusCode::NO_CONTENT);
    let listed: Value = contributor
        .get(&format!(
            "/api/v1/admin/livetv/recordings/{recording}/ad-breaks"
        ))
        .await
        .json();
    assert!(listed["items"].as_array().unwrap().is_empty());
}

#[tokio::test]
async fn ad_break_corrections_reject_bad_spans_and_unknown_ids() {
    let server = TestServer::start_api_only().await;
    let tuner = make_tuner(&server.pool, "ADBRK002").await;
    let channel = make_channel(&server.pool, tuner, 7).await;
    let recording = make_recording(&server.pool, channel).await;
    let break_id = seed_break(&server.pool, recording).await;

    let contributor =
        create_user_client(&server, "livetv-ad-contrib2@example.com", "contributor").await;

    contributor
        .patch_json(
            &format!("/api/v1/admin/livetv/recordings/{recording}/ad-breaks/{break_id}"),
            &json!({ "start_seconds": 300.0, "end_seconds": 100.0 }),
        )
        .await
        .assert_status(StatusCode::BAD_REQUEST);
    contributor
        .patch_json(
            &format!(
                "/api/v1/admin/livetv/recordings/{recording}/ad-breaks/{}",
                Uuid::new_v4()
            ),
            &json!({ "start_seconds": 10.0, "end_seconds": 40.0 }),
        )
        .await
        .assert_status(StatusCode::NOT_FOUND);
    contributor
        .get(&format!(
            "/api/v1/admin/livetv/recordings/{}/ad-breaks",
            Uuid::new_v4()
        ))
        .await
        .assert_status(StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn ad_breaks_need_a_contributor() {
    let server = TestServer::start_api_only().await;
    let tuner = make_tuner(&server.pool, "ADBRK003").await;
    let channel = make_channel(&server.pool, tuner, 7).await;
    let recording = make_recording(&server.pool, channel).await;
    let break_id = seed_break(&server.pool, recording).await;

    let viewer = create_user_client(&server, "livetv-ad-viewer@example.com", "viewer").await;
    viewer
        .get(&format!(
            "/api/v1/admin/livetv/recordings/{recording}/ad-breaks"
        ))
        .await
        .assert_status(StatusCode::FORBIDDEN);
    // Valid body: the guard runs inside the handler, so a bad body would 422 first.
    viewer
        .patch_json(
            &format!("/api/v1/admin/livetv/recordings/{recording}/ad-breaks/{break_id}"),
            &json!({ "start_seconds": 10.0, "end_seconds": 40.0 }),
        )
        .await
        .assert_status(StatusCode::FORBIDDEN);
    viewer
        .delete(&format!(
            "/api/v1/admin/livetv/recordings/{recording}/ad-breaks/{break_id}"
        ))
        .await
        .assert_status(StatusCode::FORBIDDEN);
    TestClient::new(&server.api_url)
        .get(&format!(
            "/api/v1/admin/livetv/recordings/{recording}/ad-breaks"
        ))
        .await
        .assert_status(StatusCode::UNAUTHORIZED);
}

// =============================================================================
// Recordings are library items (Decision 16)
// =============================================================================

use api::repositories::{movie_files, movies, tv_episode_files, tv_episodes, tv_seasons, tv_shows};

async fn seed_episode(pool: &sqlx::PgPool, title: &str) -> (Uuid, Uuid) {
    let show_id = tv_shows::create(
        pool,
        tv_shows::TvShowInput {
            title: title.to_string(),
            year: Some(2025),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    let season_id = tv_seasons::upsert(
        pool,
        tv_seasons::SeasonInput {
            show_id,
            season_identifier: "1".to_string(),
            season_sort_order: 1,
            title: Some("Season 1".to_string()),
            description: None,
            images: None,
            air_date: None,
            episode_count: None,
            is_monitored: false,
            is_special: false,
            auto_want_mode: "inherit".to_string(),
        },
    )
    .await
    .unwrap();
    let episode_id = tv_episodes::create(
        pool,
        tv_episodes::EpisodeInput {
            season_id,
            episode_identifier: "3".to_string(),
            episode_sort_order: 3,
            title: Some("Episode 3".to_string()),
            description: None,
            images: None,
            air_date: None,
            air_time: None,
            runtime: None,
            identifiers: json!({}),
            country: None,
        },
    )
    .await
    .unwrap();
    (show_id, episode_id)
}

async fn completed_recording(pool: &sqlx::PgPool, channel_id: Uuid, title: &str) -> Uuid {
    completed_recording_with(pool, channel_id, title, Some("S01E03")).await
}

async fn completed_recording_with(
    pool: &sqlx::PgPool,
    channel_id: Uuid,
    title: &str,
    episode_number: Option<&str>,
) -> Uuid {
    let now = OffsetDateTime::now_utc();
    livetv_recordings::create(
        pool,
        livetv_recordings::CreateRecording {
            channel_id,
            guide_id: None,
            series_rule_id: None,
            title: title.to_string(),
            episode_number: episode_number.map(str::to_string),
            episode_title: None,
            episode_key: None,
            series_id: None,
            synopsis: None,
            scheduled_start: now - time::Duration::hours(1),
            scheduled_end: now,
            actual_start: Some(now - time::Duration::hours(1)),
            status: "completed".to_string(),
            output_dir: Some("/tmp/livetv-import-test".to_string()),
            container: Some("hls".to_string()),
            video_codec: None,
            audio_codec: None,
            padding_before_secs: 0,
            padding_after_secs: 0,
            image_url: None,
        },
    )
    .await
    .unwrap()
    .id
}

/// A matched recording becomes an additional copy, which is what puts it in Up
/// next — `dashboard::watch_next_episodes` gates availability on
/// `tv_episode_file_episodes`.
#[tokio::test]
async fn a_matched_recording_joins_the_library_beside_the_downloaded_copy() {
    let server = TestServer::start_api_only().await;
    let tuner = make_tuner(&server.pool, "IMPORT01").await;
    let channel = make_channel(&server.pool, tuner, 7).await;
    let (_show, episode_id) = seed_episode(&server.pool, "Hard Quiz Import").await;

    // A downloaded copy already exists; the recording must not replace it.
    let downloaded = tv_episode_files::create(
        &server.pool,
        tv_episode_files::EpisodeFileInput {
            file_path: "/library/hard-quiz-s01e03.mkv".to_string(),
            file_size: Some(1),
            file_fingerprint: None,
            format: "mkv".to_string(),
            quality: None,
            languages: None,
            subtitles: None,
            source_type: Some("native".to_string()),
            source_metadata: None,
            video_codec: Some("h264".to_string()),
            video_width: None,
            video_height: None,
            video_bitrate_kbps: None,
            framerate: None,
            bit_depth: None,
            is_hdr: false,
            is_interlaced: false,
            hdr_format: None,
            duration_seconds: Some(1800.0),
            container: Some("mkv".to_string()),
            overall_bitrate_kbps: None,
        },
    )
    .await
    .unwrap();
    tv_episode_files::link_episodes(&server.pool, downloaded, &[episode_id])
        .await
        .unwrap();

    let recording_id = completed_recording(&server.pool, channel, "Hard Quiz Import").await;
    let row = livetv_recordings::get_by_id(&server.pool, recording_id)
        .await
        .unwrap()
        .unwrap();
    let imported = api::livetv::library::import(
        &server.pool,
        &row,
        std::path::Path::new("/tmp/livetv-import-test/recording.m3u8"),
    )
    .await
    .unwrap();

    let file_id = match imported {
        api::livetv::library::Imported::Episode {
            file_id,
            episode_id: e,
        } => {
            assert_eq!(e, episode_id);
            file_id
        }
        other => panic!("expected an episode match, got {other:?}"),
    };

    let after = livetv_recordings::get_by_id(&server.pool, recording_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(after.episode_id, Some(episode_id));
    assert!(after.match_attempted_at.is_some());

    let files = tv_episode_files::list_by_episode(&server.pool, episode_id)
        .await
        .unwrap();
    assert_eq!(
        files.len(),
        2,
        "a recording is an additional copy, not a replacement"
    );
    let recorded = files
        .iter()
        .find(|f| f.id == file_id)
        .expect("the recording");
    assert_eq!(recorded.source_type.as_deref(), Some("livetv"));
    let provenance = recorded.source_metadata.clone().expect("provenance");
    assert_eq!(provenance["recording_id"], recording_id.to_string());
    assert_eq!(provenance["channel_id"], channel.to_string());
    assert!(provenance["channel_name"].is_string());
    // The probe walker claims NULL-codec rows, and probing a manifest
    // quarantines it — so the codec must never be left unset.
    assert!(recorded.video_codec.is_some());

    let ids = tv_episode_files::get_episode_ids(&server.pool, file_id)
        .await
        .unwrap();
    assert_eq!(ids, vec![episode_id]);
}

/// A recorded film must behave like any other movie: it needs a `movie_files`
/// row, otherwise it is never playable from `/movies/{id}`, never accrues a
/// `movie_watch_states` position, and so can never reach `watch_next`.
#[tokio::test]
async fn a_matched_movie_recording_joins_the_library_and_continue_watching() {
    let server = TestServer::start_api_only().await;
    let tuner = make_tuner(&server.pool, "IMPORT03").await;
    let channel = make_channel(&server.pool, tuner, 7).await;
    let movie_id = movies::create(
        &server.pool,
        movies::MovieInput {
            title: "The Recorded Feature".to_string(),
            year: Some(1998),
            ..Default::default()
        },
    )
    .await
    .unwrap();

    let recording_id =
        completed_recording_with(&server.pool, channel, "The Recorded Feature", None).await;
    let row = livetv_recordings::get_by_id(&server.pool, recording_id)
        .await
        .unwrap()
        .unwrap();
    let imported = api::livetv::library::import(
        &server.pool,
        &row,
        std::path::Path::new("/tmp/livetv-import-test/recording.m3u8"),
    )
    .await
    .unwrap();

    let file_id = match imported {
        api::livetv::library::Imported::Movie {
            file_id,
            movie_id: m,
        } => {
            assert_eq!(m, movie_id);
            file_id
        }
        other => panic!("expected a movie match, got {other:?}"),
    };

    let after = livetv_recordings::get_by_id(&server.pool, recording_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(after.movie_id, Some(movie_id));
    assert!(after.episode_id.is_none());

    let files = movie_files::list_by_movie(&server.pool, movie_id)
        .await
        .unwrap();
    assert_eq!(files.len(), 1, "the recording is the movie's only copy");
    let recorded = files
        .iter()
        .find(|f| f.id == file_id)
        .expect("the recording");
    assert_eq!(recorded.source_type.as_deref(), Some("livetv"));
    assert!(recorded.available);
    // The probe walker claims NULL-codec rows, and probing a manifest
    // quarantines it — so the codec must never be left unset.
    assert!(recorded.video_codec.is_some());
    let provenance = recorded.source_metadata.clone().expect("provenance");
    assert_eq!(provenance["recording_id"], recording_id.to_string());
    assert_eq!(provenance["channel_id"], channel.to_string());

    // Retention deletes the manifest, so it has to be able to find and unlink
    // the copy the import created.
    assert_eq!(
        livetv_recordings::find_movie_file_for_recording(&server.pool, recording_id)
            .await
            .unwrap(),
        Some(file_id)
    );

    let detail: Value = server
        .admin_client()
        .get(&format!("/api/v1/admin/movies/{movie_id}"))
        .await
        .json();
    assert_eq!(detail["is_in_library"], true);

    server
        .admin_client()
        .patch_json(
            &format!("/api/v1/admin/movies/{movie_id}/watch-state/position"),
            &json!({ "position_seconds": 120 }),
        )
        .await
        .assert_ok();

    let discover: Value = server
        .admin_client()
        .get("/api/v1/admin/discover")
        .await
        .json();
    assert!(
        discover["watch_next"]
            .as_array()
            .unwrap()
            .iter()
            .any(|i| i["media_id"].as_str() == Some(&movie_id.to_string())),
        "a part-watched recorded movie belongs in watch_next: {}",
        discover["watch_next"]
    );
}

/// Never block on a match: the recording stays watchable under its channel and
/// programme title, it just gets no siblings.
#[tokio::test]
async fn an_unmatched_recording_is_still_completed_and_watchable() {
    let server = TestServer::start_api_only().await;
    let tuner = make_tuner(&server.pool, "IMPORT02").await;
    let channel = make_channel(&server.pool, tuner, 7).await;
    let recording_id =
        completed_recording(&server.pool, channel, "Nothing In The Library At All").await;
    let row = livetv_recordings::get_by_id(&server.pool, recording_id)
        .await
        .unwrap()
        .unwrap();

    let imported = api::livetv::library::import(
        &server.pool,
        &row,
        std::path::Path::new("/tmp/livetv-import-test/recording.m3u8"),
    )
    .await
    .unwrap();
    assert_eq!(imported, api::livetv::library::Imported::Unmatched);

    let after = livetv_recordings::get_by_id(&server.pool, recording_id)
        .await
        .unwrap()
        .unwrap();
    assert!(after.episode_id.is_none());
    assert!(after.movie_id.is_none());
    // Attempted, so the sweeper does not keep retrying a title nothing matches.
    assert!(after.match_attempted_at.is_some());

    let contributor =
        create_user_client(&server, "livetv-unmatched@example.com", "contributor").await;
    let body: Value = contributor
        .get(&format!("/api/v1/admin/livetv/recordings/{recording_id}"))
        .await
        .json();
    assert_eq!(body["status"], "completed");
    assert_eq!(body["playable_from_start"], true);
    assert_eq!(body["provenance"]["channel_name"], "CH7");
}

// =============================================================================
// Retention
// =============================================================================

#[tokio::test]
async fn retention_candidates_come_back_newest_first_within_a_rule() {
    let server = TestServer::start_api_only().await;
    let tuner = make_tuner(&server.pool, "RETAIN01").await;
    let channel = make_channel(&server.pool, tuner, 7).await;
    let rule = api::repositories::livetv_series_rules::create(
        &server.pool,
        api::repositories::livetv_series_rules::CreateSeriesRule {
            series_id: "RETAINSERIES".to_string(),
            title: "Retain Me".to_string(),
            enabled: true,
            channel_ids: vec![],
            weekdays: vec![],
            window_start: None,
            window_end: None,
            new_only: false,
            min_season: None,
            keep_count: Some(1),
            keep_until: "watched".to_string(),
            keep_until_date: None,
            padding_before_secs: 0,
            padding_after_secs: 0,
            on_clash: "move".to_string(),
            priority: 0,
        },
    )
    .await
    .unwrap()
    .id;

    let now = OffsetDateTime::now_utc();
    let mut ids = Vec::new();
    for hours in [3i64, 1] {
        let created = livetv_recordings::create(
            &server.pool,
            livetv_recordings::CreateRecording {
                channel_id: channel,
                guide_id: None,
                series_rule_id: Some(rule),
                title: "Retain Me".to_string(),
                episode_number: None,
                episode_title: None,
                episode_key: None,
                series_id: Some("RETAINSERIES".to_string()),
                synopsis: None,
                scheduled_start: now - time::Duration::hours(hours + 1),
                scheduled_end: now - time::Duration::hours(hours),
                actual_start: Some(now - time::Duration::hours(hours + 1)),
                status: "completed".to_string(),
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
        ids.push(created.id);
    }

    let candidates = livetv_recordings::retention_candidates(&server.pool)
        .await
        .unwrap();
    let mine: Vec<_> = candidates
        .iter()
        .filter(|c| c.series_rule_id == rule)
        .collect();
    assert_eq!(mine.len(), 2);
    assert_eq!(
        mine[0].id, ids[1],
        "newest first, so keep_count counts down"
    );
    assert_eq!(mine[0].keep_count, Some(1));
    assert_eq!(mine[0].keep_until, "watched");

    let expired = api::livetv::retention::expired(
        &mine.iter().map(|c| (*c).clone()).collect::<Vec<_>>(),
        &[],
        now,
        api::livetv::retention::DiskState { free_ratio: 0.8 },
    );
    assert_eq!(expired.len(), 1);
    assert_eq!(expired[0].recording_id, ids[0], "the older one goes");
}

// =============================================================================
// The affinity rollup (Decision 14)
// =============================================================================

/// `mean_completion` is what stops this being a naive counter: a series watched
/// to the end must outrank one abandoned five minutes in, at equal counts.
#[tokio::test]
async fn the_rollup_turns_watch_events_into_grains_that_rank_by_completion() {
    use api::jobs::handlers::LivetvAffinityHandler;
    use api::jobs::worker::JobHandler;
    use api::repositories::livetv_watch_events;
    use tokio_util::sync::CancellationToken;

    let server = TestServer::start_api_only().await;
    let tuner = make_tuner(&server.pool, "ROLLUP01").await;
    let channel = make_channel(&server.pool, tuner, 7).await;
    let user_id = create_user_with_role(&server, "livetv-rollup@example.com", "viewer").await;

    let now = OffsetDateTime::now_utc();
    for (series, watched, i) in [
        ("FINISHED", 1700, 0),
        ("FINISHED", 1750, 1),
        ("FINISHED", 1780, 2),
        ("BAILED", 300, 3),
        ("BAILED", 250, 4),
        ("BAILED", 280, 5),
    ] {
        livetv_watch_events::create(
            &server.pool,
            livetv_watch_events::CreateWatchEvent {
                user_id: Some(user_id),
                channel_id: channel,
                guide_id: None,
                series_id: Some(series.to_string()),
                title: Some(series.to_string()),
                episode_key: None,
                started_at: now - time::Duration::days(i + 1),
                ended_at: Some(now - time::Duration::days(i + 1) + time::Duration::minutes(30)),
                watched_secs: watched,
                programme_secs: Some(1800),
                source: "live".to_string(),
            },
        )
        .await
        .unwrap();
    }

    let out = LivetvAffinityHandler::new(server.pool.clone())
        .execute(Uuid::now_v7(), json!({}), CancellationToken::new())
        .await
        .expect("the rollup runs");
    assert!(out["users"].as_i64().unwrap() >= 1);
    assert!(out["rows"].as_i64().unwrap() > 0);

    let rows = livetv_affinity::series_affinity(
        &server.pool,
        user_id,
        &["FINISHED".to_string(), "BAILED".to_string()],
    )
    .await
    .unwrap();
    let finished = rows
        .iter()
        .find(|r| r.series_id.as_deref() == Some("FINISHED"))
        .expect("the finished series");
    let bailed = rows
        .iter()
        .find(|r| r.series_id.as_deref() == Some("BAILED"))
        .expect("the abandoned series");
    assert_eq!(finished.watch_count, 3);
    assert_eq!(bailed.watch_count, 3);
    assert!(
        finished.mean_completion > bailed.mean_completion,
        "abandonment is a negative signal, not a neutral one"
    );
    assert!(
        api::livetv::affinity::confidence(
            finished.watch_count,
            finished.mean_completion,
            0.5,
            true
        ) > api::livetv::affinity::confidence(
            bailed.watch_count,
            bailed.mean_completion,
            0.5,
            true
        )
    );

    // The channel grain is one query, not a scan of the events.
    let by_channel = livetv_affinity::channel_affinity(&server.pool, user_id, &[channel])
        .await
        .unwrap();
    assert_eq!(by_channel.len(), 1);
    assert_eq!(by_channel[0].watch_count, 6);

    // A recompute SETs rather than accumulates.
    LivetvAffinityHandler::new(server.pool.clone())
        .execute(Uuid::now_v7(), json!({}), CancellationToken::new())
        .await
        .expect("a second rollup");
    let again = livetv_affinity::channel_affinity(&server.pool, user_id, &[channel])
        .await
        .unwrap();
    assert_eq!(
        again[0].watch_count, 6,
        "the rollup recomputes, it does not add"
    );
}
