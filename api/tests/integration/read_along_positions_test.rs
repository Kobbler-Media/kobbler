use std::sync::Arc;

use crate::common::*;
use api::jobs::handlers::EbookExtractHandler;
use api::jobs::worker::JobHandler;
use api::repositories::cross_format_sync::{self, SyncPointInput};
use serde_json::{Value, json};
use tokio::sync::Semaphore;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

/// Six chapters, so spine steps run past /6/10 — where comparing CFIs as
/// strings starts putting chapter five before chapter one.
fn book_chapters() -> Vec<String> {
    (1..=6)
        .map(|n| {
            chapter_xhtml(
                &format!("Chapter {n}"),
                &[
                    &format!("Chapter {n} opens here."),
                    &format!("Chapter {n} goes on."),
                ],
            )
        })
        .collect()
}

async fn book(server: &TestServer) -> Uuid {
    let book_id = create_book(server, "Read Along").await;
    upload_epub(server, book_id, epub_with(&book_chapters())).await;
    book_id
}

async fn audio_file(server: &TestServer, book_id: Uuid, index: i32) -> Uuid {
    let id = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO audiobook_files (id, book_id, file_index, format, file_path, file_size, duration_seconds)
         VALUES ($1, $2, $3, 'MP3', $4, 1000, 3600)",
    )
    .bind(id)
    .bind(book_id)
    .bind(index)
    .bind(format!("/test/{index}.mp3"))
    .execute(&server.pool)
    .await
    .unwrap();
    id
}

async fn ebook_file(server: &TestServer, book_id: Uuid) -> Uuid {
    sqlx::query_scalar("SELECT id FROM book_files WHERE book_id = $1")
        .bind(book_id)
        .fetch_one(&server.pool)
        .await
        .unwrap()
}

async fn sync_points(server: &TestServer, book_id: Uuid, points: &[(Uuid, f64, &str)]) {
    let ebook = ebook_file(server, book_id).await;
    let inputs: Vec<SyncPointInput> = points
        .iter()
        .map(|&(audio_file_id, audio_seconds, cfi)| SyncPointInput {
            audio_file_id,
            audio_seconds,
            ebook_file_id: ebook,
            ebook_cfi: cfi.to_string(),
            confidence: 0.9,
        })
        .collect();
    cross_format_sync::insert_sync_points(&server.pool, book_id, &inputs)
        .await
        .unwrap();
}

async fn sync_position(server: &TestServer, book_id: Uuid, query: &str) -> Value {
    let resp = server
        .admin_client()
        .get(&format!(
            "/api/v1/admin/books/{book_id}/sync-position?{query}"
        ))
        .await;
    resp.assert_ok();
    resp.json()
}

#[tokio::test]
async fn extracted_text_is_located_by_real_epubcfi_in_the_file_readers_open() {
    let server = TestServer::start_api_only().await;
    let book_id = book(&server).await;

    EbookExtractHandler::new(
        server.pool.clone(),
        server.storage_root(),
        server.nats_publisher.clone(),
        server.search_backend_arc(),
        Arc::new(Semaphore::new(1)),
    )
    .execute(
        Uuid::now_v7(),
        json!({"book_id": book_id}),
        CancellationToken::new(),
    )
    .await
    .unwrap();

    let rows: Vec<(Uuid, String, String)> = sqlx::query_as(
        "SELECT book_file_id, cfi, text FROM ebook_segments s JOIN book_files f ON f.id = s.book_file_id
         WHERE f.book_id = $1 ORDER BY segment_index",
    )
    .bind(book_id)
    .fetch_all(&server.pool)
    .await
    .unwrap();
    let file = ebook_file(&server, book_id).await;
    assert!(
        rows.iter().all(|r| r.0 == file),
        "segments filed under another book file"
    );
    let cfi_of = |text: &str| {
        rows.iter()
            .find(|r| r.2.starts_with(text))
            .unwrap_or_else(|| panic!("no segment for {text:?}: {rows:?}"))
            .1
            .clone()
    };
    assert_eq!(cfi_of("Chapter 1 opens"), "epubcfi(/6/2!/4/4/1:0)");
    assert_eq!(cfi_of("Chapter 6 goes on"), "epubcfi(/6/12!/4/6/1:0)");
}

