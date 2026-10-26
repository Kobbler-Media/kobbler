use crate::common::audiobooks::*;
use crate::common::*;
use api::jobs::handlers::{AudiobookChapterRepairHandler, AudiobookProbeHandler};
use api::jobs::worker::JobHandler;
use api::repositories::media_root_files::{self, MediaRootFileInput};
use api::watcher::{FileMode, IngestOptions, process_queue_item};
use reqwest::StatusCode;
use serde_json::{Value, json};
use std::sync::Arc;
use tempfile::TempDir;
use tokio::sync::Semaphore;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

const TITLE: &str = "Harry Potter and the Philosopher's Stone";
const AUTHOR: &str = "J.K. Rowling";

const CAST: [Span; 4] = [
    ("Chapter 1", 0.0, 2.0),
    ("Chapter 2", 2.0, 4.0),
    ("Chapter 3", 4.0, 6.0),
    ("Chapter 4", 6.0, 8.0),
];

const WORDS: [&str; 48] = [
    "harry",
    "owl",
    "wand",
    "castle",
    "letter",
    "cupboard",
    "stairs",
    "dragon",
    "potion",
    "broom",
    "cloak",
    "mirror",
    "troll",
    "forest",
    "train",
    "platform",
    "giant",
    "uncle",
    "aunt",
    "cousin",
    "snake",
    "zoo",
    "scar",
    "lightning",
    "feast",
    "hall",
    "ghost",
    "quidditch",
    "snitch",
    "keeper",
    "stone",
    "philosopher",
    "flute",
    "dog",
    "door",
    "chess",
    "key",
    "wing",
    "fire",
    "ice",
    "night",
    "moon",
    "star",
    "candle",
    "book",
    "library",
    "rule",
    "point",
];

fn prose(chapter: u32) -> Vec<&'static str> {
    let mut seed = 0x9e37_79b9_u64.wrapping_mul(u64::from(chapter) + 1);
    (0..600)
        .map(|_| {
            seed = seed
                .wrapping_mul(6_364_136_223_846_793_005)
                .wrapping_add(1_442_695_040_888_963_407);
            WORDS[(seed >> 33) as usize % WORDS.len()]
        })
        .collect()
}

#[derive(Clone, Copy)]
enum Reading {
    Full,
    Retold,
    Abridged,
}

async fn narrate(server: &TestServer, file_id: Uuid, chapters: &[u32], seconds: f64, how: Reading) {
    let mut words: Vec<&str> = Vec::new();
    for chapter in chapters {
        let text = prose(*chapter);
        let text = match how {
            Reading::Abridged => text[..text.len() / 2].to_vec(),
            _ => text,
        };
        words.extend(
            text.into_iter()
                .enumerate()
                .filter(|(i, _)| !matches!(how, Reading::Retold) || i % 17 != 16)
                .map(|(_, w)| w),
        );
    }
    let lines: Vec<String> = words.chunks(12).map(|c| c.join(" ")).collect();
    let step = seconds / lines.len() as f64;
    let timed: Vec<(f64, &str)> = lines
        .iter()
        .enumerate()
        .map(|(i, l)| (i as f64 * step, l.as_str()))
        .collect();
    add_transcript(server, file_id, &timed).await;
}

fn repairer(server: &TestServer) -> AudiobookChapterRepairHandler {
    AudiobookChapterRepairHandler::new(
        server.pool.clone(),
        server.storage_root(),
        server.nats_publisher.clone(),
        server.search_backend_arc(),
    )
}

fn prober(server: &TestServer) -> AudiobookProbeHandler {
    AudiobookProbeHandler::new(
        server.pool.clone(),
        server.storage_root(),
        Arc::new(Semaphore::new(1)),
        server.runtime_config.clone(),
        server.nats_publisher.clone(),
    )
}

