//! Repository for search recents (command palette frecency tracking).

use sqlx::{Executor, Postgres};
use uuid::Uuid;

use super::Result;

/// A recent item with live display data from JOINs.
#[derive(Debug, Clone, serde::Serialize)]
pub struct RecentItem {
    pub entity_type: String,
    pub entity_id: String,
    /// Display label -- resolved from JOINs for DB entities, NULL for static types
    pub label: Option<String>,
    pub access_count: i32,
    #[serde(with = "time::serde::rfc3339")]
    pub last_accessed_at: time::OffsetDateTime,
}

/// Record an access (UPSERT + prune to keep max 50 per user).
pub async fn record_access<'e, E>(
    exec: E,
    user_id: Uuid,
    entity_type: &str,
    entity_id: &str,
) -> Result<()>
where
    E: Executor<'e, Database = Postgres>,
{
    sqlx::query!(
        r#"
        WITH upserted AS (
            INSERT INTO search_recents (user_id, entity_type, entity_id)
            VALUES ($1, $2, $3)
            ON CONFLICT (user_id, entity_type, entity_id)
            DO UPDATE SET
                access_count = search_recents.access_count + 1,
                last_accessed_at = NOW()
            RETURNING id
        ),
        ranked AS (
            SELECT id, ROW_NUMBER() OVER (
                ORDER BY access_count DESC, last_accessed_at DESC
            ) AS rn
            FROM search_recents
            WHERE user_id = $1
        )
        DELETE FROM search_recents
        WHERE id IN (SELECT id FROM ranked WHERE rn > 50)
        "#,
        user_id,
        entity_type,
        entity_id,
    )
    .execute(exec)
    .await?;

    Ok(())
}

/// Delete all recent items for a user.
pub async fn clear_for_user<'e, E>(exec: E, user_id: Uuid) -> Result<()>
where
    E: Executor<'e, Database = Postgres>,
{
    sqlx::query!("DELETE FROM search_recents WHERE user_id = $1", user_id)
        .execute(exec)
        .await?;
    Ok(())
}

/// List recent items for a user with live display data.
///
/// DB entities (book, author, series) are LEFT JOINed to fetch live labels.
/// Stale entities (deleted from DB) are filtered out automatically.
/// Static types (page, command, section) always pass through -- the frontend
/// resolves their display data from its local registry.
pub async fn list_for_user<'e, E>(exec: E, user_id: Uuid, limit: i64) -> Result<Vec<RecentItem>>
where
    E: Executor<'e, Database = Postgres>,
{
    let rows = sqlx::query!(
        r#"
        WITH typed AS (
            SELECT
                sr.*,
                -- Safe UUID extraction: only parse when entity_type is a DB type
                CASE
                    WHEN sr.entity_type IN ('book', 'author', 'series')
                    THEN sr.entity_id::uuid
                    ELSE NULL
                END AS entity_uuid
            FROM search_recents sr
            WHERE sr.user_id = $1
        )
        SELECT
            t.entity_type,
            t.entity_id,
            CASE
                WHEN t.entity_type = 'book' THEN b.title
                WHEN t.entity_type = 'author' THEN a.name
                WHEN t.entity_type = 'series' THEN s.name
                ELSE NULL
            END AS label,
            t.access_count,
            t.last_accessed_at
        FROM typed t
        LEFT JOIN books b ON t.entity_type = 'book' AND b.id = t.entity_uuid
        LEFT JOIN authors a ON t.entity_type = 'author' AND a.id = t.entity_uuid
        LEFT JOIN series s ON t.entity_type = 'series' AND s.id = t.entity_uuid
        WHERE (
            t.entity_type NOT IN ('book', 'author', 'series')
            OR (t.entity_type = 'book' AND b.id IS NOT NULL)
            OR (t.entity_type = 'author' AND a.id IS NOT NULL)
            OR (t.entity_type = 'series' AND s.id IS NOT NULL)
        )
        ORDER BY
            t.access_count + GREATEST(0, 1.0 - EXTRACT(EPOCH FROM (NOW() - t.last_accessed_at)) / 604800.0) DESC
        LIMIT $2
        "#,
        user_id,
        limit,
    )
    .fetch_all(exec)
    .await?;

    Ok(rows
        .into_iter()
        .map(|r| RecentItem {
            entity_type: r.entity_type,
            entity_id: r.entity_id,
            label: r.label,
            access_count: r.access_count,
            last_accessed_at: r.last_accessed_at,
        })
        .collect())
}
