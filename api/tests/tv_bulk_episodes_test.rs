use crate::common::*;
use reqwest::StatusCode;
use serde_json::{Value, json};
use uuid::Uuid;

// =============================================================================
// Helpers
// =============================================================================

async fn create_show(server: &TestServer, title: &str) -> Uuid {
    let resp = server
        .admin_client()
        .post_json(
            "/api/v1/admin/tv/shows",
            &json!({ "title": title, "year": 2024 }),
        )
        .await;
    resp.assert_status(StatusCode::CREATED);
    let json: Value = resp.json();
    json["id"].as_str().unwrap().parse().unwrap()
}

async fn create_season(pool: &sqlx::PgPool, show_id: Uuid, season_num: i32) -> Uuid {
    let id = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO tv_seasons (id, show_id, season_identifier, season_sort_order, is_monitored, is_special)
         VALUES ($1, $2, $3, $4, true, false)",
    )
    .bind(id)
    .bind(show_id)
    .bind(season_num.to_string())
    .bind(season_num)
    .execute(pool)
    .await
    .unwrap();
    id
}

async fn create_episode(pool: &sqlx::PgPool, season_id: Uuid, episode_num: i32) -> Uuid {
    let id = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO tv_episodes (id, season_id, episode_identifier, episode_sort_order, title)
         VALUES ($1, $2, $3, $4, $5)",
    )
    .bind(id)
    .bind(season_id)
    .bind(episode_num.to_string())
    .bind(episode_num)
    .bind(format!("Episode {episode_num}"))
    .execute(pool)
    .await
    .unwrap();
    id
}

/// Attach a file row to one or more episodes. Returns the file id.
/// The file also lands on disk so the delete paths exercise real cleanup.
async fn attach_file(pool: &sqlx::PgPool, episode_ids: &[Uuid], path: &str) -> Uuid {
    let file_id = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO tv_episode_files (id, file_path, file_size, format, available)
         VALUES ($1, $2, 100, 'mkv', true)",
    )
    .bind(file_id)
    .bind(path)
    .execute(pool)
    .await
    .unwrap();

    for ep in episode_ids {
        sqlx::query(
            "INSERT INTO tv_episode_file_episodes (episode_file_id, episode_id) VALUES ($1, $2)",
        )
        .bind(file_id)
        .bind(ep)
        .execute(pool)
        .await
        .unwrap();
    }
    file_id
}

async fn episode_exists(pool: &sqlx::PgPool, id: Uuid) -> bool {
    sqlx::query_scalar::<_, bool>("SELECT EXISTS (SELECT 1 FROM tv_episodes WHERE id = $1)")
        .bind(id)
        .fetch_one(pool)
        .await
        .unwrap()
}

async fn file_exists(pool: &sqlx::PgPool, id: Uuid) -> bool {
    sqlx::query_scalar::<_, bool>("SELECT EXISTS (SELECT 1 FROM tv_episode_files WHERE id = $1)")
        .bind(id)
        .fetch_one(pool)
        .await
        .unwrap()
}

async fn want_status(pool: &sqlx::PgPool, id: Uuid) -> String {
    sqlx::query_scalar::<_, String>("SELECT want_status FROM tv_episodes WHERE id = $1")
        .bind(id)
        .fetch_one(pool)
        .await
        .unwrap()
}

async fn set_want_status(pool: &sqlx::PgPool, id: Uuid, status: &str) {
    sqlx::query("UPDATE tv_episodes SET want_status = $2 WHERE id = $1")
        .bind(id)
        .bind(status)
        .execute(pool)
        .await
        .unwrap();
}

async fn bulk(server: &TestServer, ids: &[Uuid], action: &str) -> TestResponse {
    server
        .admin_client()
        .post_json(
            "/api/v1/admin/tv/episodes/bulk",
            &json!({ "episode_ids": ids, "action": action }),
        )
        .await
}

// =============================================================================
// delete_files — strip the media, keep the episodes
// =============================================================================

