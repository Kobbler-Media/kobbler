//! Rows picked with `LIMIT 1` / `[0]` and no `ORDER BY` come back in whatever
//! order Postgres happens to produce, so every one of these bugs is invisible
//! until a *second* row exists. Each test here sets up that second row — a
//! second author, a second show directory, a second file — and pins the choice.

use crate::common::*;
use api::media_cache::PIPELINE_VERSION;
use api::media_cache::episode_cache::{MediaKind, episode_cache_dir};
use api::repositories::tv_episode_files;
use reqwest::StatusCode;
use serde_json::{Value, json};
use sqlx::PgPool;
use std::path::Path;
use tempfile::TempDir;
use uuid::Uuid;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

// =============================================================================
// Shared fixtures
// =============================================================================

/// `book_authors` has no primary flag, so the codebase's convention is the
/// lowest `author_id`. These ids are hand-picked so that "lowest id" and
/// "physically first row" are deliberately different answers.
const AUTHOR_LO: Uuid = Uuid::from_u128(0x0000_0000_0000_4000_8000_0000_0000_0001);
const AUTHOR_HI: Uuid = Uuid::from_u128(0xffff_ffff_ffff_4fff_8fff_ffff_ffff_ffff);
const NAME_LO: &str = "Aay Writer";
const NAME_HI: &str = "Zed Writer";

async fn insert_author(pool: &PgPool, id: Uuid, name: &str) {
    sqlx::query(
        "INSERT INTO authors (id, name, normalized_name) VALUES ($1, $2, LOWER($2))
         ON CONFLICT (id) DO NOTHING",
    )
    .bind(id)
    .bind(name)
    .execute(pool)
    .await
    .expect("insert author");
}

async fn link_author(pool: &PgPool, book_id: Uuid, author_id: Uuid) {
    sqlx::query("INSERT INTO book_authors (book_id, author_id) VALUES ($1, $2)")
        .bind(book_id)
        .bind(author_id)
        .execute(pool)
        .await
        .expect("link author");
}

/// A book credited to both authors. The high-id author is linked *first* so a
/// sequential scan hands it back first — the pre-fix answer.
async fn co_authored_book(server: &TestServer, title: &str) -> Uuid {
    let book_id = create_book(server, title).await;
    sqlx::query("DELETE FROM book_authors WHERE book_id = $1")
        .bind(book_id)
        .execute(&server.pool)
        .await
        .expect("clear placeholder author");
    insert_author(&server.pool, AUTHOR_HI, NAME_HI).await;
    insert_author(&server.pool, AUTHOR_LO, NAME_LO).await;
    link_author(&server.pool, book_id, AUTHOR_HI).await;
    link_author(&server.pool, book_id, AUTHOR_LO).await;
    book_id
}

// =============================================================================
// #8 — the downloads list picks the author twice, independently
// =============================================================================

#[tokio::test]
async fn downloads_enrichment_names_the_primary_author() {
    let kobload = MockServer::start().await;
    let server = TestServer::start_api_only().await;
    let book_id = co_authored_book(&server, "Two Names On The Cover").await;

    Mock::given(method("GET"))
        .and(path("/api/v1/downloads"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "items": [{
                "id": "dl-1",
                "status": "completed",
                "metadata": { "book_id": book_id }
            }],
            "next_cursor": null
        })))
        .mount(&kobload)
        .await;

    server
        .admin_client()
        .patch_json(
            "/api/v1/admin/config",
            &json!({ "kobload_base_url": kobload.uri(), "kobload_api_key": "test-key" }),
        )
        .await
        .assert_ok();

    let resp = server.admin_client().get("/api/v1/admin/downloads").await;
    resp.assert_status(StatusCode::OK);
    let body: Value = resp.json();
    let media = &body["items"][0]["media"];

    // The name and the id are two separate subqueries: they must agree with
    // each other, and with the author every other call site would render.
    assert_eq!(media["author"].as_str(), Some(NAME_LO));
    assert_eq!(
        media["author_id"].as_str(),
        Some(AUTHOR_LO.to_string().as_str())
    );
}

// =============================================================================
// #8 — book enrichment searches the provider for an arbitrary author
// =============================================================================

#[tokio::test]
async fn book_enrichment_searches_for_the_primary_author() {
    let server = TestServer::start_api_only().await;
    let book_id = co_authored_book(&server, "Enrich Me").await;

    let author = api::jobs::handlers::book_enrich::primary_author_name(&server.pool, book_id)
        .await
        .expect("author lookup");

    assert_eq!(author.as_deref(), Some(NAME_LO));
}

