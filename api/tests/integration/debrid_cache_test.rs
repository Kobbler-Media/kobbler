use crate::common::debrid_rig::*;
use crate::common::debrid_upstream::{UpstreamPlan, WholeBody};
use crate::common::*;
use api::jobs::worker::JobHandler;
use reqwest::StatusCode;
use serde_json::{Value, json};
use std::time::Duration;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

async fn cache_rows(rig: &Rig, info_hash: &str) -> i64 {
    sqlx::query_scalar(
        "SELECT COUNT(*) FROM debrid_cache_files WHERE file_path LIKE '%/' || $1 || '/%'",
    )
    .bind(info_hash)
    .fetch_one(&rig.server.pool)
    .await
    .unwrap()
}

async fn delete_session(rig: &Rig, session_id: Uuid) {
    rig.server
        .admin_client()
        .delete(&format!("/api/v1/admin/stream/{session_id}"))
        .await
        .assert_status(StatusCode::NO_CONTENT);
}

async fn wait_for_probe(rig: &Rig, session_id: Uuid) {
    let server = &rig.server;
    wait_until_async(
        Duration::from_secs(10),
        "probe never finished",
        || async move {
            let status: Value = session_status(server, session_id).await.json();
            status["state"] != "preparing"
        },
    )
    .await;
}

/// A finished download of `info_hash`, whose session has since been deleted.
async fn orphaned_download(rig: &Rig, info_hash: &str) -> Value {
    let movie = rig.movie().await;
    let body = rig.resolve_body(info_hash, movie, direct_play_caps());
    let first = session_id_of(&rig.resolve_with(&body).await);
    rig.wait_for_complete(info_hash).await;
    wait_for_probe(rig, first).await;
    delete_session(rig, first).await;
    body
}

#[tokio::test]
async fn a_finished_download_outlives_its_session_and_serves_the_next_one() {
    let rig = Rig::start(UpstreamPlan::new(ITA_ENG_FIXTURE.to_vec()), "film.mkv").await;
    let body = orphaned_download(&rig, "keephash").await;

    assert_eq!(cache_rows(&rig, "keephash").await, 1);
    assert!(rig.cache_file("keephash").exists());
    let upstream_before = (rig.upstream.whole_gets(), rig.upstream.ranges_seen().len());

    let second = session_id_of(&rig.resolve_with(&body).await);
    let played = play(&rig.server, second, "bytes=0-").await;

    assert!(played.status.is_success(), "{}", played.status);
    assert!(
        played.body == ITA_ENG_FIXTURE,
        "the cached file came out different"
    );
    assert_eq!(
        rig.kobload_requests("/stream/resolve").await.len(),
        1,
        "the second session asked kobload for a release it already had on disk"
    );
    assert_eq!(
        (rig.upstream.whole_gets(), rig.upstream.ranges_seen().len()),
        upstream_before,
        "the second session went back to the debrid host"
    );
}

#[tokio::test]
async fn a_partial_download_is_served_locally_and_resumed() {
    let data = noise(4 * MB);
    let mut plan = UpstreamPlan::new(data.clone());
    plan.whole = WholeBody::Trickle {
        chunk: 64 * 1024,
        every: Duration::from_millis(30),
    };
    let rig = Rig::start(plan, "film.mp4").await;
    let movie = rig.movie().await;
    let body = rig.resolve_body("partialhash", movie, direct_play_caps());
    let first = session_id_of(&rig.resolve_with(&body).await);
    rig.wait_for_local("partialhash", 256 * 1024).await;
    delete_session(&rig, first).await;
    tokio::time::sleep(Duration::from_millis(300)).await;
    let settled = std::fs::metadata(rig.cache_file("partialhash"))
        .unwrap()
        .len();
    assert_eq!(cache_rows(&rig, "partialhash").await, 1);

    let second = session_id_of(&rig.resolve_with(&body).await);
    let cache_path: Option<String> =
        sqlx::query_scalar("SELECT cache_path FROM debrid_stream_sessions WHERE id = $1")
            .bind(second)
            .fetch_one(&rig.server.pool)
            .await
            .unwrap();
    assert_eq!(
        cache_path.as_deref(),
        Some(rig.cache_file("partialhash").to_string_lossy().as_ref()),
        "the new session must know its local bytes from the start"
    );

    let head = play(&rig.server, second, "bytes=0-65535").await;
    assert_eq!(head.status, StatusCode::PARTIAL_CONTENT);
    assert!(head.body == data[..65536]);
    assert!(
        !rig.upstream
            .ranges_seen()
            .iter()
            .any(|r| r == "bytes=0-65535"),
        "bytes already on disk were proxied"
    );

    rig.wait_for_local("partialhash", data.len() as u64).await;
    let resumed_from_disk = rig.upstream.ranges_seen().iter().any(|r| {
        r.trim_start_matches("bytes=")
            .split('-')
            .next()
            .and_then(|s| s.parse::<u64>().ok())
            .is_some_and(|start| start >= settled)
    });
    assert!(resumed_from_disk, "{:?}", rig.upstream.ranges_seen());
    assert!(std::fs::read(rig.cache_file("partialhash")).unwrap() == data);
}

