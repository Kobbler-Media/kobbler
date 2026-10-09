//! Per-episode credits: guest stars fetched lazily from the metadata provider
//! and cached, kept distinct from the show-level series regulars.

use crate::common::*;
use reqwest::StatusCode;
use serde_json::{Value, json};
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

// =============================================================================
// Fixtures
// =============================================================================

fn episode_credits_payload() -> Value {
    json!({
        "id": 62130,
        "guest_stars": [
            {"id": 84497, "name": "Dale Dickey", "character": "Spooge's Woman",
             "profile_path": "/dd.jpg", "order": 0},
            {"id": 1223786, "name": "David Ury", "character": "Spooge",
             "profile_path": null, "order": 1}
        ],
        "crew": [
            {"id": 66633, "name": "Peter Gould", "department": "Directing",
             "job": "Director", "profile_path": null},
            {"id": 2, "name": "Best Boy", "department": "Lighting",
             "job": "Best Boy", "profile_path": null}
        ]
    })
}

/// The season payload TMDB already returns to enrichment, guest stars included.
fn season_payload() -> Value {
    json!({
        "season_number": 2,
        "name": "Season 2",
        "air_date": "2009-03-08",
        "overview": "",
        "poster_path": null,
        "episodes": [{
            "id": 62130,
            "episode_number": 6,
            "name": "Peekaboo",
            "overview": "",
            "air_date": "2009-04-12",
            "runtime": 47,
            "still_path": null,
            "guest_stars": [
                {"id": 84497, "name": "Dale Dickey", "character": "Spooge's Woman",
                 "profile_path": "/dd.jpg", "order": 0},
                {"id": 1223786, "name": "David Ury", "character": "Spooge",
                 "profile_path": null, "order": 1}
            ],
            "crew": [
                {"id": 66633, "name": "Peter Gould", "department": "Directing",
                 "job": "Director", "profile_path": null},
                {"id": 2, "name": "Best Boy", "department": "Lighting",
                 "job": "Best Boy", "profile_path": null}
            ]
        }]
    })
}

async fn configure_tmdb(server: &TestServer, mock: &MockServer) {
    sqlx::query(
        r#"
        UPDATE video_metadata_providers
        SET config = jsonb_build_object('api_key', 'test-tmdb-key', 'base_url', $1),
            enabled = true
        WHERE provider_type = 'tmdb'
        "#,
    )
    .bind(mock.uri())
    .execute(&server.pool)
    .await
    .expect("configure tmdb");
}

struct Fixture {
    show_id: String,
    episode_id: String,
}

/// A show carrying a tmdb_id, one season, one episode.
async fn setup_show(server: &TestServer, tmdb_id: &str) -> Fixture {
    let show: Value = server
        .admin_client()
        .post_json(
            "/api/v1/admin/tv/shows",
            &json!({
                "title": "Guest Star Show",
                "year": 2009,
                "identifiers": { "tmdb_id": tmdb_id },
            }),
        )
        .await
        .json();
    let show_id = show["id"].as_str().unwrap().to_string();

    let season: Value = server
        .admin_client()
        .post_json(
            &format!("/api/v1/admin/tv/shows/{show_id}/seasons"),
            &json!({ "season_identifier": "2", "season_sort_order": 2 }),
        )
        .await
        .json();
    let season_id = season["id"].as_str().unwrap().to_string();

    let episode: Value = server
        .admin_client()
        .post_json(
            &format!("/api/v1/admin/tv/seasons/{season_id}/episodes"),
            &json!({
                "episode_identifier": "6",
                "episode_sort_order": 6,
                "title": "Peekaboo",
            }),
        )
        .await
        .json();

    Fixture {
        show_id,
        episode_id: episode["id"].as_str().unwrap().to_string(),
    }
}

async fn add_series_regular(server: &TestServer, show_id: &str, name: &str, tmdb_person: &str) {
    let person: Value = server
        .admin_client()
        .post_json(
            "/api/v1/admin/people",
            &json!({ "name": name, "identifiers": { "tmdb_id": tmdb_person } }),
        )
        .await
        .json();
    let person_id = person["id"].as_str().unwrap();

    sqlx::query(
        "INSERT INTO tv_cast (id, show_id, person_id, character_name, cast_order) \
         VALUES (gen_random_uuid(), $1::uuid, $2::uuid, $3, 0)",
    )
    .bind(show_id)
    .bind(person_id)
    .bind("Jesse Pinkman")
    .execute(&server.pool)
    .await
    .expect("insert tv_cast");
}

