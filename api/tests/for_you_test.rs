//! The For You shelf: TMDB titles related to what each user has watched, minus what they
//! have seen, routed to the library when it holds them.

use crate::common::*;
use api::jobs::handlers::DiscoverSyncHandler;
use api::jobs::worker::JobHandler;
use serde_json::{Value, json};
use std::collections::BTreeSet;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

async fn tmdb_mock(server: &TestServer) -> MockServer {
    let mock = MockServer::start().await;
    sqlx::query(
        "UPDATE video_metadata_providers
         SET config = jsonb_build_object('api_key', 'test-tmdb-key', 'base_url', $1), enabled = true
         WHERE provider_type = 'tmdb'",
    )
    .bind(mock.uri())
    .execute(&server.pool)
    .await
    .unwrap();
    mock
}

async fn mock_related(
    mock: &MockServer,
    media_type: &str,
    seed: &str,
    relation: &str,
    ids: &[i64],
) {
    let title_key = if media_type == "movie" {
        "title"
    } else {
        "name"
    };
    let results: Vec<Value> = ids
        .iter()
        .map(|id| json!({ "id": id, title_key: format!("Title {id}"), "poster_path": format!("/{id}.jpg") }))
        .collect();
    Mock::given(method("GET"))
        .and(path(format!("/{media_type}/{seed}/{relation}")))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "results": results })))
        .mount(mock)
        .await;
}

async fn finish_mock(mock: &MockServer) {
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "results": [] })))
        .mount(mock)
        .await;
}

async fn run_sync(server: &TestServer) {
    DiscoverSyncHandler::new(
        server.pool.clone(),
        server.runtime_config.clone(),
        server.storage_root(),
    )
    .execute(Uuid::now_v7(), json!({}), CancellationToken::new())
    .await
    .unwrap();
}

async fn user_id(server: &TestServer, email: &str) -> Uuid {
    sqlx::query_scalar("SELECT id FROM users WHERE email = $1")
        .bind(email)
        .fetch_one(&server.pool)
        .await
        .unwrap()
}

async fn seed_show(server: &TestServer, title: &str, tmdb_id: i64, rating: Option<&str>) -> Uuid {
    let id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO tv_shows (id, title, content_rating, identifiers)
         VALUES ($1, $2, $3, jsonb_build_object('tmdb_id', $4::text))",
    )
    .bind(id)
    .bind(title)
    .bind(rating)
    .bind(tmdb_id.to_string())
    .execute(&server.pool)
    .await
    .unwrap();
    id
}

async fn watch_episode(server: &TestServer, show_id: Uuid, user_id: Uuid) {
    let season_id = Uuid::new_v4();
    let episode_id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO tv_seasons (id, show_id, season_identifier, season_sort_order, is_monitored, is_special)
         VALUES ($1, $2, '1', 1, true, false)",
    )
    .bind(season_id)
    .bind(show_id)
    .execute(&server.pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO tv_episodes (id, season_id, episode_identifier, episode_sort_order)
         VALUES ($1, $2, '1', 1)",
    )
    .bind(episode_id)
    .bind(season_id)
    .execute(&server.pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO tv_watch_states (episode_id, user_id, watched, watched_at)
         VALUES ($1, $2, true, NOW())",
    )
    .bind(episode_id)
    .bind(user_id)
    .execute(&server.pool)
    .await
    .unwrap();
}

async fn for_you(client: &TestClient) -> Vec<Value> {
    let resp = client.get("/api/v1/admin/discover").await;
    resp.assert_ok();
    resp.json::<Value>()["for_you"].as_array().unwrap().clone()
}

fn ids(items: &[Value]) -> BTreeSet<String> {
    items
        .iter()
        .map(|i| i["provider_id"].as_str().unwrap().to_string())
        .collect()
}

fn find<'a>(items: &'a [Value], id: &str) -> &'a Value {
    items
        .iter()
        .find(|i| i["provider_id"] == id)
        .unwrap_or_else(|| panic!("{id} missing from {items:?}"))
}

