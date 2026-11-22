//! A file's only audio track, left untagged, is presumed to be in the title's original
//! language, so a foreign film still switches its subtitles on.

use crate::common::debrid_rig::*;
use crate::common::debrid_upstream::UpstreamPlan;
use crate::common::*;
use crate::subtitle_autoselect_test::{SUB_STREAM, seed_file};
use reqwest::StatusCode;
use serde_json::{Value, json};
use sqlx::PgPool;
use std::time::{Duration, Instant};
use uuid::Uuid;
use wiremock::matchers::{method, path_regex};
use wiremock::{Mock, MockServer, ResponseTemplate};

const UNTAGGED_MKV: &[u8] = include_bytes!("../fixtures/playback/interlaced.mkv");

async fn tmdb(pool: &PgPool) -> MockServer {
    let tmdb = MockServer::start().await;
    sqlx::query(
        "UPDATE video_metadata_providers
         SET config = jsonb_build_object('api_key', 'k', 'base_url', $1), enabled = true
         WHERE provider_type = 'tmdb'",
    )
    .bind(tmdb.uri())
    .execute(pool)
    .await
    .unwrap();
    tmdb
}

async fn originally(tmdb: &MockServer, path: &str, language: &str, delay: Duration) {
    Mock::given(method("GET"))
        .and(path_regex(path))
        .respond_with(
            ResponseTemplate::new(200)
                .set_delay(delay)
                .set_body_json(json!({
                    "id": 1, "title": "Original", "name": "Original", "genres": [],
                    "original_language": language
                })),
        )
        .mount(tmdb)
        .await;
}

async fn lookups(tmdb: &MockServer, path: &str) -> usize {
    tmdb.received_requests()
        .await
        .unwrap()
        .iter()
        .filter(|r| r.url.path() == path)
        .count()
}

async fn movie_with_tmdb(server: &TestServer, tmdb_id: &str, file_name: &str) -> Uuid {
    let file_id = seed_file(server, None, file_name).await;
    sqlx::query(
        "UPDATE movies SET identifiers = jsonb_build_object('tmdb_id', $2::TEXT)
         WHERE id = (SELECT movie_id FROM movie_files WHERE id = $1)",
    )
    .bind(file_id)
    .bind(tmdb_id)
    .execute(&server.pool)
    .await
    .unwrap();
    file_id
}

async fn episode_with_tmdb(server: &TestServer, tmdb_id: &str) -> Uuid {
    let (show, season, episode) = (Uuid::now_v7(), Uuid::now_v7(), Uuid::now_v7());
    sqlx::query(
        "WITH s AS (
             INSERT INTO tv_shows (id, title, year, identifiers)
             VALUES ($1, 'Show', 2024, jsonb_build_object('tmdb_id', $4::TEXT))
         ), se AS (
             INSERT INTO tv_seasons (id, show_id, season_identifier, season_sort_order, is_special)
             VALUES ($2, $1, '1', 1, false)
         )
         INSERT INTO tv_episodes (id, season_id, episode_identifier, episode_sort_order, title)
         VALUES ($3, $2, '1', 1, 'Pilot')",
    )
    .bind(show)
    .bind(season)
    .bind(episode)
    .bind(tmdb_id)
    .execute(&server.pool)
    .await
    .unwrap();
    let file_id = create_stub_episode_file(&server.pool, episode).await;
    let path = std::path::Path::new(&server.storage_root()).join(format!("{episode}.mkv"));
    std::fs::write(&path, UNTAGGED_MKV).unwrap();
    sqlx::query("UPDATE tv_episode_files SET file_path = $2 WHERE id = $1")
        .bind(file_id)
        .bind(path.to_string_lossy())
        .execute(&server.pool)
        .await
        .unwrap();
    file_id
}

