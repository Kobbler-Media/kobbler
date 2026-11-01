//! Kobload still holds wants for books that were never really wanted (the
//! discovered backlog sent before the want check learned to skip them). Its
//! events for those books must not pull them into the want flow, or the daily
//! want check sends them again.

use crate::common::*;
use api::repositories::books;
use reqwest::StatusCode;
use serde_json::json;
use uuid::Uuid;

async fn book(pool: &sqlx::PgPool, want_status: &str) -> Uuid {
    let id = Uuid::now_v7();
    sqlx::query("INSERT INTO books (id, title, want_status) VALUES ($1, $2, $3)")
        .bind(id)
        .bind(format!("Book {want_status}"))
        .bind(want_status)
        .execute(pool)
        .await
        .unwrap();
    id
}

async fn want_status(pool: &sqlx::PgPool, id: Uuid) -> String {
    sqlx::query_scalar("SELECT want_status FROM books WHERE id = $1")
        .bind(id)
        .fetch_one(pool)
        .await
        .unwrap()
}

async fn fail_download(server: &TestServer, book_id: Uuid) {
    let resp = server
        .api_client()
        .post_json(
            &format!("/api/v1/ingest/{}", server.ingest_token),
            &json!({
                "download_id": Uuid::now_v7(),
                "status": "failed",
                "metadata": { "book_id": book_id },
                "files": [],
                "error": "no seeders"
            }),
        )
        .await;
    assert_ne!(resp.status, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn a_failed_download_rearms_only_a_book_someone_wanted() {
    let server = TestServer::start_api_only().await;
    for status in ["searching", "downloading", "watching", "search_failed"] {
        let id = book(&server.pool, status).await;
        fail_download(&server, id).await;
        assert_eq!(
            want_status(&server.pool, id).await,
            "wanted",
            "from {status}"
        );
    }
    for status in ["pending", "skipped", "none", "acquired"] {
        let id = book(&server.pool, status).await;
        fail_download(&server, id).await;
        assert_eq!(want_status(&server.pool, id).await, status);
    }
}

#[tokio::test]
async fn kobload_events_are_ignored_for_a_book_nobody_wanted() {
    let server = TestServer::start_api_only().await;
    for status in [
        "wanted",
        "searching",
        "downloading",
        "watching",
        "search_failed",
    ] {
        let id = book(&server.pool, status).await;
        assert!(
            books::is_wanted_book(&server.pool, id).await.unwrap(),
            "{status}"
        );
    }
    for status in ["pending", "skipped", "none", "acquired"] {
        let id = book(&server.pool, status).await;
        assert!(
            !books::is_wanted_book(&server.pool, id).await.unwrap(),
            "{status}"
        );
    }
}
