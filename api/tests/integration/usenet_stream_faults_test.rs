//! How a usenet session rides out what the engine does to it: releases kobload finds
//! dead, articles missing mid-file, stream tokens that lapse, and the engine restarting
//! under a playing stream.

use crate::common::debrid_rig::*;
use crate::common::debrid_upstream::{Hole, HoleAnswer, UpstreamPlan, WholeBody};
use crate::common::*;
use reqwest::StatusCode;
use serde_json::{Value, json};
use std::time::{Duration, Instant};
use uuid::Uuid;
use wiremock::matchers::{body_partial_json, method, path_regex};
use wiremock::{Mock, ResponseTemplate};

/// The engine ends a response it cannot feed within 7 s and kobbler's upstream client
/// gives up on a silent one after 10 s; a player buffers a few seconds ahead, so a pause
/// longer than this is one the viewer sees.
const PLAYER_STALL_TOLERANCE: Duration = Duration::from_secs(5);

async fn usenet_rig(plan: UpstreamPlan) -> (Rig, Uuid) {
    let rig = Rig::start_as(Source::Usenet, plan, "film.mkv").await;
    let movie = rig.movie().await;
    (rig, movie)
}

async fn tap(rig: &Rig, movie: Uuid) -> Uuid {
    session_id_of(
        &rig.resolve_with(&rig.resolve_body_from(Source::Usenet, "movie", movie))
            .await,
    )
}

/// The noise these releases carry is no video, so a failed probe is expected and beside
/// the point.
fn session_error(rig: &Rig, session: Uuid) -> Option<&'static str> {
    rig.server
        .app_state
        .playback_sessions
        .get(&session)
        .and_then(|s| s.error.as_ref().map(|e| e.code))
        .filter(|code| *code != "probe_failed")
}

async fn failures(rig: &Rig, reason: &str) -> Vec<Value> {
    rig.kobload_requests("/stream/outcome")
        .await
        .into_iter()
        .filter(|r| r["ok"] == false && r["reason"] == reason)
        .collect()
}

/// kobload's own words for a release it will never serve, and what kobbler makes of them.
#[tokio::test]
async fn a_dead_usenet_release_fails_its_session_instead_of_re_resolving() {
    let cases = [
        (
            "Debrid error: Usenet release is not cached: articles are missing on every provider (missing_articles)",
            Some("not_cached"),
        ),
        (
            "Debrid error: Usenet release is not cached: it is older than every provider's retention (retention)",
            Some("not_cached"),
        ),
        (
            "Debrid error: Usenet release is blocked: its archive is password-protected (password)",
            Some("blocked"),
        ),
        (
            "Debrid error: Resolved release is not a video: it holds no playable file",
            Some("not_video"),
        ),
        (
            "Debrid error: No usenet provider can serve this right now (providers_unavailable)",
            None,
        ),
    ];
    let mut plan = UpstreamPlan::new(noise(4 * MB));
    plan.whole = WholeBody::Stall { first: 64 * 1024 };
    let rig = Rig::start(plan, "film.mkv").await;
    for (n, (kobload_says, want)) in cases.into_iter().enumerate() {
        let key = format!("nzb-dead{n}");
        Mock::given(method("POST"))
            .and(path_regex("/api/v1/stream/resolve$"))
            .and(body_partial_json(json!({ "release_key": key })))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "stream_url": rig.upstream.url("film.mkv"),
                "filename": "film.mkv",
                "filesize": 4 * MB,
                "cached": true,
                "expires_at": "2020-01-01T00:00:00Z",
                "file_id": 0,
            })))
            .up_to_n_times(1)
            .with_priority(1)
            .mount(&rig.kobload)
            .await;
        Mock::given(method("POST"))
            .and(path_regex("/api/v1/stream/resolve$"))
            .and(body_partial_json(json!({ "release_key": key })))
            .respond_with(ResponseTemplate::new(500).set_body_string(kobload_says))
            .with_priority(2)
            .mount(&rig.kobload)
            .await;
        let movie = rig.movie().await;
        let session = session_id_of(
            &rig.resolve_with(&rig.resolve_body(&key, movie, direct_play_caps()))
                .await,
        );

        read_past_the_download(&rig, session).await;
        assert_eq!(session_error(&rig, session), want, "{kobload_says}");
    }
}

