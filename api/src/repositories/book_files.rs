//! Book files repository operations.

use super::Result;
use serde_json::Value;
use sqlx::{Executor, Postgres};
use time::OffsetDateTime;
use uuid::Uuid;

/// Input for creating a book file.
#[derive(Debug, Default)]
pub struct BookFileInput {
    pub book_id: Uuid,
    pub format: String,
    pub size_bytes: i64,
    pub file_path: String,
    pub source_path: Option<String>,
    pub source_mtime: Option<OffsetDateTime>,
    pub file_hash: Option<String>,
    pub source_type: Option<String>,
    pub source_metadata: Option<Value>,
    pub ingest_queue_id: Option<Uuid>,
}

/// Create a new book file.
pub async fn create<'e, E>(exec: E, input: BookFileInput) -> Result<Uuid>
where
    E: Executor<'e, Database = Postgres>,
{
    let id = Uuid::now_v7();
    sqlx::query!(
        r#"
        INSERT INTO book_files (id, book_id, format, size_bytes, file_path, source_path, source_mtime,
                                file_hash, source_type, source_metadata, added_at)
        VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, COALESCE($10, '{}'::jsonb), NOW())
        "#,
        id,
        input.book_id,
        input.format,
        input.size_bytes,
        input.file_path,
        input.source_path as Option<String>,
        input.source_mtime,
        input.file_hash as Option<String>,
        input.source_type as Option<String>,
        input.source_metadata as Option<Value>,
    )
    .execute(exec)
    .await?;

    Ok(id)
}

/// Delete a book file by ID.
pub async fn delete<'e, E>(exec: E, id: Uuid) -> Result<bool>
where
    E: Executor<'e, Database = Postgres>,
{
    let result = sqlx::query!("DELETE FROM book_files WHERE id = $1", id)
        .execute(exec)
        .await?;
    Ok(result.rows_affected() > 0)
}

/// Move a book file to a different book.
pub async fn move_to_book<'e, E>(exec: E, file_id: Uuid, new_book_id: Uuid) -> Result<bool>
where
    E: Executor<'e, Database = Postgres>,
{
    let result = sqlx::query!(
        "UPDATE book_files SET book_id = $2 WHERE id = $1",
        file_id,
        new_book_id,
    )
    .execute(exec)
    .await?;

    Ok(result.rows_affected() > 0)
}

/// Get the file path for a book file by ID.
pub async fn get_path_by_id<'e, E>(exec: E, file_id: Uuid) -> Result<Option<String>>
where
    E: Executor<'e, Database = Postgres>,
{
    let path: Option<String> =
        sqlx::query_scalar!("SELECT file_path FROM book_files WHERE id = $1", file_id)
            .fetch_optional(exec)
            .await?;
    Ok(path)
}

/// Count book files whose path starts with the given directory prefix.
pub async fn count_in_directory<'e, E>(exec: E, dir_prefix: &str) -> Result<i64>
where
    E: Executor<'e, Database = Postgres>,
{
    let prefix = format!("{dir_prefix}/%");
    let count: i64 = sqlx::query_scalar!(
        r#"SELECT COUNT(*) AS "count!" FROM book_files WHERE file_path LIKE $1"#,
        prefix,
    )
    .fetch_one(exec)
    .await?;
    Ok(count)
}

/// Delete all book files for a book.
pub async fn delete_by_book<'e, E>(exec: E, book_id: Uuid) -> Result<u64>
where
    E: Executor<'e, Database = Postgres>,
{
    let result = sqlx::query!("DELETE FROM book_files WHERE book_id = $1", book_id)
        .execute(exec)
        .await?;
    Ok(result.rows_affected())
}

/// Delete all book files for multiple books.
pub async fn delete_by_books<'e, E>(exec: E, book_ids: &[Uuid]) -> Result<u64>
where
    E: Executor<'e, Database = Postgres>,
{
    let result = sqlx::query!("DELETE FROM book_files WHERE book_id = ANY($1)", book_ids)
        .execute(exec)
        .await?;
    Ok(result.rows_affected())
}

