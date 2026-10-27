use crate::common::*;
use api::jobs::handlers::DebridPrewarmHandler;
use reqwest::StatusCode;
use serde_json::{Value, json};
use std::time::Duration;
use uuid::Uuid;
use wiremock::matchers::{body_partial_json, method, path_regex};
use wiremock::{Mock, MockServer, ResponseTemplate};

const PROVIDER_ID: &str = "00000000-0000-0000-0000-000000000001";

async fn kobload_server() -> (TestServer, MockServer) {
    let mock = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path_regex("/api/v1/stream/outcome"))
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

async fn admin_id(server: &TestServer) -> Uuid {
    sqlx::query_scalar("SELECT id FROM users WHERE role = 'admin' LIMIT 1")
        .fetch_one(&server.pool)
        .await
        .unwrap()
}

async fn insert_movie(server: &TestServer, imdb_id: &str) -> Uuid {
    let id = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO movies (id, title, year, identifiers, enriched_at)
         VALUES ($1, 'Outcome Movie', 2025,
                 jsonb_build_object('tmdb_id', $2::TEXT, 'imdb_id', $3::TEXT), NOW())",
    )
    .bind(id)
    .bind(id.simple().to_string())
    .bind(imdb_id)
    .execute(&server.pool)
    .await
    .unwrap();
    id
}

/// Returns the show's episodes in order: S01E01, S01E02, then S02E01.
async fn insert_show(server: &TestServer, title: &str) -> Vec<Uuid> {
    let show = Uuid::now_v7();
    sqlx::query("INSERT INTO tv_shows (id, title, year) VALUES ($1, $2, 2024)")
        .bind(show)
        .bind(title)
        .execute(&server.pool)
        .await
        .unwrap();
    let mut episodes = Vec::new();
    for (season, numbers) in [(1, vec![1, 2]), (2, vec![1])] {
        let season_id = Uuid::now_v7();
        sqlx::query(
            "INSERT INTO tv_seasons (id, show_id, season_identifier, season_sort_order)
             VALUES ($1, $2, $3, $4)",
        )
        .bind(season_id)
        .bind(show)
        .bind(season.to_string())
        .bind(season)
        .execute(&server.pool)
        .await
        .unwrap();
        for n in numbers {
            let id = Uuid::now_v7();
            sqlx::query(
                "INSERT INTO tv_episodes (id, season_id, episode_identifier, episode_sort_order)
                 VALUES ($1, $2, $3, $4)",
            )
            .bind(id)
            .bind(season_id)
            .bind(n.to_string())
            .bind(n)
            .execute(&server.pool)
            .await
            .unwrap();
            episodes.push(id);
        }
    }
    episodes
}

struct SessionSeed<'a> {
    media_type: &'a str,
    media_id: Uuid,
    info_hash: &'a str,
    file_id: Option<i32>,
    debrid_url: Option<&'a str>,
    debrid_url_expires_at: Option<&'a str>,
    cache_path: Option<&'a str>,
    created_minutes_ago: i64,
    played: bool,
}

impl<'a> SessionSeed<'a> {
    fn new(media_id: Uuid, info_hash: &'a str) -> Self {
        Self {
            media_type: "movie",
            media_id,
            info_hash,
            file_id: None,
            debrid_url: None,
            debrid_url_expires_at: None,
            cache_path: None,
            created_minutes_ago: 0,
            played: false,
        }
    }

    fn episode(media_id: Uuid, info_hash: &'a str) -> Self {
        Self {
            media_type: "episode",
            ..Self::new(media_id, info_hash)
        }
    }
}

