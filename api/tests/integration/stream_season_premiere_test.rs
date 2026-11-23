//! Every episode search tells kobload when the season premiered: a season dropped whole keeps
//! its weekly listed dates, and only the premiere shows the episode is already out.

use crate::common::debrid_rig::*;
use crate::common::debrid_upstream::UpstreamPlan;
use crate::common::stremio_mock::*;
use crate::common::*;
use api::jobs::handlers::DebridDownloadAheadHandler;
use api::jobs::worker::JobHandler;
use serde_json::{Value, json};
use tokio_util::sync::CancellationToken;
use uuid::Uuid;
use wiremock::matchers::{method, path};
use wiremock::{Mock, ResponseTemplate};

const IMDB: &str = "tt7000123";
const KEY: &str = "premiere-key";

fn premiere_in(request: &Value) -> Option<String> {
    match request {
        Value::Object(fields) => fields
            .get("season_premiere")
            .and_then(Value::as_str)
            .map(str::to_string)
            .or_else(|| fields.values().find_map(premiere_in)),
        Value::Array(items) => items.iter().find_map(premiere_in),
        _ => None,
    }
}

async fn stremio_key(server: &TestServer) {
    sqlx::query(
        "INSERT INTO stremio_api_keys (user_id, api_key)
         SELECT id, $1 FROM users WHERE role = 'admin' LIMIT 1",
    )
    .bind(KEY)
    .execute(&server.pool)
    .await
    .unwrap();
}

async fn stremio_search(server: &TestServer) {
    TestClient::new(&server.api_url)
        .get(&format!("/stremio/{KEY}/stream/series/{IMDB}:1:2.json"))
        .await
        .assert_ok();
}

/// A library season that premiered ten days ago, its second episode listed in three.
async fn dropped_season(rig: &Rig) -> (Uuid, String) {
    let pool = &rig.server.pool;
    let episodes = season_of(&rig.server, 2).await;
    for (episode, days) in [(episodes[0], -10), (episodes[1], 3)] {
        sqlx::query("UPDATE tv_episodes SET air_date = CURRENT_DATE + $2::int WHERE id = $1")
            .bind(episode)
            .bind(days)
            .execute(pool)
            .await
            .unwrap();
    }
    sqlx::query(
        "UPDATE tv_shows SET identifiers = jsonb_build_object('imdb_id', $1::TEXT)
         WHERE id = (SELECT s.show_id FROM tv_seasons s JOIN tv_episodes e ON e.season_id = s.id
                     WHERE e.id = $2)",
    )
    .bind(IMDB)
    .bind(episodes[1])
    .execute(pool)
    .await
    .unwrap();
    let premiere = sqlx::query_scalar("SELECT (CURRENT_DATE - 10)::text")
        .fetch_one(pool)
        .await
        .unwrap();
    (episodes[1], premiere)
}

#[tokio::test]
async fn every_library_episode_search_carries_the_season_premiere() {
    let rig = Rig::start(UpstreamPlan::new(noise(1024)), "ep.mkv").await;
    rig.mount_search(json!([])).await;
    stremio_key(&rig.server).await;
    let (episode, premiere) = dropped_season(&rig).await;
    let client = rig.server.admin_client();
    let user: Uuid = sqlx::query_scalar("SELECT id FROM users WHERE role = 'admin' LIMIT 1")
        .fetch_one(&rig.server.pool)
        .await
        .unwrap();

    client
        .post_json(
            "/api/v1/admin/stream/search",
            &json!({ "episode_id": episode }),
        )
        .await
        .assert_ok();
    client
        .post_json(
            "/api/v1/admin/stream/prewarm",
            &json!({ "media_type": "episode", "media_id": episode }),
        )
        .await;
    DebridDownloadAheadHandler::new(rig.server.app_state.clone())
        .execute(
            Uuid::now_v7(),
            json!({ "user_id": user, "episode_id": episode }),
            CancellationToken::new(),
        )
        .await
        .unwrap();
    stremio_search(&rig.server).await;
    let searches = rig.kobload_requests("/stream/search").await;
    assert_eq!(searches.len(), 4, "search, prewarm, ahead, stremio");
    for (entry, request) in ["search", "prewarm", "ahead", "stremio"]
        .iter()
        .zip(&searches)
    {
        assert_eq!(
            premiere_in(request).as_ref(),
            Some(&premiere),
            "{entry}: {request}"
        );
    }

    client
        .get(&format!("/api/v1/admin/tv/episodes/{episode}/releases"))
        .await;
    let manual = rig.kobload_requests("/api/v1/search").await;
    assert_eq!(manual.len(), 1);
    assert_eq!(premiere_in(&manual[0]).as_ref(), Some(&premiere), "manual");
}

#[tokio::test]
async fn a_stremio_search_outside_the_library_takes_the_premiere_from_tmdb() {
    let (server, mock) = linked_server().await;
    let first = days_from_today(-10);
    mount_show(
        &mock,
        IMDB,
        777,
        "Dropped Show",
        "Returning Series",
        "TV-14",
        &[(
            1,
            &[(1, "One", first.clone()), (2, "Two", days_from_today(3))],
        )],
    )
    .await;
    Mock::given(method("POST"))
        .and(path("/api/v1/stream/search"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "options": [] })))
        .mount(&mock)
        .await;
    server
        .admin_client()
        .patch_json(
            "/api/v1/admin/config",
            &json!({ "kobload_base_url": mock.uri(), "kobload_api_key": "k" }),
        )
        .await
        .assert_ok();
    stremio_key(&server).await;

    stremio_search(&server).await;

    let requests = mock.received_requests().await.unwrap();
    let search = requests
        .iter()
        .find(|r| r.url.path() == "/api/v1/stream/search")
        .expect("no stream search");
    let body: Value = serde_json::from_slice(&search.body).unwrap();
    assert_eq!(premiere_in(&body), Some(first), "{body}");
}
