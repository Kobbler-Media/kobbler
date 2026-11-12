use crate::common::analyze::*;
use crate::common::audiobooks::{Audio, file, seed_audiobook};
use crate::common::*;
use api::jobs::worker::JobHandler;
use api::media_cache::PIPELINE_VERSION;
use api::repositories::file_loudness::{self, FileKind};
use serde_json::{Value, json};
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

const KINDS: [&str; 2] = ["movie", "episode"];

async fn enabled_server() -> TestServer {
    let server = TestServer::start_api_only().await;
    server
        .admin_client()
        .patch_json(
            "/api/v1/admin/config",
            &json!({ "enrichment_enabled": true, "skip_detect_mode": "off" }),
        )
        .await
        .assert_ok();
    server
}

async fn seed(server: &TestServer, kind: &str, cache_version: i32) -> Uuid {
    match kind {
        "movie" => seed_analyzable_movie(server, cache_version).await,
        _ => seed_analyzable_episode(server, cache_version).await,
    }
}

fn table(kind: &str) -> &'static str {
    match kind {
        "movie" => "movie_files",
        _ => "tv_episode_files",
    }
}

async fn stored(
    server: &TestServer,
    kind: &str,
    file_id: Uuid,
) -> (Option<f64>, Option<f64>, String) {
    sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "SELECT lufs, downmix_peak_db, file_path FROM {} WHERE id = $1",
        table(kind)
    )))
    .bind(file_id)
    .fetch_one(&server.pool)
    .await
    .unwrap()
}

fn measured_peak(path: &str) -> f64 {
    kob_ffmpeg::audio::measure_loudness(std::path::Path::new(path), None)
        .unwrap()
        .downmix_peak_db
        .unwrap()
}

async fn session(server: &TestServer, kind: &str, file_id: Uuid) -> Value {
    server
        .admin_client()
        .post_json(
            "/api/v1/admin/playback/sessions",
            &json!({
                "media_type": kind,
                "file_id": file_id.to_string(),
                "capabilities": { "video_codecs": ["h264"], "audio_codecs": ["aac"], "containers": ["mkv", "matroska", "webm"] }
            }),
        )
        .await
        .json()
}

#[tokio::test]
async fn analysis_stores_the_downmix_peak_beside_lufs_and_sessions_serve_both() {
    for kind in KINDS {
        let server = enabled_server().await;
        let file_id = seed(&server, kind, 0).await;

        analyzer(&server)
            .execute(
                Uuid::now_v7(),
                json!({ "file_id": file_id, "media_type": kind, "triggered_by": "manual" }),
                CancellationToken::new(),
            )
            .await
            .unwrap();

        let (lufs, peak, path) = stored(&server, kind, file_id).await;
        let peak = peak.unwrap_or_else(|| panic!("{kind}: analysis stored no downmix peak"));
        assert!(lufs.is_some(), "{kind}: analysis stored no lufs");
        assert!(
            (peak - measured_peak(&path)).abs() < 0.05,
            "{kind}: stored peak {peak} is not the file's"
        );
        let session = session(&server, kind, file_id).await;
        assert_eq!(session["lufs"], json!(lufs), "{kind}: {session}");
        assert_eq!(session["downmix_peak_db"], json!(peak), "{kind}: {session}");
    }
}

#[tokio::test]
async fn files_analysed_before_the_peak_existed_get_it_from_the_gap_backfill() {
    for kind in KINDS {
        let server = enabled_server().await;
        let file_id = seed(&server, kind, PIPELINE_VERSION).await;
        sqlx::query(sqlx::AssertSqlSafe(format!(
            "UPDATE {} SET lufs = -20.0, picture_checked_at = NOW() WHERE id = $1",
            table(kind)
        )))
        .bind(file_id)
        .execute(&server.pool)
        .await
        .unwrap();

        let before = session(&server, kind, file_id).await;
        assert_eq!(before["lufs"], json!(-20.0), "{kind}: {before}");
        assert_eq!(before["downmix_peak_db"], Value::Null, "{kind}: {before}");

        let backfill = analyze_backfill(&server);
        let walked = backfill
            .execute(Uuid::now_v7(), json!({}), CancellationToken::new())
            .await
            .unwrap();
        assert_eq!(walked["movie_found"], 0, "{kind}: {walked}");
        assert_eq!(walked["episode_found"], 0, "{kind}: {walked}");
        assert_eq!(walked["peak_gaps_enqueued"], 1, "{kind}: {walked}");
        assert_eq!(
            queued(&server, "media_analyze").await,
            0,
            "{kind}: a re-measure must not queue ahead of new imports' analysis"
        );

        server.process_pending_jobs(&peak_filler(&server)).await;

        let (lufs, peak, path) = stored(&server, kind, file_id).await;
        assert_eq!(lufs, Some(-20.0), "{kind}: the gap fill leaves lufs alone");
        let peak = peak.unwrap_or_else(|| panic!("{kind}: gap fill stored no peak"));
        assert!(
            (peak - measured_peak(&path)).abs() < 0.05,
            "{kind}: stored peak {peak} is not the file's"
        );
        let after = session(&server, kind, file_id).await;
        assert_eq!(after["downmix_peak_db"], json!(peak), "{kind}: {after}");

        let again = backfill
            .execute(Uuid::now_v7(), json!({}), CancellationToken::new())
            .await
            .unwrap();
        assert_eq!(again["peak_gaps_found"], 0, "{kind}: {again}");
    }
}

