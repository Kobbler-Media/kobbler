use crate::common::debrid_rig::*;
use crate::common::debrid_upstream::{UpstreamPlan, WholeBody};
use crate::common::*;
use api::jobs::handlers::DebridKeepHandler;
use reqwest::StatusCode;
use serde_json::{Value, json};
use std::path::PathBuf;
use uuid::Uuid;

struct Library {
    _dir: tempfile::TempDir,
    root: PathBuf,
}

async fn library(rig: &Rig) -> Library {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().to_path_buf();
    for media_type in ["movie", "tv"] {
        let path = root.join(media_type);
        std::fs::create_dir_all(&path).unwrap();
        sqlx::query(
            "INSERT INTO media_roots (id, path, media_type, is_default) VALUES ($1, $2, $3, TRUE)",
        )
        .bind(Uuid::now_v7())
        .bind(path.to_string_lossy().to_string())
        .bind(media_type)
        .execute(&rig.server.pool)
        .await
        .unwrap();
    }
    Library { _dir: dir, root }
}

async fn episode(rig: &Rig) -> Uuid {
    let (show, season, episode) = (Uuid::now_v7(), Uuid::now_v7(), Uuid::now_v7());
    sqlx::query("INSERT INTO tv_shows (id, title, year) VALUES ($1, 'Kept Show', 2024)")
        .bind(show)
        .execute(&rig.server.pool)
        .await
        .unwrap();
    sqlx::query(
        "INSERT INTO tv_seasons (id, show_id, season_identifier, season_sort_order, is_monitored, is_special)
         VALUES ($1, $2, '1', 1, TRUE, FALSE)",
    )
    .bind(season)
    .bind(show)
    .execute(&rig.server.pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO tv_episodes (id, season_id, episode_identifier, episode_sort_order, title)
         VALUES ($1, $2, '3', 3, 'The Kept One')",
    )
    .bind(episode)
    .bind(season)
    .execute(&rig.server.pool)
    .await
    .unwrap();
    episode
}

fn keep_url(session: Uuid) -> String {
    format!("/api/v1/admin/stream/{session}/keep")
}

async fn keep(rig: &Rig, session: Uuid) -> Value {
    let resp = rig
        .server
        .admin_client()
        .post_json(&keep_url(session), &json!({}))
        .await;
    resp.assert_status(StatusCode::ACCEPTED);
    resp.json()
}

async fn keep_status(rig: &Rig, session: Uuid) -> TestResponse {
    rig.server.admin_client().get(&keep_url(session)).await
}

async fn run_keep_jobs(rig: &Rig) {
    let handler = DebridKeepHandler::new(rig.server.app_state.clone());
    rig.server.process_pending_jobs(&handler).await;
}

async fn library_file(rig: &Rig, media_type: &str, media_id: Uuid) -> Option<String> {
    let sql = match media_type {
        "movie" => "SELECT file_path FROM movie_files WHERE movie_id = $1",
        _ => {
            "SELECT f.file_path FROM tv_episode_files f
             JOIN tv_episode_file_episodes l ON l.episode_file_id = f.id
             WHERE l.episode_id = $1"
        }
    };
    sqlx::query_scalar(sql)
        .bind(media_id)
        .fetch_optional(&rig.server.pool)
        .await
        .unwrap()
}

