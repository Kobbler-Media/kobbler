use crate::common::*;
use reqwest::StatusCode;
use serde_json::json;
use std::io::Write;
use uuid::Uuid;
use wiremock::matchers::{header, method, path_regex};
use wiremock::{Mock, MockServer, ResponseTemplate};

/// When the debrid_url has no expiry (debrid_url_expires_at IS NULL),
/// stream_play should use the cached URL directly instead of re-resolving.
/// This is the common case with Real-Debrid, which often omits expiry.
#[tokio::test]
async fn test_stream_play_uses_cached_url_without_expiry() {
    let mock_server = MockServer::start().await;

    // Mock kobload resolve — should NOT be called if cached URL is used
    Mock::given(method("POST"))
        .and(path_regex("/api/v1/stream/resolve"))
        .respond_with(ResponseTemplate::new(500).set_body_json(json!({
            "error": "should not be called"
        })))
        .expect(0) // assert this mock is never called
        .mount(&mock_server)
        .await;

    let server = TestServer::start_api_only().await;

    // Configure kobload (even though we don't expect it to be called)
    server
        .admin_client()
        .patch_json(
            "/api/v1/admin/config",
            &json!({
                "kobload_base_url": mock_server.uri(),
                "kobload_api_key": "test-key"
            }),
        )
        .await
        .assert_ok();

    // Create a movie so we have a valid media_id
    let movie_resp = server
        .admin_client()
        .post_json(
            "/api/v1/admin/movies",
            &json!({
                "title": "Test Movie",
                "year": 2025,
                "identifiers": { "tmdb": "12345" }
            }),
        )
        .await;
    movie_resp.assert_status(StatusCode::CREATED);
    let movie_id: Uuid = movie_resp.json::<serde_json::Value>()["id"]
        .as_str()
        .unwrap()
        .parse()
        .unwrap();

    // Get the user_id from the DB
    let user_row = sqlx::query_scalar::<_, Uuid>("SELECT id FROM users LIMIT 1")
        .fetch_one(&server.pool)
        .await
        .unwrap();

    // Insert a debrid_stream_session with a valid debrid_url but NO expiry
    let session_id = Uuid::now_v7();
    sqlx::query(
        r#"
        INSERT INTO debrid_stream_sessions
            (id, user_id, media_type, media_id, release_title, info_hash,
             debrid_provider, filename, filesize, debrid_url, debrid_url_expires_at)
        VALUES ($1, $2, 'movie', $3, 'Test.Movie.2025.1080p', 'abc123hash',
                '00000000-0000-0000-0000-000000000001', 'test.mp4', 1000000,
                'https://example.com/valid-stream-url.mp4', NULL)
        "#,
    )
    .bind(session_id)
    .bind(user_row)
    .bind(movie_id)
    .execute(&server.pool)
    .await
    .unwrap();

    // Call stream_play — it should try to use the cached URL (not re-resolve).
    // It will fail to connect to example.com, but importantly it should NOT
    // hit the kobload mock. The 502 is expected because example.com isn't a
    // real stream, but the point is kobload was never called.
    let resp = server
        .admin_client()
        .get(&format!("/api/v1/admin/stream/{session_id}/play"))
        .await;

    // The request will fail (can't connect to example.com) but the key assertion
    // is that kobload's resolve mock was NOT called (expect(0) above).
    assert!(
        resp.status == StatusCode::BAD_GATEWAY || resp.status == StatusCode::OK,
        "expected 502 (unreachable URL) or 200, got {}",
        resp.status
    );
}

