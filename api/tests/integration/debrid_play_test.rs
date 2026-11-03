use crate::common::debrid_rig::*;
use crate::common::debrid_upstream::{Upstream, UpstreamPlan, WholeBody};
use crate::common::*;
use api::jobs::worker::JobHandler;
use reqwest::StatusCode;
use serde_json::{Value, json};
use std::path::PathBuf;
use std::time::{Duration, Instant};
use tokio_util::sync::CancellationToken;
use uuid::Uuid;
use wiremock::matchers::{method, path_regex};
use wiremock::{Mock, MockServer, ResponseTemplate};

const CUE_FIXTURE: &[u8] = include_bytes!("../fixtures/cue-trim/fat-cues.mkv");

/// The incident: the player asked for `bytes=0-` a moment after the download began.
/// The proxy's first byte is a far-away CDN round trip; the bytes were already here.
#[tokio::test]
async fn an_open_ended_range_starts_from_the_growing_download() {
    let data = noise(4 * MB);
    let mut plan = UpstreamPlan::new(data.clone());
    plan.range_delay = Duration::from_secs(3);
    plan.whole = WholeBody::Pause {
        first: MB,
        pause: Duration::from_millis(800),
    };
    let rig = Rig::start(plan, "film.mp4").await;
    let session_id = rig.resolve("growinghash").await;
    rig.wait_for_local("growinghash", MB as u64).await;

    let played = play(&rig.server, session_id, "bytes=0-").await;

    assert_eq!(played.status, StatusCode::PARTIAL_CONTENT);
    assert_eq!(
        played.content_range.as_deref(),
        Some(format!("bytes 0-{}/{}", data.len() - 1, data.len()).as_str())
    );
    assert_eq!(played.content_length, Some(data.len() as u64));
    assert!(
        played.first_byte_after < Duration::from_millis(1500),
        "first byte took {:?}: it waited on the upstream instead of the local file",
        played.first_byte_after
    );
    assert!(played.body == data, "the followed file came out different");

    let status: Value = session_status(&rig.server, session_id).await.json();
    let feed = &status["feed"];
    assert_eq!(feed["requests_open"], 0, "{status}");
    assert_eq!(feed["bytes_served"], data.len(), "{status}");
    assert_eq!(feed["source"], "local", "{status}");
    assert!(feed["idle_ms"].is_u64(), "{status}");
    assert_eq!(feed["total_bytes"], data.len(), "{status}");
    assert_eq!(feed["downloaded_bytes"], data.len(), "{status}");
    assert_eq!(
        feed["download_bps"], 0,
        "a finished download reports no rate: {status}"
    );
}

#[tokio::test]
async fn the_feed_reports_a_response_while_it_is_still_open() {
    let data = noise(2 * MB);
    let mut plan = UpstreamPlan::new(data.clone());
    plan.whole = WholeBody::Pause {
        first: MB,
        pause: Duration::from_millis(1500),
    };
    let rig = Rig::start(plan, "film.mp4").await;
    let session_id = rig.resolve("openhash").await;

    let before: Value = session_status(&rig.server, session_id).await.json();
    assert_eq!(before["feed"]["requests_open"], 0, "{before}");
    assert!(
        before["feed"].get("idle_ms").is_none_or(Value::is_null),
        "{before}"
    );

    rig.wait_for_local("openhash", MB as u64).await;
    let resp = reqwest::Client::new()
        .get(format!(
            "{}/api/v1/admin/stream/{session_id}/play",
            rig.server.api_url
        ))
        .bearer_auth(&rig.server.admin_token)
        .header(reqwest::header::RANGE, "bytes=0-")
        .send()
        .await
        .unwrap();
    let mut stream = resp.bytes_stream();
    use futures::StreamExt;
    let mut got = stream.next().await.unwrap().unwrap().len();
    while got < MB {
        got += stream.next().await.unwrap().unwrap().len();
    }

    let during: Value = session_status(&rig.server, session_id).await.json();
    let feed = &during["feed"];
    assert_eq!(feed["requests_open"], 1, "{during}");
    assert!(
        feed["bytes_served"].as_u64().unwrap() >= MB as u64,
        "{during}"
    );
    assert_eq!(feed["source"], "local", "{during}");
    assert_eq!(feed["total_bytes"], data.len(), "{during}");
    assert!(
        feed["downloaded_bytes"].as_u64().unwrap() >= MB as u64,
        "{during}"
    );

    while let Some(chunk) = stream.next().await {
        got += chunk.unwrap().len();
    }
    assert_eq!(got, data.len());
    let after: Value = session_status(&rig.server, session_id).await.json();
    assert_eq!(after["feed"]["requests_open"], 0, "{after}");
}

