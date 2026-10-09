//! A regrab ("download another copy") **adds** a file; it never replaces one.
//!
//! The default on every other path stays replace — a rescan or a re-import of the
//! same episode must not leave duplicates behind, which `file_replacement_test.rs`
//! pins. What is new is an explicit intent, carried from the request that caused
//! the download all the way to the linking step, that says "keep the old one".

use crate::common::*;
use api::file_link_mode::{FileLinkMode, tag_metadata};
use api::jobs::handlers::{MovieWantPayload, SportsWantPayload, TvWantPayload};
use api::repositories::{movie_files, tv_episode_files};
use serde_json::{Value, json};
use sqlx::PgPool;
use uuid::Uuid;
use wiremock::matchers::{method, path, path_regex};
use wiremock::{Mock, MockServer, ResponseTemplate};

// =============================================================================
// Helpers
// =============================================================================

async fn create_show_season_episode(server: &TestServer, title: &str) -> (Uuid, Uuid, Uuid) {
    let show: Value = server
        .admin_client()
        .post_json("/api/v1/admin/tv/shows", &json!({ "title": title }))
        .await
        .json();
    let show_id: Uuid = show["id"].as_str().unwrap().parse().unwrap();

    let season: Value = server
        .admin_client()
        .post_json(
            &format!("/api/v1/admin/tv/shows/{show_id}/seasons"),
            &json!({ "season_identifier": "1", "season_sort_order": 1 }),
        )
        .await
        .json();
    let season_id: Uuid = season["id"].as_str().unwrap().parse().unwrap();

    let ep: Value = server
        .admin_client()
        .post_json(
            &format!("/api/v1/admin/tv/seasons/{season_id}/episodes"),
            &json!({ "episode_identifier": "1", "episode_sort_order": 1, "title": "Ep 1" }),
        )
        .await
        .json();
    let episode_id: Uuid = ep["id"].as_str().unwrap().parse().unwrap();

    (show_id, season_id, episode_id)
}

async fn create_episode_file(pool: &PgPool, tag: &str) -> Uuid {
    tv_episode_files::create(
        pool,
        tv_episode_files::EpisodeFileInput {
            file_path: format!("/test/regrab/{tag}-{}.mkv", Uuid::now_v7()),
            file_size: Some(1024),
            file_fingerprint: None,
            format: "mkv".to_string(),
            quality: None,
            languages: None,
            subtitles: None,
            source_type: Some("test".to_string()),
            source_metadata: None,
            video_codec: None,
            video_width: None,
            video_height: None,
            video_bitrate_kbps: None,
            framerate: None,
            bit_depth: None,
            is_hdr: false,
            is_interlaced: false,
            hdr_format: None,
            duration_seconds: None,
            container: None,
            overall_bitrate_kbps: None,
        },
    )
    .await
    .expect("create episode file")
}

async fn create_movie(server: &TestServer, title: &str) -> Uuid {
    let movie: Value = server
        .admin_client()
        .post_json("/api/v1/admin/movies", &json!({ "title": title }))
        .await
        .json();
    movie["id"].as_str().unwrap().parse().unwrap()
}

async fn create_movie_file(pool: &PgPool, movie_id: Uuid, tag: &str) -> Uuid {
    movie_files::create(
        pool,
        movie_files::MovieFileInput {
            movie_id,
            file_path: format!("/test/regrab/{tag}-{}.mkv", Uuid::now_v7()),
            file_size: Some(1024),
            format: "mkv".to_string(),
            ..Default::default()
        },
    )
    .await
    .expect("create movie file")
}

/// Push a file's `created_at` back so "newest" is unambiguous regardless of
/// clock granularity.
async fn age_episode_file(pool: &PgPool, file_id: Uuid, hours: i32) {
    sqlx::query(
        "UPDATE tv_episode_files SET created_at = NOW() - ($2 || ' hours')::interval WHERE id = $1",
    )
    .bind(file_id)
    .bind(hours.to_string())
    .execute(pool)
    .await
    .expect("age episode file");
}

