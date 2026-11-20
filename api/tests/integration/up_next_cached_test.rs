//! A debrid episode that came out after the viewer caught up waits off Up Next until a
//! release of it is downloaded, and a scheduled watch downloads it once one turns up.

use crate::common::debrid_rig::*;
use crate::common::*;
use crate::dashboard_test::{get_discover, mark_watched};
use api::jobs::handlers::{DebridDownloadAheadHandler, DebridNewEpisodeWatchHandler};
use api::jobs::worker::JobHandler;
use serde_json::{Value, json};
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

const AHEAD_JOB: &str = "debrid_download_ahead";

async fn exec(server: &TestServer, sql: &'static str, id: Uuid, n: i32) {
    sqlx::query(sql)
        .bind(id)
        .bind(n)
        .execute(&server.pool)
        .await
        .unwrap();
}

async fn aired_days_ago(server: &TestServer, episode: Uuid, days: i32) {
    exec(
        server,
        "UPDATE tv_episodes SET air_date = CURRENT_DATE - $2 WHERE id = $1",
        episode,
        days,
    )
    .await;
}

async fn watched_days_ago(server: &TestServer, episode: Uuid, days: i32) {
    mark_watched(server, episode).await;
    exec(
        server,
        "UPDATE tv_watch_states SET watched_at = NOW() - make_interval(days => $2),
                                    updated_at = NOW() - make_interval(days => $2)
         WHERE episode_id = $1",
        episode,
        days,
    )
    .await;
}

/// Episode 1 watched three days ago, episode 2 out since yesterday, episode 3 unaired.
async fn caught_up(server: &TestServer) -> Vec<Uuid> {
    let episodes = season_of(server, 3).await;
    aired_days_ago(server, episodes[0], 10).await;
    aired_days_ago(server, episodes[1], 1).await;
    aired_days_ago(server, episodes[2], -6).await;
    watched_days_ago(server, episodes[0], 3).await;
    episodes
}

async fn cached_release(server: &TestServer, episode: Uuid, info_hash: &str, complete: bool) {
    let path = format!("/cache/{info_hash}/ep.mkv");
    sqlx::query(
        "INSERT INTO debrid_stream_sessions
            (id, user_id, media_type, media_id, release_title, info_hash, debrid_provider,
             filename, filesize, cache_path)
         SELECT $1, id, 'episode', $2, 'Release', $3, 'realdebrid', 'ep.mkv', 1, $4
         FROM users WHERE role = 'admin' LIMIT 1",
    )
    .bind(Uuid::now_v7())
    .bind(episode)
    .bind(info_hash)
    .bind(&path)
    .execute(&server.pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO debrid_cache_files (file_path, file_size, expires_at, info_hash, filename,
                                         completed_at)
         VALUES ($1, 1, NOW() + INTERVAL '1 day', $2, 'ep.mkv',
                 CASE WHEN $3 THEN NOW() END)",
    )
    .bind(&path)
    .bind(info_hash)
    .bind(complete)
    .execute(&server.pool)
    .await
    .unwrap();
}

async fn up_next(server: &TestServer) -> Vec<String> {
    get_discover(server).await["watch_next"]
        .as_array()
        .unwrap()
        .iter()
        .map(|i| i["media_id"].as_str().unwrap().to_string())
        .collect()
}

async fn queued(server: &TestServer) -> Vec<Value> {
    sqlx::query_scalar("SELECT payload FROM job_status WHERE job_type = $1 ORDER BY created_at")
        .bind(AHEAD_JOB)
        .fetch_all(&server.pool)
        .await
        .unwrap()
}

async fn with_kobload(server: &TestServer) {
    server
        .admin_client()
        .patch_json(
            "/api/v1/admin/config",
            &json!({ "kobload_base_url": "http://127.0.0.1:9", "kobload_api_key": "k" }),
        )
        .await
        .assert_ok();
}

async fn watch(server: &TestServer) {
    DebridNewEpisodeWatchHandler::new(server.app_state.clone())
        .execute(Uuid::now_v7(), json!({}), CancellationToken::new())
        .await
        .unwrap();
}

#[tokio::test]
async fn a_new_episode_waits_off_up_next_until_its_download_completes() {
    let server = TestServer::start_api_only().await;
    let episodes = caught_up(&server).await;
    let ep2 = episodes[1].to_string();

    assert!(
        !up_next(&server).await.contains(&ep2),
        "shown with nothing cached"
    );

    cached_release(&server, episodes[1], "partial", false).await;
    assert!(
        !up_next(&server).await.contains(&ep2),
        "shown while the download is unfinished"
    );

    cached_release(&server, episodes[1], "whole", true).await;
    assert_eq!(up_next(&server).await, vec![ep2]);
}

#[tokio::test]
async fn a_cached_new_episode_shows_on_its_release_day() {
    let server = TestServer::start_api_only().await;
    let episodes = caught_up(&server).await;
    aired_days_ago(&server, episodes[1], 0).await;
    cached_release(&server, episodes[1], "today", true).await;

    assert_eq!(up_next(&server).await, vec![episodes[1].to_string()]);
}