// =============================================================================
// #11 — sidecar and artwork written relative to an arbitrary episode file
// =============================================================================

struct SplitShow {
    show_id: Uuid,
    first_episode: Uuid,
    second_episode: Uuid,
    /// Deterministic show directory: the lowest episode file path wins.
    canonical_dir: std::path::PathBuf,
    root: TempDir,
}

async fn insert_root_file(pool: &PgPool, root_id: Uuid, episode_id: Uuid, file_path: &Path) {
    std::fs::create_dir_all(file_path.parent().unwrap()).expect("create dir");
    std::fs::write(file_path, b"not really an mkv").expect("write file");
    sqlx::query(
        "INSERT INTO media_root_files (id, media_root_id, file_path, file_name, match_status,
                                       entity_type, entity_id)
         VALUES ($1, $2, $3, $4, 'added', 'tv_episode', $5)",
    )
    .bind(Uuid::now_v7())
    .bind(root_id)
    .bind(file_path.to_string_lossy().to_string())
    .bind(file_path.file_name().unwrap().to_string_lossy().to_string())
    .bind(episode_id)
    .execute(pool)
    .await
    .expect("insert media_root_file");
}

/// One show whose episodes live in two different directories — a rename that
/// only moved half the files, which is how this happens in the wild.
async fn split_show(server: &TestServer) -> SplitShow {
    let root = TempDir::new().unwrap();
    let resp = server
        .admin_client()
        .post_json(
            "/api/v1/admin/media-roots",
            &json!({
                "path": root.path().to_string_lossy(),
                "name": "Split TV",
                "media_type": "tv",
                "auto_add": false,
            }),
        )
        .await;
    resp.assert_status(StatusCode::CREATED);
    let root_id: Uuid = resp.json::<Value>()["id"]
        .as_str()
        .unwrap()
        .parse()
        .unwrap();

    let show: Value = server
        .admin_client()
        .post_json(
            "/api/v1/admin/tv/shows",
            &json!({ "title": "Split Show", "year": 2024 }),
        )
        .await
        .json();
    let show_id: Uuid = show["id"].as_str().unwrap().parse().unwrap();
    sqlx::query("UPDATE tv_shows SET media_root_id = $1 WHERE id = $2")
        .bind(root_id)
        .bind(show_id)
        .execute(&server.pool)
        .await
        .expect("set media root");

    let season: Value = server
        .admin_client()
        .post_json(
            &format!("/api/v1/admin/tv/shows/{show_id}/seasons"),
            &json!({ "season_identifier": "1", "season_sort_order": 1 }),
        )
        .await
        .json();
    let season_id = season["id"].as_str().unwrap().to_string();

    let mut episodes = Vec::new();
    for n in 1..=2 {
        let ep: Value = server
            .admin_client()
            .post_json(
                &format!("/api/v1/admin/tv/seasons/{season_id}/episodes"),
                &json!({
                    "episode_identifier": n.to_string(),
                    "episode_sort_order": n,
                    "title": format!("Episode {n}"),
                }),
            )
            .await
            .json();
        episodes.push(ep["id"].as_str().unwrap().parse::<Uuid>().unwrap());
    }

    // Inserted high-path-first so a sequential scan hands back the *other*
    // directory than the deterministic answer.
    let zed = root.path().join("Zed Show/Season 01/Zed Show - S01E02.mkv");
    insert_root_file(&server.pool, root_id, episodes[1], &zed).await;
    let alpha = root
        .path()
        .join("Alpha Show (2024)/Season 01/Alpha Show - S01E01.mkv");
    insert_root_file(&server.pool, root_id, episodes[0], &alpha).await;

    SplitShow {
        show_id,
        first_episode: episodes[0],
        second_episode: episodes[1],
        canonical_dir: root.path().join("Alpha Show (2024)"),
        root,
    }
}

fn count_files(dir: &Path, name: &str) -> usize {
    let mut found = 0;
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&d) else {
            continue;
        };
        for entry in entries.flatten() {
            let p = entry.path();
            if p.is_dir() {
                stack.push(p);
            } else if p.file_name().and_then(|s| s.to_str()) == Some(name) {
                found += 1;
            }
        }
    }
    found
}

#[tokio::test]
async fn tvshow_nfo_lands_in_one_directory_for_a_split_show() {
    let server = TestServer::start_api_only().await;
    let show = split_show(&server).await;

    // The show-level pass and the per-episode pass must agree on the show
    // directory, or each writes its own tvshow.nfo and the media player reads
    // whichever it finds first.
    api::sidecar::write_back::write_tvshow_sidecars(&server.pool, show.show_id).await;
    api::sidecar::write_back::write_episode_sidecar(&server.pool, show.first_episode).await;
    api::sidecar::write_back::write_episode_sidecar(&server.pool, show.second_episode).await;

    assert_eq!(
        count_files(show.root.path(), "tvshow.nfo"),
        1,
        "a split show must not scatter tvshow.nfo across its directories"
    );
    assert!(
        show.canonical_dir.join("tvshow.nfo").exists(),
        "tvshow.nfo belongs in the deterministically chosen show directory"
    );
}