async fn age_movie_file(pool: &PgPool, file_id: Uuid, hours: i32) {
    sqlx::query(
        "UPDATE movie_files SET created_at = NOW() - ($2 || ' hours')::interval WHERE id = $1",
    )
    .bind(file_id)
    .bind(hours.to_string())
    .execute(pool)
    .await
    .expect("age movie file");
}

async fn episode_file_count(pool: &PgPool, episode_id: Uuid) -> i64 {
    sqlx::query_scalar("SELECT COUNT(*) FROM tv_episode_file_episodes WHERE episode_id = $1")
        .bind(episode_id)
        .fetch_one(pool)
        .await
        .unwrap()
}

async fn configure_kobload(server: &TestServer, mock: &MockServer) {
    server
        .admin_client()
        .patch_json(
            "/api/v1/admin/config",
            &json!({
                "kobload_base_url": mock.uri(),
                "kobload_api_key": "test-key",
                "kobload_ingest_url": format!("{}/api/v1/ingest/{}", server.api_url, server.ingest_token),
            }),
        )
        .await
        .assert_ok();
}

async fn mock_create_download(mock: &MockServer) {
    Mock::given(method("POST"))
        .and(path("/api/v1/downloads"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "id": Uuid::now_v7(),
            "status": "queued"
        })))
        .mount(mock)
        .await;
}

/// The metadata blob kobbler handed kobload, which kobload echoes back verbatim
/// on the ingest callback. This is the wire the intent travels on.
async fn sent_metadata(mock: &MockServer) -> Value {
    let requests = mock.received_requests().await.expect("recorded requests");
    let last = requests.last().expect("a download was created");
    let body: Value = serde_json::from_slice(&last.body).expect("json body");
    body["metadata"].clone()
}

// =============================================================================
// Entry point 1: the episode linking step
// =============================================================================

#[tokio::test]
async fn regrab_adds_a_second_episode_file_and_the_new_one_plays() {
    let server = TestServer::start_api_only().await;
    let pool = &server.pool;
    let (_show, _season, episode_id) = create_show_season_episode(&server, "Regrab Adds TV").await;

    let old_file = create_episode_file(pool, "old").await;
    tv_episode_files::link_episodes(pool, old_file, &[episode_id])
        .await
        .unwrap();

    let new_file = create_episode_file(pool, "new").await;
    let replaced = tv_episode_files::link_episodes_with_mode(
        pool,
        new_file,
        &[episode_id],
        FileLinkMode::AddCopy,
    )
    .await
    .unwrap();

    assert!(
        replaced.is_empty(),
        "a regrab must not hand back files for physical cleanup, got {replaced:?}"
    );
    assert_eq!(
        episode_file_count(pool, episode_id).await,
        2,
        "the old file must still be linked to the episode"
    );
    assert!(
        tv_episode_files::get(pool, old_file)
            .await
            .unwrap()
            .is_some(),
        "the old episode file row was deleted — the bot's promise would be a lie"
    );

    age_episode_file(pool, old_file, 48).await;
    age_episode_file(pool, new_file, 1).await;
    let picked = tv_episode_files::get_preferred_available(pool, episode_id)
        .await
        .unwrap()
        .expect("a file to play");
    assert_eq!(picked.id, new_file, "the newest copy must play");
}

#[tokio::test]
async fn default_episode_link_still_replaces() {
    let server = TestServer::start_api_only().await;
    let pool = &server.pool;
    let (_show, _season, episode_id) =
        create_show_season_episode(&server, "Regrab Default TV").await;

    let old_file = create_episode_file(pool, "old").await;
    tv_episode_files::link_episodes(pool, old_file, &[episode_id])
        .await
        .unwrap();

    let new_file = create_episode_file(pool, "new").await;
    let replaced = tv_episode_files::link_episodes_with_mode(
        pool,
        new_file,
        &[episode_id],
        FileLinkMode::Replace,
    )
    .await
    .unwrap();

    assert_eq!(
        replaced.len(),
        1,
        "the superseded path comes back for cleanup"
    );
    assert_eq!(episode_file_count(pool, episode_id).await, 1);
    assert!(
        tv_episode_files::get(pool, old_file)
            .await
            .unwrap()
            .is_none(),
        "replace mode must still delete the old row"
    );
}