#[tokio::test]
async fn shelf_follows_library_and_stremio_history_and_skips_what_was_seen() {
    let server = TestServer::start_api_only().await;
    let mock = tmdb_mock(&server).await;
    let viewer = create_user_client(&server, "viewer@test.com", "viewer").await;
    let newcomer = create_user_client(&server, "new@test.com", "viewer").await;
    let viewer_id = user_id(&server, "viewer@test.com").await;

    let watched = seed_show(&server, "Show A", 100, None).await;
    watch_episode(&server, watched, viewer_id).await;
    seed_show(&server, "Owned Unwatched", 102, None).await;
    let seen_elsewhere = seed_show(&server, "Seen Elsewhere", 103, None).await;
    watch_episode(&server, seen_elsewhere, viewer_id).await;

    let account_id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO stremio_accounts (id, user_id, email, auth_key) VALUES ($1, $2, 's@test.com', 'k')",
    )
    .bind(account_id)
    .bind(viewer_id)
    .execute(&server.pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO stremio_history_items (account_id, stremio_id, media_type, name, tmdb_id, title, last_watched)
         VALUES ($1, 'tt300', 'movie', 'Stremio Film', '300', 'Stremio Film', NOW() - INTERVAL '1 day')",
    )
    .bind(account_id)
    .execute(&server.pool)
    .await
    .unwrap();

    mock_related(
        &mock,
        "tv",
        "100",
        "recommendations",
        &[101, 102, 103, 105, 106],
    )
    .await;
    mock_related(&mock, "tv", "100", "similar", &[104]).await;
    mock_related(&mock, "movie", "300", "recommendations", &[301, 302]).await;
    finish_mock(&mock).await;
    run_sync(&server).await;

    let shelf = for_you(&viewer).await;
    assert_eq!(
        ids(&shelf),
        BTreeSet::from(["101", "102", "104", "105", "106", "301", "302"].map(String::from))
    );
    assert!(find(&shelf, "101")["library_id"].is_null());
    assert!(find(&shelf, "102")["library_id"].is_string());
    assert_eq!(find(&shelf, "101")["because"], json!(["Show A"]));
    assert_eq!(find(&shelf, "301")["because"], json!(["Stremio Film"]));
    assert_eq!(find(&shelf, "301")["media_type"], "movie");

    assert!(for_you(&newcomer).await.is_empty());
}

/// TMDB's related lists carry no rating, so a Kids profile gets only what the Kids rule
/// already allows: library titles it passes, and titles on the Kids Discover shelves.
#[tokio::test]
async fn kids_shelf_keeps_only_titles_the_kids_rule_allows() {
    let server = TestServer::start_api_only().await;
    let mock = tmdb_mock(&server).await;
    create_user_with_role(&server, "kid@test.com", "viewer").await;
    let kid_id = user_id(&server, "kid@test.com").await;
    server
        .admin_client()
        .patch_json(
            &format!("/api/v1/admin/users/{kid_id}"),
            &json!({ "is_kids": true }),
        )
        .await
        .assert_ok();
    let kid = TestClient::new(&server.api_url);
    let resp = kid
        .post_json(
            "/api/v1/admin/login",
            &json!({ "email": "kid@test.com", "password": "testpass123" }),
        )
        .await;
    resp.assert_ok();
    let kid = kid.with_auth(resp.json::<Value>()["token"].as_str().unwrap());

    let watched = seed_show(&server, "Kid Show", 200, Some("TV-Y")).await;
    watch_episode(&server, watched, kid_id).await;
    for id in 210..214 {
        seed_show(&server, &format!("Gentle {id}"), id, Some("TV-G")).await;
    }
    seed_show(&server, "Grim", 220, Some("TV-MA")).await;

    mock_related(
        &mock,
        "tv",
        "200",
        "recommendations",
        &[210, 211, 212, 213, 220, 201, 202, 230],
    )
    .await;
    finish_mock(&mock).await;
    run_sync(&server).await;

    let kids_items: Vec<Value> = [201, 202]
        .iter()
        .map(|id| {
            json!({
                "provider": "tmdb", "provider_id": id.to_string(), "media_type": "tv",
                "title": format!("Title {id}"), "identifiers": { "tmdb_id": id.to_string() }
            })
        })
        .collect();
    api::repositories::metadata_cache::set(
        &server.pool,
        "discover",
        "kids_tv",
        &json!({
            "title": "Kids TV", "section_key": "kids_tv", "sort_order": 1, "kids": true,
            "items": kids_items,
        }),
        time::OffsetDateTime::now_utc() + time::Duration::hours(1),
    )
    .await
    .unwrap();

    assert_eq!(
        ids(&for_you(&kid).await),
        BTreeSet::from(["201", "202", "210", "211", "212", "213"].map(String::from))
    );
}

#[tokio::test]
async fn discover_requires_auth() {
    let server = TestServer::start_api_only().await;
    let resp = TestClient::new(&server.api_url)
        .get("/api/v1/admin/discover")
        .await;
    assert_eq!(resp.status, reqwest::StatusCode::UNAUTHORIZED);
}