#[tokio::test]
async fn keeping_a_stream_imports_it_as_a_library_file() {
    enum Download {
        Finished,
        StoppedByTheReaper,
    }
    let cases = [
        ("movie", Download::Finished),
        ("movie", Download::StoppedByTheReaper),
        ("episode", Download::Finished),
    ];
    for (i, (media_type, download)) in cases.into_iter().enumerate() {
        let mut plan = UpstreamPlan::new(ITA_ENG_FIXTURE.to_vec());
        if matches!(download, Download::StoppedByTheReaper) {
            plan.whole = WholeBody::Stall { first: 20_000 };
        }
        let rig = Rig::start(plan, "film.mkv").await;
        let lib = library(&rig).await;
        let media_id = match media_type {
            "movie" => rig.movie().await,
            _ => episode(&rig).await,
        };
        let hash = format!("keephash{i}");
        let mut body = rig.resolve_body(&hash, media_id, direct_play_caps());
        body["media_type"] = json!(media_type);
        let session = session_id_of(&rig.resolve_with(&body).await);
        match download {
            Download::Finished => rig.wait_for_complete(&hash).await,
            Download::StoppedByTheReaper => {
                rig.wait_for_local(&hash, 20_000).await;
                let live = rig
                    .server
                    .app_state
                    .playback_sessions
                    .get(&session)
                    .unwrap();
                live.download_cancel.as_ref().unwrap().cancel();
            }
        }

        let queued = keep(&rig, session).await;
        assert!(queued["job_id"].is_string(), "{queued}");
        assert_eq!(queued["state"], "queued", "{queued}");
        run_keep_jobs(&rig).await;

        let status: Value = keep_status(&rig, session).await.json();
        assert_eq!(
            status,
            json!({ "state": "done", "progress": 1.0 }),
            "{media_type}"
        );
        let kept = library_file(&rig, media_type, media_id)
            .await
            .unwrap_or_else(|| panic!("{media_type}: no library file"));
        assert!(
            kept.starts_with(&lib.root.to_string_lossy().to_string()),
            "{kept}"
        );
        assert!(std::fs::read(&kept).unwrap() == ITA_ENG_FIXTURE);
        assert!(
            !rig.cache_file(&hash).exists(),
            "the cache copy stayed behind"
        );
        let cache_rows: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM debrid_cache_files WHERE info_hash = $1")
                .bind(&hash)
                .fetch_one(&rig.server.pool)
                .await
                .unwrap();
        assert_eq!(cache_rows, 0);

        let played = play(&rig.server, session, "bytes=0-").await;
        assert!(played.body == ITA_ENG_FIXTURE, "the session lost its file");

        let again = keep(&rig, session).await;
        assert_eq!(again["job_id"], queued["job_id"], "keep is idempotent");
        assert_eq!(again["state"], "done");
    }
}

#[tokio::test]
async fn a_failed_keep_says_why_and_can_be_retried() {
    let rig = Rig::start(UpstreamPlan::new(ITA_ENG_FIXTURE.to_vec()), "film.mkv").await;
    let _lib = library(&rig).await;
    let movie = rig.movie().await;
    let session = session_id_of(
        &rig.resolve_with(&rig.resolve_body("failkeep", movie, direct_play_caps()))
            .await,
    );
    keep_status(&rig, session)
        .await
        .assert_status(StatusCode::NOT_FOUND);
    rig.wait_for_complete("failkeep").await;
    sqlx::query("DELETE FROM movies WHERE id = $1")
        .bind(movie)
        .execute(&rig.server.pool)
        .await
        .unwrap();

    let first = keep(&rig, session).await;
    run_keep_jobs(&rig).await;
    let status: Value = keep_status(&rig, session).await.json();
    assert_eq!(status["state"], "failed", "{status}");
    assert!(status["error"].is_string(), "{status}");

    let retry = keep(&rig, session).await;
    assert_eq!(retry["state"], "queued");
    assert_ne!(retry["job_id"], first["job_id"]);
}

/// The id `stream/recent` lists is the one keep takes.
#[tokio::test]
async fn keep_takes_the_id_recent_streams_list() {
    let rig = Rig::start(UpstreamPlan::new(ITA_ENG_FIXTURE.to_vec()), "film.mkv").await;
    let session = rig.resolve("recentkeep").await;
    let recent: Value = rig
        .server
        .admin_client()
        .get("/api/v1/admin/stream/recent")
        .await
        .json();
    let listed: Uuid = recent["sessions"]
        .as_array()
        .and_then(|sessions| sessions.first())
        .and_then(|item| item["id"].as_str())
        .unwrap_or_else(|| panic!("{recent}"))
        .parse()
        .unwrap();
    assert_eq!(listed, session);
    keep(&rig, listed).await;
}