async fn insert_session(server: &TestServer, user_id: Uuid, seed: SessionSeed<'_>) -> Uuid {
    let id = Uuid::now_v7();
    sqlx::query(
        r#"INSERT INTO debrid_stream_sessions
            (id, user_id, media_type, media_id, release_title, info_hash, debrid_provider,
             filename, filesize, file_id, debrid_url, debrid_url_expires_at, cache_path,
             created_at, played_at)
           VALUES ($1, $2, $3, $4, $5, $6, $7,
                   'movie.mp4', 1000000, $8, $9, $10::timestamptz, $11,
                   NOW() - make_interval(mins => $12::int),
                   CASE WHEN $13 THEN NOW() - make_interval(mins => $12::int) END)"#,
    )
    .bind(id)
    .bind(user_id)
    .bind(seed.media_type)
    .bind(seed.media_id)
    .bind(format!("Release.{}.1080p-GRP", seed.info_hash))
    .bind(seed.info_hash)
    .bind(PROVIDER_ID)
    .bind(seed.file_id)
    .bind(seed.debrid_url)
    .bind(seed.debrid_url_expires_at)
    .bind(seed.cache_path)
    .bind(seed.created_minutes_ago as i32)
    .bind(seed.played)
    .execute(&server.pool)
    .await
    .unwrap();
    id
}

async fn requests_to(mock: &MockServer, suffix: &str) -> Vec<Value> {
    mock.received_requests()
        .await
        .unwrap_or_default()
        .iter()
        .filter(|r| r.url.path().ends_with(suffix))
        .map(|r| serde_json::from_slice(&r.body).unwrap())
        .collect()
}

async fn outcome_reports(mock: &MockServer) -> Vec<Value> {
    requests_to(mock, "/stream/outcome").await
}