/// A regrab of one episode of a multi-episode file must not disturb the pack —
/// the exclusivity rule and the add-don't-replace rule are independent.
#[tokio::test]
async fn regrab_leaves_a_multi_episode_file_alone() {
    let server = TestServer::start_api_only().await;
    let pool = &server.pool;
    let (_show, season_id, ep1) = create_show_season_episode(&server, "Regrab Multi Ep").await;
    let ep2: Value = server
        .admin_client()
        .post_json(
            &format!("/api/v1/admin/tv/seasons/{season_id}/episodes"),
            &json!({ "episode_identifier": "2", "episode_sort_order": 2 }),
        )
        .await
        .json();
    let ep2: Uuid = ep2["id"].as_str().unwrap().parse().unwrap();

    let pack = create_episode_file(pool, "pack").await;
    tv_episode_files::link_episodes(pool, pack, &[ep1, ep2])
        .await
        .unwrap();

    let single = create_episode_file(pool, "single").await;
    let replaced =
        tv_episode_files::link_episodes_with_mode(pool, single, &[ep1], FileLinkMode::AddCopy)
            .await
            .unwrap();

    assert!(replaced.is_empty());
    assert!(tv_episode_files::get(pool, pack).await.unwrap().is_some());
    assert_eq!(episode_file_count(pool, ep1).await, 2);
    assert_eq!(episode_file_count(pool, ep2).await, 1);
}

// =============================================================================
// Entry point 2: the movie linking step
// =============================================================================

#[tokio::test]
async fn regrab_adds_a_second_movie_file_and_the_new_one_plays() {
    let server = TestServer::start_api_only().await;
    let pool = &server.pool;
    let movie_id = create_movie(&server, "Regrab Adds Movie").await;

    let old_file = create_movie_file(pool, movie_id, "old").await;
    let new_file = create_movie_file(pool, movie_id, "new").await;

    let replaced =
        movie_files::replace_other_files_for_movie(pool, movie_id, new_file, FileLinkMode::AddCopy)
            .await
            .unwrap();

    assert!(
        replaced.is_empty(),
        "a regrab deletes nothing, got {replaced:?}"
    );
    assert_eq!(
        movie_files::count_by_movie(pool, movie_id).await.unwrap(),
        2,
        "the old movie file must still be there"
    );

    age_movie_file(pool, old_file, 48).await;
    age_movie_file(pool, new_file, 1).await;
    let picked = movie_files::get_preferred_available(pool, movie_id)
        .await
        .unwrap()
        .expect("a file to play");
    assert_eq!(picked.id, new_file, "the newest copy must play");
}

#[tokio::test]
async fn default_movie_link_still_replaces() {
    let server = TestServer::start_api_only().await;
    let pool = &server.pool;
    let movie_id = create_movie(&server, "Regrab Default Movie").await;

    let old_file = create_movie_file(pool, movie_id, "old").await;
    let new_file = create_movie_file(pool, movie_id, "new").await;

    let replaced =
        movie_files::replace_other_files_for_movie(pool, movie_id, new_file, FileLinkMode::Replace)
            .await
            .unwrap();

    assert_eq!(replaced.len(), 1);
    assert_eq!(
        movie_files::count_by_movie(pool, movie_id).await.unwrap(),
        1
    );
    assert!(movie_files::get(pool, old_file).await.unwrap().is_none());
}

// =============================================================================
// Entry point 3: the grab endpoints the regrab tool calls
// =============================================================================

async fn queue_episode(additional_copy: Option<bool>) -> Value {
    let mock = MockServer::start().await;
    let server = TestServer::start_api_only().await;
    configure_kobload(&server, &mock).await;
    mock_create_download(&mock).await;

    let (_show, _season, episode_id) = create_show_season_episode(&server, "Queue Regrab TV").await;

    let mut body = json!({
        "source": "manual",
        "release": {
            "protocol": "usenet",
            "download_url": "https://example.com/ep.nzb",
            "title": "Show.S01E01.1080p",
        }
    });
    if let Some(flag) = additional_copy {
        body["additional_copy"] = json!(flag);
    }

    server
        .admin_client()
        .post_json(
            &format!("/api/v1/admin/tv/episodes/{episode_id}/queue"),
            &body,
        )
        .await
        .assert_status(reqwest::StatusCode::NO_CONTENT);

    sent_metadata(&mock).await
}

