//! Wanted attempts repository operations.

use super::Result;
use serde_json::Value;
use sqlx::{Executor, Postgres};
use uuid::Uuid;

/// Input for creating a wanted attempt.
#[derive(Debug, Default)]
pub struct WantedAttemptInput {
    pub book_id: Option<Uuid>,
    pub movie_id: Option<Uuid>,
    pub episode_id: Option<Uuid>,
    pub source: String,
    pub external_id: Option<String>,
    pub download_client_id: Option<i32>,
    pub download_url: Option<String>,
    pub status: String,
    pub message: Option<String>,
    pub user_id: Option<Uuid>,
}

/// Create a new wanted attempt.
pub async fn create<'e, E>(exec: E, input: WantedAttemptInput) -> Result<Uuid>
where
    E: Executor<'e, Database = Postgres>,
{
    let id = Uuid::now_v7();
    sqlx::query!(
        r#"
        INSERT INTO wanted_attempts (
            id, book_id, movie_id, episode_id, source, external_id, download_client_id,
            download_url, status, message, user_id, created_at
        )
        VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, NOW())
        "#,
        id,
        input.book_id,
        input.movie_id,
        input.episode_id,
        input.source,
        input.external_id as Option<String>,
        input.download_client_id,
        input.download_url as Option<String>,
        input.status,
        input.message as Option<String>,
        input.user_id,
    )
    .execute(exec)
    .await?;

    Ok(id)
}

/// Check if an idempotency key has already been processed.
/// Returns the attempt ID if found.
pub async fn check_idempotency_key<'e, E>(exec: E, idempotency_key: Uuid) -> Result<Option<Uuid>>
where
    E: Executor<'e, Database = Postgres>,
{
    let row: Option<Uuid> = sqlx::query_scalar!(
        "SELECT id FROM wanted_attempts WHERE idempotency_key = $1",
        idempotency_key,
    )
    .fetch_optional(exec)
    .await?;
    Ok(row)
}

/// Update status and message for a wanted attempt by external_id.
/// Optionally stores an idempotency key for deduplication.
/// Returns (book_id, movie_id, download_url) if found.
pub async fn update_status_returning<'e, E>(
    exec: E,
    external_id: &str,
    status: &str,
    message: Option<&str>,
    files_json: Option<Value>,
) -> Result<Option<(Option<Uuid>, Option<Uuid>, Option<String>)>>
where
    E: Executor<'e, Database = Postgres>,
{
    update_status_returning_with_idempotency(exec, external_id, status, message, files_json, None)
        .await
}

/// Update status and message for a wanted attempt by external_id with idempotency key.
/// Returns (book_id, movie_id, download_url) if found.
pub async fn update_status_returning_with_idempotency<'e, E>(
    exec: E,
    external_id: &str,
    status: &str,
    message: Option<&str>,
    files_json: Option<Value>,
    idempotency_key: Option<Uuid>,
) -> Result<Option<(Option<Uuid>, Option<Uuid>, Option<String>)>>
where
    E: Executor<'e, Database = Postgres>,
{
    let row = sqlx::query!(
        r#"
        UPDATE wanted_attempts
        SET status = $1, message = $2, files_json = $3, idempotency_key = COALESCE($5, idempotency_key)
        WHERE external_id = $4
        RETURNING book_id, movie_id, download_url
        "#,
        status,
        message,
        files_json as Option<Value>,
        external_id,
        idempotency_key,
    )
    .fetch_optional(exec)
    .await?;

    Ok(row.map(|r| (r.book_id, r.movie_id, r.download_url)))
}

/// Update status and message for a wanted attempt by external_id (without returning data).
pub async fn update_status<'e, E>(
    exec: E,
    external_id: &str,
    status: &str,
    message: Option<&str>,
) -> Result<bool>
where
    E: Executor<'e, Database = Postgres>,
{
    let result = sqlx::query!(
        "UPDATE wanted_attempts SET status = $1, message = $2 WHERE external_id = $3",
        status,
        message,
        external_id,
    )
    .execute(exec)
    .await?;
    Ok(result.rows_affected() > 0)
}

/// Update status for a wanted attempt by book_id and external_id.
pub async fn update_status_by_book<'e, E>(
    exec: E,
    book_id: Uuid,
    external_id: &str,
    status: &str,
) -> Result<bool>
where
    E: Executor<'e, Database = Postgres>,
{
    let result = sqlx::query!(
        "UPDATE wanted_attempts SET status = $1 WHERE book_id = $2 AND external_id = $3",
        status,
        book_id,
        external_id,
    )
    .execute(exec)
    .await?;
    Ok(result.rows_affected() > 0)
}

/// Update status and message for a wanted attempt by book_id and external_id.
pub async fn update_status_with_message_by_book<'e, E>(
    exec: E,
    book_id: Uuid,
    external_id: &str,
    status: &str,
    message: Option<&str>,
) -> Result<bool>
where
    E: Executor<'e, Database = Postgres>,
{
    let result = sqlx::query!(
        "UPDATE wanted_attempts SET status = $1, message = $2 WHERE book_id = $3 AND external_id = $4",
        status,
        message,
        book_id,
        external_id,
    )
    .execute(exec)
    .await?;
    Ok(result.rows_affected() > 0)
}

