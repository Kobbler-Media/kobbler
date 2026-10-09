//! User book data repository operations.
//!
//! Handles per-user book data: ratings and personal reading status.

use super::Result;
use serde::{Deserialize, Serialize};
use sqlx::{Executor, Postgres};
use time::OffsetDateTime;
use uuid::Uuid;

/// User's personal data for a book.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UserBookData {
    pub id: Uuid,
    pub user_id: Uuid,
    pub book_id: Uuid,
    pub rating: Option<i32>,
    pub rating_source: Option<String>,
    #[serde(with = "time::serde::rfc3339::option")]
    pub rating_updated_at: Option<OffsetDateTime>,
    pub reading_status: Option<String>,
    #[serde(with = "time::serde::rfc3339::option")]
    pub reading_status_updated_at: Option<OffsetDateTime>,
    #[serde(with = "time::serde::rfc3339::option")]
    pub started_at: Option<OffsetDateTime>,
    #[serde(with = "time::serde::rfc3339::option")]
    pub finished_at: Option<OffsetDateTime>,
    #[serde(with = "time::serde::rfc3339")]
    pub created_at: OffsetDateTime,
    #[serde(with = "time::serde::rfc3339")]
    pub updated_at: OffsetDateTime,
}

/// Input for updating user book data.
#[derive(Debug, Default)]
pub struct UserBookDataUpdate {
    pub rating: Option<Option<i32>>,
    pub rating_source: Option<Option<String>>,
    pub reading_status: Option<Option<String>>,
    pub started_at: Option<Option<OffsetDateTime>>,
    pub finished_at: Option<Option<OffsetDateTime>>,
}

/// Get user book data for a specific user and book.
pub async fn get<'e, E>(exec: E, user_id: Uuid, book_id: Uuid) -> Result<Option<UserBookData>>
where
    E: Executor<'e, Database = Postgres>,
{
    let row = sqlx::query!(
        r#"
        SELECT id, user_id, book_id, rating, rating_source, rating_updated_at,
               reading_status, reading_status_updated_at, started_at, finished_at,
               created_at, updated_at
        FROM user_book_data
        WHERE user_id = $1 AND book_id = $2
        "#,
        user_id,
        book_id,
    )
    .fetch_optional(exec)
    .await?;

    Ok(row.map(|r| UserBookData {
        id: r.id,
        user_id: r.user_id,
        book_id: r.book_id,
        rating: r.rating,
        rating_source: r.rating_source,
        rating_updated_at: r.rating_updated_at,
        reading_status: r.reading_status,
        reading_status_updated_at: r.reading_status_updated_at,
        started_at: r.started_at,
        finished_at: r.finished_at,
        created_at: r.created_at,
        updated_at: r.updated_at,
    }))
}

/// Get or create user book data entry. Returns the ID.
pub async fn get_or_create<'e, E>(exec: E, user_id: Uuid, book_id: Uuid) -> Result<Uuid>
where
    E: Executor<'e, Database = Postgres>,
{
    let id: Uuid = sqlx::query_scalar!(
        r#"
        INSERT INTO user_book_data (user_id, book_id)
        VALUES ($1, $2)
        ON CONFLICT (user_id, book_id) DO UPDATE SET user_id = user_book_data.user_id
        RETURNING id
        "#,
        user_id,
        book_id,
    )
    .fetch_one(exec)
    .await?;

    Ok(id)
}

/// Update user's rating for a book.
pub async fn update_rating<'e, E>(
    exec: E,
    user_id: Uuid,
    book_id: Uuid,
    rating: Option<i32>,
    source: &str,
) -> Result<Uuid>
where
    E: Executor<'e, Database = Postgres>,
{
    let id: Uuid = sqlx::query_scalar!(
        r#"
        INSERT INTO user_book_data (user_id, book_id, rating, rating_source, rating_updated_at)
        VALUES ($1, $2, $3, $4, NOW())
        ON CONFLICT (user_id, book_id) DO UPDATE SET
            rating = $3,
            rating_source = $4,
            rating_updated_at = NOW(),
            updated_at = NOW()
        RETURNING id
        "#,
        user_id,
        book_id,
        rating,
        source,
    )
    .fetch_one(exec)
    .await?;

    Ok(id)
}

/// Update user's reading status for a book.
pub async fn update_reading_status<'e, E>(
    exec: E,
    user_id: Uuid,
    book_id: Uuid,
    status: Option<&str>,
) -> Result<Uuid>
where
    E: Executor<'e, Database = Postgres>,
{
    // Determine if we need to set started_at or finished_at
    let set_started = status == Some("reading");
    let set_finished = status == Some("finished");

    let id: Uuid = sqlx::query_scalar!(
        r#"
        INSERT INTO user_book_data (user_id, book_id, reading_status, reading_status_updated_at, started_at, finished_at)
        VALUES ($1, $2, $3, NOW(),
                CASE WHEN $4 THEN NOW() ELSE NULL END,
                CASE WHEN $5 THEN NOW() ELSE NULL END)
        ON CONFLICT (user_id, book_id) DO UPDATE SET
            reading_status = $3,
            reading_status_updated_at = NOW(),
            started_at = CASE
                WHEN $4 AND user_book_data.started_at IS NULL THEN NOW()
                ELSE user_book_data.started_at
            END,
            finished_at = CASE
                WHEN $5 THEN NOW()
                ELSE user_book_data.finished_at
            END,
            updated_at = NOW()
        RETURNING id
        "#,
        user_id,
        book_id,
        status,
        set_started,
        set_finished,
    )
    .fetch_one(exec)
    .await?;

    Ok(id)
}

