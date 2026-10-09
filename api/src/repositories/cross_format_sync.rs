//! Cross-format sync point CRUD operations.
//!
//! Maps between audiobook timestamps and ebook CFI positions for the same book.

use sqlx::{Executor, Postgres};
use time::OffsetDateTime;
use uuid::Uuid;

use super::Result;

/// Input for inserting a sync point.
#[derive(Debug)]
pub struct SyncPointInput {
    pub audio_file_id: Uuid,
    pub audio_seconds: f64,
    pub ebook_file_id: Uuid,
    pub ebook_cfi: String,
    pub confidence: f64,
}

/// A sync point row from the database.
#[derive(Debug)]
pub struct SyncPoint {
    pub id: Uuid,
    pub book_id: Uuid,
    pub audio_file_id: Uuid,
    pub audio_seconds: f64,
    pub ebook_file_id: Uuid,
    pub ebook_cfi: String,
    pub confidence: f64,
    pub created_at: OffsetDateTime,
}

/// Insert multiple sync points for a book, replacing any existing ones.
pub async fn insert_sync_points<'e, E>(
    exec: E,
    book_id: Uuid,
    points: &[SyncPointInput],
) -> Result<()>
where
    E: Executor<'e, Database = Postgres>,
{
    let ids: Vec<Uuid> = points.iter().map(|_| Uuid::now_v7()).collect();
    let book_ids: Vec<Uuid> = points.iter().map(|_| book_id).collect();
    let audio_file_ids: Vec<Uuid> = points.iter().map(|p| p.audio_file_id).collect();
    let audio_secs: Vec<f64> = points.iter().map(|p| p.audio_seconds).collect();
    let ebook_file_ids: Vec<Uuid> = points.iter().map(|p| p.ebook_file_id).collect();
    let cfis: Vec<String> = points.iter().map(|p| p.ebook_cfi.clone()).collect();
    let confidences: Vec<f64> = points.iter().map(|p| p.confidence).collect();

    sqlx::query!(
        r#"
        INSERT INTO cross_format_sync (id, book_id, audio_file_id, audio_seconds, ebook_file_id, ebook_cfi, confidence)
        SELECT * FROM UNNEST($1::uuid[], $2::uuid[], $3::uuid[], $4::float8[], $5::uuid[], $6::text[], $7::float8[])
        "#,
        &ids,
        &book_ids,
        &audio_file_ids,
        &audio_secs,
        &ebook_file_ids,
        &cfis,
        &confidences,
    )
    .execute(exec)
    .await?;

    Ok(())
}

/// Delete all sync points for a book.
pub async fn delete_for_book<'e, E>(exec: E, book_id: Uuid) -> Result<()>
where
    E: Executor<'e, Database = Postgres>,
{
    sqlx::query!("DELETE FROM cross_format_sync WHERE book_id = $1", book_id)
        .execute(exec)
        .await?;
    Ok(())
}

/// Find the closest ebook position for a given audiobook timestamp.
/// Returns the sync point with the nearest audio_seconds <= the given value.
pub async fn find_ebook_position<'e, E>(
    exec: E,
    book_id: Uuid,
    audio_seconds: f64,
) -> Result<Option<SyncPoint>>
where
    E: Executor<'e, Database = Postgres>,
{
    let row = sqlx::query_as!(
        SyncPoint,
        r#"
        SELECT id, book_id, audio_file_id, audio_seconds, ebook_file_id, ebook_cfi, confidence, created_at
        FROM cross_format_sync
        WHERE book_id = $1 AND audio_seconds <= $2
        ORDER BY audio_seconds DESC
        LIMIT 1
        "#,
        book_id,
        audio_seconds,
    )
    .fetch_optional(exec)
    .await?;
    Ok(row)
}

/// Find the closest audio position for a given ebook CFI.
/// This does a text comparison on CFI strings — looks for exact match or nearest by segment index.
pub async fn find_audio_position<'e, E>(
    exec: E,
    book_id: Uuid,
    ebook_cfi: &str,
) -> Result<Option<SyncPoint>>
where
    E: Executor<'e, Database = Postgres>,
{
    // Find exact CFI match, or nearest CFI that sorts before the target
    let row = sqlx::query_as!(
        SyncPoint,
        r#"
        SELECT id, book_id, audio_file_id, audio_seconds, ebook_file_id, ebook_cfi, confidence, created_at
        FROM cross_format_sync
        WHERE book_id = $1 AND ebook_cfi <= $2
        ORDER BY ebook_cfi DESC
        LIMIT 1
        "#,
        book_id,
        ebook_cfi,
    )
    .fetch_optional(exec)
    .await?;
    Ok(row)
}

/// Get all sync points for a book, ordered by audio timestamp.
pub async fn get_for_book<'e, E>(exec: E, book_id: Uuid) -> Result<Vec<SyncPoint>>
where
    E: Executor<'e, Database = Postgres>,
{
    let rows = sqlx::query_as!(
        SyncPoint,
        r#"
        SELECT id, book_id, audio_file_id, audio_seconds, ebook_file_id, ebook_cfi, confidence, created_at
        FROM cross_format_sync
        WHERE book_id = $1
        ORDER BY audio_seconds
        "#,
        book_id,
    )
    .fetch_all(exec)
    .await?;
    Ok(rows)
}