/// When kobload returns cached=false with an empty stream_url (torrent
/// still being fetched), stream_play should return 503 not try to proxy "".
#[tokio::test]
async fn test_stream_play_returns_503_for_empty_resolve_url() {
    let mock_server = MockServer::start().await;

    // Mock kobload resolve returning empty URL (torrent not cached)
    Mock::given(method("POST"))
        .and(path_regex("/api/v1/stream/resolve"))
        .and(header("Authorization", "Bearer test-key"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "stream_url": "",
            "filename": "abc123hash",
            "filesize": 0,
            "cached": false
        })))
        .mount(&mock_server)
        .await;

    let server = TestServer::start_api_only().await;

    server
        .admin_client()
        .patch_json(
            "/api/v1/admin/config",
            &json!({
                "kobload_base_url": mock_server.uri(),
                "kobload_api_key": "test-key"
            }),
        )
        .await
        .assert_ok();

    let movie_resp = server
        .admin_client()
        .post_json(
            "/api/v1/admin/movies",
            &json!({
                "title": "Test Movie 2",
                "year": 2025,
                "identifiers": { "tmdb": "99999" }
            }),
        )
        .await;
    movie_resp.assert_status(StatusCode::CREATED);
    let movie_id: Uuid = movie_resp.json::<serde_json::Value>()["id"]
        .as_str()
        .unwrap()
        .parse()
        .unwrap();

    let user_row = sqlx::query_scalar::<_, Uuid>("SELECT id FROM users LIMIT 1")
        .fetch_one(&server.pool)
        .await
        .unwrap();

    // Session with an EXPIRED debrid_url so stream_play must re-resolve
    let session_id = Uuid::now_v7();
    sqlx::query(
        r#"
        INSERT INTO debrid_stream_sessions
            (id, user_id, media_type, media_id, release_title, info_hash,
             debrid_provider, filename, filesize, debrid_url, debrid_url_expires_at)
        VALUES ($1, $2, 'movie', $3, 'Test.Movie.2025.1080p', 'abc123hash',
                '00000000-0000-0000-0000-000000000001', 'test.mp4', 1000000,
                'https://expired.example.com/old.mp4',
                '2020-01-01T00:00:00Z')
        "#,
    )
    .bind(session_id)
    .bind(user_row)
    .bind(movie_id)
    .execute(&server.pool)
    .await
    .unwrap();

    let resp = server
        .admin_client()
        .get(&format!("/api/v1/admin/stream/{session_id}/play"))
        .await;

    assert_eq!(
        resp.status,
        StatusCode::SERVICE_UNAVAILABLE,
        "expected 503 when resolve returns empty URL, got {}",
        resp.status
    );
}

/// DirectPlay sessions should serve the raw file from cache, not remux.
/// Tests that a fully-cached MKV is served directly when the session's
/// decision is DirectPlay (i.e., the background probe no longer downgrades
/// DirectPlay→Remux).
#[tokio::test]
async fn test_stream_play_serves_raw_mkv_for_direct_play_session() {
    let server = TestServer::start_api_only().await;

    let user_row = sqlx::query_scalar::<_, Uuid>("SELECT id FROM users LIMIT 1")
        .fetch_one(&server.pool)
        .await
        .unwrap();

    let movie_resp = server
        .admin_client()
        .post_json(
            "/api/v1/admin/movies",
            &json!({
                "title": "DirectPlay Test Movie",
                "year": 2025,
                "identifiers": { "tmdb": "77777" }
            }),
        )
        .await;
    movie_resp.assert_status(StatusCode::CREATED);
    let movie_id: Uuid = movie_resp.json::<serde_json::Value>()["id"]
        .as_str()
        .unwrap()
        .parse()
        .unwrap();

    // Create a temp file to act as the "cached" MKV
    let cache_dir = tempfile::tempdir().unwrap();
    let cache_file = cache_dir.path().join("test.mkv");
    let file_size: i64 = 10_000;
    {
        let mut f = std::fs::File::create(&cache_file).unwrap();
        f.write_all(&vec![0u8; file_size as usize]).unwrap();
    }

    let session_id = Uuid::now_v7();
    sqlx::query(
        r#"
        INSERT INTO debrid_stream_sessions
            (id, user_id, media_type, media_id, release_title, info_hash,
             debrid_provider, filename, filesize, cache_path,
             debrid_url, debrid_url_expires_at)
        VALUES ($1, $2, 'movie', $3, 'Test.Movie.2025.1080p', 'directplay',
                '00000000-0000-0000-0000-000000000001', 'test.mkv', $4, $5,
                'https://example.com/stream.mkv', NULL)
        "#,
    )
    .bind(session_id)
    .bind(user_row)
    .bind(movie_id)
    .bind(file_size)
    .bind(cache_file.to_str().unwrap())
    .execute(&server.pool)
    .await
    .unwrap();

    // Register an in-memory playback session with DirectPlay decision.
    // In production, stream_resolve creates this; the background probe
    // now preserves DirectPlay instead of downgrading to Remux.
    server.app_state.playback_sessions.insert(
        session_id,
        api::streaming::session::PlaybackSession::test_direct_play(
            session_id,
            user_row,
            cache_file.to_path_buf(),
        ),
    );

    let resp = server
        .admin_client()
        .get(&format!("/api/v1/admin/stream/{session_id}/play"))
        .await;

    // DirectPlay: should serve the raw file (200 OK), NOT remux.
    // If remux were triggered on random bytes, ffmpeg would fail/hang.
    assert_eq!(
        resp.status,
        StatusCode::OK,
        "DirectPlay session should serve raw MKV, got {}",
        resp.status
    );
}