#[tokio::test]
async fn orphaned_cache_entries_are_still_evicted_by_ttl_lru_and_size() {
    let rig = Rig::start(UpstreamPlan::new(ITA_ENG_FIXTURE.to_vec()), "film.mkv").await;
    let cases = [
        ("ttlhash", -1, false, true),
        ("lruhash", 24, false, true),
        ("pinnedhash", -1, true, false),
    ];
    for (hash, _, _, _) in &cases {
        orphaned_download(&rig, hash).await;
    }
    let recent = "recenthash";
    orphaned_download(&rig, recent).await;
    for (hash, expires_in_hours, pinned, _) in &cases {
        sqlx::query(
            "UPDATE debrid_cache_files
             SET expires_at = NOW() + make_interval(hours => $2), pinned = $3,
                 last_accessed_at = NOW() - INTERVAL '2 hours'
             WHERE file_path LIKE '%/' || $1 || '/%'",
        )
        .bind(hash)
        .bind(*expires_in_hours)
        .bind(*pinned)
        .execute(&rig.server.pool)
        .await
        .unwrap();
    }
    rig.set_config(json!({ "debrid_cache_max_bytes": 1 })).await;

    api::jobs::handlers::DebridCacheCleanupHandler::new(
        rig.server.pool.clone(),
        std::sync::Arc::new(rig.server.runtime_config.clone()),
    )
    .execute(Uuid::now_v7(), json!({}), CancellationToken::new())
    .await
    .unwrap();

    for (hash, _, _, evicted) in cases {
        assert_eq!(cache_rows(&rig, hash).await == 0, evicted, "{hash} row");
        assert_eq!(!rig.cache_file(hash).exists(), evicted, "{hash} file");
    }
    assert_eq!(
        cache_rows(&rig, recent).await,
        1,
        "a file touched within the hour is never evicted"
    );
}

async fn cache_movie_segments(rig: &Rig, movie_id: Uuid, segments: &Value) {
    sqlx::query(
        "INSERT INTO movie_cached_segments (movie_id, segments, pipeline_version) VALUES ($1, $2, $3)",
    )
    .bind(movie_id)
    .bind(segments)
    .bind(api::media_cache::PIPELINE_VERSION)
    .execute(&rig.server.pool)
    .await
    .unwrap();
}

async fn end_playback(rig: &Rig, session_id: Uuid) {
    rig.server
        .admin_client()
        .delete(&format!("/api/v1/admin/playback/sessions/{session_id}"))
        .await;
}

async fn record_analysis(rig: &Rig, session_id: Uuid, segments: &Value, lufs: f64) {
    sqlx::query("UPDATE debrid_stream_sessions SET skip_segments = $2, lufs = $3 WHERE id = $1")
        .bind(session_id)
        .bind(segments)
        .bind(lufs)
        .execute(&rig.server.pool)
        .await
        .unwrap();
}

fn served_segments(resp: &Value) -> Vec<(String, f64)> {
    resp["skip_segments"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|s| {
            (
                s["segment_type"].as_str().unwrap().to_string(),
                s["start_seconds"].as_f64().unwrap(),
            )
        })
        .collect()
}

/// Intro and credits are timed to one file; another release of the title has its own.
#[tokio::test]
async fn a_session_opens_with_what_analysis_found_in_its_own_release_only() {
    use api::media_cache::episode_cache::{MediaKind, episode_cache_dir};
    let rig = Rig::start(UpstreamPlan::new(ITA_ENG_FIXTURE.to_vec()), "film.mkv").await;
    let movie = rig.movie().await;
    let segments = json!([
        { "segment_type": "intro", "start_seconds": 10.0, "end_seconds": 40.0 },
        { "segment_type": "credits", "start_seconds": 2700.0, "end_seconds": 2800.0 }
    ]);
    let body = rig.resolve_body("ownrelease", movie, direct_play_caps());
    let first = session_id_of(&rig.resolve_with(&body).await);
    rig.wait_for_complete("ownrelease").await;
    wait_for_probe(&rig, first).await;
    record_analysis(&rig, first, &segments, -23.5).await;
    cache_movie_segments(&rig, movie, &segments).await;
    let analysed = episode_cache_dir(
        &rig.server.storage_root(),
        &MediaKind::Movie { movie_id: movie },
    );
    std::fs::create_dir_all(&analysed).unwrap();
    std::fs::write(
        analysed.join("loudness.json"),
        r#"{"integrated_lufs":-23.5}"#,
    )
    .unwrap();
    end_playback(&rig, first).await;

    let replay = rig.resolve_with(&body).await;
    let other = rig
        .resolve_with(&rig.resolve_body("otherrelease", movie, direct_play_caps()))
        .await;

    assert_eq!(
        served_segments(&replay),
        vec![("intro".to_string(), 10.0), ("credits".to_string(), 2700.0)],
        "{replay}"
    );
    assert_eq!(replay["lufs"], json!(-23.5), "{replay}");
    assert_eq!(served_segments(&other), vec![], "{other}");
    assert_eq!(other["lufs"], Value::Null, "{other}");
    for (name, resp, expected) in [
        ("replay", &replay, Some(&segments)),
        ("other release", &other, None),
    ] {
        let stored: Option<Value> =
            sqlx::query_scalar("SELECT skip_segments FROM debrid_stream_sessions WHERE id = $1")
                .bind(session_id_of(resp))
                .fetch_one(&rig.server.pool)
                .await
                .unwrap();
        assert_eq!(
            stored.as_ref(),
            expected,
            "{name}: the status stream reads this"
        );
    }
}

