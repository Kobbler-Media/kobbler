//! Junction table operations for the many-to-many book↔series relationship.

use super::Result;
use serde::Serialize;
use sqlx::{Executor, Postgres};
use uuid::Uuid;

/// A book's membership in a series.
#[derive(Debug, Clone, Serialize)]
pub struct BookSeriesEntry {
    pub series_id: Uuid,
    pub series_name: String,
    pub series_index: Option<f64>,
    pub is_primary: bool,
    pub total_book_count: Option<i32>,
}

/// Add (or update) a book's membership in a series.
///
/// If `is_primary` is true, any existing primary for this book is demoted first.
/// Uses UPSERT so calling with an existing (book_id, series_id) pair updates index/primary.
///
/// Takes `&PgPool` because setting primary requires two sequential queries.
pub async fn add(
    pool: &sqlx::PgPool,
    book_id: Uuid,
    series_id: Uuid,
    series_index: Option<f64>,
    is_primary: bool,
) -> Result<()> {
    // Upsert the entry (always as non-primary first to avoid unique index conflicts)
    sqlx::query(
        r#"
        INSERT INTO book_series (book_id, series_id, series_index, is_primary)
        VALUES ($1, $2, $3, FALSE)
        ON CONFLICT (book_id, series_id) DO UPDATE SET
            series_index = EXCLUDED.series_index
        "#,
    )
    .bind(book_id)
    .bind(series_id)
    .bind(series_index)
    .execute(pool)
    .await?;

    if is_primary {
        // Atomically set primary — sets target to TRUE, all others to FALSE
        sqlx::query("UPDATE book_series SET is_primary = (series_id = $2) WHERE book_id = $1")
            .bind(book_id)
            .bind(series_id)
            .execute(pool)
            .await?;
    }

    Ok(())
}

/// Remove a book's membership in a series.
///
/// Returns true if a row was deleted.
pub async fn remove<'e, E>(exec: E, book_id: Uuid, series_id: Uuid) -> Result<bool>
where
    E: Executor<'e, Database = Postgres>,
{
    let result = sqlx::query("DELETE FROM book_series WHERE book_id = $1 AND series_id = $2")
        .bind(book_id)
        .bind(series_id)
        .execute(exec)
        .await?;

    Ok(result.rows_affected() > 0)
}

/// Set which series is primary for a book. The series must already be in the junction table.
pub async fn set_primary<'e, E>(exec: E, book_id: Uuid, series_id: Uuid) -> Result<()>
where
    E: Executor<'e, Database = Postgres>,
{
    // Single UPDATE: set is_primary = TRUE for the target, FALSE for all others.
    // Avoids CTE snapshot issues with the partial unique index.
    sqlx::query("UPDATE book_series SET is_primary = (series_id = $2) WHERE book_id = $1")
        .bind(book_id)
        .bind(series_id)
        .execute(exec)
        .await?;

    Ok(())
}

/// List all series memberships for a book, with series metadata.
pub async fn list_for_book<'e, E>(exec: E, book_id: Uuid) -> Result<Vec<BookSeriesEntry>>
where
    E: Executor<'e, Database = Postgres>,
{
    let rows = sqlx::query_as!(
        BookSeriesEntry,
        r#"
        SELECT bs.series_id, s.name AS series_name,
               bs.series_index, bs.is_primary AS "is_primary!",
               s.total_book_count
        FROM book_series bs
        JOIN series s ON s.id = bs.series_id
        WHERE bs.book_id = $1
        ORDER BY bs.is_primary DESC, s.total_book_count ASC NULLS LAST, s.name
        "#,
        book_id,
    )
    .fetch_all(exec)
    .await?;

    Ok(rows)
}

