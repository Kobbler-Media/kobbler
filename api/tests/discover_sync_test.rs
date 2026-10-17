//! Tests for the discover_sync scheduled job, which runs on every boot.
//!
//! TMDB is seeded `enabled = true` with an empty config by migration
//! 0049_video_foundation.sql, so every fresh install starts with it "enabled"
//! but unconfigured — this must not be treated as a real failure.

use crate::common::*;
use api::jobs::handlers::DiscoverSyncHandler;
use api::jobs::worker::JobHandler;
use serde_json::json;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

#[tokio::test]
async fn tmdb_enabled_with_no_api_key_is_a_quiet_no_op() {
    let server = TestServer::start_api_only().await;
    // Fresh DB: TMDB is `enabled = true` (migration default) with `config = '{}'`
    // — no api_key. This is every fresh install's state, not a misconfiguration.

    let handler = DiscoverSyncHandler::new(server.pool.clone(), server.runtime_config.clone());

    let result = handler
        .execute(Uuid::now_v7(), json!({}), CancellationToken::new())
        .await
        .expect("the job itself always succeeds; the bug is what it reports");

    let errors = result["errors"].as_array().expect("errors is an array");
    assert!(
        errors.is_empty(),
        "TMDB enabled-but-unconfigured on a fresh install must not be reported \
         as a per-section error, one per discover category: {errors:?}"
    );
}

/// Kids shelves come from TMDB queries narrowed to teen-and-under family titles, and are
/// flagged so only Kids profiles get them.
#[tokio::test]
async fn sync_builds_flagged_kids_shelves_from_filtered_queries() {
    use wiremock::matchers::{method, path, query_param};
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
        .and(path("/discover/movie"))
        .and(query_param("certification_country", "US"))
        .and(query_param("certification.lte", "PG-13"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "results": [{ "id": 1, "title": "Kids Pick", "release_date": "2024-01-01" }]
        })))
        .mount(&mock)
        .await;
    Mock::given(method("GET"))
        .and(path("/discover/tv"))
        .and(query_param("with_genres", "10762|10751"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "results": [{ "id": 2, "name": "Kids Show", "first_air_date": "2024-01-01" }]
        })))
        .mount(&mock)
        .await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "results": [] })))
        .mount(&mock)
        .await;

    DiscoverSyncHandler::new(server.pool.clone(), server.runtime_config.clone())
        .execute(Uuid::now_v7(), json!({}), CancellationToken::new())
        .await
        .unwrap();

    let sections = api::repositories::metadata_cache::list_by_provider(&server.pool, "discover")
        .await
        .unwrap();
    let kids: Vec<_> = sections
        .iter()
        .filter(|(_, s)| s["kids"] == true)
        .map(|(key, s)| {
            (
                key.clone(),
                s["items"][0]["title"].as_str().unwrap_or("").to_string(),
            )
        })
        .collect();
    assert!(
        kids.contains(&("kids_movies".to_string(), "Kids Pick".to_string())),
        "{kids:?}"
    );
    assert!(
        kids.contains(&("kids_tv".to_string(), "Kids Show".to_string())),
        "{kids:?}"
    );
}