async fn local_option(rig: &Rig, movie_id: Uuid) -> TestResponse {
    rig.server
        .admin_client()
        .get(&format!(
            "/api/v1/admin/stream/local?media_type=movie&media_id={movie_id}"
        ))
        .await
}

#[tokio::test]
async fn a_title_with_its_release_on_disk_offers_it_without_a_search() {
    let rig = Rig::start(UpstreamPlan::new(ITA_ENG_FIXTURE.to_vec()), "film.mkv").await;
    let movie = rig.movie().await;
    let body = rig.resolve_body("localhash", movie, direct_play_caps());
    let session = session_id_of(&rig.resolve_with(&body).await);
    rig.wait_for_complete("localhash").await;
    wait_for_probe(&rig, session).await;

    let resp = local_option(&rig, movie).await;
    resp.assert_ok();
    let option: Value = resp.json();
    assert_eq!(option["info_hash"], "localhash", "{option}");
    assert_eq!(
        option["debrid_provider_id"], body["debrid_provider_id"],
        "{option}"
    );
    assert_eq!(option["availability"], "ready", "{option}");
    assert!(option["release_title"].is_string(), "{option}");
    assert_eq!(rig.kobload_requests("/stream/search").await.len(), 0);

    local_option(&rig, rig.movie().await)
        .await
        .assert_status(StatusCode::NOT_FOUND);

    std::fs::remove_file(rig.cache_file("localhash")).unwrap();
    local_option(&rig, movie)
        .await
        .assert_status(StatusCode::NOT_FOUND);
}

fn chapter_titles(chapters: &Value) -> Vec<String> {
    chapters
        .as_array()
        .map(|a| {
            a.iter()
                .map(|c| c["title"].as_str().unwrap_or_default().to_string())
                .collect()
        })
        .unwrap_or_default()
}

/// A library file's session carries its loudness and chapters; a debrid one did neither.
#[tokio::test]
async fn a_debrid_session_carries_the_loudness_and_chapters_of_its_file() {
    let rig = Rig::start(UpstreamPlan::new(CHAPTERED_FIXTURE.to_vec()), "film.mkv").await;
    let movie = rig.movie().await;
    let body = rig.resolve_body("chapterhash", movie, direct_play_caps());
    let fresh = rig.resolve_with(&body).await;
    let session = session_id_of(&fresh);
    let resp = reqwest::Client::new()
        .get(format!(
            "{}/api/v1/admin/stream/{session}/status",
            rig.server.api_url
        ))
        .header(
            "Authorization",
            format!("Bearer {}", rig.server.admin_token),
        )
        .send()
        .await
        .unwrap();
    let mut events = resp.bytes_stream();
    let mut seen = String::new();
    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    while !seen.contains("\"type\":\"chapters\"") && std::time::Instant::now() < deadline {
        if let Ok(Some(Ok(chunk))) = tokio::time::timeout(
            Duration::from_secs(1),
            futures::StreamExt::next(&mut events),
        )
        .await
        {
            seen.push_str(&String::from_utf8_lossy(&chunk));
        }
    }
    assert!(
        seen.contains("\"title\":\"Opening\""),
        "no chapters on the stream: {seen}"
    );

    rig.wait_for_complete("chapterhash").await;
    wait_for_probe(&rig, session).await;
    record_analysis(&rig, session, &json!([]), -23.5).await;
    end_playback(&rig, session).await;
    let replay = rig.resolve_with(&body).await;
    assert_eq!(replay["lufs"], json!(-23.5), "{replay}");
    assert_eq!(
        chapter_titles(&replay["chapters"]),
        ["Opening", "Episode"],
        "{replay}"
    );
}
