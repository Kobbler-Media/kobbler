use crate::common::*;
use api::jobs::handlers::DiscoverSyncHandler;
use api::jobs::worker::JobHandler;
use reqwest::StatusCode;
use serde_json::{Value, json};
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

const VIDEO_BASES: [&str; 2] = ["/api/v1/admin/movies", "/api/v1/admin/tv/shows"];

const SENT_AND_STORED: [(f64, f64); 4] = [(7.3, 7.3), (10.0, 10.0), (73.0, 7.3), (-1.0, 0.0)];

async fn rating_of(server: &TestServer, url: &str) -> f64 {
    let resp = server.admin_client().get(url).await;
    resp.assert_ok();
    let json: Value = resp.json();
    json["rating"].as_f64().expect("rating should be set")
}

fn assert_close(actual: f64, expected: f64, context: &str) {
    assert!(
        (actual - expected).abs() < 1e-9,
        "{context}: expected {expected}, got {actual}"
    );
}

#[tokio::test]
async fn video_ratings_keep_the_ten_point_scale_on_create_and_update() {
    let server = TestServer::start_api_only().await;

    for base in VIDEO_BASES {
        for (sent, stored) in SENT_AND_STORED {
            let resp = server
                .admin_client()
                .post_json(
                    base,
                    &json!({ "title": format!("Rated {sent}"), "year": 2024, "rating": sent }),
                )
                .await;
            resp.assert_status(StatusCode::CREATED);
            let id = resp.json::<Value>()["id"].as_str().unwrap().to_string();
            let url = format!("{base}/{id}");
            assert_close(
                rating_of(&server, &url).await,
                stored,
                &format!("create {base} {sent}"),
            );

            server
                .admin_client()
                .patch_json(&url, &json!({ "rating": 1.0 }))
                .await
                .assert_status(StatusCode::NO_CONTENT);
            server
                .admin_client()
                .patch_json(&url, &json!({ "rating": sent }))
                .await
                .assert_status(StatusCode::NO_CONTENT);
            assert_close(
                rating_of(&server, &url).await,
                stored,
                &format!("update {base} {sent}"),
            );
        }
    }
}

#[tokio::test]
async fn discover_shelves_carry_tmdb_vote_average_unscaled() {
    use wiremock::matchers::{method, path_regex};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    let server = TestServer::start_api_only().await;
    let mock = MockServer::start().await;
    sqlx::query(
        "UPDATE video_metadata_providers
         SET config = jsonb_build_object('api_key', 'test-tmdb-key', 'base_url', $1), enabled = true
         WHERE provider_type = 'tmdb'",
    )
    .bind(mock.uri())
    .execute(&server.pool)
    .await
    .unwrap();

    Mock::given(method("GET"))
        .and(path_regex("/movie"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "results": [{ "id": 1, "title": "Rated Movie", "release_date": "2024-01-01", "vote_average": 7.4, "vote_count": 10 }]
        })))
        .mount(&mock)
        .await;
    Mock::given(method("GET"))
        .and(path_regex("/tv"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "results": [{ "id": 2, "name": "Rated Show", "first_air_date": "2024-01-01", "vote_average": 8.6, "vote_count": 10 }]
        })))
        .mount(&mock)
        .await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "results": [] })))
        .mount(&mock)
        .await;

    DiscoverSyncHandler::new(
        server.pool.clone(),
        server.runtime_config.clone(),
        server.storage_root(),
    )
    .execute(Uuid::now_v7(), json!({}), CancellationToken::new())
    .await
    .unwrap();

    let sections = api::repositories::metadata_cache::list_by_provider(&server.pool, "discover")
        .await
        .unwrap();
    let ratings: Vec<(String, f64)> = sections
        .iter()
        .flat_map(|(_, s)| s["items"].as_array().cloned().unwrap_or_default())
        .filter_map(|item| {
            Some((
                item["title"].as_str()?.to_string(),
                item["rating"].as_f64()?,
            ))
        })
        .collect();
    assert!(
        !ratings.is_empty(),
        "discover sync should cache rated items"
    );
    for (title, rating) in ratings {
        let expected = if title == "Rated Movie" { 7.4 } else { 8.6 };
        assert_close(rating, expected, &title);
    }
}
