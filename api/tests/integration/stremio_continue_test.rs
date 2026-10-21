//! Stremio Continue Watching on the home page: a linked instance account's library, resolved
//! through TMDB into discover-style tiles, kept on Up Next's retention rules and filtered for
//! Kids profiles.

use crate::common::stremio_mock::*;
use crate::common::*;
use api::jobs::handlers::StremioSyncHandler;
use api::jobs::worker::JobHandler;
use reqwest::StatusCode;
use serde_json::{Value, json};
use std::collections::BTreeSet;
use time::{Duration, OffsetDateTime};
use tokio_util::sync::CancellationToken;
use uuid::Uuid;
use wiremock::matchers::{body_partial_json, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

async fn stremio_row(client: &TestClient) -> Vec<Value> {
    let resp = client.get("/api/v1/admin/discover").await;
    resp.assert_ok();
    resp.json::<Value>()["stremio_continue"]
        .as_array()
        .unwrap()
        .clone()
}

fn titles(row: &[Value]) -> BTreeSet<String> {
    row.iter()
        .map(|i| i["title"].as_str().unwrap().to_string())
        .collect()
}

fn find<'a>(row: &'a [Value], title: &str) -> &'a Value {
    row.iter()
        .find(|i| i["title"] == title)
        .unwrap_or_else(|| panic!("{title} missing: {row:?}"))
}

/// Every Stremio state the row has a rule for, synced once.
async fn seed_catalogue(mock: &MockServer) {
    mount_movie(mock, "tt0000001", 101, "Halfway Movie", "R").await;
    mount_show(
        mock,
        "tt0000002",
        202,
        "Resumed Show",
        "Returning Series",
        "TV-Y",
        &[(1, &[(3, "Ep Three", days_from_today(-60))])],
    )
    .await;
    mount_show(
        mock,
        "tt0000003",
        203,
        "Stale Show",
        "Returning Series",
        "TV-PG",
        &[(1, &[(5, "Ep Five", days_from_today(-60))])],
    )
    .await;
    mount_show(
        mock,
        "tt0000004",
        204,
        "Premiere Show",
        "Returning Series",
        "TV-MA",
        &[
            (
                1,
                &[
                    (7, "Seven", days_from_today(-400)),
                    (8, "Finale", days_from_today(-393)),
                ],
            ),
            (2, &[(1, "Premiere", days_from_today(-5))]),
        ],
    )
    .await;
    mount_show(
        mock,
        "tt0000005",
        205,
        "Tomorrow Show",
        "Returning Series",
        "TV-G",
        &[(
            2,
            &[
                (4, "Four", days_from_today(-6)),
                (5, "Five", days_from_today(1)),
            ],
        )],
    )
    .await;
    mount_show(
        mock,
        "tt0000006",
        206,
        "Ended Show",
        "Ended",
        "TV-G",
        &[(1, &[(1, "Only", days_from_today(-900))])],
    )
    .await;
    mount_movie(mock, "tt0000007", 107, "Removed Movie", "G").await;
    mount_show(
        mock,
        "tt0000008",
        208,
        "Seen Show",
        "Returning Series",
        "TV-G",
        &[(
            1,
            &[
                (2, "Two", days_from_today(-17)),
                (3, "Three", days_from_today(-10)),
            ],
        )],
    )
    .await;

    let mut removed = library_item("tt0000007", "movie", 1_000_000, None, 1);
    removed["removed"] = json!(true);
    mount_library(
        mock,
        json!([
            library_item("tt0000001", "movie", 1_800_000, None, 30),
            library_item("tt0000002", "series", 600_000, Some("tt0000002:1:3"), 2),
            library_item("tt0000003", "series", 1, Some("tt0000003:1:5"), 20),
            library_item("tt0000004", "series", 0, Some("tt0000004:1:8"), 200),
            library_item("tt0000005", "series", 0, Some("tt0000005:2:4"), 3),
            library_item("tt0000006", "series", 0, Some("tt0000006:1:1"), 3),
            removed,
            library_item("tt0000008", "series", 0, Some("tt0000008:1:2"), 2),
            library_item("kitsu:123", "series", 500_000, Some("kitsu:123:1:1"), 1),
        ]),
    )
    .await;
}