#[tokio::test]
async fn reading_position_finds_the_audio_spot_before_it_in_book_order() {
    let server = TestServer::start_api_only().await;
    let book_id = book(&server).await;
    let audio = audio_file(&server, book_id, 0).await;
    sync_points(
        &server,
        book_id,
        &[
            (audio, 10.0, "epubcfi(/6/2!/4/4/1:0)"),
            (audio, 500.0, "epubcfi(/6/10!/4/4/1:0)"),
        ],
    )
    .await;

    for (cfi, seconds) in [
        ("epubcfi(/6/12!/4/4/1:3)", 500.0),
        ("epubcfi(/6/12!/4/4,/1:0,/1:12)", 500.0),
        ("epubcfi(/6/6!/4/6/1:0)", 10.0),
        ("epubcfi(/6/10!/4/2/1:0)", 10.0),
    ] {
        let found = sync_position(
            &server,
            book_id,
            &format!("from=ebook&cfi={}", urlencoding::encode(cfi)),
        )
        .await;
        assert_eq!(found["seconds"], seconds, "{cfi}");
    }
}

#[tokio::test]
async fn audio_position_is_looked_up_in_the_file_being_played() {
    let server = TestServer::start_api_only().await;
    let book_id = book(&server).await;
    let first = audio_file(&server, book_id, 0).await;
    let second = audio_file(&server, book_id, 1).await;
    sync_points(
        &server,
        book_id,
        &[
            (first, 100.0, "epubcfi(/6/2!/4/4/1:0)"),
            (second, 50.0, "epubcfi(/6/8!/4/4/1:0)"),
        ],
    )
    .await;

    let found = sync_position(
        &server,
        book_id,
        &format!("from=audio&seconds=120&file_id={second}"),
    )
    .await;
    assert_eq!(found["cfi"], "epubcfi(/6/8!/4/4/1:0)");
}

#[tokio::test]
async fn read_along_data_places_each_sync_point_in_the_reader_pack() {
    let server = TestServer::start_api_only().await;
    let book_id = book(&server).await;
    let audio = audio_file(&server, book_id, 0).await;
    sync_points(
        &server,
        book_id,
        &[(audio, 500.0, "epubcfi(/6/10!/4/6/1:4)")],
    )
    .await;

    let data: Value = server
        .admin_client()
        .get(&format!("/api/v1/admin/books/{book_id}/read-along-data"))
        .await
        .json();
    assert_eq!(
        data["sync_points"][0]["position"],
        json!({"chapter": 4, "block": 2, "offset": 4})
    );
}

async fn pending_extractions(server: &TestServer, book_id: Uuid) -> i64 {
    sqlx::query_scalar(
        "SELECT COUNT(*) FROM job_status
         WHERE job_type = 'ebook_extract' AND status = 'pending' AND payload->>'book_id' = $1",
    )
    .bind(book_id.to_string())
    .fetch_one(&server.pool)
    .await
    .unwrap()
}

async fn segment(server: &TestServer, book_id: Uuid, cfi: &str) {
    let file = ebook_file(server, book_id).await;
    api::repositories::ebook_segments::insert_segments(
        &server.pool,
        file,
        &[api::repositories::ebook_segments::EbookSegmentInput {
            segment_index: 0,
            cfi: cfi.to_string(),
            text: "Chapter 1 opens here.".to_string(),
            chapter_title: None,
        }],
    )
    .await
    .unwrap();
}

/// Text extracted before segments carried real epubcfi is re-extracted once,
/// by the boot sweep, and never again.
#[tokio::test]
async fn the_boot_sweep_re_extracts_only_books_with_legacy_positions() {
    let server = TestServer::start_api_only().await;
    let legacy = book(&server).await;
    let current = book(&server).await;
    segment(&server, legacy, "/6/2!/4/2").await;
    segment(&server, current, "epubcfi(/6/2!/4/4/1:0)").await;

    api::jobs::handlers::ReaderPackHandler::new(
        server.pool.clone(),
        server.storage_root(),
        server.nats_publisher.clone(),
        Arc::new(Semaphore::new(1)),
    )
    .execute(Uuid::now_v7(), json!({}), CancellationToken::new())
    .await
    .unwrap();

    assert_eq!(pending_extractions(&server, legacy).await, 1);
    assert_eq!(pending_extractions(&server, current).await, 0);
}
