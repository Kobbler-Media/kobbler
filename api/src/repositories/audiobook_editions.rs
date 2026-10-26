use super::Result;
use sqlx::{Executor, Postgres, Transaction};
use uuid::Uuid;

pub async fn copy_book(
    tx: &mut Transaction<'_, Postgres>,
    source: Uuid,
    narrator: Option<&str>,
    abridged: Option<bool>,
) -> Result<Uuid> {
    let id = Uuid::now_v7();
    sqlx::query!(
        r#"
        INSERT INTO books
        SELECT (jsonb_populate_record(NULL::books, to_jsonb(b) || jsonb_build_object(
            'id', $2::uuid,
            'narrator', COALESCE($3::text, b.narrator),
            'is_abridged', COALESCE($4::boolean, b.is_abridged),
            'identifiers', COALESCE(b.identifiers, '{}'::jsonb) - 'asin' - 'audible_asin',
            'created_at', NOW(),
            'updated_at', NOW(),
            'first_seen_at', NOW()
        ))).*
        FROM books b
        WHERE b.id = $1
        "#,
        source,
        id,
        narrator,
        abridged,
    )
    .execute(&mut **tx)
    .await?;
    sqlx::query!(
        "INSERT INTO book_authors (book_id, author_id, role)
         SELECT $2, author_id, role FROM book_authors WHERE book_id = $1",
        source,
        id,
    )
    .execute(&mut **tx)
    .await?;
    sqlx::query!(
        "INSERT INTO book_series (book_id, series_id, series_index, is_primary)
         SELECT $2, series_id, series_index, is_primary FROM book_series WHERE book_id = $1",
        source,
        id,
    )
    .execute(&mut **tx)
    .await?;
    sqlx::query!(
        "INSERT INTO book_genres (book_id, genre_id)
         SELECT $2, genre_id FROM book_genres WHERE book_id = $1",
        source,
        id,
    )
    .execute(&mut **tx)
    .await?;
    sqlx::query!(
        "INSERT INTO entitlements (id, book_id, scope_type, scope_id)
         SELECT gen_random_uuid(), $2, scope_type, scope_id
         FROM entitlements WHERE book_id = $1 AND NOT is_removed",
        source,
        id,
    )
    .execute(&mut **tx)
    .await?;
    Ok(id)
}

pub async fn move_files(
    tx: &mut Transaction<'_, Postgres>,
    files: &[Uuid],
    target: Uuid,
) -> Result<()> {
    sqlx::query!(
        "UPDATE media_root_files SET entity_id = $2, updated_at = NOW()
         WHERE entity_type = 'book'
           AND file_path IN (
               SELECT file_path FROM audiobook_files WHERE id = ANY($1)
               UNION SELECT source_path FROM audiobook_files WHERE id = ANY($1) AND source_path IS NOT NULL
           )",
        files,
        target,
    )
    .execute(&mut **tx)
    .await?;
    sqlx::query!(
        "UPDATE audiobook_files SET book_id = $2, file_index = file_index + 1000000 WHERE id = ANY($1)",
        files,
        target,
    )
    .execute(&mut **tx)
    .await?;
    sqlx::query!(
        "UPDATE audiobook_bookmarks SET book_id = $2 WHERE audiobook_file_id = ANY($1)",
        files,
        target,
    )
    .execute(&mut **tx)
    .await?;
    sqlx::query!(
        "DELETE FROM cross_format_sync WHERE audio_file_id = ANY($1)",
        files,
    )
    .execute(&mut **tx)
    .await?;
    Ok(())
}

pub async fn drop_files(tx: &mut Transaction<'_, Postgres>, files: &[Uuid]) -> Result<()> {
    sqlx::query!(
        "UPDATE media_root_files SET match_status = 'ignored', updated_at = NOW()
         WHERE file_path IN (
             SELECT file_path FROM audiobook_files WHERE id = ANY($1)
             UNION SELECT source_path FROM audiobook_files WHERE id = ANY($1) AND source_path IS NOT NULL
         )",
        files,
    )
    .execute(&mut **tx)
    .await?;
    sqlx::query!("DELETE FROM audiobook_files WHERE id = ANY($1)", files)
        .execute(&mut **tx)
        .await?;
    Ok(())
}

pub struct AudioPosition {
    pub state_id: Uuid,
    pub location_value: String,
}

