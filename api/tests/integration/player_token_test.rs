use crate::common::*;
use reqwest::StatusCode;
use serde_json::Value;

const PATH: &str = "/api/v1/admin/auth/player-token";

async fn login_token(server: &TestServer) -> String {
    let resp = TestClient::new(&server.api_url)
        .post_json(
            "/api/v1/admin/login",
            &serde_json::json!({ "email": "admin@test.local", "password": "testadmin123" }),
        )
        .await;
    resp.assert_ok();
    resp.json::<Value>()["token"].as_str().unwrap().to_string()
}

#[tokio::test]
async fn test_session_cookie_mints_a_separate_bearer_session() {
    let server = TestServer::start_api_only().await;
    let cookie = login_token(&server).await;

    let resp = TestClient::new(&server.api_url)
        .post_with_header(PATH, "Cookie", &format!("kobbler_session={cookie}"))
        .await;
    resp.assert_ok();
    let body: Value = resp.json();
    let minted = body["token"].as_str().unwrap().to_string();
    assert_ne!(minted, cookie);
    assert!(body["expires_at"].is_string());

    let player = TestClient::new(&server.api_url).with_auth(&minted);
    let me = player.get("/api/v1/admin/me").await;
    me.assert_ok();
    assert_eq!(me.json::<Value>()["email"], "admin@test.local");

    let sessions: Value = player.get("/api/v1/admin/sessions").await.json();
    let current = sessions
        .as_array()
        .unwrap()
        .iter()
        .find(|s| s["is_current"] == true)
        .expect("minted session is listed");
    assert_eq!(current["device_name"], "Web player");
}

#[tokio::test]
async fn test_only_a_session_cookie_can_mint() {
    let server = TestServer::start_api_only().await;
    let bearer = login_token(&server).await;

    let cases = [
        (
            "no credentials",
            TestClient::new(&server.api_url).post(PATH).await,
        ),
        (
            "bearer header",
            TestClient::new(&server.api_url)
                .with_auth(&bearer)
                .post(PATH)
                .await,
        ),
        (
            "unknown cookie",
            TestClient::new(&server.api_url)
                .post_with_header(PATH, "Cookie", "kobbler_session=not-a-token")
                .await,
        ),
    ];
    for (name, resp) in cases {
        assert_eq!(resp.status, StatusCode::UNAUTHORIZED, "{name}");
    }
}
