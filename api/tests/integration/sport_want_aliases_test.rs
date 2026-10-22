//! Every path that asks Kobload for a sport event sends the names its place,
//! league and session go by, not just the title-parsed location.
//!
//! Providers are read through the metadata cache, which these tests fill, so
//! nothing here reaches Sportarr, pulselive or WorldSBK.

use crate::common::*;
use api::jobs::handlers::SportsWantHandler;
use api::jobs::worker::JobHandler;
use api::routes::admin::stream_search_request::{self, SearchTarget};
use api::video_metadata::worldsbk;
use serde_json::{Value, json};
use tokio_util::sync::CancellationToken;
use uuid::Uuid;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

struct SportEvent {
    show: Uuid,
    season: Uuid,
    episode: Uuid,
}

fn session(number: u32, id: &str, title: &str, date: &str, place: Value) -> Value {
    json!({
        "episode_number": number,
        "title": title,
        "air_date": date,
        "runtime": null,
        "still_url": null,
        "description": null,
        "provider_id": id,
        "country": place["country"],
        "air_time": place["air_time"],
        "sport": { "venue": place["venue"], "city": place["city"] },
    })
}

pub(crate) async fn cache(pool: &sqlx::PgPool, provider: &str, key: &str, value: Value) {
    let expires = time::OffsetDateTime::now_utc() + time::Duration::days(1);
    api::repositories::metadata_cache::set(pool, provider, key, &value, expires)
        .await
        .unwrap();
}

/// An SBK league searched as "WSBK", whose round 12 Superpole Race has the
/// host country's code on its row and Sportarr's venue on another session of
/// the round only.
async fn sbk_superpole_race(pool: &sqlx::PgPool) -> SportEvent {
    let league = format!("sbk-{}", Uuid::now_v7());
    let show = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO tv_shows (id, title, content_category, identifiers, sport_metadata)
         VALUES ($1, 'SBK', 'sport', $2, $3)",
    )
    .bind(show)
    .bind(json!({ "thesportsdb": league }))
    .bind(json!({ "search_title": "WSBK" }))
    .execute(pool)
    .await
    .unwrap();
    let season = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO tv_seasons (id, show_id, season_identifier, season_sort_order, is_monitored, is_special)
         VALUES ($1, $2, '2026', 2026, true, false)",
    )
    .bind(season)
    .bind(show)
    .execute(pool)
    .await
    .unwrap();
    let episode = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO tv_episodes (id, season_id, episode_identifier, episode_sort_order, title, air_date, identifiers, country)
         VALUES ($1, $2, 'evt-r12-race', 12002, 'Pirelli Spanish Round Superpole - Race', '2026-10-18', $3, 'ES')",
    )
    .bind(episode)
    .bind(season)
    .bind(json!({ "thesportsdb_id": "ev-2" }))
    .execute(pool)
    .await
    .unwrap();

    cache(
        pool,
        "thesportsdb",
        &format!("video_season:thesportsdb:{league}:s2026"),
        json!({
            "provider": "thesportsdb",
            "show_id": league,
            "season_number": 2026,
            "title": "2026",
            "air_date": "2026-10-17",
            "description": null,
            "poster_url": null,
            "episodes": [
                session(12000, "ev-1", "Pirelli Spanish Round - Race 1", "2026-10-17", json!({
                    "venue": "Circuito de Jerez – Ángel Nieto",
                    "city": "Jerez de la Frontera",
                    "country": "ES",
                })),
                session(12002, "ev-2", "Pirelli Spanish Round Superpole - Race", "2026-10-18", json!({
                    "air_time": "2026-10-18T09:00:00+00:00",
                })),
            ],
        }),
    )
    .await;
    cache(
        pool,
        "thesportsdb",
        &format!("league_alternate_names:{league}"),
        json!(["SBK", "WSB", "WSBK", "World Superbike"]),
    )
    .await;
    cache(
        pool,
        "worldsbk",
        "sbk_rounds:2026",
        json!(
            worldsbk::parse_rounds(include_str!("../fixtures/worldsbk/rounds_2026.json")).unwrap()
        ),
    )
    .await;
    cache(
        pool,
        "worldsbk",
        "sbk_sessions:2026:JER",
        json!(
            worldsbk::parse_sessions(include_str!("../fixtures/worldsbk/sessions_2026_jer.json"))
                .unwrap()
        ),
    )
    .await;

    SportEvent {
        show,
        season,
        episode,
    }
}

fn names(want: &Value, field: &str) -> Vec<String> {
    want[field]
        .as_array()
        .unwrap_or_else(|| panic!("{field} missing from {want}"))
        .iter()
        .map(|v| v.as_str().unwrap().to_string())
        .collect()
}

