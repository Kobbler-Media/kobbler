//! Genre repository operations.

use super::Result;
use sqlx::{Executor, PgPool, Postgres};
use uuid::Uuid;

/// Get an existing genre by normalized name, or create a new one.
///
/// Uses `normalized_name` (lowercase) for case-insensitive matching.
pub async fn get_or_create<'e, E>(exec: E, name: &str) -> Result<Uuid>
where
    E: Executor<'e, Database = Postgres>,
{
    let name = name.trim();
    let normalized = name.to_lowercase();
    let id = Uuid::now_v7();

    let row_id: Uuid = sqlx::query_scalar!(
        r#"
        INSERT INTO genres (id, name, normalized_name)
        VALUES ($1, $2, $3)
        ON CONFLICT (normalized_name) DO UPDATE SET
            name = EXCLUDED.name
        RETURNING id
        "#,
        id,
        name,
        normalized,
    )
    .fetch_one(exec)
    .await?;

    Ok(row_id)
}

/// Set the genres for a book (replaces existing genres).
///
/// Creates genres that don't exist.
pub async fn set_book_genres(pool: &PgPool, book_id: Uuid, subjects: &[String]) -> Result<()> {
    // Delete existing genres for this book
    sqlx::query!("DELETE FROM book_genres WHERE book_id = $1", book_id)
        .execute(pool)
        .await?;

    // Add new genres
    for subject in subjects {
        let subject = subject.trim();
        if subject.is_empty() {
            continue;
        }

        let genre_id = get_or_create(pool, subject).await?;
        link_book(pool, book_id, genre_id).await?;
    }

    Ok(())
}

/// Link a book to a genre.
///
/// Does nothing if the link already exists.
pub async fn link_book<'e, E>(exec: E, book_id: Uuid, genre_id: Uuid) -> Result<()>
where
    E: Executor<'e, Database = Postgres>,
{
    sqlx::query!(
        "INSERT INTO book_genres (book_id, genre_id) VALUES ($1, $2) ON CONFLICT DO NOTHING",
        book_id,
        genre_id,
    )
    .execute(exec)
    .await?;

    Ok(())
}

/// Delete orphaned genres (no book links).
/// Returns IDs of deleted genres for search index cleanup.
pub async fn delete_orphaned<'e, E>(exec: E) -> Result<Vec<Uuid>>
where
    E: Executor<'e, Database = Postgres>,
{
    let ids = sqlx::query_scalar!(
        r#"
        DELETE FROM genres
        WHERE NOT EXISTS (SELECT 1 FROM book_genres WHERE genre_id = genres.id)
        RETURNING id
        "#,
    )
    .fetch_all(exec)
    .await?;

    Ok(ids)
}

/// Genre with book count.
pub struct GenreWithCount {
    pub id: Uuid,
    pub name: String,
    pub book_count: i64,
}

/// List all genres with their book counts.
pub async fn list_with_counts<'e, E>(exec: E) -> Result<Vec<GenreWithCount>>
where
    E: Executor<'e, Database = Postgres>,
{
    let rows = sqlx::query!(
        r#"
        SELECT g.id, g.name, COUNT(bg.book_id) as "book_count!: i64"
        FROM genres g
        LEFT JOIN book_genres bg ON bg.genre_id = g.id
        GROUP BY g.id, g.name
        ORDER BY g.name ASC
        "#,
    )
    .fetch_all(exec)
    .await?;

    Ok(rows
        .into_iter()
        .map(|row| GenreWithCount {
            id: row.id,
            name: row.name,
            book_count: row.book_count,
        })
        .collect())
}
