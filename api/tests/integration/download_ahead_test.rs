//! Halfway through an episode, the next one is downloaded and analysed so it starts from
//! disk with its intro and credits already known.

use crate::common::debrid_rig::*;
use crate::common::debrid_upstream::{UpstreamPlan, WholeBody};
use crate::common::*;
use api::jobs::handlers::DebridDownloadAheadHandler;
use api::jobs::worker::JobHandler;
use reqwest::StatusCode;
use serde_json::{Value, json};
use std::time::Duration;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

const JOB: &str = "debrid_download_ahead";

/// Episodes 1..=3 of one season; the first has a 3000 s library file.
async fn season(server: &TestServer) -> Vec<Uuid> {
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
    for n in 1..=3 {
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
    library_file(server, episodes[0]).await;
    episodes
}

async fn library_file(server: &TestServer, episode: Uuid) {
    let file = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO tv_episode_files (id, file_path, format, available, duration_seconds) \
         VALUES ($1, '/test/' || $1::text || '.mkv', 'MKV', true, 3000)",
    )
    .bind(file)
    .execute(&server.pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO tv_episode_file_episodes (episode_file_id, episode_id) VALUES ($1, $2)",
    )
    .bind(file)
    .bind(episode)
    .execute(&server.pool)
    .await
    .unwrap();
}

async fn save_position(server: &TestServer, episode: Uuid, seconds: i32) {
    server
        .admin_client()
        .patch_json(
            &format!("/api/v1/admin/tv/episodes/{episode}/watch-state/position"),
            &json!({ "position_seconds": seconds }),
        )
        .await
        .assert_ok();
}

async fn queued_for(server: &TestServer) -> Vec<Value> {
    sqlx::query_scalar("SELECT payload FROM job_status WHERE job_type = $1 ORDER BY created_at")
        .bind(JOB)
        .fetch_all(&server.pool)
        .await
        .unwrap()
}

#[tokio::test]
async fn the_next_episode_is_queued_once_halfway_through() {
    let rig = Rig::start(UpstreamPlan::new(noise(MB)), "ep.mkv").await;
    let server = &rig.server;
    let episodes = season(server).await;

    save_position(server, episodes[0], 1400).await;
    assert!(queued_for(server).await.is_empty(), "queued before halfway");

    for seconds in [1600, 1700, 2900] {
        save_position(server, episodes[0], seconds).await;
    }
    let queued = queued_for(server).await;
    assert_eq!(queued.len(), 1, "{queued:?}");
    assert_eq!(queued[0]["episode_id"], json!(episodes[1]));

    library_file(server, episodes[2]).await;
    sqlx::query("UPDATE tv_watch_states SET position_seconds = 0")
        .execute(&server.pool)
        .await
        .unwrap();
    library_file(server, episodes[1]).await;
    save_position(server, episodes[1], 2000).await;
    assert_eq!(
        queued_for(server).await.len(),
        1,
        "queued an episode that is in the library"
    );
}

#[tokio::test]
async fn the_queued_episode_is_downloaded_and_then_plays_from_disk() {
    let data = noise(2 * MB);
    let rig = Rig::start(UpstreamPlan::new(data.clone()), "ep.mkv").await;
    rig.mount_search(json!([rig.option("aheadhash")])).await;
    let server = &rig.server;
    let episodes = season(server).await;
    save_position(server, episodes[0], 2000).await;

    server
        .process_pending_jobs(&DebridDownloadAheadHandler::new(server.app_state.clone()))
        .await;

    assert!(std::fs::read(rig.cache_file("aheadhash")).unwrap() == data);
    let processing: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM job_status WHERE job_type = 'debrid_process' AND payload->>'cache_path' = $1",
    )
    .bind(rig.cache_file("aheadhash").to_string_lossy().to_string())
    .fetch_one(&server.pool)
    .await
    .unwrap();
    assert_eq!(processing, 1, "the download was not analysed");

    let local = server
        .admin_client()
        .get(&format!(
            "/api/v1/admin/stream/local?media_type=episode&media_id={}",
            episodes[1]
        ))
        .await;
    local.assert_status(StatusCode::OK);
    assert_eq!(local.json::<Value>()["info_hash"], "aheadhash");
}

/// Starting the episode while it downloads ahead shares that download: a second writer
/// would corrupt the file, and stopping the first throws away its connection.
#[tokio::test]
async fn a_viewer_joins_the_download_ahead_rather_than_restarting_it() {
    let data = noise(4 * MB);
    let mut plan = UpstreamPlan::new(data.clone());
    plan.whole = WholeBody::Trickle {
        chunk: 64 * 1024,
        every: Duration::from_millis(30),
    };
    let rig = Rig::start(plan, "ep.mkv").await;
    rig.mount_search(json!([rig.option("joinhash")])).await;
    let episodes = season(&rig.server).await;
    let user: Uuid = sqlx::query_scalar("SELECT id FROM users LIMIT 1")
        .fetch_one(&rig.server.pool)
        .await
        .unwrap();

    let handler = DebridDownloadAheadHandler::new(rig.server.app_state.clone());
    let payload = json!({ "user_id": user, "episode_id": episodes[1] });
    let ahead = tokio::spawn(async move {
        handler
            .execute(Uuid::now_v7(), payload, CancellationToken::new())
            .await
    });
    rig.wait_for_local("joinhash", 256 * 1024).await;

    let offered: Value = rig
        .server
        .admin_client()
        .get(&format!(
            "/api/v1/admin/stream/local?media_type=episode&media_id={}",
            episodes[1]
        ))
        .await
        .json();
    assert_eq!(
        offered["info_hash"], "joinhash",
        "the download in flight was not offered"
    );

    let session = session_id_of(
        &rig.resolve_with(&json!({
            "debrid_provider_id": PROVIDER_ID,
            "info_hash": "joinhash",
            "media_type": "episode",
            "media_id": episodes[1],
            "release_title": "Release",
            "debrid_provider": "realdebrid",
            "capabilities": direct_play_caps()
        }))
        .await,
    );
    let head = play(&rig.server, session, "bytes=0-65535").await;
    assert_eq!(head.status, StatusCode::PARTIAL_CONTENT);
    assert!(head.body == data[..65536]);

    let finished = tokio::time::timeout(Duration::from_secs(30), ahead)
        .await
        .expect("download ahead never finished")
        .unwrap()
        .unwrap();
    assert!(finished.get("handed_over").is_none(), "{finished}");
    assert!(std::fs::read(rig.cache_file("joinhash")).unwrap() == data);
    assert_eq!(
        rig.upstream.whole_gets(),
        1,
        "the file was downloaded twice"
    );
    let resumed: Vec<String> = rig
        .upstream
        .ranges_seen()
        .into_iter()
        .filter(|r| r.ends_with('-'))
        .collect();
    assert!(
        resumed.is_empty(),
        "a second writer resumed the file: {resumed:?}"
    );
}