#[tokio::test]
async fn test_bulk_delete_files_keeps_episodes() {
    let server = TestServer::start_api_only().await;
    let pool = &server.pool;

    let show_id = create_show(&server, "Bulk Delete Files").await;
    let season = create_season(pool, show_id, 1).await;
    let ep1 = create_episode(pool, season, 1).await;
    let ep2 = create_episode(pool, season, 2).await;
    let ep3 = create_episode(pool, season, 3).await;

    let f1 = attach_file(pool, &[ep1], "/tmp/bulk-del-1.mkv").await;
    let f2 = attach_file(pool, &[ep2], "/tmp/bulk-del-2.mkv").await;
    let f3 = attach_file(pool, &[ep3], "/tmp/bulk-del-3.mkv").await;

    let resp = bulk(&server, &[ep1, ep2], "delete_files").await;
    resp.assert_ok();
    let body: Value = resp.json();
    assert_eq!(body["files_deleted"], 2);
    assert_eq!(body["episodes_affected"], 2);

    // Files for the selection are gone, the untouched episode keeps its file.
    assert!(!file_exists(pool, f1).await);
    assert!(!file_exists(pool, f2).await);
    assert!(file_exists(pool, f3).await);

    // Every episode row survives — that is the whole point of this action.
    assert!(episode_exists(pool, ep1).await);
    assert!(episode_exists(pool, ep2).await);
    assert!(episode_exists(pool, ep3).await);
}

#[tokio::test]
async fn test_bulk_delete_files_counts_a_shared_file_once() {
    let server = TestServer::start_api_only().await;
    let pool = &server.pool;

    let show_id = create_show(&server, "Double Episode").await;
    let season = create_season(pool, show_id, 1).await;
    let ep1 = create_episode(pool, season, 1).await;
    let ep2 = create_episode(pool, season, 2).await;

    // One file covering both halves of a double episode.
    let shared = attach_file(pool, &[ep1, ep2], "/tmp/bulk-double.mkv").await;

    let resp = bulk(&server, &[ep1, ep2], "delete_files").await;
    resp.assert_ok();
    let body: Value = resp.json();
    assert_eq!(
        body["files_deleted"], 1,
        "a file linked to two selected episodes must be deleted once"
    );
    assert!(!file_exists(pool, shared).await);
}

// =============================================================================
// delete_episodes — remove the rows too
// =============================================================================

#[tokio::test]
async fn test_bulk_delete_episodes_removes_rows_and_files() {
    let server = TestServer::start_api_only().await;
    let pool = &server.pool;

    let show_id = create_show(&server, "Wrong Show Extras").await;
    let season = create_season(pool, show_id, 1).await;
    let keep = create_episode(pool, season, 1).await;
    let extra1 = create_episode(pool, season, 20).await;
    let extra2 = create_episode(pool, season, 21).await;

    let keep_file = attach_file(pool, &[keep], "/tmp/bulk-keep.mkv").await;
    let extra_file = attach_file(pool, &[extra1], "/tmp/bulk-extra.mkv").await;

    let resp = bulk(&server, &[extra1, extra2], "delete_episodes").await;
    resp.assert_ok();
    let body: Value = resp.json();
    assert_eq!(body["episodes_affected"], 2);
    assert_eq!(body["files_deleted"], 1);

    assert!(!episode_exists(pool, extra1).await);
    assert!(!episode_exists(pool, extra2).await);
    assert!(!file_exists(pool, extra_file).await);

    // The rest of the season is untouched.
    assert!(episode_exists(pool, keep).await);
    assert!(file_exists(pool, keep_file).await);
}

#[tokio::test]
async fn test_bulk_delete_episodes_spans_seasons() {
    let server = TestServer::start_api_only().await;
    let pool = &server.pool;

    let show_id = create_show(&server, "Cross Season").await;
    let s1 = create_season(pool, show_id, 1).await;
    let s2 = create_season(pool, show_id, 2).await;
    let a = create_episode(pool, s1, 1).await;
    let b = create_episode(pool, s2, 1).await;

    let resp = bulk(&server, &[a, b], "delete_episodes").await;
    resp.assert_ok();
    assert_eq!(resp.json::<Value>()["episodes_affected"], 2);
    assert!(!episode_exists(pool, a).await);
    assert!(!episode_exists(pool, b).await);
}

