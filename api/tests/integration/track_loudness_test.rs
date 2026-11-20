use crate::common::analyze::*;
use crate::common::audiobooks::{Audio, file, seed_audiobook};
use crate::common::debrid_rig::{ITA_ENG_FIXTURE, downloaded_session, process_download};
use crate::common::*;
use api::jobs::worker::JobHandler;
use api::media_cache::PIPELINE_VERSION;
use api::repositories::debrid_files;
use api::repositories::file_loudness::{self, FileKind};
use serde_json::{Value, json};
use std::path::Path;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

const KINDS: [&str; 2] = ["movie", "episode"];
const ITA: i64 = 1;
const ENG: i64 = 2;

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

/// A file with an Italian dub flagged default ahead of its English original, its tracks
/// recorded as an import's probe records them.
async fn seed_dubbed(server: &TestServer, kind: &str, cache_version: i32) -> Uuid {
    let file_id = match kind {
        "movie" => seed_analyzable_movie_from(server, cache_version, ITA_ENG_FIXTURE).await,
        _ => seed_analyzable_episode_from(server, cache_version, ITA_ENG_FIXTURE).await,
    };
    let (_, path) = stored(server, kind, file_id).await;
    let probe = api::ffmpeg::probe::probe(Path::new(&path)).await.unwrap();
    api::ffmpeg::persist::persist_probe_result(&server.pool, file_id, kind, &probe)
        .await
        .unwrap();
    file_id
}

fn table(kind: &str) -> &'static str {
    match kind {
        "movie" => "movie_files",
        _ => "tv_episode_files",
    }
}

async fn stored(server: &TestServer, kind: &str, file_id: Uuid) -> (Option<Value>, String) {
    sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "SELECT audio_loudness, file_path FROM {} WHERE id = $1",
        table(kind)
    )))
    .bind(file_id)
    .fetch_one(&server.pool)
    .await
    .unwrap()
}

/// Every audio stream of `path` as `measure_loudness` reads it: (stream_index, lufs, peak).
fn measured(path: impl AsRef<Path>) -> Vec<(i64, f64, f64)> {
    kob_ffmpeg::audio::measure_loudness(path.as_ref(), None)
        .unwrap()
        .into_iter()
        .map(|t| {
            (
                i64::from(t.stream_index),
                t.loudness.integrated_lufs.unwrap(),
                t.loudness.downmix_peak_db.unwrap(),
            )
        })
        .collect()
}

