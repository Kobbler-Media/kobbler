//! The watch history list and its bulk edits: the tool for cleaning someone else's
//! imported history out of a profile.

use crate::common::*;
use reqwest::StatusCode;
use serde_json::{Value, json};
use uuid::Uuid;

async fn seed_movie(server: &TestServer, title: &str, tmdb_id: i64) -> Uuid {
    let id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO movies (id, title, year, identifiers)
         VALUES ($1, $2, 2001, jsonb_build_object('tmdb_id', $3::text))",
    )
    .bind(id)
    .bind(title)
    .bind(tmdb_id.to_string())
    .execute(&server.pool)
    .await
    .unwrap();
    id
}

async fn seed_show_with_episodes(
    server: &TestServer,
    title: &str,
    tmdb_id: i64,
) -> (Uuid, Vec<Uuid>) {
    let show_id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO tv_shows (id, title, year, identifiers)
         VALUES ($1, $2, 2010, jsonb_build_object('tmdb_id', $3::text))",
    )
    .bind(show_id)
    .bind(title)
    .bind(tmdb_id.to_string())
    .execute(&server.pool)
    .await
    .unwrap();
    let season_id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO tv_seasons (id, show_id, season_identifier, season_sort_order, is_monitored, is_special)
         VALUES ($1, $2, '1', 1, true, false)",
    )
    .bind(season_id)
    .bind(show_id)
    .execute(&server.pool)
    .await
    .unwrap();
    let mut episodes = Vec::new();
    for n in 1..=2 {
        let id = Uuid::new_v4();
        sqlx::query(
            "INSERT INTO tv_episodes (id, season_id, episode_identifier, episode_sort_order)
             VALUES ($1, $2, $3, $3::int)",
        )
        .bind(id)
        .bind(season_id)
        .bind(n.to_string())
        .execute(&server.pool)
        .await
        .unwrap();
        episodes.push(id);
    }
    (show_id, episodes)
}

async fn stremio_item(
    server: &TestServer,
    user_id: Uuid,
    title: &str,
    tmdb_id: i64,
    days_ago: i32,
) {
    let account_id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO stremio_accounts (id, user_id, email, auth_key) VALUES ($1, $2, 's@test.com', 'k')",
    )
    .bind(account_id)
    .bind(user_id)
    .execute(&server.pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO stremio_history_items (account_id, stremio_id, media_type, name, tmdb_id, title, year, poster_url, last_watched)
         VALUES ($1, $2, 'movie', $3, $4, $3, 1999, 'https://img.test/p.jpg', NOW() - make_interval(days => $5))",
    )
    .bind(account_id)
    .bind(format!("tt{tmdb_id}"))
    .bind(title)
    .bind(tmdb_id.to_string())
    .bind(days_ago)
    .execute(&server.pool)
    .await
    .unwrap();
}

async fn backdate_movie(server: &TestServer, movie_id: Uuid, days_ago: i32) {
    sqlx::query(
        "UPDATE movie_watch_states SET watched_at = NOW() - make_interval(days => $2) WHERE movie_id = $1",
    )
    .bind(movie_id)
    .bind(days_ago)
    .execute(&server.pool)
    .await
    .unwrap();
}

async fn backdate_episode(server: &TestServer, episode_id: Uuid, days_ago: i32) {
    sqlx::query(
        "UPDATE tv_watch_states SET watched_at = NOW() - make_interval(days => $2) WHERE episode_id = $1",
    )
    .bind(episode_id)
    .bind(days_ago)
    .execute(&server.pool)
    .await
    .unwrap();
}

async fn history(client: &TestClient, query: &str) -> Value {
    let resp = client
        .get(&format!("/api/v1/admin/watch-history{query}"))
        .await;
    resp.assert_ok();
    resp.json()
}