// =============================================================================
// want / unwant
// =============================================================================

#[tokio::test]
async fn test_bulk_want_marks_missing_episodes() {
    let server = TestServer::start_api_only().await;
    let pool = &server.pool;

    let show_id = create_show(&server, "Bulk Want").await;
    let season = create_season(pool, show_id, 1).await;
    let ep1 = create_episode(pool, season, 1).await;
    let ep2 = create_episode(pool, season, 2).await;

    let resp = bulk(&server, &[ep1, ep2], "want").await;
    resp.assert_ok();
    assert_eq!(resp.json::<Value>()["episodes_affected"], 2);

    assert_eq!(want_status(pool, ep1).await, "wanted");
    assert_eq!(want_status(pool, ep2).await, "wanted");
}

#[tokio::test]
async fn test_bulk_want_skips_episodes_already_in_library() {
    let server = TestServer::start_api_only().await;
    let pool = &server.pool;

    let show_id = create_show(&server, "Bulk Want In Library").await;
    let season = create_season(pool, show_id, 1).await;
    let have = create_episode(pool, season, 1).await;
    let missing = create_episode(pool, season, 2).await;
    attach_file(pool, &[have], "/tmp/bulk-have.mkv").await;

    let resp = bulk(&server, &[have, missing], "want").await;
    resp.assert_ok();
    assert_eq!(
        resp.json::<Value>()["episodes_affected"],
        1,
        "an episode already in the library is not re-wanted"
    );
    assert_eq!(want_status(pool, have).await, "none");
    assert_eq!(want_status(pool, missing).await, "wanted");
}

#[tokio::test]
async fn test_bulk_unwant_clears_only_outstanding_wants() {
    let server = TestServer::start_api_only().await;
    let pool = &server.pool;

    let show_id = create_show(&server, "Bulk Unwant").await;
    let season = create_season(pool, show_id, 1).await;
    let wanted = create_episode(pool, season, 1).await;
    let idle = create_episode(pool, season, 2).await;
    set_want_status(pool, wanted, "wanted").await;

    let resp = bulk(&server, &[wanted, idle], "unwant").await;
    resp.assert_ok();
    let body: Value = resp.json();
    assert_eq!(
        body["episodes_affected"], 1,
        "only the episode that had an outstanding want counts"
    );
    assert_eq!(want_status(pool, wanted).await, "none");
}

#[tokio::test]
async fn test_bulk_unwant_disables_refilling_auto_want() {
    let server = TestServer::start_api_only().await;
    let pool = &server.pool;

    let show_id = create_show(&server, "Bulk Unwant Auto Want").await;
    let season = create_season(pool, show_id, 1).await;
    let ep = create_episode(pool, season, 1).await;
    set_want_status(pool, ep, "wanted").await;

    // A refilling mode would re-want it within the hour, so unwant must switch it off.
    sqlx::query("UPDATE tv_seasons SET auto_want_mode = 'all' WHERE id = $1")
        .bind(season)
        .execute(pool)
        .await
        .unwrap();

    let resp = bulk(&server, &[ep], "unwant").await;
    resp.assert_ok();
    assert_eq!(resp.json::<Value>()["auto_want_disabled"], true);

    let mode: String = sqlx::query_scalar("SELECT auto_want_mode FROM tv_seasons WHERE id = $1")
        .bind(season)
        .fetch_one(pool)
        .await
        .unwrap();
    assert_eq!(mode, "none");
}

// =============================================================================
// watch state
// =============================================================================