/// Auto-select the most specific series (lowest total_book_count) as primary.
///
/// If the book already has a primary, this is a no-op. Call after adding memberships
/// when no explicit primary was set. Pass `force: true` to re-select even if a primary exists.
///
/// When called inside a transaction, pass the transaction as the executor.
/// When called standalone, pass a pool — the function issues multiple sequential queries.
pub async fn auto_select_primary<'e, E>(exec: E, book_id: Uuid, force: bool) -> Result<()>
where
    E: Executor<'e, Database = Postgres>,
{
    // Use a single UPDATE with subquery that also handles the demote.
    // This avoids the CTE snapshot issue by doing it in one statement:
    // 1. Clear all primaries for this book
    // 2. Set the best one as primary
    // We combine both into one by setting is_primary = (series_id = best_id).
    let condition = if force {
        "TRUE"
    } else {
        "NOT EXISTS (SELECT 1 FROM book_series bs2 WHERE bs2.book_id = $1 AND bs2.is_primary = TRUE)"
    };

    let query = format!(
        r#"
        UPDATE book_series SET is_primary = (series_id = (
            SELECT bs3.series_id
            FROM book_series bs3
            JOIN series s ON s.id = bs3.series_id
            WHERE bs3.book_id = $1
            ORDER BY s.total_book_count ASC NULLS LAST, s.created_at ASC
            LIMIT 1
        ))
        WHERE book_id = $1 AND {condition}
        "#,
    );

    sqlx::query(sqlx::AssertSqlSafe(query.as_str()))
        .bind(book_id)
        .execute(exec)
        .await?;

    Ok(())
}

/// Bulk-replace all series memberships for a book.
///
/// Deletes existing entries and inserts new ones. Exactly one entry should have `is_primary = true`.
pub async fn set_all<'e, E>(
    exec: E,
    book_id: Uuid,
    entries: &[(Uuid, Option<f64>, bool)],
) -> Result<()>
where
    E: Executor<'e, Database = Postgres>,
{
    // Build as a single statement to avoid needing a transaction
    let mut series_ids = Vec::with_capacity(entries.len());
    let mut indexes = Vec::with_capacity(entries.len());
    let mut primaries = Vec::with_capacity(entries.len());

    for (sid, idx, primary) in entries {
        series_ids.push(*sid);
        indexes.push(*idx);
        primaries.push(*primary);
    }

    sqlx::query(
        r#"
        WITH deleted AS (
            DELETE FROM book_series WHERE book_id = $1
        )
        INSERT INTO book_series (book_id, series_id, series_index, is_primary)
        SELECT $1, unnest($2::uuid[]), unnest($3::double precision[]), unnest($4::boolean[])
        "#,
    )
    .bind(book_id)
    .bind(&series_ids)
    .bind(&indexes)
    .bind(&primaries)
    .execute(exec)
    .await?;

    Ok(())
}

/// Move all book memberships from one series to another (for merge).
///
/// Books already in the target series keep their target entry; the source entry is dropped.
pub async fn move_all_to_series<'e, E>(
    exec: E,
    from_series_id: Uuid,
    to_series_id: Uuid,
) -> Result<u64>
where
    E: Executor<'e, Database = Postgres>,
{
    // First update non-conflicting rows
    let result = sqlx::query(
        r#"
        UPDATE book_series SET series_id = $2
        WHERE series_id = $1
          AND book_id NOT IN (SELECT book_id FROM book_series WHERE series_id = $2)
        "#,
    )
    .bind(from_series_id)
    .bind(to_series_id)
    .execute(exec)
    .await?;

    let moved = result.rows_affected();

    Ok(moved)
}

/// Delete leftover source entries after a move (books that were in both series).
pub async fn delete_for_series<'e, E>(exec: E, series_id: Uuid) -> Result<u64>
where
    E: Executor<'e, Database = Postgres>,
{
    let result = sqlx::query("DELETE FROM book_series WHERE series_id = $1")
        .bind(series_id)
        .execute(exec)
        .await?;

    Ok(result.rows_affected())
}

/// Delete all entries for a book (clear all series memberships).
pub async fn clear_for_book<'e, E>(exec: E, book_id: Uuid) -> Result<u64>
where
    E: Executor<'e, Database = Postgres>,
{
    let result = sqlx::query("DELETE FROM book_series WHERE book_id = $1")
        .bind(book_id)
        .execute(exec)
        .await?;

    Ok(result.rows_affected())
}

/// Get the primary series for a book, if any.
pub async fn get_primary<'e, E>(
    exec: E,
    book_id: Uuid,
) -> Result<Option<(Uuid, String, Option<f64>)>>
where
    E: Executor<'e, Database = Postgres>,
{
    let row = sqlx::query!(
        r#"
        SELECT bs.series_id, s.name AS series_name, bs.series_index
        FROM book_series bs
        JOIN series s ON s.id = bs.series_id
        WHERE bs.book_id = $1 AND bs.is_primary = TRUE
        "#,
        book_id,
    )
    .fetch_optional(exec)
    .await?;

    Ok(row.map(|r| (r.series_id, r.series_name, r.series_index)))
}