#[tokio::test]
async fn show_poster_lands_next_to_the_tvshow_nfo() {
    let server = TestServer::start_api_only().await;
    let show = split_show(&server).await;
    let storage_root = server.app_state.config.storage_root.clone();

    // Pre-seed the image cache so no download is needed.
    let cached = api::images::local_image_path(
        &storage_root,
        api::images::ImageType::TvShowPoster,
        show.show_id,
    );
    std::fs::create_dir_all(cached.parent().unwrap()).unwrap();
    std::fs::write(&cached, b"jpegbytes").unwrap();

    api::sidecar::write_back::write_tvshow_sidecars(&server.pool, show.show_id).await;
    api::sidecar::write_back::write_tvshow_images(&server.pool, &storage_root, show.show_id).await;

    assert_eq!(count_files(show.root.path(), "poster.jpg"), 1);
    assert!(
        show.canonical_dir.join("poster.jpg").exists(),
        "the poster must land beside the tvshow.nfo, not in a sibling directory"
    );
}

// =============================================================================
// #13 — book directory reuse borrows any co-author's folder
// =============================================================================

async fn book_root(server: &TestServer) -> (Uuid, TempDir) {
    let dir = TempDir::new().unwrap();
    let resp = server
        .admin_client()
        .post_json(
            "/api/v1/admin/media-roots",
            &json!({
                "path": dir.path().to_string_lossy(),
                "name": "Books",
                "media_type": "book",
                "auto_add": false,
            }),
        )
        .await;
    resp.assert_status(StatusCode::CREATED);
    let id: Uuid = resp.json::<Value>()["id"]
        .as_str()
        .unwrap()
        .parse()
        .unwrap();
    (id, dir)
}

async fn insert_book_file(pool: &PgPool, root_id: Uuid, book_id: Uuid, file_path: &str) {
    sqlx::query(
        "INSERT INTO media_root_files (id, media_root_id, file_path, file_name, match_status,
                                       entity_type, entity_id)
         VALUES ($1, $2, $3, 'book.epub', 'added', 'book', $4)",
    )
    .bind(Uuid::now_v7())
    .bind(root_id)
    .bind(file_path)
    .bind(book_id)
    .execute(pool)
    .await
    .expect("insert media_root_file");
}

#[tokio::test]
async fn book_dir_reuse_ignores_a_co_authors_directory() {
    let server = TestServer::start_api_only().await;
    let (root_id, root_dir) = book_root(&server).await;

    // The co-author (high id, so *not* the one the naming template renders)
    // already has a folder of their own.
    let solo = create_book(&server, "Zed's Solo Book").await;
    sqlx::query("DELETE FROM book_authors WHERE book_id = $1")
        .bind(solo)
        .execute(&server.pool)
        .await
        .unwrap();
    insert_author(&server.pool, AUTHOR_HI, NAME_HI).await;
    link_author(&server.pool, solo, AUTHOR_HI).await;
    insert_book_file(
        &server.pool,
        root_id,
        solo,
        &format!(
            "{}/Zed Writer/Zed's Solo Book/Zed's Solo Book.epub",
            root_dir.path().to_string_lossy()
        ),
    )
    .await;

    let shared = co_authored_book(&server, "Shared Book").await;

    let existing = api::media_root_naming::find_existing_file_for_entity(
        &server.pool,
        root_id,
        "book",
        shared,
    )
    .await;

    assert_eq!(
        existing, None,
        "the primary author has no directory yet, so nothing may be reused"
    );
}