/// Update last_checked_at for a book file.
pub async fn touch<'e, E>(exec: E, id: Uuid) -> Result<()>
where
    E: Executor<'e, Database = Postgres>,
{
    sqlx::query!(
        "UPDATE book_files SET last_checked_at = NOW() WHERE id = $1",
        id
    )
    .execute(exec)
    .await?;
    Ok(())
}

/// Update file metadata when hash or path changed (used by directory scan).
pub async fn update_file_metadata<'e, E>(
    exec: E,
    id: Uuid,
    file_hash: &str,
    source_path: &str,
    size_bytes: i64,
    source_mtime: OffsetDateTime,
) -> Result<()>
where
    E: Executor<'e, Database = Postgres>,
{
    sqlx::query!(
        r#"
        UPDATE book_files
        SET file_hash = $2,
            source_path = $3,
            size_bytes = $4,
            source_mtime = $5,
            last_checked_at = NOW(),
            content_revision = gen_random_uuid()
        WHERE id = $1
        "#,
        id,
        file_hash,
        source_path,
        size_bytes,
        source_mtime,
    )
    .execute(exec)
    .await?;
    Ok(())
}

/// Find existing book file by source path.
pub async fn find_by_source_path<'e, E>(
    exec: E,
    source_path: &str,
) -> Result<Option<(Uuid, Option<OffsetDateTime>, Option<String>)>>
where
    E: Executor<'e, Database = Postgres>,
{
    let row = sqlx::query!(
        "SELECT id, source_mtime, file_hash FROM book_files WHERE source_path = $1 LIMIT 1",
        source_path,
    )
    .fetch_optional(exec)
    .await?;

    Ok(row.map(|r| (r.id, r.source_mtime, r.file_hash)))
}

/// Check if a book has files in KEPUB or EPUB format.
pub async fn has_ebook_format<'e, E>(exec: E, book_id: Uuid) -> Result<bool>
where
    E: Executor<'e, Database = Postgres>,
{
    let exists: bool = sqlx::query_scalar!(
        r#"SELECT EXISTS(SELECT 1 FROM book_files WHERE book_id = $1 AND format IN ('KEPUB', 'EPUB')) AS "exists!""#,
        book_id,
    )
    .fetch_one(exec)
    .await?;
    Ok(exists)
}

/// Count files for a book.
pub async fn count_by_book<'e, E>(exec: E, book_id: Uuid) -> Result<i64>
where
    E: Executor<'e, Database = Postgres>,
{
    let count: i64 = sqlx::query_scalar!(
        r#"SELECT COUNT(*) AS "count!" FROM book_files WHERE book_id = $1"#,
        book_id,
    )
    .fetch_one(exec)
    .await?;
    Ok(count)
}

/// Upsert a book file (insert or update if same book_id + format).
/// Used by ingest to add files from Calibre plugin.
pub async fn upsert<'e, E>(exec: E, input: BookFileInput) -> Result<Uuid>
where
    E: Executor<'e, Database = Postgres>,
{
    let id = Uuid::now_v7();
    let row_id: Uuid = sqlx::query_scalar!(
        r#"
        INSERT INTO book_files (id, book_id, format, size_bytes, file_path, source_type, source_metadata, added_at)
        VALUES ($1, $2, $3, $4, $5, $6, COALESCE($7, '{}'::jsonb), NOW())
        ON CONFLICT (book_id, format) DO UPDATE SET
            size_bytes = EXCLUDED.size_bytes,
            file_path = EXCLUDED.file_path,
            source_type = EXCLUDED.source_type,
            added_at = EXCLUDED.added_at,
            content_revision = gen_random_uuid()
        RETURNING id
        "#,
        id,
        input.book_id,
        input.format,
        input.size_bytes,
        input.file_path,
        input.source_type as Option<String>,
        input.source_metadata as Option<Value>,
    )
    .fetch_one(exec)
    .await?;

    Ok(row_id)
}

