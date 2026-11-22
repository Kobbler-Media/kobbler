//! A usenet session end to end: the player reads the engine's play URL, the cache-ahead
//! its cache URL, the finished file is processed, and a DLNA renderer and a cast receiver
//! read the session the way they read any other.

use crate::common::debrid_rig::*;
use crate::common::debrid_upstream::UpstreamPlan;
use crate::common::*;
use reqwest::StatusCode;
use serde_json::{Value, json};
use std::time::Duration;
use uuid::Uuid;
use wiremock::matchers::{method, path_regex};
use wiremock::{Mock, ResponseTemplate};

fn resolved(rig: &Rig) -> Value {
    json!({
        "stream_url": rig.upstream.url("play/film.mkv"),
        "cache_url": rig.upstream.url("cache/film.mkv"),
        "filename": "film.mkv",
        "filesize": ITA_ENG_FIXTURE.len(),
        "cached": true,
        "release_key": USENET_KEY,
        "file_id": 0,
    })
}

/// kobload resolving to the engine's two URLs, the first resolve taking `first_after`.
async fn engine_rig_with(first_after: Option<Duration>) -> Rig {
    let rig = Rig::start(UpstreamPlan::new(ITA_ENG_FIXTURE.to_vec()), "film.mkv").await;
    if let Some(delay) = first_after {
        Mock::given(method("POST"))
            .and(path_regex("/api/v1/stream/resolve$"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_json(resolved(&rig))
                    .set_delay(delay),
            )
            .up_to_n_times(1)
            .with_priority(1)
            .mount(&rig.kobload)
            .await;
    }
    rig.mount_resolve_with(resolved(&rig)).await;
    rig
}

async fn engine_rig() -> Rig {
    engine_rig_with(None).await
}

async fn tap(rig: &Rig, movie: Uuid) -> Value {
    rig.resolve_with(&rig.resolve_body_from(Source::Usenet, "movie", movie))
        .await
}

async fn playback_token(server: &TestServer) -> String {
    let resp = server
        .admin_client()
        .post_json("/api/v1/admin/playback/tokens", &json!({}))
        .await;
    resp.assert_ok();
    resp.json::<Value>()["token"].as_str().unwrap().to_string()
}

/// A renderer or receiver has only the URL it was handed: no cookie, no header.
async fn consumer_get(
    server: &TestServer,
    url: &str,
    token: &str,
    headers: &[(&str, &str)],
    head: bool,
) -> reqwest::Response {
    let client = reqwest::Client::new();
    let url = format!("{}{url}?token={token}", server.api_url);
    let mut req = if head {
        client.head(url)
    } else {
        client.get(url)
    };
    for (k, v) in headers {
        req = req.header(*k, *v);
    }
    req.send().await.expect("consumer request")
}

#[tokio::test]
async fn a_usenet_session_plays_caches_from_its_cache_url_and_is_processed() {
    let rig = engine_rig().await;
    let movie = rig.movie().await;
    let session = tap(&rig, movie).await;
    let id = session_id_of(&session);
    let cache_path = rig
        .cache_dir
        .path()
        .join(format!("{USENET_KEY}_f0/film.mkv"));

    let played = play(&rig.server, id, "bytes=0-").await;
    assert_eq!(played.status, StatusCode::PARTIAL_CONTENT);
    assert!(
        played.body == ITA_ENG_FIXTURE,
        "the player read came back different"
    );

    wait_until(Duration::from_secs(10), "never cached", || {
        std::fs::read(&cache_path).ok().as_deref() == Some(ITA_ENG_FIXTURE)
    })
    .await;
    let whole: Vec<String> = rig
        .upstream
        .requests()
        .into_iter()
        .filter(|r| r.range.is_none())
        .map(|r| r.path)
        .collect();
    assert_eq!(
        whole,
        ["/cache/film.mkv"],
        "the cache-ahead read the play URL"
    );

    let path = cache_path.to_string_lossy().to_string();
    wait_until_async(Duration::from_secs(10), "processing never queued", || {
        let pool = rig.server.pool.clone();
        let path = path.clone();
        async move {
            sqlx::query_scalar::<_, bool>(
                "SELECT EXISTS(SELECT 1 FROM job_status
                 WHERE job_type = 'debrid_process' AND payload->>'cache_path' = $1)",
            )
            .bind(path)
            .fetch_one(&pool)
            .await
            .unwrap()
        }
    })
    .await;
    use api::jobs::worker::JobHandler as _;
    let handler = api::jobs::handlers::DebridProcessHandler::new(
        rig.server.pool.clone(),
        rig.server.nats_publisher.clone(),
        rig.server.runtime_config.clone(),
        rig.server.app_state.gpu_lock.clone(),
        rig.server.storage_root(),
    );
    assert_eq!(handler.job_type(), "debrid_process");
    rig.server.process_pending_jobs(&handler).await;
    let analysed: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM debrid_files WHERE cache_path = $1")
            .bind(&path)
            .fetch_one(&rig.server.pool)
            .await
            .unwrap();
    assert_eq!(analysed, 1, "the finished download was not processed");
}

