use crate::common::analyze::*;
use crate::common::*;
use api::jobs::worker::JobHandler;
use serde_json::json;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

async fn queued_skip_detects(server: &TestServer) -> i64 {
    sqlx::query_scalar("SELECT COUNT(*) FROM job_status WHERE job_type = 'skip_detect'")
        .fetch_one(&server.pool)
        .await
        .unwrap()
}

#[tokio::test]
async fn analysis_queues_skip_detection_only_when_it_is_switched_on() {
    for (mode, expected) in [("off", 0), ("full", 1)] {
        let server = TestServer::start_api_only().await;
        server
            .admin_client()
            .patch_json(
                "/api/v1/admin/config",
                &json!({ "enrichment_enabled": true, "skip_detect_mode": mode }),
            )
            .await
            .assert_ok();
        let file_id = seed_analyzable_movie(&server, 0).await;

        let result = analyzer(&server)
            .execute(
                Uuid::now_v7(),
                json!({ "file_id": file_id, "media_type": "movie", "triggered_by": "manual" }),
                CancellationToken::new(),
            )
            .await
            .unwrap();

        assert_eq!(queued_skip_detects(&server).await, expected, "mode={mode}");
        assert!(
            result["branch_results"].get("embeddings").is_none(),
            "no vision service configured, so no embedding attempt: {}",
            result["branch_results"]
        );
    }
}

/// What one analysis left on disk is enough for the next to see nothing changed, so a
/// backfill pass or a re-click does not decode the whole file again.
#[tokio::test]
async fn a_second_analysis_of_an_unchanged_file_skips_the_decode() {
    let server = TestServer::start_api_only().await;
    server
        .admin_client()
        .patch_json(
            "/api/v1/admin/config",
            &json!({ "enrichment_enabled": true, "skip_detect_mode": "off" }),
        )
        .await
        .assert_ok();
    for (kind, file_id) in [
        ("movie", seed_analyzable_movie(&server, 0).await),
        ("episode", seed_analyzable_episode(&server, 0).await),
    ] {
        let handler = analyzer(&server);
        let payload = json!({ "file_id": file_id, "media_type": kind, "triggered_by": "manual" });
        handler
            .execute(Uuid::now_v7(), payload.clone(), CancellationToken::new())
            .await
            .unwrap();
        let again = handler
            .execute(Uuid::now_v7(), payload, CancellationToken::new())
            .await
            .unwrap();
        assert_eq!(again["status"], "skipped_decode", "{kind}: {again}");
    }
}