/// Extended input for full book file upsert (used by watcher ingest).
#[derive(Debug, Default)]
pub struct BookFileFullInput {
    pub book_id: Uuid,
    pub format: String,
    pub size_bytes: i64,
    pub file_path: String,
    pub source_path: Option<String>,
    pub source_mtime: Option<OffsetDateTime>,
    pub file_hash: Option<String>,
    pub source_type: Option<String>,
    pub source_metadata: Option<Value>,
}

/// Upsert a book file with all fields (insert or update if same book_id + format).
/// Used by watcher service when ingesting files.
pub async fn upsert_full<'e, E>(exec: E, input: BookFileFullInput) -> Result<Uuid>
where
    E: Executor<'e, Database = Postgres>,
{
    let id = Uuid::now_v7();
    let row_id: Uuid = sqlx::query_scalar!(
        r#"
        INSERT INTO book_files (id, book_id, format, size_bytes, file_path, source_path, source_mtime, file_hash,
                                source_type, source_metadata, added_at)
        VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, COALESCE($10, '{}'::jsonb), NOW())
        ON CONFLICT (book_id, format) DO UPDATE SET
            size_bytes = EXCLUDED.size_bytes,
            file_path = EXCLUDED.file_path,
            source_path = EXCLUDED.source_path,
            source_mtime = EXCLUDED.source_mtime,
            file_hash = EXCLUDED.file_hash,
            source_type = EXCLUDED.source_type,
            source_metadata = EXCLUDED.source_metadata,
            added_at = EXCLUDED.added_at,
            content_revision = gen_random_uuid()
        RETURNING id
        "#,
        id,
        input.book_id,
        input.format,
        input.size_bytes,
        input.file_path,
        input.source_path as Option<String>,
        input.source_mtime,
        input.file_hash as Option<String>,
        input.source_type as Option<String>,
        input.source_metadata as Option<Value>,
    )
    .fetch_one(exec)
    .await?;

    Ok(row_id)
}

/// Check if a file hash exists in book_files.
pub async fn hash_exists<'e, E>(exec: E, hash: &str) -> Result<bool>
where
    E: Executor<'e, Database = Postgres>,
{
    let exists: bool = sqlx::query_scalar!(
        r#"SELECT EXISTS(SELECT 1 FROM book_files WHERE file_hash = $1) AS "exists!""#,
        hash
    )
    .fetch_one(exec)
    .await?;
    Ok(exists)
}

/// File info for availability checking.
#[derive(Debug)]
pub struct FileAvailabilityInfo {
    pub id: Uuid,
    pub book_id: Uuid,
    pub file_path: String,
    pub available: bool,
}

/// List files for availability checking, optionally filtered by book.
pub async fn list_for_availability_check<'e, E>(
    exec: E,
    book_id: Option<Uuid>,
) -> Result<Vec<FileAvailabilityInfo>>
where
    E: Executor<'e, Database = Postgres>,
{
    let rows = sqlx::query_as!(
        FileAvailabilityInfo,
        r#"
        SELECT id, book_id, file_path, available
        FROM book_files
        WHERE ($1::uuid IS NULL OR book_id = $1)
        "#,
        book_id as Option<Uuid>,
    )
    .fetch_all(exec)
    .await?;

    Ok(rows)
}

/// Mark a file as available (was unavailable, now exists again).
pub async fn set_available<'e, E>(exec: E, id: Uuid) -> Result<()>
where
    E: Executor<'e, Database = Postgres>,
{
    sqlx::query!(
        r#"
        UPDATE book_files
        SET available = TRUE,
            last_checked_at = NOW(),
            unavailable_since = NULL
        WHERE id = $1
        "#,
        id,
    )
    .execute(exec)
    .await?;
    Ok(())
}

/// Mark a file as unavailable (file no longer exists).
pub async fn set_unavailable<'e, E>(exec: E, id: Uuid) -> Result<()>
where
    E: Executor<'e, Database = Postgres>,
{
    sqlx::query!(
        r#"
        UPDATE book_files
        SET available = FALSE,
            last_checked_at = NOW(),
            unavailable_since = NOW()
        WHERE id = $1
        "#,
        id,
    )
    .execute(exec)
    .await?;
    Ok(())
}

