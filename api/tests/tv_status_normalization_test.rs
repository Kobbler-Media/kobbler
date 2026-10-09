use crate::common::*;
use reqwest::StatusCode;
use serde_json::{Value, json};

async fn create_show(server: &TestServer, title: &str, status: &str) -> String {
    let resp = server
        .admin_client()
        .post_json(
            "/api/v1/admin/tv/shows",
            &json!({ "title": title, "year": 2024, "status": status }),
        )
        .await;
    resp.assert_status(StatusCode::CREATED);
    let json: Value = resp.json();
    json["id"].as_str().unwrap().to_string()
}

async fn show_status(server: &TestServer, id: &str) -> String {
    let resp = server
        .admin_client()
        .get(&format!("/api/v1/admin/tv/shows/{id}"))
        .await;
    resp.assert_ok();
    let json: Value = resp.json();
    json["status"].as_str().unwrap_or_default().to_string()
}

#[tokio::test]
async fn test_create_show_normalizes_provider_status() {
    let server = TestServer::start_api_only().await;

    for (raw, expected) in [
        ("Returning Series", "continuing"),
        ("Ended", "ended"),
        ("Canceled", "canceled"),
        ("Cancelled", "canceled"),
        ("Continuing", "continuing"),
        ("In Production", "upcoming"),
        ("Running", "continuing"),
    ] {
        let id = create_show(&server, &format!("Show {raw}"), raw).await;
        assert_eq!(
            show_status(&server, &id).await,
            expected,
            "POST /admin/tv/shows must store {raw:?} as {expected:?}"
        );
    }
}

#[tokio::test]
async fn test_patch_show_normalizes_provider_status() {
    let server = TestServer::start_api_only().await;
    let id = create_show(&server, "Patch Target", "continuing").await;

    let resp = server
        .admin_client()
        .patch_json(
            &format!("/api/v1/admin/tv/shows/{id}"),
            &json!({ "status": "Ended" }),
        )
        .await;
    resp.assert_status(StatusCode::NO_CONTENT);

    assert_eq!(
        show_status(&server, &id).await,
        "ended",
        "PATCH /admin/tv/shows/:id must store \"Ended\" as \"ended\""
    );
}

#[tokio::test]
async fn test_status_filter_matches_show_created_with_provider_casing() {
    let server = TestServer::start_api_only().await;
    let id = create_show(&server, "Filterable", "Returning Series").await;

    let resp = server
        .admin_client()
        .get("/api/v1/admin/tv/shows?status=continuing&limit=100")
        .await;
    resp.assert_ok();
    let json: Value = resp.json();
    let ids: Vec<&str> = json["items"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|i| i["id"].as_str())
        .collect();

    assert!(
        ids.contains(&id.as_str()),
        "?status=continuing must return a show written as \"Returning Series\"; got {ids:?}"
    );
}

#[tokio::test]
async fn test_unknown_status_passes_through_unchanged() {
    let server = TestServer::start_api_only().await;
    let id = create_show(&server, "Unknowable", "Hiatus Forever").await;
    assert_eq!(
        show_status(&server, &id).await,
        "hiatus forever",
        "an unmapped provider status must survive the write, lowercased"
    );
}