/// A read the download has not reached needs a fresh URL from kobload. Depending on how
/// far the download is guessed to reach, the refusal arrives before the response or cuts
/// it short; either way only the session's verdict matters.
async fn read_past_the_download(rig: &Rig, session: Uuid) {
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

/// Every attempt stops where the articles are missing, whether the engine says why or
/// just ends the response: once the resumes are spent kobload hears about it, once.
#[tokio::test]
async fn missing_articles_mid_file_are_reported_to_kobload_once() {
    let cases = [
        (
            HoleAnswer::Error(StatusCode::BAD_GATEWAY, "missing_articles"),
            "missing_articles",
            Duration::from_secs(5),
        ),
        (
            HoleAnswer::Error(StatusCode::BAD_GATEWAY, "corrupt_articles"),
            "corrupt_articles",
            Duration::from_secs(5),
        ),
        (
            HoleAnswer::Abort,
            "upstream_aborted",
            Duration::from_secs(30),
        ),
    ];
    for (answer, reason, within) in cases {
        let mut plan = UpstreamPlan::new(noise(2 * MB));
        plan.hole = Some(Hole { at: MB, answer });
        let (rig, movie) = usenet_rig(plan).await;
        let session = tap(&rig, movie).await;
        rig.wait_for_local(USENET_KEY, MB as u64).await;

        let started = Instant::now();
        let reports = loop {
            let reports = failures(&rig, reason).await;
            if !reports.is_empty() || started.elapsed() > within {
                break reports;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        };
        assert_eq!(reports.len(), 1, "{reason}: {reports:?}");
        assert_eq!(reports[0]["release_key"], USENET_KEY, "{reason}");

        rig.server
            .app_state
            .playback_sessions
            .get(&session)
            .unwrap()
            .download_cancel
            .as_ref()
            .unwrap()
            .cancel();
        play(&rig.server, session, "bytes=0-1023").await;
        tokio::time::sleep(Duration::from_secs(2)).await;
        if reason != "upstream_aborted" {
            assert_eq!(
                failures(&rig, reason).await.len(),
                1,
                "a restarted cache-ahead reported the same session again"
            );
        }
    }
}

/// An engine that times out or has no provider free says nothing about the release: the
/// cache-ahead keeps resuming and kobload is never told the release is dead.
#[tokio::test]
async fn transient_engine_errors_are_resumed_and_never_reported() {
    for (status, code) in [
        (StatusCode::GATEWAY_TIMEOUT, "timeout"),
        (StatusCode::SERVICE_UNAVAILABLE, "providers_unavailable"),
    ] {
        let mut plan = UpstreamPlan::new(noise(2 * MB));
        plan.hole = Some(Hole {
            at: MB,
            answer: HoleAnswer::Error(status, code),
        });
        let (rig, movie) = usenet_rig(plan).await;
        let session = tap(&rig, movie).await;
        rig.wait_for_local(USENET_KEY, MB as u64).await;
        tokio::time::sleep(Duration::from_secs(3)).await;
        let resumes = rig
            .upstream
            .ranges_seen()
            .into_iter()
            .filter(|r| r == &format!("bytes={MB}-"))
            .count();
        assert!(resumes >= 2, "{code}: resumed {resumes} times");
        let reported: Vec<Value> = rig
            .kobload_requests("/stream/outcome")
            .await
            .into_iter()
            .filter(|r| r["ok"] == false && r["reason"] != "probe_failed")
            .collect();
        assert!(reported.is_empty(), "{code}: {reported:?}");
        assert_eq!(session_error(&rig, session), None, "{code}");
    }
}

/// The cache-ahead's URL carries a signed token that expires: when the engine refuses it,
/// or kobload said it has lapsed, a fresh one is resolved and the download carries on.
#[tokio::test]
async fn the_cache_ahead_carries_on_with_a_fresh_url_when_its_token_lapses() {
    struct Case {
        what: &'static str,
        expires_in: time::Duration,
        revoke: bool,
    }
    for case in [
        Case {
            what: "refused with 403",
            expires_in: time::Duration::hours(6),
            revoke: true,
        },
        Case {
            what: "expiry known from expires_at",
            expires_in: time::Duration::seconds(60),
            revoke: false,
        },
    ] {
        let data = noise(3 * MB);
        let mut plan = UpstreamPlan::new(data.clone());
        plan.whole = WholeBody::Pause {
            first: MB,
            pause: Duration::from_secs(120),
        };
        let rig = Rig::start(plan, "film.mkv").await;
        let movie = rig.movie().await;
        let expires_at = (time::OffsetDateTime::now_utc() + case.expires_in)
            .format(&time::format_description::well_known::Rfc3339)
            .unwrap();
        for (token, times) in [("tok1", Some(1)), ("tok2", None)] {
            let mut mock = Mock::given(method("POST"))
                .and(path_regex("/api/v1/stream/resolve$"))
                .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                    "stream_url": rig.upstream.url(&format!("{token}/play/film.mkv")),
                    "cache_url": rig.upstream.url(&format!("{token}/cache/film.mkv")),
                    "filename": "film.mkv",
                    "filesize": data.len(),
                    "cached": true,
                    "expires_at": expires_at,
                    "release_key": USENET_KEY,
                    "file_id": 0,
                })))
                .with_priority(if times.is_some() { 1 } else { 2 });
            if let Some(n) = times {
                mock = mock.up_to_n_times(n);
            }
            mock.mount(&rig.kobload).await;
        }
        let session = tap(&rig, movie).await;
        let cache_path = rig
            .cache_dir
            .path()
            .join(format!("{USENET_KEY}_f0/film.mkv"));
        wait_until(Duration::from_secs(10), "the first MB never landed", || {
            std::fs::metadata(&cache_path).map(|m| m.len()).unwrap_or(0) >= MB as u64
        })
        .await;

        if case.revoke {
            rig.upstream.revoke("/tok1/");
        }
        rig.upstream.go_down(Duration::ZERO);

        wait_until(
            Duration::from_secs(15),
            &format!("{}: the cache-ahead never finished", case.what),
            || std::fs::metadata(&cache_path).map(|m| m.len()).unwrap_or(0) >= data.len() as u64,
        )
        .await;
        assert!(std::fs::read(&cache_path).unwrap() == data, "{}", case.what);
        assert_eq!(session_error(&rig, session), None, "{}", case.what);
        let resumed: Vec<_> = rig
            .upstream
            .requests()
            .into_iter()
            .filter(|r| r.range.is_some() && r.path == "/tok2/cache/film.mkv")
            .collect();
        assert!(
            !resumed.is_empty(),
            "{}: never resumed from the fresh URL",
            case.what
        );
    }
}