async fn repair(server: &TestServer) {
    server
        .admin_client()
        .post("/api/v1/admin/books/repair-audiobook-chapters")
        .await
        .assert_status(StatusCode::ACCEPTED);
    let handler = repairer(server);
    for _ in 0..3 {
        server.process_pending_jobs(&handler).await;
    }
}

async fn probe_pending(server: &TestServer) {
    let handler = prober(server);
    for _ in 0..3 {
        server.process_pending_jobs(&handler).await;
    }
}

async fn sweep(server: &TestServer, dry_run: bool) -> Value {
    repairer(server)
        .execute(
            Uuid::now_v7(),
            json!({ "dry_run": dry_run }),
            CancellationToken::new(),
        )
        .await
        .unwrap()
}

async fn audio_files(server: &TestServer, book_id: Uuid) -> Vec<String> {
    let resp = server
        .admin_client()
        .get(&format!("/api/v1/admin/books/{book_id}/audio-files"))
        .await;
    resp.assert_ok();
    let body: Value = resp.json();
    body.as_array()
        .unwrap()
        .iter()
        .map(|f| f["file_name"].as_str().unwrap().to_string())
        .collect()
}

async fn file_paths(server: &TestServer, book_id: Uuid) -> Vec<String> {
    sqlx::query_scalar(
        "SELECT file_path FROM audiobook_files WHERE book_id = $1 ORDER BY file_index",
    )
    .bind(book_id)
    .fetch_all(&server.pool)
    .await
    .unwrap()
}

fn names(paths: &[String]) -> Vec<String> {
    paths
        .iter()
        .map(|p| {
            let path = std::path::Path::new(p);
            let dir = path
                .parent()
                .and_then(|d| d.file_name())
                .map(|d| d.to_string_lossy().to_string())
                .unwrap_or_default();
            format!("{dir}/{}", path.file_name().unwrap().to_string_lossy())
        })
        .collect()
}

async fn editions_of(server: &TestServer, title: &str) -> Vec<Uuid> {
    sqlx::query_scalar(
        "SELECT b.id FROM books b
         WHERE b.title = $1 AND EXISTS (SELECT 1 FROM audiobook_files af WHERE af.book_id = b.id)
         ORDER BY b.created_at, b.id",
    )
    .bind(title)
    .fetch_all(&server.pool)
    .await
    .unwrap()
}

async fn other_edition(server: &TestServer, book_id: Uuid) -> Uuid {
    let books = editions_of(server, TITLE).await;
    assert_eq!(books.len(), 2, "expected two editions, got {books:?}");
    *books.iter().find(|b| **b != book_id).unwrap()
}

async fn listen(client: &TestClient, book_id: Uuid, file_id: Uuid, at: f64) {
    client
        .put_json(
            &format!("/api/v1/admin/books/{book_id}/audio-position"),
            &json!({"file_id": file_id, "position_seconds": at, "progress_percent": 10.0}),
        )
        .await
        .assert_success();
}

async fn position(client: &TestClient, book_id: Uuid) -> (Option<String>, Option<f64>) {
    let resp = client
        .get(&format!("/api/v1/admin/books/{book_id}/audio-position"))
        .await;
    resp.assert_ok();
    let body: Value = resp.json();
    (
        body["file_id"].as_str().map(String::from),
        body["position_seconds"].as_f64(),
    )
}

async fn authors_of(server: &TestServer, book_id: Uuid) -> Vec<String> {
    sqlx::query_scalar(
        "SELECT a.name FROM book_authors ba JOIN authors a ON a.id = ba.author_id
         WHERE ba.book_id = $1 ORDER BY a.name",
    )
    .bind(book_id)
    .fetch_all(&server.pool)
    .await
    .unwrap()
}

