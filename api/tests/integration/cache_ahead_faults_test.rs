//! The cache-ahead under outages, busy engines, reset storms and hosts that answer with the
//! wrong bytes: it carries on when it can, gives up without blaming the release when it
//! cannot, and never writes a byte twice.

use crate::common::debrid_rig::*;
use crate::common::debrid_upstream::{Hole, HoleAnswer, Upstream, UpstreamPlan, WholeBody};
use crate::common::usenet_faults::*;
use crate::common::*;
use reqwest::StatusCode;
use serde_json::Value;
use std::time::Duration;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

/// Longer than the usenet engine takes to restart, which is all the old budget allowed for.
const OUTAGE: Duration = Duration::from_secs(14);

fn held_after_first_mb(data: &[u8]) -> UpstreamPlan {
    let mut plan = UpstreamPlan::new(data.to_vec());
    plan.whole = WholeBody::Pause {
        first: MB,
        pause: Duration::from_secs(120),
    };
    plan
}

/// The whole file landed, and nothing was written after it.
async fn assert_downloaded(rig: &Rig, key: &str, data: &[u8], within: Duration, what: &str) {
    let path = rig.cache_file(key);
    wait_until(within, &format!("{what}: never finished"), || {
        std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0) >= data.len() as u64
    })
    .await;
    tokio::time::sleep(Duration::from_millis(500)).await;
    assert!(
        std::fs::read(&path).unwrap() == data,
        "{what}: the file holds other bytes"
    );
}

async fn release_failures(rig: &Rig) -> Vec<Value> {
    rig.kobload_requests("/stream/outcome")
        .await
        .into_iter()
        .filter(|r| r["ok"] == false && r["reason"] != "probe_failed")
        .collect()
}

fn download_cancel(rig: &Rig, session: Uuid) -> CancellationToken {
    rig.server
        .app_state
        .playback_sessions
        .get(&session)
        .unwrap()
        .download_cancel
        .clone()
        .unwrap()
}

fn resumes_from(upstream: &Upstream, offset: usize) -> usize {
    let asked = format!("bytes={offset}-");
    upstream
        .ranges_seen()
        .into_iter()
        .filter(|r| *r == asked)
        .count()
}

/// The idle reaper stops a cache-ahead while its resume waits on the host, and the next
/// read starts another. Whichever answer comes back first, only one of them appends.
#[tokio::test]
async fn a_reaped_cache_ahead_waiting_on_the_host_never_writes_beside_its_restart() {
    let data = noise(3 * MB);
    let mut plan = held_after_first_mb(&data);
    plan.range_delay = Duration::from_secs(1);
    let rig = Rig::start(plan, "film.mkv").await;
    for round in 0..6 {
        let key = format!("reapedhash{round}");
        let session = rig.resolve(&key).await;
        rig.wait_for_local(&key, MB as u64).await;

        rig.upstream.go_down(Duration::ZERO);
        tokio::time::sleep(Duration::from_millis(500)).await;
        download_cancel(&rig, session).cancel();
        play(&rig.server, session, "bytes=0-1023").await;

        assert_downloaded(
            &rig,
            &key,
            &data,
            Duration::from_secs(10),
            &format!("round {round}"),
        )
        .await;
    }
}

