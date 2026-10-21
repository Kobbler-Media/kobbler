use super::*;
use api::jobs::handlers::StremioSyncHandler;
use api::jobs::worker::JobHandler;
use serde_json::{Value, json};
use time::{Duration, OffsetDateTime};
use tokio_util::sync::CancellationToken;
use uuid::Uuid;
use wiremock::matchers::{body_partial_json, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

/// (episode number, title, air date)
pub type EpisodeSpec<'a> = (u32, &'a str, String);

pub const AUTH_KEY: &str = "stremio-session";

pub fn days_from_today(offset: i64) -> String {
    let d = OffsetDateTime::now_utc().date() + Duration::days(offset);
    format!("{:04}-{:02}-{:02}", d.year(), u8::from(d.month()), d.day())
}

pub fn days_ago(days: i64) -> String {
    (OffsetDateTime::now_utc() - Duration::days(days))
        .format(&time::format_description::well_known::Rfc3339)
        .unwrap()
}

pub fn library_item(
    id: &str,
    kind: &str,
    offset: u64,
    video_id: Option<&str>,
    watched_days_ago: i64,
) -> Value {
    json!({
        "_id": id,
        "type": kind,
        "name": id,
        "removed": false,
        "temp": false,
        "_mtime": days_ago(watched_days_ago),
        "state": {
            "lastWatched": days_ago(watched_days_ago),
            "timeOffset": offset,
            "duration": 6_000_000,
            "video_id": video_id.unwrap_or(""),
            "timesWatched": 1,
        }
    })
}

pub async fn mount_movie(mock: &MockServer, imdb: &str, tmdb: i64, title: &str, cert: &str) {
    Mock::given(method("GET"))
        .and(path(format!("/find/{imdb}")))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(json!({ "movie_results": [{ "id": tmdb }] })),
        )
        .mount(mock)
        .await;
    Mock::given(method("GET"))
        .and(path(format!("/movie/{tmdb}")))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "id": tmdb, "title": title, "genres": [],
            "release_dates": { "results": [{ "iso_3166_1": "US", "release_dates": [{ "certification": cert }] }] }
        })))
        .mount(mock)
        .await;
}

/// `seasons`: (season number, [(episode number, title, air date)]).
pub async fn mount_show(
    mock: &MockServer,
    imdb: &str,
    tmdb: i64,
    title: &str,
    status: &str,
    rating: &str,
    seasons: &[(u32, &[EpisodeSpec<'_>])],
) {
    Mock::given(method("GET"))
        .and(path(format!("/find/{imdb}")))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(json!({ "tv_results": [{ "id": tmdb }] })),
        )
        .mount(mock)
        .await;
    let summaries: Vec<Value> = seasons
        .iter()
        .map(|(n, eps)| json!({ "season_number": n, "episode_count": eps.len() }))
        .collect();
    Mock::given(method("GET"))
        .and(path(format!("/tv/{tmdb}")))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "id": tmdb, "name": title, "status": status, "genres": [], "seasons": summaries,
            "content_ratings": { "results": [{ "iso_3166_1": "US", "rating": rating }] }
        })))
        .mount(mock)
        .await;
    for (n, eps) in seasons {
        let episodes: Vec<Value> = eps
            .iter()
            .map(|(e, name, air)| json!({ "episode_number": e, "name": name, "air_date": air }))
            .collect();
        Mock::given(method("GET"))
            .and(path(format!("/tv/{tmdb}/season/{n}")))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_json(json!({ "season_number": n, "episodes": episodes })),
            )
            .mount(mock)
            .await;
    }
}

pub async fn mount_library(mock: &MockServer, items: Value) {
    Mock::given(method("POST"))
        .and(path("/api/datastoreGet"))
        .and(body_partial_json(
            json!({ "authKey": AUTH_KEY, "collection": "libraryItem" }),
        ))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "result": items })))
        .mount(mock)
        .await;
}

pub async fn mount_login(mock: &MockServer) {
    Mock::given(method("POST"))
        .and(path("/api/login"))
        .and(body_partial_json(json!({ "password": "right" })))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(json!({ "result": { "authKey": AUTH_KEY, "user": {} } })),
        )
        .mount(mock)
        .await;
    Mock::given(method("POST"))
        .and(path("/api/login"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "error": { "message": "Wrong passphrase", "code": 3 }
        })))
        .mount(mock)
        .await;
}

/// Server whose Stremio API and TMDB both point at one mock, with the instance account linked.
pub async fn linked_server() -> (TestServer, MockServer) {
    let mock = MockServer::start().await;
    let uri = mock.uri();
    let server = TestServer::start_api_only_with(|c| c.stremio_api_url = uri).await;
    sqlx::query(
        "UPDATE video_metadata_providers
         SET config = jsonb_build_object('api_key', 'test-tmdb-key', 'base_url', $1), enabled = true
         WHERE provider_type = 'tmdb'",
    )
    .bind(mock.uri())
    .execute(&server.pool)
    .await
    .unwrap();
    mount_login(&mock).await;
    server
        .admin_client()
        .put_json(
            "/api/v1/admin/stremio/account",
            &json!({ "email": "me@example.com", "password": "right" }),
        )
        .await
        .assert_ok();
    (server, mock)
}

pub async fn run_sync(server: &TestServer, mock: &MockServer) -> Value {
    StremioSyncHandler::new(server.pool.clone(), mock.uri())
        .execute(Uuid::now_v7(), json!({}), CancellationToken::new())
        .await
        .unwrap()
}

pub async fn kids_client(server: &TestServer) -> TestClient {
    let user_id = create_user_with_role(server, "kid@test.local", "viewer").await;
    server
        .admin_client()
        .patch_json(
            &format!("/api/v1/admin/users/{user_id}"),
            &json!({ "is_kids": true }),
        )
        .await
        .assert_ok();
    let client = TestClient::new(&server.api_url);
    let resp = client
        .post_json(
            "/api/v1/admin/login",
            &json!({ "email": "kid@test.local", "password": "testpass123" }),
        )
        .await;
    resp.assert_ok();
    let token = resp.json::<Value>()["token"].as_str().unwrap().to_string();
    client.with_auth(&token)
}
