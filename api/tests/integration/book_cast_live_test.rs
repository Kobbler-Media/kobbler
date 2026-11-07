//! Runs every cast pass on a real epub against a real koblem and writes what
//! came back to a file, for judging a cast by eye. Ignored; drive it with
//! CAST_EPUB, CAST_KOBLEM_URL, CAST_KOBLEM_KEY, CAST_OUT and CAST_CHAPTERS, or
//! CAST_DB to resume a cast kept from an earlier run (rename its database out
//! of `test_%` first, or the next run's stale-database sweep drops it).

use crate::common::*;
use api::book_cast::{bible, quotes};
use api::jobs::handlers::{
    BookCastBibleHandler, BookCastChaptersHandler, BookCastVoicesHandler, EbookExtractHandler,
};
use api::jobs::worker::JobHandler;
use api::repositories::{book_casts, ebook_segments};
use serde_json::{Value, json};
use sqlx::PgPool;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Instant;
use tokio::sync::Semaphore;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

fn env(name: &str) -> String {
    std::env::var(name).unwrap_or_else(|_| panic!("{name} is not set"))
}

async fn fresh_cast(server: &TestServer) -> Uuid {
    let book_id = create_book(server, "Live Cast").await;
    upload_epub(server, book_id, std::fs::read(env("CAST_EPUB")).unwrap()).await;
    api::jobs::enqueue_ebook_extract(&server.pool, &server.nats_publisher, book_id, "Live Cast")
        .await;
    server
        .process_pending_jobs(&EbookExtractHandler::new(
            server.pool.clone(),
            server.storage_root(),
            server.nats_publisher.clone(),
            server.search_backend_arc(),
            Arc::new(Semaphore::new(1)),
        ))
        .await;

    if let Ok(keep) = std::env::var("CAST_CHAPTERS") {
        let keep: i64 = keep.parse().unwrap();
        let removed = sqlx::query(
            "WITH runs AS (
                SELECT segment_index, chapter_title IS DISTINCT FROM
                    lag(chapter_title) OVER (ORDER BY segment_index) AS starts
                FROM ebook_segments s JOIN book_files f ON f.id = s.book_file_id
                WHERE f.book_id = $1),
             numbered AS (
                SELECT segment_index, count(*) FILTER (WHERE starts)
                    OVER (ORDER BY segment_index) AS chapter FROM runs)
             DELETE FROM ebook_segments s USING book_files f, numbered n
             WHERE f.id = s.book_file_id AND f.book_id = $1
               AND n.segment_index = s.segment_index AND n.chapter > $2",
        )
        .bind(book_id)
        .bind(keep)
        .execute(&server.pool)
        .await
        .unwrap()
        .rows_affected();
        eprintln!("kept {keep} chapters, dropped {removed} paragraphs");
    }

    server
        .admin_client()
        .post_json(&format!("/api/v1/admin/books/{book_id}/cast"), &json!({}))
        .await
        .assert_success();
    book_id
}

async fn kept_cast(name: &str) -> (PgPool, Uuid) {
    let base = env("DATABASE_URL_TEST");
    let url = format!("{}/{name}", &base[..base.rfind('/').unwrap()]);
    let pool = PgPool::connect(&url).await.unwrap();
    let book_id = sqlx::query_scalar("SELECT book_id FROM book_casts LIMIT 1")
        .fetch_one(&pool)
        .await
        .unwrap();
    (pool, book_id)
}

#[tokio::test]
#[ignore]
async fn live_cast_of_a_real_book() {
    let server = TestServer::start_api_only().await;
    server
        .admin_client()
        .patch_json(
            "/api/v1/admin/config",
            &json!({"koblem_service_url": env("CAST_KOBLEM_URL"), "koblem_api_key": env("CAST_KOBLEM_KEY")}),
        )
        .await
        .assert_ok();
    let (pool, book_id) = match std::env::var("CAST_DB") {
        Ok(name) => kept_cast(&name).await,
        Err(_) => (server.pool.clone(), fresh_cast(&server).await),
    };
    let payload = json!({"book_id": book_id});

    let began = Instant::now();
    let result = BookCastChaptersHandler::new(
        pool.clone(),
        server.runtime_config.clone(),
        server.nats_publisher.clone(),
    )
    .execute(Uuid::now_v7(), payload.clone(), CancellationToken::new())
    .await;
    eprintln!("chapter pass: {:?} {result:?}", began.elapsed());

    let notes = book_casts::chapters(&pool, book_id).await.unwrap();
    std::fs::write(
        format!("{}.notes.json", env("CAST_OUT")),
        serde_json::to_string_pretty(&notes).unwrap(),
    )
    .unwrap();
    let lines = book_casts::lines(&pool, book_id, None).await.unwrap();
    let (prompt, _) = bible::prompt(&notes, &lines, &HashMap::new());
    eprintln!("cast prompt: {} chars", prompt.len());

    let began = Instant::now();
    let result = BookCastBibleHandler::new(
        pool.clone(),
        server.runtime_config.clone(),
        server.nats_publisher.clone(),
    )
    .execute(Uuid::now_v7(), payload.clone(), CancellationToken::new())
    .await;
    eprintln!("bible pass: {:?} {result:?}", began.elapsed());

    let began = Instant::now();
    let result = BookCastVoicesHandler::new(
        pool.clone(),
        server.runtime_config.clone(),
        server.storage_root(),
    )
    .execute(Uuid::now_v7(), payload, CancellationToken::new())
    .await;
    eprintln!("voices pass: {:?} {result:?}", began.elapsed());

    let cast = book_casts::get(&pool, book_id).await.unwrap().unwrap();
    let texts: HashMap<i32, String> = ebook_segments::get_for_file(&pool, cast.book_file_id)
        .await
        .unwrap()
        .into_iter()
        .map(|s| (s.segment_index, s.text))
        .collect();
    let lines: Vec<Value> = book_casts::lines(&pool, book_id, None)
        .await
        .unwrap()
        .into_iter()
        .map(|l| {
            let text = texts
                .get(&l.segment_index)
                .and_then(|t| t.get(l.span_start as usize..l.span_end as usize))
                .map(quotes::spoken);
            let mut v = serde_json::to_value(&l).unwrap();
            v["text"] = json!(text);
            v
        })
        .collect();
    let characters = api::repositories::book_cast_vetting::characters(&pool, book_id)
        .await
        .unwrap();
    std::fs::write(
        env("CAST_OUT"),
        serde_json::to_string_pretty(&json!({
            "cast": cast, "characters": characters, "lines": lines,
        }))
        .unwrap(),
    )
    .unwrap();
    eprintln!(
        "state {:?} error {:?} characters {}",
        cast.state,
        cast.error,
        characters.len()
    );
}
