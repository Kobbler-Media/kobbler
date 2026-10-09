//! User sync provider repository operations.
//!
//! Per-user sync provider configuration (API keys, OAuth tokens, enabled flags)
//! and per-device sync provider toggles. Used for both book sync (Hardcover)
//! and video sync (Trakt, etc.) providers.

use super::Result;
use serde_json::Value;
use sqlx::{Executor, Postgres};
use time::OffsetDateTime;
use uuid::Uuid;

// =============================================================================
// Types
// =============================================================================

/// Stored user sync provider configuration.
#[derive(Debug)]
pub struct StoredProvider {
    pub id: Uuid,
    pub user_id: Uuid,
    pub provider_type: String,
    pub enabled: bool,
    pub config: Value,
    pub created_at: OffsetDateTime,
    pub updated_at: OffsetDateTime,
}

// =============================================================================
// Per-User Provider Config
// =============================================================================

/// Get a user's sync provider config by type.
pub async fn get_by_type<'e, E>(
    exec: E,
    user_id: Uuid,
    provider_type: &str,
) -> Result<Option<StoredProvider>>
where
    E: Executor<'e, Database = Postgres>,
{
    let row = sqlx::query!(
        r#"
        SELECT id, user_id, provider_type, enabled, config, created_at, updated_at
        FROM user_sync_providers
        WHERE user_id = $1 AND provider_type = $2
        "#,
        user_id,
        provider_type,
    )
    .fetch_optional(exec)
    .await?;

    Ok(row.map(|r| StoredProvider {
        id: r.id,
        user_id: r.user_id,
        provider_type: r.provider_type,
        enabled: r.enabled,
        config: r.config,
        created_at: r.created_at,
        updated_at: r.updated_at,
    }))
}

/// Get a user's API key for a specific sync provider.
pub async fn get_api_key<'e, E>(
    exec: E,
    user_id: Uuid,
    provider_type: &str,
) -> Result<Option<String>>
where
    E: Executor<'e, Database = Postgres>,
{
    let config: Option<Value> = sqlx::query_scalar!(
        r#"
        SELECT config FROM user_sync_providers
        WHERE user_id = $1 AND provider_type = $2 AND enabled = true
        "#,
        user_id,
        provider_type,
    )
    .fetch_optional(exec)
    .await?;

    Ok(config.and_then(|c| c.get("api_key").and_then(|v| v.as_str()).map(String::from)))
}

/// Upsert a user's sync provider config.
pub async fn upsert<'e, E>(
    exec: E,
    user_id: Uuid,
    provider_type: &str,
    enabled: bool,
    config: &Value,
) -> Result<Uuid>
where
    E: Executor<'e, Database = Postgres>,
{
    let id: Uuid = sqlx::query_scalar!(
        r#"
        INSERT INTO user_sync_providers (user_id, provider_type, enabled, config)
        VALUES ($1, $2, $3, $4)
        ON CONFLICT (user_id, provider_type) DO UPDATE SET
            enabled = $3,
            config = $4,
            updated_at = NOW()
        RETURNING id
        "#,
        user_id,
        provider_type,
        enabled,
        config,
    )
    .fetch_one(exec)
    .await?;

    Ok(id)
}

/// Update just the enabled flag for a user's sync provider.
pub async fn update_enabled<'e, E>(
    exec: E,
    user_id: Uuid,
    provider_type: &str,
    enabled: bool,
) -> Result<bool>
where
    E: Executor<'e, Database = Postgres>,
{
    let result = sqlx::query!(
        r#"
        UPDATE user_sync_providers SET enabled = $3, updated_at = NOW()
        WHERE user_id = $1 AND provider_type = $2
        "#,
        user_id,
        provider_type,
        enabled,
    )
    .execute(exec)
    .await?;

    Ok(result.rows_affected() > 0)
}

