//! A title streamed from Discover becomes a library item the moment its stream starts,
//! and the Discover page has to say so: clients redirect to it on back.

use crate::common::debrid_rig::*;
use crate::common::debrid_upstream::UpstreamPlan;
use crate::common::*;
use reqwest::StatusCode;
use serde_json::{Value, json};
use std::time::Duration;
use uuid::Uuid;
use wiremock::matchers::{method, path_regex};
use wiremock::{Mock, MockServer, ResponseTemplate};

async fn with_tmdb(rig: &Rig) -> MockServer {
    let tmdb = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path_regex(r"^/movie/\d+$"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "id": 1, "title": "Discovered", "release_date": "2024-01-01", "genres": []
        })))
        .mount(&tmdb)
        .await;
    sqlx::query(
        "UPDATE video_metadata_providers
         SET config = jsonb_build_object('api_key', 'k', 'base_url', $1), enabled = true
         WHERE provider_type = 'tmdb'",
    )
    .bind(tmdb.uri())
    .execute(&rig.server.pool)
    .await
    .unwrap();
    tmdb
}

async fn discover_library_id(rig: &Rig, tmdb_id: &str) -> Value {
    let resp = rig
        .server
        .admin_client()
        .get(&format!("/api/v1/admin/discover/movie/tmdb/{tmdb_id}"))
        .await;
    resp.assert_ok();
    resp.json::<Value>()["library_id"].clone()
}

fn discover_resolve(info_hash: &str, tmdb_id: Value) -> Value {
    json!({
        "debrid_provider_id": PROVIDER_ID,
        "info_hash": info_hash,
        "media_type": "movie",
        "media_id": Uuid::nil(),
        "release_title": "Discovered.2024",
        "debrid_provider": "realdebrid",
        "capabilities": direct_play_caps(),
        "auto_create": { "title": "Discovered", "year": 2024, "identifiers": { "tmdb_id": tmdb_id } }
    })
}

async fn session_media_id(rig: &Rig, session_id: Uuid) -> Uuid {
    sqlx::query_scalar("SELECT media_id FROM debrid_stream_sessions WHERE id = $1")
        .bind(session_id)
        .fetch_one(&rig.server.pool)
        .await
        .unwrap()
}

#[tokio::test]
async fn discover_names_the_library_item_a_stream_created() {
    let rig = Rig::start(UpstreamPlan::new(ITA_ENG_FIXTURE.to_vec()), "film.mkv").await;
    let _tmdb = with_tmdb(&rig).await;

    let cached_movie = rig.movie().await;
    let warm = rig.resolve_body("onDiskHash", cached_movie, direct_play_caps());
    let warm_session = session_id_of(&rig.resolve_with(&warm).await);
    rig.wait_for_complete("onDiskHash").await;
    rig.server
        .admin_client()
        .delete(&format!("/api/v1/admin/stream/{warm_session}"))
        .await
        .assert_status(StatusCode::NO_CONTENT);
    tokio::time::sleep(Duration::from_millis(100)).await;

    let cases = [
        ("string tmdb id", "9001", "hashA", json!("9001")),
        ("numeric tmdb id", "9002", "hashB", json!(9002)),
        (
            "release already on disk",
            "9003",
            "onDiskHash",
            json!("9003"),
        ),
    ];
    for (name, tmdb_id, hash, id_value) in cases {
        assert!(discover_library_id(&rig, tmdb_id).await.is_null(), "{name}");

        let body = discover_resolve(hash, id_value);
        let session = session_id_of(&rig.resolve_with(&body).await);
        let library_id = discover_library_id(&rig, tmdb_id).await;

        assert!(
            library_id.is_string(),
            "{name}: no library_id after streaming"
        );
        assert_eq!(
            library_id,
            json!(session_media_id(&rig, session).await),
            "{name}: the session tracks a different item than Discover shows"
        );

        rig.resolve_with(&body).await;
        let copies: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM movies WHERE identifiers->>'tmdb_id' = $1")
                .bind(tmdb_id)
                .fetch_one(&rig.server.pool)
                .await
                .unwrap();
        assert_eq!(copies, 1, "{name}: streaming again created a duplicate");
    }
}