async fn wait_for_reports(mock: &MockServer, count: usize) -> Vec<Value> {
    let deadline = std::time::Instant::now() + Duration::from_secs(30);
    loop {
        let reports = outcome_reports(mock).await;
        if reports.len() >= count || std::time::Instant::now() > deadline {
            return reports;
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
}

fn summary(report: &Value) -> (String, Option<i64>, bool, Option<String>) {
    (
        report["info_hash"].as_str().unwrap().to_string(),
        report["file_id"].as_i64(),
        report["ok"].as_bool().unwrap(),
        report["reason"].as_str().map(String::from),
    )
}

fn option(info_hash: &str, file_id: Option<i32>, playback: Option<&str>) -> Value {
    let id = match file_id {
        Some(f) => format!("torrent_{info_hash}_f{f}_{PROVIDER_ID}"),
        None => format!("torrent_{info_hash}_{PROVIDER_ID}"),
    };
    let files: Vec<Value> = file_id
        .map(|f| json!({ "id": f, "path": format!("Show/E{f}.mkv"), "size": 1000 }))
        .into_iter()
        .collect();
    let mut value = json!({
        "id": id,
        "release_title": format!("Release.{info_hash}"),
        "size": 1000,
        "indexer": "test",
        "info_hash": info_hash,
        "total_score": 1.0,
        "match_score": 1.0,
        "quality_score": 1.0,
        "debrid_provider": "realdebrid",
        "debrid_provider_id": PROVIDER_ID,
        "cached": true,
        "source_type": "torrent",
        "files": files,
    });
    if let Some(p) = playback {
        value["playback"] = json!(p);
    }
    value
}

async fn mount_search(mock: &MockServer, options: Vec<Value>) {
    Mock::given(method("POST"))
        .and(path_regex("/api/v1/stream/search"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(json!({ "options": options, "errors": ["one indexer timed out"] })),
        )
        .mount(mock)
        .await;
}

async fn search(server: &TestServer, body: Value) -> Value {
    let resp = server
        .admin_client()
        .post_json("/api/v1/admin/stream/search", &body)
        .await;
    resp.assert_ok();
    resp.json()
}

fn outcome_path(session_id: Uuid) -> String {
    format!("/api/v1/admin/stream/{session_id}/outcome")
}

async fn post_progress(server: &TestServer, session: Uuid, seconds: f64) {
    server
        .admin_client()
        .post_json(
            &format!("/api/v1/admin/stream/{session}/progress"),
            &json!({ "progress_seconds": seconds }),
        )
        .await
        .assert_status(StatusCode::NO_CONTENT);
}

#[tokio::test]
async fn test_outcome_ok_forwards_one_success_per_session() {
    let (server, mock) = kobload_server().await;
    let user = admin_id(&server).await;
    let movie = insert_movie(&server, "tt9000001").await;
    let mut seed = SessionSeed::new(movie, "okhash");
    seed.file_id = Some(3);
    let first = insert_session(&server, user, seed).await;
    let mut seed = SessionSeed::new(movie, "okhash");
    seed.file_id = Some(3);
    let second = insert_session(&server, user, seed).await;

    for session in [first, first, second] {
        server
            .admin_client()
            .post_json(&outcome_path(session), &json!({ "ok": true }))
            .await
            .assert_status(StatusCode::NO_CONTENT);
    }

    let reports = outcome_reports(&mock).await;
    assert_eq!(
        reports.iter().map(summary).collect::<Vec<_>>(),
        vec![
            ("okhash".to_string(), Some(3), true, None),
            ("okhash".to_string(), Some(3), true, None),
        ],
        "a repeated ok from one session must be forwarded once; a second session counts again"
    );
    assert_eq!(reports[0]["debrid_provider_id"], PROVIDER_ID);
    assert_eq!(reports[0]["release_title"], "Release.okhash.1080p-GRP");
    assert_eq!(reports[0]["filename"], "movie.mp4");
}

#[tokio::test]
async fn test_outcome_failure_forwards_each_report_with_reason() {
    let (server, mock) = kobload_server().await;
    let user = admin_id(&server).await;
    let movie = insert_movie(&server, "tt9000002").await;
    let session = insert_session(&server, user, SessionSeed::new(movie, "failhash")).await;

    for body in [
        json!({ "ok": false, "reason": "decoder_error" }),
        json!({ "ok": false }),
    ] {
        server
            .admin_client()
            .post_json(&outcome_path(session), &body)
            .await
            .assert_status(StatusCode::NO_CONTENT);
    }
    assert_eq!(
        outcome_reports(&mock)
            .await
            .iter()
            .map(summary)
            .collect::<Vec<_>>(),
        vec![
            (
                "failhash".to_string(),
                None,
                false,
                Some("decoder_error".to_string())
            ),
            (
                "failhash".to_string(),
                None,
                false,
                Some("client_reported".to_string())
            ),
        ]
    );
}

#[tokio::test]
async fn test_outcome_rejects_unknown_foreign_and_anonymous_callers() {
    let (server, mock) = kobload_server().await;
    let alice = create_user_with_role(&server, "alice-outcome@test.com", "viewer").await;
    let bob = create_user_client(&server, "bob-outcome@test.com", "viewer").await;
    let movie = insert_movie(&server, "tt9000003").await;
    let alices = insert_session(&server, alice, SessionSeed::new(movie, "privatehash")).await;
    let body = json!({ "ok": true });

    server
        .admin_client()
        .post_json(&outcome_path(Uuid::now_v7()), &body)
        .await
        .assert_status(StatusCode::NOT_FOUND);
    bob.post_json(&outcome_path(alices), &body)
        .await
        .assert_status(StatusCode::NOT_FOUND);
    TestClient::new(&server.api_url)
        .post_json(&outcome_path(alices), &body)
        .await
        .assert_status(StatusCode::UNAUTHORIZED);

    assert!(outcome_reports(&mock).await.is_empty());
}

#[tokio::test]
async fn test_progress_past_a_minute_forwards_one_success() {
    let (server, mock) = kobload_server().await;
    let user = admin_id(&server).await;
    let movie = insert_movie(&server, "tt9000004").await;
    let session = insert_session(&server, user, SessionSeed::new(movie, "progresshash")).await;

    for (progress, reports) in [(30.0, 0), (90.0, 1), (180.0, 1)] {
        post_progress(&server, session, progress).await;
        assert_eq!(
            outcome_reports(&mock).await.len(),
            reports,
            "after progress {progress}"
        );
    }
    server
        .admin_client()
        .post_json(&outcome_path(session), &json!({ "ok": true }))
        .await
        .assert_status(StatusCode::NO_CONTENT);
    assert_eq!(
        outcome_reports(&mock).await.len(),
        1,
        "an explicit ok after progress already counted is the same success"
    );
}

#[tokio::test]
async fn test_search_passes_kobload_ranking_through_unchanged() {
    let (server, mock) = kobload_server().await;
    let mut binged = option("hashb", None, None);
    binged["binge_group"] = json!("torrentio|1080p|WEB");
    let options = vec![
        binged,
        option("hashc", Some(2), Some("worked")),
        option("hasha", Some(1), Some("failed")),
    ];
    mount_search(&mock, options.clone()).await;

    let body = search(&server, json!({ "title": "Outcome Movie", "year": 2025 })).await;

    let fields = |opts: &Value| -> Vec<(Value, Value, Value)> {
        opts.as_array()
            .unwrap()
            .iter()
            .map(|o| {
                (
                    o["id"].clone(),
                    o["playback"].clone(),
                    o["binge_group"].clone(),
                )
            })
            .collect()
    };
    assert_eq!(fields(&body["options"]), fields(&json!(options)));
    assert_eq!(body["errors"], json!(["one indexer timed out"]));
}

#[tokio::test]
async fn test_search_prefers_only_streams_that_played() {
    let (server, mock) = kobload_server().await;
    mount_search(
        &mock,
        vec![
            option("workedhash", None, Some("worked")),
            option("brokenhash", None, Some("failed")),
        ],
    )
    .await;
    let user = admin_id(&server).await;

    let worked_then_untried = insert_movie(&server, "tt9000010").await;
    let mut seed = SessionSeed::new(worked_then_untried, "workedhash");
    seed.created_minutes_ago = 60;
    seed.played = true;
    insert_session(&server, user, seed).await;
    insert_session(
        &server,
        user,
        SessionSeed::new(worked_then_untried, "untriedhash"),
    )
    .await;

    let only_untried = insert_movie(&server, "tt9000011").await;
    insert_session(&server, user, SessionSeed::new(only_untried, "loneuntried")).await;

    let played_then_failed = insert_movie(&server, "tt9000012").await;
    let mut seed = SessionSeed::new(played_then_failed, "brokenhash");
    seed.played = true;
    insert_session(&server, user, seed).await;

    let cases = [
        (worked_then_untried, Some("workedhash")),
        (only_untried, None),
        (played_then_failed, None),
    ];
    for (movie, preferred) in cases {
        let body = search(&server, json!({ "movie_id": movie })).await;
        assert_eq!(
            body.get("preferred_info_hash").and_then(|v| v.as_str()),
            preferred,
            "movie {movie}: {body}"
        );
    }

    let prefers: Vec<Value> = requests_to(&mock, "/stream/search")
        .await
        .into_iter()
        .map(|r| r["prefer"].clone())
        .collect();
    assert_eq!(
        prefers,
        vec![
            json!({ "info_hash": "workedhash", "file_id": null,
                    "release_title": "Release.workedhash.1080p-GRP" }),
            Value::Null,
            json!({ "info_hash": "brokenhash", "file_id": null,
                    "release_title": "Release.brokenhash.1080p-GRP" }),
        ],
        "kobload is told what played so it ranks and warms it before anything else"
    );
}

#[tokio::test]
async fn test_episode_search_carries_continuity_from_the_last_played_episode() {
    let (server, mock) = kobload_server().await;
    mount_search(&mock, vec![option("anyhash", None, None)]).await;
    let user = admin_id(&server).await;
    let other = create_user_with_role(&server, "other-binge@test.com", "viewer").await;
    let episodes = insert_show(&server, "Binge Show").await;

    let mut mine = SessionSeed::episode(episodes[0], "packhash");
    mine.file_id = Some(1);
    mine.played = true;
    mine.created_minutes_ago = 30;
    insert_session(&server, user, mine).await;
    let mut theirs = SessionSeed::episode(episodes[0], "theirhash");
    theirs.played = true;
    insert_session(&server, other, theirs).await;

    search(&server, json!({ "episode_id": episodes[1] })).await;

    let requests = requests_to(&mock, "/stream/search").await;
    assert_eq!(
        requests[0]["prefer"],
        json!({ "info_hash": "packhash", "file_id": 1,
                "release_title": "Release.packhash.1080p-GRP" })
    );
}

#[tokio::test]
async fn test_next_episode_prewarm_fires_once_and_matches_the_real_search() {
    let (server, mock) = kobload_server().await;
    mount_search(&mock, vec![option("anyhash", None, None)]).await;
    let user = admin_id(&server).await;
    let episodes = insert_show(&server, "Prewarm Show").await;
    let mut seed = SessionSeed::episode(episodes[1], "s1pack");
    seed.file_id = Some(2);
    let session = insert_session(&server, user, seed).await;

    for seconds in [10.0, 90.0, 400.0] {
        post_progress(&server, session, seconds).await;
    }
    server
        .admin_client()
        .post_json(&outcome_path(session), &json!({ "ok": true }))
        .await
        .assert_status(StatusCode::NO_CONTENT);
    let pending: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM job_status WHERE job_type = 'debrid_prewarm' AND status = 'pending'",
    )
    .fetch_one(&server.pool)
    .await
    .unwrap();
    assert_eq!(
        pending, 1,
        "one prewarm per session, however often it reports"
    );

    server
        .process_pending_jobs(&DebridPrewarmHandler::new(server.app_state.clone()))
        .await;
    let prewarm = requests_to(&mock, "/stream/search").await;
    assert_eq!(prewarm.len(), 1);
    assert_eq!(
        prewarm[0]["media_want"]["season"], "2",
        "the episode after S01E02 is S02E01: {}",
        prewarm[0]
    );
    assert_eq!(
        prewarm[0]["prefer"],
        json!({ "info_hash": "s1pack", "file_id": 2,
                "release_title": "Release.s1pack.1080p-GRP" })
    );

    search(&server, json!({ "episode_id": episodes[2] })).await;
    let searches = requests_to(&mock, "/stream/search").await;
    assert_eq!(
        searches[1], searches[0],
        "the real search must be the one kobload already warmed, or its caches miss"
    );
}

#[tokio::test]
async fn test_stremio_keeps_kobload_order_and_drops_failed_from_fallbacks() {
    let (server, mock) = kobload_server().await;
    let user = admin_id(&server).await;
    insert_movie(&server, "tt9000020").await;
    sqlx::query("INSERT INTO stremio_api_keys (user_id, api_key) VALUES ($1, 'outcome-key')")
        .bind(user)
        .execute(&server.pool)
        .await
        .unwrap();
    mount_search(
        &mock,
        vec![
            option("stremok", None, None),
            option("stremnext", None, None),
            option("stremfail", None, Some("failed")),
        ],
    )
    .await;

    let resp = TestClient::new(&server.api_url)
        .get("/stremio/outcome-key/stream/movie/tt9000020.json")
        .await;
    resp.assert_ok();
    let body: Value = resp.json();
    let urls: Vec<&str> = body["streams"]
        .as_array()
        .unwrap()
        .iter()
        .map(|s| s["url"].as_str().unwrap())
        .collect();
    assert_eq!(urls.len(), 3);
    for (url, hash) in urls.iter().zip(["stremok", "stremnext", "stremfail"]) {
        assert!(
            url.contains(hash),
            "expected {hash} in {url}; order {urls:?}"
        );
    }

    let fallbacks = server
        .app_state
        .stremio_play_cache
        .get(&format!("torrent_stremok_{PROVIDER_ID}"))
        .unwrap()
        .fallback_option_ids
        .clone();
    assert_eq!(fallbacks, vec![format!("torrent_stremnext_{PROVIDER_ID}")]);
}

#[tokio::test]
async fn test_resolve_errors_are_friendly_and_left_to_kobload() {
    let (server, mock) = kobload_server().await;
    let cases = [
        (
            "rdblocked",
            "Debrid error: Real-Debrid refused this release. Try a different source.",
            "blocked",
        ),
        (
            "rdlimited",
            "Debrid error: Failed to add magnet: Rate limited",
            "rate_limited",
        ),
        (
            "rdauth",
            "Debrid error: Failed to add magnet: Authentication failed",
            "auth_failed",
        ),
        (
            "rdslow",
            "Debrid error: Timed out waiting for usenet download",
            "timeout",
        ),
    ];
    for (hash, raw, _) in &cases {
        Mock::given(method("POST"))
            .and(path_regex("/api/v1/stream/resolve"))
            .and(body_partial_json(json!({ "info_hash": hash })))
            .respond_with(ResponseTemplate::new(500).set_body_string(*raw))
            .mount(&mock)
            .await;
    }

    for (hash, raw, reason) in cases {
        let resp = server
            .admin_client()
            .post_json(
                "/api/v1/admin/stream/resolve",
                &json!({
                    "debrid_provider_id": PROVIDER_ID,
                    "info_hash": hash,
                    "media_type": "movie",
                    "release_title": "Some.Release",
                    "debrid_provider": "realdebrid"
                }),
            )
            .await;
        resp.assert_status(StatusCode::BAD_GATEWAY);
        let body: Value = resp.json();
        assert_eq!(body["reason"], reason, "{hash}: {body}");
        let error = body["error"].as_str().unwrap();
        assert!(
            !error.is_empty() && !error.contains("HTTP"),
            "{hash}: error must be a human sentence, got {error:?}"
        );
        assert!(
            body["detail"].as_str().unwrap().contains(raw),
            "{hash}: detail must carry the raw kobload error, got {body}"
        );
    }
    assert!(
        outcome_reports(&mock).await.is_empty(),
        "kobload saw the provider's error first-hand and already recorded it"
    );
}

#[tokio::test]
async fn test_resolve_non_video_is_forwarded_against_the_release() {
    let (server, mock) = kobload_server().await;
    Mock::given(method("POST"))
        .and(path_regex("/api/v1/stream/resolve"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "stream_url": "https://example.invalid/setup.exe",
            "filename": "setup.exe",
            "filesize": 1000,
            "cached": true
        })))
        .mount(&mock)
        .await;
    let movie = insert_movie(&server, "tt9000030").await;

    let resp = server
        .admin_client()
        .post_json(
            "/api/v1/admin/stream/resolve",
            &json!({
                "debrid_provider_id": PROVIDER_ID,
                "info_hash": "exehash",
                "file_id": 2,
                "media_type": "movie",
                "media_id": movie,
                "release_title": "Fake.Release",
                "debrid_provider": "realdebrid"
            }),
        )
        .await;
    resp.assert_status(StatusCode::UNPROCESSABLE_ENTITY);
    let body: Value = resp.json();
    assert_eq!(body["reason"], "not_video", "{body}");
    assert!(body["detail"].as_str().unwrap().contains(".exe"), "{body}");
    let reports = outcome_reports(&mock).await;
    assert_eq!(
        reports.iter().map(summary).collect::<Vec<_>>(),
        vec![(
            "exehash".to_string(),
            Some(2),
            false,
            Some("not_video".to_string())
        )]
    );
    assert_eq!(reports[0]["release_title"], "Fake.Release");
    assert_eq!(reports[0]["filename"], "setup.exe");
}

