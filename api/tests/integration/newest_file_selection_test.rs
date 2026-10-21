//! An episode or movie can end up with more than one file — a regrab adds a
//! second rather than replacing the first. The decision is that the **newest**
//! one plays; nothing is deleted.

use crate::common::*;
use api::repositories::{movie_files, tv_episode_files};
use reqwest::StatusCode;
use serde_json::json as sj;
use serde_json::{Value, json};
use sqlx::PgPool;
use uuid::Uuid;

// =============================================================================
// Helpers
// =============================================================================

/// The shared stub helpers derive the path from the entity id, so two files on
/// one entity collide on `uq_tv_episode_files_path`. These take a tag.
async fn create_episode_file(pool: &PgPool, episode_id: Uuid, tag: &str) -> Uuid {
    let file_id = tv_episode_files::create(
        pool,
        tv_episode_files::EpisodeFileInput {
            file_path: format!("/test/episodes/{episode_id}-{tag}.mkv"),
            file_size: Some(1024),
            file_fingerprint: None,
            format: "mkv".to_string(),
            quality: None,
            languages: None,
            subtitles: None,
            source_type: Some("test".to_string()),
            source_metadata: Some(sj!({})),
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
    .expect("create episode file");
    // Deliberately NOT `link_episodes`: that path deletes the previous
    // exclusive file, which is exactly the state under test here. An episode
    // reaches two files via multi-episode files today, and via a regrab once
    // the add-don't-replace change lands.
    sqlx::query(
        "INSERT INTO tv_episode_file_episodes (episode_file_id, episode_id) VALUES ($1, $2)",
    )
    .bind(file_id)
    .bind(episode_id)
    .execute(pool)
    .await
    .expect("link episode file");
    file_id
}

async fn create_movie_file(pool: &PgPool, movie_id: Uuid, tag: &str) -> Uuid {
    movie_files::create(
        pool,
        movie_files::MovieFileInput {
            movie_id,
            file_path: format!("/test/movies/{movie_id}-{tag}.mkv"),
            file_size: Some(1024),
            format: "mkv".to_string(),
            ..Default::default()
        },
    )
    .await
    .expect("create movie file")
}

/// Push a file's `created_at` back so "newest" is unambiguous regardless of
/// clock granularity.
async fn age_episode_file(pool: &PgPool, file_id: Uuid, hours: i32) {
    sqlx::query(
        "UPDATE tv_episode_files SET created_at = NOW() - ($2 || ' hours')::interval WHERE id = $1",
    )
    .bind(file_id)
    .bind(hours.to_string())
    .execute(pool)
    .await
    .expect("age episode file");
}

async fn age_movie_file(pool: &PgPool, file_id: Uuid, hours: i32) {
    sqlx::query(
        "UPDATE movie_files SET created_at = NOW() - ($2 || ' hours')::interval WHERE id = $1",
    )
    .bind(file_id)
    .bind(hours.to_string())
    .execute(pool)
    .await
    .expect("age movie file");
}

struct TwoEpisodes {
    first: Uuid,
    second: Uuid,
    old_file: Uuid,
    new_file: Uuid,
}

/// A show with two episodes; the second has an old file and a newer regrab.
async fn setup_two_files(server: &TestServer) -> TwoEpisodes {
    let show: Value = server
        .admin_client()
        .post_json(
            "/api/v1/admin/tv/shows",
            &json!({ "title": "Regrab Test Show", "year": 2024 }),
        )
        .await
        .json();
    let show_id = show["id"].as_str().unwrap();

    let season: Value = server
        .admin_client()
        .post_json(
            &format!("/api/v1/admin/tv/shows/{show_id}/seasons"),
            &json!({ "season_identifier": "1", "season_sort_order": 1 }),
        )
        .await
        .json();
    let season_id = season["id"].as_str().unwrap();

    let mut episode_ids = Vec::new();
    for n in 1..=2 {
        let ep: Value = server
            .admin_client()
            .post_json(
                &format!("/api/v1/admin/tv/seasons/{season_id}/episodes"),
                &json!({
                    "episode_identifier": n.to_string(),
                    "episode_sort_order": n,
                    "title": format!("Episode {n}"),
                }),
            )
            .await
            .json();
        episode_ids.push(ep["id"].as_str().unwrap().parse::<Uuid>().unwrap());
    }

    let second = episode_ids[1];
    let old_file = create_episode_file(&server.pool, second, "old").await;
    let new_file = create_episode_file(&server.pool, second, "new").await;
    age_episode_file(&server.pool, old_file, 48).await;
    age_episode_file(&server.pool, new_file, 1).await;

    TwoEpisodes {
        first: episode_ids[0],
        second,
        old_file,
        new_file,
    }
}

// =============================================================================
// Entry point 1: the repository's episode file picker
// =============================================================================

#[tokio::test]
async fn episode_file_picker_returns_the_newest_file() {
    let server = TestServer::start_api_only().await;
    let eps = setup_two_files(&server).await;

    let picked = tv_episode_files::get_preferred_available(&server.pool, eps.second)
        .await
        .expect("query failed")
        .expect("an available file should be found");

    assert_eq!(
        picked.id, eps.new_file,
        "the newest file must win; got the older one instead"
    );
    assert_ne!(picked.id, eps.old_file);
}

// =============================================================================
// Entry point 2: the movie file picker
// =============================================================================

#[tokio::test]
async fn movie_file_picker_returns_the_newest_file() {
    let server = TestServer::start_api_only().await;

    let movie: Value = server
        .admin_client()
        .post_json(
            "/api/v1/admin/movies",
            &json!({ "title": "Regrab Test Movie", "year": 2024 }),
        )
        .await
        .json();
    let movie_id: Uuid = movie["id"].as_str().unwrap().parse().unwrap();

    let old_file = create_movie_file(&server.pool, movie_id, "old").await;
    let new_file = create_movie_file(&server.pool, movie_id, "new").await;
    age_movie_file(&server.pool, old_file, 48).await;
    age_movie_file(&server.pool, new_file, 1).await;

    let picked = movie_files::get_preferred_available(&server.pool, movie_id)
        .await
        .expect("query failed")
        .expect("an available file should be found");

    assert_eq!(picked.id, new_file, "the newest movie file must win");
    assert_ne!(picked.id, old_file);
}

// =============================================================================
// Entry point 3: play-next (two inline subqueries, one per season branch)
// =============================================================================

#[tokio::test]
async fn play_next_within_a_season_returns_the_newest_file() {
    let server = TestServer::start_api_only().await;
    let eps = setup_two_files(&server).await;

    let resp = server
        .admin_client()
        .get(&format!("/api/v1/admin/tv/episodes/{}/next", eps.first))
        .await;
    resp.assert_ok();

    let json: Value = resp.json();
    assert_eq!(json["episode_id"].as_str().unwrap(), eps.second.to_string());
    assert_eq!(
        json["file_id"].as_str().unwrap(),
        eps.new_file.to_string(),
        "play-next handed the player a stale file"
    );
}

#[tokio::test]
async fn play_next_across_seasons_returns_the_newest_file() {
    let server = TestServer::start_api_only().await;

    let show: Value = server
        .admin_client()
        .post_json(
            "/api/v1/admin/tv/shows",
            &json!({ "title": "Cross Season Show", "year": 2024 }),
        )
        .await
        .json();
    let show_id = show["id"].as_str().unwrap();

    let mut episode_ids = Vec::new();
    for season_no in 1..=2 {
        let season: Value = server
            .admin_client()
            .post_json(
                &format!("/api/v1/admin/tv/shows/{show_id}/seasons"),
                &json!({
                    "season_identifier": season_no.to_string(),
                    "season_sort_order": season_no,
                }),
            )
            .await
            .json();
        let season_id = season["id"].as_str().unwrap();

        let ep: Value = server
            .admin_client()
            .post_json(
                &format!("/api/v1/admin/tv/seasons/{season_id}/episodes"),
                &json!({ "episode_identifier": "1", "episode_sort_order": 1 }),
            )
            .await
            .json();
        episode_ids.push(ep["id"].as_str().unwrap().parse::<Uuid>().unwrap());
    }

    let next_season_ep = episode_ids[1];
    let old_file = create_episode_file(&server.pool, next_season_ep, "old").await;
    let new_file = create_episode_file(&server.pool, next_season_ep, "new").await;
    age_episode_file(&server.pool, old_file, 48).await;
    age_episode_file(&server.pool, new_file, 1).await;

    let resp = server
        .admin_client()
        .get(&format!(
            "/api/v1/admin/tv/episodes/{}/next",
            episode_ids[0]
        ))
        .await;
    resp.assert_ok();

    let json: Value = resp.json();
    assert_eq!(
        json["episode_id"].as_str().unwrap(),
        next_season_ep.to_string()
    );
    assert_eq!(
        json["file_id"].as_str().unwrap(),
        new_file.to_string(),
        "cross-season play-next handed the player a stale file"
    );
}

// =============================================================================
// Entry point 4: the /files listings the web player reads files[0] from
// =============================================================================

#[tokio::test]
async fn episode_files_listing_puts_the_newest_first() {
    let server = TestServer::start_api_only().await;
    let eps = setup_two_files(&server).await;

    let resp = server
        .admin_client()
        .get(&format!("/api/v1/admin/tv/episodes/{}/files", eps.second))
        .await;
    resp.assert_ok();

    let files: Value = resp.json();
    let files = files.as_array().expect("files array");
    assert_eq!(files.len(), 2, "the old file must still be there");
    assert_eq!(files[0]["id"].as_str().unwrap(), eps.new_file.to_string());
    assert_eq!(files[1]["id"].as_str().unwrap(), eps.old_file.to_string());
}

#[tokio::test]
async fn episode_files_listing_requires_auth() {
    let server = TestServer::start_api_only().await;
    let eps = setup_two_files(&server).await;

    let resp = TestClient::new(&server.api_url)
        .get(&format!("/api/v1/admin/tv/episodes/{}/files", eps.second))
        .await;
    assert_eq!(resp.status, StatusCode::UNAUTHORIZED);
}