/// Skip the latest 'needs_review' attempt for a book by marking it as failed.
pub async fn skip_latest_needs_review<'e, E>(exec: E, book_id: Uuid) -> Result<bool>
where
    E: Executor<'e, Database = Postgres>,
{
    let result = sqlx::query!(
        r#"
        UPDATE wanted_attempts
        SET status = 'failed', message = 'Skipped by user'
        WHERE id = (
            SELECT id FROM wanted_attempts
            WHERE book_id = $1 AND status = 'needs_review'
            ORDER BY created_at DESC
            LIMIT 1
        )
        "#,
        book_id,
    )
    .execute(exec)
    .await?;
    Ok(result.rows_affected() > 0)
}

/// Get the latest attempt with files for a book.
pub async fn get_latest_with_files<'e, E>(
    exec: E,
    book_id: Uuid,
) -> Result<Option<(Uuid, Option<String>, Option<Value>, String)>>
where
    E: Executor<'e, Database = Postgres>,
{
    let row = sqlx::query!(
        r#"
        SELECT id, external_id, files_json, status
        FROM wanted_attempts
        WHERE book_id = $1 AND files_json IS NOT NULL
        ORDER BY created_at DESC
        LIMIT 1
        "#,
        book_id,
    )
    .fetch_optional(exec)
    .await?;

    Ok(row.map(|r| (r.id, r.external_id, r.files_json, r.status)))
}

/// Input for upserting a book attempt keyed on `(book_id, source, external_id)`.
#[derive(Debug, Default)]
pub struct BookAttemptUpsert<'a> {
    pub book_id: Uuid,
    pub source: &'a str,
    pub external_id: &'a str,
    pub status: &'a str,
    pub message: Option<&'a str>,
    pub files_json: Option<Value>,
    pub user_id: Option<Uuid>,
}

/// Upsert a wanted attempt by external_id (for watch callbacks).
pub async fn upsert_by_external_id<'e, E>(exec: E, input: BookAttemptUpsert<'_>) -> Result<Uuid>
where
    E: Executor<'e, Database = Postgres>,
{
    let id = Uuid::now_v7();
    let created_id: Uuid = sqlx::query_scalar!(
        r#"
        INSERT INTO wanted_attempts (id, book_id, source, external_id, status, message, files_json, user_id, created_at)
        VALUES ($1, $2, $3, $4, $5, $6, $7, $8, NOW())
        ON CONFLICT (book_id, source, external_id) WHERE external_id IS NOT NULL
        DO UPDATE SET status = EXCLUDED.status, message = EXCLUDED.message, files_json = EXCLUDED.files_json,
                      user_id = COALESCE(wanted_attempts.user_id, EXCLUDED.user_id)
        RETURNING id
        "#,
        id,
        input.book_id,
        input.source,
        input.external_id,
        input.status,
        input.message,
        input.files_json as Option<Value>,
        input.user_id,
    )
    .fetch_one(exec)
    .await?;

    Ok(created_id)
}

/// Delete orphaned wanted attempts (where the linked entity has been deleted).
/// Returns the number of deleted rows.
pub async fn delete_orphaned<'e, E>(exec: E) -> Result<u64>
where
    E: Executor<'e, Database = Postgres>,
{
    let result: sqlx::postgres::PgQueryResult = sqlx::query!(
        "DELETE FROM wanted_attempts WHERE book_id IS NULL AND movie_id IS NULL AND episode_id IS NULL"
    )
    .execute(exec)
    .await?;

    Ok(result.rows_affected())
}

/// Status counts for wanted books.
#[derive(Debug, Default)]
pub struct WantedStatusCounts {
    pub pending: i64,
    pub wanted: i64,
    pub searching: i64,
    pub downloading: i64,
    pub ingesting: i64,
    pub watching: i64,
    pub search_failed: i64,
    pub queued: i64,
    pub kobload_error: i64,
    pub needs_review: i64,
    pub completed: i64,
    pub failed: i64,
}