/// File info for metadata writing (format + path).
#[derive(Debug)]
pub struct MetadataWriteFileInfo {
    pub id: Uuid,
    pub format: String,
    pub file_path: String,
}

/// List KEPUB and EPUB files for a book (for metadata writing).
pub async fn list_for_metadata_write<'e, E>(
    exec: E,
    book_id: Uuid,
) -> Result<Vec<MetadataWriteFileInfo>>
where
    E: Executor<'e, Database = Postgres>,
{
    let rows = sqlx::query_as!(
        MetadataWriteFileInfo,
        r#"
        SELECT id, format, file_path
        FROM book_files
        WHERE book_id = $1 AND format IN ('KEPUB', 'EPUB', 'AZW', 'AZW3', 'AZW4', 'MOBI', 'PDF', 'FB2', 'DOCX', 'LRF', 'PDB', 'PRC', 'RTF')
        "#,
        book_id,
    )
    .fetch_all(exec)
    .await?;

    Ok(rows)
}

/// Get file path by book ID and format.
pub async fn get_path_by_format<'e, E>(
    exec: E,
    book_id: Uuid,
    format: &str,
) -> Result<Option<String>>
where
    E: Executor<'e, Database = Postgres>,
{
    let fmt = format.to_uppercase();
    let path: Option<String> = sqlx::query_scalar!(
        "SELECT file_path FROM book_files WHERE book_id = $1 AND format = $2",
        book_id,
        fmt
    )
    .fetch_optional(exec)
    .await?;
    Ok(path)
}

/// Which of `ids` still name a book file.
pub async fn existing_ids<'e, E>(exec: E, ids: &[Uuid]) -> Result<Vec<Uuid>>
where
    E: Executor<'e, Database = Postgres>,
{
    let found = sqlx::query_scalar!("SELECT id FROM book_files WHERE id = ANY($1)", ids)
        .fetch_all(exec)
        .await?;
    Ok(found)
}

/// Every book that has a file in one of `formats`.
pub async fn list_book_ids_with_formats<'e, E>(exec: E, formats: &[&str]) -> Result<Vec<Uuid>>
where
    E: Executor<'e, Database = Postgres>,
{
    let formats: Vec<String> = formats.iter().map(|f| f.to_uppercase()).collect();
    let ids = sqlx::query_scalar!(
        "SELECT DISTINCT book_id FROM book_files WHERE format = ANY($1)",
        &formats,
    )
    .fetch_all(exec)
    .await?;
    Ok(ids)
}

/// A book's file row of one format, with the revision that changes whenever
/// its content is replaced.
pub async fn get_revision_by_format<'e, E>(
    exec: E,
    book_id: Uuid,
    format: &str,
) -> Result<Option<(Uuid, String, Uuid)>>
where
    E: Executor<'e, Database = Postgres>,
{
    let fmt = format.to_uppercase();
    let row = sqlx::query!(
        "SELECT id, file_path, content_revision FROM book_files WHERE book_id = $1 AND format = $2",
        book_id,
        fmt
    )
    .fetch_optional(exec)
    .await?;
    Ok(row.map(|r| (r.id, r.file_path, r.content_revision)))
}

/// Get path of any available file for a book (prefers EPUB, then any).
pub async fn get_any_available_path<'e, E>(exec: E, book_id: Uuid) -> Result<Option<String>>
where
    E: Executor<'e, Database = Postgres>,
{
    let path: Option<String> = sqlx::query_scalar!(
        r#"
        SELECT file_path FROM book_files
        WHERE book_id = $1 AND available = TRUE
        ORDER BY CASE format WHEN 'EPUB' THEN 1 WHEN 'KEPUB' THEN 2 ELSE 3 END
        LIMIT 1
        "#,
        book_id,
    )
    .fetch_optional(exec)
    .await?;
    Ok(path)
}

