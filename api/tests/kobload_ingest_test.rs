use crate::common::*;
use reqwest::StatusCode;
use serde_json::json;
use uuid::Uuid;

#[tokio::test]
async fn test_kobload_ingest_requires_token() {
    let server = TestServer::start_api_only().await;
    let client = TestClient::new(&server.api_url);

    // With an invalid token, should return UNAUTHORIZED
    let resp = client
        .post_json(
            "/api/v1/ingest/invalid-token",
            &json!({
                "download_id": Uuid::now_v7(),
                "status": "completed",
                "metadata": {},
                "files": [
                    {"path": "test.epub", "size": 1024, "extension": "epub"}
                ],
                "error": null
            }),
        )
        .await;

    assert_eq!(resp.status, StatusCode::UNAUTHORIZED);
}
