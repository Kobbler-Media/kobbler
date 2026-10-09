//! Notification agents repository operations.

use super::Result;
use serde_json::Value;
use sqlx::{Executor, Postgres};
use time::OffsetDateTime;
use uuid::Uuid;

// =============================================================================
// Types
// =============================================================================

/// Stored notification agent from database.
#[derive(Debug)]
pub struct NotificationAgentRow {
    pub id: Uuid,
    pub agent_type: String,
    pub name: String,
    pub enabled: bool,
    pub config: Value,
    pub triggers: Value,
    pub created_at: OffsetDateTime,
    pub updated_at: OffsetDateTime,
}

/// Input for creating a new notification agent.
#[derive(Debug)]
pub struct CreateNotificationAgent {
    pub agent_type: String,
    pub name: String,
    pub enabled: bool,
    pub config: Value,
    pub triggers: Value,
}

/// Input for updating an existing notification agent.
#[derive(Debug, Default)]
pub struct UpdateNotificationAgent {
    pub name: Option<String>,
    pub enabled: Option<bool>,
    pub config: Option<std::collections::HashMap<String, Value>>,
    pub triggers: Option<Value>,
}

// =============================================================================
// List Functions
// =============================================================================

/// Get all notification agents ordered by created_at.
pub async fn list_all<'e, E>(exec: E) -> Result<Vec<NotificationAgentRow>>
where
    E: Executor<'e, Database = Postgres>,
{
    let rows = sqlx::query!(
        r#"
        SELECT id, agent_type, name, enabled, config, triggers, created_at, updated_at
        FROM notification_agents
        ORDER BY created_at ASC
        "#,
    )
    .fetch_all(exec)
    .await?;

    Ok(rows
        .into_iter()
        .map(|r| NotificationAgentRow {
            id: r.id,
            agent_type: r.agent_type,
            name: r.name,
            enabled: r.enabled,
            config: r.config,
            triggers: r.triggers,
            created_at: r.created_at,
            updated_at: r.updated_at,
        })
        .collect())
}

/// Get enabled notification agents only.
pub async fn list_enabled<'e, E>(exec: E) -> Result<Vec<NotificationAgentRow>>
where
    E: Executor<'e, Database = Postgres>,
{
    let rows = sqlx::query!(
        r#"
        SELECT id, agent_type, name, enabled, config, triggers, created_at, updated_at
        FROM notification_agents
        WHERE enabled = true
        ORDER BY created_at ASC
        "#,
    )
    .fetch_all(exec)
    .await?;

    Ok(rows
        .into_iter()
        .map(|r| NotificationAgentRow {
            id: r.id,
            agent_type: r.agent_type,
            name: r.name,
            enabled: r.enabled,
            config: r.config,
            triggers: r.triggers,
            created_at: r.created_at,
            updated_at: r.updated_at,
        })
        .collect())
}

// =============================================================================
// Get Functions
// =============================================================================

/// Get a notification agent by ID.
pub async fn get_by_id<'e, E>(exec: E, id: Uuid) -> Result<Option<NotificationAgentRow>>
where
    E: Executor<'e, Database = Postgres>,
{
    let row = sqlx::query!(
        r#"
        SELECT id, agent_type, name, enabled, config, triggers, created_at, updated_at
        FROM notification_agents
        WHERE id = $1
        "#,
        id,
    )
    .fetch_optional(exec)
    .await?;

    Ok(row.map(|r| NotificationAgentRow {
        id: r.id,
        agent_type: r.agent_type,
        name: r.name,
        enabled: r.enabled,
        config: r.config,
        triggers: r.triggers,
        created_at: r.created_at,
        updated_at: r.updated_at,
    }))
}

// =============================================================================
// Create Functions
// =============================================================================

/// Create a new notification agent.
pub async fn create<'e, E>(exec: E, input: CreateNotificationAgent) -> Result<NotificationAgentRow>
where
    E: Executor<'e, Database = Postgres>,
{
    let r = sqlx::query!(
        r#"
        INSERT INTO notification_agents (agent_type, name, enabled, config, triggers)
        VALUES ($1, $2, $3, $4, $5)
        RETURNING id, agent_type, name, enabled, config, triggers, created_at, updated_at
        "#,
        input.agent_type,
        input.name,
        input.enabled,
        input.config,
        input.triggers,
    )
    .fetch_one(exec)
    .await?;

    Ok(NotificationAgentRow {
        id: r.id,
        agent_type: r.agent_type,
        name: r.name,
        enabled: r.enabled,
        config: r.config,
        triggers: r.triggers,
        created_at: r.created_at,
        updated_at: r.updated_at,
    })
}

// =============================================================================
// Update Functions
// =============================================================================

/// Update a notification agent with config merge (fetches current config first).
///
/// Returns the updated row, or None if the agent was not found.
pub async fn update(
    pool: &sqlx::PgPool,
    id: Uuid,
    input: &UpdateNotificationAgent,
) -> Result<Option<NotificationAgentRow>> {
    let new_config = if let Some(ref config_updates) = input.config {
        let current: Option<Value> =
            sqlx::query_scalar!("SELECT config FROM notification_agents WHERE id = $1", id,)
                .fetch_optional(pool)
                .await?;

        let mut merged = current
            .and_then(|v| v.as_object().cloned())
            .unwrap_or_default();

        for (key, value) in config_updates {
            if value.is_null() {
                merged.remove(key);
            } else {
                merged.insert(key.clone(), value.clone());
            }
        }

        Some(Value::Object(merged))
    } else {
        None
    };

    let row = sqlx::query!(
        r#"
        UPDATE notification_agents SET
            updated_at = NOW(),
            name = COALESCE($2, name),
            enabled = COALESCE($3, enabled),
            config = COALESCE($4, config),
            triggers = COALESCE($5, triggers)
        WHERE id = $1
        RETURNING id, agent_type, name, enabled, config, triggers, created_at, updated_at
        "#,
        id,
        input.name.as_deref(),
        input.enabled,
        new_config,
        input.triggers,
    )
    .fetch_optional(pool)
    .await?;

    Ok(row.map(|r| NotificationAgentRow {
        id: r.id,
        agent_type: r.agent_type,
        name: r.name,
        enabled: r.enabled,
        config: r.config,
        triggers: r.triggers,
        created_at: r.created_at,
        updated_at: r.updated_at,
    }))
}

// =============================================================================
// Delete Functions
// =============================================================================

/// Delete a notification agent by ID. Returns true if a row was deleted.
pub async fn delete<'e, E>(exec: E, id: Uuid) -> Result<bool>
where
    E: Executor<'e, Database = Postgres>,
{
    let result = sqlx::query!(
        r#"
        DELETE FROM notification_agents
        WHERE id = $1
        "#,
        id,
    )
    .execute(exec)
    .await?;

    Ok(result.rows_affected() > 0)
}