/// Get a file's ID and path for a book, preferring EPUB format.
pub async fn get_preferred_file<'e, E>(exec: E, book_id: Uuid) -> Result<Option<(Uuid, String)>>
where
    E: Executor<'e, Database = Postgres>,
{
    let row = sqlx::query!(
        r#"
        SELECT id, file_path FROM book_files
        WHERE book_id = $1 AND available = TRUE
        ORDER BY CASE format WHEN 'EPUB' THEN 1 WHEN 'KEPUB' THEN 2 ELSE 3 END
        LIMIT 1
        "#,
        book_id,
    )
    .fetch_optional(exec)
    .await?;
    Ok(row.map(|r| (r.id, r.file_path)))
}

/// Get a file's ID, path, and format for a book, excluding specified formats.
/// Returns the best available format that isn't in the exclusion list.
/// Used by Kindle delivery which can't handle KEPUB format.
pub async fn get_preferred_file_excluding<'e, E>(
    exec: E,
    book_id: Uuid,
    exclude_formats: &[&str],
) -> Result<Option<(Uuid, String, String)>>
where
    E: Executor<'e, Database = Postgres>,
{
    // Convert exclusion list to uppercase for comparison
    let exclude_upper: Vec<String> = exclude_formats.iter().map(|f| f.to_uppercase()).collect();

    let row = sqlx::query!(
        r#"
        SELECT id, file_path, format FROM book_files
        WHERE book_id = $1 AND available = TRUE AND format != ALL($2)
        ORDER BY CASE format WHEN 'EPUB' THEN 1 ELSE 2 END
        LIMIT 1
        "#,
        book_id,
        &exclude_upper,
    )
    .fetch_optional(exec)
    .await?;
    Ok(row.map(|r| (r.id, r.file_path, r.format)))
}

/// Get the best Kindle-compatible file for a book.
/// Prefers MOBI > AZW3 > AZW > EPUB > KEPUB (native Kindle formats first, then convertible).
/// KEPUB is lowest priority since it requires conversion but does work on Kindle.
pub async fn get_kindle_file<'e, E>(
    exec: E,
    book_id: Uuid,
) -> Result<Option<(Uuid, String, String)>>
where
    E: Executor<'e, Database = Postgres>,
{
    let row = sqlx::query!(
        r#"
        SELECT id, file_path, format FROM book_files
        WHERE book_id = $1 AND available = TRUE
        ORDER BY CASE format
            WHEN 'MOBI' THEN 1
            WHEN 'AZW3' THEN 2
            WHEN 'AZW' THEN 3
            WHEN 'EPUB' THEN 4
            WHEN 'KEPUB' THEN 5
            ELSE 6
        END
        LIMIT 1
        "#,
        book_id,
    )
    .fetch_optional(exec)
    .await?;
    Ok(row.map(|r| (r.id, r.file_path, r.format)))
}

/// Check if a book has a file in any of the specified formats.
pub async fn has_format<'e, E>(exec: E, book_id: Uuid, formats: &[&str]) -> Result<bool>
where
    E: Executor<'e, Database = Postgres>,
{
    let formats_upper: Vec<String> = formats.iter().map(|f| f.to_uppercase()).collect();
    let exists: bool = sqlx::query_scalar!(
        r#"SELECT EXISTS(SELECT 1 FROM book_files WHERE book_id = $1 AND format = ANY($2) AND available = TRUE) AS "exists!""#,
        book_id,
        &formats_upper,
    )
    .fetch_one(exec)
    .await?;
    Ok(exists)
}

/// Full file info for listing.
#[derive(Debug)]
pub struct BookFileListInfo {
    pub id: Uuid,
    pub format: String,
    pub size_bytes: i64,
    pub file_path: String,
    pub source_type: Option<String>,
    pub source_metadata: Option<Value>,
    pub added_at: OffsetDateTime,
    pub available: bool,
}

/// List all files for a book with full details.
pub async fn list_by_book<'e, E>(exec: E, book_id: Uuid) -> Result<Vec<BookFileListInfo>>
where
    E: Executor<'e, Database = Postgres>,
{
    let rows = sqlx::query_as!(
        BookFileListInfo,
        r#"
        SELECT id, format, size_bytes, file_path, source_type, source_metadata, added_at, available
        FROM book_files
        WHERE book_id = $1
        ORDER BY added_at DESC
        "#,
        book_id,
    )
    .fetch_all(exec)
    .await?;

    Ok(rows)
}

