use crate::common::debrid_rig::*;
use crate::common::debrid_upstream::UpstreamPlan;
use crate::common::*;
use reqwest::StatusCode;
use serde_json::Value;
use std::time::{Duration, Instant};
use uuid::Uuid;

/// The engine ends a response it cannot feed within 7 s and kobbler's upstream client
/// gives up on a silent one after 10 s; a player buffers a few seconds ahead, so a pause
/// longer than this is one the viewer sees.
pub const PLAYER_STALL_TOLERANCE: Duration = Duration::from_secs(5);

pub async fn usenet_rig(plan: UpstreamPlan) -> (Rig, Uuid) {
    let rig = Rig::start_as(Source::Usenet, plan, "film.mkv").await;
    let movie = rig.movie().await;
    (rig, movie)
}

pub async fn tap(rig: &Rig, movie: Uuid) -> Uuid {
    session_id_of(
        &rig.resolve_with(&rig.resolve_body_from(Source::Usenet, "movie", movie))
            .await,
    )
}

/// The noise these releases carry is no video, so a failed probe is expected and beside
/// the point.
pub fn session_error(rig: &Rig, session: Uuid) -> Option<&'static str> {
    rig.server
        .app_state
        .playback_sessions
        .get(&session)
        .and_then(|s| s.error.as_ref().map(|e| e.code))
        .filter(|code| *code != "probe_failed")
}

pub async fn failures(rig: &Rig, reason: &str) -> Vec<Value> {
    rig.kobload_requests("/stream/outcome")
        .await
        .into_iter()
        .filter(|r| r["ok"] == false && r["reason"] == reason)
        .collect()
}

/// A read the download has not reached needs a fresh URL from kobload. Depending on how
/// far the download is guessed to reach, the refusal arrives before the response or cuts
/// it short; either way only the session's verdict matters.
pub async fn read_past_the_download(rig: &Rig, session: Uuid) {
    let resp = reqwest::Client::new()
        .get(format!(
            "{}/api/v1/admin/stream/{session}/play",
            rig.server.api_url
        ))
        .bearer_auth(&rig.server.admin_token)
        .header(reqwest::header::RANGE, "bytes=3000000-3000999")
        .send()
        .await
        .expect("play request");
    let _ = resp.bytes().await;
}

pub struct Timed {
    pub status: StatusCode,
    pub body: Vec<u8>,
    pub longest_wait: Duration,
}

pub async fn play_timed(server: &TestServer, session: Uuid, range: &str) -> Timed {
    use futures::StreamExt;
    let resp = reqwest::Client::new()
        .get(format!(
            "{}/api/v1/admin/stream/{session}/play",
            server.api_url
        ))
        .bearer_auth(&server.admin_token)
        .header(reqwest::header::RANGE, range)
        .send()
        .await
        .expect("play request");
    let status = resp.status();
    let mut body = Vec::new();
    let mut longest_wait = Duration::ZERO;
    let mut last = Instant::now();
    let mut stream = resp.bytes_stream();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.expect("the player read failed");
        longest_wait = longest_wait.max(last.elapsed());
        last = Instant::now();
        body.extend_from_slice(&chunk);
    }
    Timed {
        status,
        body,
        longest_wait,
    }
}
