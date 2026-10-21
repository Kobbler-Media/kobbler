use crate::common::*;
use api::livetv::{classify, guide_sync, hdhomerun};
use api::repositories::{livetv_channels, livetv_guide, livetv_recordings, livetv_tuners};
use reqwest::StatusCode;
use serde_json::{Value, json};
use time::OffsetDateTime;
use uuid::Uuid;

const LINEUP_FIXTURE: &str = include_str!("../fixtures/livetv/lineup.json");
const GUIDE_FIXTURE: &str = include_str!("../fixtures/livetv/guide.json");

// =============================================================================
// Fixtures
// =============================================================================

async fn make_tuner(pool: &sqlx::PgPool, device_id: &str) -> Uuid {
    livetv_tuners::upsert(
        pool,
        livetv_tuners::CreateTuner {
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

/// Mirrors what `livetv_scan` does with a lineup, without touching the device.
/// The guide route only lists visible channels, so a test that seeds against an id the
/// route would never return proves nothing.
async fn enabled_channels(pool: &sqlx::PgPool, tuner_id: Uuid, want: i64) -> Vec<Uuid> {
    let (rows, _) = livetv_channels::list_filtered(
        pool,
        &livetv_channels::ChannelFilters {
            tuner_id: Some(tuner_id),
            enabled: Some(true),
            ..Default::default()
        },
        None,
        want,
        0,
    )
    .await
    .unwrap();
    rows.iter().map(|r| r.channel.id).collect()
}

async fn ingest_lineup(pool: &sqlx::PgPool, tuner_id: Uuid) -> Vec<Uuid> {
    let entries: Vec<hdhomerun::LineupEntry> = serde_json::from_str(LINEUP_FIXTURE).unwrap();
    let mut ids = Vec::new();
    for e in &entries {
        let class = classify::classify(e, false);
        let (row, _) = livetv_channels::upsert_scan(
            pool,
            livetv_channels::UpsertScanChannel {
                tuner_id,
                lcn: e.lcn(),
                scan_name: e.guide_name.clone(),
                frequency: e.frequency,
                program_number: i32::try_from(e.program_number).unwrap(),
                onid: e.original_network_id.and_then(|v| i32::try_from(v).ok()),
                tsid: e.transport_stream_id.and_then(|v| i32::try_from(v).ok()),
                modulation: e.modulation.clone(),
                video_codec: e.video_codec.clone(),
                audio_codec: e.audio_codec.clone(),
                hd: e.is_hd(),
                signal_strength: e.signal_strength,
                signal_quality: e.signal_quality,
                initial_kind: class.kind.as_str().to_string(),
                initial_enabled: class.enabled_on_first_scan,
            },
        )
        .await
        .unwrap();
        ids.push(row.id);
    }
    ids
}

/// The cached guide feed is anchored to the hour it was captured. Rebase it onto
/// "now" so the prune step and `is_now` stay deterministic as the fixture ages.
fn rebased_guide() -> Vec<hdhomerun::GuideChannel> {
    let mut feed: Vec<hdhomerun::GuideChannel> = serde_json::from_str(GUIDE_FIXTURE).unwrap();
    let earliest = feed
        .iter()
        .flat_map(|c| c.guide.iter())
        .map(|p| p.start_time)
        .min()
        .unwrap();
    let offset = OffsetDateTime::now_utc().unix_timestamp() - 1800 - earliest;
    for c in &mut feed {
        for p in &mut c.guide {
            p.start_time += offset;
            p.end_time += offset;
        }
    }
    feed
}

async fn seed_guide(pool: &sqlx::PgPool, channel_id: Uuid, offsets_mins: &[(i64, i64)]) {
    let now = OffsetDateTime::now_utc();
    for (i, (start, end)) in offsets_mins.iter().enumerate() {
        livetv_guide::upsert(
            pool,
            livetv_guide::UpsertGuideEntry {
                channel_id,
                start_time: now + time::Duration::minutes(*start),
                end_time: now + time::Duration::minutes(*end),
                title: format!("Programme {i}"),
                episode_number: Some(format!("S01E0{i}")),
                episode_title: None,
                synopsis: Some("A synopsis".to_string()),
                original_airdate: None,
                series_id: Some("SERIES1".to_string()),
                genres: vec!["News".to_string()],
                image_url: None,
                poster_url: None,
                is_first: false,
                episode_key: Some(format!("SERIES1|E:S01E0{i}")),
                episode_key_kind: Some("episode".to_string()),
            },
        )
        .await
        .unwrap();
    }
}

// =============================================================================
// Scan + guide ingestion against the real captured feeds
// =============================================================================

#[tokio::test]
async fn scan_classifies_the_real_lineup_into_the_expected_buckets() {
    let server = TestServer::start_api_only().await;
    let tuner_id = make_tuner(&server.pool, "SCAN0001").await;
    let ids = ingest_lineup(&server.pool, tuner_id).await;
    assert_eq!(ids.len(), 107);

    let (total, enabled) = livetv_channels::count_by_tuner(&server.pool, tuner_id)
        .await
        .unwrap();
    assert_eq!(total, 107);
    assert_eq!(
        enabled, 69,
        "only real TV services are on after a first scan"
    );

    let (rows, _) = livetv_channels::list_filtered(
        &server.pool,
        &livetv_channels::ChannelFilters {
            tuner_id: Some(tuner_id),
            ..Default::default()
        },
        None,
        1000,
        0,
    )
    .await
    .unwrap();

    let mut buckets = std::collections::HashMap::new();
    for r in &rows {
        let kind = classify::Kind::parse(&r.channel.kind).unwrap();
        let label = classify::type_label(kind, r.channel.video_codec.as_deref(), r.channel.hd);
        *buckets.entry(label).or_insert(0) += 1;
    }
    assert_eq!(buckets["HD"], 39);
    assert_eq!(buckets["SD H.264"], 20);
    assert_eq!(buckets["SD MPEG-2"], 10);
    assert_eq!(buckets["Radio"], 17);
    assert_eq!(buckets["Dead"], 21);
}

#[tokio::test]
async fn rescan_overwrites_scan_columns_but_never_curation() {
    let server = TestServer::start_api_only().await;
    let tuner_id = make_tuner(&server.pool, "SCAN0002").await;
    let ids = ingest_lineup(&server.pool, tuner_id).await;

    let curated = ids[0];
    livetv_channels::update_curated(
        &server.pool,
        curated,
        livetv_channels::UpdateCuratedChannel {
            enabled: Some(false),
            favourite: Some(true),
            display_name: Some(Some("My Name".to_string())),
            region: Some(Some("Region".to_string())),
            ..Default::default()
        },
    )
    .await
    .unwrap();

    ingest_lineup(&server.pool, tuner_id).await;

    let after = livetv_channels::get_by_id(&server.pool, curated)
        .await
        .unwrap()
        .unwrap();
    assert!(
        !after.enabled,
        "a rescan must not re-enable a hidden channel"
    );
    assert!(after.favourite);
    assert_eq!(after.display_name.as_deref(), Some("My Name"));
    assert_eq!(after.region.as_deref(), Some("Region"));
    assert!(after.scan_name.is_some());

    let (total, _) = livetv_channels::count_by_tuner(&server.pool, tuner_id)
        .await
        .unwrap();
    assert_eq!(total, 107, "physical identity dedupes the rescan");
}

#[tokio::test]
async fn a_channel_missing_from_a_rescan_is_disabled_not_deleted() {
    let server = TestServer::start_api_only().await;
    let tuner_id = make_tuner(&server.pool, "SCAN0003").await;
    let ids = ingest_lineup(&server.pool, tuner_id).await;

    let survivors: Vec<Uuid> = ids.iter().skip(1).copied().collect();
    let disabled = livetv_channels::disable_unseen(&server.pool, tuner_id, &survivors)
        .await
        .unwrap();
    assert_eq!(disabled, 1);

    let gone = livetv_channels::get_by_id(&server.pool, ids[0])
        .await
        .unwrap();
    assert!(gone.is_some(), "curation must survive a bad scan");
    assert!(!gone.unwrap().enabled);
}

#[tokio::test]
async fn guide_ingest_matches_every_lcn_and_keys_almost_every_programme() {
    let server = TestServer::start_api_only().await;
    let tuner_id = make_tuner(&server.pool, "GUIDE001").await;
    ingest_lineup(&server.pool, tuner_id).await;

    let feed = rebased_guide();
    let stats = guide_sync::ingest(&server.pool, tuner_id, &feed)
        .await
        .unwrap();

    assert_eq!(stats.channels, 85);
    assert_eq!(stats.programmes, 449);
    assert_eq!(stats.upserted, 449);
    assert_eq!(stats.unmatched_lcns, 0, "every guide LCN must resolve");
    assert_eq!(stats.unmatched_programmes, 0);
    assert_eq!(stats.safe_episode_keys, 441);
    assert_eq!(stats.title_only_keys, 8);
    assert_eq!(livetv_guide::count_all(&server.pool).await.unwrap(), 449);

    // Re-running is an upsert, not a duplicate.
    let again = guide_sync::ingest(&server.pool, tuner_id, &feed)
        .await
        .unwrap();
    assert_eq!(again.upserted, 449);
    assert_eq!(livetv_guide::count_all(&server.pool).await.unwrap(), 449);

    let tuner = livetv_tuners::get_by_id(&server.pool, tuner_id)
        .await
        .unwrap()
        .unwrap();
    assert!(tuner.last_guide_sync_at.is_some());
}

#[tokio::test]
async fn guide_ingest_sets_network_once_and_never_overwrites_it() {
    let server = TestServer::start_api_only().await;
    let tuner_id = make_tuner(&server.pool, "GUIDE002").await;
    ingest_lineup(&server.pool, tuner_id).await;

    let feed = rebased_guide();
    guide_sync::ingest(&server.pool, tuner_id, &feed)
        .await
        .unwrap();

    let lcn_map: std::collections::HashMap<i32, Uuid> =
        livetv_channels::lcn_map(&server.pool, tuner_id)
            .await
            .unwrap()
            .into_iter()
            .collect();
    let with_affiliate = feed.iter().find(|c| c.affiliate.is_some()).unwrap();
    let channel_id = lcn_map[&with_affiliate.lcn().unwrap()];

    let row = livetv_channels::get_by_id(&server.pool, channel_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(row.network.as_deref(), with_affiliate.affiliate.as_deref());
    assert_eq!(
        row.callsign.as_deref(),
        with_affiliate.guide_name.as_deref()
    );

    livetv_channels::update_curated(
        &server.pool,
        channel_id,
        livetv_channels::UpdateCuratedChannel {
            network: Some(Some("User Network".to_string())),
            ..Default::default()
        },
    )
    .await
    .unwrap();

    guide_sync::ingest(&server.pool, tuner_id, &feed)
        .await
        .unwrap();
    let row = livetv_channels::get_by_id(&server.pool, channel_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(row.network.as_deref(), Some("User Network"));
}

#[tokio::test]
async fn guide_ingest_fills_the_channel_logo_once_and_never_overwrites_it() {
    let server = TestServer::start_api_only().await;
    let tuner_id = make_tuner(&server.pool, "GUIDE003").await;
    ingest_lineup(&server.pool, tuner_id).await;

    let feed = rebased_guide();
    guide_sync::ingest(&server.pool, tuner_id, &feed)
        .await
        .unwrap();

    let lcn_map: std::collections::HashMap<i32, Uuid> =
        livetv_channels::lcn_map(&server.pool, tuner_id)
            .await
            .unwrap()
            .into_iter()
            .collect();
    let with_logo = feed.iter().find(|c| c.image_url.is_some()).unwrap();
    let channel_id = lcn_map[&with_logo.lcn().unwrap()];

    let row = livetv_channels::get_by_id(&server.pool, channel_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(row.logo_url.as_deref(), with_logo.image_url.as_deref());

    livetv_channels::update_curated(
        &server.pool,
        channel_id,
        livetv_channels::UpdateCuratedChannel {
            logo_url: Some(Some("https://example.test/mine.png".to_string())),
            ..Default::default()
        },
    )
    .await
    .unwrap();

    guide_sync::ingest(&server.pool, tuner_id, &feed)
        .await
        .unwrap();
    let row = livetv_channels::get_by_id(&server.pool, channel_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        row.logo_url.as_deref(),
        Some("https://example.test/mine.png")
    );
}

#[tokio::test]
async fn guide_sync_prunes_entries_older_than_a_day() {
    let server = TestServer::start_api_only().await;
    let tuner_id = make_tuner(&server.pool, "PRUNE001").await;
    let ids = ingest_lineup(&server.pool, tuner_id).await;

    seed_guide(&server.pool, ids[0], &[(-3000, -2940), (-30, 30)]).await;
    assert_eq!(livetv_guide::count_all(&server.pool).await.unwrap(), 2);

    let pruned = livetv_guide::prune_expired(&server.pool, guide_sync::PRUNE_AFTER_HOURS)
        .await
        .unwrap();
    assert_eq!(pruned, 1);
    assert_eq!(livetv_guide::count_all(&server.pool).await.unwrap(), 1);
}

// =============================================================================
// Tuner routes
// =============================================================================

#[tokio::test]
async fn tuner_list_delete_and_scan_enqueue() {
    let server = TestServer::start_api_only().await;
    let client = server.admin_client();
    let tuner_id = make_tuner(&server.pool, "ROUTE001").await;
    ingest_lineup(&server.pool, tuner_id).await;

    let resp = client.get("/api/v1/admin/livetv/tuners").await;
    resp.assert_ok();
    let body: Value = resp.json();
    let item = body["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|t| t["id"] == tuner_id.to_string())
        .unwrap();
    assert_eq!(item["device_id"], "ROUTE001");
    assert_eq!(item["channel_count"], 107);
    assert_eq!(item["enabled_channel_count"], 69);
    assert_eq!(item["leases_in_use"], 0);
    assert!(item.get("device_auth").is_none(), "device auth is a secret");

    let resp = client
        .post(&format!("/api/v1/admin/livetv/tuners/{tuner_id}/scan"))
        .await;
    resp.assert_status(StatusCode::ACCEPTED);
    assert!(resp.json::<Value>()["job_id"].is_string());

    let resp = client
        .post(&format!(
            "/api/v1/admin/livetv/tuners/{tuner_id}/guide-sync"
        ))
        .await;
    resp.assert_status(StatusCode::ACCEPTED);

    let resp = client
        .delete(&format!("/api/v1/admin/livetv/tuners/{tuner_id}"))
        .await;
    resp.assert_status(StatusCode::NO_CONTENT);

    let resp = client
        .delete(&format!("/api/v1/admin/livetv/tuners/{tuner_id}"))
        .await;
    resp.assert_status(StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn deleting_a_tuner_refuses_to_take_its_recordings_with_it() {
    let server = TestServer::start_api_only().await;
    let client = server.admin_client();
    let tuner_id = make_tuner(&server.pool, "TUNERDEL1").await;
    let channels = ingest_lineup(&server.pool, tuner_id).await;
    let channel_id = channels[0];

    let now = OffsetDateTime::now_utc();
    livetv_recordings::create(
        &server.pool,
        livetv_recordings::CreateRecording {
            channel_id,
            guide_id: None,
            series_rule_id: None,
            title: "Something already captured".to_string(),
            episode_number: None,
            episode_title: None,
            episode_key: None,
            series_id: None,
            synopsis: None,
            scheduled_start: now,
            scheduled_end: now + time::Duration::minutes(30),
            actual_start: Some(now),
            status: "completed".to_string(),
            output_dir: None,
            container: Some("mp4".to_string()),
            video_codec: None,
            audio_codec: None,
            padding_before_secs: 0,
            padding_after_secs: 0,
            image_url: None,
        },
    )
    .await
    .unwrap();

    let resp = client
        .delete(&format!("/api/v1/admin/livetv/tuners/{tuner_id}"))
        .await;
    resp.assert_status(StatusCode::CONFLICT);

    assert!(
        livetv_tuners::get_by_id(&server.pool, tuner_id)
            .await
            .unwrap()
            .is_some(),
        "the refused delete must leave the tuner in place"
    );

    let resp = client
        .delete(&format!(
            "/api/v1/admin/livetv/tuners/{tuner_id}?force=true"
        ))
        .await;
    resp.assert_status(StatusCode::NO_CONTENT);
}

#[tokio::test]
async fn scan_of_an_unknown_tuner_is_a_404() {
    let server = TestServer::start_api_only().await;
    let resp = server
        .admin_client()
        .post(&format!(
            "/api/v1/admin/livetv/tuners/{}/scan",
            Uuid::new_v4()
        ))
        .await;
    resp.assert_status(StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn adding_an_unreachable_tuner_is_a_400() {
    let server = TestServer::start_api_only().await;
    let resp = server
        .admin_client()
        .post_json(
            "/api/v1/admin/livetv/tuners",
            &json!({ "base_url": "http://127.0.0.1:1" }),
        )
        .await;
    resp.assert_status(StatusCode::BAD_REQUEST);
}

// =============================================================================
// Channel routes
// =============================================================================

#[tokio::test]
async fn channel_list_prepopulates_derived_fields_and_facets() {
    let server = TestServer::start_api_only().await;
    let client = server.admin_client();
    let tuner_id = make_tuner(&server.pool, "CHAN0001").await;
    ingest_lineup(&server.pool, tuner_id).await;

    let resp = client
        .get("/api/v1/admin/livetv/channels?limit=500&sort=lcn")
        .await;
    resp.assert_ok();
    let body: Value = resp.json();
    assert_eq!(body["total"], 107);

    let first = &body["items"][0];
    assert!(first["name"].is_string());
    assert!(
        first["mux_label"].as_str().unwrap().ends_with(" MHz"),
        "mux_label must be server-computed"
    );
    assert!(first["type_label"].is_string());
    assert_eq!(first["has_guide"], false);
    assert_eq!(first["guide_entry_count"], 0);

    let types = body["facets"]["type"].as_array().unwrap();
    let hd = types.iter().find(|f| f["value"] == "HD").unwrap();
    assert_eq!(hd["count"], 39);
    assert!(!body["facets"]["mux"].as_array().unwrap().is_empty());
    assert!(!body["facets"]["status"].as_array().unwrap().is_empty());

    let resp = client
        .get("/api/v1/admin/livetv/channels?kind=radio&limit=500")
        .await;
    resp.assert_ok();
    assert_eq!(resp.json::<Value>()["total"], 17);

    let resp = client
        .get("/api/v1/admin/livetv/channels?enabled=true&hd=true&limit=500")
        .await;
    resp.assert_ok();
    assert_eq!(resp.json::<Value>()["total"], 39);
}

#[tokio::test]
async fn patch_rejects_every_scan_derived_column() {
    let server = TestServer::start_api_only().await;
    let client = server.admin_client();
    let tuner_id = make_tuner(&server.pool, "PATCH001").await;
    let ids = ingest_lineup(&server.pool, tuner_id).await;
    let id = ids[0];

    for (field, value) in [
        ("lcn", json!(5)),
        ("scan_name", json!("Nope")),
        ("frequency", json!(1)),
        ("hd", json!(true)),
        ("video_codec", json!("H264")),
        ("signal_quality", json!(50)),
        ("callsign", json!("ABC")),
    ] {
        let resp = client
            .patch_json(
                &format!("/api/v1/admin/livetv/channels/{id}"),
                &json!({ field: value }),
            )
            .await;
        resp.assert_status(StatusCode::BAD_REQUEST);
        let err = resp.json::<Value>()["error"].as_str().unwrap().to_string();
        assert!(err.contains(field), "error should name `{field}`: {err}");
        assert!(err.contains("scan-derived"), "{err}");
    }

    let resp = client
        .patch_json(
            &format!("/api/v1/admin/livetv/channels/{id}"),
            &json!({ "wat": 1 }),
        )
        .await;
    resp.assert_status(StatusCode::BAD_REQUEST);
    assert!(
        resp.json::<Value>()["error"]
            .as_str()
            .unwrap()
            .contains("unknown field")
    );
}

#[tokio::test]
async fn patch_writes_curated_columns_and_recomputes_the_name() {
    let server = TestServer::start_api_only().await;
    let client = server.admin_client();
    let tuner_id = make_tuner(&server.pool, "PATCH002").await;
    let ids = ingest_lineup(&server.pool, tuner_id).await;
    let id = ids[0];

    let resp = client
        .patch_json(
            &format!("/api/v1/admin/livetv/channels/{id}"),
            &json!({
                "display_name": "My Channel",
                "favourite": true,
                "enabled": false,
                "region": "Region",
                "sort_order": 3
            }),
        )
        .await;
    resp.assert_ok();
    let body: Value = resp.json();
    assert_eq!(body["display_name"], "My Channel");
    assert_eq!(body["name"], "My Channel");
    assert_eq!(body["favourite"], true);
    assert_eq!(body["enabled"], false);
    assert_eq!(body["region"], "Region");
    assert_eq!(body["sort_order"], 3);

    // An explicit null clears a curated column and the name falls back to the scan name.
    let resp = client
        .patch_json(
            &format!("/api/v1/admin/livetv/channels/{id}"),
            &json!({ "display_name": null }),
        )
        .await;
    resp.assert_ok();
    let body: Value = resp.json();
    assert!(body["display_name"].is_null());
    assert_eq!(body["name"], body["scan_name"]);

    let resp = client
        .patch_json(
            &format!("/api/v1/admin/livetv/channels/{id}"),
            &json!({ "kind": "nonsense" }),
        )
        .await;
    resp.assert_status(StatusCode::BAD_REQUEST);

    let resp = client
        .patch_json(
            &format!("/api/v1/admin/livetv/channels/{}", Uuid::new_v4()),
            &json!({ "favourite": true }),
        )
        .await;
    resp.assert_status(StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn bulk_channel_actions() {
    let server = TestServer::start_api_only().await;
    let client = server.admin_client();
    let tuner_id = make_tuner(&server.pool, "BULK0001").await;
    let ids = ingest_lineup(&server.pool, tuner_id).await;
    let subset: Vec<String> = ids.iter().take(5).map(|i| i.to_string()).collect();

    for action in ["enable", "hide", "favourite", "unfavourite"] {
        let resp = client
            .post_json(
                "/api/v1/admin/livetv/channels/bulk",
                &json!({ "ids": subset, "action": action }),
            )
            .await;
        resp.assert_ok();
        assert_eq!(resp.json::<Value>()["updated"], 5);
    }

    let resp = client
        .post_json(
            "/api/v1/admin/livetv/channels/bulk",
            &json!({ "ids": subset, "action": "explode" }),
        )
        .await;
    resp.assert_status(StatusCode::BAD_REQUEST);

    let resp = client
        .post_json(
            "/api/v1/admin/livetv/channels/bulk",
            &json!({ "ids": [], "action": "enable" }),
        )
        .await;
    resp.assert_status(StatusCode::BAD_REQUEST);
}

// =============================================================================
// Guide routes
// =============================================================================

#[tokio::test]
async fn guide_window_now_and_entry_lookup() {
    let server = TestServer::start_api_only().await;
    let client = server.admin_client();
    let tuner_id = make_tuner(&server.pool, "GROUTE01").await;
    let ids = ingest_lineup(&server.pool, tuner_id).await;

    let enabled = {
        let (rows, _) = livetv_channels::list_filtered(
            &server.pool,
            &livetv_channels::ChannelFilters {
                tuner_id: Some(tuner_id),
                enabled: Some(true),
                ..Default::default()
            },
            None,
            1,
            0,
        )
        .await
        .unwrap();
        rows[0].channel.id
    };
    assert!(ids.contains(&enabled));

    seed_guide(&server.pool, enabled, &[(-20, 40), (40, 100)]).await;

    let resp = client.get("/api/v1/admin/livetv/guide").await;
    resp.assert_ok();
    let body: Value = resp.json();
    let channel = body["channels"]
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["channel"]["id"] == enabled.to_string())
        .unwrap();
    let entries = channel["entries"].as_array().unwrap();
    assert_eq!(entries.len(), 2);
    assert_eq!(entries[0]["duration_seconds"], 3600);
    assert_eq!(entries[0]["is_now"], true);
    let pct = entries[0]["progress_pct"].as_f64().unwrap();
    assert!(pct > 30.0 && pct < 36.0, "progress_pct was {pct}");
    assert!(entries[0]["recording"].is_null());
    assert_eq!(entries[0]["repeat_count"], 0);

    let resp = client
        .get(&format!(
            "/api/v1/admin/livetv/guide/now?channel_ids={enabled}"
        ))
        .await;
    resp.assert_ok();
    let body: Value = resp.json();
    let item = &body["items"][0];
    assert_eq!(item["channel"]["id"], enabled.to_string());
    assert_eq!(item["now"]["title"], "Programme 0");
    assert_eq!(item["next"]["title"], "Programme 1");
    let mins = item["minutes_left"].as_i64().unwrap();
    assert!((38..=40).contains(&mins), "minutes_left was {mins}");

    let guide_id = entries[0]["id"].as_str().unwrap();
    let resp = client
        .get(&format!("/api/v1/admin/livetv/guide/{guide_id}"))
        .await;
    resp.assert_ok();
    assert_eq!(resp.json::<Value>()["title"], "Programme 0");

    let resp = client
        .get(&format!("/api/v1/admin/livetv/guide/{}", Uuid::new_v4()))
        .await;
    resp.assert_status(StatusCode::NOT_FOUND);
}

/// The index carries a projection of a guide entry — no episode number, duration,
/// recording state or repeat count. Hydrating its hits from Postgres is what keeps one
/// response shape whether the entries were found by index or by scan, so the hydration
/// query has to answer with whole rows and honour the same window the scan does.
#[tokio::test]
async fn guide_rows_hydrate_by_id_inside_the_window() {
    let server = TestServer::start_api_only().await;
    let tuner_id = make_tuner(&server.pool, "GHYDRA01").await;
    ingest_lineup(&server.pool, tuner_id).await;
    let visible = enabled_channels(&server.pool, tuner_id, 2).await;
    let channel = visible[0];
    let other = visible[1];

    seed_guide(&server.pool, channel, &[(-20, 40), (600, 660)]).await;
    seed_guide(&server.pool, other, &[(-20, 40)]).await;

    let now = OffsetDateTime::now_utc();
    let window_from = now - time::Duration::hours(1);
    let window_to = now + time::Duration::hours(4);

    let all = livetv_guide::list_window(&server.pool, window_from, window_to, None, None, None)
        .await
        .unwrap();
    let every_id: Vec<Uuid> = livetv_guide::list_window(
        &server.pool,
        now - time::Duration::hours(1),
        now + time::Duration::hours(24),
        None,
        None,
        None,
    )
    .await
    .unwrap()
    .iter()
    .map(|r| r.id)
    .collect();
    assert_eq!(
        every_id.len(),
        3,
        "seeded three airings across two channels"
    );
    assert_eq!(
        all.len(),
        2,
        "only two of them fall inside the four hour window"
    );

    // Whole rows, not a projection.
    let hydrated =
        livetv_guide::list_by_ids(&server.pool, &every_id, window_from, window_to, None, None)
            .await
            .unwrap();
    assert_eq!(
        hydrated.len(),
        2,
        "the entry 10 h out is outside the window"
    );
    let row = hydrated.iter().find(|r| r.channel_id == channel).unwrap();
    assert_eq!(row.episode_number.as_deref(), Some("S01E00"));
    assert_eq!(row.episode_key_kind.as_deref(), Some("episode"));
    assert_eq!(row.synopsis.as_deref(), Some("A synopsis"));
    assert_eq!(row.genres, vec!["News".to_string()]);

    // The channel filter is the guide route's own, and has to apply here too.
    let scoped = livetv_guide::list_by_ids(
        &server.pool,
        &every_id,
        window_from,
        window_to,
        Some(&[channel]),
        None,
    )
    .await
    .unwrap();
    assert_eq!(scoped.len(), 1);
    assert_eq!(scoped[0].channel_id, channel);

    // A genre nothing carries excludes everything.
    let by_genre = livetv_guide::list_by_ids(
        &server.pool,
        &every_id,
        window_from,
        window_to,
        None,
        Some("Sport"),
    )
    .await
    .unwrap();
    assert!(by_genre.is_empty(), "no seeded airing is Sport");

    // Ids the index knows about but the table no longer does are simply absent.
    let stale = livetv_guide::list_by_ids(
        &server.pool,
        &[Uuid::new_v4()],
        window_from,
        window_to,
        None,
        None,
    )
    .await
    .unwrap();
    assert!(stale.is_empty());
}

/// A query is answered across every channel at once, so the ~107 channels that matched
/// nothing are pure payload — and the clients render a flat result list from it, not a
/// grid that needs a row per channel. The unfiltered guide still carries them all.
#[tokio::test]
async fn a_guide_search_only_returns_channels_that_matched() {
    let server = TestServer::start_api_only().await;
    let client = server.admin_client();
    let tuner_id = make_tuner(&server.pool, "GSEARCH1").await;
    ingest_lineup(&server.pool, tuner_id).await;
    let visible = enabled_channels(&server.pool, tuner_id, 1).await;
    let matched = visible[0];

    seed_guide(&server.pool, matched, &[(-20, 40)]).await;

    let resp = client.get("/api/v1/admin/livetv/guide").await;
    resp.assert_ok();
    let all: Value = resp.json();
    let unfiltered = all["channels"].as_array().unwrap().len();
    assert!(
        unfiltered > 1,
        "the plain guide keeps every channel, got {unfiltered}"
    );

    let resp = client
        .get("/api/v1/admin/livetv/guide?search=Programme")
        .await;
    resp.assert_ok();
    let body: Value = resp.json();
    let channels = body["channels"].as_array().unwrap();
    assert_eq!(channels.len(), 1, "only the channel with a match");
    assert_eq!(channels[0]["channel"]["id"], matched.to_string());

    // The scan is still the fallback: the stub backend answers every index query with
    // nothing, and a search has to keep working on it.
    let entries = channels[0]["entries"].as_array().unwrap();
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0]["title"], "Programme 0");
    // Hydrated, not projected.
    assert_eq!(entries[0]["episode_number"], "S01E00");
    assert_eq!(entries[0]["duration_seconds"], 3600);
    assert_eq!(entries[0]["is_now"], true);
    assert_eq!(entries[0]["repeat_count"], 0);

    let resp = client
        .get("/api/v1/admin/livetv/guide?search=nothingmatchesthis")
        .await;
    resp.assert_ok();
    let body: Value = resp.json();
    assert!(body["channels"].as_array().unwrap().is_empty());
}

#[tokio::test]
async fn guide_rejects_a_bad_window() {
    let server = TestServer::start_api_only().await;
    let client = server.admin_client();

    let resp = client
        .get("/api/v1/admin/livetv/guide?from=not-a-time")
        .await;
    resp.assert_status(StatusCode::BAD_REQUEST);

    let resp = client
        .get("/api/v1/admin/livetv/guide?from=2026-08-29T00:00:00Z&to=2026-08-28T00:00:00Z")
        .await;
    resp.assert_status(StatusCode::BAD_REQUEST);

    let resp = client
        .get("/api/v1/admin/livetv/guide?channel_ids=nope")
        .await;
    resp.assert_status(StatusCode::BAD_REQUEST);
}

// =============================================================================
// Auth
// =============================================================================

#[tokio::test]
async fn reads_need_contributor_and_writes_need_admin() {
    let server = TestServer::start_api_only().await;
    let tuner_id = make_tuner(&server.pool, "AUTH0001").await;
    let ids = ingest_lineup(&server.pool, tuner_id).await;
    let id = ids[0];

    let user = create_user_client(&server, "livetv-user@example.com", "viewer").await;
    let contributor =
        create_user_client(&server, "livetv-contrib@example.com", "contributor").await;

    for path in [
        "/api/v1/admin/livetv/tuners",
        "/api/v1/admin/livetv/channels",
        "/api/v1/admin/livetv/guide",
        "/api/v1/admin/livetv/guide/now",
    ] {
        user.get(path).await.assert_status(StatusCode::FORBIDDEN);
        contributor.get(path).await.assert_ok();
    }

    // Valid bodies: the guard runs inside the handler, so a bad body would 422 first.
    contributor
        .patch_json(
            &format!("/api/v1/admin/livetv/channels/{id}"),
            &json!({ "favourite": true }),
        )
        .await
        .assert_status(StatusCode::FORBIDDEN);

    contributor
        .post_json(
            "/api/v1/admin/livetv/channels/bulk",
            &json!({ "ids": [id.to_string()], "action": "enable" }),
        )
        .await
        .assert_status(StatusCode::FORBIDDEN);

    contributor
        .post_json("/api/v1/admin/livetv/tuners", &json!({ "discover": true }))
        .await
        .assert_status(StatusCode::FORBIDDEN);

    contributor
        .post(&format!("/api/v1/admin/livetv/tuners/{tuner_id}/scan"))
        .await
        .assert_status(StatusCode::FORBIDDEN);

    contributor
        .delete(&format!("/api/v1/admin/livetv/tuners/{tuner_id}"))
        .await
        .assert_status(StatusCode::FORBIDDEN);
}

// =============================================================================
// Real device. Opt-in: LIVETV_DEVICE_URL=http://192.0.2.10 cargo test -- --ignored
// =============================================================================

#[tokio::test]
#[ignore]
async fn end_to_end_against_a_real_hdhomerun() {
    let Ok(base_url) = std::env::var("LIVETV_DEVICE_URL") else {
        panic!("set LIVETV_DEVICE_URL to run this test");
    };
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
    assert_eq!(tuner["tuner_count"], 4);

    let discover = hdhomerun::discover(&base_url).await.unwrap();
    let lineup = hdhomerun::lineup(&base_url).await.unwrap();
    let feed = hdhomerun::guide(discover.device_auth.as_deref().unwrap())
        .await
        .unwrap();

    for e in &lineup {
        let class = classify::classify(e, false);
        livetv_channels::upsert_scan(
            &server.pool,
            livetv_channels::UpsertScanChannel {
                tuner_id,
                lcn: e.lcn(),
                scan_name: e.guide_name.clone(),
                frequency: e.frequency,
                program_number: i32::try_from(e.program_number).unwrap(),
                onid: e.original_network_id.and_then(|v| i32::try_from(v).ok()),
                tsid: e.transport_stream_id.and_then(|v| i32::try_from(v).ok()),
                modulation: e.modulation.clone(),
                video_codec: e.video_codec.clone(),
                audio_codec: e.audio_codec.clone(),
                hd: e.is_hd(),
                signal_strength: e.signal_strength,
                signal_quality: e.signal_quality,
                initial_kind: class.kind.as_str().to_string(),
                initial_enabled: class.enabled_on_first_scan,
            },
        )
        .await
        .unwrap();
    }

    let stats = guide_sync::ingest(&server.pool, tuner_id, &feed)
        .await
        .unwrap();
    let (total, enabled) = livetv_channels::count_by_tuner(&server.pool, tuner_id)
        .await
        .unwrap();

    println!(
        "device={} lineup={total} enabled={enabled} guide_channels={} programmes={} \
         unmatched_lcns={} safe_keys={} title_only={}",
        discover.device_id,
        stats.channels,
        stats.programmes,
        stats.unmatched_lcns,
        stats.safe_episode_keys,
        stats.title_only_keys
    );

    assert_eq!(stats.unmatched_lcns, 0);
    assert!(total > 100);
    assert!(enabled > 50);
    assert!(stats.safe_episode_keys as f64 / stats.programmes as f64 > 0.95);

    let resp = client
        .get("/api/v1/admin/livetv/channels?limit=500&with_now=true")
        .await;
    resp.assert_ok();
    let body: Value = resp.json();
    assert_eq!(body["total"], total);
    assert!(
        body["items"]
            .as_array()
            .unwrap()
            .iter()
            .any(|c| !c["now"].is_null()),
        "at least one channel should be showing something right now"
    );
}
