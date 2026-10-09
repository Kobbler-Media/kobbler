//! Book-author linking operations.

use super::Result;
use sqlx::{Executor, PgPool, Postgres};
use uuid::Uuid;

/// Set the authors for a book (replaces existing authors).
pub async fn set_authors(pool: &PgPool, book_id: Uuid, author_ids: &[Uuid]) -> Result<()> {
    sqlx::query!("DELETE FROM book_authors WHERE book_id = $1", book_id)
        .execute(pool)
        .await?;

    for author_id in author_ids {
        sqlx::query!(
            "INSERT INTO book_authors (book_id, author_id) VALUES ($1, $2) ON CONFLICT DO NOTHING",
            book_id,
            author_id,
        )
        .execute(pool)
        .await?;
    }

    Ok(())
}

/// Clear all authors for a book.
pub async fn clear_authors<'e, E>(exec: E, book_id: Uuid) -> Result<()>
where
    E: Executor<'e, Database = Postgres>,
{
    sqlx::query!("DELETE FROM book_authors WHERE book_id = $1", book_id)
        .execute(exec)
        .await?;

    Ok(())
}

/// Link a book to an author.
pub async fn link_author<'e, E>(exec: E, book_id: Uuid, author_id: Uuid) -> Result<()>
where
    E: Executor<'e, Database = Postgres>,
{
    sqlx::query!(
        "INSERT INTO book_authors (book_id, author_id) VALUES ($1, $2) ON CONFLICT DO NOTHING",
        book_id,
        author_id,
    )
    .execute(exec)
    .await?;

    Ok(())
}
