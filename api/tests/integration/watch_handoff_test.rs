use crate::common::*;
use reqwest::StatusCode;
use serde_json::{Value, json};
use uuid::Uuid;

/// A show with one season and two episodes; returns (show_id, ep1, ep2).
async fn seed_show(server: &TestServer) -> (String, String, String) {
    let resp = server
        .admin_client()
        .post_json(
            "/api/v1/admin/tv/shows",
            &json!({ "title": "Handoff Show", "year": 2024 }),
        )
        .await;
    resp.assert_status(StatusCode::CREATED);
    let show_id = resp.json::<Value>()["id"].as_str().unwrap().to_string();

    let season_id = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO tv_seasons (id, show_id, season_identifier, season_sort_order, is_monitored, is_special) \
         VALUES ($1, $2::uuid, '1', 1, true, false)",
    )
    .bind(season_id)
    .bind(Uuid::parse_str(&show_id).unwrap())
    .execute(&server.pool)
    .await
    .unwrap();

    let mut eps = Vec::new();
    for n in 1..=2 {
        let id = Uuid::now_v7();
        sqlx::query(
            "INSERT INTO tv_episodes (id, season_id, episode_identifier, episode_sort_order, title) \
             VALUES ($1, $2, $3, $4, $5)",
        )
        .bind(id)
        .bind(season_id)
        .bind(n.to_string())
        .bind(n)
        .bind(format!("Episode {n}"))
        .execute(&server.pool)
        .await
        .unwrap();
        eps.push(id.to_string());
    }
    (show_id, eps[0].clone(), eps[1].clone())
}

#[tokio::test]
async fn a_save_from_one_device_is_the_handoff_for_the_whole_show() {
    let server = TestServer::start_api_only().await;
    let (show_id, ep1, ep2) = seed_show(&server).await;

    server
        .admin_client()
        .patch_json(
            &format!("/api/v1/admin/tv/episodes/{ep2}/watch-state/position"),
            &json!({ "position_seconds": 600, "client_id": "tv-1", "client_label": "Living Room TV" }),
        )
        .await
        .assert_ok();

    // Asked from the episode the other device left behind: same show, so same answer.
    let resp: Value = server
        .admin_client()
        .get(&format!(
            "/api/v1/admin/playback/handoff?media_type=episode&media_id={ep1}"
        ))
        .await
        .json();
    assert_eq!(resp["scope_id"], show_id.as_str());
    let got = &resp["latest"];
    assert_eq!(got["media_id"], ep2.as_str());
    assert_eq!(got["scope_id"], show_id.as_str());
    assert_eq!(got["position_seconds"], 600);
    assert_eq!(got["client_id"], "tv-1");
    assert_eq!(got["client_label"], "Living Room TV");
    assert_eq!(got["episode"]["episode_identifier"], "2");
    assert_eq!(got["episode"]["show_title"], "Handoff Show");
}

#[tokio::test]
async fn a_save_without_a_client_id_saves_but_is_not_relayed() {
    let server = TestServer::start_api_only().await;
    let (_, ep1, _) = seed_show(&server).await;

    server
        .admin_client()
        .patch_json(
            &format!("/api/v1/admin/tv/episodes/{ep1}/watch-state/position"),
            &json!({ "position_seconds": 120 }),
        )
        .await
        .assert_ok();

    let ws: Value = server
        .admin_client()
        .get(&format!("/api/v1/admin/tv/episodes/{ep1}/watch-state"))
        .await
        .json();
    assert_eq!(ws["position_seconds"], 120);

    let got: Value = server
        .admin_client()
        .get(&format!(
            "/api/v1/admin/playback/handoff?media_type=episode&media_id={ep1}"
        ))
        .await
        .json();
    assert!(got["latest"].is_null());
}

#[tokio::test]
async fn a_handoff_is_only_visible_to_the_user_who_saved_it() {
    let server = TestServer::start_api_only().await;
    let (_, ep1, _) = seed_show(&server).await;

    server
        .admin_client()
        .patch_json(
            &format!("/api/v1/admin/tv/episodes/{ep1}/watch-state/position"),
            &json!({ "position_seconds": 300, "client_id": "phone-1" }),
        )
        .await
        .assert_ok();

    let viewer = create_user_client(&server, "viewer@test.local", "viewer").await;
    let got: Value = viewer
        .get(&format!(
            "/api/v1/admin/playback/handoff?media_type=episode&media_id={ep1}"
        ))
        .await
        .json();
    assert!(got["latest"].is_null());
}

#[tokio::test]
async fn movie_handoff_is_scoped_to_the_movie() {
    let server = TestServer::start_api_only().await;
    let resp = server
        .admin_client()
        .post_json(
            "/api/v1/admin/movies",
            &json!({ "title": "Handoff Movie", "year": 2024 }),
        )
        .await;
    resp.assert_status(StatusCode::CREATED);
    let movie_id = resp.json::<Value>()["id"].as_str().unwrap().to_string();

    server
        .admin_client()
        .patch_json(
            &format!("/api/v1/admin/movies/{movie_id}/watch-state/position"),
            &json!({ "position_seconds": 1500, "client_id": "laptop-1" }),
        )
        .await
        .assert_ok();

    let resp: Value = server
        .admin_client()
        .get(&format!(
            "/api/v1/admin/playback/handoff?media_type=movie&media_id={movie_id}"
        ))
        .await
        .json();
    let got = &resp["latest"];
    assert_eq!(got["scope_id"], movie_id.as_str());
    assert_eq!(got["movie_title"], "Handoff Movie");
    assert_eq!(got["position_seconds"], 1500);
}

#[tokio::test]
async fn handoff_requires_auth() {
    let server = TestServer::start_api_only().await;
    server
        .api_client()
        .get(&format!(
            "/api/v1/admin/playback/handoff?media_type=movie&media_id={}",
            Uuid::now_v7()
        ))
        .await
        .assert_status(StatusCode::UNAUTHORIZED);
}
