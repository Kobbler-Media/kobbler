//! Integration regression test for the V2 sibling-readiness gate.
//!
//! Pre-fix: `sibling_ready` required `audio_peaks.bin` in every sibling's
//! manifest. `media_analyze` never writes that sidecar for video files
//! (audiobook-only, see the comment at `media_analyze.rs:717-721`), so every
//! episode's manifest failed the gate, every `skip_detect` returned
//! `{"status": "deferred", ...}`, the cross-season detect never fired, and
//! zero rows landed in `episode_skip_segments`. Observed on Leftovers S3.
//!
//! This test is the gap the bug shipped through: there was no end-to-end
//! check that a V2 run on a season with prepared video-shaped manifests
//! actually passes the gate. Two episodes is enough to trigger the same
//! circular-wait pattern as eight — the last handler to run must not defer.
//!
//! We don't assert a specific segment count because mocking realistic
//! chromaprint/vhash bytes that would yield cross-episode matches is a
//! rabbit hole. The bug was "deferred forever", so asserting the response
//! is a non-deferred outcome is the tight regression guard.

use crate::common::*;
use api::jobs::handlers::SkipDetectHandler;
use api::jobs::worker::JobHandler;
use api::media_cache::episode_cache::{MediaKind, episode_cache_dir};
use api::media_cache::writers::{
    ArtefactInfo, new_manifest_at, write_manifest_at, write_sidecar_at,
};
use api::media_cache::{Branch, PIPELINE_VERSION};
use api::repositories::{tv_episode_files, video_chapters};
use api::runtime_config::RuntimeConfigUpdate;
use serde_json::json;
use sqlx::PgPool;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

/// Seed a season with `count` episode files. Minimal fields — just enough
/// for `lookup_season_for_file` + `list_file_ids_by_season` to find them.
async fn seed_season(pool: &PgPool, count: usize, duration_seconds: f64) -> (Uuid, Vec<Uuid>) {
    let show_id = Uuid::now_v7();
    sqlx::query("INSERT INTO tv_shows (id, title) VALUES ($1, 'V2 Gate Test')")
        .bind(show_id)
        .execute(pool)
        .await
        .unwrap();

    let season_id = Uuid::now_v7();
    sqlx::query(
        r#"INSERT INTO tv_seasons (id, show_id, season_identifier, season_sort_order, is_monitored, is_special)
           VALUES ($1, $2, '1', 1, true, false)"#,
    )
    .bind(season_id)
    .bind(show_id)
    .execute(pool)
    .await
    .unwrap();

    let mut file_ids = Vec::with_capacity(count);
    for i in 1..=count {
        let ep_id = Uuid::now_v7();
        sqlx::query(
            r#"INSERT INTO tv_episodes (id, season_id, episode_identifier, episode_sort_order, title)
               VALUES ($1, $2, $3, $4, $5)"#,
        )
        .bind(ep_id)
        .bind(season_id)
        .bind(i.to_string())
        .bind(i as i32)
        .bind(format!("Episode {i}"))
        .execute(pool)
        .await
        .unwrap();

        let file_id = tv_episode_files::create(
            pool,
            tv_episode_files::EpisodeFileInput {
                file_path: format!("/nonexistent/v2-gate/s01e{i:02}.mkv"),
                file_size: Some(1024),
                file_fingerprint: None,
                format: "mkv".to_string(),
                quality: None,
                languages: None,
                subtitles: None,
                source_type: Some("test".to_string()),
                source_metadata: Some(json!({})),
                video_codec: None,
                video_width: None,
                video_height: None,
                video_bitrate_kbps: None,
                framerate: None,
                bit_depth: None,
                is_hdr: false,
                is_interlaced: false,
                hdr_format: None,
                duration_seconds: Some(duration_seconds),
                container: None,
                overall_bitrate_kbps: None,
            },
        )
        .await
        .unwrap();
        tv_episode_files::link_episodes(pool, file_id, &[ep_id])
            .await
            .unwrap();
        file_ids.push(file_id);
    }
    (season_id, file_ids)
}