/// Get status counts for wanted books (combined book want_status and attempt status).
pub async fn get_status_counts<'e, E>(exec: E) -> Result<WantedStatusCounts>
where
    E: Executor<'e, Database = Postgres>,
{
    let row = sqlx::query!(
        r#"
        WITH book_counts AS (
            SELECT
                SUM(CASE WHEN b.want_status = 'pending' THEN 1 ELSE 0 END) as pending,
                SUM(CASE WHEN b.want_status = 'wanted' THEN 1 ELSE 0 END) as wanted,
                SUM(CASE WHEN b.want_status = 'searching' THEN 1 ELSE 0 END) as searching,
                SUM(CASE WHEN b.want_status = 'downloading' THEN 1 ELSE 0 END) as downloading,
                SUM(CASE WHEN b.want_status = 'watching' THEN 1 ELSE 0 END) as watching,
                SUM(CASE WHEN b.want_status = 'search_failed' THEN 1 ELSE 0 END) as search_failed
            FROM books b
            WHERE NOT (EXISTS (SELECT 1 FROM book_files bf WHERE bf.book_id = b.id) OR EXISTS (SELECT 1 FROM audiobook_files af WHERE af.book_id = b.id))
              AND b.want_status IN ('pending', 'wanted', 'searching', 'downloading', 'watching', 'search_failed')
        ),
        attempt_counts AS (
            SELECT
                COALESCE(SUM(CASE WHEN wa.status = 'queued' THEN 1 ELSE 0 END), 0) as queued,
                COALESCE(SUM(CASE WHEN wa.status = 'ingesting' THEN 1 ELSE 0 END), 0) as ingesting,
                COALESCE(SUM(CASE WHEN wa.status = 'kobload_error' THEN 1 ELSE 0 END), 0) as kobload_error,
                COALESCE(SUM(CASE WHEN wa.status = 'needs_review' THEN 1 ELSE 0 END), 0) as needs_review,
                COALESCE(SUM(CASE WHEN wa.status = 'completed' THEN 1 ELSE 0 END), 0) as completed,
                COALESCE(SUM(CASE WHEN wa.status = 'failed' THEN 1 ELSE 0 END), 0) as failed
            FROM wanted_attempts wa
            JOIN books b ON b.id = wa.book_id
            WHERE NOT (EXISTS (SELECT 1 FROM book_files bf WHERE bf.book_id = b.id) OR EXISTS (SELECT 1 FROM audiobook_files af WHERE af.book_id = b.id))
            AND wa.id = (
                SELECT wa2.id FROM wanted_attempts wa2
                WHERE wa2.book_id = wa.book_id
                ORDER BY wa2.created_at DESC LIMIT 1
            )
        )
        SELECT
            COALESCE(bc.pending, 0) as "pending!",
            COALESCE(bc.wanted, 0) as "wanted!",
            COALESCE(bc.searching, 0) as "searching!",
            COALESCE(bc.downloading, 0) as "downloading!",
            COALESCE(bc.watching, 0) as "watching!",
            COALESCE(bc.search_failed, 0) as "search_failed!",
            ac.queued as "queued!",
            ac.ingesting as "ingesting!",
            ac.kobload_error as "kobload_error!",
            ac.needs_review as "needs_review!",
            ac.completed as "completed!",
            ac.failed as "failed!"
        FROM book_counts bc, attempt_counts ac
        "#,
    )
    .fetch_one(exec)
    .await?;

    Ok(WantedStatusCounts {
        pending: row.pending,
        wanted: row.wanted,
        searching: row.searching,
        downloading: row.downloading,
        ingesting: row.ingesting,
        watching: row.watching,
        search_failed: row.search_failed,
        queued: row.queued,
        kobload_error: row.kobload_error,
        needs_review: row.needs_review,
        completed: row.completed,
        failed: row.failed,
    })
}

/// Attempt row for listing.
#[derive(Debug)]
pub struct AttemptRow {
    pub id: Uuid,
    pub book_id: Option<Uuid>,
    pub movie_id: Option<Uuid>,
    pub source: String,
    pub external_id: Option<String>,
    pub download_client_id: Option<i32>,
    pub download_url: Option<String>,
    pub status: String,
    pub message: Option<String>,
    pub created_at: time::OffsetDateTime,
}

/// List all attempts for a book.
pub async fn list_by_book<'e, E>(exec: E, book_id: Uuid) -> Result<Vec<AttemptRow>>
where
    E: Executor<'e, Database = Postgres>,
{
    let rows = sqlx::query!(
        r#"
        SELECT id, book_id, movie_id, source, external_id, download_client_id,
               download_url, status, message, created_at
        FROM wanted_attempts
        WHERE book_id = $1
        ORDER BY created_at DESC
        "#,
        book_id,
    )
    .fetch_all(exec)
    .await?;

    Ok(rows
        .into_iter()
        .map(|r| AttemptRow {
            id: r.id,
            book_id: r.book_id,
            movie_id: r.movie_id,
            source: r.source,
            external_id: r.external_id,
            download_client_id: r.download_client_id,
            download_url: r.download_url,
            status: r.status,
            message: r.message,
            created_at: r.created_at,
        })
        .collect())
}

/// The most recent person who asked for this entity, if anyone did.
///
/// Feeds "your download finished" attribution: schedulers leave `user_id` NULL, so a NULL
/// here means nobody asked and the event is not news to any particular person.
pub async fn latest_requester<'e, E>(exec: E, entity_id: Uuid) -> Result<Option<Uuid>>
where
    E: Executor<'e, Database = Postgres>,
{
    let row = sqlx::query_scalar!(
        r#"
        SELECT user_id AS "user_id!"
        FROM wanted_attempts
        WHERE user_id IS NOT NULL
          AND (book_id = $1 OR movie_id = $1 OR episode_id = $1)
        ORDER BY created_at DESC, id DESC
        LIMIT 1
        "#,
        entity_id,
    )
    .fetch_optional(exec)
    .await?;
    Ok(row)
}
