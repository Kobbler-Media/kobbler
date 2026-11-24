//! One retry policy for both ways an episode is queued for download ahead: a fresh air date
//! is worth looking at again sooner, and a job that worked or is running is left alone.

use crate::common::debrid_rig::*;
use crate::common::*;
use crate::dashboard_test::mark_watched;
use api::jobs::handlers::{DebridDownloadAheadHandler, DebridNewEpisodeWatchHandler};
use api::jobs::worker::JobHandler;
use serde_json::{Value, json};
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

const JOB: &str = "debrid_download_ahead";

#[derive(Clone, Copy)]
struct LastJob {
    status: &'static str,
    result: Option<&'static str>,
    created_hours_ago: i32,
    updated_hours_ago: i32,
}

const fn ended(status: &'static str, result: Option<&'static str>, hours: i32) -> LastJob {
    LastJob {
        status,
        result,
        created_hours_ago: hours,
        updated_hours_ago: hours,
    }
}

const fn nothing_found(hours: i32) -> LastJob {
    ended("completed", Some(r#"{"skipped":"not_cached"}"#), hours)
}

const fn downloaded(hours: i32) -> LastJob {
    ended("completed", Some(r#"{"session_id":"x"}"#), hours)
}

struct Case {
    label: &'static str,
    aired_days_ago: Option<i32>,
    last: LastJob,
    watch_queues: bool,
    mid_watch_queues: bool,
}

const fn case(
    label: &'static str,
    aired_days_ago: Option<i32>,
    last: LastJob,
    watch_queues: bool,
    mid_watch_queues: bool,
) -> Case {
    Case {
        label,
        aired_days_ago,
        last,
        watch_queues,
        mid_watch_queues,
    }
}

const CASES: &[Case] = &[
    case("aired today, 4h", Some(0), nothing_found(4), true, true),
    case("aired today, 1h", Some(0), nothing_found(1), false, false),
    case("aired today, 3h", Some(0), nothing_found(3), true, true),
    case("aired 2d ago, 4h", Some(2), nothing_found(4), true, true),
    case(
        "aired 2d ago, failed 4h",
        Some(2),
        ended("failed", None, 4),
        true,
        true,
    ),
    case("aired 3d ago, 4h", Some(3), nothing_found(4), false, false),
    case("aired 3d ago, 13h", Some(3), nothing_found(13), true, true),
    case(
        "back catalogue, 4h",
        Some(30),
        nothing_found(4),
        false,
        false,
    ),
    case(
        "back catalogue, 13h",
        Some(30),
        nothing_found(13),
        false,
        true,
    ),
    case("no air date, 4h", None, nothing_found(4), false, false),
    case("no air date, 13h", None, nothing_found(13), false, true),
    case("pre-air 2d, 4h", Some(-2), nothing_found(4), false, false),
    case("pre-air 2d, 13h", Some(-2), nothing_found(13), true, true),
    case("pre-air 3d, 13h", Some(-3), nothing_found(13), true, true),
    case("pre-air 4d, 13h", Some(-4), nothing_found(13), false, false),
    case("pre-air 5d, 13h", Some(-5), nothing_found(13), false, false),
    case(
        "fresh, downloaded 4h ago",
        Some(0),
        downloaded(4),
        false,
        false,
    ),
    case(
        "fresh, downloaded 13h ago",
        Some(0),
        downloaded(13),
        true,
        true,
    ),
    case(
        "fresh, still running after 20h",
        Some(0),
        LastJob {
            status: "running",
            result: None,
            created_hours_ago: 20,
            updated_hours_ago: 0,
        },
        false,
        false,
    ),
    case(
        "fresh, retrying",
        Some(0),
        LastJob {
            status: "retrying",
            result: None,
            created_hours_ago: 5,
            updated_hours_ago: 1,
        },
        false,
        false,
    ),
    case(
        "fresh, pending and lost for 13h",
        Some(0),
        LastJob {
            status: "pending",
            result: None,
            created_hours_ago: 13,
            updated_hours_ago: 13,
        },
        true,
        true,
    ),
];

async fn caught_up(server: &TestServer, aired_days_ago: Option<i32>) -> (Uuid, Uuid) {
    let episodes = season_of(server, 2).await;
    sqlx::query("UPDATE tv_episodes SET air_date = CURRENT_DATE - 40 WHERE id = $1")
        .bind(episodes[0])
        .execute(&server.pool)
        .await
        .unwrap();
    sqlx::query("UPDATE tv_episodes SET air_date = CURRENT_DATE - $2::int WHERE id = $1")
        .bind(episodes[1])
        .bind(aired_days_ago)
        .execute(&server.pool)
        .await
        .unwrap();
    mark_watched(server, episodes[0]).await;
    sqlx::query(
        "UPDATE tv_watch_states SET watched_at = NOW() - INTERVAL '5 days',
                                    updated_at = NOW() - INTERVAL '5 days'
         WHERE episode_id = $1",
    )
    .bind(episodes[0])
    .execute(&server.pool)
    .await
    .unwrap();
    (episodes[0], episodes[1])
}

async fn seed_job(server: &TestServer, episode: Uuid, last: LastJob) {
    let result: Option<Value> = last.result.map(|r| serde_json::from_str(r).unwrap());
    sqlx::query(
        "INSERT INTO job_status (id, job_type, status, payload, result, created_at, updated_at,
                                 completed_at)
         VALUES ($1, $2, $3, $4, $5,
                 NOW() - make_interval(hours => $6), NOW() - make_interval(hours => $7),
                 CASE WHEN $3 IN ('completed', 'failed')
                      THEN NOW() - make_interval(hours => $7) END)",
    )
    .bind(Uuid::now_v7())
    .bind(JOB)
    .bind(last.status)
    .bind(json!({ "episode_id": episode }))
    .bind(result)
    .bind(last.created_hours_ago)
    .bind(last.updated_hours_ago)
    .execute(&server.pool)
    .await
    .unwrap();
}

async fn jobs_for(server: &TestServer, episode: Uuid) -> i64 {
    sqlx::query_scalar(
        "SELECT COUNT(*) FROM job_status WHERE job_type = $1 AND payload->>'episode_id' = $2",
    )
    .bind(JOB)
    .bind(episode.to_string())
    .fetch_one(&server.pool)
    .await
    .unwrap()
}

#[tokio::test]
async fn both_queue_paths_share_one_retry_policy() {
    let server = TestServer::start_api_only().await;
    server
        .admin_client()
        .patch_json(
            "/api/v1/admin/config",
            &json!({ "kobload_base_url": "http://127.0.0.1:9", "kobload_api_key": "k" }),
        )
        .await
        .assert_ok();
    let user: Uuid = sqlx::query_scalar("SELECT id FROM users WHERE role = 'admin' LIMIT 1")
        .fetch_one(&server.pool)
        .await
        .unwrap();

    let mut watch_targets = Vec::new();
    for case in CASES {
        let (_, next) = caught_up(&server, case.aired_days_ago).await;
        seed_job(&server, next, case.last).await;
        watch_targets.push((case, next));
    }
    DebridNewEpisodeWatchHandler::new(server.app_state.clone())
        .execute(Uuid::now_v7(), json!({}), CancellationToken::new())
        .await
        .unwrap();
    for (case, next) in watch_targets {
        assert_eq!(
            jobs_for(&server, next).await > 1,
            case.watch_queues,
            "new-episode watch: {}",
            case.label
        );
    }

    for case in CASES {
        let (current, next) = caught_up(&server, case.aired_days_ago).await;
        seed_job(&server, next, case.last).await;
        api::download_ahead::consider(&server.app_state, user, current).await;
        assert_eq!(
            jobs_for(&server, next).await > 1,
            case.mid_watch_queues,
            "mid-watch: {}",
            case.label
        );
    }
}

#[tokio::test]
async fn an_empty_search_before_the_air_date_is_no_failure_and_does_not_loop() {
    let rig = Rig::start(
        crate::common::debrid_upstream::UpstreamPlan::new(noise(1024)),
        "ep.mkv",
    )
    .await;
    rig.mount_search(json!([])).await;
    let server = &rig.server;
    let (_, next) = caught_up(server, Some(-2)).await;
    let watch = DebridNewEpisodeWatchHandler::new(server.app_state.clone());

    watch
        .execute(Uuid::now_v7(), json!({}), CancellationToken::new())
        .await
        .unwrap();
    assert_eq!(jobs_for(server, next).await, 1, "not queued before air");
    server
        .process_pending_jobs(&DebridDownloadAheadHandler::new(server.app_state.clone()))
        .await;
    let (status, error, result): (String, Option<String>, Option<Value>) = sqlx::query_as(
        "SELECT status, error, result FROM job_status
         WHERE job_type = $1 AND payload->>'episode_id' = $2",
    )
    .bind(JOB)
    .bind(next.to_string())
    .fetch_one(&server.pool)
    .await
    .unwrap();
    assert_eq!((status.as_str(), error), ("completed", None));
    assert_eq!(result.unwrap()["skipped"], "nothing streamable");

    watch
        .execute(Uuid::now_v7(), json!({}), CancellationToken::new())
        .await
        .unwrap();
    assert_eq!(jobs_for(server, next).await, 1, "requeued inside 12 hours");
}

#[tokio::test]
async fn the_mid_watch_preload_waits_for_the_early_release_window() {
    let server = TestServer::start_api_only().await;
    server
        .admin_client()
        .patch_json(
            "/api/v1/admin/config",
            &json!({ "kobload_base_url": "http://127.0.0.1:9", "kobload_api_key": "k" }),
        )
        .await
        .assert_ok();
    let user: Uuid = sqlx::query_scalar("SELECT id FROM users WHERE role = 'admin' LIMIT 1")
        .fetch_one(&server.pool)
        .await
        .unwrap();
    for (label, next_season, listed_in_days, queues) in [
        ("premiered, listed in 10 days", false, 10, false),
        ("premiered, listed in 3 days", false, 3, true),
        ("next season, premieres in 2 days", true, 2, false),
        ("next season, premieres in 10 days", true, 10, false),
    ] {
        let (current, same_season_next) = caught_up(&server, Some(-listed_in_days)).await;
        let next = if next_season {
            sqlx::query("DELETE FROM tv_episodes WHERE id = $1")
                .bind(same_season_next)
                .execute(&server.pool)
                .await
                .unwrap();
            let season = Uuid::now_v7();
            sqlx::query(
                "INSERT INTO tv_seasons (id, show_id, season_identifier, season_sort_order)
                 SELECT $1, s.show_id, '2', 2 FROM tv_episodes e
                 JOIN tv_seasons s ON s.id = e.season_id WHERE e.id = $2",
            )
            .bind(season)
            .bind(current)
            .execute(&server.pool)
            .await
            .unwrap();
            let episode = Uuid::now_v7();
            sqlx::query(
                "INSERT INTO tv_episodes (id, season_id, episode_identifier, episode_sort_order,
                                          air_date)
                 VALUES ($1, $2, '1', 1, CURRENT_DATE + $3::int)",
            )
            .bind(episode)
            .bind(season)
            .bind(listed_in_days)
            .execute(&server.pool)
            .await
            .unwrap();
            episode
        } else {
            same_season_next
        };
        assert_eq!(
            api::repositories::tv_episodes::next_episode_to_preload(&server.pool, current)
                .await
                .unwrap(),
            queues.then_some(next),
            "{label}"
        );
        api::download_ahead::consider(&server.app_state, user, current).await;
        assert_eq!(jobs_for(&server, next).await > 0, queues, "{label}");
    }
}

/// A league's season has always "premiered", and its future events really are in the future.
#[tokio::test]
async fn a_future_sport_event_is_not_preloaded() {
    let server = TestServer::start_api_only().await;
    let (current, next) = caught_up(&server, Some(-10)).await;
    sqlx::query(
        "UPDATE tv_shows SET content_category = 'sport'
         WHERE id = (SELECT s.show_id FROM tv_seasons s JOIN tv_episodes e ON e.season_id = s.id
                     WHERE e.id = $1)",
    )
    .bind(next)
    .execute(&server.pool)
    .await
    .unwrap();
    assert_eq!(
        api::repositories::tv_episodes::next_episode_to_preload(&server.pool, current)
            .await
            .unwrap(),
        None
    );
}
