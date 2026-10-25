//! A kobbler wired to a mock kobload and a debrid host stand-in, for tests that drive
//! debrid streams end to end: resolve, play, delete, resolve again.

use crate::common::debrid_upstream::{Upstream, UpstreamPlan};
use crate::common::*;
use reqwest::StatusCode;
use serde_json::{Value, json};
use std::path::PathBuf;
use std::time::{Duration, Instant};
use uuid::Uuid;
use wiremock::matchers::{method, path_regex};
use wiremock::{Mock, MockServer, ResponseTemplate};

pub const PROVIDER_ID: &str = "00000000-0000-0000-0000-000000000001";
pub const MB: usize = 1024 * 1024;
pub const ITA_ENG_FIXTURE: &[u8] = include_bytes!("../../fixtures/audio-lang/ita-eng.mkv");

pub fn noise(len: usize) -> Vec<u8> {
    let mut x: u32 = 0x9e37_79b9;
    (0..len)
        .map(|_| {
            x ^= x << 13;
            x ^= x >> 17;
            x ^= x << 5;
            x as u8
        })
        .collect()
}

pub fn direct_play_caps() -> Value {
    json!({
        "video_codecs": ["h264", "hevc"],
        "audio_codecs": ["aac"],
        "containers": ["mp4", "mkv", "matroska"]
    })
}

pub struct Rig {
    pub server: TestServer,
    pub upstream: Upstream,
    pub cache_dir: tempfile::TempDir,
    pub kobload: MockServer,
    pub filename: String,
}

impl Rig {
    pub async fn start(plan: UpstreamPlan, filename: &str) -> Self {
        let upstream = Upstream::start(plan).await;
        let kobload = MockServer::start().await;
        let server = TestServer::start_api_only().await;
        let cache_dir = tempfile::tempdir().unwrap();
        server
            .admin_client()
            .patch_json(
                "/api/v1/admin/config",
                &json!({
                    "kobload_base_url": kobload.uri(),
                    "kobload_api_key": "test-key",
                    "debrid_cache_dir": cache_dir.path().to_string_lossy(),
                }),
            )
            .await
            .assert_ok();
        let rig = Self {
            server,
            upstream,
            cache_dir,
            kobload,
            filename: filename.to_string(),
        };
        rig.mount_resolve().await;
        rig
    }

    /// kobload resolving any release to this rig's file, and accepting outcome reports.
    pub async fn mount_resolve(&self) {
        Mock::given(method("POST"))
            .and(path_regex("/api/v1/stream/resolve$"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "stream_url": self.upstream.url(&self.filename),
                "filename": self.filename,
                "filesize": self.upstream.size(),
                "cached": true
            })))
            .mount(&self.kobload)
            .await;
        Mock::given(method("POST"))
            .and(path_regex("/api/v1/stream/outcome"))
            .respond_with(ResponseTemplate::new(204))
            .mount(&self.kobload)
            .await;
    }

    /// Answer every stream search with these options.
    pub async fn mount_search(&self, options: Value) {
        Mock::given(method("POST"))
            .and(path_regex("/api/v1/stream/search"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "options": options })))
            .mount(&self.kobload)
            .await;
    }

    /// A cached option for this rig's file, as kobload would rank it first.
    pub fn option(&self, info_hash: &str) -> Value {
        json!({
            "id": format!("opt-{info_hash}"),
            "release_title": "Feed.Movie.2025.1080p",
            "size": 1,
            "indexer": "test",
            "info_hash": info_hash,
            "total_score": 1.0,
            "match_score": 1.0,
            "quality_score": 1.0,
            "debrid_provider": "realdebrid",
            "debrid_provider_id": PROVIDER_ID,
            "cached": true,
            "source_type": "torrent",
            "availability": "ready"
        })
    }

    pub async fn movie(&self) -> Uuid {
        let movie_id = Uuid::now_v7();
        sqlx::query(
            "INSERT INTO movies (id, title, year, identifiers, enriched_at)
             VALUES ($1, 'Feed Movie', 2025, jsonb_build_object('tmdb_id', $2::TEXT), NOW())",
        )
        .bind(movie_id)
        .bind(movie_id.simple().to_string())
        .execute(&self.server.pool)
        .await
        .unwrap();
        movie_id
    }

    pub fn resolve_body(&self, info_hash: &str, movie_id: Uuid, caps: Value) -> Value {
        json!({
            "debrid_provider_id": PROVIDER_ID,
            "info_hash": info_hash,
            "media_type": "movie",
            "media_id": movie_id,
            "release_title": "Feed.Movie.2025",
            "debrid_provider": "realdebrid",
            "capabilities": caps
        })
    }

    pub async fn resolve_with(&self, body: &Value) -> Value {
        let resp = self
            .server
            .admin_client()
            .post_json("/api/v1/admin/stream/resolve", body)
            .await;
        resp.assert_ok();
        resp.json()
    }

    pub async fn resolve(&self, info_hash: &str) -> Uuid {
        let movie_id = self.movie().await;
        session_id_of(
            &self
                .resolve_with(&self.resolve_body(info_hash, movie_id, direct_play_caps()))
                .await,
        )
    }

    pub fn cache_file(&self, info_hash: &str) -> PathBuf {
        self.cache_dir.path().join(info_hash).join(&self.filename)
    }

    pub async fn wait_for_local(&self, info_hash: &str, at_least: u64) {
        let path = self.cache_file(info_hash);
        wait_until(
            Duration::from_secs(10),
            "download never got far enough",
            || std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0) >= at_least,
        )
        .await;
    }

    /// The whole file is on disk and the download has recorded it.
    pub async fn wait_for_complete(&self, info_hash: &str) {
        let total = self.upstream_size() as u64;
        self.wait_for_local(info_hash, total).await;
        let pool = self.server.pool.clone();
        let hash = info_hash.to_string();
        wait_until_async(
            Duration::from_secs(10),
            "cache entry never recorded",
            || {
                let pool = pool.clone();
                let hash = hash.clone();
                async move {
                    sqlx::query_scalar::<_, i64>(
                    "SELECT COUNT(*) FROM debrid_cache_files WHERE file_path LIKE '%' || $1 || '%'",
                )
                .bind(hash)
                .fetch_one(&pool)
                .await
                .unwrap()
                    > 0
                }
            },
        )
        .await;
    }

    pub fn upstream_size(&self) -> usize {
        self.upstream.size()
    }

    pub async fn kobload_requests(&self, path_suffix: &str) -> Vec<Value> {
        self.kobload
            .received_requests()
            .await
            .unwrap_or_default()
            .into_iter()
            .filter(|r| r.url.path().ends_with(path_suffix))
            .map(|r| serde_json::from_slice(&r.body).unwrap_or(Value::Null))
            .collect()
    }

    pub async fn set_config(&self, patch: Value) {
        self.server
            .admin_client()
            .patch_json("/api/v1/admin/config", &patch)
            .await
            .assert_ok();
    }
}

