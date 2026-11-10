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
