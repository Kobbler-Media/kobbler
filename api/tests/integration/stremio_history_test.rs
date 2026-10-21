//! Importing a shared Stremio account's watch history into one profile's own watch state, so
//! Kobbler's Up Next takes over from Stremio's Continue Watching.

use crate::common::stremio_mock::*;
use crate::common::*;
use reqwest::StatusCode;
use serde_json::{Value, json};
use std::collections::BTreeSet;
use wiremock::MockServer;

const CAUGHT_UP: &str = "tt0000011";
const MIDWAY: &str = "tt0000012";
const SEEN_MOVIE: &str = "tt0000013";
const UNWATCHED_MOVIE: &str = "tt0000014";
const HALFWAY_MOVIE: &str = "tt0000015";
const MATURE_SHOW: &str = "tt0000016";

fn series(id: &str, offset: u64, video_id: &str, watched_days_ago: i64) -> Value {
    library_item(id, "series", offset, Some(video_id), watched_days_ago)
}

fn movie(id: &str, offset: u64, times_watched: u32, watched_days_ago: i64) -> Value {
    let mut item = library_item(id, "movie", offset, None, watched_days_ago);
    item["state"]["timesWatched"] = json!(times_watched);
    item
}

fn episodes(n: u32, aired_days_ago: i64) -> Vec<EpisodeSpec<'static>> {
    (1..=n)
        .map(|e| (e, "Episode", days_from_today(-aired_days_ago)))
        .collect()
}

async fn seed_catalogue(mock: &MockServer) {
    mount_show(
        mock,
        CAUGHT_UP,
        311,
        "Caught Up Show",
        "Returning Series",
        "TV-G",
        &[(1, &episodes(3, 400)), (2, &episodes(1, 5))],
    )
    .await;
    mount_show(
        mock,
        MIDWAY,
        312,
        "Midway Show",
        "Returning Series",
        "TV-Y",
        &[(1, &episodes(2, 400)), (2, &episodes(3, 300))],
    )
    .await;
    mount_movie(mock, SEEN_MOVIE, 113, "Seen Movie", "G").await;
    mount_movie(mock, UNWATCHED_MOVIE, 114, "Unwatched Movie", "G").await;
    mount_movie(mock, HALFWAY_MOVIE, 115, "Halfway Movie", "PG").await;
    mount_show(
        mock,
        MATURE_SHOW,
        316,
        "Mature Show",
        "Returning Series",
        "TV-MA",
        &[(1, &episodes(2, 400))],
    )
    .await;
}

async fn stub(mock: &MockServer, midway_video: &str) {
    mock.reset().await;
    seed_catalogue(mock).await;
    let mut caught_up = series(CAUGHT_UP, 0, &format!("{CAUGHT_UP}:1:3"), 100);
    caught_up["removed"] = json!(true);
    let mut unwatched = movie(UNWATCHED_MOVIE, 0, 0, 0);
    unwatched["state"]["lastWatched"] = json!("");
    mount_library(
        mock,
        json!([
            caught_up,
            series(MIDWAY, 1, midway_video, 3),
            movie(SEEN_MOVIE, 0, 1, 50),
            unwatched,
            movie(HALFWAY_MOVIE, 1_800_000, 0, 4),
            series(MATURE_SHOW, 1, &format!("{MATURE_SHOW}:1:2"), 6),
        ]),
    )
    .await;
}

async fn history(client: &TestClient) -> Vec<Value> {
    let resp = client.get("/api/v1/admin/stremio/history").await;
    resp.assert_ok();
    resp.json::<Value>()["items"].as_array().unwrap().clone()
}

fn find<'a>(items: &'a [Value], title: &str) -> &'a Value {
    items
        .iter()
        .find(|i| i["title"] == title)
        .unwrap_or_else(|| panic!("{title} missing: {items:?}"))
}

fn titles(items: &[Value]) -> BTreeSet<String> {
    items
        .iter()
        .map(|i| i["title"].as_str().unwrap().to_string())
        .collect()
}

async fn select(client: &TestClient, ids: &[&str]) -> Value {
    let resp = client
        .put_json(
            "/api/v1/admin/stremio/history/imports",
            &json!({ "stremio_ids": ids }),
        )
        .await;
    resp.assert_ok();
    resp.json()
}

async fn watched_episodes(server: &TestServer, email: &str, show: &str) -> BTreeSet<String> {
    sqlx::query_scalar::<_, String>(
        "SELECT format('S%sE%s', ts.season_identifier, te.episode_identifier)
         FROM tv_watch_states ws
         JOIN users u ON u.id = ws.user_id
         JOIN tv_episodes te ON te.id = ws.episode_id
         JOIN tv_seasons ts ON ts.id = te.season_id
         JOIN tv_shows s ON s.id = ts.show_id
         WHERE u.email = $1 AND s.title = $2 AND ws.watched",
    )
    .bind(email)
    .bind(show)
    .fetch_all(&server.pool)
    .await
    .unwrap()
    .into_iter()
    .collect()
}

async fn what_to_watch(client: &TestClient) -> BTreeSet<String> {
    let resp = client.get("/api/v1/admin/discover").await;
    resp.assert_ok();
    resp.json::<Value>()["watch_next"]
        .as_array()
        .unwrap()
        .iter()
        .map(|i| {
            let subtitle = i["subtitle"].as_str().unwrap_or("");
            let episode = subtitle.split(" - ").next().unwrap_or("");
            format!("{} {}", i["title"].as_str().unwrap(), episode)
                .trim()
                .to_string()
        })
        .collect()
}

