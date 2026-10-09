//! Metadata providers repository operations.

use super::Result;
use serde_json::Value;
use sqlx::{Executor, Postgres};
use time::OffsetDateTime;
use uuid::Uuid;

// =============================================================================
// Types
// =============================================================================

/// Stored provider from database.
#[derive(Debug)]
pub struct StoredProvider {
    pub id: Uuid,
    pub provider_type: String,
    pub enabled: bool,
    pub priority: i32,
    pub config: Value,
    pub created_at: OffsetDateTime,
    pub updated_at: OffsetDateTime,
}

/// Update request for a provider.
#[derive(Debug, Default)]
pub struct ProviderUpdate {
    pub enabled: Option<bool>,
    pub priority: Option<i32>,
    pub config: Option<std::collections::HashMap<String, Value>>,
}

/// Get config for a specific provider type (if enabled).
pub async fn get_config<'e, E>(exec: E, provider_type: &str) -> Result<Option<Value>>
where
    E: Executor<'e, Database = Postgres>,
{
    let config: Option<Value> = sqlx::query_scalar!(
        "SELECT config FROM metadata_providers WHERE provider_type = $1 AND enabled = TRUE",
        provider_type,
    )
    .fetch_optional(exec)
    .await?;

    Ok(config)
}

/// Get API key from provider config.
pub async fn get_api_key<'e, E>(exec: E, provider_type: &str) -> Result<Option<String>>
where
    E: Executor<'e, Database = Postgres>,
{
    let config = get_config(exec, provider_type).await?;
    Ok(config.and_then(|c| c.get("api_key").and_then(|k| k.as_str().map(String::from))))
}

// =============================================================================
// List Functions
// =============================================================================

/// Get list of enabled providers ordered by priority.
pub async fn list_enabled<'e, E>(exec: E) -> Result<Vec<StoredProvider>>
where
    E: Executor<'e, Database = Postgres>,
{
    let rows = sqlx::query!(
        r#"
        SELECT id, provider_type, enabled, priority, config, created_at, updated_at
        FROM metadata_providers
        WHERE enabled = true
        ORDER BY priority ASC
        "#,
    )
    .fetch_all(exec)
    .await?;

    Ok(rows
        .into_iter()
        .map(|r| StoredProvider {
            id: r.id,
            provider_type: r.provider_type,
            enabled: r.enabled,
            priority: r.priority,
            config: r.config,
            created_at: r.created_at,
            updated_at: r.updated_at,
        })
        .collect())
}

/// Get all providers (enabled and disabled) ordered by priority.
pub async fn list_all<'e, E>(exec: E) -> Result<Vec<StoredProvider>>
where
    E: Executor<'e, Database = Postgres>,
{
    let rows = sqlx::query!(
        r#"
        SELECT id, provider_type, enabled, priority, config, created_at, updated_at
        FROM metadata_providers
        ORDER BY priority ASC
        "#,
    )
    .fetch_all(exec)
    .await?;

    Ok(rows
        .into_iter()
        .map(|r| StoredProvider {
            id: r.id,
            provider_type: r.provider_type,
            enabled: r.enabled,
            priority: r.priority,
            config: r.config,
            created_at: r.created_at,
            updated_at: r.updated_at,
        })
        .collect())
}

// =============================================================================
// Update Functions
// =============================================================================

/// Update provider configuration.
pub async fn update<'e, E>(exec: E, provider_id: Uuid, update: &ProviderUpdate) -> Result<bool>
where
    E: Executor<'e, Database = Postgres>,
{
    // If config updates provided, we need to merge with existing config
    // For that we need a separate query to get current config
    let new_config = if let Some(ref config_updates) = update.config {
        // For now, serialize updates directly - caller should have merged already
        // Or we can do the merge here with an additional query
        let mut merged = serde_json::Map::new();

        // Merge updates (null values remove keys)
        for (key, value) in config_updates {
            if value.is_null() {
                merged.remove(key);
            } else {
                // Normalize API keys: strip "Bearer " prefix
                let normalized_value = if key == "api_key" {
                    if let Some(s) = value.as_str() {
                        let trimmed = s.trim();
                        if trimmed.to_lowercase().starts_with("bearer ") {
                            Value::String(trimmed[7..].trim().to_string())
                        } else {
                            value.clone()
                        }
                    } else {
                        value.clone()
                    }
                } else {
                    value.clone()
                };
                merged.insert(key.clone(), normalized_value);
            }
        }

        Some(Value::Object(merged))
    } else {
        None
    };

    // Build dynamic update query based on what's being updated
    let result = sqlx::query!(
        r#"
        UPDATE metadata_providers SET
            updated_at = NOW(),
            enabled = COALESCE($2, enabled),
            priority = COALESCE($3, priority),
            config = COALESCE($4, config)
        WHERE id = $1
        "#,
        provider_id,
        update.enabled,
        update.priority,
        new_config,
    )
    .execute(exec)
    .await?;

    Ok(result.rows_affected() > 0)
}

/// Update provider with config merge (fetches current config first).
pub async fn update_with_merge(
    pool: &sqlx::PgPool,
    provider_id: Uuid,
    update: &ProviderUpdate,
) -> Result<bool> {
    // If config updates provided, merge with existing config
    let new_config = if let Some(ref config_updates) = update.config {
        // Get current config
        let current: Option<Value> = sqlx::query_scalar!(
            "SELECT config FROM metadata_providers WHERE id = $1",
            provider_id,
        )
        .fetch_optional(pool)
        .await?;

        let mut merged = current
            .and_then(|v| v.as_object().cloned())
            .unwrap_or_default();

        // Merge updates (null values remove keys)
        for (key, value) in config_updates {
            if value.is_null() {
                merged.remove(key);
            } else {
                // Normalize API keys: strip "Bearer " prefix
                let normalized_value = if key == "api_key" {
                    if let Some(s) = value.as_str() {
                        let trimmed = s.trim();
                        if trimmed.to_lowercase().starts_with("bearer ") {
                            Value::String(trimmed[7..].trim().to_string())
                        } else {
                            value.clone()
                        }
                    } else {
                        value.clone()
                    }
                } else {
                    value.clone()
                };
                merged.insert(key.clone(), normalized_value);
            }
        }

        Some(Value::Object(merged))
    } else {
        None
    };

    let result = sqlx::query!(
        r#"
        UPDATE metadata_providers SET
            updated_at = NOW(),
            enabled = COALESCE($2, enabled),
            priority = COALESCE($3, priority),
            config = COALESCE($4, config)
        WHERE id = $1
        "#,
        provider_id,
        update.enabled,
        update.priority,
        new_config,
    )
    .execute(pool)
    .await?;

    Ok(result.rows_affected() > 0)
}
