//! Web push subscriptions repository operations.

use super::Result;
use sqlx::{Executor, Postgres};
use time::OffsetDateTime;
use uuid::Uuid;

// =============================================================================
// Types
// =============================================================================

/// Stored web push subscription from database.
#[derive(Debug)]
pub struct WebPushSubscriptionRow {
    pub id: Uuid,
    pub user_id: Uuid,
    pub endpoint: String,
    pub p256dh: String,
    pub auth: String,
    pub user_agent: Option<String>,
    pub created_at: OffsetDateTime,
}

/// Input for creating a new web push subscription.
#[derive(Debug)]
pub struct CreateWebPushSubscription {
    pub user_id: Uuid,
    pub endpoint: String,
    pub p256dh: String,
    pub auth: String,
    pub user_agent: Option<String>,
}

// =============================================================================
// List Functions
// =============================================================================

/// Get all web push subscriptions.
pub async fn list_all<'e, E>(exec: E) -> Result<Vec<WebPushSubscriptionRow>>
where
    E: Executor<'e, Database = Postgres>,
{
    let rows = sqlx::query!(
        r#"
        SELECT id, user_id, endpoint, p256dh, auth, user_agent, created_at
        FROM webpush_subscriptions
        ORDER BY created_at ASC
        "#,
    )
    .fetch_all(exec)
    .await?;

    Ok(rows
        .into_iter()
        .map(|r| WebPushSubscriptionRow {
            id: r.id,
            user_id: r.user_id,
            endpoint: r.endpoint,
            p256dh: r.p256dh,
            auth: r.auth,
            user_agent: r.user_agent,
            created_at: r.created_at,
        })
        .collect())
}

/// Subscriptions belonging to users with any of `roles`.
pub async fn list_for_roles<'e, E>(exec: E, roles: &[&str]) -> Result<Vec<WebPushSubscriptionRow>>
where
    E: Executor<'e, Database = Postgres>,
{
    let roles: Vec<String> = roles.iter().map(|r| r.to_string()).collect();
    let rows = sqlx::query!(
        r#"
        SELECT s.id, s.user_id, s.endpoint, s.p256dh, s.auth, s.user_agent, s.created_at
        FROM webpush_subscriptions s
        JOIN users u ON u.id = s.user_id
        WHERE u.role = ANY($1)
        ORDER BY s.created_at ASC
        "#,
        &roles,
    )
    .fetch_all(exec)
    .await?;

    Ok(rows
        .into_iter()
        .map(|r| WebPushSubscriptionRow {
            id: r.id,
            user_id: r.user_id,
            endpoint: r.endpoint,
            p256dh: r.p256dh,
            auth: r.auth,
            user_agent: r.user_agent,
            created_at: r.created_at,
        })
        .collect())
}

// =============================================================================
// Create Functions
// =============================================================================

/// Create or update a web push subscription (upsert on endpoint).
pub async fn create<'e, E>(
    exec: E,
    input: CreateWebPushSubscription,
) -> Result<WebPushSubscriptionRow>
where
    E: Executor<'e, Database = Postgres>,
{
    let r = sqlx::query!(
        r#"
        INSERT INTO webpush_subscriptions (user_id, endpoint, p256dh, auth, user_agent)
        VALUES ($1, $2, $3, $4, $5)
        ON CONFLICT (endpoint) DO UPDATE SET
            user_id = EXCLUDED.user_id,
            p256dh = EXCLUDED.p256dh,
            auth = EXCLUDED.auth,
            user_agent = EXCLUDED.user_agent
        RETURNING id, user_id, endpoint, p256dh, auth, user_agent, created_at
        "#,
        input.user_id,
        input.endpoint,
        input.p256dh,
        input.auth,
        input.user_agent,
    )
    .fetch_one(exec)
    .await?;

    Ok(WebPushSubscriptionRow {
        id: r.id,
        user_id: r.user_id,
        endpoint: r.endpoint,
        p256dh: r.p256dh,
        auth: r.auth,
        user_agent: r.user_agent,
        created_at: r.created_at,
    })
}

// =============================================================================
// Delete Functions
// =============================================================================

/// Delete one of `user_id`'s subscriptions by endpoint. Returns true if a row
/// was deleted. Scoped by user so an endpoint cannot be revoked by anyone else.
pub async fn delete_by_endpoint<'e, E>(exec: E, user_id: Uuid, endpoint: &str) -> Result<bool>
where
    E: Executor<'e, Database = Postgres>,
{
    let result = sqlx::query!(
        r#"
        DELETE FROM webpush_subscriptions
        WHERE endpoint = $1 AND user_id = $2
        "#,
        endpoint,
        user_id,
    )
    .execute(exec)
    .await?;

    Ok(result.rows_affected() > 0)
}

/// Delete a web push subscription by ID. Returns true if a row was deleted.
pub async fn delete_by_id<'e, E>(exec: E, id: Uuid) -> Result<bool>
where
    E: Executor<'e, Database = Postgres>,
{
    let result = sqlx::query!(
        r#"
        DELETE FROM webpush_subscriptions
        WHERE id = $1
        "#,
        id,
    )
    .execute(exec)
    .await?;

    Ok(result.rows_affected() > 0)
}
