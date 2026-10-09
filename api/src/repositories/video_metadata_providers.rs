//! Video metadata providers repository operations.

use super::Result;
use serde_json::Value;
use sqlx::{Executor, Postgres};
use time::OffsetDateTime;
use uuid::Uuid;

// =============================================================================
// Types
// =============================================================================

/// Stored video provider from database.
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

/// Update request for a video provider.
#[derive(Debug, Default)]
pub struct ProviderUpdate {
    pub enabled: Option<bool>,
    pub priority: Option<i32>,
    pub config: Option<std::collections::HashMap<String, Value>>,
}

// =============================================================================
// List Functions
// =============================================================================

/// Get list of enabled video providers ordered by priority.
pub async fn list_enabled<'e, E>(exec: E) -> Result<Vec<StoredProvider>>
where
    E: Executor<'e, Database = Postgres>,
{
    let rows = sqlx::query!(
        r#"
        SELECT id, provider_type, enabled, priority, config, created_at, updated_at
        FROM video_metadata_providers
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

/// Get all video providers (enabled and disabled) ordered by priority.
pub async fn list_all<'e, E>(exec: E) -> Result<Vec<StoredProvider>>
where
    E: Executor<'e, Database = Postgres>,
{
    let rows = sqlx::query!(
        r#"
        SELECT id, provider_type, enabled, priority, config, created_at, updated_at
        FROM video_metadata_providers
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

/// Update video provider with config merge (fetches current config first).
pub async fn update_with_merge(
    pool: &sqlx::PgPool,
    provider_id: Uuid,
    update: &ProviderUpdate,
) -> Result<bool> {
    let new_config = if let Some(ref config_updates) = update.config {
        let current: Option<Value> = sqlx::query_scalar!(
            "SELECT config FROM video_metadata_providers WHERE id = $1",
            provider_id,
        )
        .fetch_optional(pool)
        .await?;

        let mut merged = current
            .and_then(|v| v.as_object().cloned())
            .unwrap_or_default();

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
        UPDATE video_metadata_providers SET
            updated_at = NOW(),
            enabled = COALESCE($2, enabled),
            priority = COALESCE($3, priority),
            config = COALESCE($4, config)
        WHERE id = $1
        "#,
        provider_id,
        update.enabled,
        update.priority,
        new_config as Option<Value>,
    )
    .execute(pool)
    .await?;

    Ok(result.rows_affected() > 0)
}