/// A local read serves whatever the cache file holds, so a status that counts fewer
/// bytes downloaded than the file holds tells a client it was served bytes that are not
/// there yet.
#[tokio::test]
async fn the_reported_download_never_trails_the_cache_file() {
    let data = noise(8 * MB);
    let mut plan = UpstreamPlan::new(data);
    plan.whole = WholeBody::Trickle {
        chunk: 64 * 1024,
        every: Duration::from_millis(250),
    };
    let rig = Rig::start(plan, "film.mp4").await;
    let session_id = rig.resolve("tricklehash").await;
    let path = rig.cache_file("tricklehash");
    let on_disk = || std::fs::metadata(&path).map_or(0, |m| m.len());

    let mut seen = 0;
    for _ in 0..4 {
        let grown_past = seen;
        wait_until(
            Duration::from_secs(10),
            "the download stopped growing",
            || {
                seen = on_disk();
                seen > grown_past
            },
        )
        .await;
        let status: Value = session_status(&rig.server, session_id).await.json();
        assert!(
            status["feed"]["downloaded_bytes"].as_u64().unwrap() >= seen,
            "{seen} bytes on disk: {status}"
        );
    }
}

#[tokio::test]
async fn a_stalled_download_hands_the_rest_to_the_proxy() {
    let data = noise(3 * MB);
    let mut plan = UpstreamPlan::new(data.clone());
    plan.whole = WholeBody::Stall { first: MB };
    let rig = Rig::start(plan, "film.mp4").await;
    let session_id = rig.resolve("stallhash").await;
    rig.wait_for_local("stallhash", MB as u64).await;

    let played = play(&rig.server, session_id, "bytes=0-").await;

    assert_eq!(played.status, StatusCode::PARTIAL_CONTENT);
    assert_eq!(played.content_length, Some(data.len() as u64));
    assert!(played.body == data, "the spliced body came out different");
    let status: Value = session_status(&rig.server, session_id).await.json();
    assert_eq!(status["feed"]["source"], "proxy", "{status}");
    assert_eq!(status["feed"]["bytes_served"], data.len(), "{status}");
}

#[tokio::test]
async fn a_seek_far_past_the_download_goes_straight_to_the_proxy() {
    let data = noise(8 * MB);
    let mut plan = UpstreamPlan::new(data.clone());
    plan.whole = WholeBody::Stall { first: MB };
    let rig = Rig::start(plan, "film.mp4").await;
    let session_id = rig.resolve("seekhash").await;
    rig.wait_for_local("seekhash", MB as u64).await;

    let from = 7 * MB;
    let played = play(&rig.server, session_id, &format!("bytes={from}-")).await;

    assert_eq!(played.status, StatusCode::PARTIAL_CONTENT);
    assert_eq!(
        played.content_range.as_deref(),
        Some(format!("bytes {from}-{}/{}", data.len() - 1, data.len()).as_str())
    );
    assert!(
        played.body == data[from..],
        "the proxied tail came out different"
    );
    assert!(
        played.first_byte_after < Duration::from_secs(2),
        "a far seek waited on the download: {:?}",
        played.first_byte_after
    );
    let status: Value = session_status(&rig.server, session_id).await.json();
    assert_eq!(status["feed"]["source"], "proxy", "{status}");
}

async fn trimmed_fixture(server: &TestServer, session_id: Uuid) -> Vec<u8> {
    let resp = server
        .admin_client()
        .get(&format!(
            "/api/v1/admin/playback/sessions/{session_id}/direct"
        ))
        .await;
    resp.assert_ok();
    let trimmed = resp.bytes().to_vec();
    assert_eq!(trimmed.len(), CUE_FIXTURE.len());
    assert!(
        trimmed != CUE_FIXTURE,
        "the fixture should be worth trimming"
    );
    trimmed
}

/// Building the trim took three sequential upstream round trips before the proxy even
/// asked for the bytes; none of them matter to a range that ends before the index.
#[tokio::test]
async fn the_cue_trim_never_holds_up_bytes_before_the_index() {
    let delay = Duration::from_millis(800);
    let mut plan = UpstreamPlan::new(CUE_FIXTURE.to_vec());
    plan.range_delay = delay;
    let (server, _upstream, session_id, _dir) = proxied_mkv_session(plan).await;
    let expected = trimmed_fixture(&server, session_id).await;
    let first_change = (0..expected.len())
        .find(|&i| expected[i] != CUE_FIXTURE[i])
        .unwrap();
    assert!(first_change > 4096);

    let head = play(&server, session_id, "bytes=0-4095").await;
    assert_eq!(head.status, StatusCode::PARTIAL_CONTENT);
    assert!(head.body == CUE_FIXTURE[..4096]);
    assert!(
        head.first_byte_after < delay * 2,
        "a range before the cue index waited {:?} for the trim",
        head.first_byte_after
    );

    let whole = play(&server, session_id, "bytes=0-").await;
    assert!(
        whole.body == expected,
        "the session served an index other than the trimmed one"
    );
    let tail = play(&server, session_id, &format!("bytes={first_change}-")).await;
    assert!(
        tail.body == expected[first_change..],
        "a later read of the index disagreed with the earlier one"
    );
}