#[tokio::test]
async fn row_follows_up_next_retention_rules() {
    let (server, mock) = linked_server().await;
    seed_catalogue(&mock).await;
    let summary = run_sync(&server, &mock).await;
    assert_eq!(summary["errors"], json!([]), "{summary}");

    let row = stremio_row(&server.admin_client()).await;
    assert_eq!(
        titles(&row),
        BTreeSet::from([
            "Halfway Movie".into(),
            "Resumed Show".into(),
            "Premiere Show".into()
        ]),
        "in-progress movies never expire; TV expires after 14 days unless a season premiere aired \
         in the last 30; a caught-up show returns only for an episode aired since the last watch; \
         removed, ended and non-IMDb items never show"
    );

    let movie = find(&row, "Halfway Movie");
    assert_eq!(movie["media_type"], "movie");
    assert_eq!(movie["provider"], "tmdb");
    assert_eq!(movie["provider_id"], "101");
    assert_eq!(movie["progress_percent"], 30.0);

    let resumed = find(&row, "Resumed Show");
    assert_eq!(resumed["subtitle"], "S01E03 - Ep Three");
    assert_eq!(resumed["season"], 1);
    assert_eq!(resumed["progress_percent"], 10.0);

    let premiere = find(&row, "Premiere Show");
    assert_eq!(premiere["subtitle"], "New · S02E01 - Premiere");
    assert!(premiere["progress_percent"].is_null());
}

#[tokio::test]
async fn kids_profile_sees_only_titles_the_kids_rule_allows() {
    let (server, mock) = linked_server().await;
    seed_catalogue(&mock).await;
    run_sync(&server, &mock).await;
    let kids = kids_client(&server).await;

    assert_eq!(
        titles(&stremio_row(&kids).await),
        BTreeSet::from(["Resumed Show".into()])
    );

    kids.get("/api/v1/admin/discover/tv/tmdb/202")
        .await
        .assert_ok();
    kids.get("/api/v1/admin/discover/tv/tmdb/204")
        .await
        .assert_status(StatusCode::NOT_FOUND);
    kids.get("/api/v1/admin/discover/movie/tmdb/101")
        .await
        .assert_status(StatusCode::NOT_FOUND);
}

/// A title the library holds is judged by the library — its manual Kids override included —
/// and one already on What to Watch is not repeated.
#[tokio::test]
async fn library_titles_use_library_rules_and_are_not_repeated() {
    let (server, mock) = linked_server().await;
    seed_catalogue(&mock).await;
    run_sync(&server, &mock).await;

    let admin_id: Uuid =
        sqlx::query_scalar("SELECT id FROM users WHERE email = 'admin@test.local'")
            .fetch_one(&server.pool)
            .await
            .unwrap();
    let movie_id = Uuid::new_v4();
    sqlx::query("INSERT INTO movies (id, title, identifiers) VALUES ($1, 'Halfway Movie', '{\"tmdb_id\": \"101\"}')")
        .bind(movie_id)
        .execute(&server.pool)
        .await
        .unwrap();
    sqlx::query("INSERT INTO movie_watch_states (movie_id, user_id, position_seconds, watched) VALUES ($1, $2, 600, FALSE)")
        .bind(movie_id)
        .bind(admin_id)
        .execute(&server.pool)
        .await
        .unwrap();
    sqlx::query(
        "INSERT INTO tv_shows (id, title, content_rating, kids_override, identifiers)
         VALUES ($1, 'Premiere Show', 'TV-MA', TRUE, '{\"tmdb_id\": \"204\"}')",
    )
    .bind(Uuid::new_v4())
    .execute(&server.pool)
    .await
    .unwrap();

    let admin_row = stremio_row(&server.admin_client()).await;
    assert!(
        !titles(&admin_row).contains("Halfway Movie"),
        "{admin_row:?}"
    );

    let kids = kids_client(&server).await;
    assert_eq!(
        titles(&stremio_row(&kids).await),
        BTreeSet::from(["Resumed Show".into(), "Premiere Show".into()])
    );
    kids.get("/api/v1/admin/discover/tv/tmdb/204")
        .await
        .assert_ok();
}

