//! User notifications repository operations.

use super::Result;
use sqlx::{Executor, Postgres};
use time::OffsetDateTime;
use uuid::Uuid;

// =============================================================================
// Types
// =============================================================================

/// Stored user notification from database.
#[derive(Debug)]
pub struct UserNotificationRow {
    pub id: Uuid,
    pub user_id: Uuid,
    pub title: String,
    pub message: String,
    pub entity_type: Option<String>,
    pub entity_id: Option<Uuid>,
    pub entity_url: Option<String>,
    pub read: bool,
    pub created_at: OffsetDateTime,
}

/// Input for creating a new user notification.
#[derive(Debug)]
pub struct CreateUserNotification {
    pub user_id: Uuid,
    pub title: String,
    pub message: String,
    pub entity_type: Option<String>,
    pub entity_id: Option<Uuid>,
    pub entity_url: Option<String>,
}

// =============================================================================
// List Functions
// =============================================================================

/// List notifications for a user, paginated, ordered by created_at DESC.
pub async fn list(
    pool: &sqlx::PgPool,
    user_id: Uuid,
    limit: i64,
    offset: i64,
    unread_only: bool,
) -> Result<(Vec<UserNotificationRow>, i64)> {
    let rows = sqlx::query!(
        r#"
        SELECT id, user_id, title, message, entity_type, entity_id, entity_url, read, created_at
        FROM user_notifications
        WHERE user_id = $1 AND ($4 = false OR read = false)
        ORDER BY created_at DESC
        LIMIT $2 OFFSET $3
        "#,
        user_id,
        limit,
        offset,
        unread_only,
    )
    .fetch_all(pool)
    .await?;

    let total = sqlx::query_scalar!(
        r#"
        SELECT COUNT(*) as "count!"
        FROM user_notifications
        WHERE user_id = $1 AND ($2 = false OR read = false)
        "#,
        user_id,
        unread_only,
    )
    .fetch_one(pool)
    .await?;

    Ok((
        rows.into_iter()
            .map(|r| UserNotificationRow {
                id: r.id,
                user_id: r.user_id,
                title: r.title,
                message: r.message,
                entity_type: r.entity_type,
                entity_id: r.entity_id,
                entity_url: r.entity_url,
                read: r.read,
                created_at: r.created_at,
            })
            .collect(),
        total,
    ))
}

// =============================================================================
// Count Functions
// =============================================================================

/// Count unread notifications for a user.
pub async fn count_unread<'e, E>(exec: E, user_id: Uuid) -> Result<i64>
where
    E: Executor<'e, Database = Postgres>,
{
    let count = sqlx::query_scalar!(
        r#"
        SELECT COUNT(*) as "count!"
        FROM user_notifications
        WHERE user_id = $1 AND read = false
        "#,
        user_id,
    )
    .fetch_one(exec)
    .await?;

    Ok(count)
}

// =============================================================================
// Create Functions
// =============================================================================

/// Create a new user notification.
pub async fn create<'e, E>(exec: E, input: CreateUserNotification) -> Result<UserNotificationRow>
where
    E: Executor<'e, Database = Postgres>,
{
    let r = sqlx::query!(
        r#"
        INSERT INTO user_notifications (user_id, title, message, entity_type, entity_id, entity_url)
        VALUES ($1, $2, $3, $4, $5, $6)
        RETURNING id, user_id, title, message, entity_type, entity_id, entity_url, read, created_at
        "#,
        input.user_id,
        input.title,
        input.message,
        input.entity_type,
        input.entity_id,
        input.entity_url,
    )
    .fetch_one(exec)
    .await?;

    Ok(UserNotificationRow {
        id: r.id,
        user_id: r.user_id,
        title: r.title,
        message: r.message,
        entity_type: r.entity_type,
        entity_id: r.entity_id,
        entity_url: r.entity_url,
        read: r.read,
        created_at: r.created_at,
    })
}

// =============================================================================
// Update Functions
// =============================================================================

/// Mark a single notification as read. Returns true if the notification was found and updated.
pub async fn mark_read<'e, E>(exec: E, id: Uuid, user_id: Uuid) -> Result<bool>
where
    E: Executor<'e, Database = Postgres>,
{
    let result = sqlx::query!(
        r#"
        UPDATE user_notifications
        SET read = true
        WHERE id = $1 AND user_id = $2 AND read = false
        "#,
        id,
        user_id,
    )
    .execute(exec)
    .await?;

    Ok(result.rows_affected() > 0)
}

/// Mark all unread notifications as read for a user. Returns count of affected rows.
pub async fn mark_all_read<'e, E>(exec: E, user_id: Uuid) -> Result<u64>
where
    E: Executor<'e, Database = Postgres>,
{
    let result = sqlx::query!(
        r#"
        UPDATE user_notifications
        SET read = true
        WHERE user_id = $1 AND read = false
        "#,
        user_id,
    )
    .execute(exec)
    .await?;

    Ok(result.rows_affected())
}