fn titles(page: &Value) -> Vec<String> {
    page["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|i| i["title"].as_str().unwrap().to_string())
        .collect()
}

struct Fixture {
    server: TestServer,
    viewer: TestClient,
    other: TestClient,
    movie_id: Uuid,
    show_id: Uuid,
    episodes: Vec<Uuid>,
}

/// A local film watched 1 day ago, a show with two episodes watched 3 days ago, a
/// Stremio film 5 days ago, and someone else's film that must never show up.
async fn fixture() -> Fixture {
    let server = TestServer::start_api_only().await;
    let viewer = create_user_client(&server, "viewer@test.com", "viewer").await;
    let viewer_id: Uuid =
        sqlx::query_scalar("SELECT id FROM users WHERE email = 'viewer@test.com'")
            .fetch_one(&server.pool)
            .await
            .unwrap();
    let other = create_user_client(&server, "other@test.com", "viewer").await;

    let movie_id = seed_movie(&server, "Local Film", 500).await;
    viewer
        .put_json(
            &format!("/api/v1/admin/movies/{movie_id}/watch-state"),
            &json!({ "watched": true }),
        )
        .await
        .assert_ok();
    backdate_movie(&server, movie_id, 1).await;

    let (show_id, episodes) = seed_show_with_episodes(&server, "Local Show", 600).await;
    for ep in &episodes {
        viewer
            .put_json(
                &format!("/api/v1/admin/tv/episodes/{ep}/watch-state"),
                &json!({ "watched": true }),
            )
            .await
            .assert_ok();
        backdate_episode(&server, *ep, 3).await;
    }

    stremio_item(&server, viewer_id, "Stremio Film", 700, 5).await;

    let theirs = seed_movie(&server, "Their Film", 800).await;
    other
        .put_json(
            &format!("/api/v1/admin/movies/{theirs}/watch-state"),
            &json!({ "watched": true }),
        )
        .await
        .assert_ok();

    Fixture {
        server,
        viewer,
        other,
        movie_id,
        show_id,
        episodes,
    }
}

#[tokio::test]
async fn lists_one_row_per_title_newest_first() {
    let f = fixture().await;
    let page = history(&f.viewer, "").await;
    assert_eq!(page["total"], 3);
    assert_eq!(
        titles(&page),
        vec!["Local Film", "Local Show", "Stremio Film"]
    );
    let items = page["items"].as_array().unwrap();
    assert_eq!(items[0]["media_type"], "movie");
    assert_eq!(items[0]["tmdb_id"], 500);
    assert_eq!(items[0]["movie_id"], f.movie_id.to_string());
    assert_eq!(items[0]["year"], 2001);
    assert_eq!(items[0]["sources"], json!(["local"]));
    assert_eq!(items[0]["excluded"], false);
    assert!(items[0]["last_watched_at"].is_string());
    assert_eq!(items[1]["media_type"], "tv");
    assert_eq!(items[1]["show_id"], f.show_id.to_string());
    assert_eq!(items[1]["tmdb_id"], 600);
    assert_eq!(items[2]["sources"], json!(["stremio"]));
    assert_eq!(items[2]["poster_url"], "https://img.test/p.jpg");
    assert!(items[2]["movie_id"].is_null());
}

#[tokio::test]
async fn search_source_and_paging_narrow_the_list() {
    let f = fixture().await;
    assert_eq!(
        titles(&history(&f.viewer, "?search=show").await),
        vec!["Local Show"]
    );
    let stremio = history(&f.viewer, "?source=stremio").await;
    assert_eq!(titles(&stremio), vec!["Stremio Film"]);
    assert_eq!(stremio["total"], 1);
    let page = history(&f.viewer, "?limit=1&offset=1").await;
    assert_eq!(titles(&page), vec!["Local Show"]);
    assert_eq!(page["total"], 3);
    let resp = f
        .viewer
        .get("/api/v1/admin/watch-history?source=netflix")
        .await;
    assert_eq!(resp.status, StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn exclude_flags_titles_without_touching_watch_state() {
    let f = fixture().await;
    f.viewer
        .post_json(
            "/api/v1/admin/watch-history/exclude",
            &json!({ "items": [{ "media_type": "tv", "tmdb_id": 600 }], "excluded": true }),
        )
        .await
        .assert_status(StatusCode::NO_CONTENT);
    let page = history(&f.viewer, "").await;
    assert_eq!(page["total"], 3);
    assert_eq!(page["items"][1]["excluded"], true);
    assert_eq!(page["items"][0]["excluded"], false);

    f.viewer
        .post_json(
            "/api/v1/admin/watch-history/exclude",
            &json!({ "items": [{ "media_type": "tv", "tmdb_id": 600 }], "excluded": false }),
        )
        .await
        .assert_status(StatusCode::NO_CONTENT);
    assert_eq!(history(&f.viewer, "").await["items"][1]["excluded"], false);
}

#[tokio::test]
async fn remove_clears_library_watch_state_and_excludes_what_lives_in_stremio() {
    let f = fixture().await;
    f.viewer
        .post_json(
            "/api/v1/admin/watch-history/remove",
            &json!({ "items": [
                { "media_type": "movie", "tmdb_id": 500 },
                { "media_type": "tv", "tmdb_id": 600 },
                { "media_type": "movie", "tmdb_id": 700 }
            ] }),
        )
        .await
        .assert_status(StatusCode::NO_CONTENT);

    let page = history(&f.viewer, "").await;
    assert_eq!(titles(&page), vec!["Stremio Film"]);
    assert_eq!(page["items"][0]["excluded"], true);

    let watched: Vec<bool> =
        sqlx::query_scalar("SELECT watched FROM tv_watch_states WHERE episode_id = ANY($1)")
            .bind(&f.episodes)
            .fetch_all(&f.server.pool)
            .await
            .unwrap();
    assert!(watched.iter().all(|w| !w));
    let movie_watched: bool =
        sqlx::query_scalar("SELECT watched FROM movie_watch_states WHERE movie_id = $1")
            .bind(f.movie_id)
            .fetch_one(&f.server.pool)
            .await
            .unwrap();
    assert!(!movie_watched);

    assert_eq!(titles(&history(&f.other, "").await), vec!["Their Film"]);
}

#[tokio::test]
async fn bulk_edits_validate_their_input() {
    let f = fixture().await;
    for path in ["exclude", "remove"] {
        let resp = f
            .viewer
            .post_json(
                &format!("/api/v1/admin/watch-history/{path}"),
                &json!({ "items": [{ "media_type": "book", "tmdb_id": 1 }], "excluded": true }),
            )
            .await;
        assert_eq!(resp.status, StatusCode::BAD_REQUEST, "{path}");
    }
}

#[tokio::test]
async fn watch_history_requires_auth() {
    let server = TestServer::start_api_only().await;
    let anon = TestClient::new(&server.api_url);
    assert_eq!(
        anon.get("/api/v1/admin/watch-history").await.status,
        StatusCode::UNAUTHORIZED
    );
    for path in ["exclude", "remove"] {
        let resp = anon
            .post_json(
                &format!("/api/v1/admin/watch-history/{path}"),
                &json!({ "items": [], "excluded": true }),
            )
            .await;
        assert_eq!(resp.status, StatusCode::UNAUTHORIZED);
    }
}