/// Whatever the trim decided for this session, a sidecar arriving later must not
/// change the bytes of an index the player may already hold.
#[tokio::test]
async fn the_index_a_session_sees_does_not_change_under_it() {
    let (server, _upstream, session_id, dir) =
        proxied_mkv_session(UpstreamPlan::new(CUE_FIXTURE.to_vec())).await;
    let first = play(&server, session_id, "bytes=0-").await.body;

    server
        .app_state
        .playback_sessions
        .get_mut(&session_id)
        .unwrap()
        .subtitle_files = vec![api::streaming::session::SubtitleFile {
        track_id: Uuid::new_v4(),
        stream_index: 2,
        codec: "subrip".into(),
        language: Some("eng".into()),
        title: None,
        file_path: dir.path().join("sub.vtt"),
        extracted: false,
        is_forced: false,
        is_hearing_impaired: false,
    }];

    let second = play(&server, session_id, "bytes=0-").await.body;
    assert!(first == second, "the session's index changed mid-playback");
}

#[tokio::test]
async fn a_refused_upstream_fails_the_session() {
    let mut plan = UpstreamPlan::new(CUE_FIXTURE.to_vec());
    plan.refuse = Some(StatusCode::FORBIDDEN);
    let (server, _upstream, session_id, _dir) = proxied_mkv_session(plan).await;

    let played = play(&server, session_id, "bytes=0-").await;
    assert_eq!(played.status, StatusCode::BAD_GATEWAY);

    let status: Value = session_status(&server, session_id).await.json();
    assert_eq!(status["state"], "failed", "{status}");
    assert_eq!(status["error"], "upstream_refused", "{status}");
}

#[tokio::test]
async fn deleting_a_stream_stops_its_download() {
    let mut plan = UpstreamPlan::new(noise(4 * MB));
    plan.whole = WholeBody::Trickle {
        chunk: 64 * 1024,
        every: Duration::from_millis(30),
    };
    let rig = Rig::start(plan, "film.mp4").await;
    let session_id = rig.resolve("deletehash").await;
    rig.wait_for_local("deletehash", 256 * 1024).await;

    rig.server
        .admin_client()
        .delete(&format!("/api/v1/admin/stream/{session_id}"))
        .await
        .assert_status(StatusCode::NO_CONTENT);

    session_status(&rig.server, session_id)
        .await
        .assert_status(StatusCode::NOT_FOUND);
    assert!(
        rig.server
            .app_state
            .debrid_session_index
            .get("deletehash")
            .is_none()
    );
    let path = rig.cache_file("deletehash");
    tokio::time::sleep(Duration::from_millis(300)).await;
    let settled = std::fs::metadata(&path).unwrap().len();
    tokio::time::sleep(Duration::from_millis(600)).await;
    assert_eq!(
        std::fs::metadata(&path).unwrap().len(),
        settled,
        "the download kept going after its session was deleted"
    );
}

#[tokio::test]
async fn processing_a_deleted_session_is_a_no_op() {
    let server = TestServer::start_api_only().await;
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("film.mkv");
    std::fs::write(&file, noise(4096)).unwrap();
    let handler = api::jobs::handlers::DebridProcessHandler::new(
        server.pool.clone(),
        server.nats_publisher.clone(),
        server.runtime_config.clone(),
        server.storage_root(),
    );

    let result = handler
        .execute(
            Uuid::now_v7(),
            json!({ "session_id": Uuid::now_v7(), "cache_path": file.to_string_lossy() }),
            CancellationToken::new(),
        )
        .await;

    assert!(result.is_ok(), "{result:?}");
}

#[tokio::test]
async fn thumbnails_for_a_session_that_is_gone_say_so() {
    let server = TestServer::start_api_only().await;
    let missing = Uuid::now_v7();
    server
        .admin_client()
        .get(&format!(
            "/api/v1/admin/playback/sessions/{missing}/thumbnails.vtt"
        ))
        .await
        .assert_status(StatusCode::GONE);

    let dir = tempfile::tempdir().unwrap();
    let live = Uuid::now_v7();
    let mut session = api::streaming::session::PlaybackSession::test_direct_play(
        live,
        Uuid::now_v7(),
        dir.path().join("film.mkv"),
    );
    session.thumbnail_dir = Some(dir.path().join("trickplay"));
    server.app_state.playback_sessions.insert(live, session);
    server
        .admin_client()
        .get(&format!(
            "/api/v1/admin/playback/sessions/{live}/thumbnails.vtt"
        ))
        .await
        .assert_status(StatusCode::NOT_FOUND);
}

/// Kobload only knows a pack's hash-level title, so a play it cannot name logs under the
/// wrong episode.
#[tokio::test]
async fn test_opening_a_stream_tells_kobload_which_release_it_is() {
    let rig = Rig::start(UpstreamPlan::new(noise(64 * 1024)), "Feed.Movie.2025.mkv").await;
    rig.resolve(&"a".repeat(40)).await;
    let requests = rig.kobload_requests("/stream/resolve").await;
    assert!(!requests.is_empty());
    for req in requests {
        assert_eq!(req["release_title"], "Feed.Movie.2025", "{req}");
    }
}
