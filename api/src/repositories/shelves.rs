//! Shelf repository operations.

use super::Result;
use sqlx::{Executor, Postgres};
use uuid::Uuid;

/// Create a new shelf.
pub async fn create<'e, E>(exec: E, user_id: Uuid, name: &str) -> Result<Uuid>
where
    E: Executor<'e, Database = Postgres>,
{
    let id = Uuid::new_v4();
    sqlx::query!(
        "INSERT INTO shelves (id, user_id, name) VALUES ($1, $2, $3)",
        id,
        user_id,
        name,
    )
    .execute(exec)
    .await?;

    Ok(id)
}

/// Rename a shelf.
pub async fn rename<'e, E>(exec: E, id: Uuid, name: &str) -> Result<bool>
where
    E: Executor<'e, Database = Postgres>,
{
    let result = sqlx::query!(
        "UPDATE shelves SET name = $1, updated_at = NOW() WHERE id = $2",
        name,
        id,
    )
    .execute(exec)
    .await?;

    Ok(result.rows_affected() > 0)
}

/// Delete a shelf by ID.
pub async fn delete<'e, E>(exec: E, id: Uuid) -> Result<bool>
where
    E: Executor<'e, Database = Postgres>,
{
    let result = sqlx::query!("DELETE FROM shelves WHERE id = $1", id)
        .execute(exec)
        .await?;

    Ok(result.rows_affected() > 0)
}

/// Touch the updated_at timestamp for a shelf.
pub async fn touch<'e, E>(exec: E, id: Uuid) -> Result<()>
where
    E: Executor<'e, Database = Postgres>,
{
    sqlx::query!("UPDATE shelves SET updated_at = NOW() WHERE id = $1", id)
        .execute(exec)
        .await?;

    Ok(())
}

/// Get shelf user_id and name by ID.
pub async fn get_user_and_name<'e, E>(exec: E, id: Uuid) -> Result<Option<(Uuid, String)>>
where
    E: Executor<'e, Database = Postgres>,
{
    let row = sqlx::query!("SELECT user_id, name FROM shelves WHERE id = $1", id,)
        .fetch_optional(exec)
        .await?;

    Ok(row.map(|r| (r.user_id, r.name)))
}

/// Archive a shelf (for sync purposes - records deleted shelf).
pub async fn archive<'e, E>(exec: E, id: Uuid, user_id: Uuid, name: &str) -> Result<()>
where
    E: Executor<'e, Database = Postgres>,
{
    sqlx::query!(
        r#"
        INSERT INTO shelf_archives (id, user_id, name, last_modified)
        VALUES ($1, $2, $3, NOW())
        ON CONFLICT (id) DO UPDATE SET name = EXCLUDED.name, last_modified = NOW()
        "#,
        id,
        user_id,
        name,
    )
    .execute(exec)
    .await?;

    Ok(())
}

/// Get the user_id that owns a shelf.
pub async fn get_user_id<'e, E>(exec: E, id: Uuid) -> Result<Option<Uuid>>
where
    E: Executor<'e, Database = Postgres>,
{
    let row = sqlx::query_scalar!("SELECT user_id FROM shelves WHERE id = $1", id)
        .fetch_optional(exec)
        .await?;
    Ok(row)
}

/// Check if a shelf exists.
pub async fn exists<'e, E>(exec: E, id: Uuid) -> Result<bool>
where
    E: Executor<'e, Database = Postgres>,
{
    let exists: bool = sqlx::query_scalar!(
        r#"SELECT EXISTS(SELECT 1 FROM shelves WHERE id = $1) as "exists!""#,
        id,
    )
    .fetch_one(exec)
    .await?;
    Ok(exists)
}

/// Get shelf name by ID and user.
pub async fn get_name<'e, E>(exec: E, id: Uuid, user_id: Uuid) -> Result<Option<String>>
where
    E: Executor<'e, Database = Postgres>,
{
    let name: Option<String> = sqlx::query_scalar!(
        "SELECT name FROM shelves WHERE id = $1 AND user_id = $2",
        id,
        user_id,
    )
    .fetch_optional(exec)
    .await?;
    Ok(name)
}