enum Outage {
    Down,
    Busy(StatusCode, &'static str),
}

/// The engine is down, or has no provider free, for longer than a restart: the cache-ahead
/// waits it out and finishes, and the release is never blamed.
#[tokio::test]
async fn an_engine_outage_longer_than_a_restart_is_ridden_out() {
    let ride_out = |outage: Outage| async move {
        let data = noise(3 * MB);
        let (rig, movie) = usenet_rig(held_after_first_mb(&data)).await;
        let session = tap(&rig, movie).await;
        rig.wait_for_local(USENET_KEY, MB as u64).await;
        let what = match outage {
            Outage::Down => {
                rig.upstream.go_down(OUTAGE);
                "connection refused".to_string()
            }
            Outage::Busy(status, code) => {
                rig.upstream.busy_for(status, code, OUTAGE);
                rig.upstream.go_down(Duration::ZERO);
                code.to_string()
            }
        };
        assert_downloaded(
            &rig,
            USENET_KEY,
            &data,
            OUTAGE + Duration::from_secs(15),
            &what,
        )
        .await;
        assert_eq!(session_error(&rig, session), None, "{what}");
        let reported = release_failures(&rig).await;
        assert!(reported.is_empty(), "{what}: {reported:?}");
    };
    futures::future::join_all([
        ride_out(Outage::Down),
        ride_out(Outage::Busy(
            StatusCode::SERVICE_UNAVAILABLE,
            "providers_unavailable",
        )),
        ride_out(Outage::Busy(StatusCode::GATEWAY_TIMEOUT, "timeout")),
    ])
    .await;
}

/// The engine is busy before the first byte: the cache-ahead waits for it instead of
/// failing the session.
#[tokio::test]
async fn an_engine_busy_at_the_first_byte_is_waited_out() {
    for (status, code) in [
        (StatusCode::SERVICE_UNAVAILABLE, "providers_unavailable"),
        (StatusCode::GATEWAY_TIMEOUT, "timeout"),
    ] {
        let data = noise(2 * MB);
        let (rig, movie) = usenet_rig(UpstreamPlan::new(data.clone())).await;
        rig.upstream.busy_for(status, code, Duration::from_secs(3));
        let session = tap(&rig, movie).await;
        assert_downloaded(&rig, USENET_KEY, &data, Duration::from_secs(15), code).await;
        assert_eq!(session_error(&rig, session), None, "{code}");
    }
}

/// A host that keeps cutting the response short says nothing about the release, as
/// Real-Debrid's CDN under load: the cache-ahead gives up without reporting it, and the
/// next player read starts it again.
#[tokio::test]
async fn a_reset_storm_is_never_reported_and_the_next_read_restarts_the_download() {
    let mut plan = UpstreamPlan::new(noise(2 * MB));
    plan.hole = Some(Hole {
        at: MB,
        answer: HoleAnswer::Abort,
    });
    let rig = Rig::start(plan, "film.mkv").await;
    let session = rig.resolve("stormhash").await;
    rig.wait_for_local("stormhash", MB as u64).await;

    wait_until(
        Duration::from_secs(25),
        "the cache-ahead never gave up",
        || download_cancel(&rig, session).is_cancelled(),
    )
    .await;
    let reported = release_failures(&rig).await;
    assert!(reported.is_empty(), "{reported:?}");
    assert_eq!(session_error(&rig, session), None);

    let before = resumes_from(&rig.upstream, MB);
    play(&rig.server, session, "bytes=0-1023").await;
    wait_until(
        Duration::from_secs(5),
        "the next read did not restart the download",
        || resumes_from(&rig.upstream, MB) > before,
    )
    .await;
}

/// A 206 that starts somewhere other than where the download asked is never appended;
/// the download asks again.
#[tokio::test]
async fn a_misplaced_range_answer_is_not_appended() {
    let data = noise(3 * MB);
    let rig = Rig::start(held_after_first_mb(&data), "film.mkv").await;
    rig.resolve("misplacedhash").await;
    rig.wait_for_local("misplacedhash", MB as u64).await;

    rig.upstream.misplace_range_at(MB as u64);
    rig.upstream.go_down(Duration::ZERO);

    assert_downloaded(
        &rig,
        "misplacedhash",
        &data,
        Duration::from_secs(10),
        "misplaced 206",
    )
    .await;
}

/// A response that ends cleanly before the file does is resumed, not taken for the whole
/// file.
#[tokio::test]
async fn a_short_clean_response_is_resumed_not_marked_complete() {
    let data = noise(3 * MB);
    let mut plan = UpstreamPlan::new(data.clone());
    plan.whole = WholeBody::Short { len: MB };
    let rig = Rig::start(plan, "film.mkv").await;
    rig.resolve("shorthash").await;

    assert_downloaded(
        &rig,
        "shorthash",
        &data,
        Duration::from_secs(10),
        "short response",
    )
    .await;
}