async fn narrated_and_full_cast(server: &TestServer) -> (Uuid, Vec<Uuid>) {
    let book_id = create_book_with_author(server, TITLE, AUTHOR).await;
    let files = seed_files(
        server,
        book_id,
        &[
            file("fry/Philosopher (1).mp3", 4000.0, Audio::Tone(2.0)).by("Stephen Fry"),
            file("fry/Philosopher (2).mp3", 4000.0, Audio::Tone(2.0)).by("Stephen Fry"),
            file(
                "cast/Philosopher (Full-Cast Edition).m4b",
                8200.0,
                Audio::Chaptered(8.0, &CAST),
            )
            .by("Full Cast"),
        ],
    )
    .await;
    narrate(server, files[0], &[1, 2], 4000.0, Reading::Full).await;
    narrate(server, files[1], &[3, 4], 4000.0, Reading::Full).await;
    narrate(server, files[2], &[1, 2, 3, 4], 8200.0, Reading::Retold).await;
    (book_id, files)
}

#[tokio::test]
async fn repair_files_a_second_edition_as_its_own_book() {
    let server = TestServer::start_api_only().await;
    let (book_id, _) = narrated_and_full_cast(&server).await;

    repair(&server).await;

    assert_eq!(
        names(&file_paths(&server, book_id).await),
        vec!["cast/Philosopher (Full-Cast Edition).m4b"]
    );
    let other = other_edition(&server, book_id).await;
    assert_eq!(
        names(&file_paths(&server, other).await),
        vec!["fry/Philosopher (1).mp3", "fry/Philosopher (2).mp3"]
    );
    assert_eq!(authors_of(&server, other).await, vec![AUTHOR]);
    let narrator: Option<String> = sqlx::query_scalar("SELECT narrator FROM books WHERE id = $1")
        .bind(other)
        .fetch_one(&server.pool)
        .await
        .unwrap();
    assert_eq!(narrator.as_deref(), Some("Stephen Fry"));

    assert_eq!(
        audio_files(&server, book_id).await,
        vec!["Philosopher (Full-Cast Edition).m4b"]
    );
    let chapters = unified(&server, book_id).await;
    assert_eq!(
        titles(&chapters),
        CAST.iter().map(|(t, _, _)| *t).collect::<Vec<_>>()
    );
    assert!(
        chapters.iter().all(|c| c.source == "embedded"),
        "{chapters:?}"
    );
    assert!(chapters.last().unwrap().end <= 8200.0);

    let durations: Vec<Option<i32>> = sqlx::query_scalar(
        "SELECT duration_seconds FROM books WHERE id = ANY($1) ORDER BY created_at",
    )
    .bind(vec![book_id, other])
    .fetch_all(&server.pool)
    .await
    .unwrap();
    assert_eq!(durations, vec![Some(8200), Some(8000)]);

    let segments: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM audiobook_transcripts t JOIN audiobook_files af ON af.id = t.audiobook_file_id
         WHERE af.book_id = $1 AND af.format <> 'M4B'",
    )
    .bind(book_id)
    .fetch_one(&server.pool)
    .await
    .unwrap();
    assert_eq!(segments, 0, "the narrated transcript left with its files");

    let again = sweep(&server, true).await;
    assert_eq!(again["books_to_repair"], 0, "{again}");
}

#[tokio::test]
async fn listeners_stay_on_the_edition_they_were_playing() {
    let server = TestServer::start_api_only().await;
    let (book_id, files) = narrated_and_full_cast(&server).await;
    let fry_a = create_user_client(&server, "fry-a@test.com", "viewer").await;
    let fry_b = create_user_client(&server, "fry-b@test.com", "viewer").await;
    let cast = create_user_client(&server, "cast@test.com", "viewer").await;
    listen(&fry_a, book_id, files[1], 100.0).await;
    listen(&fry_b, book_id, files[0], 50.0).await;
    listen(&cast, book_id, files[2], 500.0).await;

    repair(&server).await;

    assert_eq!(
        names(&file_paths(&server, book_id).await),
        vec!["fry/Philosopher (1).mp3", "fry/Philosopher (2).mp3"],
        "the edition people are listening to keeps the book"
    );
    assert_eq!(
        position(&fry_a, book_id).await,
        (Some(files[1].to_string()), Some(100.0))
    );
    assert_eq!(
        position(&fry_b, book_id).await,
        (Some(files[0].to_string()), Some(50.0))
    );
    let other = other_edition(&server, book_id).await;
    assert_eq!(
        position(&cast, other).await,
        (Some(files[2].to_string()), Some(500.0))
    );
    assert_eq!(position(&cast, book_id).await, (None, None));
}