pub fn session_id_of(resp: &Value) -> Uuid {
    resp["session_id"]
        .as_str()
        .unwrap_or_else(|| panic!("no session_id in {resp}"))
        .parse()
        .unwrap()
}

pub async fn wait_until(within: Duration, what: &str, mut done: impl FnMut() -> bool) {
    let deadline = Instant::now() + within;
    while !done() {
        assert!(Instant::now() < deadline, "{what}");
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

pub async fn wait_until_async<F, Fut>(within: Duration, what: &str, mut done: F)
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = bool>,
{
    let deadline = Instant::now() + within;
    while !done().await {
        assert!(Instant::now() < deadline, "{what}");
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

pub struct Played {
    pub status: StatusCode,
    pub content_range: Option<String>,
    pub content_length: Option<u64>,
    pub first_byte_after: Duration,
    pub body: Vec<u8>,
}

pub async fn play(server: &TestServer, session_id: Uuid, range: &str) -> Played {
    use futures::StreamExt;
    let started = Instant::now();
    let resp = reqwest::Client::new()
        .get(format!(
            "{}/api/v1/admin/stream/{session_id}/play",
            server.api_url
        ))
        .bearer_auth(&server.admin_token)
        .header(reqwest::header::RANGE, range)
        .send()
        .await
        .expect("play request");
    let status = resp.status();
    let header = |h| {
        resp.headers()
            .get(h)
            .and_then(|v: &reqwest::header::HeaderValue| v.to_str().ok())
            .map(str::to_string)
    };
    let content_range = header(reqwest::header::CONTENT_RANGE);
    let content_length = header(reqwest::header::CONTENT_LENGTH).and_then(|v| v.parse().ok());
    let mut body = Vec::new();
    let mut first_byte_after = None;
    let mut stream = resp.bytes_stream();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.expect("body chunk");
        if !chunk.is_empty() && first_byte_after.is_none() {
            first_byte_after = Some(started.elapsed());
        }
        body.extend_from_slice(&chunk);
    }
    Played {
        status,
        content_range,
        content_length,
        first_byte_after: first_byte_after.unwrap_or_else(|| started.elapsed()),
        body,
    }
}

pub async fn session_status(server: &TestServer, session_id: Uuid) -> TestResponse {
    server
        .admin_client()
        .get(&format!(
            "/api/v1/admin/playback/sessions/{session_id}/status"
        ))
        .await
}

/// A debrid session that has never been downloaded, read straight off a slow upstream,
/// with the same file behind a library direct-play session of the same id.
pub async fn proxied_mkv_session(
    plan: UpstreamPlan,
) -> (TestServer, Upstream, Uuid, tempfile::TempDir) {
    let total = plan.data.len();
    let data = plan.data.clone();
    let upstream = Upstream::start(plan).await;
    let server = TestServer::start_api_only().await;
    let user_id = sqlx::query_scalar::<_, Uuid>("SELECT id FROM users LIMIT 1")
        .fetch_one(&server.pool)
        .await
        .unwrap();
    let movie_id = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO movies (id, title, year, identifiers) VALUES ($1, 'Cue Movie', 2025, '{}')",
    )
    .bind(movie_id)
    .execute(&server.pool)
    .await
    .unwrap();
    let session_id = Uuid::now_v7();
    sqlx::query(
        r#"INSERT INTO debrid_stream_sessions
            (id, user_id, media_type, media_id, release_title, info_hash,
             debrid_provider, filename, filesize, debrid_url, debrid_url_expires_at)
        VALUES ($1, $2, 'movie', $3, 'Cue.Movie.2025', $4, $5, 'film.mkv', $6, $7, NULL)"#,
    )
    .bind(session_id)
    .bind(user_id)
    .bind(movie_id)
    .bind(format!("cue{}", session_id.simple()))
    .bind(PROVIDER_ID)
    .bind(total as i64)
    .bind(upstream.url("film.mkv"))
    .execute(&server.pool)
    .await
    .unwrap();

    let dir = tempfile::tempdir().unwrap();
    let local = dir.path().join("film.mkv");
    std::fs::write(&local, data.as_slice()).unwrap();
    server.app_state.playback_sessions.insert(
        session_id,
        api::streaming::session::PlaybackSession::test_direct_play(session_id, user_id, local),
    );
    (server, upstream, session_id, dir)
}
