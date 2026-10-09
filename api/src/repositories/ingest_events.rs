//! Ingest events repository operations.

use super::Result;
use sqlx::{Executor, Postgres};
use time::OffsetDateTime;
use uuid::Uuid;

/// Create an ingest event.
pub async fn create<'e, E>(
    exec: E,
    source: &str,
    book_id: Option<Uuid>,
    status: &str,
    message: Option<&str>,
    device_id: Option<Uuid>,
) -> Result<Uuid>
where
    E: Executor<'e, Database = Postgres>,
{
    let id = Uuid::now_v7();
    sqlx::query!(
        "INSERT INTO ingest_events (id, source, book_id, status, message, device_id) VALUES ($1, $2, $3, $4, $5, $6)",
        id,
        source,
        book_id,
        status,
        message,
        device_id,
    )
    .execute(exec)
    .await?;
    Ok(id)
}

/// Ingest event row for listing.
#[derive(Debug)]
pub struct IngestEventRow {
    pub id: Uuid,
    pub source: String,
    pub book_id: Option<Uuid>,
    pub status: String,
    pub message: Option<String>,
    pub created_at: OffsetDateTime,
    pub book_title: Option<String>,
    pub device_id: Option<Uuid>,
    pub device_name: Option<String>,
}

/// Count total ingest events (for pagination total).
pub async fn count<'e, E>(exec: E) -> Result<i64>
where
    E: Executor<'e, Database = Postgres>,
{
    let count = sqlx::query_scalar!(r#"SELECT COUNT(*) as "count!" FROM ingest_events"#)
        .fetch_one(exec)
        .await?;
    Ok(count)
}

/// List ingest events with optional cursor pagination.
/// Returns events ordered by created_at DESC.
pub async fn list<'e, E>(
    exec: E,
    cursor_ts: Option<OffsetDateTime>,
    cursor_id: Option<Uuid>,
    limit: i64,
) -> Result<Vec<IngestEventRow>>
where
    E: Executor<'e, Database = Postgres>,
{
    let rows = if let (Some(ts), Some(id)) = (cursor_ts, cursor_id) {
        sqlx::query!(
            r#"
            SELECT ie.id, ie.source, ie.book_id, ie.status, ie.message, ie.created_at,
                   b.title as "book_title?", ie.device_id, d.name as "device_name?"
            FROM ingest_events ie
            LEFT JOIN books b ON b.id = ie.book_id
            LEFT JOIN devices d ON d.id = ie.device_id
            WHERE (ie.created_at, ie.id) < ($1, $2)
            ORDER BY ie.created_at DESC, ie.id DESC
            LIMIT $3
            "#,
            ts,
            id,
            limit,
        )
        .fetch_all(exec)
        .await?
        .into_iter()
        .map(|r| IngestEventRow {
            id: r.id,
            source: r.source,
            book_id: r.book_id,
            status: r.status,
            message: r.message,
            created_at: r.created_at,
            book_title: r.book_title,
            device_id: r.device_id,
            device_name: r.device_name,
        })
        .collect()
    } else {
        sqlx::query!(
            r#"
            SELECT ie.id, ie.source, ie.book_id, ie.status, ie.message, ie.created_at,
                   b.title as "book_title?", ie.device_id, d.name as "device_name?"
            FROM ingest_events ie
            LEFT JOIN books b ON b.id = ie.book_id
            LEFT JOIN devices d ON d.id = ie.device_id
            ORDER BY ie.created_at DESC, ie.id DESC
            LIMIT $1
            "#,
            limit,
        )
        .fetch_all(exec)
        .await?
        .into_iter()
        .map(|r| IngestEventRow {
            id: r.id,
            source: r.source,
            book_id: r.book_id,
            status: r.status,
            message: r.message,
            created_at: r.created_at,
            book_title: r.book_title,
            device_id: r.device_id,
            device_name: r.device_name,
        })
        .collect()
    };

    Ok(rows)
}
