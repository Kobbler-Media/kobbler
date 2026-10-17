use crate::common::*;
use reqwest::StatusCode;
use serde_json::json;
use uuid::Uuid;

// Only admins and contributors change the library. A viewer's writes are their own: what
// they watched, where they stopped, how they rated a show.

async fn seed_movie(server: &TestServer) -> Uuid {
    let id = Uuid::new_v4();
    sqlx::query("INSERT INTO movies (id, title) VALUES ($1, 'Edit Me')")
        .bind(id)
        .execute(&server.pool)
        .await
        .unwrap();
    id
}

async fn seed_show_with_episode(server: &TestServer) -> (Uuid, Uuid) {
    let (show, season, episode) = (Uuid::new_v4(), Uuid::new_v4(), Uuid::new_v4());
    sqlx::query("INSERT INTO tv_shows (id, title) VALUES ($1, 'Edit Me Too')")
        .bind(show)
        .execute(&server.pool)
        .await
        .unwrap();
    sqlx::query(
        "INSERT INTO tv_seasons (id, show_id, season_identifier, season_sort_order) VALUES ($1, $2, '1', 1)",
    )
    .bind(season)
    .bind(show)
    .execute(&server.pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO tv_episodes (id, season_id, episode_identifier, episode_sort_order) VALUES ($1, $2, '1', 1)",
    )
    .bind(episode)
    .bind(season)
    .execute(&server.pool)
    .await
    .unwrap();
    (show, episode)
}

#[tokio::test]
async fn viewers_cannot_change_the_library() {
    let server = TestServer::start_api_only().await;
    let viewer = create_user_client(&server, "viewer@test.com", "viewer").await;
    let movie = seed_movie(&server).await;
    let (show, episode) = seed_show_with_episode(&server).await;

    let forbidden = [
        viewer
            .patch_json(
                &format!("/api/v1/admin/movies/{movie}"),
                &json!({ "title": "Hacked" }),
            )
            .await,
        viewer
            .delete(&format!("/api/v1/admin/movies/{movie}"))
            .await,
        viewer
            .post_json(&format!("/api/v1/admin/movies/{movie}/want"), &json!({}))
            .await,
        viewer
            .patch_json(
                &format!("/api/v1/admin/tv/shows/{show}"),
                &json!({ "title": "Hacked" }),
            )
            .await,
        viewer
            .delete(&format!("/api/v1/admin/tv/shows/{show}"))
            .await,
        viewer
            .patch_json(
                &format!("/api/v1/admin/tv/episodes/{episode}"),
                &json!({ "title": "Hacked" }),
            )
            .await,
        viewer
            .post_json(
                "/api/v1/admin/tv/episodes/bulk",
                &json!({ "episode_ids": [episode], "action": "delete_files" }),
            )
            .await,
        viewer
            .post_json(
                "/api/v1/admin/sports/leagues",
                &json!({ "title": "League" }),
            )
            .await,
    ];
    for resp in &forbidden {
        resp.assert_status(StatusCode::FORBIDDEN);
    }
}

#[tokio::test]
async fn viewers_keep_their_own_watch_state_and_ratings() {
    let server = TestServer::start_api_only().await;
    let viewer = create_user_client(&server, "viewer@test.com", "viewer").await;
    let movie = seed_movie(&server).await;
    let (show, episode) = seed_show_with_episode(&server).await;

    let own = [
        viewer
            .put_json(
                &format!("/api/v1/admin/movies/{movie}/watch-state"),
                &json!({ "watched": true }),
            )
            .await,
        viewer
            .put_json(
                &format!("/api/v1/admin/tv/episodes/{episode}/watch-state"),
                &json!({ "watched": true }),
            )
            .await,
        viewer
            .post_json(
                "/api/v1/admin/tv/episodes/bulk",
                &json!({ "episode_ids": [episode], "action": "mark_unwatched" }),
            )
            .await,
        viewer
            .put_json(
                &format!("/api/v1/admin/tv/shows/{show}/rating"),
                &json!({ "rating": 4 }),
            )
            .await,
    ];
    for resp in &own {
        assert!(resp.status.is_success(), "{} {}", resp.status, resp.text());
    }
}

#[tokio::test]
async fn contributors_edit_the_library() {
    let server = TestServer::start_api_only().await;
    let contributor = create_user_client(&server, "contrib@test.com", "contributor").await;
    let movie = seed_movie(&server).await;
    contributor
        .patch_json(
            &format!("/api/v1/admin/movies/{movie}"),
            &json!({ "title": "Renamed" }),
        )
        .await
        .assert_status(StatusCode::NO_CONTENT);
}