/// File info for deletion operations.
#[derive(Debug)]
pub struct BookFileDeleteInfo {
    pub id: Uuid,
    pub format: String,
    pub source_path: Option<String>,
    pub file_path: Option<String>,
}

/// Get a file by ID, verifying it belongs to the specified book.
pub async fn get_by_id_for_book<'e, E>(
    exec: E,
    file_id: Uuid,
    book_id: Uuid,
) -> Result<Option<BookFileDeleteInfo>>
where
    E: Executor<'e, Database = Postgres>,
{
    let row = sqlx::query_as!(
        BookFileDeleteInfo,
        r#"
        SELECT id, format, source_path, file_path
        FROM book_files
        WHERE id = $1 AND book_id = $2
        "#,
        file_id,
        book_id,
    )
    .fetch_optional(exec)
    .await?;

    Ok(row)
}

/// File format and size info.
#[derive(Debug, serde::Deserialize)]
pub struct FileFormatInfo {
    pub format: String,
    pub size_bytes: i64,
    #[serde(default = "Uuid::nil")]
    pub content_revision: Uuid,
}

/// List files with format and size for a book (for Kobo metadata).
pub async fn list_format_and_size<'e, E>(exec: E, book_id: Uuid) -> Result<Vec<FileFormatInfo>>
where
    E: Executor<'e, Database = Postgres>,
{
    let rows = sqlx::query_as!(
        FileFormatInfo,
        "SELECT format, size_bytes, content_revision FROM book_files WHERE book_id = $1",
        book_id,
    )
    .fetch_all(exec)
    .await?;

    Ok(rows)
}

/// Update only the size_bytes for a file (no content_revision bump).
/// Used after metadata writes to keep sync response sizes accurate.
pub async fn update_size_bytes<'e, E>(exec: E, id: Uuid, size_bytes: i64) -> Result<()>
where
    E: Executor<'e, Database = Postgres>,
{
    sqlx::query!(
        "UPDATE book_files SET size_bytes = $2 WHERE id = $1",
        id,
        size_bytes
    )
    .execute(exec)
    .await?;
    Ok(())
}

/// Update the file path for a book file (used when moving to a media root).
pub async fn update_path<'e, E>(exec: E, id: Uuid, new_path: &str) -> Result<()>
where
    E: Executor<'e, Database = Postgres>,
{
    sqlx::query!(
        r#"
        UPDATE book_files SET file_path = $2, content_revision = gen_random_uuid()
        WHERE id = $1
        "#,
        id,
        new_path,
    )
    .execute(exec)
    .await?;
    Ok(())
}

// =============================================================================
// Repair
// =============================================================================

/// Record that a file's bytes were replaced in place.
///
/// Bumps `content_revision`, which is what the Kobo reads as `RevisionId` — a
/// device that already holds the book will not fetch the repaired copy unless
/// that changes. Provenance (`source_type`, `source_metadata`) is deliberately
/// left as it was: a repair re-makes the file from an original, so where the
/// book came from has not changed.
///
/// `source_path`/`source_mtime` move the staleness baseline to the original
/// this re-make used; `None` keeps whatever was recorded.
pub async fn mark_rewritten<'e, E>(
    exec: E,
    id: Uuid,
    size_bytes: i64,
    source_path: Option<&str>,
    source_mtime: Option<OffsetDateTime>,
) -> Result<()>
where
    E: Executor<'e, Database = Postgres>,
{
    sqlx::query!(
        r#"
        UPDATE book_files
        SET size_bytes = $2,
            content_revision = gen_random_uuid(),
            source_path = COALESCE($3, source_path),
            source_mtime = COALESCE($4, source_mtime)
        WHERE id = $1
        "#,
        id,
        size_bytes,
        source_path,
        source_mtime,
    )
    .execute(exec)
    .await?;
    Ok(())
}

