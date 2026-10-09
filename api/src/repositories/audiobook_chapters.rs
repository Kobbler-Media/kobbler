//! Audiobook chapter CRUD operations.

use sqlx::{Executor, Postgres};
use time::OffsetDateTime;
use uuid::Uuid;

use super::Result;

/// Input for inserting a chapter.
#[derive(Debug)]
pub struct ChapterInput {
    pub chapter_index: i32,
    pub title: Option<String>,
    pub start_seconds: f64,
    pub end_seconds: f64,
    pub file_path: Option<String>,
    pub source: String,
}

/// A chapter row from the database.
#[derive(Debug)]
pub struct AudiobookChapter {
    pub id: Uuid,
    pub audiobook_file_id: Uuid,
    pub chapter_index: i32,
    pub title: Option<String>,
    pub start_seconds: f64,
    pub end_seconds: f64,
    pub file_path: Option<String>,
    pub source: String,
    pub created_at: OffsetDateTime,
}

/// Insert multiple chapters for a book file, replacing any existing ones.
pub async fn insert_chapters<'e, E>(
    exec: E,
    audiobook_file_id: Uuid,
    chapters: &[ChapterInput],
) -> Result<()>
where
    E: Executor<'e, Database = Postgres>,
{
    // Use a single INSERT with UNNEST for efficiency
    let ids: Vec<Uuid> = chapters.iter().map(|_| Uuid::now_v7()).collect();
    let indices: Vec<i32> = chapters.iter().map(|c| c.chapter_index).collect();
    let titles: Vec<Option<String>> = chapters.iter().map(|c| c.title.clone()).collect();
    let starts: Vec<f64> = chapters.iter().map(|c| c.start_seconds).collect();
    let ends: Vec<f64> = chapters.iter().map(|c| c.end_seconds).collect();
    let paths: Vec<Option<String>> = chapters.iter().map(|c| c.file_path.clone()).collect();
    let file_ids: Vec<Uuid> = chapters.iter().map(|_| audiobook_file_id).collect();
    let sources: Vec<String> = chapters.iter().map(|c| c.source.clone()).collect();

    sqlx::query!(
        r#"
        INSERT INTO audiobook_chapters (id, audiobook_file_id, chapter_index, title, start_seconds, end_seconds, file_path, source)
        SELECT * FROM UNNEST($1::uuid[], $2::uuid[], $3::int[], $4::text[], $5::float8[], $6::float8[], $7::text[], $8::text[])
        ON CONFLICT (audiobook_file_id, chapter_index) DO UPDATE SET
            title = EXCLUDED.title,
            start_seconds = EXCLUDED.start_seconds,
            end_seconds = EXCLUDED.end_seconds,
            file_path = EXCLUDED.file_path,
            source = EXCLUDED.source
        "#,
        &ids,
        &file_ids,
        &indices,
        &titles as &[Option<String>],
        &starts,
        &ends,
        &paths as &[Option<String>],
        &sources,
    )
    .execute(exec)
    .await?;

    Ok(())
}

/// Get all chapters for a book file, ordered by chapter index.
pub async fn get_chapters<'e, E>(exec: E, audiobook_file_id: Uuid) -> Result<Vec<AudiobookChapter>>
where
    E: Executor<'e, Database = Postgres>,
{
    let rows = sqlx::query_as!(
        AudiobookChapter,
        r#"
        SELECT id, audiobook_file_id, chapter_index, title, start_seconds, end_seconds, file_path, source, created_at
        FROM audiobook_chapters
        WHERE audiobook_file_id = $1
        ORDER BY chapter_index
        "#,
        audiobook_file_id,
    )
    .fetch_all(exec)
    .await?;

    Ok(rows)
}

/// Delete all chapters for a book file.
pub async fn delete_chapters<'e, E>(exec: E, audiobook_file_id: Uuid) -> Result<u64>
where
    E: Executor<'e, Database = Postgres>,
{
    let result = sqlx::query!(
        "DELETE FROM audiobook_chapters WHERE audiobook_file_id = $1",
        audiobook_file_id,
    )
    .execute(exec)
    .await?;

    Ok(result.rows_affected())
}

/// Update a single chapter's title, start, and end.
pub async fn update_chapter<'e, E>(
    exec: E,
    chapter_id: Uuid,
    title: Option<&str>,
    start_seconds: f64,
    end_seconds: f64,
) -> Result<bool>
where
    E: Executor<'e, Database = Postgres>,
{
    let result = sqlx::query!(
        r#"
        UPDATE audiobook_chapters
        SET title = $2, start_seconds = $3, end_seconds = $4
        WHERE id = $1
        "#,
        chapter_id,
        title,
        start_seconds,
        end_seconds,
    )
    .execute(exec)
    .await?;
    Ok(result.rows_affected() > 0)
}

