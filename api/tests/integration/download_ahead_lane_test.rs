//! A download ahead fetches through a Warm URL, below library downloads. A viewer who joins
//! it must not wait at that rank.

use crate::common::debrid_rig::*;
use crate::common::debrid_upstream::{UpstreamPlan, WholeBody};
use crate::common::*;
use api::jobs::handlers::DebridDownloadAheadHandler;
use api::jobs::worker::JobHandler;
use serde_json::{Value, json};
use std::time::Duration;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;
use wiremock::matchers::{body_partial_json, method, path_regex};
use wiremock::{Mock, ResponseTemplate};

const KEY: &str = "lanehash";

fn urls(rig: &Rig, lane: &str, size: usize) -> Value {
    json!({
        "stream_url": rig.upstream.url(&format!("{lane}/play/ep.mkv")),
        "cache_url": rig.upstream.url(&format!("{lane}/cache/ep.mkv")),
        "filename": "ep.mkv",
        "filesize": size,
        "cached": true,
    })
}

async fn answer(rig: &Rig, priority: &str, body: Value, times: Option<u64>, rank: u8) {
    let mock = Mock::given(method("POST"))
        .and(path_regex("/api/v1/stream/resolve$"))
        .and(body_partial_json(json!({ "priority": priority })))
        .respond_with(ResponseTemplate::new(200).set_body_json(body))
        .with_priority(rank);
    match times {
        Some(n) => mock.up_to_n_times(n).mount(&rig.kobload).await,
        None => mock.mount(&rig.kobload).await,
    }
}

async fn trickling_rig(data: &[u8]) -> Rig {
    let mut plan = UpstreamPlan::new(data.to_vec());
    plan.whole = WholeBody::Trickle {
        chunk: 64 * 1024,
        every: Duration::from_millis(50),
    };
    let rig = Rig::start(plan, "ep.mkv").await;
    rig.mount_search(json!([rig.option(KEY)])).await;
    rig
}

struct Ahead {
    episode: Uuid,
    job: tokio::task::JoinHandle<api::jobs::JobResult<Value>>,
}

async fn start_ahead(rig: &Rig) -> Ahead {
    let episodes = season_of(&rig.server, 2).await;
    let user: Uuid = sqlx::query_scalar("SELECT id FROM users WHERE role = 'admin' LIMIT 1")
        .fetch_one(&rig.server.pool)
        .await
        .unwrap();
    let handler = DebridDownloadAheadHandler::new(rig.server.app_state.clone());
    let payload = json!({ "user_id": user, "episode_id": episodes[1] });
    let job = tokio::spawn(async move {
        handler
            .execute(Uuid::now_v7(), payload, CancellationToken::new())
            .await
    });
    rig.wait_for_local(KEY, 256 * 1024).await;
    Ahead {
        episode: episodes[1],
        job,
    }
}

async fn join(rig: &Rig, episode: Uuid) -> Uuid {
    session_id_of(
        &rig.resolve_with(&json!({
            "debrid_provider_id": PROVIDER_ID,
            "release_key": KEY,
            "media_type": "episode",
            "media_id": episode,
            "release_title": "Release",
            "debrid_provider": "realdebrid",
            "capabilities": direct_play_caps()
        }))
        .await,
    )
}

fn resumed_through(rig: &Rig, path: &str) -> Vec<u64> {
    rig.upstream
        .requests()
        .into_iter()
        .filter(|r| r.path == path)
        .filter_map(|r| {
            r.range?
                .strip_prefix("bytes=")?
                .strip_suffix('-')?
                .parse()
                .ok()
        })
        .collect()
}

async fn finish(rig: &Rig, ahead: Ahead, data: &[u8]) {
    let done = tokio::time::timeout(Duration::from_secs(30), ahead.job)
        .await
        .expect("download ahead never finished")
        .unwrap()
        .unwrap();
    assert!(done.get("handed_over").is_none(), "{done}");
    assert!(
        std::fs::read(rig.cache_file(KEY)).unwrap() == data,
        "two writers or a wrong offset damaged the file"
    );
    assert_eq!(rig.upstream.whole_gets(), 1, "the file was restarted");
}