/// Update the config JSONB with merge semantics (null values remove keys).
pub async fn update_config_merge(
    pool: &sqlx::PgPool,
    user_id: Uuid,
    provider_type: &str,
    config_updates: &std::collections::HashMap<String, Value>,
) -> Result<bool> {
    let current: Option<Value> = sqlx::query_scalar!(
        "SELECT config FROM user_sync_providers WHERE user_id = $1 AND provider_type = $2",
        user_id,
        provider_type,
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
            merged.insert(key.clone(), value.clone());
        }
    }

    let new_config = Value::Object(merged);

    let result = sqlx::query!(
        r#"
        UPDATE user_sync_providers SET config = $3, updated_at = NOW()
        WHERE user_id = $1 AND provider_type = $2
        "#,
        user_id,
        provider_type,
        new_config,
    )
    .execute(pool)
    .await?;

    Ok(result.rows_affected() > 0)
}

/// List all sync providers for a user.
pub async fn list_for_user<'e, E>(exec: E, user_id: Uuid) -> Result<Vec<StoredProvider>>
where
    E: Executor<'e, Database = Postgres>,
{
    let rows = sqlx::query!(
        r#"
        SELECT id, user_id, provider_type, enabled, config, created_at, updated_at
        FROM user_sync_providers
        WHERE user_id = $1
        ORDER BY provider_type ASC
        "#,
        user_id,
    )
    .fetch_all(exec)
    .await?;

    Ok(rows
        .into_iter()
        .map(|r| StoredProvider {
            id: r.id,
            user_id: r.user_id,
            provider_type: r.provider_type,
            enabled: r.enabled,
            config: r.config,
            created_at: r.created_at,
            updated_at: r.updated_at,
        })
        .collect())
}

/// List all enabled sync providers for a user (with API key present).
pub async fn list_enabled_for_user<'e, E>(exec: E, user_id: Uuid) -> Result<Vec<StoredProvider>>
where
    E: Executor<'e, Database = Postgres>,
{
    let rows = sqlx::query!(
        r#"
        SELECT id, user_id, provider_type, enabled, config, created_at, updated_at
        FROM user_sync_providers
        WHERE user_id = $1 AND enabled = true AND config->>'api_key' IS NOT NULL
        ORDER BY provider_type ASC
        "#,
        user_id,
    )
    .fetch_all(exec)
    .await?;

    Ok(rows
        .into_iter()
        .map(|r| StoredProvider {
            id: r.id,
            user_id: r.user_id,
            provider_type: r.provider_type,
            enabled: r.enabled,
            config: r.config,
            created_at: r.created_at,
            updated_at: r.updated_at,
        })
        .collect())
}

/// List all users with a specific provider enabled and configured.
pub async fn list_users_with_provider<'e, E>(
    exec: E,
    provider_type: &str,
) -> Result<Vec<(Uuid, String)>>
where
    E: Executor<'e, Database = Postgres>,
{
    let rows = sqlx::query!(
        r#"
        SELECT user_id, config->>'api_key' AS "api_key!"
        FROM user_sync_providers
        WHERE provider_type = $1 AND enabled = true AND config->>'api_key' IS NOT NULL
        "#,
        provider_type,
    )
    .fetch_all(exec)
    .await?;

    Ok(rows.into_iter().map(|r| (r.user_id, r.api_key)).collect())
}

/// Delete a user's sync provider config.
pub async fn delete<'e, E>(exec: E, user_id: Uuid, provider_type: &str) -> Result<bool>
where
    E: Executor<'e, Database = Postgres>,
{
    let result = sqlx::query!(
        "DELETE FROM user_sync_providers WHERE user_id = $1 AND provider_type = $2",
        user_id,
        provider_type,
    )
    .execute(exec)
    .await?;

    Ok(result.rows_affected() > 0)
}

// =============================================================================
// Per-Device Provider Toggles
// =============================================================================

/// Check if a sync provider is enabled for a device.
/// Defaults to true if no explicit row exists.
pub async fn is_device_enabled<'e, E>(exec: E, device_id: Uuid, provider_type: &str) -> Result<bool>
where
    E: Executor<'e, Database = Postgres>,
{
    let enabled: Option<bool> = sqlx::query_scalar!(
        "SELECT enabled FROM device_sync_providers WHERE device_id = $1 AND provider_type = $2",
        device_id,
        provider_type,
    )
    .fetch_optional(exec)
    .await?;

    Ok(enabled.unwrap_or(true))
}

