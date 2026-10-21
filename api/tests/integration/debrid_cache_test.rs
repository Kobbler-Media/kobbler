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