/// The watch gives up on an episode a week after it aired; Up Next must not keep hiding
/// one it never found a release for.
#[tokio::test]
async fn a_new_episode_out_over_a_week_shows_without_a_cache() {
    let server = TestServer::start_api_only().await;
    let episodes = caught_up(&server).await;
    aired_days_ago(&server, episodes[1], 8).await;
    exec(
        &server,
        "UPDATE tv_watch_states SET watched_at = NOW() - make_interval(days => $2),
                                    updated_at = NOW() - make_interval(days => $2)
         WHERE episode_id = $1",
        episodes[0],
        9,
    )
    .await;

    assert_eq!(up_next(&server).await, vec![episodes[1].to_string()]);
}

/// Bingeing aired episodes keeps the air-date rule: a download ahead that failed or was
/// evicted must not drop the show.
#[tokio::test]
async fn an_episode_that_aired_before_the_last_watch_needs_no_cache() {
    let server = TestServer::start_api_only().await;
    let episodes = season_of(&server, 2).await;
    aired_days_ago(&server, episodes[0], 30).await;
    aired_days_ago(&server, episodes[1], 20).await;
    watched_days_ago(&server, episodes[0], 1).await;

    assert_eq!(up_next(&server).await, vec![episodes[1].to_string()]);
}

#[tokio::test]
async fn the_watch_downloads_the_new_episode_which_then_shows() {
    let rig = Rig::start(
        crate::common::debrid_upstream::UpstreamPlan::new(noise(MB)),
        "ep.mkv",
    )
    .await;
    rig.mount_search(json!([rig.option("newhash")])).await;
    let server = &rig.server;
    let episodes = caught_up(server).await;

    watch(server).await;
    let queued_now = queued(server).await;
    assert_eq!(queued_now.len(), 1, "{queued_now:?}");
    assert_eq!(queued_now[0]["episode_id"], json!(episodes[1]));

    server
        .process_pending_jobs(&DebridDownloadAheadHandler::new(server.app_state.clone()))
        .await;
    assert_eq!(up_next(server).await, vec![episodes[1].to_string()]);

    let held: bool = sqlx::query_scalar(
        "SELECT expires_at > NOW() + INTERVAL '13 days' FROM debrid_cache_files WHERE info_hash = 'newhash'",
    )
    .fetch_one(&server.pool)
    .await
    .unwrap();
    assert!(held, "the new episode may be evicted before it is watched");

    watch(server).await;
    assert_eq!(
        queued(server).await.len(),
        1,
        "queued a cached episode again"
    );
}

#[tokio::test]
async fn the_watch_retries_only_after_a_pause() {
    let server = TestServer::start_api_only().await;
    with_kobload(&server).await;
    caught_up(&server).await;

    watch(&server).await;
    watch(&server).await;
    assert_eq!(queued(&server).await.len(), 1);

    sqlx::query(
        "UPDATE job_status SET created_at = NOW() - INTERVAL '3 hours' WHERE job_type = $1",
    )
    .bind(AHEAD_JOB)
    .execute(&server.pool)
    .await
    .unwrap();
    watch(&server).await;
    assert_eq!(queued(&server).await.len(), 2);
}

#[derive(Debug, Clone, Copy)]
enum Skip {
    Unaired,
    AiredOverAWeekAgo,
    WantedThroughKobload,
    AlreadyCached,
    ShowDismissed,
    ShowAbandoned,
}

async fn arrange(server: &TestServer, skip: Skip, e: &[Uuid]) {
    let watched_ago = "UPDATE tv_watch_states SET watched_at = NOW() - make_interval(days => $2),
                                                  updated_at = NOW() - make_interval(days => $2)
                       WHERE episode_id = $1";
    match skip {
        Skip::Unaired => aired_days_ago(server, e[1], -1).await,
        Skip::AiredOverAWeekAgo => {
            aired_days_ago(server, e[0], 30).await;
            aired_days_ago(server, e[1], 8).await;
            exec(server, watched_ago, e[0], 9).await;
        }
        Skip::WantedThroughKobload => {
            exec(
                server,
                "UPDATE tv_episodes SET want_status = 'wanted' WHERE id = $1 AND $2 = $2",
                e[1],
                0,
            )
            .await
        }
        Skip::AlreadyCached => cached_release(server, e[1], "have", true).await,
        Skip::ShowDismissed => {
            exec(
                server,
                "INSERT INTO up_next_dismissals (user_id, kind, target)
                 SELECT u.id, 'show', se.show_id::text
                 FROM users u, tv_episodes te JOIN tv_seasons se ON se.id = te.season_id
                 WHERE u.role = 'admin' AND te.id = $1 AND $2 = $2",
                e[1],
                0,
            )
            .await
        }
        Skip::ShowAbandoned => exec(server, watched_ago, e[0], 20).await,
    }
}

#[tokio::test]
async fn the_watch_leaves_episodes_it_should_not_fetch() {
    let server = TestServer::start_api_only().await;
    with_kobload(&server).await;
    for skip in [
        Skip::Unaired,
        Skip::AiredOverAWeekAgo,
        Skip::WantedThroughKobload,
        Skip::AlreadyCached,
        Skip::ShowDismissed,
        Skip::ShowAbandoned,
    ] {
        sqlx::query("TRUNCATE tv_shows, job_status, up_next_dismissals CASCADE")
            .execute(&server.pool)
            .await
            .unwrap();
        let episodes = caught_up(&server).await;
        arrange(&server, skip, &episodes).await;
        watch(&server).await;
        assert!(queued(&server).await.is_empty(), "{skip:?}");
    }
}
