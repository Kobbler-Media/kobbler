//! Repository for deferred ingest operations.
//!
//! When a kobload ingest cannot be processed because the target media root is
//! temporarily unavailable, the ingest payload is stored here. The scan_check
//! scheduler re-enqueues these when the root recovers.

use super::Result;
use serde_json::Value;
use sqlx::{Executor, Postgres, Row};
use time::OffsetDateTime;
use uuid::Uuid;

/// A deferred ingest record.
#[derive(Debug)]
pub struct DeferredIngest {
    pub id: Uuid,
    pub download_id: Uuid,
    pub media_root_id: Uuid,
    pub payload: Value,
    pub response_mode: String,
    pub created_at: OffsetDateTime,
}

/// Store a deferred ingest. Uses ON CONFLICT to update if the same download_id
/// is deferred again (e.g. from a redelivered NATS message).
pub async fn insert<'e, E>(
    exec: E,
    download_id: Uuid,
    media_root_id: Uuid,
    payload: Value,
    response_mode: &str,
) -> Result<Uuid>
where
    E: Executor<'e, Database = Postgres>,
{
    let id = Uuid::now_v7();
    let row = sqlx::query(
        r#"
        INSERT INTO deferred_ingests (id, download_id, media_root_id, payload, response_mode)
        VALUES ($1, $2, $3, $4, $5)
        ON CONFLICT (download_id) DO UPDATE SET
            payload = EXCLUDED.payload,
            response_mode = EXCLUDED.response_mode
        RETURNING id
        "#,
    )
    .bind(id)
    .bind(download_id)
    .bind(media_root_id)
    .bind(&payload)
    .bind(response_mode)
    .fetch_one(exec)
    .await?;

    Ok(row.get("id"))
}

/// List all deferred ingests waiting for a specific media root.
pub async fn list_for_media_root<'e, E>(exec: E, media_root_id: Uuid) -> Result<Vec<DeferredIngest>>
where
    E: Executor<'e, Database = Postgres>,
{
    let rows = sqlx::query(
        r#"
        SELECT id, download_id, media_root_id, payload, response_mode, created_at
        FROM deferred_ingests
        WHERE media_root_id = $1
        ORDER BY created_at
        "#,
    )
    .bind(media_root_id)
    .fetch_all(exec)
    .await?;

    Ok(rows
        .into_iter()
        .map(|r| DeferredIngest {
            id: r.get("id"),
            download_id: r.get("download_id"),
            media_root_id: r.get("media_root_id"),
            payload: r.get("payload"),
            response_mode: r.get("response_mode"),
            created_at: r.get("created_at"),
        })
        .collect())
}

/// Delete a deferred ingest by ID.
pub async fn delete<'e, E>(exec: E, id: Uuid) -> Result<bool>
where
    E: Executor<'e, Database = Postgres>,
{
    let result = sqlx::query("DELETE FROM deferred_ingests WHERE id = $1")
        .bind(id)
        .execute(exec)
        .await?;
    Ok(result.rows_affected() > 0)
}

/// Delete a deferred ingest by download ID (cleanup after successful import).
pub async fn delete_by_download_id<'e, E>(exec: E, download_id: Uuid) -> Result<bool>
where
    E: Executor<'e, Database = Postgres>,
{
    let result = sqlx::query("DELETE FROM deferred_ingests WHERE download_id = $1")
        .bind(download_id)
        .execute(exec)
        .await?;
    Ok(result.rows_affected() > 0)
}

/// Count deferred ingests for a media root.
pub async fn count_for_media_root<'e, E>(exec: E, media_root_id: Uuid) -> Result<i64>
where
    E: Executor<'e, Database = Postgres>,
{
    let row = sqlx::query("SELECT COUNT(*) AS cnt FROM deferred_ingests WHERE media_root_id = $1")
        .bind(media_root_id)
        .fetch_one(exec)
        .await?;

    Ok(row.get::<i64, _>("cnt"))
}
