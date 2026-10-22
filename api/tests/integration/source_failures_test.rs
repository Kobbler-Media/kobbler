//! The optional metadata sources a want is enriched from fail cheaply: a
//! source that errors is left alone for an hour, a slow one is cut off, and
//! the want still goes out with what the other sources found. A key that
//! genuinely has nothing (no such team) is remembered like any result.

use crate::common::*;
use crate::sport_want_aliases_test::{league, run_sports_want};
use serde_json::{Value, json};
use std::time::{Duration, Instant};
use uuid::Uuid;
use wiremock::matchers::{method, path, path_regex};
use wiremock::{Mock, MockServer, ResponseTemplate};

async fn sport_sources_at(server: &TestServer, mock: &MockServer) {
    sqlx::query(
        "UPDATE video_metadata_providers
         SET config = config || jsonb_build_object('pulselive_url', $1::TEXT,
                                                   'jolpica_url', $1::TEXT,
                                                   'sportarr_url', $1::TEXT)
         WHERE provider_type = 'thesportsdb'",
    )
    .bind(mock.uri())
    .execute(&server.pool)
    .await
    .unwrap();
}

async fn calls(mock: &MockServer, prefix: &str) -> usize {
    mock.received_requests()
        .await
        .unwrap()
        .iter()
        .filter(|r| r.url.path().starts_with(prefix))
        .count()
}

async fn expire_failures(server: &TestServer) {
    sqlx::query(
        "UPDATE metadata_cache SET expires_at = NOW() - INTERVAL '1 second'
         WHERE query_key LIKE 'failed:%'",
    )
    .execute(&server.pool)
    .await
    .unwrap();
}

async fn motogp_race(server: &TestServer) -> (Uuid, Uuid, Vec<Uuid>) {
    league(
        &server.pool,
        "MotoGP",
        &[(
            "ev-r",
            "USA - Race",
            "2026-03-29",
            json!({ "venue": "Circuit of the Americas", "city": "Austin" }),
        )],
    )
    .await
}

async fn wanted(server: &TestServer, tip: u64, episode: Uuid) -> Value {
    let sent = want_requests_since(server, tip, &episode.to_string()).await;
    sent.last().expect("the want was not sent")["media_want"].clone()
}

#[tokio::test]
async fn a_failing_source_is_left_alone_for_an_hour() {
    let server = TestServer::start_api_only().await;
    configure_kobload(&server).await;
    let mock = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path_regex("^/seasons"))
        .respond_with(ResponseTemplate::new(503))
        .mount(&mock)
        .await;
    sport_sources_at(&server, &mock).await;
    let (show, season, episodes) = motogp_race(&server).await;

    for attempt in 0..2 {
        let tip = run_sports_want(&server, show, season, &episodes).await;
        let want = wanted(&server, tip, episodes[0]).await;
        assert_eq!(
            want["venue"], "Circuit of the Americas",
            "attempt {attempt}: {want}"
        );
    }
    assert_eq!(
        calls(&mock, "/seasons").await,
        1,
        "retried inside the window"
    );

    expire_failures(&server).await;
    run_sports_want(&server, show, season, &episodes).await;
    assert_eq!(
        calls(&mock, "/seasons").await,
        2,
        "not retried after the window"
    );
}

#[tokio::test]
async fn a_slow_source_is_cut_off() {
    let server = TestServer::start_api_only().await;
    configure_kobload(&server).await;
    let mock = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path_regex("^/seasons"))
        .respond_with(ResponseTemplate::new(200).set_delay(Duration::from_secs(25)))
        .mount(&mock)
        .await;
    sport_sources_at(&server, &mock).await;
    let (show, season, episodes) = motogp_race(&server).await;

    let started = Instant::now();
    let tip = run_sports_want(&server, show, season, &episodes).await;
    let took = started.elapsed();
    assert!(
        took < Duration::from_secs(12),
        "waited {took:?} on a slow source"
    );
    let want = wanted(&server, tip, episodes[0]).await;
    assert_eq!(want["venue"], "Circuit of the Americas", "{want}");
}

/// No such team is an answer, not an outage: it is not asked again.
#[tokio::test]
async fn a_team_sportarr_does_not_know_is_not_asked_again() {
    let server = TestServer::start_api_only().await;
    configure_kobload(&server).await;
    let mock = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/lookup/team/tm-gone"))
        .respond_with(ResponseTemplate::new(404))
        .mount(&mock)
        .await;
    sport_sources_at(&server, &mock).await;
    let (show, season, episodes) = league(
        &server.pool,
        "English Premier League",
        &[(
            "ev-x",
            "Arsenal vs Gone FC",
            "2026-08-22",
            json!({ "home_team": "Arsenal", "away_team": "Gone FC", "away_team_id": "tm-gone" }),
        )],
    )
    .await;

    for _ in 0..2 {
        let tip = run_sports_want(&server, show, season, &episodes).await;
        let want = wanted(&server, tip, episodes[0]).await;
        assert_eq!(
            want["teams"],
            json!([{ "name": "Arsenal" }, { "name": "Gone FC" }]),
            "{want}"
        );
    }
    expire_failures(&server).await;
    run_sports_want(&server, show, season, &episodes).await;
    assert_eq!(calls(&mock, "/lookup/team").await, 1);
}

/// TMDB's alternative titles go through the same guard: a Discover search
/// while TMDB is failing is sent with the title's own variants, and TMDB is
/// not asked again on the next search.
#[tokio::test]
async fn failing_tmdb_alternate_titles_are_not_retried_per_search() {
    let kobload = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path_regex("/api/v1/stream/search$"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "options": [] })))
        .mount(&kobload)
        .await;
    let tmdb = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/movie/1091/alternative_titles"))
        .respond_with(ResponseTemplate::new(500))
        .mount(&tmdb)
        .await;
    let server = TestServer::start_api_only().await;
    configure_kobload_at(&server, &kobload.uri()).await;
    sqlx::query(
        "UPDATE video_metadata_providers
         SET config = jsonb_build_object('api_key', 'test-tmdb-key', 'base_url', $1), enabled = true
         WHERE provider_type = 'tmdb'",
    )
    .bind(tmdb.uri())
    .execute(&server.pool)
    .await
    .unwrap();

    for _ in 0..2 {
        server
            .admin_client()
            .post_json(
                "/api/v1/admin/stream/search",
                &json!({ "title": "The Thing", "tmdb_id": 1091, "media_type": "movie" }),
            )
            .await
            .assert_ok();
        let sent = kobload.received_requests().await.unwrap();
        let want: Value = serde_json::from_slice(&sent.last().unwrap().body).unwrap();
        assert_eq!(want["media_want"]["alternate_titles"], json!(["Thing"]));
    }
    assert_eq!(calls(&tmdb, "/movie/1091/alternative_titles").await, 1);
}
