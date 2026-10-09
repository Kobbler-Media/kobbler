//! IPTV favourites repository operations.

use super::Result;
use sqlx::{Executor, Postgres};
use time::OffsetDateTime;
use uuid::Uuid;

// =============================================================================
// Types
// =============================================================================

#[derive(Debug)]
pub struct FavouriteRow {
    pub user_id: Uuid,
    pub channel_id: Uuid,
    pub display_order: i32,
    pub created_at: OffsetDateTime,
}

// =============================================================================
// Operations
// =============================================================================

/// Add a channel to a user's favourites.
pub async fn add<'e, E>(exec: E, user_id: Uuid, channel_id: Uuid) -> Result<()>
where
    E: Executor<'e, Database = Postgres>,
{
    sqlx::query!(
        r#"
        INSERT INTO iptv_favourites (user_id, channel_id, display_order)
        VALUES ($1, $2, COALESCE(
            (SELECT MAX(display_order) + 1 FROM iptv_favourites WHERE user_id = $1),
            0
        ))
        ON CONFLICT (user_id, channel_id) DO NOTHING
        "#,
        user_id,
        channel_id,
    )
    .execute(exec)
    .await?;

    Ok(())
}

/// Remove a channel from a user's favourites.
pub async fn remove<'e, E>(exec: E, user_id: Uuid, channel_id: Uuid) -> Result<bool>
where
    E: Executor<'e, Database = Postgres>,
{
    let result = sqlx::query!(
        r#"
        DELETE FROM iptv_favourites WHERE user_id = $1 AND channel_id = $2
        "#,
        user_id,
        channel_id,
    )
    .execute(exec)
    .await?;

    Ok(result.rows_affected() > 0)
}

/// List a user's favourite channels ordered by display_order.
pub async fn list_for_user<'e, E>(exec: E, user_id: Uuid) -> Result<Vec<FavouriteRow>>
where
    E: Executor<'e, Database = Postgres>,
{
    let rows = sqlx::query!(
        r#"
        SELECT user_id, channel_id, display_order, created_at
        FROM iptv_favourites
        WHERE user_id = $1
        ORDER BY display_order ASC
        "#,
        user_id,
    )
    .fetch_all(exec)
    .await?;

    Ok(rows
        .into_iter()
        .map(|r| FavouriteRow {
            user_id: r.user_id,
            channel_id: r.channel_id,
            display_order: r.display_order,
            created_at: r.created_at,
        })
        .collect())
}

// =============================================================================
// Group Favourites
// =============================================================================

/// Add a channel group to a user's favourites.
pub async fn add_group<'e, E>(exec: E, user_id: Uuid, group_id: Uuid) -> Result<()>
where
    E: Executor<'e, Database = Postgres>,
{
    sqlx::query(
        r#"
        INSERT INTO iptv_favourite_groups (user_id, group_id)
        VALUES ($1, $2)
        ON CONFLICT (user_id, group_id) DO NOTHING
        "#,
    )
    .bind(user_id)
    .bind(group_id)
    .execute(exec)
    .await?;

    Ok(())
}

/// Remove a channel group from a user's favourites.
pub async fn remove_group<'e, E>(exec: E, user_id: Uuid, group_id: Uuid) -> Result<bool>
where
    E: Executor<'e, Database = Postgres>,
{
    let result = sqlx::query(
        r#"
        DELETE FROM iptv_favourite_groups WHERE user_id = $1 AND group_id = $2
        "#,
    )
    .bind(user_id)
    .bind(group_id)
    .execute(exec)
    .await?;

    Ok(result.rows_affected() > 0)
}

/// List a user's favourite group IDs.
pub async fn list_favourite_group_ids(pool: &sqlx::PgPool, user_id: Uuid) -> Result<Vec<Uuid>> {
    let rows: Vec<(Uuid,)> =
        sqlx::query_as("SELECT group_id FROM iptv_favourite_groups WHERE user_id = $1")
            .bind(user_id)
            .fetch_all(pool)
            .await?;

    Ok(rows.into_iter().map(|(id,)| id).collect())
}

/// Reorder a favourite channel for a user.
pub async fn reorder<'e, E>(
    exec: E,
    user_id: Uuid,
    channel_id: Uuid,
    new_order: i32,
) -> Result<bool>
where
    E: Executor<'e, Database = Postgres>,
{
    let result = sqlx::query!(
        r#"
        UPDATE iptv_favourites SET display_order = $3
        WHERE user_id = $1 AND channel_id = $2
        "#,
        user_id,
        channel_id,
        new_order,
    )
    .execute(exec)
    .await?;

    Ok(result.rows_affected() > 0)
}