#[tokio::test]
async fn book_dir_reuse_finds_the_primary_authors_directory() {
    let server = TestServer::start_api_only().await;
    let (root_id, root_dir) = book_root(&server).await;
    let root = root_dir.path().to_string_lossy().to_string();

    let zed_book = create_book(&server, "Zed's Solo Book").await;
    let aay_book = create_book(&server, "Aay's Solo Book").await;
    for b in [zed_book, aay_book] {
        sqlx::query("DELETE FROM book_authors WHERE book_id = $1")
            .bind(b)
            .execute(&server.pool)
            .await
            .unwrap();
    }
    insert_author(&server.pool, AUTHOR_HI, NAME_HI).await;
    insert_author(&server.pool, AUTHOR_LO, NAME_LO).await;
    link_author(&server.pool, zed_book, AUTHOR_HI).await;
    link_author(&server.pool, aay_book, AUTHOR_LO).await;

    let zed_path = format!("{root}/Zed Writer/Zed's Solo Book/Zed's Solo Book.epub");
    let aay_path = format!("{root}/Aay Writer/Aay's Solo Book/Aay's Solo Book.epub");
    insert_book_file(&server.pool, root_id, zed_book, &zed_path).await;
    insert_book_file(&server.pool, root_id, aay_book, &aay_path).await;

    let shared = co_authored_book(&server, "Shared Book").await;
    let existing = api::media_root_naming::find_existing_file_for_entity(
        &server.pool,
        root_id,
        "book",
        shared,
    )
    .await;

    assert_eq!(
        existing.as_deref(),
        Some(aay_path.as_str()),
        "reuse must follow the author the naming template renders"
    );
}

// =============================================================================
// #13 — season cache detection walks an episode's files in an arbitrary order
// =============================================================================

async fn create_episode_file(pool: &PgPool, episode_id: Uuid, tag: &str, age_hours: i32) -> Uuid {
    let file_id = tv_episode_files::create(
        pool,
        tv_episode_files::EpisodeFileInput {
            file_path: format!("/test/episodes/{episode_id}-{tag}.mkv"),
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
    .expect("create episode file");
    sqlx::query(
        "INSERT INTO tv_episode_file_episodes (episode_file_id, episode_id) VALUES ($1, $2)",
    )
    .bind(file_id)
    .bind(episode_id)
    .execute(pool)
    .await
    .expect("link episode file");
    sqlx::query(
        "UPDATE tv_episode_files SET created_at = NOW() - ($2 || ' hours')::interval WHERE id = $1",
    )
    .bind(file_id)
    .bind(age_hours.to_string())
    .execute(pool)
    .await
    .expect("age episode file");
    file_id
}

fn write_manifest(storage_root: &Path, file_id: Uuid, duration: f64) {
    let dir = episode_cache_dir(
        &storage_root.to_string_lossy(),
        &MediaKind::TvFile { file_id },
    );
    std::fs::create_dir_all(&dir).expect("create cache dir");
    let manifest = json!({
        "pipeline_version": PIPELINE_VERSION,
        "generated_at": "2026-01-01T00:00:00Z",
        "input_duration_seconds": duration,
        "trickplay_dir": dir.join("trickplay").to_string_lossy(),
        "artefacts": {},
    });
    std::fs::write(dir.join("manifest.json"), manifest.to_string()).expect("write manifest");
}

#[tokio::test]
async fn season_cache_lookup_uses_the_newest_file() {
    let server = TestServer::start_api_only().await;
    let storage = TempDir::new().unwrap();

    let show: Value = server
        .admin_client()
        .post_json(
            "/api/v1/admin/tv/shows",
            &json!({ "title": "Cache Show", "year": 2024 }),
        )
        .await
        .json();
    let show_id = show["id"].as_str().unwrap().to_string();
    let season: Value = server
        .admin_client()
        .post_json(
            &format!("/api/v1/admin/tv/shows/{show_id}/seasons"),
            &json!({ "season_identifier": "1", "season_sort_order": 1 }),
        )
        .await
        .json();
    let season_id = season["id"].as_str().unwrap().to_string();
    let ep: Value = server
        .admin_client()
        .post_json(
            &format!("/api/v1/admin/tv/seasons/{season_id}/episodes"),
            &json!({ "episode_identifier": "1", "episode_sort_order": 1, "title": "Ep 1" }),
        )
        .await
        .json();
    let episode_id: Uuid = ep["id"].as_str().unwrap().parse().unwrap();

    // A regrab adds a second file rather than replacing the first; both have a
    // usable cache, so "first hit wins" has two answers to choose from.
    let old_file = create_episode_file(&server.pool, episode_id, "old", 48).await;
    let new_file = create_episode_file(&server.pool, episode_id, "new", 1).await;
    write_manifest(storage.path(), old_file, 1000.0);
    write_manifest(storage.path(), new_file, 2000.0);

    let files = api::skip_detect::cache_detect::season_episode_files(
        &server.pool,
        &storage.path().to_string_lossy(),
        episode_id,
    )
    .await
    .expect("season episode files");

    assert_eq!(files.len(), 1);
    assert_eq!(
        files[0].file_id, new_file,
        "the cache read must follow the file that actually plays — the newest"
    );
    assert_eq!(files[0].duration_seconds, 2000.0);
}