/// Raw-metadata TV stream search without both `season` and `episode` must
/// 400. Show-level imdb_id searches can't return anything instantly playable,
/// and silently downgrading to a movie-style search wastes debrid API calls
/// on results nobody can use.
#[tokio::test]
async fn test_stream_search_tv_without_season_episode_returns_400() {
    let mock_server = MockServer::start().await;

    // Kobload must be configured for the handler to progress, but the 400
    // path short-circuits before any outbound call is made.
    Mock::given(method("POST"))
        .and(path_regex("/api/v1/stream/search"))
        .respond_with(ResponseTemplate::new(500))
        .expect(0)
        .mount(&mock_server)
        .await;

    let server = TestServer::start_api_only().await;

    server
        .admin_client()
        .patch_json(
            "/api/v1/admin/config",
            &json!({
                "kobload_base_url": mock_server.uri(),
                "kobload_api_key": "test-key"
            }),
        )
        .await
        .assert_ok();

    let resp = server
        .admin_client()
        .post_json(
            "/api/v1/admin/stream/search",
            &json!({
                "title": "Some Show",
                "year": 2024,
                "imdb_id": "tt1234567",
                "media_type": "tv"
            }),
        )
        .await;

    resp.assert_status(StatusCode::BAD_REQUEST);
    let body: serde_json::Value = resp.json();
    let err = body["error"].as_str().unwrap_or("");
    assert!(
        err.contains("season") && err.contains("episode"),
        "expected error mentioning season+episode, got: {err}"
    );
}

/// master.m3u8 during probe (file_path empty) must return 503 + Retry-After,
/// NOT 409. hls.js treats 4xx on manifest load as fatal and skips its retry
/// budget; returning 5xx makes it retry, which is what we want.
#[tokio::test]
async fn test_master_playlist_returns_503_not_409_when_probe_pending() {
    let server = TestServer::start_api_only().await;

    let user_row = sqlx::query_scalar::<_, Uuid>("SELECT id FROM users LIMIT 1")
        .fetch_one(&server.pool)
        .await
        .unwrap();

    // Register an in-memory playback session with file_path DELIBERATELY
    // empty — this is the state right after session creation, before the
    // background probe has stamped session state.
    let session_id = Uuid::now_v7();
    let mut session = api::streaming::session::PlaybackSession::test_direct_play(
        session_id,
        user_row,
        std::path::PathBuf::new(),
    );
    session.file_path = std::path::PathBuf::new();
    server
        .app_state
        .playback_sessions
        .insert(session_id, session);

    let resp = server
        .admin_client()
        .get(&format!(
            "/api/v1/admin/playback/sessions/{session_id}/master.m3u8"
        ))
        .await;

    assert_eq!(
        resp.status,
        StatusCode::SERVICE_UNAVAILABLE,
        "pending debrid session must return 503 (retry-friendly), not 409 (hls.js-fatal); got {}",
        resp.status
    );
    assert_eq!(
        resp.headers
            .get("retry-after")
            .and_then(|v| v.to_str().ok()),
        Some("2"),
        "503 must carry a Retry-After header so hls.js waits the right amount",
    );
}

/// master.m3u8 during the POST-probe / PRE-ffmpeg window (file_path set,
/// transcode still None) must return 200 with a valid HLS master playlist.
/// This is the state where the background probe has stamped session fields
/// but the ffmpeg process is still starting up.
#[tokio::test]
async fn test_master_playlist_serves_200_when_file_path_set_but_transcode_pending() {
    let server = TestServer::start_api_only().await;

    let user_row = sqlx::query_scalar::<_, Uuid>("SELECT id FROM users LIMIT 1")
        .fetch_one(&server.pool)
        .await
        .unwrap();

    let tmp_dir = tempfile::tempdir().unwrap();
    let file_path = tmp_dir.path().join("partial.mkv");
    std::fs::write(&file_path, b"not actually video data").unwrap();

    let session_id = Uuid::now_v7();
    let session =
        api::streaming::session::PlaybackSession::test_direct_play(session_id, user_row, file_path);
    // transcode = None, decision defaulted by test_direct_play; the master
    // playlist handler only requires file_path to be non-empty.
    server
        .app_state
        .playback_sessions
        .insert(session_id, session);

    let resp = server
        .admin_client()
        .get(&format!(
            "/api/v1/admin/playback/sessions/{session_id}/master.m3u8"
        ))
        .await;

    assert_eq!(
        resp.status,
        StatusCode::OK,
        "master.m3u8 must serve once file_path is set, got {}",
        resp.status
    );
    let body = String::from_utf8_lossy(resp.bytes());
    assert!(
        body.starts_with("#EXTM3U"),
        "response must be an m3u8 playlist, got: {body:?}"
    );
    assert!(
        body.contains("stream.m3u8"),
        "master must reference the variant playlist, got: {body:?}"
    );
}

