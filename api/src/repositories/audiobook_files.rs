//! Audiobook files repository — stores multiple audio files per book.

use super::Result;
use serde_json::Value;
use sqlx::{Executor, Postgres};
use time::OffsetDateTime;
use uuid::Uuid;

/// Input for creating/upserting an audiobook file.
#[derive(Debug, Default)]
pub struct AudiobookFileInput {
    pub book_id: Uuid,
    pub format: String,
    pub file_path: String,
    pub file_size: i64,
    pub file_hash: Option<String>,
    pub duration_seconds: Option<f64>,
    pub bitrate: Option<i32>,
    pub codec: Option<String>,
    pub channels: Option<i32>,
    pub sample_rate: Option<i32>,
    pub track_number: Option<i32>,
    pub disc_number: Option<i32>,
    pub source_path: Option<String>,
    pub source_mtime: Option<OffsetDateTime>,
    pub source_type: Option<String>,
    pub source_metadata: Option<Value>,
}

/// Create a new audiobook file with auto-assigned file_index (MAX+1 within book).
pub async fn create<'e, E>(exec: E, input: AudiobookFileInput) -> Result<Uuid>
where
    E: Executor<'e, Database = Postgres>,
{
    let id = Uuid::now_v7();
    sqlx::query_scalar!(
        r#"
        INSERT INTO audiobook_files (
            id, book_id, file_index, format, file_path, file_size, file_hash,
            duration_seconds, bitrate, codec, channels, sample_rate,
            track_number, disc_number,
            source_path, source_mtime, source_type, source_metadata, added_at
        )
        VALUES (
            $1, $2,
            COALESCE((SELECT MAX(file_index) + 1 FROM audiobook_files WHERE book_id = $2), 0),
            $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15, $16,
            COALESCE($17, '{}'::jsonb), NOW()
        )
        RETURNING id
        "#,
        id,
        input.book_id,
        input.format,
        input.file_path,
        input.file_size,
        input.file_hash as Option<String>,
        input.duration_seconds,
        input.bitrate,
        input.codec as Option<String>,
        input.channels,
        input.sample_rate,
        input.track_number,
        input.disc_number,
        input.source_path as Option<String>,
        input.source_mtime,
        input.source_type as Option<String>,
        input.source_metadata as Option<Value>,
    )
    .fetch_one(exec)
    .await?;

    Ok(id)
}

/// Upsert an audiobook file by path (for re-ingest / re-scan).
/// On conflict with (book_id, file_path), updates metadata.
pub async fn upsert_by_path(pool: &sqlx::PgPool, input: AudiobookFileInput) -> Result<Uuid> {
    let id = Uuid::now_v7();
    let row_id: Uuid = sqlx::query_scalar!(
        r#"
        INSERT INTO audiobook_files (
            id, book_id, file_index, format, file_path, file_size, file_hash,
            duration_seconds, bitrate, codec, channels, sample_rate,
            track_number, disc_number,
            source_path, source_mtime, source_type, source_metadata, added_at
        )
        VALUES (
            $1, $2,
            COALESCE((SELECT MAX(file_index) + 1 FROM audiobook_files WHERE book_id = $2), 0),
            $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15, $16,
            COALESCE($17, '{}'::jsonb), NOW()
        )
        ON CONFLICT (book_id, file_path) DO UPDATE SET
            file_size = EXCLUDED.file_size,
            file_hash = COALESCE(EXCLUDED.file_hash, audiobook_files.file_hash),
            duration_seconds = COALESCE(EXCLUDED.duration_seconds, audiobook_files.duration_seconds),
            bitrate = COALESCE(EXCLUDED.bitrate, audiobook_files.bitrate),
            codec = COALESCE(EXCLUDED.codec, audiobook_files.codec),
            channels = COALESCE(EXCLUDED.channels, audiobook_files.channels),
            sample_rate = COALESCE(EXCLUDED.sample_rate, audiobook_files.sample_rate),
            track_number = COALESCE(EXCLUDED.track_number, audiobook_files.track_number),
            disc_number = COALESCE(EXCLUDED.disc_number, audiobook_files.disc_number),
            source_path = COALESCE(EXCLUDED.source_path, audiobook_files.source_path),
            source_mtime = COALESCE(EXCLUDED.source_mtime, audiobook_files.source_mtime),
            source_type = COALESCE(EXCLUDED.source_type, audiobook_files.source_type),
            source_metadata = COALESCE(EXCLUDED.source_metadata, audiobook_files.source_metadata),
            available = TRUE,
            unavailable_since = NULL
        RETURNING id AS "id!"
        "#,
        id,
        input.book_id,
        input.format,
        input.file_path,
        input.file_size,
        input.file_hash as Option<String>,
        input.duration_seconds,
        input.bitrate,
        input.codec as Option<String>,
        input.channels,
        input.sample_rate,
        input.track_number,
        input.disc_number,
        input.source_path as Option<String>,
        input.source_mtime,
        input.source_type as Option<String>,
        input.source_metadata as Option<Value>,
    )
    .fetch_one(pool)
    .await?;

    Ok(row_id)
}

