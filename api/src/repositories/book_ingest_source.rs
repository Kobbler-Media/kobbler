//! Book ingest source repository operations.
//!
//! Stores provenance data for ingested books - where they came from,
//! original file details, and pending author/series data before approval.

use super::Result;
use serde_json::Value;
use sqlx::{Executor, Postgres};
use time::OffsetDateTime;
use uuid::Uuid;

/// Input for creating an ingest source record.
#[derive(Debug, Default)]
pub struct BookIngestSourceInput {
    pub book_id: Uuid,
    pub file_path: String,
    pub file_name: String,
    pub file_size: i64,
    pub file_hash: Option<String>,
    pub source_mtime: Option<OffsetDateTime>,
    pub source_format: String,
    pub extracted_cover: Option<Vec<u8>>,
    /// Pending authors not yet linked: [{name, hardcover_id?, image_url?}]
    pub pending_authors: Option<Value>,
    /// Pending series not yet linked: {name, hardcover_id?}
    pub pending_series: Option<Value>,
    pub pending_series_index: Option<f64>,
    /// Source metadata: provenance info (source_type, provider, external_id, subjects)
    pub source_metadata: Option<Value>,
    pub conversion_error: Option<String>,
    pub update_target_book_id: Option<Uuid>,
}

/// Input for updating an ingest source record.
#[derive(Debug, Default)]
pub struct BookIngestSourceUpdate {
    pub extracted_cover: Option<Vec<u8>>,
    pub pending_authors: Option<Value>,
    pub pending_series: Option<Value>,
    pub pending_series_index: Option<f64>,
    /// Source metadata: provenance info (source_type, provider, external_id, subjects)
    pub source_metadata: Option<Value>,
    pub conversion_error: Option<String>,
}

/// Create a new ingest source record.
pub async fn create<'e, E>(exec: E, input: BookIngestSourceInput) -> Result<()>
where
    E: Executor<'e, Database = Postgres>,
{
    sqlx::query!(
        r#"
        INSERT INTO book_ingest_source (
            book_id, file_path, file_name, file_size, file_hash,
            source_mtime, source_format, extracted_cover, pending_authors,
            pending_series, pending_series_index, source_metadata,
            conversion_error, update_target_book_id
        )
        VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14)
        "#,
        input.book_id,
        input.file_path,
        input.file_name,
        input.file_size,
        input.file_hash as Option<String>,
        input.source_mtime,
        input.source_format,
        input.extracted_cover as Option<Vec<u8>>,
        input.pending_authors as Option<Value>,
        input.pending_series as Option<Value>,
        input.pending_series_index,
        input.source_metadata as Option<Value>,
        input.conversion_error as Option<String>,
        input.update_target_book_id as Option<Uuid>,
    )
    .execute(exec)
    .await?;

    Ok(())
}

/// Create an ingest source record if file hash doesn't already exist.
/// Returns the book_id if created, None if duplicate hash exists.
pub async fn create_if_hash_unique<'e, E>(
    exec: E,
    input: BookIngestSourceInput,
) -> Result<Option<Uuid>>
where
    E: Executor<'e, Database = Postgres>,
{
    let result = sqlx::query_scalar!(
        r#"
        INSERT INTO book_ingest_source (
            book_id, file_path, file_name, file_size, file_hash,
            source_mtime, source_format, extracted_cover, pending_authors,
            pending_series, pending_series_index, source_metadata,
            conversion_error, update_target_book_id
        )
        VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14)
        ON CONFLICT (file_hash) WHERE file_hash IS NOT NULL DO NOTHING
        RETURNING book_id
        "#,
        input.book_id,
        input.file_path,
        input.file_name,
        input.file_size,
        input.file_hash as Option<String>,
        input.source_mtime,
        input.source_format,
        input.extracted_cover as Option<Vec<u8>>,
        input.pending_authors as Option<Value>,
        input.pending_series as Option<Value>,
        input.pending_series_index,
        input.source_metadata as Option<Value>,
        input.conversion_error as Option<String>,
        input.update_target_book_id as Option<Uuid>,
    )
    .fetch_optional(exec)
    .await?;

    Ok(result)
}

/// Get ingest source by book ID.
pub async fn get_by_book_id<'e, E>(exec: E, book_id: Uuid) -> Result<Option<BookIngestSourceRow>>
where
    E: Executor<'e, Database = Postgres>,
{
    let row = sqlx::query_as!(
        BookIngestSourceRow,
        r#"
        SELECT book_id, file_path, file_name, file_size, file_hash, source_mtime,
               source_format, (extracted_cover IS NOT NULL) AS "has_cover!",
               pending_authors, pending_series, pending_series_index,
               source_metadata, conversion_error,
               update_target_book_id, created_at
        FROM book_ingest_source
        WHERE book_id = $1
        "#,
        book_id,
    )
    .fetch_optional(exec)
    .await?;

    Ok(row)
}

