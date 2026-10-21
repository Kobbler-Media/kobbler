use crate::common::*;
use api::runtime_config::RuntimeConfigUpdate;
use reqwest::StatusCode;
use serde_json::{Value, json};
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

const TMDB_ID: &str = "550";

fn release(release_type: u8, date: &str) -> Value {
    json!({ "certification": "", "release_date": format!("{date}T00:00:00.000Z"), "type": release_type })
}

fn movie_payload() -> Value {
    json!({
        "id": 550, "title": "Dated", "release_date": "2024-01-15", "genres": [],
        "release_dates": { "results": [
            { "iso_3166_1": "US", "release_dates": [release(3, "2024-01-15")] },
            { "iso_3166_1": "AU", "release_dates": [
                release(1, "2024-01-02"),
                release(3, "2024-02-09"),
                release(3, "2024-02-01"),
                release(4, "2024-04-10"),
                release(5, "2024-06-01"),
            ]},
        ]}
    })
}

fn au_dates() -> Value {
    json!({
        "region": "AU", "premiere": "2024-01-02", "theatrical_limited": null,
        "theatrical": "2024-02-01", "digital": "2024-04-10", "physical": "2024-06-01", "tv": null
    })
}

async fn server_with_tmdb(region: Option<&str>) -> (TestServer, MockServer) {
    let server = TestServer::start_api_only().await;
    let tmdb = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path(format!("/movie/{TMDB_ID}")))
        .respond_with(ResponseTemplate::new(200).set_body_json(movie_payload()))
        .mount(&tmdb)
        .await;
    sqlx::query(
        "UPDATE video_metadata_providers
         SET config = jsonb_build_object('api_key', 'k', 'base_url', $1), enabled = true
         WHERE provider_type = 'tmdb'",
    )
    .bind(tmdb.uri())
    .execute(&server.pool)
    .await
    .unwrap();
    if let Some(region) = region {
        server
            .runtime_config
            .update(RuntimeConfigUpdate {
                discover_region: Some(region.to_string()),
                ..Default::default()
            })
            .await
            .unwrap();
    }
    (server, tmdb)
}

async fn discover_release_dates(server: &TestServer) -> Value {
    let resp = server
        .admin_client()
        .get(&format!("/api/v1/admin/discover/movie/tmdb/{TMDB_ID}"))
        .await;
    resp.assert_ok();
    resp.json::<Value>()["details"]["release_dates"].clone()
}

#[tokio::test]
async fn discover_detail_uses_the_configured_region() {
    let (server, _tmdb) = server_with_tmdb(Some("AU")).await;
    assert_eq!(discover_release_dates(&server).await, au_dates());
}

#[tokio::test]
async fn discover_detail_defaults_to_us() {
    let (server, _tmdb) = server_with_tmdb(None).await;
    let dates = discover_release_dates(&server).await;
    assert_eq!(dates["region"], "US");
    assert_eq!(dates["theatrical"], "2024-01-15");
    assert_eq!(dates["premiere"], Value::Null);
}

#[tokio::test]
async fn discover_detail_requires_auth() {
    let (server, _tmdb) = server_with_tmdb(None).await;
    server
        .api_client()
        .get(&format!("/api/v1/admin/discover/movie/tmdb/{TMDB_ID}"))
        .await
        .assert_status(StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn library_movie_persists_release_dates_on_refresh() {
    let (server, tmdb) = server_with_tmdb(Some("AU")).await;
    let client = server.admin_client();
    let created = client
        .post_json(
            "/api/v1/admin/movies",
            &json!({ "title": "Dated", "year": 2024, "identifiers": { "tmdb_id": TMDB_ID } }),
        )
        .await;
    created.assert_status(StatusCode::CREATED);
    let id = created.json::<Value>()["id"].as_str().unwrap().to_string();

    let refreshed = client
        .post_json(
            &format!("/api/v1/admin/movies/{id}/refresh-metadata"),
            &json!({}),
        )
        .await;
    refreshed.assert_ok();
    assert_eq!(refreshed.json::<Value>()["release_dates"], au_dates());

    tmdb.reset().await;
    let detail = client.get(&format!("/api/v1/admin/movies/{id}")).await;
    detail.assert_ok();
    assert_eq!(detail.json::<Value>()["release_dates"], au_dates());
}
