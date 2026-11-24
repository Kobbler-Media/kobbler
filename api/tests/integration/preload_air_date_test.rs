//! A next episode listed well ahead is searched or grabbed only once its release is plausible:
//! 21 of 21 grabs made 4+ days before the listed date were fake. A season that has visibly
//! dropped ahead of its schedule is the exception.

use crate::common::debrid_rig::season_of;
use crate::common::*;
use crate::debrid_outcomes_test::{
    SessionSeed, admin_id, insert_session, kobload_server, mount_search, option, outcome_path,
    requests_to,
};
use api::jobs::handlers::DebridPrewarmHandler;
use reqwest::StatusCode;
use serde_json::json;
use uuid::Uuid;
use wiremock::matchers::{method, path_regex};
use wiremock::{Mock, MockServer, ResponseTemplate};

#[derive(Clone, Copy)]
struct Case {
    label: &'static str,
    listed_in_days: &'static [Option<i32>],
    playable: &'static [usize],
    on_disk: &'static [usize],
    second_season_from: Option<usize>,
    sport: bool,
    preloads: bool,
}

const BASE: Case = Case {
    label: "",
    listed_in_days: &[],
    playable: &[],
    on_disk: &[],
    second_season_from: None,
    sport: false,
    preloads: false,
};

const WATCHING: usize = 1;
const NEXT: usize = 2;

const CASES: &[Case] = &[
    Case {
        label: "weekly show, next in 10 days",
        listed_in_days: &[Some(-14), Some(-7), Some(10)],
        ..BASE
    },
    Case {
        label: "weekly show, next in 4 days",
        listed_in_days: &[Some(-14), Some(-3), Some(4)],
        ..BASE
    },
    Case {
        label: "next in 3 days",
        listed_in_days: &[Some(-14), Some(-4), Some(3)],
        preloads: true,
        ..BASE
    },
    Case {
        label: "next out today",
        listed_in_days: &[Some(-14), Some(-7), Some(0)],
        preloads: true,
        ..BASE
    },
    Case {
        label: "next has no listed date",
        listed_in_days: &[Some(-14), Some(-7), None],
        preloads: true,
        ..BASE
    },
    Case {
        label: "binge: the episode being watched is itself listed ahead",
        listed_in_days: &[Some(-7), Some(7), Some(14)],
        preloads: true,
        ..BASE
    },
    Case {
        label: "weekly show, the episode being watched came out a day early",
        listed_in_days: &[Some(-6), Some(1), Some(8)],
        ..BASE
    },
    Case {
        label: "weekly show, an episode with a file came out 3 days early",
        listed_in_days: &[Some(-14), Some(-7), Some(8), Some(3)],
        playable: &[3],
        ..BASE
    },
    Case {
        label: "binge: the episode being watched is listed two weeks out",
        listed_in_days: &[Some(7), Some(14), Some(21)],
        preloads: true,
        ..BASE
    },
    Case {
        label: "binge: a later episode already has a file",
        listed_in_days: &[Some(-14), Some(-7), Some(10), Some(17)],
        playable: &[3],
        preloads: true,
        ..BASE
    },
    Case {
        label: "binge: a later episode is already whole in the debrid cache",
        listed_in_days: &[Some(-14), Some(-7), Some(10), Some(17)],
        on_disk: &[3],
        preloads: true,
        ..BASE
    },
    Case {
        label: "a file for an aired episode proves nothing",
        listed_in_days: &[Some(-14), Some(-7), Some(10)],
        playable: &[0, 1],
        ..BASE
    },
    Case {
        label: "a copy of the next episode cannot vouch for itself",
        listed_in_days: &[Some(-14), Some(-7), Some(10)],
        on_disk: &[NEXT],
        ..BASE
    },
    Case {
        label: "does not walk past a far-off next episode to an undated one",
        listed_in_days: &[Some(-14), Some(-7), Some(10), None],
        ..BASE
    },
    Case {
        label: "an early season does not vouch for the next season",
        listed_in_days: &[Some(-7), Some(7), Some(14), Some(21)],
        second_season_from: Some(NEXT),
        ..BASE
    },
    Case {
        label: "next season premieres inside the window but has not started",
        listed_in_days: &[Some(-14), Some(-7), Some(2)],
        second_season_from: Some(NEXT),
        ..BASE
    },
    Case {
        label: "sport: a future event inside the window",
        listed_in_days: &[Some(-14), Some(-7), Some(2)],
        sport: true,
        ..BASE
    },
    Case {
        label: "sport: an event listed ahead does not vouch for the next",
        listed_in_days: &[Some(-7), Some(1), Some(8)],
        sport: true,
        ..BASE
    },
    Case {
        label: "sport: a past event",
        listed_in_days: &[Some(-14), Some(-7), Some(-1)],
        sport: true,
        preloads: true,
        ..BASE
    },
];