/// Clients rank and skip on fields kobload adds to an option; kobbler must not be the
/// layer that strips the ones it has not heard of yet.
#[tokio::test]
async fn test_search_passes_option_fields_through_untouched() {
    let (server, mock) = kobload_server().await;
    let mut ready = option("passready", None, Some("worked"));
    ready["availability"] = json!("ready");
    ready["some_future_field"] = json!({ "nested": [1, 2] });
    mount_search(&mock, vec![ready]).await;

    let resp = server
        .admin_client()
        .post_json(
            "/api/v1/admin/stream/search",
            &json!({ "title": "Outcome Movie", "year": 2025 }),
        )
        .await;
    resp.assert_ok();
    let body: Value = resp.json();
    let got = &body["options"][0];
    assert_eq!(got["availability"], "ready", "{body}");
    assert_eq!(got["playback"], "worked", "{body}");
    assert_eq!(
        got["some_future_field"],
        json!({ "nested": [1, 2] }),
        "{body}"
    );
    assert_eq!(body["errors"][0], "one indexer timed out", "{body}");
}

#[tokio::test]
async fn test_resolve_of_an_uncached_release_reports_caching() {
    let (server, mock) = kobload_server().await;
    Mock::given(method("POST"))
        .and(path_regex("/api/v1/stream/resolve"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "stream_url": "",
            "filename": "",
            "filesize": 0,
            "cached": false,
            "torrent_id": "rd-123"
        })))
        .mount(&mock)
        .await;

    let resp = server
        .admin_client()
        .post_json(
            "/api/v1/admin/stream/resolve",
            &json!({
                "debrid_provider_id": PROVIDER_ID,
                "info_hash": "uncachedhash",
                "media_type": "movie",
                "release_title": "Some.Release",
                "debrid_provider": "realdebrid"
            }),
        )
        .await;
    resp.assert_ok();
    let body: Value = resp.json();
    assert_eq!(body["status"], "caching", "{body}");
    assert!(body["session_id"].is_null(), "{body}");
    assert_eq!(body["torrent_id"], "rd-123", "{body}");
}