async fn play_file(server: &TestServer, media_type: &str, file_id: Uuid) -> Value {
    let resp = server
        .admin_client()
        .post_json(
            "/api/v1/admin/playback/sessions",
            &json!({
                "media_type": media_type,
                "file_id": file_id.to_string(),
                "preferred_audio_language": "english",
                "preferred_subtitle_language": "english",
                "capabilities": {
                    "video_codecs": ["h264"],
                    "audio_codecs": ["aac"],
                    "containers": ["mkv", "matroska", "webm"]
                }
            }),
        )
        .await;
    resp.assert_ok();
    resp.json()
}

fn assert_audio(session: &Value, language: Option<&str>, name: Option<&str>) {
    let tracks = session["audio_tracks"].as_array().unwrap();
    assert_eq!(tracks.len(), 1, "{session}");
    assert_eq!(tracks[0]["language"], json!(language), "{session}");
    assert_eq!(tracks[0]["language_name"], json!(name), "{session}");
}

#[tokio::test]
async fn a_library_file_with_one_untagged_track_plays_in_the_original_language() {
    let server = TestServer::start_api_only().await;
    let tmdb = tmdb(&server.pool).await;
    originally(&tmdb, "^/movie/10[012]$", "ko", Duration::ZERO).await;
    originally(&tmdb, "^/movie/200$", "en", Duration::ZERO).await;
    originally(&tmdb, "^/movie/300$", "ko", Duration::from_secs(20)).await;

    let cases = [
        ("100", "film.mkv", Some(("kor", "Korean")), Some(SUB_STREAM)),
        ("101", "Film.2019.DUBBED.1080p.mkv", None, None),
        ("200", "film.mkv", Some(("eng", "English")), None),
        ("404", "film.mkv", None, None),
    ];
    for (tmdb_id, file_name, presumed, subtitle) in cases {
        let file_id = movie_with_tmdb(&server, tmdb_id, file_name).await;
        let session = play_file(&server, "movie", file_id).await;
        assert_audio(&session, presumed.map(|p| p.0), presumed.map(|p| p.1));
        assert_eq!(
            session["selected_subtitle_index"],
            json!(subtitle),
            "tmdb {tmdb_id}, {file_name}: {session}"
        );
    }
    assert_eq!(
        lookups(&tmdb, "/movie/101").await,
        0,
        "a dubbed release asks nothing"
    );

    let file_id = movie_with_tmdb(&server, "102", "film.mkv").await;
    for _ in 0..2 {
        play_file(&server, "movie", file_id).await;
    }
    assert_eq!(
        lookups(&tmdb, "/movie/102").await,
        1,
        "asked once, then stored"
    );
    let stored: Option<String> = sqlx::query_scalar(
        "SELECT original_language FROM movies
         WHERE id = (SELECT movie_id FROM movie_files WHERE id = $1)",
    )
    .bind(file_id)
    .fetch_one(&server.pool)
    .await
    .unwrap();
    assert_eq!(stored.as_deref(), Some("ko"));

    let slow = movie_with_tmdb(&server, "300", "film.mkv").await;
    let started = Instant::now();
    let session = play_file(&server, "movie", slow).await;
    assert!(
        started.elapsed() < Duration::from_secs(8),
        "playback waited on TMDB"
    );
    assert_audio(&session, None, None);
}

#[tokio::test]
async fn an_episode_is_presumed_to_be_in_its_shows_original_language() {
    let server = TestServer::start_api_only().await;
    let tmdb = tmdb(&server.pool).await;
    originally(&tmdb, "^/tv/500$", "ja", Duration::ZERO).await;

    let file_id = episode_with_tmdb(&server, "500").await;
    for _ in 0..2 {
        let session = play_file(&server, "episode", file_id).await;
        assert_audio(&session, Some("jpn"), Some("Japanese"));
    }
    assert_eq!(lookups(&tmdb, "/tv/500").await, 1);
}