#[tokio::test]
async fn linking_is_admin_only_and_keeps_no_password() {
    let (server, mock) = linked_server().await;

    let account = server
        .admin_client()
        .get("/api/v1/admin/stremio/account")
        .await;
    account.assert_ok();
    assert_eq!(
        account.json::<Value>()["account"]["email"],
        "me@example.com"
    );
    let stored: (String,) = sqlx::query_as("SELECT auth_key FROM stremio_accounts")
        .fetch_one(&server.pool)
        .await
        .unwrap();
    assert_eq!(stored.0, AUTH_KEY);

    server
        .admin_client()
        .put_json(
            "/api/v1/admin/stremio/account",
            &json!({ "email": "me@example.com", "password": "wrong" }),
        )
        .await
        .assert_status(StatusCode::UNAUTHORIZED);

    let viewer = create_user_client(&server, "viewer@test.local", "viewer").await;
    viewer
        .put_json(
            "/api/v1/admin/stremio/account",
            &json!({ "email": "x@example.com", "password": "right" }),
        )
        .await
        .assert_status(StatusCode::FORBIDDEN);
    viewer
        .get("/api/v1/admin/stremio/account")
        .await
        .assert_status(StatusCode::FORBIDDEN);
    viewer
        .delete("/api/v1/admin/stremio/account")
        .await
        .assert_status(StatusCode::FORBIDDEN);

    mount_library(
        &mock,
        json!([library_item("tt0000001", "movie", 1_800_000, None, 1)]),
    )
    .await;
    mount_movie(&mock, "tt0000001", 101, "Halfway Movie", "R").await;
    run_sync(&server, &mock).await;
    assert_eq!(
        stremio_row(&viewer).await.len(),
        1,
        "every profile reads the instance account"
    );

    server
        .admin_client()
        .delete("/api/v1/admin/stremio/account")
        .await
        .assert_status(StatusCode::NO_CONTENT);
    let account = server
        .admin_client()
        .get("/api/v1/admin/stremio/account")
        .await;
    assert!(account.json::<Value>()["account"].is_null());
    assert!(stremio_row(&viewer).await.is_empty());
}

/// A dead Stremio session or a TMDB outage is reported on the account and leaves the last good
/// row in place rather than emptying it.
#[tokio::test]
async fn failed_sync_reports_and_keeps_previous_rows() {
    let (server, mock) = linked_server().await;
    mount_movie(&mock, "tt0000001", 101, "Halfway Movie", "R").await;
    mount_library(
        &mock,
        json!([library_item("tt0000001", "movie", 1_800_000, None, 1)]),
    )
    .await;
    run_sync(&server, &mock).await;
    assert_eq!(stremio_row(&server.admin_client()).await.len(), 1);

    mock.reset().await;
    Mock::given(method("POST"))
        .and(path("/api/datastoreGet"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(json!({ "error": { "message": "Session not found" } })),
        )
        .mount(&mock)
        .await;
    run_sync(&server, &mock).await;
    let account = server
        .admin_client()
        .get("/api/v1/admin/stremio/account")
        .await
        .json::<Value>();
    assert!(
        account["account"]["last_error"]
            .as_str()
            .unwrap()
            .contains("Session not found"),
        "{account}"
    );
    assert_eq!(stremio_row(&server.admin_client()).await.len(), 1);

    mock.reset().await;
    mount_library(
        &mock,
        json!([library_item("tt0000009", "movie", 1_800_000, None, 1)]),
    )
    .await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(500))
        .mount(&mock)
        .await;
    run_sync(&server, &mock).await;
    let account = server
        .admin_client()
        .get("/api/v1/admin/stremio/account")
        .await
        .json::<Value>();
    assert!(
        account["account"]["last_error"]
            .as_str()
            .unwrap()
            .contains("TMDB"),
        "{account}"
    );
    assert_eq!(
        titles(&stremio_row(&server.admin_client()).await),
        BTreeSet::from(["Halfway Movie".into()])
    );
}

/// Stremio's library can hold the same title twice; the newer entry wins in both the row and
/// the history rather than the duplicate failing the sync.
#[tokio::test]
async fn duplicate_library_entries_keep_the_newest() {
    let (server, mock) = linked_server().await;
    mount_show(
        &mock,
        "tt0000002",
        202,
        "Resumed Show",
        "Returning Series",
        "TV-Y",
        &[(1, &[(3, "Ep Three", days_from_today(-60))])],
    )
    .await;
    let mut older = library_item("tt0000002", "series", 600_000, Some("tt0000002:1:3"), 2);
    older["_mtime"] = json!(days_ago(3));
    let newer = library_item("tt0000002", "series", 1_200_000, Some("tt0000002:1:3"), 1);
    mount_library(&mock, json!([newer, older])).await;

    let summary = run_sync(&server, &mock).await;
    assert_eq!(summary["errors"], json!([]), "{summary}");
    let row = stremio_row(&server.admin_client()).await;
    assert_eq!(row.len(), 1, "{row:?}");
    assert_eq!(find(&row, "Resumed Show")["progress_percent"], 20.0);
}