#[tokio::test]
async fn a_copy_of_one_part_from_another_release_is_dropped() {
    let server = TestServer::start_api_only().await;
    let book_id = create_book_with_author(&server, TITLE, AUTHOR).await;
    let files = seed_files(
        &server,
        book_id,
        &[
            file("LS2/part 1 of 2.mp3", 20861.97, Audio::Tone(2.0))
                .by("Chris Colfer")
                .tag("album", "Book 2"),
            file("LS2/part 2 of 2.mp3", 19338.65, Audio::Tone(2.0))
                .by("Chris Colfer")
                .tag("album", "Book 2"),
            file("LS3/part 2 of 2.mp3", 19338.65, Audio::Tone(2.0))
                .by("Chris Colfer")
                .tag("album", "Book 3"),
        ],
    )
    .await;
    let copy: String = file_paths(&server, book_id).await.remove(2);
    let root = sqlx::query_scalar::<_, Uuid>(
        "INSERT INTO media_roots (id, name, path, media_type) VALUES ($1, 'Audiobooks', $2, 'audiobook') RETURNING id",
    )
    .bind(Uuid::now_v7())
    .bind(server.storage_root())
    .fetch_one(&server.pool)
    .await
    .unwrap();
    let (copy_mrf, _) = media_root_files::upsert_by_path(
        &server.pool,
        MediaRootFileInput {
            media_root_id: root,
            file_path: copy.clone(),
            file_name: "part 2 of 2.mp3".into(),
            file_size: Some(1000),
            file_mtime: None,
            file_fingerprint: None,
        },
    )
    .await
    .unwrap();
    media_root_files::set_added(&server.pool, copy_mrf, "book", book_id)
        .await
        .unwrap();
    let reader = create_user_client(&server, "reader@test.com", "viewer").await;
    listen(&reader, book_id, files[2], 1234.0).await;

    repair(&server).await;

    assert_eq!(
        names(&file_paths(&server, book_id).await),
        vec!["LS2/part 1 of 2.mp3", "LS2/part 2 of 2.mp3"]
    );
    assert_eq!(editions_of(&server, TITLE).await, vec![book_id]);
    let mrf = media_root_files::get_by_id(&server.pool, copy_mrf)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(mrf.match_status, "ignored", "a rescan must not add it back");
    assert_eq!(
        position(&reader, book_id).await,
        (Some(files[1].to_string()), Some(1234.0))
    );
    assert!(
        std::path::Path::new(&copy).is_file(),
        "the copy stays on disk"
    );
}