/// Upsert the device-level sync provider toggle.
pub async fn upsert_device_enabled<'e, E>(
    exec: E,
    device_id: Uuid,
    provider_type: &str,
    enabled: bool,
) -> Result<()>
where
    E: Executor<'e, Database = Postgres>,
{
    sqlx::query!(
        r#"
        INSERT INTO device_sync_providers (device_id, provider_type, enabled)
        VALUES ($1, $2, $3)
        ON CONFLICT (device_id, provider_type) DO UPDATE SET enabled = $3
        "#,
        device_id,
        provider_type,
        enabled,
    )
    .execute(exec)
    .await?;

    Ok(())
}

/// List all enabled sync provider types for a device.
/// Returns provider types that are enabled at BOTH user and device level.
pub async fn list_enabled_for_device<'e, E>(
    exec: E,
    device_id: Uuid,
) -> Result<Vec<(String, String)>>
where
    E: Executor<'e, Database = Postgres>,
{
    struct Row {
        provider_type: String,
        api_key: String,
    }

    let rows = sqlx::query_as!(
        Row,
        r#"
        SELECT usp.provider_type, usp.config->>'api_key' AS "api_key!"
        FROM user_sync_providers usp
        JOIN devices d ON d.user_id = usp.user_id
        WHERE d.id = $1
          AND usp.enabled = true
          AND usp.config->>'api_key' IS NOT NULL
          AND NOT EXISTS (
              SELECT 1 FROM device_sync_providers dsp
              WHERE dsp.device_id = $1
                AND dsp.provider_type = usp.provider_type
                AND dsp.enabled = false
          )
        "#,
        device_id,
    )
    .fetch_all(exec)
    .await?;

    Ok(rows
        .into_iter()
        .map(|r| (r.provider_type, r.api_key))
        .collect())
}

// =============================================================================
// Provider Read ID Tracking (JSONB on user_book_data)
// =============================================================================

/// Get the stored read ID for a specific sync provider on a user+book.
pub async fn get_provider_read_id<'e, E>(
    exec: E,
    user_id: Uuid,
    book_id: Uuid,
    provider_type: &str,
) -> Result<Option<i64>>
where
    E: Executor<'e, Database = Postgres>,
{
    let row = sqlx::query!(
        r#"
        SELECT provider_read_ids
        FROM user_book_data
        WHERE user_id = $1 AND book_id = $2
        "#,
        user_id,
        book_id,
    )
    .fetch_optional(exec)
    .await?;

    Ok(row
        .and_then(|r| r.provider_read_ids.get(provider_type).cloned())
        .and_then(|v: Value| v.as_i64()))
}

/// Store a read ID for a specific sync provider on a user+book.
pub async fn set_provider_read_id<'e, E>(
    exec: E,
    user_id: Uuid,
    book_id: Uuid,
    provider_type: &str,
    read_id: i64,
) -> Result<()>
where
    E: Executor<'e, Database = Postgres>,
{
    let read_id_json = serde_json::json!({ provider_type: read_id });
    sqlx::query!(
        r#"
        INSERT INTO user_book_data (user_id, book_id, provider_read_ids)
        VALUES ($1, $2, $3)
        ON CONFLICT (user_id, book_id) DO UPDATE SET
            provider_read_ids = user_book_data.provider_read_ids || $3,
            updated_at = NOW()
        "#,
        user_id,
        book_id,
        read_id_json,
    )
    .execute(exec)
    .await?;

    Ok(())
}

/// Clear the read ID for a specific sync provider on a user+book.
pub async fn clear_provider_read_id<'e, E>(
    exec: E,
    user_id: Uuid,
    book_id: Uuid,
    provider_type: &str,
) -> Result<()>
where
    E: Executor<'e, Database = Postgres>,
{
    sqlx::query!(
        r#"
        UPDATE user_book_data
        SET provider_read_ids = provider_read_ids - $3::text,
            updated_at = NOW()
        WHERE user_id = $1 AND book_id = $2
        "#,
        user_id,
        book_id,
        provider_type,
    )
    .execute(exec)
    .await?;

    Ok(())
}
