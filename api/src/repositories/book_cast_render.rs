use serde::Serialize;
use sqlx::types::Json;
use sqlx::{Executor, Postgres};
use uuid::Uuid;

use super::Result;
use crate::book_cast_render::fingerprint::Fingerprint;
use crate::repositories::book_casts::CastState;

pub struct RenderedChapter {
    pub chapter_index: i32,
    pub duration_ms: i64,
    pub fingerprint: Json<Fingerprint>,
    pub render_ms: i64,
}

#[derive(Debug, Serialize)]
pub struct CastAudiobook {
    pub book_id: Uuid,
    pub file_id: Uuid,
}

pub struct BookTags {
    pub title: String,
    pub author: Option<String>,
}

/// Moves the cast into rendering from the state it was seen in, so two starts
/// cannot both queue a render.
pub async fn begin<'e, E>(exec: E, book_id: Uuid, from: CastState) -> Result<bool>
where
    E: Executor<'e, Database = Postgres>,
{
    let done = sqlx::query!(
        r#"UPDATE book_casts SET state = 'rendering', error = NULL, updated_at = NOW()
           WHERE book_id = $1 AND state = $2"#,
        book_id,
        from.as_str()
    )
    .execute(exec)
    .await?;
    Ok(done.rows_affected() > 0)
}

pub async fn rendered<'e, E>(exec: E, book_id: Uuid) -> Result<Vec<RenderedChapter>>
where
    E: Executor<'e, Database = Postgres>,
{
    Ok(sqlx::query_as!(
        RenderedChapter,
        r#"SELECT chapter_index, duration_ms,
                  fingerprint AS "fingerprint: Json<Fingerprint>", render_ms
           FROM book_cast_renders
           WHERE book_id = $1 ORDER BY chapter_index"#,
        book_id
    )
    .fetch_all(exec)
    .await?)
}

pub async fn save_rendered<'e, E>(exec: E, book_id: Uuid, chapter: &RenderedChapter) -> Result<()>
where
    E: Executor<'e, Database = Postgres>,
{
    sqlx::query!(
        r#"INSERT INTO book_cast_renders (book_id, chapter_index, duration_ms, fingerprint, render_ms)
           VALUES ($1, $2, $3, $4, $5)
           ON CONFLICT (book_id, chapter_index) DO UPDATE SET
               duration_ms = EXCLUDED.duration_ms, fingerprint = EXCLUDED.fingerprint,
               render_ms = EXCLUDED.render_ms, rendered_at = NOW()"#,
        book_id,
        chapter.chapter_index,
        chapter.duration_ms,
        &chapter.fingerprint as &Json<Fingerprint>,
        chapter.render_ms
    )
    .execute(exec)
    .await?;
    Ok(())
}

pub async fn finish<'e, E>(exec: E, book_id: Uuid, audiobook_file_id: Uuid) -> Result<()>
where
    E: Executor<'e, Database = Postgres>,
{
    sqlx::query!(
        r#"UPDATE book_casts SET state = 'done', error = NULL, audiobook_file_id = $2,
               updated_at = NOW()
           WHERE book_id = $1"#,
        book_id,
        audiobook_file_id
    )
    .execute(exec)
    .await?;
    Ok(())
}

/// Where the imported audiobook lives now: an edition split can move its file
/// to a book of its own.
pub async fn audiobook<'e, E>(exec: E, book_id: Uuid) -> Result<Option<CastAudiobook>>
where
    E: Executor<'e, Database = Postgres>,
{
    Ok(sqlx::query_as!(
        CastAudiobook,
        r#"SELECT f.book_id, f.id AS file_id
           FROM book_casts c JOIN audiobook_files f ON f.id = c.audiobook_file_id
           WHERE c.book_id = $1"#,
        book_id
    )
    .fetch_optional(exec)
    .await?)
}

pub async fn tags<'e, E>(exec: E, book_id: Uuid) -> Result<Option<BookTags>>
where
    E: Executor<'e, Database = Postgres>,
{
    Ok(sqlx::query_as!(
        BookTags,
        r#"SELECT b.title,
                  (SELECT a.name FROM book_authors ba JOIN authors a ON a.id = ba.author_id
                   WHERE ba.book_id = b.id ORDER BY ba.author_id LIMIT 1) AS "author?"
           FROM books b WHERE b.id = $1"#,
        book_id
    )
    .fetch_optional(exec)
    .await?)
}

pub async fn imported_file<'e, E>(exec: E, book_id: Uuid, source_path: &str) -> Result<Option<Uuid>>
where
    E: Executor<'e, Database = Postgres>,
{
    Ok(sqlx::query_scalar!(
        r#"SELECT id FROM audiobook_files WHERE book_id = $1 AND source_path = $2
           ORDER BY added_at DESC LIMIT 1"#,
        book_id,
        source_path
    )
    .fetch_optional(exec)
    .await?)
}