async fn fry_and_dale(
    server: &TestServer,
    dale: &[FileSpec<'_>],
    how: Reading,
) -> (Uuid, Vec<Uuid>) {
    let book_id = create_book_with_author(server, TITLE, AUTHOR).await;
    let specs: Vec<FileSpec> = ["fry/(1).mp3", "fry/(2).mp3", "fry/(3).mp3", "fry/(4).mp3"]
        .into_iter()
        .map(|name| file(name, 4000.0, Audio::Tone(2.0)).by("Stephen Fry"))
        .collect();
    let mut ids = seed_files(server, book_id, &specs).await;
    for (i, id) in ids.iter().enumerate() {
        narrate(server, *id, &[i as u32 + 1], 4000.0, Reading::Full).await;
    }
    let dale_ids = seed_files(server, book_id, dale).await;
    for (i, id) in dale_ids.iter().enumerate() {
        let chapters: Vec<u32> = match how {
            Reading::Abridged => vec![i as u32 * 2 + 1, i as u32 * 2 + 2],
            _ => vec![i as u32 + 3],
        };
        narrate(server, *id, &chapters, dale[i].duration, how).await;
    }
    ids.extend(dale_ids);
    (book_id, ids)
}

#[tokio::test]
async fn a_partial_second_edition_is_dropped_not_split() {
    let server = TestServer::start_api_only().await;
    let (book_id, _) = fry_and_dale(
        &server,
        &[
            file("dale/(5).mp3", 2600.0, Audio::Tone(2.0)).by("J. K. Rowling"),
            file("dale/(6).mp3", 2600.0, Audio::Tone(2.0)).by("J. K. Rowling"),
        ],
        Reading::Retold,
    )
    .await;

    repair(&server).await;

    assert_eq!(
        names(&file_paths(&server, book_id).await),
        vec!["fry/(1).mp3", "fry/(2).mp3", "fry/(3).mp3", "fry/(4).mp3"]
    );
    assert_eq!(editions_of(&server, TITLE).await, vec![book_id]);
}

#[tokio::test]
async fn an_abridged_edition_is_split_and_marked_abridged() {
    let server = TestServer::start_api_only().await;
    let (book_id, _) = fry_and_dale(
        &server,
        &[
            file("dale/(5).mp3", 3600.0, Audio::Tone(2.0))
                .by("J. K. Rowling")
                .tag("comment", "Abridged"),
            file("dale/(6).mp3", 3600.0, Audio::Tone(2.0))
                .by("J. K. Rowling")
                .tag("comment", "Abridged"),
        ],
        Reading::Abridged,
    )
    .await;

    repair(&server).await;

    assert_eq!(file_paths(&server, book_id).await.len(), 4);
    let other = other_edition(&server, book_id).await;
    assert_eq!(
        names(&file_paths(&server, other).await),
        vec!["dale/(5).mp3", "dale/(6).mp3"]
    );
    let abridged: bool = sqlx::query_scalar("SELECT is_abridged FROM books WHERE id = $1")
        .bind(other)
        .fetch_one(&server.pool)
        .await
        .unwrap();
    assert!(abridged);
}

#[tokio::test]
async fn one_release_spread_over_folders_or_tags_stays_together() {
    let server = TestServer::start_api_only().await;
    let cases: Vec<(&str, Vec<FileSpec>)> = vec![
        (
            "disc folders",
            vec![
                file("CD1/01.mp3", 3000.0, Audio::Tone(2.0)).by("Jim Dale"),
                file("CD2/01.mp3", 3000.0, Audio::Tone(2.0)).by("Jim Dale"),
            ],
        ),
        (
            "parts tagged differently",
            vec![
                file("part1.mp3", 3000.0, Audio::Tone(2.0)).by("Jim Dale"),
                file("part2.m4b", 3000.0, Audio::Tone(2.0)).by("J.K. Rowling"),
            ],
        ),
        (
            "same length parts of one release",
            vec![
                file("a/01.mp3", 600.0, Audio::Tone(2.0)).by("Jim Dale"),
                file("b/02.mp3", 600.0, Audio::Tone(2.0)).by("Jim Dale"),
            ],
        ),
    ];
    let mut books = Vec::new();
    for (case, specs) in &cases {
        let book_id = create_book_with_author(&server, case, AUTHOR).await;
        let ids = seed_files(&server, book_id, specs).await;
        if *case == "parts tagged differently" {
            narrate(&server, ids[0], &[1], 3000.0, Reading::Full).await;
            narrate(&server, ids[1], &[2], 3000.0, Reading::Full).await;
        }
        books.push((case, book_id, file_paths(&server, book_id).await));
    }

    repair(&server).await;

    for (case, book_id, before) in books {
        assert_eq!(file_paths(&server, book_id).await, before, "{case}");
        assert_eq!(editions_of(&server, case).await, vec![book_id], "{case}");
    }
}

#[tokio::test]
async fn a_dry_run_reports_editions_and_changes_nothing() {
    let server = TestServer::start_api_only().await;
    let (book_id, _) = narrated_and_full_cast(&server).await;
    let before = file_paths(&server, book_id).await;

    let report = sweep(&server, true).await;

    let finding = report["findings"]
        .as_array()
        .unwrap()
        .iter()
        .find(|f| f["book_id"] == book_id.to_string())
        .unwrap_or_else(|| panic!("no finding for the book: {report}"));
    let editions = finding["editions"].as_array().expect("editions in finding");
    assert_eq!(editions.len(), 1, "{finding}");
    assert_eq!(editions[0]["action"], "split");
    assert_eq!(editions[0]["files"].as_array().unwrap().len(), 2);
    assert_eq!(file_paths(&server, book_id).await, before);
    assert_eq!(editions_of(&server, TITLE).await, vec![book_id]);
    let jobs: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM job_status WHERE job_type = 'audiobook_chapter_repair' AND status = 'pending'",
    )
    .fetch_one(&server.pool)
    .await
    .unwrap();
    assert_eq!(jobs, 0);
}