/// Delete a shelf by ID and user.
pub async fn delete_for_user<'e, E>(exec: E, id: Uuid, user_id: Uuid) -> Result<bool>
where
    E: Executor<'e, Database = Postgres>,
{
    let result = sqlx::query!(
        "DELETE FROM shelves WHERE id = $1 AND user_id = $2",
        id,
        user_id,
    )
    .execute(exec)
    .await?;
    Ok(result.rows_affected() > 0)
}

/// Rename a shelf for a user.
pub async fn rename_for_user<'e, E>(exec: E, id: Uuid, user_id: Uuid, name: &str) -> Result<bool>
where
    E: Executor<'e, Database = Postgres>,
{
    let result = sqlx::query!(
        "UPDATE shelves SET name = $1, updated_at = NOW() WHERE id = $2 AND user_id = $3",
        name,
        id,
        user_id,
    )
    .execute(exec)
    .await?;
    Ok(result.rows_affected() > 0)
}

/// Shelf info for sync operations.
#[derive(Debug)]
pub struct ShelfSyncInfo {
    pub id: Uuid,
    pub name: String,
    pub created_at: time::OffsetDateTime,
    pub updated_at: time::OffsetDateTime,
    pub item_last_modified: Option<time::OffsetDateTime>,
}

/// List shelves modified since a timestamp (for sync).
pub async fn list_modified_since<'e, E>(
    exec: E,
    user_id: Uuid,
    since: time::OffsetDateTime,
) -> Result<Vec<ShelfSyncInfo>>
where
    E: Executor<'e, Database = Postgres>,
{
    let rows = sqlx::query!(
        r#"
        SELECT s.id, s.name, s.created_at, s.updated_at, MAX(si.added_at) AS item_last_modified
        FROM shelves s
        LEFT JOIN shelf_items si ON si.shelf_id = s.id
        WHERE s.user_id = $1
        GROUP BY s.id
        HAVING s.updated_at > $2 OR MAX(si.added_at) > $2
        ORDER BY GREATEST(s.updated_at, COALESCE(MAX(si.added_at), s.updated_at)) ASC
        "#,
        user_id,
        since,
    )
    .fetch_all(exec)
    .await?;

    Ok(rows
        .into_iter()
        .map(|r| ShelfSyncInfo {
            id: r.id,
            name: r.name,
            created_at: r.created_at,
            updated_at: r.updated_at,
            item_last_modified: r.item_last_modified,
        })
        .collect())
}

/// Deleted shelf info from archive.
#[derive(Debug)]
pub struct ShelfArchiveInfo {
    pub id: Uuid,
    pub name: Option<String>,
    pub last_modified: time::OffsetDateTime,
}

/// List deleted shelves since a timestamp (from archive).
pub async fn list_archives_since<'e, E>(
    exec: E,
    user_id: Uuid,
    since: time::OffsetDateTime,
) -> Result<Vec<ShelfArchiveInfo>>
where
    E: Executor<'e, Database = Postgres>,
{
    let rows = sqlx::query!(
        "SELECT id, name, last_modified FROM shelf_archives WHERE user_id = $1 AND last_modified > $2 ORDER BY last_modified ASC",
        user_id,
        since,
    )
    .fetch_all(exec)
    .await?;

    Ok(rows
        .into_iter()
        .map(|r| ShelfArchiveInfo {
            id: r.id,
            name: r.name,
            last_modified: r.last_modified,
        })
        .collect())
}

// =============================================================================
// Shelf Listing with Filters
// =============================================================================

/// Filter parameters for shelf listing.
#[derive(Debug, Default)]
pub struct ShelfListFilter {
    /// User ID (required).
    pub user_id: Uuid,
    /// Cursor for pagination (name, id).
    pub cursor: Option<(String, Uuid)>,
    /// Limit results (fetches limit+1 for has_more).
    pub limit: i64,
}