#[tokio::test]
async fn test_search_error_body_matches_resolve_shape() {
    let (server, mock) = kobload_server().await;
    Mock::given(method("POST"))
        .and(path_regex("/api/v1/stream/search"))
        .respond_with(
            ResponseTemplate::new(500).set_body_string("Search failed: all indexers down"),
        )
        .mount(&mock)
        .await;

    let resp = server
        .admin_client()
        .post_json(
            "/api/v1/admin/stream/search",
            &json!({ "title": "Outcome Movie", "year": 2025 }),
        )
        .await;
    resp.assert_status(StatusCode::BAD_GATEWAY);
    let body: Value = resp.json();
    assert_eq!(body["reason"], "unavailable", "{body}");
    assert!(!body["error"].as_str().unwrap().is_empty());
    assert!(
        body["detail"]
            .as_str()
            .unwrap()
            .contains("all indexers down")
    );
}

#[tokio::test]
async fn test_background_probe_failure_is_forwarded() {
    let (server, mock) = kobload_server().await;
    Mock::given(method("POST"))
        .and(path_regex("/api/v1/stream/resolve"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "stream_url": "http://127.0.0.1:9/unreachable.mkv",
            "filename": "unreachable.mkv",
            "filesize": 1_000_000,
            "cached": true
        })))
        .mount(&mock)
        .await;
    let movie = insert_movie(&server, "tt9000040").await;

    let resp = server
        .admin_client()
        .post_json(
            "/api/v1/admin/stream/resolve",
            &json!({
                "debrid_provider_id": PROVIDER_ID,
                "info_hash": "probehash",
                "file_id": 5,
                "media_type": "movie",
                "media_id": movie,
                "release_title": "Probe.Release",
                "debrid_provider": "realdebrid"
            }),
        )
        .await;
    resp.assert_ok();

    let reports = wait_for_reports(&mock, 1).await;
    assert_eq!(
        reports.iter().map(summary).collect::<Vec<_>>(),
        vec![(
            "probehash".to_string(),
            Some(5),
            false,
            Some("probe_failed".to_string())
        )]
    );
}