async fn review_file(server: &TestServer, root: Uuid, path: &std::path::Path) -> Uuid {
    let (id, _) = media_root_files::upsert_by_path(
        &server.pool,
        MediaRootFileInput {
            media_root_id: root,
            file_path: path.to_string_lossy().to_string(),
            file_name: path.file_name().unwrap().to_string_lossy().to_string(),
            file_size: Some(1000),
            file_mtime: None,
            file_fingerprint: None,
        },
    )
    .await
    .unwrap();
    media_root_files::set_review(
        &server.pool,
        id,
        &json!([{
            "provider": "hardcover",
            "provider_id": "hc-ps",
            "title": TITLE,
            "authors": [AUTHOR],
            "confidence": 0.95
        }]),
    )
    .await
    .unwrap();
    id
}

async fn audiobook_root(server: &TestServer) -> (Uuid, TempDir) {
    let dir = TempDir::new().unwrap();
    let resp = server
        .admin_client()
        .post_json(
            "/api/v1/admin/media-roots",
            &json!({
                "path": dir.path().to_string_lossy(),
                "name": "Audiobooks",
                "media_type": "audiobook",
                "auto_add": false,
            }),
        )
        .await;
    resp.assert_status(StatusCode::CREATED);
    let id = resp.json::<Value>()["id"]
        .as_str()
        .unwrap()
        .parse()
        .unwrap();
    (id, dir)
}

fn write_into(dir: &std::path::Path, spec: &FileSpec<'_>) -> std::path::PathBuf {
    let path = dir.join(spec.name);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    write_audio(&path, &spec.audio, &spec.tags);
    path
}

