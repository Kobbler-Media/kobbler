use crate::common::*;
use reqwest::StatusCode;
use serde_json::{Value, json};

#[tokio::test]
async fn a_report_is_stored_with_everything_the_server_could_not_have_known() {
    let server = TestServer::start_api_only().await;
    let client = server.admin_client();

    let created: Value = client
        .post_json(
            "/api/v1/admin/diagnostics/playback",
            &json!({
                "detail": "Playback failed\n\nStream: http://127.0.0.1:35079/...\nVLC playback error",
                "client_kind": "android",
                "app_version": "0.1.67 (70)",
                "device_label": "Pixel 10 Pro",
                "media_type": "livetv",
                "media_id": "01a04fbc-b3bd-73b0-a855-0ac253f41d00",
                "stream_url": "https://kobbler.example/api/v1/admin/livetv/channels/x/stream.ts",
                "session_id": "01a05694-e81a-7690-af54-360434aa6d6b",
            }),
        )
        .await
        .assert_status(StatusCode::CREATED)
        .json();
    assert!(created["id"].is_string());

    let listed: Value = client
        .get("/api/v1/admin/diagnostics/playback")
        .await
        .assert_ok()
        .json();
    let item = &listed["items"][0];
    assert_eq!(item["client_kind"], "android");
    assert_eq!(item["device_label"], "Pixel 10 Pro");
    assert_eq!(item["media_type"], "livetv");
    assert_eq!(
        item["stream_url"],
        "https://kobbler.example/api/v1/admin/livetv/channels/x/stream.ts"
    );
    assert!(
        item["detail"]
            .as_str()
            .unwrap()
            .contains("VLC playback error"),
        "the detail is the text the dialog showed, verbatim"
    );
}

/// Only `detail` is load-bearing; a client that cannot fill the rest still gets
/// its report kept rather than dropped on a 422.
#[tokio::test]
async fn detail_alone_is_a_valid_report() {
    let server = TestServer::start_api_only().await;
    let client = server.admin_client();

    client
        .post_json(
            "/api/v1/admin/diagnostics/playback",
            &json!({ "detail": "something broke" }),
        )
        .await
        .assert_status(StatusCode::CREATED);

    let listed: Value = client
        .get("/api/v1/admin/diagnostics/playback")
        .await
        .assert_ok()
        .json();
    assert_eq!(listed["items"][0]["client_kind"], "unknown");
}

#[tokio::test]
async fn an_empty_report_is_rejected() {
    let server = TestServer::start_api_only().await;
    let client = server.admin_client();

    client
        .post_json(
            "/api/v1/admin/diagnostics/playback",
            &json!({ "detail": "   " }),
        )
        .await
        .assert_status(StatusCode::BAD_REQUEST);
}

/// Reporting is a contributor action — anyone who can play can say it broke — but
/// reading everyone's reports is not. The bodies are valid on purpose: the role
/// check runs inside the handler, so a malformed one would 422 before the 403.
#[tokio::test]
async fn a_viewer_may_report_but_may_not_read_the_pile() {
    let server = TestServer::start_api_only().await;
    let viewer = create_user_client(&server, "viewer@example.com", "viewer").await;

    viewer
        .post_json(
            "/api/v1/admin/diagnostics/playback",
            &json!({ "detail": "it broke" }),
        )
        .await
        .assert_status(StatusCode::FORBIDDEN);

    let contributor = create_user_client(&server, "contrib@example.com", "contributor").await;
    contributor
        .post_json(
            "/api/v1/admin/diagnostics/playback",
            &json!({ "detail": "it broke" }),
        )
        .await
        .assert_status(StatusCode::CREATED);

    contributor
        .get("/api/v1/admin/diagnostics/playback")
        .await
        .assert_status(StatusCode::FORBIDDEN);
}
