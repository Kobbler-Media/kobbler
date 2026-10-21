use crate::common::*;
use serde_json::Value;

#[tokio::test]
async fn test_types_expose_webpush_triggers() {
    let server = TestServer::start().await;

    let resp = server
        .admin_client()
        .get("/api/v1/admin/notifications/types")
        .await;
    resp.assert_ok();

    let types: Value = resp.json();
    let webpush = types
        .as_array()
        .expect("types array")
        .iter()
        .find(|t| t["id"] == "webpush")
        .expect("webpush type is listed so the browser push section can render its triggers");

    let triggers: Vec<&str> = webpush["supported_triggers"]
        .as_array()
        .expect("supported_triggers array")
        .iter()
        .map(|t| t.as_str().expect("trigger string"))
        .collect();

    for expected in ["on_grab", "on_import", "on_failed", "on_media_corrupt"] {
        assert!(
            triggers.contains(&expected),
            "webpush should support {expected}, got {triggers:?}"
        );
    }
}

#[tokio::test]
async fn test_webpush_subscribe_defaults_to_all_triggers() {
    let server = TestServer::start().await;

    server
        .admin_client()
        .post_json(
            "/api/v1/admin/notifications/webpush/subscribe",
            &serde_json::json!({
                "endpoint": "https://push.example.com/sub/1",
                "p256dh": "test-p256dh",
                "auth": "test-auth"
            }),
        )
        .await
        .assert_status(reqwest::StatusCode::CREATED);

    let resp = server
        .admin_client()
        .get("/api/v1/admin/notifications/agents")
        .await;
    resp.assert_ok();

    let agents: Value = resp.json();
    let agent = agents
        .as_array()
        .expect("agents array")
        .iter()
        .find(|a| a["agent_type"] == "webpush")
        .expect("subscribing auto-creates the webpush agent");

    let triggers: Vec<&str> = agent["triggers"]
        .as_array()
        .expect("triggers array")
        .iter()
        .map(|t| t.as_str().expect("trigger string"))
        .collect();

    assert!(
        triggers.contains(&"on_media_corrupt"),
        "corrupt media should be on by default, got {triggers:?}"
    );
}