/// Shelf summary row for listing.
#[derive(Debug)]
pub struct ShelfSummaryRow {
    pub id: Uuid,
    pub name: String,
    pub user_id: Uuid,
    pub updated_at: time::OffsetDateTime,
    pub book_count: i64,
    pub item_last_modified: Option<time::OffsetDateTime>,
    pub sample_book_ids: Vec<Uuid>,
}

/// Count shelves matching filters (for pagination total).
pub async fn count_with_filters<'e, E>(exec: E, filter: &ShelfListFilter) -> Result<i64>
where
    E: Executor<'e, Database = Postgres>,
{
    let count: i64 = sqlx::query_scalar!(
        r#"SELECT COUNT(*) as "count!" FROM shelves WHERE user_id = $1"#,
        filter.user_id,
    )
    .fetch_one(exec)
    .await?;
    Ok(count)
}

/// List shelves with filters (for admin listing).
pub async fn list_with_filters<'e, E>(
    exec: E,
    filter: &ShelfListFilter,
) -> Result<Vec<ShelfSummaryRow>>
where
    E: Executor<'e, Database = Postgres>,
{
    let rows = if let Some((ref name, id)) = filter.cursor {
        sqlx::query!(
            r#"
            SELECT s.id, s.name, s.user_id, s.updated_at,
                   COUNT(si.book_id) AS "book_count!",
                   MAX(si.added_at) AS item_last_modified,
                   (
                     SELECT COALESCE(array_agg(book_id), ARRAY[]::uuid[])
                     FROM (
                       SELECT si2.book_id FROM shelf_items si2
                       WHERE si2.shelf_id = s.id
                       ORDER BY si2.added_at DESC
                       LIMIT 3
                     ) sub
                   ) AS "sample_book_ids!"
            FROM shelves s
            LEFT JOIN shelf_items si ON si.shelf_id = s.id
            WHERE s.user_id = $1 AND (s.name, s.id) > ($2, $3)
            GROUP BY s.id
            ORDER BY s.name ASC, s.id ASC
            LIMIT $4
            "#,
            filter.user_id,
            name,
            id,
            filter.limit + 1,
        )
        .fetch_all(exec)
        .await?
        .into_iter()
        .map(|r| ShelfSummaryRow {
            id: r.id,
            name: r.name,
            user_id: r.user_id,
            updated_at: r.updated_at,
            book_count: r.book_count,
            item_last_modified: r.item_last_modified,
            sample_book_ids: r.sample_book_ids,
        })
        .collect()
    } else {
        sqlx::query!(
            r#"
            SELECT s.id, s.name, s.user_id, s.updated_at,
                   COUNT(si.book_id) AS "book_count!",
                   MAX(si.added_at) AS item_last_modified,
                   (
                     SELECT COALESCE(array_agg(book_id), ARRAY[]::uuid[])
                     FROM (
                       SELECT si2.book_id FROM shelf_items si2
                       WHERE si2.shelf_id = s.id
                       ORDER BY si2.added_at DESC
                       LIMIT 3
                     ) sub
                   ) AS "sample_book_ids!"
            FROM shelves s
            LEFT JOIN shelf_items si ON si.shelf_id = s.id
            WHERE s.user_id = $1
            GROUP BY s.id
            ORDER BY s.name ASC, s.id ASC
            LIMIT $2
            "#,
            filter.user_id,
            filter.limit + 1,
        )
        .fetch_all(exec)
        .await?
        .into_iter()
        .map(|r| ShelfSummaryRow {
            id: r.id,
            name: r.name,
            user_id: r.user_id,
            updated_at: r.updated_at,
            book_count: r.book_count,
            item_last_modified: r.item_last_modified,
            sample_book_ids: r.sample_book_ids,
        })
        .collect()
    };

    Ok(rows)
}