/// `tracks` (stored rows or a session's `audio_tracks`) carry each stream's own levels.
fn assert_each_track_its_own(what: &str, tracks: &Value, path: impl AsRef<Path>) {
    let expected = measured(path);
    assert_eq!(
        expected.iter().map(|e| e.0).collect::<Vec<_>>(),
        [ITA, ENG],
        "the fixture is a dub ahead of its original"
    );
    assert!(
        (expected[0].2 - expected[1].2).abs() > 1.0,
        "the fixture's tracks must differ to tell them apart: {expected:?}"
    );
    for (stream, lufs, peak) in expected {
        let track = tracks
            .as_array()
            .unwrap_or_else(|| panic!("{what}: no tracks in {tracks}"))
            .iter()
            .find(|t| t["stream_index"] == stream)
            .unwrap_or_else(|| panic!("{what}: no stream {stream} in {tracks}"));
        let got = |k: &str| {
            track[k]
                .as_f64()
                .unwrap_or_else(|| panic!("{what}: stream {stream} has no {k}: {tracks}"))
        };
        assert!(
            (got("lufs") - lufs).abs() < 0.05,
            "{what}: stream {stream} lufs: {tracks}"
        );
        assert!(
            (got("downmix_peak_db") - peak).abs() < 0.05,
            "{what}: stream {stream} peak: {tracks}"
        );
    }
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

fn assert_no_file_level_loudness(what: &str, session: &Value) {
    assert!(session.get("lufs").is_none(), "{what}: {session}");
    assert!(
        session.get("downmix_peak_db").is_none(),
        "{what}: {session}"
    );
}

#[tokio::test]
async fn analysis_measures_every_audio_track_and_sessions_serve_each_its_own() {
    for kind in KINDS {
        let server = enabled_server().await;
        let file_id = seed_dubbed(&server, kind, 0).await;

        analyzer(&server)
            .execute(
                Uuid::now_v7(),
                json!({ "file_id": file_id, "media_type": kind, "triggered_by": "manual" }),
                CancellationToken::new(),
            )
            .await
            .unwrap();

        let (tracks, path) = stored(&server, kind, file_id).await;
        let tracks = tracks.unwrap_or_else(|| panic!("{kind}: analysis stored no loudness"));
        assert_each_track_its_own(&format!("{kind} stored"), &tracks, &path);

        let session = session(&server, kind, file_id).await;
        assert_each_track_its_own(&format!("{kind} session"), &session["audio_tracks"], &path);
        assert_no_file_level_loudness(kind, &session);
    }
}

#[tokio::test]
async fn switching_audio_tracks_keeps_serving_each_tracks_levels() {
    let server = enabled_server().await;
    let file_id = seed_dubbed(&server, "movie", 0).await;
    analyzer(&server)
        .execute(
            Uuid::now_v7(),
            json!({ "file_id": file_id, "media_type": "movie", "triggered_by": "manual" }),
            CancellationToken::new(),
        )
        .await
        .unwrap();
    let session_id = session(&server, "movie", file_id).await["session_id"]
        .as_str()
        .unwrap()
        .to_string();

    let resp = server
        .admin_client()
        .post_json(
            &format!("/api/v1/admin/playback/sessions/{session_id}/switch-audio"),
            &json!({ "audio_stream_index": ENG }),
        )
        .await;
    resp.assert_ok();
    let switched: Value = resp.json();

    let (_, path) = stored(&server, "movie", file_id).await;
    assert_each_track_its_own("switched", &switched["audio_tracks"], &path);
    assert_no_file_level_loudness("switched", &switched);
}

#[tokio::test]
async fn files_analysed_before_per_track_loudness_are_re_measured_by_the_fill() {
    for kind in KINDS {
        let server = enabled_server().await;
        let file_id = seed_dubbed(&server, kind, PIPELINE_VERSION).await;
        sqlx::query(sqlx::AssertSqlSafe(format!(
            "UPDATE {} SET picture_checked_at = NOW() WHERE id = $1",
            table(kind)
        )))
        .bind(file_id)
        .execute(&server.pool)
        .await
        .unwrap();

        let backfill = analyze_backfill(&server);
        let walked = backfill
            .execute(Uuid::now_v7(), json!({}), CancellationToken::new())
            .await
            .unwrap();
        assert_eq!(walked["movie_found"], 0, "{kind}: {walked}");
        assert_eq!(walked["episode_found"], 0, "{kind}: {walked}");
        assert_eq!(walked["loudness_gaps_enqueued"], 1, "{kind}: {walked}");
        assert_eq!(
            queued(&server, "media_analyze").await,
            0,
            "{kind}: a re-measure must not queue ahead of new imports' analysis"
        );

        server.process_pending_jobs(&loudness_filler(&server)).await;

        let (tracks, path) = stored(&server, kind, file_id).await;
        let tracks = tracks.unwrap_or_else(|| panic!("{kind}: the fill stored nothing"));
        assert_each_track_its_own(kind, &tracks, &path);

        let again = backfill
            .execute(Uuid::now_v7(), json!({}), CancellationToken::new())
            .await
            .unwrap();
        assert_eq!(again["loudness_gaps_found"], 0, "{kind}: {again}");
    }
}

async fn dubbed_download(server: &TestServer, info_hash: &str) -> (Uuid, std::path::PathBuf) {
    let movie: Value = server
        .admin_client()
        .post_json(
            "/api/v1/admin/movies",
            &json!({ "title": "Dubbed", "year": 2024 }),
        )
        .await
        .json();
    let movie_id: Uuid = movie["id"].as_str().unwrap().parse().unwrap();
    let path = Path::new(&server.storage_root()).join(format!("{info_hash}.mkv"));
    std::fs::write(&path, ITA_ENG_FIXTURE).unwrap();
    let session = downloaded_session(server, "movie", movie_id, info_hash, &path).await;
    (session, path)
}

async fn session_loudness(server: &TestServer, session: Uuid) -> Option<Value> {
    sqlx::query_scalar("SELECT audio_loudness FROM debrid_stream_sessions WHERE id = $1")
        .bind(session)
        .fetch_one(&server.pool)
        .await
        .unwrap()
}

#[tokio::test]
async fn a_debrid_download_is_measured_per_track_by_its_analysis() {
    let server = enabled_server().await;
    let (session, path) = dubbed_download(&server, "dubbedhash").await;

    process_download(&server, session, &path).await;

    let tracks = session_loudness(&server, session)
        .await
        .expect("analysis stored no loudness on the session");
    assert_each_track_its_own("debrid analysis", &tracks, &path);
}

#[tokio::test]
async fn debrid_downloads_measured_before_per_track_loudness_are_re_measured_by_the_fill() {
    let server = enabled_server().await;
    let (session, path) = dubbed_download(&server, "olddubhash").await;
    let (_, movie_id): (String, Uuid) =
        sqlx::query_as("SELECT media_type, media_id FROM debrid_stream_sessions WHERE id = $1")
            .bind(session)
            .fetch_one(&server.pool)
            .await
            .unwrap();
    debrid_files::upsert(&server.pool, &path.to_string_lossy(), "movie", movie_id)
        .await
        .unwrap();
    sqlx::query(
        "INSERT INTO debrid_cache_files (file_path, file_size, expires_at)
         VALUES ($1, $2, NOW() + INTERVAL '1 day')",
    )
    .bind(path.to_string_lossy())
    .bind(ITA_ENG_FIXTURE.len() as i64)
    .execute(&server.pool)
    .await
    .unwrap();
    let (partial, _) = dubbed_download(&server, "stillloading").await;
    let twin = downloaded_session(&server, "movie", movie_id, "olddubhash", &path).await;

    let walked = analyze_backfill(&server)
        .execute(Uuid::now_v7(), json!({}), CancellationToken::new())
        .await
        .unwrap();
    assert_eq!(walked["loudness_gaps_enqueued"], 1, "{walked}");
    server.process_pending_jobs(&loudness_filler(&server)).await;

    for s in [session, twin] {
        let tracks = session_loudness(&server, s)
            .await
            .unwrap_or_else(|| panic!("session {s} sharing the download got no loudness"));
        assert_each_track_its_own("debrid fill", &tracks, &path);
    }
    assert_eq!(
        session_loudness(&server, partial).await,
        None,
        "a download still in progress is not measured"
    );
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
    assert_eq!(walked["loudness_gaps_enqueued"], 1, "{walked}");
    server.process_pending_jobs(&loudness_filler(&server)).await;

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
        (peak - measured(&path)[0].2).abs() < 0.05,
        "stored peak {peak}"
    );
}