struct Timed {
    status: StatusCode,
    body: Vec<u8>,
    longest_wait: Duration,
}

async fn play_timed(server: &TestServer, session: Uuid, range: &str) -> Timed {
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

/// The engine restarts under a playing stream: every connection drops and the port
/// refuses for two seconds. The player's read finishes with every byte, without an error
/// and without a pause it would notice, and the cache-ahead finishes the file.
#[tokio::test]
async fn an_engine_restart_mid_stream_is_ridden_out() {
    let data = noise(6 * MB);
    let mut plan = UpstreamPlan::new(data.clone());
    plan.whole = WholeBody::Pause {
        first: 2 * MB,
        pause: Duration::from_secs(120),
    };
    let (rig, movie) = usenet_rig(plan).await;
    let session = tap(&rig, movie).await;
    rig.wait_for_local(USENET_KEY, 2 * MB as u64).await;

    rig.upstream.go_down(Duration::from_secs(2));
    let played = play_timed(&rig.server, session, "bytes=0-").await;

    assert_eq!(played.status, StatusCode::PARTIAL_CONTENT);
    assert!(played.body == data, "the player read came back different");
    assert!(
        played.longest_wait < PLAYER_STALL_TOLERANCE,
        "the player waited {:?} for bytes",
        played.longest_wait
    );
    wait_until(
        Duration::from_secs(10),
        "the cache-ahead never finished after the restart",
        || std::fs::read(rig.cache_file(USENET_KEY)).ok().as_deref() == Some(&data[..]),
    )
    .await;
    assert_eq!(session_error(&rig, session), None);
}