fn assert_carries_the_jerez_round(entry: &str, want: &Value) {
    assert_eq!(want["kind"], "sport_event", "{entry}: {want}");
    assert_eq!(want["league_name"], "WSBK", "{entry}");
    assert_eq!(want["location"], "Spain", "{entry}");
    assert_eq!(want["session_type"], "Superpole Race", "{entry}");
    assert_eq!(want["class"], "SBK", "{entry}");
    assert_eq!(want["venue"], "Circuito de Jerez – Ángel Nieto", "{entry}");
    assert_eq!(
        want["session_start"], "2026-10-18T09:00:00+00:00",
        "{entry}"
    );
    let places = names(want, "location_aliases");
    for place in ["Spain", "Jerez", "Spanish", "Andalusia"] {
        assert!(
            places.iter().any(|p| p == place),
            "{entry}: {place} not in {places:?}"
        );
    }
    assert_eq!(
        names(want, "league_aliases"),
        ["SBK", "WSB", "World Superbike"],
        "{entry}"
    );
    assert!(
        names(want, "event_aliases").contains(&"Pirelli Spanish Round".to_string()),
        "{entry}: {want}"
    );
    assert_eq!(want["identifiers"]["official_id"], "2026-JER", "{entry}");
}

#[tokio::test]
async fn sports_want_job_sends_the_rounds_aliases() {
    let server = TestServer::start_api_only().await;
    configure_kobload(&server).await;
    let event = sbk_superpole_race(&server.pool).await;

    let handler = SportsWantHandler::new(
        server.pool.clone(),
        server.runtime_config.clone(),
        server.nats_publisher.clone(),
        server.library_events.clone(),
    );
    let tip = stream_tip(&server).await;
    handler
        .execute(
            Uuid::now_v7(),
            json!({
                "show_id": event.show,
                "season_id": event.season,
                "episode_ids": [event.episode],
            }),
            CancellationToken::new(),
        )
        .await
        .unwrap();

    let sent = want_requests_since(&server, tip, &event.episode.to_string()).await;
    let request = sent.last().expect("sports_want published nothing");
    assert_carries_the_jerez_round("sports_want", &request["media_want"]);
}

#[tokio::test]
async fn stream_search_sends_the_rounds_aliases() {
    let server = TestServer::start_api_only().await;
    let event = sbk_superpole_race(&server.pool).await;

    let Ok(prepared) = stream_search_request::prepare(
        &server.app_state,
        Uuid::now_v7(),
        &SearchTarget::Episode(event.episode),
        None,
    )
    .await
    else {
        panic!("stream search request was refused");
    };
    let want = serde_json::to_value(prepared.request.media_want).unwrap();
    assert_carries_the_jerez_round("stream search", &want);
}

#[tokio::test]
async fn release_searches_send_the_rounds_aliases() {
    let kobload = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/api/v1/search"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "results": [] })))
        .mount(&kobload)
        .await;

    let server = TestServer::start_api_only().await;
    configure_kobload_at(&server, &kobload.uri()).await;
    let event = sbk_superpole_race(&server.pool).await;

    for route in [
        format!("/api/v1/admin/sports/events/{}/releases", event.episode),
        format!("/api/v1/admin/tv/episodes/{}/releases", event.episode),
    ] {
        server.admin_client().get(&route).await.assert_ok();
        let requests = kobload.received_requests().await.unwrap();
        let body: Value = serde_json::from_slice(&requests.last().unwrap().body).unwrap();
        assert_carries_the_jerez_round(&route, &body["media_want"]);
    }
}

/// A league with one season and the given events, each `(id, title, date,
/// sport source)`. Returns (show, season, episode ids).
pub(crate) async fn league(
    pool: &sqlx::PgPool,
    title: &str,
    events: &[(&str, &str, &str, Value)],
) -> (Uuid, Uuid, Vec<Uuid>) {
    let league = format!("lg-{}", Uuid::now_v7());
    let show = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO tv_shows (id, title, content_category, identifiers) VALUES ($1, $2, 'sport', $3)",
    )
    .bind(show)
    .bind(title)
    .bind(json!({ "thesportsdb": league }))
    .execute(pool)
    .await
    .unwrap();
    let season = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO tv_seasons (id, show_id, season_identifier, season_sort_order, is_monitored, is_special)
         VALUES ($1, $2, '2026', 2026, true, false)",
    )
    .bind(season)
    .bind(show)
    .execute(pool)
    .await
    .unwrap();

    let mut episodes = Vec::new();
    let mut rows = Vec::new();
    for (n, (id, event_title, date, sport)) in events.iter().enumerate() {
        let episode = Uuid::now_v7();
        sqlx::query(
            "INSERT INTO tv_episodes (id, season_id, episode_identifier, episode_sort_order, title, air_date, identifiers)
             VALUES ($1, $2, $3, $4, $5, $6::DATE, $7)",
        )
        .bind(episode)
        .bind(season)
        .bind(format!("evt-{id}"))
        .bind(1000 + n as i32)
        .bind(event_title)
        .bind(date)
        .bind(json!({ "thesportsdb_id": id }))
        .execute(pool)
        .await
        .unwrap();
        episodes.push(episode);
        rows.push(json!({
            "episode_number": 1000 + n,
            "title": event_title,
            "air_date": date,
            "runtime": null,
            "still_url": null,
            "description": null,
            "provider_id": id,
            "sport": sport,
        }));
    }
    cache(
        pool,
        "thesportsdb",
        &format!("video_season:thesportsdb:{league}:s2026"),
        json!({
            "provider": "thesportsdb", "show_id": league, "season_number": 2026,
            "title": "2026", "air_date": null, "description": null, "poster_url": null,
            "episodes": rows,
        }),
    )
    .await;
    cache(
        pool,
        "thesportsdb",
        &format!("league_alternate_names:{league}"),
        json!([]),
    )
    .await;
    (show, season, episodes)
}