/// SSE `probe_complete` must forward the refined `method` field from
/// `probe_data` so the frontend can detect when the probe refines the
/// optimistic initial method (e.g. DirectPlay -> AudioTranscode) and re-init
/// its player. Without this, a raw <video src=...> keeps trying to decode a
/// container the browser can't actually handle and playback fails silently.
#[tokio::test]
async fn test_sse_probe_complete_forwards_method() {
    let server = TestServer::start_api_only().await;

    let user_row = sqlx::query_scalar::<_, Uuid>("SELECT id FROM users LIMIT 1")
        .fetch_one(&server.pool)
        .await
        .unwrap();

    let movie_resp = server
        .admin_client()
        .post_json(
            "/api/v1/admin/movies",
            &json!({
                "title": "Method Refine Test",
                "year": 2026,
                "identifiers": { "tmdb": "77776" }
            }),
        )
        .await;
    movie_resp.assert_status(StatusCode::CREATED);
    let movie_id: Uuid = movie_resp.json::<serde_json::Value>()["id"]
        .as_str()
        .unwrap()
        .parse()
        .unwrap();

    let session_id = Uuid::now_v7();
    let probe_data = json!({
        "duration": 1234.5,
        "method": "audio_transcode",
        "video_codec": "h264",
        "selected_audio_stream_index": 1,
        "audio_streams": []
    });
    sqlx::query(
        r#"
        INSERT INTO debrid_stream_sessions
            (id, user_id, media_type, media_id, release_title, info_hash,
             debrid_provider, filename, filesize, probe_data)
        VALUES ($1, $2, 'movie', $3, 'Refine.Test.2026', 'refhash',
                '00000000-0000-0000-0000-000000000001', 'test.mkv', 1000, $4)
        "#,
    )
    .bind(session_id)
    .bind(user_row)
    .bind(movie_id)
    .bind(&probe_data)
    .execute(&server.pool)
    .await
    .unwrap();

    let url = format!("{}/api/v1/admin/stream/{session_id}/status", server.api_url);
    let resp = reqwest::Client::new()
        .get(&url)
        .header("Authorization", format!("Bearer {}", server.admin_token))
        .send()
        .await
        .expect("SSE request failed");
    assert_eq!(resp.status(), StatusCode::OK);
    let mut body = resp.bytes_stream();

    let mut collected = String::new();
    let window = std::time::Duration::from_secs(5);
    let start = std::time::Instant::now();
    while start.elapsed() < window && !collected.contains("\"type\":\"probe_complete\"") {
        let remaining = window.saturating_sub(start.elapsed());
        match tokio::time::timeout(remaining, futures::StreamExt::next(&mut body)).await {
            Ok(Some(Ok(chunk))) => collected.push_str(&String::from_utf8_lossy(&chunk)),
            Ok(Some(Err(_))) | Ok(None) => break,
            Err(_) => break,
        }
    }

    assert!(
        collected.contains("\"type\":\"probe_complete\""),
        "probe_complete event was never emitted; got:\n{collected}"
    );
    assert!(
        collected.contains("\"method\":\"audio_transcode\""),
        "probe_complete must forward the refined method from probe_data; got:\n{collected}"
    );
}

