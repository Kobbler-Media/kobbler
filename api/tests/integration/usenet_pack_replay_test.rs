//! A pack's episodes played from Stremio, whose option id is the same for every episode
//! of the pack, and episodes already on disk or warm while kobload is down.

use crate::common::debrid_rig::*;
use crate::common::usenet_pack::{Episode, Pack, pack};
use crate::common::*;
use reqwest::StatusCode;
use serde_json::{Value, json};
use std::time::Duration;
use uuid::Uuid;
use wiremock::matchers::{method, path_regex};
use wiremock::{Mock, ResponseTemplate};

const IMDB: &str = "tt7000005";
const STREMIO_KEY: &str = "pack-key";

async fn stremio(pack: &Pack) {
    let server = &pack.rig.server;
    sqlx::query(
        "UPDATE tv_shows SET identifiers = jsonb_build_object('imdb_id', $1::TEXT)
         WHERE id = (SELECT s.show_id FROM tv_seasons s JOIN tv_episodes e ON e.season_id = s.id
                     WHERE e.id = $2)",
    )
    .bind(IMDB)
    .bind(pack.e05.id)
    .execute(&server.pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO stremio_api_keys (user_id, api_key)
         SELECT id, $1 FROM users WHERE role = 'admin' LIMIT 1",
    )
    .bind(STREMIO_KEY)
    .execute(&server.pool)
    .await
    .unwrap();
    let mut option = pack.rig.option_from(Source::Usenet);
    option["id"] = json!(format!("usenet_guid-usenet-1_{PROVIDER_ID}"));
    option["files"] = json!([
        { "id": pack.e06.file_id, "path": "Show.S01E06.mkv", "size": pack.e06.data.len() },
        { "id": pack.e05.file_id, "path": "Show.S01E05.mkv", "size": pack.e05.data.len() },
    ]);
    pack.rig.mount_search(json!([option])).await;
}

/// The play URL Stremio lists for the episode.
async fn listed(pack: &Pack, episode: &Episode) -> String {
    let resp = TestClient::new(&pack.rig.server.api_url)
        .get(&format!(
            "/stremio/{STREMIO_KEY}/stream/series/{IMDB}:1:{}.json",
            episode.number
        ))
        .await;
    resp.assert_ok();
    let body: Value = resp.json();
    let url = body["streams"][0]["url"]
        .as_str()
        .unwrap_or_else(|| panic!("nothing listed: {body}"));
    url[url.find("/stremio/").unwrap()..].to_string()
}

/// The session a Stremio play redirects to.
async fn follow(pack: &Pack, play_path: &str) -> Uuid {
    let resp = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .unwrap()
        .get(format!("{}{play_path}", pack.rig.server.api_url))
        .send()
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::TEMPORARY_REDIRECT,
        "{play_path} did not play"
    );
    let location = resp.headers()[reqwest::header::LOCATION].to_str().unwrap();
    location.rsplit('/').next().unwrap().parse().unwrap()
}

async fn stremio_play(pack: &Pack, episode: &Episode) -> Uuid {
    let path = listed(pack, episode).await;
    follow(pack, &path).await
}

async fn assert_plays(pack: &Pack, session: Uuid, episode: &Episode) {
    let played = play(&pack.rig.server, session, "bytes=0-").await;
    assert!(
        played.body == episode.data,
        "E{:02} played another file",
        episode.number
    );
}

/// kobload has gone: every resolve fails as an unreachable engine would.
async fn kobload_down(pack: &Pack) {
    pack.rig.kobload.reset().await;
    Mock::given(method("POST"))
        .and(path_regex("/api/v1/stream/resolve$"))
        .respond_with(ResponseTemplate::new(500).set_body_string(
            "Debrid error: No usenet provider can serve this right now (providers_unavailable)",
        ))
        .mount(&pack.rig.kobload)
        .await;
}

#[tokio::test]
async fn a_stremio_pack_play_of_the_next_episode_plays_that_episode() {
    let pack = pack().await;
    stremio(&pack).await;

    let e05 = stremio_play(&pack, &pack.e05).await;
    assert_plays(&pack, e05, &pack.e05).await;
    let e06 = stremio_play(&pack, &pack.e06).await;

    assert_ne!(e06, e05, "E06 was handed E05's live session");
    assert_plays(&pack, e06, &pack.e06).await;
    assert_eq!(stremio_play(&pack, &pack.e05).await, e05);
    let resolves = pack.rig.kobload_requests("/stream/resolve").await;
    let e06_asks: Vec<&Value> = resolves
        .iter()
        .filter(|r| r["episode"] == json!({ "season": 1, "episode": 6 }))
        .collect();
    assert!(
        !e06_asks.is_empty(),
        "E06 was never asked for: {resolves:?}"
    );
    for ask in e06_asks {
        assert!(
            ask["file_id"].is_null(),
            "kobload picks file_id over the episode: {ask}"
        );
    }
}

#[tokio::test]
async fn a_downloaded_pack_episode_plays_from_disk_with_kobload_down() {
    let pack = pack().await;
    let first = pack.tap(&pack.e05).await;
    pack.assert_played(&pack.e05, first).await;
    end_playback(&pack.rig.server, first).await;
    stremio(&pack).await;
    let path = listed(&pack, &pack.e05).await;

    kobload_down(&pack).await;
    let session = follow(&pack, &path).await;

    assert_plays(&pack, session, &pack.e05).await;
}

#[tokio::test]
async fn a_warm_pack_episode_is_claimed_with_kobload_down() {
    let pack = pack().await;
    pack.rig
        .mount_search(json!([pack.rig.option_from(Source::Usenet)]))
        .await;
    let body = json!({
        "media_type": "episode",
        "media_id": pack.e05.id,
        "capabilities": direct_play_caps(),
    });
    let deadline = std::time::Instant::now() + Duration::from_secs(15);
    loop {
        let resp = pack
            .rig
            .server
            .admin_client()
            .post_json("/api/v1/admin/stream/prewarm", &body)
            .await;
        resp.assert_ok();
        let reply: Value = resp.json();
        if reply["state"] == "ready" {
            break;
        }
        assert!(std::time::Instant::now() < deadline, "never ready: {reply}");
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    let warm = pack.rig.server.app_state.stream_prewarms.entries_snapshot()[0].1;

    kobload_down(&pack).await;
    let session = pack.tap(&pack.e05).await;

    assert_eq!(session, warm, "the warm session was not claimed");
    assert_plays(&pack, session, &pack.e05).await;
}