pub(crate) async fn run_sports_want(
    server: &TestServer,
    show: Uuid,
    season: Uuid,
    episodes: &[Uuid],
) -> u64 {
    let handler = SportsWantHandler::new(
        server.pool.clone(),
        server.runtime_config.clone(),
        server.nats_publisher.clone(),
        server.library_events.clone(),
    );
    let tip = stream_tip(server).await;
    handler
        .execute(
            Uuid::now_v7(),
            json!({ "show_id": show, "season_id": season, "episode_ids": episodes }),
            CancellationToken::new(),
        )
        .await
        .unwrap();
    tip
}

#[tokio::test]
async fn sports_want_skips_called_off_events_and_names_the_teams() {
    let server = TestServer::start_api_only().await;
    configure_kobload(&server).await;
    let team = |home: &str, home_id: &str, away: &str, away_id: &str, status: &str| {
        json!({
            "status": status,
            "home_team": home, "home_team_id": home_id,
            "away_team": away, "away_team_id": away_id,
        })
    };
    let (show, season, episodes) = league(
        &server.pool,
        "English Premier League",
        &[
            (
                "ev-a",
                "Arsenal vs Coventry City",
                "2026-08-22",
                team(
                    "Arsenal",
                    "tm-000006",
                    "Coventry City",
                    "tm-000032",
                    "scheduled",
                ),
            ),
            (
                "ev-b",
                "Chelsea vs Fulham",
                "2026-08-23",
                team("Chelsea", "tm-1", "Fulham", "tm-2", "postponed"),
            ),
            (
                "ev-c",
                "Leeds United vs Everton",
                "2026-08-24",
                team("Leeds United", "tm-3", "Everton", "tm-4", "Match Cancelled"),
            ),
        ],
    )
    .await;
    cache(
        &server.pool,
        "thesportsdb",
        "team:tm-000006",
        json!({ "name": "Arsenal", "short": "ARS", "aliases": ["Gunners"] }),
    )
    .await;
    cache(
        &server.pool,
        "thesportsdb",
        "team:tm-000032",
        json!({ "name": "Coventry City", "short": "COV" }),
    )
    .await;

    let tip = run_sports_want(&server, show, season, &episodes).await;

    for called_off in &episodes[1..] {
        assert!(
            want_requests_since(&server, tip, &called_off.to_string())
                .await
                .is_empty(),
            "a called-off event was wanted"
        );
    }
    let sent = want_requests_since(&server, tip, &episodes[0].to_string()).await;
    let want = &sent.last().expect("the scheduled match was not wanted")["media_want"];
    assert_eq!(
        want["teams"],
        json!([
            { "name": "Arsenal", "short": "ARS", "aliases": ["Gunners"] },
            { "name": "Coventry City", "short": "COV" },
        ]),
        "{want}"
    );
}

#[tokio::test]
async fn f1_race_carries_jolpica_circuit_and_session_start() {
    let server = TestServer::start_api_only().await;
    configure_kobload(&server).await;
    let (show, season, episodes) = league(
        &server.pool,
        "Formula 1",
        &[(
            "ev-m",
            "Miami Grand Prix Qualifying",
            "2026-05-02",
            json!({}),
        )],
    )
    .await;
    cache(
        &server.pool,
        "jolpica",
        "f1_races:2026",
        json!([{
            "season": "2026", "round": "4", "raceName": "Miami Grand Prix",
            "Circuit": { "circuitId": "miami", "circuitName": "Miami International Autodrome",
                         "Location": { "locality": "Miami", "country": "USA" } },
            "date": "2026-05-03", "time": "20:00:00Z",
            "FirstPractice": { "date": "2026-05-01", "time": "16:00:00Z" },
            "Qualifying": { "date": "2026-05-02", "time": "20:00:00Z" },
            "Sprint": { "date": "2026-05-02", "time": "16:00:00Z" },
        }]),
    )
    .await;

    let tip = run_sports_want(&server, show, season, &episodes).await;
    let sent = want_requests_since(&server, tip, &episodes[0].to_string()).await;
    let want = &sent.last().expect("the qualifying was not wanted")["media_want"];
    assert_eq!(want["class"], "F1", "{want}");
    assert_eq!(want["venue"], "Miami International Autodrome", "{want}");
    assert_eq!(want["city"], "Miami", "{want}");
    assert_eq!(want["country_iso"], "US", "{want}");
    assert_eq!(want["session_start"], "2026-05-02T20:00:00Z", "{want}");
    assert!(
        names(want, "event_aliases").contains(&"Miami Grand Prix".to_string()),
        "{want}"
    );
}
