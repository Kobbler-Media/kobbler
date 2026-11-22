//! What a player read past the download makes of the engine's refusals: damage fails the
//! session and is reported once, a busy or unreachable engine is never blamed on the
//! release, and a plain refusal is not reported on every read.

use crate::common::debrid_rig::*;
use crate::common::debrid_upstream::{Hole, HoleAnswer, UpstreamPlan, WholeBody};
use crate::common::usenet_faults::*;
use crate::common::*;
use reqwest::StatusCode;
use serde_json::Value;
use std::time::Duration;
use uuid::Uuid;

const PAST_THE_DOWNLOAD: &str = "bytes=7000000-7000999";

/// The cache-ahead holds at its first chunk, so a read at 7 MB is proxied to the engine,
/// which cannot serve anything from 5 MB on.
fn stalled_with_hole(answer: HoleAnswer) -> UpstreamPlan {
    let mut plan = UpstreamPlan::new(noise(8 * MB));
    plan.whole = WholeBody::Stall { first: 64 * 1024 };
    plan.hole = Some(Hole { at: 5 * MB, answer });
    plan
}

async fn reported(rig: &Rig) -> Vec<Value> {
    rig.kobload_requests("/stream/outcome")
        .await
        .into_iter()
        .filter(|r| r["ok"] == false && r["reason"] != "probe_failed")
        .collect()
}

async fn read_three_times(rig: &Rig, session: Uuid) -> Vec<StatusCode> {
    let mut statuses = Vec::new();
    for _ in 0..3 {
        statuses.push(play(&rig.server, session, PAST_THE_DOWNLOAD).await.status);
    }
    tokio::time::sleep(Duration::from_millis(500)).await;
    statuses
}

#[tokio::test]
async fn damage_met_by_a_player_read_fails_the_session_and_is_reported_once() {
    for code in ["missing_articles", "corrupt_articles"] {
        let (rig, movie) = usenet_rig(stalled_with_hole(HoleAnswer::Error(
            StatusCode::BAD_GATEWAY,
            code,
        )))
        .await;
        let session = tap(&rig, movie).await;
        rig.wait_for_local(USENET_KEY, 64 * 1024).await;

        let statuses = read_three_times(&rig, session).await;

        assert!(
            statuses.iter().all(|s| !s.is_success()),
            "{code}: {statuses:?}"
        );
        assert_eq!(session_error(&rig, session), Some("not_cached"), "{code}");
        let reports = reported(&rig).await;
        assert_eq!(reports.len(), 1, "{code}: {reports:?}");
        assert_eq!(reports[0]["reason"], code);
        assert_eq!(reports[0]["release_key"], USENET_KEY);
    }
}

/// The cache-ahead found the damage first; the player meeting it again is the same
/// session's verdict.
#[tokio::test]
async fn damage_the_cache_ahead_reported_is_not_reported_again_by_the_player() {
    let mut plan = UpstreamPlan::new(noise(8 * MB));
    plan.hole = Some(Hole {
        at: MB,
        answer: HoleAnswer::Error(StatusCode::BAD_GATEWAY, "missing_articles"),
    });
    let (rig, movie) = usenet_rig(plan).await;
    let session = tap(&rig, movie).await;
    wait_until_async(
        Duration::from_secs(10),
        "the cache-ahead never reported",
        || async { !reported(&rig).await.is_empty() },
    )
    .await;

    read_three_times(&rig, session).await;

    assert_eq!(reported(&rig).await.len(), 1);
}

#[tokio::test]
async fn a_busy_or_unreachable_engine_is_never_reported_against_the_release() {
    enum Fault {
        Code(StatusCode, &'static str),
        Unreachable,
    }
    for fault in [
        Fault::Code(StatusCode::GATEWAY_TIMEOUT, "timeout"),
        Fault::Code(StatusCode::SERVICE_UNAVAILABLE, "providers_unavailable"),
        Fault::Unreachable,
    ] {
        let (answer, what) = match fault {
            Fault::Code(status, code) => (HoleAnswer::Error(status, code), code),
            Fault::Unreachable => (HoleAnswer::Abort, "unreachable"),
        };
        let (rig, movie) = usenet_rig(stalled_with_hole(answer)).await;
        let session = tap(&rig, movie).await;
        rig.wait_for_local(USENET_KEY, 64 * 1024).await;
        if matches!(fault, Fault::Unreachable) {
            rig.upstream.go_down(Duration::from_secs(30));
        }

        let statuses = read_three_times(&rig, session).await;

        assert!(
            statuses.iter().all(|s| !s.is_success()),
            "{what}: {statuses:?}"
        );
        let reports = reported(&rig).await;
        assert!(reports.is_empty(), "{what}: {reports:?}");
        assert_eq!(session_error(&rig, session), None, "{what}");
    }
}

#[tokio::test]
async fn a_plain_refusal_is_reported_once_and_a_plain_server_error_never() {
    for (status, want) in [
        (StatusCode::NOT_FOUND, 1),
        (StatusCode::INTERNAL_SERVER_ERROR, 0),
    ] {
        let mut plan = UpstreamPlan::new(noise(8 * MB));
        plan.refuse = Some(status);
        let (rig, movie) = usenet_rig(plan).await;
        let session = tap(&rig, movie).await;

        read_three_times(&rig, session).await;

        let reports = reported(&rig).await;
        assert_eq!(reports.len(), want, "{status}: {reports:?}");
    }
}