/// Row returned by list_by_book.
#[derive(Debug)]
pub struct AudiobookFileInfo {
    pub id: Uuid,
    pub file_index: i32,
    pub format: String,
    pub file_path: String,
    pub file_size: i64,
    pub duration_seconds: Option<f64>,
    pub available: bool,
    pub intro_end_seconds: Option<f64>,
    pub outro_start_seconds: Option<f64>,
}

/// List all audio files for a book, ordered by file_index then file_path.
/// The file_path fallback ensures correct order even if file_index was assigned
/// non-deterministically during bulk operations.
pub async fn list_by_book<'e, E>(exec: E, book_id: Uuid) -> Result<Vec<AudiobookFileInfo>>
where
    E: Executor<'e, Database = Postgres>,
{
    let rows = sqlx::query_as!(
        AudiobookFileInfo,
        r#"
        SELECT id, file_index, format, file_path, file_size, duration_seconds, available,
               intro_end_seconds, outro_start_seconds
        FROM audiobook_files
        WHERE book_id = $1
        ORDER BY file_index, file_path
        "#,
        book_id,
    )
    .fetch_all(exec)
    .await?;

    Ok(rows)
}

/// Minimal info for streaming endpoint — verify file belongs to book.
#[derive(Debug)]
pub struct AudiobookFileStreamInfo {
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
) -> Result<Option<AudiobookFileStreamInfo>>
where
    E: Executor<'e, Database = Postgres>,
{
    let row = sqlx::query_as!(
        AudiobookFileStreamInfo,
        r#"
        SELECT id, format, source_path, file_path AS "file_path?"
        FROM audiobook_files
        WHERE id = $1 AND book_id = $2
        "#,
        file_id,
        book_id,
    )
    .fetch_optional(exec)
    .await?;

    Ok(row)
}

/// Count audio files for a book.
pub async fn count_by_book<'e, E>(exec: E, book_id: Uuid) -> Result<i64>
where
    E: Executor<'e, Database = Postgres>,
{
    let count = sqlx::query_scalar!(
        r#"SELECT COUNT(*) AS "count!" FROM audiobook_files WHERE book_id = $1"#,
        book_id,
    )
    .fetch_one(exec)
    .await?;

    Ok(count)
}

/// Delete an audiobook file by ID.
pub async fn delete<'e, E>(exec: E, id: Uuid) -> Result<bool>
where
    E: Executor<'e, Database = Postgres>,
{
    let result = sqlx::query!("DELETE FROM audiobook_files WHERE id = $1", id)
        .execute(exec)
        .await?;
    Ok(result.rows_affected() > 0)
}

/// Delete all audiobook files for a book.
pub async fn delete_by_book<'e, E>(exec: E, book_id: Uuid) -> Result<u64>
where
    E: Executor<'e, Database = Postgres>,
{
    let result = sqlx::query!("DELETE FROM audiobook_files WHERE book_id = $1", book_id)
        .execute(exec)
        .await?;
    Ok(result.rows_affected())
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
        FROM audiobook_files
        WHERE ($1::uuid IS NULL OR book_id = $1)
        "#,
        book_id as Option<Uuid>,
    )
    .fetch_all(exec)
    .await?;

    Ok(rows)
}

