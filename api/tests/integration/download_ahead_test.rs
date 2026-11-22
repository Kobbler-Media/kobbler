//! Halfway through an episode, the next one is downloaded and analysed so it starts from
//! disk with its intro and credits already known.

use crate::common::debrid_rig::*;
use crate::common::debrid_upstream::{Hole, HoleAnswer, UpstreamPlan, WholeBody};
use crate::common::*;
use api::jobs::handlers::DebridDownloadAheadHandler;
use api::jobs::worker::JobHandler;
use reqwest::StatusCode;
use serde_json::{Value, json};
use std::time::Duration;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;
use wiremock::matchers::{method, path_regex};
use wiremock::{Mock, ResponseTemplate};

const JOB: &str = "debrid_download_ahead";

/// Episodes 1..=3 of one season; the first has a 3000 s library file.
async fn season(server: &TestServer) -> Vec<Uuid> {
    let episodes = season_of(server, 3).await;
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
    assert_eq!(local.json::<Value>()["release_key"], "aheadhash");
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
        offered["release_key"], "joinhash",
        "the download in flight was not offered"
    );

    let session = session_id_of(
        &rig.resolve_with(&json!({
            "debrid_provider_id": PROVIDER_ID,
            "release_key": "joinhash",
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

async fn admin(server: &TestServer) -> Uuid {
    sqlx::query_scalar("SELECT id FROM users WHERE role = 'admin' LIMIT 1")
        .fetch_one(&server.pool)
        .await
        .unwrap()
}

async fn run_ahead(rig: &Rig, episode: Uuid) -> Result<Value, api::jobs::JobError> {
    DebridDownloadAheadHandler::new(rig.server.app_state.clone())
        .execute(
            Uuid::now_v7(),
            json!({ "user_id": admin(&rig.server).await, "episode_id": episode }),
            CancellationToken::new(),
        )
        .await
}

/// kobload's verdict on a release is final for a download nobody is waiting on; only a
/// provider that cannot serve right now is worth another attempt.
#[tokio::test]
async fn a_release_kobload_will_not_serve_is_skipped_not_retried() {
    let cases = [
        (
            "Debrid error: Usenet release is not cached: articles are missing on every provider (missing_articles)",
            Some("not_cached"),
        ),
        (
            "Debrid error: Usenet release is blocked: its archive is password-protected (password)",
            Some("blocked"),
        ),
        (
            "Debrid error: Resolved release is not a video: it holds no playable file",
            Some("not_video"),
        ),
        (
            "Debrid error: Not warmed: only the release a search would auto-play is grabbed from the indexer ahead of a tap",
            Some("not_cached"),
        ),
        (
            "Debrid error: No usenet provider can serve this right now (providers_unavailable)",
            None,
        ),
    ];
    let rig = Rig::start(UpstreamPlan::new(noise(MB)), "ep.mkv").await;
    rig.mount_search(json!([rig.option("refusedhash")])).await;
    let episodes = season(&rig.server).await;
    for (kobload_says, skipped) in cases {
        rig.kobload.reset().await;
        rig.mount_search(json!([rig.option("refusedhash")])).await;
        Mock::given(method("POST"))
            .and(path_regex("/api/v1/stream/resolve$"))
            .respond_with(ResponseTemplate::new(500).set_body_string(kobload_says))
            .mount(&rig.kobload)
            .await;

        let result = run_ahead(&rig, episodes[1]).await;

        match skipped {
            Some(code) => assert_eq!(result.unwrap()["skipped"], code, "{kobload_says}"),
            None => assert!(result.unwrap_err().is_retryable(), "{kobload_says}"),
        }
    }
}

/// Damage met while downloading ahead is reported once, and the release is not fetched
/// again for another attempt to meet it.
#[tokio::test]
async fn damage_met_downloading_ahead_is_reported_once_and_not_retried() {
    let mut plan = UpstreamPlan::new(noise(2 * MB));
    plan.hole = Some(Hole {
        at: MB,
        answer: HoleAnswer::Error(StatusCode::BAD_GATEWAY, "missing_articles"),
    });
    let rig = Rig::start(plan, "ep.mkv").await;
    rig.mount_search(json!([rig.option("damagedhash")])).await;
    let episodes = season(&rig.server).await;

    let first = run_ahead(&rig, episodes[1]).await;
    let again = run_ahead(&rig, episodes[1]).await;

    assert!(
        first.as_ref().is_err_and(|e| !e.is_retryable()),
        "{first:?}"
    );
    assert!(again.is_ok(), "{again:?}");
    let reports: Vec<Value> = rig
        .kobload_requests("/stream/outcome")
        .await
        .into_iter()
        .filter(|r| r["reason"] == "missing_articles")
        .collect();
    assert_eq!(reports.len(), 1, "{reports:?}");
}