/// Update user book data from a sync provider pull (doesn't overwrite manual entries).
/// Only updates if the source matches the provider or the field is empty.
/// The `status` should already be translated to our status strings (want_to_read, reading, finished, did_not_finish).
/// `progress_changed` controls whether `reading_status_updated_at` is bumped — only when
/// the reading status changed or the provider reports new progress.
#[allow(clippy::too_many_arguments)]
pub async fn update_from_sync_provider<'e, E>(
    exec: E,
    user_id: Uuid,
    book_id: Uuid,
    rating: Option<i32>,
    status: Option<&str>,
    progress_changed: bool,
    provider_updated_at: Option<OffsetDateTime>,
    provider_type: &str,
) -> Result<bool>
where
    E: Executor<'e, Database = Postgres>,
{
    let rating_source = if rating.is_some() {
        Some(provider_type)
    } else {
        None
    };

    // Only update rating if source matches this provider or rating is null.
    // Only update status if it's empty or already from sync.
    // Only bump reading_status_updated_at when status or progress actually changed,
    // so unchanged syncs don't push books to the top of the dashboard.
    let result = sqlx::query!(
        r#"
        INSERT INTO user_book_data (user_id, book_id, rating, rating_source, rating_updated_at,
                                     reading_status, reading_status_updated_at)
        VALUES ($1, $2, $3, $7, $5, $4, $5)
        ON CONFLICT (user_id, book_id) DO UPDATE SET
            rating = CASE
                WHEN user_book_data.rating_source IS NULL OR user_book_data.rating_source = $7
                THEN COALESCE($3, user_book_data.rating)
                ELSE user_book_data.rating
            END,
            rating_source = CASE
                WHEN user_book_data.rating_source IS NULL OR user_book_data.rating_source = $7
                THEN COALESCE($6, user_book_data.rating_source)
                ELSE user_book_data.rating_source
            END,
            rating_updated_at = CASE
                WHEN user_book_data.rating_source IS NULL OR user_book_data.rating_source = $7
                THEN COALESCE($5, user_book_data.rating_updated_at)
                ELSE user_book_data.rating_updated_at
            END,
            reading_status = COALESCE($4, user_book_data.reading_status),
            reading_status_updated_at = CASE
                WHEN $8 OR user_book_data.reading_status IS DISTINCT FROM $4
                THEN COALESCE($5, NOW())
                ELSE user_book_data.reading_status_updated_at
            END,
            updated_at = NOW()
        "#,
        user_id,
        book_id,
        rating,
        status,
        provider_updated_at,
        rating_source as Option<&str>,
        provider_type,
        progress_changed,
    )
    .execute(exec)
    .await?;

    Ok(result.rows_affected() > 0)
}

/// Delete user book data.
pub async fn delete<'e, E>(exec: E, user_id: Uuid, book_id: Uuid) -> Result<bool>
where
    E: Executor<'e, Database = Postgres>,
{
    let result = sqlx::query!(
        "DELETE FROM user_book_data WHERE user_id = $1 AND book_id = $2",
        user_id,
        book_id,
    )
    .execute(exec)
    .await?;

    Ok(result.rows_affected() > 0)
}

/// List all books with user data for a user (for sync provider pull).
pub async fn list_for_user<'e, E>(exec: E, user_id: Uuid) -> Result<Vec<UserBookData>>
where
    E: Executor<'e, Database = Postgres>,
{
    let rows = sqlx::query!(
        r#"
        SELECT id, user_id, book_id, rating, rating_source, rating_updated_at,
               reading_status, reading_status_updated_at, started_at, finished_at,
               created_at, updated_at
        FROM user_book_data
        WHERE user_id = $1
        ORDER BY updated_at DESC
        "#,
        user_id,
    )
    .fetch_all(exec)
    .await?;

    Ok(rows
        .into_iter()
        .map(|r| UserBookData {
            id: r.id,
            user_id: r.user_id,
            book_id: r.book_id,
            rating: r.rating,
            rating_source: r.rating_source,
            rating_updated_at: r.rating_updated_at,
            reading_status: r.reading_status,
            reading_status_updated_at: r.reading_status_updated_at,
            started_at: r.started_at,
            finished_at: r.finished_at,
            created_at: r.created_at,
            updated_at: r.updated_at,
        })
        .collect())
}
