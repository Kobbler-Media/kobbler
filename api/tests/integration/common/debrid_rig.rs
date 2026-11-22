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
pub const LETTERBOXED_FIXTURE: &[u8] = include_bytes!("../../fixtures/picture/letterboxed.mkv");
pub const CHAPTERED_FIXTURE: &[u8] =
    include_bytes!("../../fixtures/audio-lang/ita-eng-chapters.mkv");

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

/// A two-second 64x64 mkv encoded with these codecs.
pub fn encoded_mkv(video: &str, audio: &str) -> Vec<u8> {
    let dir = tempfile::tempdir().unwrap();
    let out = dir.path().join("film.mkv");
    let status = std::process::Command::new("ffmpeg")
        .args(["-v", "error", "-y", "-f", "lavfi", "-i"])
        .arg("testsrc=size=64x64:rate=24:duration=2")
        .args(["-f", "lavfi", "-i", "sine=duration=2"])
        .args([
            "-c:v",
            video,
            "-x265-params",
            "log-level=error",
            "-c:a",
            audio,
        ])
        .arg(&out)
        .status()
        .expect("ffmpeg");
    assert!(status.success());
    std::fs::read(out).unwrap()
}

pub fn mkv_h264_aac_caps() -> Value {
    json!({ "video_codecs": ["h264"], "audio_codecs": ["aac"], "containers": ["mkv", "matroska"] })
}

pub fn direct_play_caps() -> Value {
    json!({
        "video_codecs": ["h264", "hevc"],
        "audio_codecs": ["aac"],
        "containers": ["mp4", "mkv", "matroska"]
    })
}

/// Where a release comes from, and so how a client names it before kobload has resolved it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Source {
    Torrent,
    Usenet,
}

pub const TORRENT_KEY: &str = "0f1e2d3c4b5a69788796a5b4c3d2e1f00f1e2d3c";
pub const USENET_KEY: &str = "nzb-9a8b7c6d5e4f30211203f4e5d6c7b8a99a8b7c6d5e4f30211203f4e5d6c7b8a9";

impl Source {
    pub const ALL: [Source; 2] = [Source::Torrent, Source::Usenet];