#[tokio::test]
async fn what_is_being_watched_is_re_measured_first() {
    let server = enabled_server().await;
    let movie = seed_analyzable_movie(&server, PIPELINE_VERSION).await;
    let watched = seed_analyzable_episode(&server, PIPELINE_VERSION).await;
    let newest = seed_analyzable_episode(&server, PIPELINE_VERSION).await;
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

    let order = file_loudness::list_needing_loudness(&server.pool, 10)
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
async fn files_gone_or_never_analysed_are_not_re_measured() {
    let server = enabled_server().await;
    let movie = seed_analyzable_movie(&server, PIPELINE_VERSION).await;
    let episode = seed_analyzable_episode(&server, PIPELINE_VERSION).await;
    seed_analyzable_movie(&server, 0).await;
    let (_, audiobook) =
        seed_audiobook(&server, "Gone", &[file("01.mp3", 3.0, Audio::Tone(440.0))]).await;
    sqlx::query("UPDATE audiobook_files SET lufs = -20.0, available = false WHERE id = $1")
        .bind(audiobook[0])
        .execute(&server.pool)
        .await
        .unwrap();
    for (table, id) in [("movie_files", movie), ("tv_episode_files", episode)] {
        sqlx::query(sqlx::AssertSqlSafe(format!(
            "UPDATE {table} SET available = false WHERE id = $1"
        )))
        .bind(id)
        .execute(&server.pool)
        .await
        .unwrap();
    }

    let order = file_loudness::list_needing_loudness(&server.pool, 10)
        .await
        .unwrap();
    assert_eq!(order, vec![]);
}