/// Delete a single chapter by ID.
pub async fn delete_chapter<'e, E>(exec: E, chapter_id: Uuid) -> Result<bool>
where
    E: Executor<'e, Database = Postgres>,
{
    let result = sqlx::query!("DELETE FROM audiobook_chapters WHERE id = $1", chapter_id)
        .execute(exec)
        .await?;
    Ok(result.rows_affected() > 0)
}

/// Atomically replace all chapters for a file (delete + insert).
pub async fn replace_chapters(
    pool: &sqlx::PgPool,
    audiobook_file_id: Uuid,
    chapters: &[ChapterInput],
) -> Result<()> {
    let mut tx = pool.begin().await?;
    sqlx::query!(
        "DELETE FROM audiobook_chapters WHERE audiobook_file_id = $1",
        audiobook_file_id,
    )
    .execute(&mut *tx)
    .await?;

    if !chapters.is_empty() {
        let ids: Vec<Uuid> = chapters.iter().map(|_| Uuid::now_v7()).collect();
        let indices: Vec<i32> = chapters.iter().map(|c| c.chapter_index).collect();
        let titles: Vec<Option<String>> = chapters.iter().map(|c| c.title.clone()).collect();
        let starts: Vec<f64> = chapters.iter().map(|c| c.start_seconds).collect();
        let ends: Vec<f64> = chapters.iter().map(|c| c.end_seconds).collect();
        let paths: Vec<Option<String>> = chapters.iter().map(|c| c.file_path.clone()).collect();
        let file_ids: Vec<Uuid> = chapters.iter().map(|_| audiobook_file_id).collect();
        let sources: Vec<String> = chapters.iter().map(|c| c.source.clone()).collect();

        sqlx::query!(
            r#"
            INSERT INTO audiobook_chapters (id, audiobook_file_id, chapter_index, title, start_seconds, end_seconds, file_path, source)
            SELECT * FROM UNNEST($1::uuid[], $2::uuid[], $3::int[], $4::text[], $5::float8[], $6::float8[], $7::text[], $8::text[])
            "#,
            &ids,
            &file_ids,
            &indices,
            &titles as &[Option<String>],
            &starts,
            &ends,
            &paths as &[Option<String>],
            &sources,
        )
        .execute(&mut *tx)
        .await?;
    }

    tx.commit().await?;
    Ok(())
}

/// Chapter with associated file info, for unified timeline queries.
pub struct ChapterWithFileInfo {
    pub id: Uuid,
    pub audiobook_file_id: Uuid,
    pub file_path: String,
    pub file_duration_seconds: Option<f64>,
    pub chapter_index: i32,
    pub title: Option<String>,
    pub start_seconds: f64,
    pub end_seconds: f64,
    pub source: String,
}

/// Get all chapters for a book, joined with file info, ordered for unified timeline.
pub async fn get_all_for_book<'e, E>(exec: E, book_id: Uuid) -> Result<Vec<ChapterWithFileInfo>>
where
    E: Executor<'e, Database = Postgres>,
{
    let rows = sqlx::query_as!(
        ChapterWithFileInfo,
        r#"
        SELECT
            c.id,
            c.audiobook_file_id,
            af.file_path,
            af.duration_seconds AS file_duration_seconds,
            c.chapter_index,
            c.title,
            c.start_seconds,
            c.end_seconds,
            c.source
        FROM audiobook_chapters c
        JOIN audiobook_files af ON af.id = c.audiobook_file_id
        WHERE af.book_id = $1
        ORDER BY af.file_index, af.file_path, c.chapter_index
        "#,
        book_id,
    )
    .fetch_all(exec)
    .await?;

    Ok(rows)
}

/// Delete chapters for a file with a specific source.
pub async fn delete_by_source<'e, E>(exec: E, audiobook_file_id: Uuid, source: &str) -> Result<u64>
where
    E: Executor<'e, Database = Postgres>,
{
    let result = sqlx::query!(
        "DELETE FROM audiobook_chapters WHERE audiobook_file_id = $1 AND source = $2",
        audiobook_file_id,
        source,
    )
    .execute(exec)
    .await?;
    Ok(result.rows_affected())
}

/// Check if any chapters with a given source exist for a book.
pub async fn has_chapters_by_source<'e, E>(exec: E, book_id: Uuid, source: &str) -> Result<bool>
where
    E: Executor<'e, Database = Postgres>,
{
    let row = sqlx::query_scalar!(
        r#"
        SELECT EXISTS(
            SELECT 1 FROM audiobook_chapters c
            JOIN audiobook_files af ON af.id = c.audiobook_file_id
            WHERE af.book_id = $1 AND c.source = $2
        ) AS "exists!"
        "#,
        book_id,
        source,
    )
    .fetch_one(exec)
    .await?;
    Ok(row)
}