// =============================================================================
// Tests
// =============================================================================

/// The headline claim: the guest list is already in a response kobbler makes,
/// so answering costs no extra API call.
#[tokio::test]
async fn guest_stars_come_from_the_season_payload_without_an_extra_call() {
    let server = TestServer::start_api_only().await;
    let mock = MockServer::start().await;
    configure_tmdb(&server, &mock).await;

    Mock::given(method("GET"))
        .and(path("/tv/1396/season/2"))
        .respond_with(ResponseTemplate::new(200).set_body_json(season_payload()))
        .mount(&mock)
        .await;
    Mock::given(method("GET"))
        .and(path("/tv/1396/season/2/episode/6/credits"))
        .respond_with(ResponseTemplate::new(200).set_body_json(episode_credits_payload()))
        .expect(0)
        .mount(&mock)
        .await;

    let fx = setup_show(&server, "1396").await;
    let body: Value = server
        .admin_client()
        .get(&format!("/api/v1/admin/tv/episodes/{}/cast", fx.episode_id))
        .await
        .json();

    assert_eq!(body["guest_source"], "provider");
    let guests: Vec<&str> = body["guest_stars"]
        .as_array()
        .unwrap()
        .iter()
        .map(|g| g["name"].as_str().unwrap())
        .collect();
    assert_eq!(guests, vec!["Dale Dickey", "David Ury"]);

    mock.verify().await;
}

#[tokio::test]
async fn guest_stars_are_fetched_on_first_ask_and_cached() {
    let server = TestServer::start_api_only().await;
    let mock = MockServer::start().await;
    configure_tmdb(&server, &mock).await;

    // The season payload is unavailable, so the per-episode endpoint answers.
    Mock::given(method("GET"))
        .and(path("/tv/1396/season/2"))
        .respond_with(ResponseTemplate::new(404))
        .mount(&mock)
        .await;
    Mock::given(method("GET"))
        .and(path("/tv/1396/season/2/episode/6/credits"))
        .respond_with(ResponseTemplate::new(200).set_body_json(episode_credits_payload()))
        // Exactly once: the second ask must come from tv_episode_cast.
        .expect(1)
        .mount(&mock)
        .await;

    let fx = setup_show(&server, "1396").await;
    add_series_regular(&server, &fx.show_id, "Aaron Paul", "84497000").await;

    let url = format!("/api/v1/admin/tv/episodes/{}/cast", fx.episode_id);

    let first: Value = server.admin_client().get(&url).await.json();
    assert_eq!(first["guest_source"], "provider");
    let guests: Vec<&str> = first["guest_stars"]
        .as_array()
        .unwrap()
        .iter()
        .map(|g| g["name"].as_str().unwrap())
        .collect();
    assert_eq!(guests, vec!["Dale Dickey", "David Ury"]);
    assert_eq!(first["guest_stars"][0]["character"], "Spooge's Woman");
    assert_eq!(first["crew"].as_array().unwrap().len(), 1);
    assert_eq!(first["crew"][0]["name"], "Peter Gould");
    assert_eq!(first["series_regulars"][0]["name"], "Aaron Paul");

    let cached: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM tv_episode_cast WHERE episode_id = $1::uuid")
            .bind(&fx.episode_id)
            .fetch_one(&server.pool)
            .await
            .unwrap();
    assert_eq!(cached, 3, "two guests and one director should be cached");

    let second: Value = server.admin_client().get(&url).await.json();
    assert_eq!(second["guest_source"], "cache");
    assert_eq!(second["guest_stars"], first["guest_stars"]);

    mock.verify().await;
}