async fn queued(server: &TestServer, job_type: &str) -> i64 {
    sqlx::query_scalar("SELECT COUNT(*) FROM job_status WHERE job_type = $1")
        .bind(job_type)
        .fetch_one(&server.pool)
        .await
        .unwrap()
}

#[tokio::test]
async fn audiobook_files_get_their_peak_and_the_player_is_told_both_figures() {
    let server = enabled_server().await;
    let (book_id, files) = seed_audiobook(
        &server,
        "Levels",
        &[file("01.mp3", 3.0, Audio::Tone(440.0))],
    )
    .await;
    sqlx::query("UPDATE audiobook_files SET lufs = -19.0 WHERE id = $1")
        .bind(files[0])
        .execute(&server.pool)
        .await
        .unwrap();

    let walked = analyze_backfill(&server)
        .execute(Uuid::now_v7(), json!({}), CancellationToken::new())
        .await
        .unwrap();
    assert_eq!(walked["peak_gaps_enqueued"], 1, "{walked}");
    server.process_pending_jobs(&peak_filler(&server)).await;

    let listed: Value = server
        .admin_client()
        .get(&format!("/api/v1/admin/books/{book_id}/audio-files"))
        .await
        .json();
    assert_eq!(listed[0]["lufs"], json!(-19.0), "{listed}");
    let peak = listed[0]["downmix_peak_db"]
        .as_f64()
        .unwrap_or_else(|| panic!("no peak served: {listed}"));
    let path: String = sqlx::query_scalar("SELECT file_path FROM audiobook_files WHERE id = $1")
        .bind(files[0])
        .fetch_one(&server.pool)
        .await
        .unwrap();
    assert!(
        (peak - measured_peak(&path)).abs() < 0.05,
        "stored peak {peak}"
    );
}

#[tokio::test]
async fn what_is_being_watched_is_re_measured_first() {
    let server = enabled_server().await;
    let movie = seed(&server, "movie", PIPELINE_VERSION).await;
    let watched = seed(&server, "episode", PIPELINE_VERSION).await;
    let newest = seed(&server, "episode", PIPELINE_VERSION).await;
    for (kind, id) in [("movie", movie), ("episode", watched), ("episode", newest)] {
        sqlx::query(sqlx::AssertSqlSafe(format!(
            "UPDATE {} SET lufs = -20.0 WHERE id = $1",
            table(kind)
        )))
        .bind(id)
        .execute(&server.pool)
        .await
        .unwrap();
    }
    let user_id: Uuid = sqlx::query_scalar("SELECT id FROM users LIMIT 1")
        .fetch_one(&server.pool)
        .await
        .unwrap();
    sqlx::query(
        "INSERT INTO tv_watch_states (episode_id, user_id, position_seconds)
         SELECT episode_id, $2, 60 FROM tv_episode_file_episodes WHERE episode_file_id = $1",
    )
    .bind(watched)
    .bind(user_id)
    .execute(&server.pool)
    .await
    .unwrap();

    let order = file_loudness::list_needing_downmix_peak(&server.pool, 10)
        .await
        .unwrap();
    assert_eq!(
        order,
        vec![
            (FileKind::Episode, watched),
            (FileKind::Episode, newest),
            (FileKind::Movie, movie),
        ]
    );
}

#[tokio::test]
async fn files_gone_from_disk_are_not_re_measured() {
    let server = enabled_server().await;
    let movie = seed(&server, "movie", PIPELINE_VERSION).await;
    let episode = seed(&server, "episode", PIPELINE_VERSION).await;
    let (_, audiobook) =
        seed_audiobook(&server, "Gone", &[file("01.mp3", 3.0, Audio::Tone(440.0))]).await;
    for (table, id) in [
        ("movie_files", movie),
        ("tv_episode_files", episode),
        ("audiobook_files", audiobook[0]),
    ] {
        sqlx::query(sqlx::AssertSqlSafe(format!(
            "UPDATE {table} SET lufs = -20.0, available = false WHERE id = $1"
        )))
        .bind(id)
        .execute(&server.pool)
        .await
        .unwrap();
    }

    let order = file_loudness::list_needing_downmix_peak(&server.pool, 10)
        .await
        .unwrap();
    assert_eq!(order, vec![]);
}