/// Mark a file as available.
pub async fn set_available<'e, E>(exec: E, id: Uuid) -> Result<()>
where
    E: Executor<'e, Database = Postgres>,
{
    sqlx::query!(
        r#"
        UPDATE audiobook_files
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

/// Mark a file as unavailable.
pub async fn set_unavailable<'e, E>(exec: E, id: Uuid) -> Result<()>
where
    E: Executor<'e, Database = Postgres>,
{
    sqlx::query!(
        r#"
        UPDATE audiobook_files
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

#[derive(Debug, Default)]
pub struct ProbeUpdate {
    pub duration_seconds: Option<f64>,
    pub bitrate: Option<i32>,
    pub codec: Option<String>,
    pub channels: Option<i32>,
    pub sample_rate: Option<i32>,
    pub file_size: Option<i64>,
}

/// Update an audiobook file with probe results (duration, bitrate, codec, etc.).
pub async fn update_probe_data<'e, E>(exec: E, id: Uuid, update: &ProbeUpdate) -> Result<()>
where
    E: Executor<'e, Database = Postgres>,
{
    sqlx::query!(
        r#"
        UPDATE audiobook_files SET
            duration_seconds = COALESCE($2, duration_seconds),
            bitrate = COALESCE($3, bitrate),
            codec = COALESCE($4, codec),
            channels = COALESCE($5, channels),
            sample_rate = COALESCE($6, sample_rate),
            file_size = COALESCE($7, file_size)
        WHERE id = $1
        "#,
        id,
        update.duration_seconds,
        update.bitrate,
        update.codec.clone() as Option<String>,
        update.channels,
        update.sample_rate,
        update.file_size,
    )
    .execute(exec)
    .await?;
    Ok(())
}

/// Update the file path for an audiobook file (used when moving to a media root).
pub async fn update_file_path<'e, E>(exec: E, id: Uuid, new_path: &str) -> Result<()>
where
    E: Executor<'e, Database = Postgres>,
{
    sqlx::query!(
        "UPDATE audiobook_files SET file_path = $2 WHERE id = $1",
        id,
        new_path,
    )
    .execute(exec)
    .await?;
    Ok(())
}

/// Recompute file_index for all files of a book using natural sort order.
///
/// Sort priority: disc_number, track_number, last numeric sequence in
/// filename stem (compared as integer), file_path lexicographic fallback.
pub async fn reindex_file_order(pool: &sqlx::PgPool, book_id: Uuid) -> Result<()> {
    // Two-step update to avoid UNIQUE(book_id, file_index) conflicts:
    // 1. Shift all indices to negative
    sqlx::query("UPDATE audiobook_files SET file_index = -(file_index + 1) WHERE book_id = $1")
        .bind(book_id)
        .execute(pool)
        .await?;
    // 2. Assign correct 0-based indices using natural sort
    sqlx::query(
        r#"
        WITH numbered AS (
            SELECT id,
                   ROW_NUMBER() OVER (
                       ORDER BY disc_number ASC NULLS LAST,
                              track_number ASC NULLS LAST,
                              NULLIF(
                                  (regexp_match(
                                      regexp_replace(
                                          regexp_replace(file_path, '^.*/', ''),
                                          '\.[^.]+$', ''
                                      ),
                                      '(\d+)(?!.*\d)'
                                  ))[1],
                                  NULL
                              )::int ASC NULLS LAST,
                              file_path ASC
                   ) - 1 AS new_index
            FROM audiobook_files
            WHERE book_id = $1
        )
        UPDATE audiobook_files af
        SET file_index = n.new_index
        FROM numbered n
        WHERE af.id = n.id
        "#,
    )
    .bind(book_id)
    .execute(pool)
    .await?;
    Ok(())
}

/// Update the LUFS (loudness) measurement for an audiobook file.
pub async fn update_lufs<'e, E>(exec: E, id: Uuid, lufs: Option<f64>) -> Result<bool>
where
    E: Executor<'e, Database = Postgres>,
{
    let result = sqlx::query("UPDATE audiobook_files SET lufs = $2 WHERE id = $1")
        .bind(id)
        .bind(lufs)
        .execute(exec)
        .await?;
    Ok(result.rows_affected() > 0)
}

/// Bump `media_cache_version` + `media_cache_generated_at` after a successful
/// `media_analyze` pass.
pub async fn set_media_cache_version<'e, E>(exec: E, id: Uuid, version: i32) -> Result<()>
where
    E: Executor<'e, Database = Postgres>,
{
    sqlx::query!(
        "UPDATE audiobook_files SET media_cache_version = $2, media_cache_generated_at = NOW() WHERE id = $1",
        id,
        version,
    )
    .execute(exec)
    .await?;
    Ok(())
}

/// Current media-cache pipeline version recorded for a file (None if the row is
/// gone). 0 means the cache was invalidated and must be regenerated.
pub async fn get_media_cache_version<'e, E>(exec: E, id: Uuid) -> Result<Option<i32>>
where
    E: Executor<'e, Database = Postgres>,
{
    let v = sqlx::query_scalar!(
        "SELECT media_cache_version FROM audiobook_files WHERE id = $1",
        id,
    )
    .fetch_optional(exec)
    .await?;
    Ok(v)
}

/// Find available audiobook files whose media cache is older than the current
/// pipeline version. Used by the V2 backfill walker.
pub async fn list_needing_media_cache<'e, E>(exec: E, current_version: i32) -> Result<Vec<Uuid>>
where
    E: Executor<'e, Database = Postgres>,
{
    let rows = sqlx::query_scalar!(
        r#"SELECT af.id FROM audiobook_files af
         JOIN media_root_files mrf
           ON (mrf.file_path = af.file_path OR mrf.file_path = af.source_path)
           AND mrf.available = true
         WHERE af.media_cache_version < $1
           AND af.duration_seconds IS NOT NULL
           AND af.available = true
         ORDER BY af.added_at DESC"#,
        current_version,
    )
    .fetch_all(exec)
    .await?;
    Ok(rows)
}

/// Find an audiobook file by source_path.
pub async fn find_by_source_path<'e, E>(exec: E, source_path: &str) -> Result<Option<Uuid>>
where
    E: Executor<'e, Database = Postgres>,
{
    let id = sqlx::query_scalar!(
        r#"SELECT id FROM audiobook_files WHERE source_path = $1 LIMIT 1"#,
        source_path,
    )
    .fetch_optional(exec)
    .await?;

    Ok(id)
}
