use crate::common::*;
use serde_json::json;
use wiremock::matchers::{body_partial_json, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

#[tokio::test]
async fn pin_login_publishes_to_configured_pin_service() {
    let relay = MockServer::start().await;
    Mock::given(method("PUT"))
        .and(path("/pin/kobbler/ABCD12"))
        .and(body_partial_json(
            json!({"api_url": "https://media.example"}),
        ))
        .respond_with(ResponseTemplate::new(202))
        .expect(1)
        .mount(&relay)
        .await;

    let relay_url = relay.uri();
    let server = TestServer::start_api_only_with(|c| c.pin_service_url = relay_url).await;

    server
        .admin_client()
        .post_json(
            "/api/v1/admin/pin-login",
            &json!({"pin": "abcd12", "api_url": "https://media.example/"}),
        )
        .await
        .assert_status(reqwest::StatusCode::NO_CONTENT);
}