#[tokio::test]
async fn imported_history_feeds_up_next_and_re_runs_add_new_watches() {
    let (server, mock) = linked_server().await;
    stub(&mock, &format!("{MIDWAY}:2:1")).await;
    let summary = run_sync(&server, &mock).await;
    assert_eq!(summary["errors"], json!([]), "{summary}");

    let viewer_email = "viewer@test.local";
    let viewer = create_user_client(&server, viewer_email, "viewer").await;
    let items = history(&viewer).await;
    assert_eq!(
        titles(&items),
        BTreeSet::from([
            "Caught Up Show".into(),
            "Midway Show".into(),
            "Seen Movie".into(),
            "Halfway Movie".into(),
            "Mature Show".into(),
        ]),
        "removed items are still history; a title never played is not"
    );
    assert_eq!(find(&items, "Midway Show")["progress"], "Up to S02E01");
    assert_eq!(
        find(&items, "Caught Up Show")["progress"],
        "Watched through S01E03"
    );
    assert_eq!(find(&items, "Seen Movie")["progress"], "Watched");
    assert_eq!(find(&items, "Halfway Movie")["progress"], "In progress");
    assert_eq!(find(&items, "Midway Show")["imported"], false);
    assert_eq!(find(&items, "Midway Show")["provider_id"], "312");

    let selected = select(&viewer, &[CAUGHT_UP, MIDWAY, SEEN_MOVIE, HALFWAY_MOVIE]).await;
    assert_eq!(selected["selected"], 4);
    let summary = run_sync(&server, &mock).await;
    assert_eq!(summary["errors"], json!([]), "{summary}");

    assert_eq!(
        watched_episodes(&server, viewer_email, "Caught Up Show").await,
        BTreeSet::from(["S1E1".into(), "S1E2".into(), "S1E3".into()])
    );
    assert_eq!(
        watched_episodes(&server, viewer_email, "Midway Show").await,
        BTreeSet::from(["S1E1".into(), "S1E2".into()])
    );
    let seen: bool = sqlx::query_scalar(
        "SELECT ws.watched FROM movie_watch_states ws JOIN movies m ON m.id = ws.movie_id
         WHERE m.title = 'Seen Movie'",
    )
    .fetch_one(&server.pool)
    .await
    .unwrap();
    assert!(seen);
    assert_eq!(
        what_to_watch(&viewer).await,
        BTreeSet::from([
            "Caught Up Show S02E01".into(),
            "Midway Show S02E01".into(),
            "Halfway Movie".into(),
        ]),
        "a new season brings a long-finished show back; a recent show waits on its next episode"
    );
    assert!(
        what_to_watch(&server.admin_client()).await.is_empty(),
        "another profile's watch state is untouched"
    );
    assert_eq!(
        find(&history(&viewer).await, "Midway Show")["imported"],
        true
    );

    stub(&mock, &format!("{MIDWAY}:2:2")).await;
    run_sync(&server, &mock).await;
    assert_eq!(
        watched_episodes(&server, viewer_email, "Midway Show").await,
        BTreeSet::from(["S1E1".into(), "S1E2".into(), "S2E1".into()]),
        "a re-run adds what was watched in Stremio since"
    );
    let shows: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM tv_shows WHERE identifiers->>'tmdb_id' = '312'")
            .fetch_one(&server.pool)
            .await
            .unwrap();
    assert_eq!(shows, 1);

    viewer
        .delete(&format!("/api/v1/admin/stremio/history/imports/{MIDWAY}"))
        .await
        .assert_status(StatusCode::NO_CONTENT);
    stub(&mock, &format!("{MIDWAY}:2:3")).await;
    run_sync(&server, &mock).await;
    assert_eq!(
        watched_episodes(&server, viewer_email, "Midway Show").await,
        BTreeSet::from(["S1E1".into(), "S1E2".into(), "S2E1".into()]),
        "an unselected title stops following Stremio but keeps what was imported"
    );
}

#[tokio::test]
async fn kids_profile_sees_and_imports_only_allowed_titles() {
    let (server, mock) = linked_server().await;
    stub(&mock, &format!("{MIDWAY}:2:1")).await;
    run_sync(&server, &mock).await;
    let kids = kids_client(&server).await;

    let items = history(&kids).await;
    assert!(!titles(&items).contains("Mature Show"), "{items:?}");
    assert!(titles(&items).contains("Midway Show"));

    let selected = select(&kids, &[MATURE_SHOW, MIDWAY]).await;
    assert_eq!(selected["selected"], 1);
    run_sync(&server, &mock).await;
    assert!(
        watched_episodes(&server, "kid@test.local", "Mature Show")
            .await
            .is_empty()
    );
    assert_eq!(
        watched_episodes(&server, "kid@test.local", "Midway Show").await,
        BTreeSet::from(["S1E1".into(), "S1E2".into()])
    );
}

#[tokio::test]
async fn history_without_a_linked_account_is_empty() {
    let server = TestServer::start_api_only().await;
    let viewer = create_user_client(&server, "viewer@test.local", "viewer").await;
    let resp = viewer.get("/api/v1/admin/stremio/history").await;
    resp.assert_ok();
    let body = resp.json::<Value>();
    assert_eq!(body["linked"], false);
    assert_eq!(body["items"], json!([]));
    viewer
        .put_json(
            "/api/v1/admin/stremio/history/imports",
            &json!({ "stremio_ids": [MIDWAY] }),
        )
        .await
        .assert_ok();
}