#[tokio::test]
async fn test_stream_play_upstream_failure_is_forwarded() {
    let (server, mock) = kobload_server().await;
    let user = admin_id(&server).await;
    let movie = insert_movie(&server, "tt9000050").await;
    let mut seed = SessionSeed::new(movie, "upstreamhash");
    seed.file_id = Some(9);
    seed.debrid_url = Some("http://127.0.0.1:9/gone.mp4");
    let session = insert_session(&server, user, seed).await;

    server
        .admin_client()
        .get(&format!("/api/v1/admin/stream/{session}/play"))
        .await
        .assert_status(StatusCode::BAD_GATEWAY);

    let reports = wait_for_reports(&mock, 1).await;
    assert_eq!(
        reports.iter().map(summary).collect::<Vec<_>>(),
        vec![(
            "upstreamhash".to_string(),
            Some(9),
            false,
            Some("upstream_failed".to_string())
        )]
    );
}

/// A season-pack session that re-resolves after its URL expired must ask for the
/// same file, or the player gets whichever episode the provider lists first.
#[tokio::test]
async fn test_stream_play_reresolve_keeps_session_file_id() {
    struct Case {
        in_memory_resume: bool,
        expected_requests: usize,
    }
    let cases = [
        Case {
            in_memory_resume: false,
            expected_requests: 1,
        },
        Case {
            in_memory_resume: true,
            expected_requests: 2,
        },
    ];
    for case in cases {
        let (server, mock) = kobload_server().await;
        Mock::given(method("POST"))
            .and(path_regex("/api/v1/stream/resolve"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "stream_url": "",
                "filename": "packhash",
                "filesize": 0,
                "cached": false
            })))
            .mount(&mock)
            .await;
        let user = admin_id(&server).await;
        let movie = insert_movie(&server, "tt9000060").await;
        let dir = tempfile::tempdir().unwrap();
        let partial = dir.path().join("E07.mkv");
        std::fs::write(&partial, vec![0u8; 10]).unwrap();
        let partial_str = partial.to_str().unwrap().to_string();

        let mut seed = SessionSeed::new(movie, "packhash");
        seed.file_id = Some(7);
        if case.in_memory_resume {
            seed.cache_path = Some(&partial_str);
        } else {
            seed.debrid_url = Some("https://expired.example.com/old.mkv");
            seed.debrid_url_expires_at = Some("2020-01-01T00:00:00Z");
        }
        let session = insert_session(&server, user, seed).await;
        if case.in_memory_resume {
            let mut ps = api::streaming::session::PlaybackSession::test_direct_play(
                session,
                user,
                partial.clone(),
            );
            let cancelled = tokio_util::sync::CancellationToken::new();
            cancelled.cancel();
            ps.download_cancel = Some(cancelled);
            server.app_state.playback_sessions.insert(session, ps);
        }

        server
            .admin_client()
            .get(&format!("/api/v1/admin/stream/{session}/play"))
            .await
            .assert_status(StatusCode::SERVICE_UNAVAILABLE);

        let deadline = std::time::Instant::now() + Duration::from_secs(10);
        let requests = loop {
            let resolves = requests_to(&mock, "/stream/resolve").await;
            if resolves.len() >= case.expected_requests || std::time::Instant::now() > deadline {
                break resolves;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        };
        assert_eq!(requests.len(), case.expected_requests, "{requests:?}");
        for req in requests {
            assert_eq!(req["file_id"], 7, "re-resolve dropped the file: {req}");
            assert_eq!(
                req["release_title"], "Release.packhash.1080p-GRP",
                "kobload logs a pack's play under the pack's title without it: {req}"
            );
            assert_eq!(req["file_name"], "movie.mp4", "{req}");
        }
    }
}