/// SSE `download_complete` must fire exactly once, ONLY after the on-disk
/// file actually reaches `filesize` — not on every poll tick from the moment
/// the progressive downloader creates the cache file. The previous behaviour
/// spammed the frontend and caused it to hit master.m3u8 before the session
/// was ready, triggering the 409 trap.
#[tokio::test]
async fn test_sse_download_complete_fires_once_on_real_completion() {
    let server = TestServer::start_api_only().await;

    let user_row = sqlx::query_scalar::<_, Uuid>("SELECT id FROM users LIMIT 1")
        .fetch_one(&server.pool)
        .await
        .unwrap();

    let movie_resp = server
        .admin_client()
        .post_json(
            "/api/v1/admin/movies",
            &json!({
                "title": "SSE Test Movie",
                "year": 2026,
                "identifiers": { "tmdb": "88888" }
            }),
        )
        .await;
    movie_resp.assert_status(StatusCode::CREATED);
    let movie_id: Uuid = movie_resp.json::<serde_json::Value>()["id"]
        .as_str()
        .unwrap()
        .parse()
        .unwrap();

    let tmp_dir = tempfile::tempdir().unwrap();
    let cache_file = tmp_dir.path().join("partial.mkv");
    let filesize: i64 = 100;
    // Start with a small/incomplete file — this is the case that USED to
    // fire download_complete every 2s forever.
    std::fs::write(&cache_file, vec![0u8; 10]).unwrap();

    let session_id = Uuid::now_v7();
    sqlx::query(
        r#"
        INSERT INTO debrid_stream_sessions
            (id, user_id, media_type, media_id, release_title, info_hash,
             debrid_provider, filename, filesize, cache_path)
        VALUES ($1, $2, 'movie', $3, 'SSE.Test.2026', 'ssehash',
                '00000000-0000-0000-0000-000000000001', 'test.mkv', $4, $5)
        "#,
    )
    .bind(session_id)
    .bind(user_row)
    .bind(movie_id)
    .bind(filesize)
    .bind(cache_file.to_str().unwrap())
    .execute(&server.pool)
    .await
    .unwrap();

    // Open the SSE stream manually so we can read incrementally.
    let url = format!("{}/api/v1/admin/stream/{session_id}/status", server.api_url);
    let resp = reqwest::Client::new()
        .get(&url)
        .header("Authorization", format!("Bearer {}", server.admin_token))
        .send()
        .await
        .expect("SSE request failed");
    assert_eq!(resp.status(), StatusCode::OK, "SSE endpoint must 200");
    let mut body = resp.bytes_stream();

    // Collect events for ~3s while the file is incomplete — long enough for
    // at least one SSE poll tick (interval = 2s).
    let mut collected = String::new();
    let window = std::time::Duration::from_secs(3);
    let start = std::time::Instant::now();
    while start.elapsed() < window {
        let remaining = window.saturating_sub(start.elapsed());
        match tokio::time::timeout(remaining, futures::StreamExt::next(&mut body)).await {
            Ok(Some(Ok(chunk))) => collected.push_str(&String::from_utf8_lossy(&chunk)),
            Ok(Some(Err(_))) | Ok(None) => break,
            Err(_) => break,
        }
    }

    assert!(
        !collected.contains("\"type\":\"download_complete\""),
        "download_complete MUST NOT fire while file is incomplete (10/100 bytes); \
         got events:\n{collected}"
    );

    // Now grow the file to the full filesize — simulating download finishing.
    std::fs::write(&cache_file, vec![0u8; filesize as usize]).unwrap();

    // Collect for another ~5s (interval=2s, plus slack) and count
    // download_complete events.
    let window = std::time::Duration::from_secs(5);
    let start = std::time::Instant::now();
    let before_len = collected.len();
    while start.elapsed() < window {
        let remaining = window.saturating_sub(start.elapsed());
        match tokio::time::timeout(remaining, futures::StreamExt::next(&mut body)).await {
            Ok(Some(Ok(chunk))) => collected.push_str(&String::from_utf8_lossy(&chunk)),
            Ok(Some(Err(_))) | Ok(None) => break,
            Err(_) => break,
        }
    }

    let after = &collected[before_len..];
    let emissions = after.matches("\"type\":\"download_complete\"").count();
    assert_eq!(
        emissions, 1,
        "download_complete must fire exactly once after real completion; \
         got {emissions} in:\n{after}"
    );
}