/// Write a manifest + the sidecars `load_file_cache` hard-requires
/// (chromaprint + vhash) for one episode. Mirrors what `media_analyze`
/// produces for a video file with vision off: trickplay/vhash/chromaprint/
/// silence/loudness/black_frames — no audio_peaks, no embeddings. Any
/// extra branches `sibling_ready` or `load_file_cache` demand beyond this
/// set are bugs — this test is the regression guard.
fn write_video_cache(storage_root: &str, file_id: Uuid, duration: f64) {
    let dir = episode_cache_dir(storage_root, &MediaKind::TvFile { file_id });
    std::fs::create_dir_all(&dir).expect("create episode_cache dir");

    // Sidecars that load_file_cache reads with `?` (hard fail on missing).
    // Empty chromaprint is fine — `read_chromaprint` accepts any len%4==0
    // byte buffer. vhash needs a valid header: descriptor_size:u16 LE +
    // frame_count:u32 LE. Empty frames are acceptable.
    write_sidecar_at(&dir, Branch::Chromaprint, &[]).expect("write chromaprint");
    let mut vhash_bytes = Vec::with_capacity(6);
    vhash_bytes.extend_from_slice(&64u16.to_le_bytes()); // descriptor_size
    vhash_bytes.extend_from_slice(&0u32.to_le_bytes()); // frame_count
    write_sidecar_at(&dir, Branch::Vhash, &vhash_bytes).expect("write vhash");

    let mut m = new_manifest_at(&dir, duration);
    m.analyzed_file_id = Some(file_id);
    for b in [
        Branch::Trickplay,
        Branch::Vhash,
        Branch::Chromaprint,
        Branch::Silence,
        Branch::Loudness,
        Branch::BlackFrames,
    ] {
        m.artefacts.set(
            b,
            ArtefactInfo {
                present: true,
                params: Default::default(),
            },
        );
    }
    write_manifest_at(&dir, &m).expect("write manifest");
}

#[tokio::test]
async fn two_episode_v2_run_passes_sibling_gate() {
    let server = TestServer::start_api_only().await;

    // Vision off → gate uses `require_embeddings=false`. That's the narrower
    // fix surface and the most common prod config during the bench that
    // surfaced this bug.
    server
        .runtime_config
        .update(RuntimeConfigUpdate {
            skip_detect_mode: Some("full".to_string()),
            vision_service_url: Some(String::new()),
            ..Default::default()
        })
        .await
        .expect("set skip_detect config");

    let storage_root = server.storage_root();
    let (_season_id, file_ids) = seed_season(&server.pool, 2, 1800.0).await;
    for fid in &file_ids {
        write_video_cache(&storage_root, *fid, 1800.0);
        // The season staleness gate reads `media_cache_generated_at`, which
        // `media_analyze` stamps alongside the on-disk cache. Without it the
        // fixture looks cache-less and the whole season is gated out.
        tv_episode_files::set_media_cache_version(&server.pool, *fid, PIPELINE_VERSION)
            .await
            .expect("stamp media_cache_version");
    }

    // Seed chapter rows that the handler's chapter-sourced segment path picks
    // up (`load_chapter_segments_episode`). Pre-fix, the handler deferred
    // before ever reaching this path, so NO rows were written to
    // episode_skip_segments. Post-fix, the handler runs the chapter pass and
    // persists these deterministic segments — that's what we assert below.
    let chapter_inputs: Vec<video_chapters::ChapterInput> = file_ids
        .iter()
        .flat_map(|fid| {
            [
                video_chapters::ChapterInput {
                    movie_file_id: None,
                    episode_file_id: Some(*fid),
                    chapter_index: 0,
                    title: Some("Opening".to_string()),
                    start_seconds: 60.0,
                    end_seconds: 150.0,
                },
                video_chapters::ChapterInput {
                    movie_file_id: None,
                    episode_file_id: Some(*fid),
                    chapter_index: 1,
                    title: Some("Main".to_string()),
                    start_seconds: 150.0,
                    end_seconds: 1650.0,
                },
                video_chapters::ChapterInput {
                    movie_file_id: None,
                    episode_file_id: Some(*fid),
                    chapter_index: 2,
                    title: Some("End Credits".to_string()),
                    start_seconds: 1650.0,
                    end_seconds: 1800.0,
                },
            ]
        })
        .collect();
    video_chapters::insert_chapters(&server.pool, &chapter_inputs)
        .await
        .expect("insert chapters");

    // Nothing subscribes to the bus here; the handler's segment announcements
    // are sent and dropped, which is exactly what happens in prod when no
    // player is connected.
    let (library_events, _) = tokio::sync::broadcast::channel(16);
    let handler = SkipDetectHandler::new(
        server.pool.clone(),
        server.nats_publisher.clone(),
        server.runtime_config.clone(),
        storage_root.clone(),
        library_events,
    );

    // Run skip_detect for each file (mirrors the prod sequence where every
    // `media_analyze` chains one immediately). All manifests are on disk, so
    // the sibling gate must not defer any of them.
    let mut outcomes = Vec::new();
    for (i, fid) in file_ids.iter().enumerate() {
        let result = handler
            .execute(
                Uuid::now_v7(),
                json!({
                    "file_id": fid,
                    "media_type": "episode",
                    "run_id": Uuid::now_v7(),
                }),
                CancellationToken::new(),
            )
            .await
            .unwrap_or_else(|e| panic!("handler errored on episode {i}: {e:?}"));

        // Earlier episodes may legitimately defer (later siblings' manifests
        // not yet on disk in prod). In this test they're ALL already on disk,
        // so no file should defer — that's the whole point of the regression.
        let status = result.get("status").and_then(|v| v.as_str());
        assert_ne!(
            status,
            Some("deferred"),
            "episode {i} deferred unexpectedly — all sibling manifests are present: {result:?}"
        );
        outcomes.push(result);
    }

    // Detection is season-scoped: the first run processes every sibling and
    // stamps `skip_detect_ran_at` on all of them, so exactly one run does the
    // work and the rest short-circuit on the staleness gate.
    let completed: Vec<_> = outcomes
        .iter()
        .filter(|r| r.get("episodes_processed").is_some() || r.get("segments_saved").is_some())
        .collect();
    assert_eq!(
        completed.len(),
        1,
        "exactly one run should detect the whole season, got: {outcomes:?}"
    );
    for later in &outcomes[1..] {
        assert_eq!(
            later.get("reason").and_then(|v| v.as_str()),
            Some("season_already_current"),
            "runs after the season pass must skip as already-current, got: {later:?}"
        );
    }

    // The critical Done criterion: episode_skip_segments must have rows for
    // the season's file_ids after a completed run. Pre-fix this count was
    // zero (handler deferred forever); post-fix the chapter-sourced path
    // produces at least one intro + one credits row per episode.
    let row_count: i64 = sqlx::query_scalar(
        r#"SELECT COUNT(*)::BIGINT FROM episode_skip_segments
           WHERE episode_file_id = ANY($1)"#,
    )
    .bind(&file_ids)
    .fetch_one(&server.pool)
    .await
    .expect("count episode_skip_segments");
    assert!(
        row_count >= (file_ids.len() as i64) * 2,
        "expected ≥{} rows (intro + credits per episode) in episode_skip_segments, got {row_count}",
        file_ids.len() * 2,
    );
}

