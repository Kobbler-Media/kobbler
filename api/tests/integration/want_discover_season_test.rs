//! `POST /api/v1/admin/discover/want-season` — the season sibling of
//! `want-episode`. It must report what is already present rather than silently
//! re-queueing a season the household mostly has.

use crate::common::*;
use api::repositories::tv_episode_files;
use reqwest::StatusCode;
use serde_json::{Value, json};
use sqlx::PgPool;
use uuid::Uuid;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

// =============================================================================
// Fixtures
// =============================================================================

/// A four-episode TMDB season, one of which TVMaze would give a time for.
fn season_payload(episodes: u32) -> Value {
    let eps: Vec<Value> = (1..=episodes)
        .map(|n| {
            json!({
                "id": 1000 + n,
                "episode_number": n,
                "name": format!("Episode {n}"),
                "overview": "",
                "air_date": "2026-09-04",
                "runtime": 42,
                "still_path": null,
                "guest_stars": [],
                "crew": []
            })
        })
        .collect();
    json!({
        "season_number": 24,
        "name": "Season 24",
        "air_date": "2026-09-04",
        "overview": "",
        "poster_path": null,
        "episodes": eps
    })
}

async fn configure_tmdb(server: &TestServer, mock: &MockServer) {
    sqlx::query(
        r#"
        UPDATE video_metadata_providers
        SET config = jsonb_build_object('api_key', 'test-tmdb-key', 'base_url', $1),
            enabled = true
        WHERE provider_type = 'tmdb'
        "#,
    )
    .bind(mock.uri())
    .execute(&server.pool)
    .await
    .expect("configure tmdb");
}

async fn mount_season(mock: &MockServer, episodes: u32) {
    Mock::given(method("GET"))
        .and(path("/tv/9001/season/24"))
        .respond_with(ResponseTemplate::new(200).set_body_json(season_payload(episodes)))
        .mount(mock)
        .await;
}

fn want_season_body() -> Value {
    json!({
        "provider": "tmdb",
        "provider_id": "9001",
        "season_number": 24,
        "title": "Season Want Show",
        "identifiers": { "tmdb_id": "9001" },
    })
}

async fn give_episode_a_file(pool: &PgPool, episode_id: Uuid) {
    let file_id = tv_episode_files::create(
        pool,
        tv_episode_files::EpisodeFileInput {
            file_path: format!("/test/episodes/{episode_id}.mkv"),
            file_size: Some(1024),
            file_fingerprint: None,
            format: "mkv".to_string(),
            quality: None,
            languages: None,
            subtitles: None,
            source_type: Some("test".to_string()),
            source_metadata: Some(json!({})),
            video_codec: None,
            video_width: None,
            video_height: None,
            video_bitrate_kbps: None,
            framerate: None,
            bit_depth: None,
            is_hdr: false,
            is_interlaced: false,
            hdr_format: None,
            duration_seconds: None,
            container: None,
            overall_bitrate_kbps: None,
        },
    )
    .await
    .expect("create file");
    tv_episode_files::link_episodes(pool, file_id, &[episode_id])
        .await
        .expect("link file");
}

async fn episode_ids(pool: &PgPool, season_id: Uuid) -> Vec<Uuid> {
    sqlx::query_scalar(
        "SELECT id FROM tv_episodes WHERE season_id = $1 ORDER BY episode_sort_order",
    )
    .bind(season_id)
    .fetch_all(pool)
    .await
    .expect("list episodes")
}

async fn want_statuses(pool: &PgPool, season_id: Uuid) -> Vec<String> {
    sqlx::query_scalar(
        "SELECT want_status FROM tv_episodes WHERE season_id = $1 ORDER BY episode_sort_order",
    )
    .bind(season_id)
    .fetch_all(pool)
    .await
    .expect("list want statuses")
}

// =============================================================================
// Tests
// =============================================================================

#[tokio::test]
async fn a_brand_new_season_wants_every_episode() {
    let server = TestServer::start_api_only().await;
    let mock = MockServer::start().await;
    configure_tmdb(&server, &mock).await;
    mount_season(&mock, 4).await;

    let resp = server
        .admin_client()
        .post_json("/api/v1/admin/discover/want-season", &want_season_body())
        .await;
    resp.assert_ok();

    let body: Value = resp.json();
    assert_eq!(body["season_number"], 24);
    assert_eq!(body["total_episodes"], 4);
    assert_eq!(body["already_have"], 0);
    assert_eq!(body["already_wanted"], 0);
    assert_eq!(body["newly_wanted"], 4);
    assert_eq!(body["queued"], true);
    assert_eq!(body["newly_wanted_episodes"].as_array().unwrap().len(), 4);

    let season_id: Uuid = body["season_id"].as_str().unwrap().parse().unwrap();
    assert_eq!(
        want_statuses(&server.pool, season_id).await,
        vec!["wanted"; 4]
    );
}