/// Resolving a stream must enqueue enrichment for the title, or a streamed
/// entity keeps the thin discover payload it was created from forever — no
/// cast, no crew, no tagline, no content rating. Covers the catch-up case too:
/// the movie already exists (Resume Stream on a detail page) but was created
/// before enrichment was wired, so `enriched_at` is still NULL.
#[tokio::test]
async fn test_stream_resolve_enqueues_enrichment_for_unenriched_movie() {
    let (server, _kobload) = stream_resolve_server().await;

    let movie_id = insert_stream_movie(&server, "Unenriched Movie", false).await;
    resolve_stream_for_movie(&server, movie_id).await;

    assert!(
        enrich_job_exists(&server, movie_id).await,
        "expected a movie_enrich job for a movie with enriched_at IS NULL"
    );
}

/// An already-enriched title must not re-enqueue on every play — enrichment is
/// a provider round-trip, and Resume Stream is pressed a lot.
#[tokio::test]
async fn test_stream_resolve_skips_enrichment_for_enriched_movie() {
    let (server, _kobload) = stream_resolve_server().await;

    let movie_id = insert_stream_movie(&server, "Enriched Movie", true).await;
    resolve_stream_for_movie(&server, movie_id).await;

    assert!(
        !enrich_job_exists(&server, movie_id).await,
        "expected no movie_enrich job for an already-enriched movie"
    );
}

/// Enrichment needs a provider id to resolve against. A title with neither
/// tmdb_id nor imdb_id would only produce a job that fails.
#[tokio::test]
async fn test_stream_resolve_skips_enrichment_without_identifiers() {
    let (server, _kobload) = stream_resolve_server().await;

    let movie_id = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO movies (id, title, year, identifiers, enriched_at)
         VALUES ($1, 'No Identifiers', 2025, '{}'::jsonb, NULL)",
    )
    .bind(movie_id)
    .execute(&server.pool)
    .await
    .unwrap();

    resolve_stream_for_movie(&server, movie_id).await;

    assert!(
        !enrich_job_exists(&server, movie_id).await,
        "expected no movie_enrich job for a movie with no provider identifiers"
    );
}

async fn stream_resolve_server() -> (TestServer, MockServer) {
    let mock_server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path_regex("/api/v1/stream/resolve"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "stream_url": "https://example.invalid/stream.mkv",
            "filename": "stream.mkv",
            "filesize": 1_000_000,
            "cached": true
        })))
        .mount(&mock_server)
        .await;

    let server = TestServer::start_api_only().await;
    server
        .admin_client()
        .patch_json(
            "/api/v1/admin/config",
            &json!({
                "kobload_base_url": mock_server.uri(),
                "kobload_api_key": "test-key"
            }),
        )
        .await
        .assert_ok();
    (server, mock_server)
}

async fn insert_stream_movie(server: &TestServer, title: &str, enriched: bool) -> Uuid {
    let movie_id = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO movies (id, title, year, identifiers, enriched_at)
         VALUES ($1, $2, 2025, jsonb_build_object('tmdb_id', $3::TEXT),
                 CASE WHEN $4 THEN NOW() ELSE NULL END)",
    )
    .bind(movie_id)
    .bind(title)
    .bind(movie_id.simple().to_string())
    .bind(enriched)
    .execute(&server.pool)
    .await
    .unwrap();
    movie_id
}

/// The response is not the subject: session creation probes a URL that does not
/// resolve, so this ends in a 5xx. Enrichment is enqueued before that point.
async fn resolve_stream_for_movie(server: &TestServer, movie_id: Uuid) {
    server
        .admin_client()
        .post_json(
            "/api/v1/admin/stream/resolve",
            &json!({
                "debrid_provider_id": Uuid::nil(),
                "info_hash": format!("hash{}", movie_id.simple()),
                "media_type": "movie",
                "media_id": movie_id,
                "release_title": "Some.Release.2025.1080p",
                "debrid_provider": "test"
            }),
        )
        .await;
}

async fn enrich_job_exists(server: &TestServer, movie_id: Uuid) -> bool {
    sqlx::query_scalar::<_, bool>(
        "SELECT EXISTS(
             SELECT 1 FROM job_status
             WHERE job_type = 'movie_enrich' AND payload->>'movie_id' = $1
         )",
    )
    .bind(movie_id.to_string())
    .fetch_one(&server.pool)
    .await
    .unwrap()
}

