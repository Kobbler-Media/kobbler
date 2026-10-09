//! Ingest queue CRUD operations.
//!
//! Now queries books + book_ingest_source instead of the old ingest_queue table.

use super::{IngestQueueInput, IngestQueueUpdate, Result};
use crate::repositories::book_ingest_source;
use sqlx::{Executor, Postgres};
use uuid::Uuid;

/// Create a new ingest queue item (book + ingest source).
///
/// When `update_target_book_id` is set, the ingest source is linked to the existing book
/// (e.g., a wanted book from Hardcover) and no new book is created. The existing book's
/// ingest_status is set to track processing. When not set, a new book record is created.
pub async fn create(pool: &sqlx::PgPool, input: IngestQueueInput) -> Result<Uuid> {
    if let Some(target_book_id) = input.update_target_book_id {
        // Link to existing book — don't create a new one.
        // Use a single CTE to set ingest_status and create the source record.
        sqlx::query(
            r#"
            WITH updated_book AS (
                UPDATE books SET ingest_status = $1, updated_at = NOW()
                WHERE id = $2
                RETURNING id
            )
            INSERT INTO book_ingest_source (
                book_id, file_path, file_name, file_size, file_hash,
                source_mtime, source_format, extracted_cover, pending_authors,
                pending_series, source_metadata, update_target_book_id
            )
            SELECT id, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13
            FROM updated_book
            ON CONFLICT (book_id) DO UPDATE SET
                file_path = EXCLUDED.file_path,
                file_name = EXCLUDED.file_name,
                file_size = EXCLUDED.file_size,
                file_hash = EXCLUDED.file_hash,
                source_mtime = EXCLUDED.source_mtime,
                source_format = EXCLUDED.source_format,
                extracted_cover = EXCLUDED.extracted_cover,
                pending_authors = EXCLUDED.pending_authors,
                pending_series = EXCLUDED.pending_series,
                source_metadata = EXCLUDED.source_metadata,
                update_target_book_id = EXCLUDED.update_target_book_id
            "#,
        )
        .bind(&input.status)
        .bind(target_book_id)
        .bind(&input.file_path)
        .bind(&input.file_name)
        .bind(input.file_size)
        .bind(&input.file_hash)
        .bind(input.source_mtime)
        .bind(&input.source_format)
        .bind(&input.cover)
        .bind(&input.pending_authors)
        .bind(&input.pending_series)
        .bind(&input.source_metadata)
        .bind(input.update_target_book_id)
        .execute(pool)
        .await?;

        Ok(target_book_id)
    } else {
        let id = Uuid::now_v7();
        let title = input
            .title
            .clone()
            .unwrap_or_else(|| input.file_name.clone());

        // Create new book and ingest source together
        sqlx::query(
            r#"
            WITH new_book AS (
                INSERT INTO books (id, title, description, language, publisher, pubdate,
                                  want_status, ingest_status)
                VALUES ($1, $2, $3, $4, $5, $6, 'none', $7)
                RETURNING id
            )
            INSERT INTO book_ingest_source (
                book_id, file_path, file_name, file_size, file_hash,
                source_mtime, source_format, extracted_cover, pending_authors,
                pending_series, source_metadata
            )
            SELECT id, $8, $9, $10, $11, $12, $13, $14, $15, $16, $17
            FROM new_book
            "#,
        )
        .bind(id)
        .bind(&title)
        .bind(&input.description)
        .bind(&input.language)
        .bind(&input.publisher)
        .bind(input.pubdate)
        .bind(&input.status)
        .bind(&input.file_path)
        .bind(&input.file_name)
        .bind(input.file_size)
        .bind(&input.file_hash)
        .bind(input.source_mtime)
        .bind(&input.source_format)
        .bind(&input.cover)
        .bind(&input.pending_authors)
        .bind(&input.pending_series)
        .bind(&input.source_metadata)
        .execute(pool)
        .await?;

        Ok(id)
    }
}