async fn probed_audio(rig: &Rig, session_id: Uuid) -> Value {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let probe: Option<Value> =
            sqlx::query_scalar("SELECT probe_data FROM debrid_stream_sessions WHERE id = $1")
                .bind(session_id)
                .fetch_one(&rig.server.pool)
                .await
                .unwrap();
        if let Some(probe) = probe {
            return probe["audio_streams"].clone();
        }
        assert!(Instant::now() < deadline, "never probed");
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

#[tokio::test]
async fn a_debrid_release_with_one_untagged_track_plays_in_the_original_language() {
    let rig = Rig::start(UpstreamPlan::new(UNTAGGED_MKV.to_vec()), "film.mkv").await;
    let tmdb = tmdb(&rig.server.pool).await;
    originally(&tmdb, "^/movie/", "ko", Duration::ZERO).await;
    let movie = rig.movie().await;

    let body = rig.resolve_body("presumedhash", movie, direct_play_caps());
    let remote = session_id_of(&rig.resolve_with(&body).await);
    assert_eq!(probed_audio(&rig, remote).await[0]["language"], "kor");
    assert_audio(&rig.resolve_with(&body).await, Some("kor"), Some("Korean"));

    rig.wait_for_complete("presumedhash").await;
    rig.server
        .admin_client()
        .delete(&format!("/api/v1/admin/stream/{remote}"))
        .await
        .assert_status(StatusCode::NO_CONTENT);
    assert_audio(&rig.resolve_with(&body).await, Some("kor"), Some("Korean"));

    let mut dubbed = rig.resolve_body("dubbedhash", movie, direct_play_caps());
    dubbed["release_title"] = json!("Feed.Movie.2025.Dual.Audio.1080p");
    let dubbed = session_id_of(&rig.resolve_with(&dubbed).await);
    assert_eq!(probed_audio(&rig, dubbed).await[0]["language"], Value::Null);

    let discovered = json!({
        "debrid_provider_id": PROVIDER_ID,
        "release_key": "discoveredhash",
        "media_type": "movie",
        "media_id": Uuid::nil(),
        "release_title": "Discovered.2024",
        "debrid_provider": "realdebrid",
        "capabilities": direct_play_caps(),
        "auto_create": { "title": "Discovered", "year": 2024, "identifiers": { "tmdb_id": 4242 } }
    });
    let discovered = session_id_of(&rig.resolve_with(&discovered).await);
    assert_eq!(probed_audio(&rig, discovered).await[0]["language"], "kor");
}

#[tokio::test]
async fn enriching_a_title_records_its_original_language() {
    let server = TestServer::start_api_only().await;
    let tmdb = tmdb(&server.pool).await;
    originally(&tmdb, "^/movie/100$", "ko", Duration::ZERO).await;
    originally(&tmdb, "^/tv/500$", "ja", Duration::ZERO).await;

    let (movie, show) = (Uuid::now_v7(), Uuid::now_v7());
    sqlx::query(
        "WITH m AS (
             INSERT INTO movies (id, title, year, identifiers)
             VALUES ($1, 'Film', 2024, '{\"tmdb_id\": \"100\"}')
         )
         INSERT INTO tv_shows (id, title, year, identifiers)
         VALUES ($2, 'Show', 2024, '{\"tmdb_id\": \"500\"}')",
    )
    .bind(movie)
    .bind(show)
    .execute(&server.pool)
    .await
    .unwrap();

    api::jobs::handlers::movie_enrich::enrich_movie(&server.pool, movie, false)
        .await
        .unwrap();
    api::jobs::handlers::tv_show_enrich::enrich_tv_show(&server.pool, show, false)
        .await
        .unwrap();

    let stored: (Option<String>, Option<String>) = sqlx::query_as(
        "SELECT (SELECT original_language FROM movies WHERE id = $1),
                (SELECT original_language FROM tv_shows WHERE id = $2)",
    )
    .bind(movie)
    .bind(show)
    .fetch_one(&server.pool)
    .await
    .unwrap();
    assert_eq!(stored, (Some("ko".into()), Some("ja".into())));
}