/// Mark one episode's cache as carrying embeddings from `model`.
fn set_embeddings_model(storage_root: &str, file_id: Uuid, model: &str) {
    let dir = episode_cache_dir(storage_root, &MediaKind::TvFile { file_id });
    let mut m = api::media_cache::readers::read_manifest_at(&dir).expect("manifest");
    let mut params = serde_json::Map::new();
    params.insert("model".into(), json!(model));
    m.artefacts.set(
        Branch::Embeddings,
        ArtefactInfo {
            present: true,
            params,
        },
    );
    write_manifest_at(&dir, &m).expect("write manifest");
}

async fn pending_media_analyze(pool: &PgPool) -> Vec<String> {
    sqlx::query_scalar(
        "SELECT payload->>'file_id' FROM job_status
         WHERE job_type = 'media_analyze' AND status = 'pending' ORDER BY 1",
    )
    .fetch_all(pool)
    .await
    .unwrap()
}

async fn vision_season(server: &TestServer) -> (SkipDetectHandler, Vec<Uuid>) {
    server
        .runtime_config
        .update(RuntimeConfigUpdate {
            skip_detect_mode: Some("full".to_string()),
            // Never contacted: a deferred season makes no HTTP calls, and a
            // detected one treats an unreachable text service as "no vision".
            vision_service_url: Some("http://127.0.0.1:9".to_string()),
            ..Default::default()
        })
        .await
        .expect("set skip_detect config");
    let storage_root = server.storage_root();
    let (_season_id, file_ids) = seed_season(&server.pool, 2, 1800.0).await;
    for fid in &file_ids {
        write_video_cache(&storage_root, *fid, 1800.0);
        tv_episode_files::set_media_cache_version(&server.pool, *fid, PIPELINE_VERSION)
            .await
            .expect("stamp media_cache_version");
    }
    let (library_events, _) = tokio::sync::broadcast::channel(16);
    let handler = SkipDetectHandler::new(
        server.pool.clone(),
        server.nats_publisher.clone(),
        server.runtime_config.clone(),
        storage_root,
        library_events,
    );
    (handler, file_ids)
}