#[tokio::test]
async fn episode_grab_carries_the_regrab_intent_to_kobload() {
    let metadata = queue_episode(Some(true)).await;
    assert_eq!(
        metadata["additional_copy"],
        json!(true),
        "the regrab intent never left kobbler: {metadata}"
    );
}

#[tokio::test]
async fn episode_grab_without_the_flag_still_means_replace() {
    let metadata = queue_episode(None).await;
    assert_eq!(metadata["additional_copy"], json!(false));
}

#[tokio::test]
async fn movie_grab_carries_the_regrab_intent_to_kobload() {
    let mock = MockServer::start().await;
    let server = TestServer::start_api_only().await;
    configure_kobload(&server, &mock).await;
    mock_create_download(&mock).await;

    let movie_id = create_movie(&server, "Queue Regrab Movie").await;

    server
        .admin_client()
        .post_json(
            &format!("/api/v1/admin/movies/{movie_id}/queue"),
            &json!({
                "source": "manual",
                "additional_copy": true,
                "release": {
                    "protocol": "usenet",
                    "download_url": "https://example.com/movie.nzb",
                    "title": "Movie.2024.1080p",
                }
            }),
        )
        .await
        .assert_status(reqwest::StatusCode::NO_CONTENT);

    let metadata = sent_metadata(&mock).await;
    assert_eq!(metadata["additional_copy"], json!(true));
    assert_eq!(metadata["source_type"], json!("movie"));
}

/// The want routes take the intent as a query flag; the extractor must not
/// reject a plain request, and must accept the opt-in.
#[tokio::test]
async fn want_episode_accepts_the_additional_copy_flag() {
    let server = TestServer::start_api_only().await;
    let (_show, _season, episode_id) = create_show_season_episode(&server, "Want Regrab TV").await;

    server
        .admin_client()
        .post(&format!("/api/v1/admin/tv/episodes/{episode_id}/want"))
        .await
        .assert_success();

    server
        .admin_client()
        .post(&format!(
            "/api/v1/admin/tv/episodes/{episode_id}/want?additional_copy=true"
        ))
        .await
        .assert_success();
}

// =============================================================================
// End to end: the ingest callback that actually links the file
// =============================================================================

/// Drive a real kobload ingest callback for one episode and return the file id.
/// This is the hop the whole change exists for: the intent set on the grab has
/// to still be there when `link_episodes_with_mode` runs, minutes later, in a
/// different process.
async fn ingest_episode_download(
    server: &TestServer,
    ids: (Uuid, Uuid, Uuid),
    file_name: &str,
    additional_copy: bool,
) {
    let (show_id, season_id, episode_id) = ids;
    server
        .api_client()
        .post_json(
            &format!("/api/v1/ingest/{}", server.ingest_token),
            &json!({
                "download_id": Uuid::now_v7(),
                "status": "completed",
                "name": file_name,
                "metadata": {
                    "source_type": "tv_episode",
                    "show_id": show_id,
                    "season_id": season_id,
                    "episode_id": episode_id,
                    "additional_copy": additional_copy,
                },
                "files": [
                    { "path": file_name, "size": 2048, "extension": "mkv" }
                ],
            }),
        )
        .await
        .assert_success();

    server.process_pending_ingest_jobs().await;
}

