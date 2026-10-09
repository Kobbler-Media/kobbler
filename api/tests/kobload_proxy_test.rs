use crate::common::*;
use reqwest::StatusCode;

#[tokio::test]
async fn test_proxy_quality_profiles_requires_auth() {
    let server = TestServer::start_api_only().await;
    let client = TestClient::new(&server.api_url);

    let resp = client.get("/api/v1/admin/kobload/quality-profiles").await;
    assert_eq!(resp.status, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn test_proxy_quality_profiles_returns_503_when_kobload_not_configured() {
    let server = TestServer::start_api_only().await;

    // No kobload_base_url configured in runtime config, so the endpoint
    // should return 503 Service Unavailable.
    let resp = server
        .admin_client()
        .get("/api/v1/admin/kobload/quality-profiles")
        .await;
    assert_eq!(resp.status, StatusCode::SERVICE_UNAVAILABLE);
}