#[tokio::test]
async fn already_present_episodes_are_counted_not_requeued() {
    let server = TestServer::start_api_only().await;
    let mock = MockServer::start().await;
    configure_tmdb(&server, &mock).await;
    mount_season(&mock, 4).await;

    // First call creates the show, the season and all four episodes.
    let first: Value = server
        .admin_client()
        .post_json("/api/v1/admin/discover/want-season", &want_season_body())
        .await
        .json();
    let season_id: Uuid = first["season_id"].as_str().unwrap().parse().unwrap();

    // Two of them arrive; reset the other two so they look untouched.
    let ids = episode_ids(&server.pool, season_id).await;
    give_episode_a_file(&server.pool, ids[0]).await;
    give_episode_a_file(&server.pool, ids[1]).await;
    sqlx::query("UPDATE tv_episodes SET want_status = 'none' WHERE season_id = $1")
        .bind(season_id)
        .execute(&server.pool)
        .await
        .unwrap();

    let resp = server
        .admin_client()
        .post_json("/api/v1/admin/discover/want-season", &want_season_body())
        .await;
    resp.assert_ok();

    let body: Value = resp.json();
    assert_eq!(body["total_episodes"], 4);
    assert_eq!(body["already_have"], 2, "\"you've already got 2 of those\"");
    assert_eq!(body["newly_wanted"], 2, "\"grabbing the other 2\"");
    assert_eq!(body["already_wanted"], 0);
    assert_eq!(body["queued"], true);

    let newly: Vec<i64> = body["newly_wanted_episodes"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| e["episode_number"].as_i64().unwrap())
        .collect();
    assert_eq!(newly, vec![3, 4], "only the missing episodes are named");

    let statuses = want_statuses(&server.pool, season_id).await;
    assert_eq!(statuses, vec!["none", "none", "wanted", "wanted"]);
}

#[tokio::test]
async fn already_wanted_episodes_are_not_requeued() {
    let server = TestServer::start_api_only().await;
    let mock = MockServer::start().await;
    configure_tmdb(&server, &mock).await;
    mount_season(&mock, 4).await;

    let first: Value = server
        .admin_client()
        .post_json("/api/v1/admin/discover/want-season", &want_season_body())
        .await
        .json();
    let season_id: Uuid = first["season_id"].as_str().unwrap().parse().unwrap();

    // Everything is already wanted from the first call.
    let resp = server
        .admin_client()
        .post_json("/api/v1/admin/discover/want-season", &want_season_body())
        .await;
    resp.assert_ok();

    let body: Value = resp.json();
    assert_eq!(body["already_wanted"], 4);
    assert_eq!(body["newly_wanted"], 0);
    assert_eq!(
        body["queued"], false,
        "a season already in flight must not be re-queued"
    );
    assert!(body["newly_wanted_episodes"].as_array().unwrap().is_empty());

    let _ = season_id;
}

#[tokio::test]
async fn a_fully_present_season_queues_nothing() {
    let server = TestServer::start_api_only().await;
    let mock = MockServer::start().await;
    configure_tmdb(&server, &mock).await;
    mount_season(&mock, 3).await;

    let first: Value = server
        .admin_client()
        .post_json("/api/v1/admin/discover/want-season", &want_season_body())
        .await
        .json();
    let season_id: Uuid = first["season_id"].as_str().unwrap().parse().unwrap();

    for id in episode_ids(&server.pool, season_id).await {
        give_episode_a_file(&server.pool, id).await;
    }
    sqlx::query("UPDATE tv_episodes SET want_status = 'none' WHERE season_id = $1")
        .bind(season_id)
        .execute(&server.pool)
        .await
        .unwrap();

    let body: Value = server
        .admin_client()
        .post_json("/api/v1/admin/discover/want-season", &want_season_body())
        .await
        .json();

    assert_eq!(body["already_have"], 3);
    assert_eq!(body["newly_wanted"], 0);
    assert_eq!(body["queued"], false);
    assert_eq!(
        want_statuses(&server.pool, season_id).await,
        vec!["none"; 3]
    );
}

#[tokio::test]
async fn a_season_the_provider_does_not_know_is_404() {
    let server = TestServer::start_api_only().await;
    let mock = MockServer::start().await;
    configure_tmdb(&server, &mock).await;

    Mock::given(method("GET"))
        .and(path("/tv/9001/season/24"))
        .respond_with(ResponseTemplate::new(404))
        .mount(&mock)
        .await;

    let resp = server
        .admin_client()
        .post_json("/api/v1/admin/discover/want-season", &want_season_body())
        .await;

    resp.assert_status(StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn requires_authentication() {
    let server = TestServer::start_api_only().await;

    let resp = TestClient::new(&server.api_url)
        .post_json("/api/v1/admin/discover/want-season", &want_season_body())
        .await;

    assert_eq!(resp.status, StatusCode::UNAUTHORIZED);
}

/// The body has to be valid or axum 422s on deserialisation before the role
/// check inside the handler ever runs.
#[tokio::test]
async fn a_viewer_is_forbidden_with_a_valid_body() {
    let server = TestServer::start_api_only().await;
    let viewer = create_user_client(&server, "viewer-season@test.com", "viewer").await;

    let resp = viewer
        .post_json("/api/v1/admin/discover/want-season", &want_season_body())
        .await;

    resp.assert_status(StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn a_contributor_may_want_a_season() {
    let server = TestServer::start_api_only().await;
    let mock = MockServer::start().await;
    configure_tmdb(&server, &mock).await;
    mount_season(&mock, 2).await;

    let contrib = create_user_client(&server, "contrib-season@test.com", "contributor").await;
    let resp = contrib
        .post_json("/api/v1/admin/discover/want-season", &want_season_body())
        .await;

    resp.assert_ok();
    let body: Value = resp.json();
    assert_eq!(body["newly_wanted"], 2);
}