#[tokio::test]
async fn test_bulk_mark_watched_and_unwatched() {
    let server = TestServer::start_api_only().await;
    let pool = &server.pool;

    let show_id = create_show(&server, "Bulk Watch Episodes").await;
    let s1 = create_season(pool, show_id, 1).await;
    let s2 = create_season(pool, show_id, 2).await;
    let ep1 = create_episode(pool, s1, 1).await;
    let ep2 = create_episode(pool, s1, 2).await;
    let untouched = create_episode(pool, s2, 1).await;

    let resp = bulk(&server, &[ep1, ep2], "mark_watched").await;
    resp.assert_ok();
    assert_eq!(resp.json::<Value>()["episodes_affected"], 2);

    for id in [ep1, ep2] {
        let watched: bool =
            sqlx::query_scalar("SELECT watched FROM tv_watch_states WHERE episode_id = $1")
                .bind(id)
                .fetch_one(pool)
                .await
                .unwrap();
        assert!(watched);
    }

    let none: Option<bool> =
        sqlx::query_scalar("SELECT watched FROM tv_watch_states WHERE episode_id = $1")
            .bind(untouched)
            .fetch_optional(pool)
            .await
            .unwrap();
    assert!(
        none.is_none(),
        "episodes outside the selection are untouched"
    );

    let resp = bulk(&server, &[ep1, ep2], "mark_unwatched").await;
    resp.assert_ok();
    assert_eq!(resp.json::<Value>()["episodes_affected"], 2);

    let watched: bool =
        sqlx::query_scalar("SELECT watched FROM tv_watch_states WHERE episode_id = $1")
            .bind(ep1)
            .fetch_one(pool)
            .await
            .unwrap();
    assert!(!watched);
}

// =============================================================================
// Validation
// =============================================================================