/// A file the conversion pipeline produced, and the book it belongs to.
#[derive(Debug)]
pub struct DerivedFileInfo {
    pub id: Uuid,
    pub book_id: Uuid,
    pub format: String,
    pub file_path: String,
    pub source_type: String,
    pub book_title: String,
    /// The original this was converted from, if the row recorded one.
    pub source_path: Option<String>,
    /// The source's mtime at conversion time — the staleness baseline.
    pub source_mtime: Option<OffsetDateTime>,
    /// Last confirmed present on disk. Derived files that are not are still
    /// re-makeable from their original, so the sweep includes them.
    pub available: bool,
}

/// List every book file the conversion pipeline produced, so it can be checked
/// against the current pipeline and re-made if it is damaged, stale, or gone.
///
/// Scoped to files we made: a KEPUB is always our output, and an EPUB is ours
/// when it was converted rather than scanned. Files that came in from a media
/// root are the user's own copies — if one of those is malformed that is the
/// source, and re-writing it over the top would destroy the original.
///
/// Files marked unavailable are included: a derived file that has gone missing
/// from disk is still re-makeable from its original, which is exactly what the
/// repair should do.
pub async fn list_derived<'e, E>(exec: E) -> Result<Vec<DerivedFileInfo>>
where
    E: Executor<'e, Database = Postgres>,
{
    let rows = sqlx::query_as!(
        DerivedFileInfo,
        r#"
        SELECT bf.id, bf.book_id, bf.format, bf.file_path, bf.source_type,
               bf.source_path, bf.source_mtime, bf.available, b.title AS book_title
        FROM book_files bf
        JOIN books b ON b.id = bf.book_id
        WHERE (bf.format = 'KEPUB'
               OR (bf.format = 'EPUB'
                   AND bf.source_type IN ('on_demand_conversion', 'format_conversion')))
        ORDER BY b.title
        "#,
    )
    .fetch_all(exec)
    .await?;

    Ok(rows)
}

/// The best file to re-convert a book from: an original the pipeline did not
/// write, preferring EPUB (no format round-trip) then the Kindle formats.
pub async fn get_conversion_source<'e, E>(
    exec: E,
    book_id: Uuid,
    exclude_file_id: Uuid,
) -> Result<Option<(String, String)>>
where
    E: Executor<'e, Database = Postgres>,
{
    let row = sqlx::query!(
        r#"
        SELECT format, file_path
        FROM book_files
        WHERE book_id = $1
          AND id <> $2
          AND available
          AND format <> 'KEPUB'
          AND source_type NOT IN ('on_demand_conversion', 'format_conversion')
        ORDER BY CASE format
                     WHEN 'EPUB' THEN 0
                     WHEN 'AZW3' THEN 1
                     WHEN 'AZW'  THEN 2
                     WHEN 'MOBI' THEN 3
                     ELSE 4
                 END
        LIMIT 1
        "#,
        book_id,
        exclude_file_id,
    )
    .fetch_optional(exec)
    .await?;

    Ok(row.map(|r| (r.format, r.file_path)))
}

// =============================================================================
// Kobload Source Matching
// =============================================================================

/// File with source metadata for checking previous downloads.
#[derive(Debug)]
pub struct FileSourceInfo {
    pub id: Uuid,
    pub book_id: Uuid,
    pub added_at: OffsetDateTime,
    pub source_metadata: Value,
    pub book_title: String,
}

/// List files with kobload source metadata for download matching.
pub async fn list_with_kobload_source<'e, E>(exec: E) -> Result<Vec<FileSourceInfo>>
where
    E: Executor<'e, Database = Postgres>,
{
    let rows = sqlx::query_as!(
        FileSourceInfo,
        r#"
        SELECT bf.id, bf.book_id, bf.added_at, bf.source_metadata AS "source_metadata!", b.title AS book_title
        FROM book_files bf
        JOIN books b ON b.id = bf.book_id
        WHERE bf.source_type = 'kobload'
          AND (bf.source_metadata->>'info_hash' IS NOT NULL
               OR bf.source_metadata->>'release_title' IS NOT NULL)
        "#,
    )
    .fetch_all(exec)
    .await?;

    Ok(rows)
}