async fn show(server: &TestServer, case: &Case) -> Vec<Uuid> {
    let pool = &server.pool;
    let episodes = season_of(server, case.listed_in_days.len() as i32).await;
    for (episode, days) in episodes.iter().zip(case.listed_in_days) {
        sqlx::query("UPDATE tv_episodes SET air_date = CURRENT_DATE + $2::int WHERE id = $1")
            .bind(episode)
            .bind(days)
            .execute(pool)
            .await
            .unwrap();
    }
    if let Some(from) = case.second_season_from {
        let season = Uuid::now_v7();
        sqlx::query(
            "INSERT INTO tv_seasons (id, show_id, season_identifier, season_sort_order)
             SELECT $1, s.show_id, '2', 2 FROM tv_episodes e
             JOIN tv_seasons s ON s.id = e.season_id WHERE e.id = $2",
        )
        .bind(season)
        .bind(episodes[0])
        .execute(pool)
        .await
        .unwrap();
        sqlx::query("UPDATE tv_episodes SET season_id = $1 WHERE id = ANY($2)")
            .bind(season)
            .bind(&episodes[from..])
            .execute(pool)
            .await
            .unwrap();
    }
    if case.sport {
        sqlx::query(
            "UPDATE tv_shows SET content_category = 'sport'
             WHERE id = (SELECT s.show_id FROM tv_seasons s
                         JOIN tv_episodes e ON e.season_id = s.id WHERE e.id = $1)",
        )
        .bind(episodes[0])
        .execute(pool)
        .await
        .unwrap();
    }
    for &index in case.playable {
        library_file(server, episodes[index]).await;
    }
    for &index in case.on_disk {
        whole_in_debrid_cache(server, episodes[index]).await;
    }
    episodes
}

async fn library_file(server: &TestServer, episode: Uuid) {
    let file = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO tv_episode_files (id, file_path, format, available, duration_seconds)
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

async fn whole_in_debrid_cache(server: &TestServer, episode: Uuid) {
    let user = admin_id(server).await;
    let path = format!("/cache/{episode}.mkv");
    let key = format!("disk-{episode}");
    let mut seed = SessionSeed::episode(episode, &key);
    seed.cache_path = Some(&path);
    insert_session(server, user, seed).await;
    sqlx::query(
        "INSERT INTO debrid_cache_files (file_path, file_size, expires_at, release_key, filename,
                                         completed_at)
         VALUES ($1, 1, NOW() + INTERVAL '1 day', $2, 'ep.mkv', NOW())",
    )
    .bind(&path)
    .bind(&key)
    .execute(&server.pool)
    .await
    .unwrap();
}

async fn download_ahead_jobs(server: &TestServer, episode: Uuid) -> i64 {
    sqlx::query_scalar(
        "SELECT COUNT(*) FROM job_status
         WHERE job_type = 'debrid_download_ahead' AND payload->>'episode_id' = $1",
    )
    .bind(episode.to_string())
    .fetch_one(&server.pool)
    .await
    .unwrap()
}

async fn play(server: &TestServer, user: Uuid, episode: Uuid) {
    let key = format!("played-{}", Uuid::now_v7());
    let session = insert_session(server, user, SessionSeed::episode(episode, &key)).await;
    server
        .admin_client()
        .post_json(&outcome_path(session), &json!({ "ok": true }))
        .await
        .assert_status(StatusCode::NO_CONTENT);
    server
        .process_pending_jobs(&DebridPrewarmHandler::new(server.app_state.clone()))
        .await;
}

async fn searches(mock: &MockServer) -> usize {
    requests_to(mock, "/stream/search").await.len()
}

#[tokio::test]
async fn both_preloads_search_only_when_the_next_release_is_plausible() {
    let (server, mock) = kobload_server().await;
    mount_search(&mock, vec![]).await;
    let user = admin_id(&server).await;
    let mut wrong = Vec::new();
    for case in CASES {
        let episodes = show(&server, case).await;

        api::download_ahead::consider(&server.app_state, user, episodes[WATCHING]).await;
        let grabbed = &episodes[NEXT..];
        let mut queued = 0;
        for episode in grabbed {
            queued += download_ahead_jobs(&server, *episode).await;
        }
        if (queued > 0) != case.preloads {
            wrong.push(format!("download ahead: {} (queued {queued})", case.label));
        }

        let before = searches(&mock).await;
        play(&server, user, episodes[WATCHING]).await;
        let searched = searches(&mock).await - before;
        if (searched > 0) != case.preloads {
            wrong.push(format!("prewarm: {} (searched {searched})", case.label));
        }
    }
    assert!(wrong.is_empty(), "{}", wrong.join("\n"));
}

async fn mount_kobload(mock: &MockServer, found: bool) {
    mock.reset().await;
    Mock::given(method("POST"))
        .and(path_regex("/api/v1/stream/outcome"))
        .respond_with(ResponseTemplate::new(204))
        .mount(mock)
        .await;
    let options = if found {
        vec![option("anyhash", None, None)]
    } else {
        vec![]
    };
    mount_search(mock, options).await;
}

#[tokio::test]
async fn a_repeated_play_follows_the_shared_retry_policy() {
    let (server, mock) = kobload_server().await;
    let user = admin_id(&server).await;
    for (label, found, hours_ago, searches_again) in [
        ("nothing found, 1h ago", false, 1, false),
        ("nothing found, 3h ago", false, 3, true),
        ("found, 3h ago", true, 3, false),
        ("found, 13h ago", true, 13, true),
    ] {
        mount_kobload(&mock, found).await;
        let case = Case {
            listed_in_days: &[Some(-14), Some(-7), Some(-1)],
            ..BASE
        };
        let episodes = show(&server, &case).await;

        play(&server, user, episodes[WATCHING]).await;
        play(&server, user, episodes[WATCHING]).await;
        assert_eq!(searches(&mock).await, 1, "{label}: replay searched again");

        sqlx::query(
            "UPDATE job_status SET created_at = NOW() - make_interval(hours => $2),
                                   updated_at = NOW() - make_interval(hours => $2),
                                   completed_at = NOW() - make_interval(hours => $2)
             WHERE job_type = 'debrid_prewarm' AND payload->>'episode_id' = $1",
        )
        .bind(episodes[NEXT].to_string())
        .bind(hours_ago)
        .execute(&server.pool)
        .await
        .unwrap();
        play(&server, user, episodes[WATCHING]).await;
        assert_eq!(
            searches(&mock).await,
            if searches_again { 2 } else { 1 },
            "{label}"
        );
    }
}