/// Reads the stream for `window`, returning what arrived and whether the server closed it.
async fn read_sse(
    body: &mut (impl futures::Stream<Item = reqwest::Result<bytes::Bytes>> + Unpin),
    window: std::time::Duration,
) -> (String, bool) {
    let mut collected = String::new();
    let start = std::time::Instant::now();
    while start.elapsed() < window {
        let remaining = window.saturating_sub(start.elapsed());
        match tokio::time::timeout(remaining, futures::StreamExt::next(body)).await {
            Ok(Some(Ok(chunk))) => collected.push_str(&String::from_utf8_lossy(&chunk)),
            Ok(Some(Err(_))) | Ok(None) => return (collected, true),
            Err(_) => break,
        }
    }
    (collected, false)
}

/// Cross-episode detect runs after the download and after the trickplay sheet is
/// written, so a stream that closed once probe, download and thumbnails were out never
/// carried the credits to the player watching.
#[tokio::test]
async fn test_sse_stays_open_until_processing_has_delivered_its_segments() {
    let server = TestServer::start_api_only().await;
    let user_id = sqlx::query_scalar::<_, Uuid>("SELECT id FROM users LIMIT 1")
        .fetch_one(&server.pool)
        .await
        .unwrap();
    let movie_id: Uuid = server
        .admin_client()
        .post_json(
            "/api/v1/admin/movies",
            &json!({ "title": "SSE Processing Movie", "year": 2026 }),
        )
        .await
        .json::<serde_json::Value>()["id"]
        .as_str()
        .unwrap()
        .parse()
        .unwrap();

    let tmp_dir = tempfile::tempdir().unwrap();
    let cache_file = tmp_dir.path().join("done.mkv");
    std::fs::write(&cache_file, vec![0u8; 100]).unwrap();
    let trickplay = tmp_dir.path().join("media_cache").join("trickplay");
    std::fs::create_dir_all(&trickplay).unwrap();
    std::fs::write(trickplay.join("thumbnails.vtt"), "WEBVTT\n").unwrap();
    let cache_path = cache_file.to_str().unwrap();

    let session_id = Uuid::now_v7();
    sqlx::query(
        r#"
        INSERT INTO debrid_stream_sessions
            (id, user_id, media_type, media_id, release_title, info_hash,
             debrid_provider, filename, filesize, cache_path, probe_data)
        VALUES ($1, $2, 'movie', $3, 'SSE.Processing.2026', 'prochash', 'rd', 'done.mkv', 100,
                $4, '{"duration": 3000.0}')
        "#,
    )
    .bind(session_id)
    .bind(user_id)
    .bind(movie_id)
    .bind(cache_path)
    .execute(&server.pool)
    .await
    .unwrap();
    let job_id = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO job_status (id, job_type, status, payload) VALUES ($1, 'debrid_process', 'running', $2)",
    )
    .bind(job_id)
    .bind(json!({ "session_id": session_id, "cache_path": cache_path }))
    .execute(&server.pool)
    .await
    .unwrap();

    let resp = reqwest::Client::new()
        .get(format!(
            "{}/api/v1/admin/stream/{session_id}/status",
            server.api_url
        ))
        .header("Authorization", format!("Bearer {}", server.admin_token))
        .send()
        .await
        .unwrap();
    let mut body = resp.bytes_stream();
    let (early, closed) = read_sse(&mut body, std::time::Duration::from_secs(5)).await;
    assert!(early.contains("\"type\":\"thumbnails_ready\""), "{early}");
    assert!(early.contains("\"type\":\"download_complete\""), "{early}");
    assert!(
        !closed,
        "the stream closed while processing was running:\n{early}"
    );

    sqlx::query(
        r#"UPDATE debrid_stream_sessions
           SET skip_segments = '[{"segment_type":"credits","start_seconds":2800.0,"end_seconds":3000.0}]',
               lufs = -20.5, downmix_peak_db = -3.5
           WHERE id = $1"#,
    )
    .bind(session_id)
    .execute(&server.pool)
    .await
    .unwrap();
    sqlx::query("UPDATE job_status SET status = 'completed' WHERE id = $1")
        .bind(job_id)
        .execute(&server.pool)
        .await
        .unwrap();

    let (late, closed) = read_sse(&mut body, std::time::Duration::from_secs(6)).await;
    assert!(late.contains("\"type\":\"skip_segments_ready\""), "{late}");
    assert!(late.contains("credits"), "{late}");
    assert!(
        late.contains(r#"{"downmix_peak_db":-3.5,"lufs":-20.5,"type":"loudness"}"#),
        "{late}"
    );
    assert!(late.contains("\"type\":\"done\""), "{late}");
    assert!(closed, "the stream outlived the processing it reports on");
}