/// Update an ingest queue item (updates both book and ingest source).
pub async fn update(pool: &sqlx::PgPool, id: Uuid, input: IngestQueueUpdate) -> Result<bool> {
    // Update book fields
    let book_result = sqlx::query(
        r#"
        UPDATE books SET
            title = COALESCE($2, title),
            description = COALESCE($3, description),
            language = COALESCE($4, language),
            publisher = COALESCE($5, publisher),
            pubdate = COALESCE($6, pubdate),
            ingest_status = COALESCE($7, ingest_status),
            updated_at = NOW()
        WHERE id = $1 AND ingest_status IS NOT NULL
        "#,
    )
    .bind(id)
    .bind(&input.title)
    .bind(&input.description)
    .bind(&input.language)
    .bind(&input.publisher)
    .bind(input.pubdate)
    .bind(&input.status)
    .execute(pool)
    .await?;

    // Update ingest source fields
    sqlx::query(
        r#"
        UPDATE book_ingest_source SET
            extracted_cover = COALESCE($2, extracted_cover),
            pending_authors = COALESCE($3, pending_authors),
            pending_series = COALESCE($4, pending_series),
            pending_series_index = COALESCE($5, pending_series_index),
            source_metadata = COALESCE($6, source_metadata),
            update_target_book_id = COALESCE($7, update_target_book_id),
            conversion_error = COALESCE($8, conversion_error)
        WHERE book_id = $1
        "#,
    )
    .bind(id)
    .bind(&input.cover)
    .bind(&input.pending_authors)
    .bind(&input.pending_series)
    .bind(input.pending_series_index)
    .bind(&input.source_metadata)
    .bind(input.update_target_book_id)
    .bind(&input.conversion_error)
    .execute(pool)
    .await?;

    Ok(book_result.rows_affected() > 0)
}

/// Delete an ingest queue item (deletes book, which cascades to ingest source).
pub async fn delete<'e, E>(exec: E, id: Uuid) -> Result<bool>
where
    E: Executor<'e, Database = Postgres>,
{
    // Only delete if it's an ingest item (has ingest_status set)
    let result = sqlx::query("DELETE FROM books WHERE id = $1 AND ingest_status IS NOT NULL")
        .bind(id)
        .execute(exec)
        .await?;
    Ok(result.rows_affected() > 0)
}

/// Check if a file path already exists in the ingest queue.
pub async fn exists_by_path<'e, E>(exec: E, file_path: &str) -> Result<bool>
where
    E: Executor<'e, Database = Postgres>,
{
    book_ingest_source::exists_by_path(exec, file_path).await
}

/// Check if a file hash already exists in the ingest queue or book_files.
pub async fn hash_exists<'e, E>(exec: E, hash: &str) -> Result<bool>
where
    E: Executor<'e, Database = Postgres>,
{
    book_ingest_source::hash_exists(exec, hash).await
}

/// Set status to 'approved' (clear ingest_status).
/// Note: The book_id parameter is no longer needed as the book already exists.
pub async fn approve(pool: &sqlx::PgPool, id: Uuid, _book_id: Uuid) -> Result<()> {
    sqlx::query(
        r#"
        UPDATE books
        SET ingest_status = NULL, want_status = 'none', updated_at = NOW()
        WHERE id = $1
        "#,
    )
    .bind(id)
    .execute(pool)
    .await?;

    // Clear conversion error on the ingest source
    book_ingest_source::clear_conversion_error(pool, id).await?;

    Ok(())
}

/// Set status to 'rejected'. Returns true if the item was found.
pub async fn reject<'e, E>(exec: E, id: Uuid) -> Result<bool>
where
    E: Executor<'e, Database = Postgres>,
{
    let result = sqlx::query(
        "UPDATE books SET ingest_status = 'rejected', updated_at = NOW() WHERE id = $1 AND ingest_status IS NOT NULL",
    )
    .bind(id)
    .execute(exec)
    .await?;
    Ok(result.rows_affected() > 0)
}

/// Set status to 'error' with error message.
pub async fn set_error(pool: &sqlx::PgPool, id: Uuid, error: &str) -> Result<()> {
    sqlx::query("UPDATE books SET ingest_status = 'error', updated_at = NOW() WHERE id = $1")
        .bind(id)
        .execute(pool)
        .await?;

    book_ingest_source::set_conversion_error(pool, id, error).await?;

    Ok(())
}