#[tokio::test]
async fn test_stremio_play_rebuilds_fallbacks_after_restart() {
    let (server, mock) = kobload_server().await;
    let user = admin_id(&server).await;
    insert_movie(&server, "tt9000021").await;
    sqlx::query("INSERT INTO stremio_api_keys (user_id, api_key) VALUES ($1, 'rebuild-key')")
        .bind(user)
        .execute(&server.pool)
        .await
        .unwrap();
    mount_search(
        &mock,
        vec![
            option("rebuildfirst", None, None),
            option("rebuildfail", None, Some("failed")),
            option("rebuildnext", None, None),
        ],
    )
    .await;
    Mock::given(method("POST"))
        .and(path_regex("/api/v1/stream/resolve"))
        .respond_with(
            ResponseTemplate::new(500)
                .set_body_string("Debrid error: Real-Debrid refused this release."),
        )
        .mount(&mock)
        .await;

    let resp = TestClient::new(&server.api_url)
        .get("/stremio/rebuild-key/stream/movie/tt9000021.json")
        .await;
    resp.assert_ok();
    let body: Value = resp.json();
    let url = body["streams"][0]["url"].as_str().unwrap();
    let play_path = &url[url.find("/stremio/").unwrap()..];

    // Stremio replays the stream list it cached before the API restarted.
    server.app_state.stremio_play_cache.clear();
    TestClient::new(&server.api_url)
        .get(play_path)
        .await
        .assert_status(StatusCode::SERVICE_UNAVAILABLE);

    let tried: Vec<String> = requests_to(&mock, "/stream/resolve")
        .await
        .iter()
        .map(|r| r["info_hash"].as_str().unwrap().to_string())
        .collect();
    assert_eq!(tried, ["rebuildfirst", "rebuildnext"]);

    let searches = requests_to(&mock, "/stream/search").await;
    assert_eq!(searches.len(), 2);
    assert_eq!(
        searches[1], searches[0],
        "the rebuilt chain must come from the same ranked search Stremio listed"
    );
}