#[tokio::test]
async fn test_bulk_rejects_empty_selection() {
    let server = TestServer::start_api_only().await;

    let resp = server
        .admin_client()
        .post_json(
            "/api/v1/admin/tv/episodes/bulk",
            &json!({ "episode_ids": [], "action": "want" }),
        )
        .await;
    resp.assert_status(StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn test_bulk_rejects_unknown_action() {
    let server = TestServer::start_api_only().await;
    let pool = &server.pool;

    let show_id = create_show(&server, "Bad Action").await;
    let season = create_season(pool, show_id, 1).await;
    let ep = create_episode(pool, season, 1).await;

    let resp = server
        .admin_client()
        .post_json(
            "/api/v1/admin/tv/episodes/bulk",
            &json!({ "episode_ids": [ep], "action": "explode" }),
        )
        .await;
    resp.assert_status(StatusCode::UNPROCESSABLE_ENTITY);
}

#[tokio::test]
async fn test_bulk_404s_when_no_episode_exists() {
    let server = TestServer::start_api_only().await;

    let resp = bulk(&server, &[Uuid::now_v7()], "want").await;
    resp.assert_status(StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn test_bulk_ignores_ids_that_no_longer_exist() {
    let server = TestServer::start_api_only().await;
    let pool = &server.pool;

    let show_id = create_show(&server, "Racy Selection").await;
    let season = create_season(pool, show_id, 1).await;
    let ep = create_episode(pool, season, 1).await;

    // A selection can race a delete from another tab; the batch still applies.
    let resp = bulk(&server, &[ep, Uuid::now_v7()], "want").await;
    resp.assert_ok();
    assert_eq!(resp.json::<Value>()["episodes_affected"], 1);
    assert_eq!(want_status(pool, ep).await, "wanted");
}

// =============================================================================
// want_status after the media goes away
//
// A successful import parks an episode at 'imported'. Every "want missing" path
// (list_missing, want_season, auto_want) matches want_status = 'none', so an
// episode left at 'imported' with no file is stranded: it renders as missing but
// nothing will ever pick it up again.
// =============================================================================

#[tokio::test]
async fn test_bulk_delete_files_resets_imported_status() {
    let server = TestServer::start_api_only().await;
    let pool = &server.pool;

    let show_id = create_show(&server, "Imported Then Deleted").await;
    let season = create_season(pool, show_id, 1).await;
    let ep = create_episode(pool, season, 1).await;
    attach_file(pool, &[ep], "/tmp/bulk-imported.mkv").await;
    set_want_status(pool, ep, "imported").await;

    let resp = bulk(&server, &[ep], "delete_files").await;
    resp.assert_ok();

    assert_eq!(
        want_status(pool, ep).await,
        "none",
        "an episode whose last file was deleted must become wantable again"
    );
}

#[tokio::test]
async fn test_bulk_delete_files_preserves_in_flight_acquisition() {
    let server = TestServer::start_api_only().await;
    let pool = &server.pool;

    let show_id = create_show(&server, "Upgrade In Flight").await;
    let season = create_season(pool, show_id, 1).await;
    let ep = create_episode(pool, season, 1).await;
    attach_file(pool, &[ep], "/tmp/bulk-upgrading.mkv").await;
    set_want_status(pool, ep, "downloading").await;

    let resp = bulk(&server, &[ep], "delete_files").await;
    resp.assert_ok();

    assert_eq!(
        want_status(pool, ep).await,
        "downloading",
        "deleting the old file must not cancel an in-flight upgrade"
    );
}

#[tokio::test]
async fn test_delete_single_episode_file_resets_imported_status() {
    let server = TestServer::start_api_only().await;
    let pool = &server.pool;

    let show_id = create_show(&server, "Single File Delete").await;
    let season = create_season(pool, show_id, 1).await;
    let ep = create_episode(pool, season, 1).await;
    let file = attach_file(pool, &[ep], "/tmp/single-imported.mkv").await;
    set_want_status(pool, ep, "imported").await;

    let resp = server
        .admin_client()
        .delete(&format!("/api/v1/admin/tv/episodes/{ep}/files/{file}"))
        .await;
    resp.assert_status(StatusCode::NO_CONTENT);

    assert_eq!(
        want_status(pool, ep).await,
        "none",
        "the single-file delete is the same bug's other entry point"
    );
}

#[tokio::test]
async fn test_delete_one_of_two_files_keeps_imported_status() {
    let server = TestServer::start_api_only().await;
    let pool = &server.pool;

    let show_id = create_show(&server, "Two Copies").await;
    let season = create_season(pool, show_id, 1).await;
    let ep = create_episode(pool, season, 1).await;
    let a = attach_file(pool, &[ep], "/tmp/two-copies-a.mkv").await;
    attach_file(pool, &[ep], "/tmp/two-copies-b.mkv").await;
    set_want_status(pool, ep, "imported").await;

    let resp = server
        .admin_client()
        .delete(&format!("/api/v1/admin/tv/episodes/{ep}/files/{a}"))
        .await;
    resp.assert_status(StatusCode::NO_CONTENT);

    assert_eq!(
        want_status(pool, ep).await,
        "imported",
        "the episode still has a copy, so it is still in the library"
    );
}

/// The stranded state the 0177 backfill exists to clear: 'imported' with no
/// file makes an episode invisible to every want path. Proves the end state is
/// actually wantable rather than just differently-labelled.
#[tokio::test]
async fn test_reset_episode_is_visible_to_want_season_again() {
    let server = TestServer::start_api_only().await;
    let pool = &server.pool;

    let show_id = create_show(&server, "Stranded Then Freed").await;
    let season = create_season(pool, show_id, 1).await;
    let ep = create_episode(pool, season, 1).await;
    attach_file(pool, &[ep], "/tmp/stranded.mkv").await;
    set_want_status(pool, ep, "imported").await;

    bulk(&server, &[ep], "delete_files").await.assert_ok();

    // want_season only touches episodes list_missing returns, and that matches
    // want_status = 'none' — the whole reason the stranded state was a dead end.
    let resp = server
        .admin_client()
        .post_json(
            &format!("/api/v1/admin/tv/seasons/{season}/want"),
            &json!({}),
        )
        .await;
    resp.assert_status(StatusCode::NO_CONTENT);

    assert_eq!(
        want_status(pool, ep).await,
        "wanted",
        "a freed episode must be reachable by Want Missing"
    );
}