#[tokio::test]
async fn ingesting_a_regrab_leaves_the_first_episode_file_in_place() {
    let mock = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path_regex(r"^/api/v1/downloads/.+/files/.+$"))
        .respond_with(ResponseTemplate::new(200).set_body_bytes(vec![0u8; 2048]))
        .mount(&mock)
        .await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(204))
        .mount(&mock)
        .await;

    let server = TestServer::start_api_only().await;
    configure_kobload(&server, &mock).await;
    let ids = create_show_season_episode(&server, "E2E Regrab Show").await;
    let episode_id = ids.2;

    ingest_episode_download(&server, ids, "Show.S01E01.WEB.mkv", false).await;
    assert_eq!(
        episode_file_count(&server.pool, episode_id).await,
        1,
        "the first ingest should land one file"
    );

    ingest_episode_download(&server, ids, "Show.S01E01.BluRay.mkv", true).await;

    assert_eq!(
        episode_file_count(&server.pool, episode_id).await,
        2,
        "the regrab replaced the first file instead of joining it"
    );

    let files = tv_episode_files::list_by_episode(&server.pool, episode_id)
        .await
        .unwrap();
    assert!(
        files.iter().any(|f| f.file_path.ends_with("WEB.mkv")),
        "the original copy is gone: {:?}",
        files.iter().map(|f| &f.file_path).collect::<Vec<_>>()
    );

    let picked = tv_episode_files::get_preferred_available(&server.pool, episode_id)
        .await
        .unwrap()
        .expect("a file to play");
    assert!(
        picked.file_path.ends_with("BluRay.mkv"),
        "the regrabbed copy should play, got {}",
        picked.file_path
    );
}

/// The same flow without the flag — the ordinary upgrade path — must still
/// replace, or every RSS grab starts stacking duplicates.
#[tokio::test]
async fn ingesting_without_the_flag_still_replaces_the_episode_file() {
    let mock = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path_regex(r"^/api/v1/downloads/.+/files/.+$"))
        .respond_with(ResponseTemplate::new(200).set_body_bytes(vec![0u8; 2048]))
        .mount(&mock)
        .await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(204))
        .mount(&mock)
        .await;

    let server = TestServer::start_api_only().await;
    configure_kobload(&server, &mock).await;
    let ids = create_show_season_episode(&server, "E2E Replace Show").await;
    let episode_id = ids.2;

    ingest_episode_download(&server, ids, "Show.S01E01.WEB.mkv", false).await;
    ingest_episode_download(&server, ids, "Show.S01E01.BluRay.mkv", false).await;

    assert_eq!(
        episode_file_count(&server.pool, episode_id).await,
        1,
        "the default path must still replace"
    );
}

// =============================================================================
// Entry point 4: the want payloads, and old in-flight messages
// =============================================================================

#[test]
fn want_payloads_default_to_replace_when_the_field_is_absent() {
    let show_id = Uuid::now_v7();
    let season_id = Uuid::now_v7();

    let tv: TvWantPayload =
        serde_json::from_value(json!({ "show_id": show_id, "season_id": season_id })).unwrap();
    assert!(
        !tv.additional_copy,
        "an in-flight tv_want must still decode"
    );

    let movie: MovieWantPayload =
        serde_json::from_value(json!({ "movie_id": Uuid::now_v7() })).unwrap();
    assert!(!movie.additional_copy);

    let sports: SportsWantPayload =
        serde_json::from_value(json!({ "show_id": show_id, "season_id": season_id })).unwrap();
    assert!(!sports.additional_copy);
}

#[test]
fn want_payloads_carry_the_flag_when_present() {
    let show_id = Uuid::now_v7();
    let season_id = Uuid::now_v7();

    let tv: TvWantPayload = serde_json::from_value(json!({
        "show_id": show_id,
        "season_id": season_id,
        "additional_copy": true,
    }))
    .unwrap();
    assert!(tv.additional_copy);

    let movie: MovieWantPayload = serde_json::from_value(json!({
        "movie_id": Uuid::now_v7(),
        "additional_copy": true,
    }))
    .unwrap();
    assert!(movie.additional_copy);

    let sports: SportsWantPayload = serde_json::from_value(json!({
        "show_id": show_id,
        "season_id": season_id,
        "additional_copy": true,
    }))
    .unwrap();
    assert!(sports.additional_copy);
}

#[test]
fn tagging_metadata_is_lossless_and_additive() {
    let tagged = tag_metadata(
        json!({ "source_type": "tv_episode", "user_id": null }),
        FileLinkMode::AddCopy,
    );
    assert_eq!(tagged["additional_copy"], json!(true));
    assert_eq!(tagged["source_type"], json!("tv_episode"));

    let untagged = tag_metadata(json!({ "source_type": "movie" }), FileLinkMode::Replace);
    assert_eq!(untagged["additional_copy"], json!(false));
}