/// Get the extracted cover for an item.
/// Returns None if item doesn't exist or has no cover.
pub async fn get_cover<'e, E>(exec: E, id: Uuid) -> Result<Option<Vec<u8>>>
where
    E: Executor<'e, Database = Postgres>,
{
    book_ingest_source::get_cover(exec, id).await
}

/// Update just the status of an item.
pub async fn update_status<'e, E>(exec: E, id: Uuid, status: &str) -> Result<bool>
where
    E: Executor<'e, Database = Postgres>,
{
    let result = sqlx::query(
        "UPDATE books SET ingest_status = $2, updated_at = NOW() WHERE id = $1 AND ingest_status IS NOT NULL",
    )
    .bind(id)
    .bind(status)
    .execute(exec)
    .await?;
    Ok(result.rows_affected() > 0)
}

/// Update status for a pending item only.
pub async fn update_status_if_pending<'e, E>(exec: E, id: Uuid, status: &str) -> Result<bool>
where
    E: Executor<'e, Database = Postgres>,
{
    let result = sqlx::query(
        "UPDATE books SET ingest_status = $2, updated_at = NOW() WHERE id = $1 AND ingest_status = 'pending'",
    )
    .bind(id)
    .bind(status)
    .execute(exec)
    .await?;
    Ok(result.rows_affected() > 0)
}

/// Restore a rejected item back to pending.
pub async fn restore<'e, E>(exec: E, id: Uuid) -> Result<bool>
where
    E: Executor<'e, Database = Postgres>,
{
    let result = sqlx::query(
        "UPDATE books SET ingest_status = 'pending', updated_at = NOW() WHERE id = $1 AND ingest_status = 'rejected'",
    )
    .bind(id)
    .execute(exec)
    .await?;
    Ok(result.rows_affected() > 0)
}

/// Batch reject multiple items (only if pending).
pub async fn batch_reject<'e, E>(exec: E, ids: &[Uuid]) -> Result<u64>
where
    E: Executor<'e, Database = Postgres>,
{
    let result = sqlx::query(
        "UPDATE books SET ingest_status = 'rejected', updated_at = NOW() WHERE id = ANY($1) AND ingest_status = 'pending'",
    )
    .bind(ids)
    .execute(exec)
    .await?;
    Ok(result.rows_affected())
}

/// Update extracted cover for an item.
pub async fn update_cover<'e, E>(exec: E, id: Uuid, cover: &[u8]) -> Result<()>
where
    E: Executor<'e, Database = Postgres>,
{
    book_ingest_source::update_cover(exec, id, cover).await
}

/// Update extracted cover and set status to pending in one call.
pub async fn update_cover_and_set_pending(
    pool: &sqlx::PgPool,
    id: Uuid,
    cover: Option<&[u8]>,
) -> Result<()> {
    // Update status on book
    sqlx::query("UPDATE books SET ingest_status = 'pending', updated_at = NOW() WHERE id = $1")
        .bind(id)
        .execute(pool)
        .await?;

    // Update cover on ingest source if provided
    if let Some(cover_bytes) = cover {
        sqlx::query("UPDATE book_ingest_source SET extracted_cover = $2 WHERE book_id = $1")
            .bind(id)
            .bind(cover_bytes)
            .execute(pool)
            .await?;
    }
    Ok(())
}