#[tokio::test]
async fn importing_a_second_edition_files_it_as_its_own_book() {
    let server = TestServer::start_api_only().await;
    let book_id = create_book_with_author(&server, TITLE, AUTHOR).await;
    seed_files(
        &server,
        book_id,
        &[
            file("fry/(1).mp3", 600.0, Audio::Tone(600.0)).by("Stephen Fry"),
            file("fry/(2).mp3", 600.0, Audio::Tone(600.0)).by("Stephen Fry"),
        ],
    )
    .await;
    probe(&server, book_id).await;
    assert_eq!(file_paths(&server, book_id).await.len(), 2);

    let (root, dir) = audiobook_root(&server).await;
    let cast = write_into(
        dir.path(),
        &file("Full Cast/Philosopher.m4b", 1200.0, Audio::Tone(1200.0)).by("Full Cast"),
    );
    let mrf = review_file(&server, root, &cast).await;
    server
        .admin_client()
        .post(&format!("/api/v1/admin/media-roots/files/{mrf}/add"))
        .await
        .assert_ok();
    assert_eq!(
        file_paths(&server, book_id).await.len(),
        3,
        "appended first"
    );

    probe_pending(&server).await;

    let books = editions_of(&server, TITLE).await;
    assert_eq!(books.len(), 2, "{books:?}");
    let mut by_book = Vec::new();
    for b in &books {
        by_book.push(names(&file_paths(&server, *b).await));
    }
    assert!(
        by_book.contains(&vec!["Full Cast/Philosopher.m4b".to_string()]),
        "{by_book:?}"
    );
    assert!(
        by_book.contains(&vec!["fry/(1).mp3".to_string(), "fry/(2).mp3".to_string()]),
        "{by_book:?}"
    );

    let more_fry = write_into(
        dir.path(),
        &file("fry/(3).mp3", 600.0, Audio::Tone(600.0)).by("Stephen Fry"),
    );
    let mrf = review_file(&server, root, &more_fry).await;
    server
        .admin_client()
        .post(&format!("/api/v1/admin/media-roots/files/{mrf}/add"))
        .await
        .assert_ok();
    probe_pending(&server).await;

    let mut by_book = Vec::new();
    for b in editions_of(&server, TITLE).await {
        by_book.push(names(&file_paths(&server, b).await));
    }
    assert!(
        by_book.contains(&vec!["Full Cast/Philosopher.m4b".to_string()]),
        "{by_book:?}"
    );
    assert!(
        by_book.contains(&vec![
            "fry/(1).mp3".to_string(),
            "fry/(2).mp3".to_string(),
            "fry/(3).mp3".to_string()
        ]),
        "a later file of the same release joins its edition: {by_book:?}"
    );
}

#[tokio::test]
async fn auto_ingesting_a_second_edition_into_an_existing_book_files_it_separately() {
    let server = TestServer::start_api_only().await;
    let book_id = create_book_with_author(&server, TITLE, AUTHOR).await;
    seed_files(
        &server,
        book_id,
        &[
            file("fry/(1).mp3", 600.0, Audio::Tone(600.0)).by("Stephen Fry"),
            file("fry/(2).mp3", 600.0, Audio::Tone(600.0)).by("Stephen Fry"),
        ],
    )
    .await;
    probe(&server, book_id).await;

    let watch = TempDir::new().unwrap();
    let cast = write_into(
        watch.path(),
        &file("Philosopher.m4b", 1200.0, Audio::Tone(1200.0)).by("Full Cast"),
    );
    let item = add_queue_item(&server, cast.to_str().unwrap(), "Philosopher.m4b", "M4B").await;
    sqlx::query("UPDATE book_ingest_source SET update_target_book_id = $2 WHERE book_id = $1")
        .bind(item)
        .bind(book_id)
        .execute(&server.pool)
        .await
        .unwrap();
    process_queue_item(
        &server.pool,
        &server.storage_root(),
        &server.library_events,
        &server.nats_publisher,
        &server.runtime_config,
        item,
        IngestOptions::auto(FileMode::Copy),
    )
    .await
    .unwrap();

    probe_pending(&server).await;

    let mut by_book = Vec::new();
    for b in editions_of(&server, TITLE).await {
        by_book.push(
            file_paths(&server, b)
                .await
                .iter()
                .map(|p| {
                    std::path::Path::new(p)
                        .file_name()
                        .unwrap()
                        .to_string_lossy()
                        .to_string()
                })
                .collect::<Vec<_>>(),
        );
    }
    by_book.sort();
    assert_eq!(
        by_book,
        vec![
            vec!["(1).mp3".to_string(), "(2).mp3".to_string()],
            vec!["Philosopher.m4b".to_string()],
        ]
    );
}

async fn probe(server: &TestServer, book_id: Uuid) {
    prober(server)
        .execute(
            Uuid::now_v7(),
            json!({ "book_id": book_id }),
            CancellationToken::new(),
        )
        .await
        .unwrap();
}