pub async fn audio_positions<'e, E>(exec: E, book_id: Uuid) -> Result<Vec<AudioPosition>>
where
    E: Executor<'e, Database = Postgres>,
{
    let rows = sqlx::query_as!(
        AudioPosition,
        r#"
        SELECT rs.id AS state_id, rb.location_value AS "location_value!"
        FROM reading_states rs
        JOIN reading_bookmark rb ON rb.reading_state_id = rs.id
        WHERE rs.book_id = $1
          AND rb.location_type = 'audiobook'
          AND rb.location_value IS NOT NULL
        "#,
        book_id,
    )
    .fetch_all(exec)
    .await?;
    Ok(rows)
}

pub async fn set_position(
    tx: &mut Transaction<'_, Postgres>,
    state_id: Uuid,
    book_id: Uuid,
    location_value: &str,
    progress_percent: Option<f64>,
) -> Result<()> {
    sqlx::query!(
        "UPDATE reading_bookmark
         SET location_value = $2,
             progress_percent = COALESCE($3, progress_percent),
             last_modified_at = NOW()
         WHERE reading_state_id = $1",
        state_id,
        location_value,
        progress_percent,
    )
    .execute(&mut **tx)
    .await?;
    sqlx::query!(
        "UPDATE reading_states rs SET book_id = $2
         WHERE rs.id = $1
           AND rs.book_id <> $2
           AND NOT EXISTS (
               SELECT 1 FROM reading_states o
               WHERE o.book_id = $2 AND o.user_id = rs.user_id AND o.device_id = rs.device_id
           )",
        state_id,
        book_id,
    )
    .execute(&mut **tx)
    .await?;
    Ok(())
}

pub struct FileBookmark {
    pub id: Uuid,
    pub audiobook_file_id: Uuid,
    pub position_seconds: f64,
}

pub async fn bookmarks_in<'e, E>(exec: E, files: &[Uuid]) -> Result<Vec<FileBookmark>>
where
    E: Executor<'e, Database = Postgres>,
{
    let rows = sqlx::query_as!(
        FileBookmark,
        r#"
        SELECT id, audiobook_file_id AS "audiobook_file_id!", position_seconds
        FROM audiobook_bookmarks
        WHERE audiobook_file_id = ANY($1)
        "#,
        files,
    )
    .fetch_all(exec)
    .await?;
    Ok(rows)
}

pub async fn set_bookmark(
    tx: &mut Transaction<'_, Postgres>,
    id: Uuid,
    book_id: Uuid,
    file_id: Uuid,
    seconds: f64,
) -> Result<()> {
    sqlx::query!(
        "UPDATE audiobook_bookmarks SET book_id = $2, audiobook_file_id = $3, position_seconds = $4 WHERE id = $1",
        id,
        book_id,
        file_id,
        seconds,
    )
    .execute(&mut **tx)
    .await?;
    Ok(())
}

pub async fn owners<'e, E>(exec: E, files: &[Uuid]) -> Result<Vec<(Uuid, Uuid)>>
where
    E: Executor<'e, Database = Postgres>,
{
    let rows = sqlx::query!(
        "SELECT id, book_id FROM audiobook_files WHERE id = ANY($1)",
        files,
    )
    .fetch_all(exec)
    .await?;
    Ok(rows.into_iter().map(|r| (r.id, r.book_id)).collect())
}

pub struct SiblingFile {
    pub book_id: Uuid,
    pub file_path: String,
    pub format: String,
}

/// Other books with the same title and an author in common, each with its
/// first audio file.
pub async fn siblings<'e, E>(exec: E, book_id: Uuid) -> Result<Vec<SiblingFile>>
where
    E: Executor<'e, Database = Postgres>,
{
    let rows = sqlx::query_as!(
        SiblingFile,
        r#"
        SELECT DISTINCT ON (b.id) b.id AS book_id, af.file_path, af.format
        FROM books b
        JOIN audiobook_files af ON af.book_id = b.id
        WHERE b.id <> $1
          AND LOWER(b.title) = (SELECT LOWER(title) FROM books WHERE id = $1)
          AND EXISTS (
              SELECT 1 FROM book_authors x
              JOIN book_authors y ON y.author_id = x.author_id
              WHERE x.book_id = b.id AND y.book_id = $1
          )
        ORDER BY b.id, af.file_index
        "#,
        book_id,
    )
    .fetch_all(exec)
    .await?;
    Ok(rows)
}
