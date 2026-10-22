//! Every path that asks Kobload for a movie or an episode sends the title's
//! alternate names, with and without a leading "The", and titles that are not
//! in the library get theirs from TMDB once, through the metadata cache.

use crate::common::*;
use api::jobs::handlers::{MovieWantHandler, TvWantHandler};
use api::jobs::worker::JobHandler;
use serde_json::{Value, json};
use tokio_util::sync::CancellationToken;
use uuid::Uuid;
use wiremock::matchers::{method, path, path_regex};
use wiremock::{Mock, MockServer, ResponseTemplate};

const MOVIE_ALTERNATES: &[&str] = &["La Cosa", "Thing"];
const SHOW_ALTERNATES: &[&str] = &["Office (US)", "Office"];

struct Rig {
    server: TestServer,
    kobload: MockServer,
    stremio_key: String,
}

async fn rig() -> Rig {
    let kobload = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path_regex("/api/v1/stream/search$"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "options": [] })))
        .mount(&kobload)
        .await;
    Mock::given(method("POST"))
        .and(path("/api/v1/search"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "results": [] })))
        .mount(&kobload)
        .await;

    let server = TestServer::start_api_only().await;
    configure_kobload_at(&server, &kobload.uri()).await;
    let user: Uuid = sqlx::query_scalar("SELECT id FROM users WHERE role = 'admin' LIMIT 1")
        .fetch_one(&server.pool)
        .await
        .unwrap();
    let stremio_key = format!("alt-{}", Uuid::now_v7().simple());
    sqlx::query("INSERT INTO stremio_api_keys (user_id, api_key) VALUES ($1, $2)")
        .bind(user)
        .bind(&stremio_key)
        .execute(&server.pool)
        .await
        .unwrap();
    Rig {
        server,
        kobload,
        stremio_key,
    }
}

impl Rig {
    async fn sent_to_kobload(&self, suffix: &str) -> Value {
        let requests = self.kobload.received_requests().await.unwrap();
        let last = requests
            .iter()
            .rev()
            .find(|r| r.url.path().ends_with(suffix))
            .unwrap_or_else(|| panic!("nothing sent to {suffix}"));
        serde_json::from_slice::<Value>(&last.body).unwrap()["media_want"].clone()
    }

    async fn stream_search(&self, body: Value) -> Value {
        self.server
            .admin_client()
            .post_json("/api/v1/admin/stream/search", &body)
            .await
            .assert_ok();
        self.sent_to_kobload("/stream/search").await
    }

    async fn release_search(&self, route: &str) -> Value {
        self.server.admin_client().get(route).await.assert_ok();
        self.sent_to_kobload("/api/v1/search").await
    }

    async fn stremio(&self, kind: &str, id: &str) -> Value {
        TestClient::new(&self.server.api_url)
            .get(&format!(
                "/stremio/{}/stream/{kind}/{id}.json",
                self.stremio_key
            ))
            .await
            .assert_ok();
        self.sent_to_kobload("/stream/search").await
    }
}

fn assert_alternates(entry: &str, want: &Value, expected: &[&str], primary: &str) {
    let titles: Vec<&str> = want["alternate_titles"]
        .as_array()
        .unwrap_or_else(|| panic!("{entry}: no alternate_titles in {want}"))
        .iter()
        .map(|t| t.as_str().unwrap())
        .collect();
    for title in expected {
        assert!(titles.contains(title), "{entry}: {title} not in {titles:?}");
    }
    assert!(
        !titles.contains(&primary),
        "{entry}: the title itself is not an alternate: {titles:?}"
    );
}

async fn library_movie(pool: &sqlx::PgPool, imdb_id: &str) -> Uuid {
    let id = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO movies (id, title, year, identifiers, alternate_titles, enriched_at)
         VALUES ($1, 'The Thing', 1982, $2, ARRAY['La Cosa'], NOW())",
    )
    .bind(id)
    .bind(json!({ "tmdb_id": "1091", "imdb_id": imdb_id }))
    .execute(pool)
    .await
    .unwrap();
    id
}

/// Returns (show, season, episode).
async fn library_show(pool: &sqlx::PgPool, imdb_id: &str) -> (Uuid, Uuid, Uuid) {
    let show = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO tv_shows (id, title, year, identifiers, alternate_titles)
         VALUES ($1, 'The Office', 2005, $2, ARRAY['Office (US)'])",
    )
    .bind(show)
    .bind(json!({ "tmdb_id": "2316", "imdb_id": imdb_id }))
    .execute(pool)
    .await
    .unwrap();
    let season = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO tv_seasons (id, show_id, season_identifier, season_sort_order, is_monitored, is_special)
         VALUES ($1, $2, '1', 1, true, false)",
    )
    .bind(season)
    .bind(show)
    .execute(pool)
    .await
    .unwrap();
    let episode = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO tv_episodes (id, season_id, episode_identifier, episode_sort_order, title)
         VALUES ($1, $2, '1', 1, 'Pilot')",
    )
    .bind(episode)
    .bind(season)
    .execute(pool)
    .await
    .unwrap();
    (show, season, episode)
}