    pub fn key(self) -> &'static str {
        match self {
            Source::Torrent => TORRENT_KEY,
            Source::Usenet => USENET_KEY,
        }
    }

    pub fn nzb_ref(self) -> Value {
        match self {
            Source::Torrent => Value::Null,
            Source::Usenet => json!({ "indexer_id": "7", "guid": "guid-usenet-1" }),
        }
    }

    pub fn source_type(self) -> &'static str {
        match self {
            Source::Torrent => "torrent",
            Source::Usenet => "usenet",
        }
    }

    /// What kobload's search lists: an NZB it has not imported yet has no key.
    pub fn listed_key(self) -> Value {
        match self {
            Source::Torrent => json!(TORRENT_KEY),
            Source::Usenet => Value::Null,
        }
    }

    /// How a client names the release in a resolve body.
    pub fn release_fields(self) -> Value {
        json!({ "release_key": self.listed_key(), "nzb_ref": self.nzb_ref() })
    }
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

    /// A rig whose kobload resolves the way it would for `source`: an NZB learns its key
    /// at import, so only kobload's resolve can name it.
    pub async fn start_as(source: Source, plan: UpstreamPlan, filename: &str) -> Self {
        let rig = Self::start(plan, filename).await;
        if source == Source::Usenet {
            rig.mount_resolve_with(json!({ "release_key": USENET_KEY }))
                .await;
        }
        rig
    }

    /// kobload resolving any release to this rig's file, with `extra` over the defaults.
    pub async fn mount_resolve_with(&self, extra: Value) {
        let mut body = self.resolved();
        body.as_object_mut()
            .unwrap()
            .extend(extra.as_object().unwrap().clone());
        Mock::given(method("POST"))
            .and(path_regex("/api/v1/stream/resolve$"))
            .respond_with(ResponseTemplate::new(200).set_body_json(body))
            .with_priority(1)
            .mount(&self.kobload)
            .await;
    }

    fn resolved(&self) -> Value {
        json!({
            "stream_url": self.upstream.url(&self.filename),
            "filename": self.filename,
            "filesize": self.upstream.size(),
            "cached": true
        })
    }

    /// kobload resolving any release to this rig's file, and accepting outcome reports.
    pub async fn mount_resolve(&self) {
        Mock::given(method("POST"))
            .and(path_regex("/api/v1/stream/resolve$"))
            .respond_with(ResponseTemplate::new(200).set_body_json(self.resolved()))
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
    pub fn option(&self, release_key: &str) -> Value {
        json!({
            "id": format!("opt-{release_key}"),
            "release_title": "Feed.Movie.2025.1080p",
            "size": 1,
            "indexer": "test",
            "release_key": release_key,
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

    /// The cached option kobload lists for a release from `source`.
    pub fn option_from(&self, source: Source) -> Value {
        let mut option = self.option(source.key());
        let fields = option.as_object_mut().unwrap();
        fields.insert("id".into(), json!(format!("opt-{}", source.source_type())));
        fields.insert("release_key".into(), source.listed_key());
        fields.insert("source_type".into(), json!(source.source_type()));
        if source == Source::Usenet {
            fields.insert("nzb_ref".into(), source.nzb_ref());
        }
        option
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

    /// A resolve of a release from `source`, named the way the client has it.
    pub fn resolve_body_from(&self, source: Source, media_type: &str, media_id: Uuid) -> Value {
        let mut body = self.resolve_body(source.key(), media_id, direct_play_caps());
        let fields = body.as_object_mut().unwrap();
        fields.extend(source.release_fields().as_object().unwrap().clone());
        fields.insert("media_type".into(), json!(media_type));
        body
    }

    pub fn resolve_body(&self, release_key: &str, movie_id: Uuid, caps: Value) -> Value {
        json!({
            "debrid_provider_id": PROVIDER_ID,
            "release_key": release_key,
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

    pub async fn resolve(&self, release_key: &str) -> Uuid {
        let movie_id = self.movie().await;
        session_id_of(
            &self
                .resolve_with(&self.resolve_body(release_key, movie_id, direct_play_caps()))
                .await,
        )
    }

    /// Download the release to the local cache and end the session that fetched it, so
    /// the next resolve of `release_key` plays it from disk.
    pub async fn put_on_disk(&self, release_key: &str, movie: Uuid) {
        let first = session_id_of(
            &self
                .resolve_with(&self.resolve_body(release_key, movie, direct_play_caps()))
                .await,
        );
        self.wait_for_complete(release_key).await;
        wait_for_probe(self, first).await;
        end_playback(&self.server, first).await;
    }

    pub fn cache_file(&self, release_key: &str) -> PathBuf {
        self.cache_dir.path().join(release_key).join(&self.filename)
    }

    pub async fn wait_for_local(&self, release_key: &str, at_least: u64) {
        let path = self.cache_file(release_key);
        wait_until(
            Duration::from_secs(10),
            "download never got far enough",
            || std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0) >= at_least,
        )
        .await;
    }

    /// The whole file is on disk and the download has recorded it.
    pub async fn wait_for_complete(&self, release_key: &str) {
        let total = self.upstream_size() as u64;
        self.wait_for_local(release_key, total).await;
        let pool = self.server.pool.clone();
        let hash = release_key.to_string();
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

    /// A default movie and TV media root, so a kept stream has somewhere to go.
    pub async fn media_roots(&self) -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        for media_type in ["movie", "tv"] {
            let path = dir.path().join(media_type);
            std::fs::create_dir_all(&path).unwrap();
            sqlx::query(
                "INSERT INTO media_roots (id, path, media_type, is_default) VALUES ($1, $2, $3, TRUE)",
            )
            .bind(Uuid::now_v7())
            .bind(path.to_string_lossy().to_string())
            .bind(media_type)
            .execute(&self.server.pool)
            .await
            .unwrap();
        }
        dir
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
            (id, user_id, media_type, media_id, release_title, release_key,
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

/// Episodes 1..=`count` of one new season.
pub async fn season_of(server: &TestServer, count: i32) -> Vec<Uuid> {
    let show = Uuid::now_v7();
    sqlx::query("INSERT INTO tv_shows (id, title, year) VALUES ($1, 'Ahead Show', 2024)")
        .bind(show)
        .execute(&server.pool)
        .await
        .unwrap();
    let season = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO tv_seasons (id, show_id, season_identifier, season_sort_order) VALUES ($1, $2, '1', 1)",
    )
    .bind(season)
    .bind(show)
    .execute(&server.pool)
    .await
    .unwrap();
    let mut episodes = Vec::new();
    for n in 1..=count {
        let id = Uuid::now_v7();
        sqlx::query(
            "INSERT INTO tv_episodes (id, season_id, episode_identifier, episode_sort_order) VALUES ($1, $2, $3, $4)",
        )
        .bind(id)
        .bind(season)
        .bind(n.to_string())
        .bind(n)
        .execute(&server.pool)
        .await
        .unwrap();
        episodes.push(id);
    }
    episodes
}

/// A release of `media_id` downloaded to `path` by nobody in particular: nothing has
/// processed it yet.
pub async fn downloaded_session(
    server: &TestServer,
    media_type: &str,
    media_id: Uuid,
    release_key: &str,
    path: &std::path::Path,
) -> Uuid {
    let session = Uuid::now_v7();
    let size = std::fs::metadata(path).map(|m| m.len() as i64).unwrap_or(0);
    sqlx::query(
        "INSERT INTO debrid_stream_sessions
            (id, user_id, media_type, media_id, release_title, release_key, debrid_provider,
             filename, filesize, cache_path)
         SELECT $1, id, $2, $3, 'Ahead.Release', $4, 'realdebrid', $5, $6, $7
         FROM users WHERE role = 'admin' LIMIT 1",
    )
    .bind(session)
    .bind(media_type)
    .bind(media_id)
    .bind(release_key)
    .bind(path.file_name().unwrap().to_string_lossy())
    .bind(size)
    .bind(path.to_string_lossy())
    .execute(&server.pool)
    .await
    .unwrap();
    session
}

pub async fn process_download(server: &TestServer, session: Uuid, path: &std::path::Path) {
    use api::jobs::worker::JobHandler;
    api::jobs::handlers::DebridProcessHandler::new(
        server.pool.clone(),
        server.nats_publisher.clone(),
        server.runtime_config.clone(),
        server.app_state.gpu_lock.clone(),
        server.storage_root(),
    )
    .execute(
        Uuid::now_v7(),
        json!({ "session_id": session, "cache_path": path.to_string_lossy() }),
        tokio_util::sync::CancellationToken::new(),
    )
    .await
    .unwrap();
}

pub async fn end_playback(server: &TestServer, session_id: Uuid) {
    server
        .admin_client()
        .delete(&format!("/api/v1/admin/playback/sessions/{session_id}"))
        .await;
}

pub async fn wait_for_probe(rig: &Rig, session_id: Uuid) {
    let server = &rig.server;
    wait_until_async(
        Duration::from_secs(10),
        "probe never finished",
        || async move {
            let status: Value = session_status(server, session_id).await.json();
            status["state"] != "preparing"
        },
    )
    .await;
}

/// The stored decision's method and whether it deinterlaces.
pub fn stored_decision(rig: &Rig, session_id: Uuid) -> (String, bool) {
    let session = rig
        .server
        .app_state
        .playback_sessions
        .get(&session_id)
        .expect("session");
    (
        serde_json::to_value(session.decision.method)
            .unwrap()
            .as_str()
            .unwrap()
            .to_string(),
        session.decision.deinterlace,
    )
}

pub fn stored_h264_level(rig: &Rig, session_id: Uuid) -> Option<u32> {
    rig.server
        .app_state
        .playback_sessions
        .get(&session_id)
        .expect("session")
        .decision
        .h264_level
}
