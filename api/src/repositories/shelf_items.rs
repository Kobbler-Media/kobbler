//! Shelf items repository operations.

use super::Result;
use sqlx::{Executor, Postgres};
use uuid::Uuid;

/// Delete all shelf items for a book.
pub async fn delete_by_book<'e, E>(exec: E, book_id: Uuid) -> Result<u64>
where
    E: Executor<'e, Database = Postgres>,
{
    let result = sqlx::query!("DELETE FROM shelf_items WHERE book_id = $1", book_id)
        .execute(exec)
        .await?;
    Ok(result.rows_affected())
}

/// Add a book to a shelf (ON CONFLICT DO NOTHING).
pub async fn add<'e, E>(exec: E, shelf_id: Uuid, book_id: Uuid) -> Result<()>
where
    E: Executor<'e, Database = Postgres>,
{
    sqlx::query!(
        "INSERT INTO shelf_items (shelf_id, book_id) VALUES ($1, $2) ON CONFLICT DO NOTHING",
        shelf_id,
        book_id,
    )
    .execute(exec)
    .await?;

    Ok(())
}

/// Remove a book from a shelf.
pub async fn remove<'e, E>(exec: E, shelf_id: Uuid, book_id: Uuid) -> Result<bool>
where
    E: Executor<'e, Database = Postgres>,
{
    let result = sqlx::query!(
        "DELETE FROM shelf_items WHERE shelf_id = $1 AND book_id = $2",
        shelf_id,
        book_id,
    )
    .execute(exec)
    .await?;

    Ok(result.rows_affected() > 0)
}

/// List all book IDs for a shelf.
pub async fn list_book_ids<'e, E>(exec: E, shelf_id: Uuid) -> Result<Vec<Uuid>>
where
    E: Executor<'e, Database = Postgres>,
{
    let ids: Vec<Uuid> = sqlx::query_scalar!(
        "SELECT book_id FROM shelf_items WHERE shelf_id = $1",
        shelf_id
    )
    .fetch_all(exec)
    .await?;
    Ok(ids)
}

/// Fetch shelf memberships for a batch of books, scoped to a user.
/// Returns a map of book_id → Vec<ShelfRef>.
pub async fn list_shelves_for_books<'e, E>(
    exec: E,
    user_id: Uuid,
    book_ids: &[Uuid],
) -> Result<std::collections::HashMap<Uuid, Vec<crate::routes::library::types::ShelfRef>>>
where
    E: Executor<'e, Database = Postgres>,
{
    use std::collections::HashMap;

    if book_ids.is_empty() {
        return Ok(HashMap::new());
    }

    let rows = sqlx::query(
        r#"
        SELECT si.book_id, s.id AS shelf_id, s.name AS shelf_name
        FROM shelf_items si
        JOIN shelves s ON s.id = si.shelf_id
        WHERE s.user_id = $1 AND si.book_id = ANY($2)
        ORDER BY s.name
        "#,
    )
    .bind(user_id)
    .bind(book_ids)
    .fetch_all(exec)
    .await?;

    let mut map: HashMap<Uuid, Vec<crate::routes::library::types::ShelfRef>> = HashMap::new();
    for row in rows {
        use sqlx::Row;
        let book_id: Uuid = row.get("book_id");
        let shelf_id: Uuid = row.get("shelf_id");
        let shelf_name: String = row.get("shelf_name");
        map.entry(book_id)
            .or_default()
            .push(crate::routes::library::types::ShelfRef {
                id: shelf_id,
                name: shelf_name,
            });
    }

    Ok(map)
}
