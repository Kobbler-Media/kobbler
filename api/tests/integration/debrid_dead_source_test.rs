use crate::common::*;
use serde_json::{Value, json};
use std::time::{Duration, Instant};
use uuid::Uuid;
use wiremock::matchers::{method, path, path_regex};
use wiremock::{Mock, MockServer, ResponseTemplate};

/// One retry against the host, and well inside the six seconds the probe alone took to give
/// up on a 404 with a message that never mentioned it.
const FAIL_WITHIN: Duration = Duration::from_secs(4);

async fn dead_source_server(upstream_status: u16) -> (TestServer, MockServer) {
    let mock = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path_regex("/api/v1/stream/resolve"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "stream_url": format!("{}/d/dead/417%20-%20Hackidu.mkv", mock.uri()),
            "filename": "417 - Hackidu.mkv",
            "filesize": 497_548_841,
            "cached": true
        })))
        .mount(&mock)
        .await;
    Mock::given(method("GET"))
        .and(path_regex("^/d/dead/"))
        .respond_with(ResponseTemplate::new(upstream_status))
        .mount(&mock)
        .await;
    Mock::given(method("POST"))
        .and(path("/api/v1/stream/outcome"))
        .respond_with(ResponseTemplate::new(204))
        .mount(&mock)
        .await;

    let server = TestServer::start_api_only().await;
    server
        .admin_client()
        .patch_json(
            "/api/v1/admin/config",
            &json!({ "kobload_base_url": mock.uri(), "kobload_api_key": "test-key" }),
        )
        .await
        .assert_ok();
    (server, mock)
}

async fn open_session(server: &TestServer) -> String {
    let movie_id = Uuid::now_v7();
    sqlx::query("INSERT INTO movies (id, title, year) VALUES ($1, 'Dead Source', 1999)")
        .bind(movie_id)
        .execute(&server.pool)
        .await
        .unwrap();
    let resp = server
        .admin_client()
        .post_json(
            "/api/v1/admin/stream/resolve",
            &json!({
                "debrid_provider_id": Uuid::nil(),
                "info_hash": format!("{}", movie_id.simple()),
                "file_id": 16,
                "media_type": "movie",
                "media_id": movie_id,
                "release_title": "Dead.Source.1999.1080p.hevc",
                "debrid_provider": "real_debrid",
                "capabilities": {
                    "video_codecs": ["h264", "hevc"],
                    "audio_codecs": ["aac"],
                    "containers": ["mkv", "mp4"]
                }
            }),
        )
        .await;
    resp.assert_ok();
    resp.json::<Value>()["session_id"]
        .as_str()
        .expect("a cached release opens a session")
        .to_string()
}

async fn status_once_settled(server: &TestServer, session_id: &str) -> (Value, Duration) {
    let started = Instant::now();
    loop {
        let status = server
            .admin_client()
            .get(&format!(
                "/api/v1/admin/playback/sessions/{session_id}/status"
            ))
            .await
            .json::<Value>();
        if status["state"] == "failed" || started.elapsed() > FAIL_WITHIN {
            return (status, started.elapsed());
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
}

/// The incident's release resolved to a link the debrid host would not serve. The session
/// sat in `preparing` while the probe retried, so the client only heard about it once its
/// own patience ran out — the server knew from the first refused request.
#[tokio::test]
async fn a_session_whose_source_is_refused_fails_promptly_and_says_why() {
    for upstream in [404u16, 503] {
        let (server, _mock) = dead_source_server(upstream).await;
        let session_id = open_session(&server).await;

        let (status, took) = status_once_settled(&server, &session_id).await;

        assert_eq!(
            status["state"], "failed",
            "HTTP {upstream}: still {} after {took:?}: {status}",
            status["state"]
        );
        assert!(took < FAIL_WITHIN, "HTTP {upstream}: took {took:?}");
        let message = status["message"].as_str().unwrap_or_default();
        assert!(
            message.contains(&upstream.to_string()),
            "HTTP {upstream}: the reason did not say what the host answered: {message}"
        );
    }
}