/// Get an ingest queue item by ID.
pub async fn get_by_id<'e, E>(exec: E, id: Uuid) -> Result<Option<crate::watcher::IngestQueueItem>>
where
    E: Executor<'e, Database = Postgres>,
{
    let item: Option<crate::watcher::IngestQueueItem> = sqlx::query_as(
        r#"
        SELECT
            b.id, bis.file_path, bis.file_name, bis.file_size,
            bis.file_hash, bis.source_mtime, bis.source_format,
            b.title,
            COALESCE(
                (SELECT jsonb_agg(DISTINCT jsonb_build_object('id', a.id, 'name', a.name))
                 FROM book_authors ba
                 JOIN authors a ON a.id = ba.author_id
                 WHERE ba.book_id = b.id),
                '[]'::jsonb
            ) AS linked_authors,
            (SELECT array_agg(DISTINCT author_name) FROM (
                SELECT a.name as author_name FROM book_authors ba JOIN authors a ON a.id = ba.author_id WHERE ba.book_id = b.id
                UNION ALL
                SELECT elem->>'name' as author_name FROM jsonb_array_elements(COALESCE(bis.pending_authors, '[]'::jsonb)) AS elem
            ) combined WHERE author_name IS NOT NULL) AS merged_author_names,
            b.description,
            bs.series_id,
            COALESCE(s.name, bis.pending_series->>'name') AS series_name,
            COALESCE(bs.series_index, bis.pending_series_index) AS series_index,
            b.language,
            b.publisher,
            b.pubdate,
            COALESCE(b.identifiers, '{}'::jsonb) AS identifiers,
            b.rating,
            b.page_count,
            (bis.extracted_cover IS NOT NULL) AS has_cover,
            bis.pending_authors,
            bis.pending_series,
            bis.pending_series_index,
            bis.source_metadata,
            b.ingest_status AS status,
            bis.conversion_error,
            bis.update_target_book_id,
            b.created_at,
            b.updated_at
        FROM books b
        JOIN book_ingest_source bis ON bis.book_id = b.id
        LEFT JOIN book_series bs ON bs.book_id = b.id AND bs.is_primary = TRUE
        LEFT JOIN series s ON s.id = bs.series_id
        WHERE b.id = $1 AND b.ingest_status IS NOT NULL
        "#,
    )
    .bind(id)
    .fetch_optional(exec)
    .await?;
    Ok(item)
}

/// Input for creating an upload queue item.
#[derive(Debug)]
pub struct UploadInput {
    pub file_path: String,
    pub file_name: String,
    pub file_size: i64,
    pub file_hash: String,
    pub source_format: String,
}

/// Create an ingest queue item for a file upload, returning None if duplicate hash exists.
pub async fn create_for_upload(pool: &sqlx::PgPool, input: UploadInput) -> Result<Option<Uuid>> {
    let id = Uuid::now_v7();

    // Check for duplicate hash first
    let exists = hash_exists(pool, &input.file_hash).await?;
    if exists {
        return Ok(None);
    }

    // Create book + ingest source
    sqlx::query(
        r#"
        WITH new_book AS (
            INSERT INTO books (id, title, want_status, ingest_status)
            VALUES ($1, $2, 'none', 'pending')
            RETURNING id
        )
        INSERT INTO book_ingest_source (
            book_id, file_path, file_name, file_size, file_hash, source_format
        )
        SELECT id, $3, $4, $5, $6, $7
        FROM new_book
        "#,
    )
    .bind(id)
    .bind(&input.file_name)
    .bind(&input.file_path)
    .bind(&input.file_name)
    .bind(input.file_size)
    .bind(&input.file_hash)
    .bind(&input.source_format)
    .execute(pool)
    .await?;

    Ok(Some(id))
}

/// Set status to 'converting'.
pub async fn set_converting<'e, E>(exec: E, id: Uuid) -> Result<()>
where
    E: Executor<'e, Database = Postgres>,
{
    sqlx::query("UPDATE books SET ingest_status = 'converting', updated_at = NOW() WHERE id = $1")
        .bind(id)
        .execute(exec)
        .await?;
    Ok(())
}

/// Set status to 'approved' (clear ingest_status) and clear conversion_error.
/// Note: The book_id parameter is no longer needed since the book already exists.
pub async fn set_approved(pool: &sqlx::PgPool, id: Uuid, _book_id: Uuid) -> Result<()> {
    sqlx::query(
        r#"
        UPDATE books
        SET ingest_status = NULL, want_status = 'none', updated_at = NOW()
        WHERE id = $1
        "#,
    )
    .bind(id)
    .execute(pool)
    .await?;

    book_ingest_source::clear_conversion_error(pool, id).await?;

    Ok(())
}
