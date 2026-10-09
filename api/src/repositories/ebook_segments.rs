//! Ebook text segment CRUD operations.

use sqlx::{Executor, Postgres};
use time::OffsetDateTime;
use uuid::Uuid;

use super::Result;

/// Input for inserting an ebook text segment.
#[derive(Debug)]
pub struct EbookSegmentInput {
    pub segment_index: i32,
    pub cfi: String,
    pub text: String,
    pub chapter_title: Option<String>,
}

/// An ebook text segment row from the database.
#[derive(Debug)]
pub struct EbookSegment {
    pub id: Uuid,
    pub book_file_id: Uuid,
    pub segment_index: i32,
    pub cfi: String,
    pub text: String,
    pub chapter_title: Option<String>,
    pub created_at: OffsetDateTime,
}

/// Insert multiple ebook text segments for a book file.
pub async fn insert_segments<'e, E>(
    exec: E,
    book_file_id: Uuid,
    segments: &[EbookSegmentInput],
) -> Result<()>
where
    E: Executor<'e, Database = Postgres>,
{
    let ids: Vec<Uuid> = segments.iter().map(|_| Uuid::now_v7()).collect();
    let file_ids: Vec<Uuid> = segments.iter().map(|_| book_file_id).collect();
    let indices: Vec<i32> = segments.iter().map(|s| s.segment_index).collect();
    let cfis: Vec<String> = segments.iter().map(|s| s.cfi.clone()).collect();
    let texts: Vec<String> = segments.iter().map(|s| s.text.clone()).collect();
    let chapters: Vec<Option<String>> = segments.iter().map(|s| s.chapter_title.clone()).collect();

    sqlx::query!(
        r#"
        INSERT INTO ebook_segments (id, book_file_id, segment_index, cfi, text, chapter_title)
        SELECT * FROM UNNEST($1::uuid[], $2::uuid[], $3::int[], $4::text[], $5::text[], $6::text[])
        ON CONFLICT (book_file_id, segment_index) DO UPDATE SET
            cfi = EXCLUDED.cfi,
            text = EXCLUDED.text,
            chapter_title = EXCLUDED.chapter_title
        "#,
        &ids,
        &file_ids,
        &indices,
        &cfis,
        &texts,
        &chapters as &[Option<String>],
    )
    .execute(exec)
    .await?;

    Ok(())
}

/// Delete all ebook segments for a book file.
pub async fn delete_for_file<'e, E>(exec: E, book_file_id: Uuid) -> Result<()>
where
    E: Executor<'e, Database = Postgres>,
{
    sqlx::query!(
        "DELETE FROM ebook_segments WHERE book_file_id = $1",
        book_file_id,
    )
    .execute(exec)
    .await?;
    Ok(())
}

/// Get all ebook segments for a book file, ordered by segment index.
pub async fn get_for_file<'e, E>(exec: E, book_file_id: Uuid) -> Result<Vec<EbookSegment>>
where
    E: Executor<'e, Database = Postgres>,
{
    let rows = sqlx::query_as!(
        EbookSegment,
        r#"
        SELECT id, book_file_id, segment_index, cfi, text, chapter_title, created_at
        FROM ebook_segments
        WHERE book_file_id = $1
        ORDER BY segment_index
        "#,
        book_file_id,
    )
    .fetch_all(exec)
    .await?;
    Ok(rows)
}

/// Get all ebook segments for a book (via book_files join), ordered by segment index.
pub async fn get_for_book<'e, E>(exec: E, book_id: Uuid) -> Result<Vec<EbookSegment>>
where
    E: Executor<'e, Database = Postgres>,
{
    let rows = sqlx::query_as!(
        EbookSegment,
        r#"
        SELECT e.id, e.book_file_id, e.segment_index, e.cfi, e.text, e.chapter_title, e.created_at
        FROM ebook_segments e
        JOIN book_files bf ON bf.id = e.book_file_id
        WHERE bf.book_id = $1
        ORDER BY e.segment_index
        "#,
        book_id,
    )
    .fetch_all(exec)
    .await?;
    Ok(rows)
}

/// Check if any ebook segments exist for a book.
pub async fn has_segments<'e, E>(exec: E, book_id: Uuid) -> Result<bool>
where
    E: Executor<'e, Database = Postgres>,
{
    let row = sqlx::query_scalar!(
        r#"
        SELECT EXISTS(
            SELECT 1 FROM ebook_segments e
            JOIN book_files bf ON bf.id = e.book_file_id
            WHERE bf.book_id = $1
        ) AS "exists!"
        "#,
        book_id,
    )
    .fetch_one(exec)
    .await?;
    Ok(row)
}