async fn run(handler: &SkipDetectHandler, file_id: Uuid) -> serde_json::Value {
    handler
        .execute(
            Uuid::now_v7(),
            json!({"file_id": file_id, "media_type": "episode", "run_id": Uuid::now_v7()}),
            CancellationToken::new(),
        )
        .await
        .expect("skip_detect ran")
}

/// Embeddings from the previous model are ignored by the reader, so detecting
/// over them would run without vision and stamp the season current. The season
/// is re-embedded first: only the stale file is queued, and nothing is detected.
#[tokio::test]
async fn embeddings_from_another_model_are_re_embedded_before_detection() {
    let server = TestServer::start_api_only().await;
    let (handler, file_ids) = vision_season(&server).await;
    let storage_root = server.storage_root();
    set_embeddings_model(
        &storage_root,
        file_ids[0],
        "google/siglip2-so400m-patch16-naflex",
    );
    set_embeddings_model(&storage_root, file_ids[1], api::embedding::EMBED_MODEL);

    let result = run(&handler, file_ids[0]).await;

    assert_eq!(result["status"], "deferred", "{result}");
    assert_eq!(result["reason"], "awaiting_embeddings", "{result}");
    assert_eq!(
        pending_media_analyze(&server.pool).await,
        vec![file_ids[0].to_string()]
    );
    let ran: Option<time::OffsetDateTime> =
        sqlx::query_scalar("SELECT skip_detect_ran_at FROM tv_episode_files WHERE id = $1")
            .bind(file_ids[0])
            .fetch_one(&server.pool)
            .await
            .unwrap();
    assert!(ran.is_none(), "a deferred season must stay stale");

    // A second trigger while the supplement is queued does not queue another.
    run(&handler, file_ids[1]).await;
    assert_eq!(pending_media_analyze(&server.pool).await.len(), 1);
}

/// Once every sibling carries current-model embeddings — or a failed attempt
/// under the current model — the season is detected as normal.
#[tokio::test]
async fn current_model_embeddings_and_failed_attempts_do_not_defer() {
    let server = TestServer::start_api_only().await;
    let (handler, file_ids) = vision_season(&server).await;
    let storage_root = server.storage_root();
    set_embeddings_model(&storage_root, file_ids[0], api::embedding::EMBED_MODEL);
    let dir = episode_cache_dir(
        &storage_root,
        &MediaKind::TvFile {
            file_id: file_ids[1],
        },
    );
    let mut m = api::media_cache::readers::read_manifest_at(&dir).unwrap();
    let mut params = serde_json::Map::new();
    params.insert("model".into(), json!(api::embedding::EMBED_MODEL));
    params.insert("error".into(), json!("vision service not healthy"));
    m.artefacts.set(
        Branch::Embeddings,
        ArtefactInfo {
            present: false,
            params,
        },
    );
    write_manifest_at(&dir, &m).unwrap();

    let result = run(&handler, file_ids[0]).await;

    assert_ne!(result["status"], "deferred", "{result}");
    assert!(result.get("episodes_processed").is_some(), "{result}");
    assert!(pending_media_analyze(&server.pool).await.is_empty());
}

/// A re-embed that ran and still left the file stale (it could not write the
/// cache) must not be queued again: its own chained detect would loop. The
/// season is detected without vision for that file instead.
#[tokio::test]
async fn a_re_embed_that_did_not_take_is_not_retried() {
    let server = TestServer::start_api_only().await;
    let (handler, file_ids) = vision_season(&server).await;
    let storage_root = server.storage_root();
    set_embeddings_model(
        &storage_root,
        file_ids[0],
        "google/siglip2-so400m-patch16-naflex",
    );
    set_embeddings_model(&storage_root, file_ids[1], api::embedding::EMBED_MODEL);
    sqlx::query(
        "INSERT INTO job_status (id, job_type, status, payload, completed_at)
         VALUES ($1, 'media_analyze', 'completed', $2, NOW())",
    )
    .bind(Uuid::now_v7())
    .bind(json!({"file_id": file_ids[0], "media_type": "episode"}))
    .execute(&server.pool)
    .await
    .unwrap();

    let result = run(&handler, file_ids[0]).await;

    assert_ne!(result["status"], "deferred", "{result}");
    assert!(result.get("episodes_processed").is_some(), "{result}");
    assert!(pending_media_analyze(&server.pool).await.is_empty());
}