#[tokio::test]
async fn a_viewer_joining_moves_the_download_to_a_cache_url_at_the_same_offset() {
    let data = noise(8 * MB);
    let rig = trickling_rig(&data).await;
    answer(&rig, "warm", urls(&rig, "warm", data.len()), None, 1).await;
    answer(&rig, "user", urls(&rig, "user", data.len()), None, 1).await;
    let ahead = start_ahead(&rig).await;

    join(&rig, ahead.episode).await;

    let path = "/user/cache/ep.mkv";
    wait_until(
        Duration::from_secs(5),
        "the download stayed on its Warm URL",
        || !resumed_through(&rig, path).is_empty(),
    )
    .await;
    assert!(resumed_through(&rig, path)[0] >= 256 * 1024);
    finish(&rig, ahead, &data).await;
}

#[tokio::test]
async fn a_failed_upgrade_keeps_downloading_and_is_retried() {
    let data = noise(8 * MB);
    let rig = trickling_rig(&data).await;
    answer(&rig, "warm", urls(&rig, "warm", data.len()), None, 1).await;
    let mut no_cache_url = urls(&rig, "user", data.len());
    no_cache_url.as_object_mut().unwrap().remove("cache_url");
    answer(&rig, "user", no_cache_url, Some(1), 1).await;
    Mock::given(method("POST"))
        .and(path_regex("/api/v1/stream/resolve$"))
        .and(body_partial_json(json!({ "priority": "user" })))
        .respond_with(ResponseTemplate::new(503).set_body_string("kobload is busy"))
        .up_to_n_times(1)
        .with_priority(2)
        .mount(&rig.kobload)
        .await;
    answer(&rig, "user", urls(&rig, "user", data.len()), None, 3).await;
    let ahead = start_ahead(&rig).await;

    join(&rig, ahead.episode).await;

    let path = "/user/cache/ep.mkv";
    wait_until(
        Duration::from_secs(5),
        "the failed upgrade was never retried",
        || !resumed_through(&rig, path).is_empty(),
    )
    .await;
    let asked = rig
        .kobload_requests("/stream/resolve")
        .await
        .into_iter()
        .filter(|r| r["priority"] == "user")
        .count();
    assert!(asked >= 3, "only {asked} viewer resolves");
    finish(&rig, ahead, &data).await;
}

/// A refused URL is re-resolved on the lane of the download it serves.
#[tokio::test]
async fn a_forced_re_resolve_uses_the_lane_of_its_download() {
    for (lane, ahead) in [("warm", true), ("user", false)] {
        let data = noise(MB);
        let rig = Rig::start(UpstreamPlan::new(data.clone()), "ep.mkv").await;
        rig.mount_search(json!([rig.option(KEY)])).await;
        let refused = json!({
            "stream_url": rig.upstream.url(&format!("{lane}/play/ep.mkv")),
            "cache_url": rig.upstream.url(&format!("{lane}/refused/ep.mkv")),
            "filename": "ep.mkv",
            "filesize": data.len(),
            "cached": true,
        });
        answer(&rig, lane, refused, Some(1), 1).await;
        answer(&rig, lane, urls(&rig, lane, data.len()), None, 2).await;
        rig.upstream.revoke(&format!("/{lane}/refused"));

        if ahead {
            let episodes = season_of(&rig.server, 2).await;
            let user: Uuid =
                sqlx::query_scalar("SELECT id FROM users WHERE role = 'admin' LIMIT 1")
                    .fetch_one(&rig.server.pool)
                    .await
                    .unwrap();
            DebridDownloadAheadHandler::new(rig.server.app_state.clone())
                .execute(
                    Uuid::now_v7(),
                    json!({ "user_id": user, "episode_id": episodes[1] }),
                    CancellationToken::new(),
                )
                .await
                .unwrap();
        } else {
            rig.resolve(KEY).await;
        }
        rig.wait_for_local(KEY, data.len() as u64).await;

        let asked: Vec<Value> = rig
            .kobload_requests("/stream/resolve")
            .await
            .into_iter()
            .map(|r| r["priority"].clone())
            .collect();
        assert_eq!(asked, [json!(lane), json!(lane)], "{lane}");
    }
}
