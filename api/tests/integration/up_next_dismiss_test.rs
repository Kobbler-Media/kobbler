use crate::common::*;
use crate::dashboard_test::{
    add_watchlist, create_episode, create_movie, create_season, create_show, get_discover,
    mark_watched, watchlist_ids,
};
use reqwest::StatusCode;
use serde_json::{Value, json};
use uuid::Uuid;

const DISMISS: &str = "/api/v1/admin/up-next/dismiss";

async fn dismiss(server: &TestServer, media_type: &str, media_id: &str) -> TestResponse {
    server
        .admin_client()
        .post_json(
            DISMISS,
            &json!({ "media_type": media_type, "media_id": media_id }),
        )
        .await
}

async fn dismiss_ok(server: &TestServer, media_type: &str, media_id: &str) {
    dismiss(server, media_type, media_id)
        .await
        .assert_status(StatusCode::NO_CONTENT);
}

fn row_ids(discover: &Value, row: &str, id_field: &str) -> Vec<String> {
    discover[row]
        .as_array()
        .unwrap()
        .iter()
        .map(|i| i[id_field].as_str().unwrap().to_string())
        .collect()
}

async fn watch_next_ids(server: &TestServer) -> Vec<String> {
    row_ids(&get_discover(server).await, "watch_next", "media_id")
}

async fn stremio_tmdb_ids(server: &TestServer) -> Vec<String> {
    row_ids(
        &get_discover(server).await,
        "stremio_continue",
        "provider_id",
    )
}

async fn set_movie_position(server: &TestServer, movie_id: Uuid, position: i32) {
    server
        .admin_client()
        .put_json(
            &format!("/api/v1/admin/movies/{movie_id}/watch-state"),
            &json!({ "watched": false, "position_seconds": position }),
        )
        .await
        .assert_ok();
}

async fn in_progress_movie(server: &TestServer, title: &str) -> Uuid {
    let movie_id = create_movie(server, title).await;
    create_stub_movie_file(&server.pool, movie_id).await;
    set_movie_position(server, movie_id, 120).await;
    movie_id
}

async fn show_with_episodes(server: &TestServer, title: &str, count: i32) -> Vec<Uuid> {
    let show_id = create_show(server, title).await;
    let season_id = create_season(&server.pool, show_id, 1).await;
    let mut episodes = Vec::new();
    for n in 1..=count {
        let ep = create_episode(&server.pool, season_id, n).await;
        create_stub_episode_file(&server.pool, ep).await;
        episodes.push(ep);
    }
    episodes
}

async fn seed_stremio_movie(server: &TestServer, stremio_id: &str, tmdb_id: &str) {
    sqlx::query(
        "INSERT INTO stremio_accounts (id, user_id, email, auth_key)
         VALUES ($1, NULL, 'stremio@test.local', 'key')
         ON CONFLICT DO NOTHING",
    )
    .bind(Uuid::now_v7())
    .execute(&server.pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO stremio_continue_items
             (account_id, stremio_id, media_type, kind, tmdb_id, title, last_watched)
         SELECT id, $1, 'movie', 'resume', $2, $1, NOW() - INTERVAL '1 hour'
         FROM stremio_accounts WHERE user_id IS NULL",
    )
    .bind(stremio_id)
    .bind(tmdb_id)
    .execute(&server.pool)
    .await
    .unwrap();
}

#[tokio::test]
async fn dismissed_movie_leaves_up_next_until_watched_again() {
    let server = TestServer::start_api_only().await;
    let movie_id = in_progress_movie(&server, "Halfway").await;
    assert_eq!(watch_next_ids(&server).await, vec![movie_id.to_string()]);

    dismiss_ok(&server, "movie", &movie_id.to_string()).await;
    assert!(watch_next_ids(&server).await.is_empty());

    dismiss_ok(&server, "movie", &movie_id.to_string()).await;
    assert!(watch_next_ids(&server).await.is_empty());

    set_movie_position(&server, movie_id, 300).await;
    assert_eq!(watch_next_ids(&server).await, vec![movie_id.to_string()]);
}