#[tokio::test]
async fn dlna_and_cast_consumers_read_a_usenet_session_by_range() {
    let rig = engine_rig().await;
    let movie = rig.movie().await;
    let session = tap(&rig, movie).await;
    let url = session["direct_url"]
        .as_str()
        .unwrap_or_else(|| panic!("no direct_url: {session}"));
    let token = playback_token(&rig.server).await;
    let total = ITA_ENG_FIXTURE.len();
    let (from, to) = (total / 3, total / 3 + 4095);
    let range = format!("bytes={from}-{to}");

    let cases: [(&str, Vec<(&str, &str)>); 2] = [
        (
            "dlna",
            vec![("getcontentFeatures.dlna.org", "1"), ("range", &range)],
        ),
        ("cast", vec![("range", &range)]),
    ];
    for (who, headers) in cases {
        let head = consumer_get(&rig.server, url, &token, &headers, true).await;
        assert_eq!(head.status(), StatusCode::PARTIAL_CONTENT, "{who} HEAD");
        let resp = consumer_get(&rig.server, url, &token, &headers, false).await;
        assert_eq!(resp.status(), StatusCode::PARTIAL_CONTENT, "{who}");
        assert_eq!(
            resp.headers()["content-range"],
            format!("bytes {from}-{to}/{total}").as_str(),
            "{who}"
        );
        if who == "dlna" {
            let features = resp
                .headers()
                .get("contentfeatures.dlna.org")
                .map(|v| v.to_str().unwrap().to_string());
            assert!(
                features
                    .as_deref()
                    .is_some_and(|f| f.contains("DLNA.ORG_OP=01")),
                "a renderer asked for seekable content features: {features:?}"
            );
        }
        let body = resp.bytes().await.unwrap();
        assert!(
            body[..] == ITA_ENG_FIXTURE[from..=to],
            "{who} read other bytes"
        );
    }
}

/// kobload keeps importing an NZB after the tap that asked for it gave up (the client's
/// own timeout is shorter than a pack import), and a later tap reuses that import. The
/// abandoned tap must leave nothing behind that stops the later one.
#[tokio::test]
async fn a_tap_abandoned_while_kobload_imports_leaves_the_next_tap_free_to_play() {
    let rig = engine_rig_with(Some(Duration::from_secs(4))).await;
    let movie = rig.movie().await;
    let body = rig.resolve_body_from(Source::Usenet, "movie", movie);

    let abandoned = reqwest::Client::new()
        .post(format!(
            "{}/api/v1/admin/stream/resolve",
            rig.server.api_url
        ))
        .bearer_auth(&rig.server.admin_token)
        .json(&body)
        .timeout(Duration::from_secs(1))
        .send()
        .await;
    assert!(abandoned.is_err(), "the first tap was meant to give up");

    let started = std::time::Instant::now();
    let session = session_id_of(&rig.resolve_with(&body).await);
    assert!(
        started.elapsed() < Duration::from_secs(2),
        "the later tap waited on the abandoned one: {:?}",
        started.elapsed()
    );
    let played = play(&rig.server, session, "bytes=0-1023").await;
    assert_eq!(played.status, StatusCode::PARTIAL_CONTENT);
    tokio::time::sleep(Duration::from_secs(4)).await;
    let failed: Vec<Value> = rig
        .kobload_requests("/stream/outcome")
        .await
        .into_iter()
        .filter(|r| r["ok"] == false)
        .collect();
    assert!(
        failed.is_empty(),
        "the abandoned tap demoted the release: {failed:?}"
    );
}