#[tokio::test]
async fn library_movie_wants_carry_alternate_titles() {
    let rig = rig().await;
    let movie = library_movie(&rig.server.pool, "tt0084787").await;

    let handler = MovieWantHandler::new(
        rig.server.pool.clone(),
        rig.server.runtime_config.clone(),
        rig.server.nats_publisher.clone(),
        rig.server.library_events.clone(),
    );
    let tip = stream_tip(&rig.server).await;
    handler
        .execute(
            Uuid::now_v7(),
            json!({ "movie_id": movie }),
            CancellationToken::new(),
        )
        .await
        .unwrap();
    let sent = want_requests_since(&rig.server, tip, &movie.to_string()).await;
    let job = sent.last().expect("movie_want published nothing")["media_want"].clone();

    let entries = [
        ("movie_want", job),
        (
            "movie release search",
            rig.release_search(&format!("/api/v1/admin/movies/{movie}/releases"))
                .await,
        ),
        (
            "stream search",
            rig.stream_search(json!({ "movie_id": movie })).await,
        ),
        ("stremio", rig.stremio("movie", "tt0084787").await),
    ];
    for (entry, want) in entries {
        assert_alternates(entry, &want, MOVIE_ALTERNATES, "The Thing");
    }
}

#[tokio::test]
async fn library_show_wants_carry_alternate_titles() {
    let rig = rig().await;
    let (show, season, episode) = library_show(&rig.server.pool, "tt0386676").await;

    let handler = TvWantHandler::new(
        rig.server.pool.clone(),
        rig.server.runtime_config.clone(),
        rig.server.nats_publisher.clone(),
        rig.server.library_events.clone(),
    );
    let tip = stream_tip(&rig.server).await;
    handler
        .execute(
            Uuid::now_v7(),
            json!({ "show_id": show, "season_id": season, "episode_ids": [episode] }),
            CancellationToken::new(),
        )
        .await
        .unwrap();
    let sent = want_requests_since(&rig.server, tip, &season.to_string()).await;
    let job = sent.last().expect("tv_want published nothing")["media_want"].clone();

    let built = rig
        .server
        .admin_client()
        .post_json(
            "/api/v1/providers/library/build-media-want",
            &json!({ "show_id": show, "season_id": season }),
        )
        .await;
    built.assert_ok();
    let built: Value = built.json();

    let entries = [
        ("tv_want", job),
        ("build-media-want", built),
        (
            "episode release search",
            rig.release_search(&format!("/api/v1/admin/tv/episodes/{episode}/releases"))
                .await,
        ),
        (
            "stream search",
            rig.stream_search(json!({ "episode_id": episode })).await,
        ),
        ("stremio", rig.stremio("series", "tt0386676:1:1").await),
    ];
    for (entry, want) in entries {
        assert_alternates(entry, &want, SHOW_ALTERNATES, "The Office");
    }
}

async fn tmdb(rig: &Rig) -> MockServer {
    let tmdb = MockServer::start().await;
    sqlx::query(
        "UPDATE video_metadata_providers
         SET config = jsonb_build_object('api_key', 'test-tmdb-key', 'base_url', $1), enabled = true
         WHERE provider_type = 'tmdb'",
    )
    .bind(tmdb.uri())
    .execute(&rig.server.pool)
    .await
    .unwrap();

    let routes = [
        (
            "/movie/1091/alternative_titles",
            json!({ "id": 1091, "titles": [
                { "iso_3166_1": "US", "title": "La Cosa" },
                { "iso_3166_1": "FR", "title": "The Thing (FR)" },
            ]}),
        ),
        (
            "/tv/2316/alternative_titles",
            json!({ "id": 2316, "results": [{ "iso_3166_1": "US", "title": "Office (US)" }]}),
        ),
        (
            "/find/tt0084787",
            json!({ "movie_results": [{ "id": 1091 }], "tv_results": [] }),
        ),
        (
            "/find/tt0386676",
            json!({ "movie_results": [], "tv_results": [{ "id": 2316 }] }),
        ),
        (
            "/movie/1091",
            json!({ "id": 1091, "title": "The Thing", "release_date": "1982-06-25", "genres": [] }),
        ),
        (
            "/tv/2316",
            json!({ "id": 2316, "name": "The Office", "first_air_date": "2005-03-24", "genres": [] }),
        ),
    ];
    for (route, body) in routes {
        Mock::given(method("GET"))
            .and(path(route))
            .respond_with(ResponseTemplate::new(200).set_body_json(body))
            .mount(&tmdb)
            .await;
    }
    tmdb
}

async fn tmdb_calls(tmdb: &MockServer, route: &str) -> usize {
    tmdb.received_requests()
        .await
        .unwrap()
        .iter()
        .filter(|r| r.url.path() == route)
        .count()
}

/// Discover and Stremio titles that are not in the library: the alternates
/// come from TMDB, which is asked once per title however often it is searched.
#[tokio::test]
async fn unlisted_titles_fetch_alternates_once_through_the_cache() {
    let rig = rig().await;
    let tmdb = tmdb(&rig).await;

    let discover_movie = json!({ "title": "The Thing", "tmdb_id": 1091, "media_type": "movie" });
    let discover_show = json!({
        "title": "The Office", "tmdb_id": 2316, "media_type": "tv", "season": 1, "episode": 1,
    });
    for round in 0..2 {
        let entries = [
            (
                "discover movie",
                rig.stream_search(discover_movie.clone()).await,
                MOVIE_ALTERNATES,
                "The Thing",
            ),
            (
                "discover episode",
                rig.stream_search(discover_show.clone()).await,
                SHOW_ALTERNATES,
                "The Office",
            ),
            (
                "stremio movie",
                rig.stremio("movie", "tt0084787").await,
                MOVIE_ALTERNATES,
                "The Thing",
            ),
            (
                "stremio episode",
                rig.stremio("series", "tt0386676:1:1").await,
                SHOW_ALTERNATES,
                "The Office",
            ),
        ];
        for (entry, want, expected, primary) in entries {
            assert_alternates(&format!("{entry} #{round}"), &want, expected, primary);
        }
    }

    assert_eq!(tmdb_calls(&tmdb, "/movie/1091/alternative_titles").await, 1);
    assert_eq!(tmdb_calls(&tmdb, "/tv/2316/alternative_titles").await, 1);
}