#[tokio::test]
async fn dismissing_an_episode_hides_its_show_until_watched_again() {
    let server = TestServer::start_api_only().await;
    let eps = show_with_episodes(&server, "Bored Of It", 3).await;
    mark_watched(&server, eps[0]).await;
    assert_eq!(watch_next_ids(&server).await, vec![eps[1].to_string()]);

    dismiss_ok(&server, "episode", &eps[1].to_string()).await;
    assert!(watch_next_ids(&server).await.is_empty());

    mark_watched(&server, eps[1]).await;
    assert_eq!(watch_next_ids(&server).await, vec![eps[2].to_string()]);
}

#[tokio::test]
async fn dismissing_a_watchlisted_title_drops_it_from_the_watchlist() {
    let server = TestServer::start_api_only().await;
    let movie_id = create_movie(&server, "Someday Movie").await;
    let eps = show_with_episodes(&server, "Someday Show", 1).await;
    let show_id: Uuid = sqlx::query_scalar(
        "SELECT s.show_id FROM tv_episodes e JOIN tv_seasons s ON s.id = e.season_id WHERE e.id = $1",
    )
    .bind(eps[0])
    .fetch_one(&server.pool)
    .await
    .unwrap();
    add_watchlist(&server, "movie", movie_id).await;
    add_watchlist(&server, "show", show_id).await;
    assert_eq!(watch_next_ids(&server).await.len(), 2);

    dismiss_ok(&server, "movie", &movie_id.to_string()).await;
    dismiss_ok(&server, "episode", &eps[0].to_string()).await;

    assert!(watch_next_ids(&server).await.is_empty());
    assert!(watchlist_ids(&server).await.is_empty());
}

#[tokio::test]
async fn dismissed_stremio_item_returns_when_played_again() {
    let server = TestServer::start_api_only().await;
    seed_stremio_movie(&server, "tt0000901", "901").await;
    assert_eq!(stremio_tmdb_ids(&server).await, vec!["901"]);

    dismiss_ok(&server, "stremio", "tt0000901").await;
    assert!(stremio_tmdb_ids(&server).await.is_empty());

    sqlx::query("UPDATE stremio_continue_items SET last_watched = NOW() WHERE stremio_id = $1")
        .bind("tt0000901")
        .execute(&server.pool)
        .await
        .unwrap();
    assert_eq!(stremio_tmdb_ids(&server).await, vec!["901"]);
}

#[tokio::test]
async fn dismissed_library_title_does_not_resurface_from_stremio() {
    let server = TestServer::start_api_only().await;
    let movie_id = in_progress_movie(&server, "Both Places").await;
    sqlx::query(
        "UPDATE movies SET identifiers = jsonb_build_object('tmdb_id', '902') WHERE id = $1",
    )
    .bind(movie_id)
    .execute(&server.pool)
    .await
    .unwrap();
    seed_stremio_movie(&server, "tt0000902", "902").await;
    assert!(stremio_tmdb_ids(&server).await.is_empty());

    dismiss_ok(&server, "movie", &movie_id.to_string()).await;

    assert!(watch_next_ids(&server).await.is_empty());
    assert!(stremio_tmdb_ids(&server).await.is_empty());
}

#[tokio::test]
async fn dismiss_rejects_bad_requests() {
    let server = TestServer::start_api_only().await;
    let unknown = Uuid::now_v7().to_string();
    let cases = [
        ("book", unknown.as_str(), StatusCode::BAD_REQUEST),
        ("show", unknown.as_str(), StatusCode::BAD_REQUEST),
        ("movie", unknown.as_str(), StatusCode::NOT_FOUND),
        ("episode", unknown.as_str(), StatusCode::NOT_FOUND),
        ("event", unknown.as_str(), StatusCode::NOT_FOUND),
        ("movie", "not-a-uuid", StatusCode::NOT_FOUND),
        ("stremio", "tt0000999", StatusCode::NOT_FOUND),
    ];
    for (media_type, media_id, expected) in cases {
        let resp = dismiss(&server, media_type, media_id).await;
        assert_eq!(resp.status, expected, "{media_type} {media_id}");
    }
}

#[tokio::test]
async fn dismiss_requires_auth() {
    let server = TestServer::start_api_only().await;
    let resp = TestClient::new(&server.api_url)
        .post_json(
            DISMISS,
            &json!({ "media_type": "movie", "media_id": Uuid::now_v7() }),
        )
        .await;
    assert_eq!(resp.status, StatusCode::UNAUTHORIZED);
}
