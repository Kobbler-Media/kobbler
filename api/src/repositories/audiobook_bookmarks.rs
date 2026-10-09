//! Audiobook bookmarks repository.

use super::Result;
use sqlx::{Executor, Postgres, Row};
use uuid::Uuid;

#[derive(Debug, serde::Serialize)]
pub struct BookmarkRow {
    pub id: Uuid,
    pub book_id: Uuid,
    pub user_id: Uuid,
    pub audiobook_file_id: Option<Uuid>,
    pub position_seconds: f64,
    pub note: Option<String>,
    #[serde(with = "time::serde::rfc3339")]
    pub created_at: time::OffsetDateTime,
}

pub async fn create<'e, E>(
    exec: E,
    user_id: Uuid,
    book_id: Uuid,
    audiobook_file_id: Option<Uuid>,
    position_seconds: f64,
    note: Option<&str>,
) -> Result<Uuid>
where
    E: Executor<'e, Database = Postgres>,
{
    let id = Uuid::now_v7();
    sqlx::query(
        r#"
        INSERT INTO audiobook_bookmarks (id, user_id, book_id, audiobook_file_id, position_seconds, note)
        VALUES ($1, $2, $3, $4, $5, $6)
        "#,
    )
    .bind(id)
    .bind(user_id)
    .bind(book_id)
    .bind(audiobook_file_id)
    .bind(position_seconds)
    .bind(note)
    .execute(exec)
    .await?;
    Ok(id)
}

pub async fn list_by_book<'e, E>(exec: E, book_id: Uuid, user_id: Uuid) -> Result<Vec<BookmarkRow>>
where
    E: Executor<'e, Database = Postgres>,
{
    let rows = sqlx::query(
        r#"
        SELECT id, book_id, user_id, audiobook_file_id, position_seconds, note, created_at
        FROM audiobook_bookmarks
        WHERE book_id = $1 AND user_id = $2
        ORDER BY position_seconds
        "#,
    )
    .bind(book_id)
    .bind(user_id)
    .fetch_all(exec)
    .await?;

    Ok(rows
        .into_iter()
        .map(|r| BookmarkRow {
            id: r.get("id"),
            book_id: r.get("book_id"),
            user_id: r.get("user_id"),
            audiobook_file_id: r.get("audiobook_file_id"),
            position_seconds: r.get("position_seconds"),
            note: r.get("note"),
            created_at: r.get("created_at"),
        })
        .collect())
}

pub async fn delete<'e, E>(exec: E, id: Uuid, user_id: Uuid) -> Result<bool>
where
    E: Executor<'e, Database = Postgres>,
{
    let result = sqlx::query("DELETE FROM audiobook_bookmarks WHERE id = $1 AND user_id = $2")
        .bind(id)
        .bind(user_id)
        .execute(exec)
        .await?;
    Ok(result.rows_affected() > 0)
}