/// Update an ingest source record.
pub async fn update<'e, E>(exec: E, book_id: Uuid, input: BookIngestSourceUpdate) -> Result<bool>
where
    E: Executor<'e, Database = Postgres>,
{
    let result = sqlx::query!(
        r#"
        UPDATE book_ingest_source SET
            extracted_cover = COALESCE($2, extracted_cover),
            pending_authors = COALESCE($3, pending_authors),
            pending_series = COALESCE($4, pending_series),
            pending_series_index = COALESCE($5, pending_series_index),
            source_metadata = COALESCE($6, source_metadata),
            conversion_error = COALESCE($7, conversion_error)
        WHERE book_id = $1
        "#,
        book_id,
        input.extracted_cover as Option<Vec<u8>>,
        input.pending_authors as Option<Value>,
        input.pending_series as Option<Value>,
        input.pending_series_index,
        input.source_metadata as Option<Value>,
        input.conversion_error as Option<String>,
    )
    .execute(exec)
    .await?;

    Ok(result.rows_affected() > 0)
}

/// Delete an ingest source record.
pub async fn delete<'e, E>(exec: E, book_id: Uuid) -> Result<bool>
where
    E: Executor<'e, Database = Postgres>,
{
    let result = sqlx::query!("DELETE FROM book_ingest_source WHERE book_id = $1", book_id)
        .execute(exec)
        .await?;
    Ok(result.rows_affected() > 0)
}

/// Get the extracted cover for an ingest book.
/// Returns None if book doesn't exist or has no cover.
pub async fn get_cover<'e, E>(exec: E, book_id: Uuid) -> Result<Option<Vec<u8>>>
where
    E: Executor<'e, Database = Postgres>,
{
    let cover: Option<Option<Vec<u8>>> = sqlx::query_scalar!(
        "SELECT extracted_cover FROM book_ingest_source WHERE book_id = $1",
        book_id
    )
    .fetch_optional(exec)
    .await?;
    Ok(cover.flatten())
}

/// Update the extracted cover for an ingest book.
pub async fn update_cover<'e, E>(exec: E, book_id: Uuid, cover: &[u8]) -> Result<()>
where
    E: Executor<'e, Database = Postgres>,
{
    sqlx::query!(
        "UPDATE book_ingest_source SET extracted_cover = $2 WHERE book_id = $1",
        book_id,
        cover
    )
    .execute(exec)
    .await?;
    Ok(())
}

/// Check if a file hash already exists in book_ingest_source or book_files.
pub async fn hash_exists<'e, E>(exec: E, hash: &str) -> Result<bool>
where
    E: Executor<'e, Database = Postgres>,
{
    let exists: bool = sqlx::query_scalar!(
        r#"
        SELECT EXISTS(
            SELECT 1 FROM book_ingest_source WHERE file_hash = $1
            UNION
            SELECT 1 FROM book_files WHERE file_hash = $1
        ) AS "exists!"
        "#,
        hash,
    )
    .fetch_one(exec)
    .await?;
    Ok(exists)
}

/// Check if a file path already exists in book_ingest_source.
pub async fn exists_by_path<'e, E>(exec: E, file_path: &str) -> Result<bool>
where
    E: Executor<'e, Database = Postgres>,
{
    let exists: bool = sqlx::query_scalar!(
        r#"SELECT EXISTS(SELECT 1 FROM book_ingest_source WHERE file_path = $1) AS "exists!""#,
        file_path
    )
    .fetch_one(exec)
    .await?;
    Ok(exists)
}

/// Set the conversion error for an ingest book.
pub async fn set_conversion_error<'e, E>(exec: E, book_id: Uuid, error: &str) -> Result<()>
where
    E: Executor<'e, Database = Postgres>,
{
    sqlx::query!(
        "UPDATE book_ingest_source SET conversion_error = $2 WHERE book_id = $1",
        book_id,
        error
    )
    .execute(exec)
    .await?;
    Ok(())
}

/// Clear the conversion error for an ingest book.
pub async fn clear_conversion_error<'e, E>(exec: E, book_id: Uuid) -> Result<()>
where
    E: Executor<'e, Database = Postgres>,
{
    sqlx::query!(
        "UPDATE book_ingest_source SET conversion_error = NULL WHERE book_id = $1",
        book_id
    )
    .execute(exec)
    .await?;
    Ok(())
}

/// Row returned from ingest source queries (without cover bytes).
#[derive(Debug, Clone, serde::Serialize)]
pub struct BookIngestSourceRow {
    pub book_id: Uuid,
    pub file_path: String,
    pub file_name: String,
    pub file_size: i64,
    pub file_hash: Option<String>,
    pub source_mtime: Option<OffsetDateTime>,
    pub source_format: String,
    pub has_cover: bool,
    /// Pending authors not yet linked: [{name, hardcover_id?, image_url?}]
    pub pending_authors: Option<Value>,
    /// Pending series not yet linked: {name, hardcover_id?}
    pub pending_series: Option<Value>,
    pub pending_series_index: Option<f64>,
    /// Source metadata: provenance info (source_type, provider, external_id, subjects)
    pub source_metadata: Option<Value>,
    pub conversion_error: Option<String>,
    pub update_target_book_id: Option<Uuid>,
    pub created_at: OffsetDateTime,
}