#[tokio::test]
async fn an_episode_with_no_guests_is_not_refetched() {
    let server = TestServer::start_api_only().await;
    let mock = MockServer::start().await;
    configure_tmdb(&server, &mock).await;

    Mock::given(method("GET"))
        .and(path("/tv/1396/season/2"))
        .respond_with(ResponseTemplate::new(404))
        .mount(&mock)
        .await;
    Mock::given(method("GET"))
        .and(path("/tv/1396/season/2/episode/6/credits"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(json!({ "id": 1, "guest_stars": [], "crew": [] })),
        )
        .expect(1)
        .mount(&mock)
        .await;

    let fx = setup_show(&server, "1396").await;
    let url = format!("/api/v1/admin/tv/episodes/{}/cast", fx.episode_id);

    let first: Value = server.admin_client().get(&url).await.json();
    assert_eq!(first["guest_source"], "provider");
    assert!(first["guest_stars"].as_array().unwrap().is_empty());

    let second: Value = server.admin_client().get(&url).await.json();
    assert_eq!(
        second["guest_source"], "cache",
        "an empty guest list is an answer, not a cache miss"
    );

    mock.verify().await;
}

#[tokio::test]
async fn a_series_regular_is_never_listed_as_a_guest() {
    let server = TestServer::start_api_only().await;
    let mock = MockServer::start().await;
    configure_tmdb(&server, &mock).await;

    Mock::given(method("GET"))
        .and(path("/tv/1396/season/2"))
        .respond_with(ResponseTemplate::new(404))
        .mount(&mock)
        .await;
    Mock::given(method("GET"))
        .and(path("/tv/1396/season/2/episode/6/credits"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "id": 1,
            "guest_stars": [
                {"id": 17419, "name": "Bryan Cranston", "character": "Walter White", "order": 0}
            ],
            "crew": []
        })))
        .mount(&mock)
        .await;

    let fx = setup_show(&server, "1396").await;
    add_series_regular(&server, &fx.show_id, "Bryan Cranston", "17419").await;

    let body: Value = server
        .admin_client()
        .get(&format!("/api/v1/admin/tv/episodes/{}/cast", fx.episode_id))
        .await
        .json();

    assert!(
        body["guest_stars"].as_array().unwrap().is_empty(),
        "the whole point of the endpoint is the regular/guest distinction"
    );
    assert_eq!(body["series_regulars"][0]["name"], "Bryan Cranston");
}

#[tokio::test]
async fn unavailable_when_no_provider_can_answer() {
    let server = TestServer::start_api_only().await;

    // No tmdb_id on the show, so nothing can be asked.
    let show: Value = server
        .admin_client()
        .post_json(
            "/api/v1/admin/tv/shows",
            &json!({ "title": "Unidentified Show", "year": 2009 }),
        )
        .await
        .json();
    let show_id = show["id"].as_str().unwrap();
    let season: Value = server
        .admin_client()
        .post_json(
            &format!("/api/v1/admin/tv/shows/{show_id}/seasons"),
            &json!({ "season_identifier": "1", "season_sort_order": 1 }),
        )
        .await
        .json();
    let episode: Value = server
        .admin_client()
        .post_json(
            &format!(
                "/api/v1/admin/tv/seasons/{}/episodes",
                season["id"].as_str().unwrap()
            ),
            &json!({ "episode_identifier": "1", "episode_sort_order": 1 }),
        )
        .await
        .json();

    let body: Value = server
        .admin_client()
        .get(&format!(
            "/api/v1/admin/tv/episodes/{}/cast",
            episode["id"].as_str().unwrap()
        ))
        .await
        .json();

    assert_eq!(
        body["guest_source"], "unavailable",
        "\"we could not find out\" must not look like \"nobody guest-starred\""
    );
}

#[tokio::test]
async fn unknown_episode_is_404() {
    let server = TestServer::start_api_only().await;

    let resp = server
        .admin_client()
        .get(&format!(
            "/api/v1/admin/tv/episodes/{}/cast",
            uuid::Uuid::now_v7()
        ))
        .await;

    resp.assert_status(StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn requires_authentication() {
    let server = TestServer::start_api_only().await;
    let fx = setup_show(&server, "1396").await;

    let resp = TestClient::new(&server.api_url)
        .get(&format!("/api/v1/admin/tv/episodes/{}/cast", fx.episode_id))
        .await;

    assert_eq!(resp.status, StatusCode::UNAUTHORIZED);
}

/// Reads are free for every role — the design's role table gives `viewer`
/// "ask anything", and "who's that guy" is exactly that.
#[tokio::test]
async fn a_viewer_may_ask() {
    let server = TestServer::start_api_only().await;
    let fx = setup_show(&server, "1396").await;
    let viewer = create_user_client(&server, "viewer-cast@test.com", "viewer").await;

    let resp = viewer
        .get(&format!("/api/v1/admin/tv/episodes/{}/cast", fx.episode_id))
        .await;

    assert_ne!(resp.status, StatusCode::FORBIDDEN);
    resp.assert_ok();
}
