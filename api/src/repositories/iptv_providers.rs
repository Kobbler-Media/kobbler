//! IPTV providers repository operations.

use super::Result;
use serde_json::Value;
use sqlx::{Executor, Postgres};
use time::OffsetDateTime;
use uuid::Uuid;

// =============================================================================
// Types
// =============================================================================

#[derive(Debug)]
pub struct ProviderRow {
    pub id: Uuid,
    pub name: String,
    pub url: String,
    pub username: String,
    pub password: String,
    pub preferred_format: String,
    pub sync_interval_secs: i32,
    pub enabled: bool,
    pub account_info: Value,
    pub last_sync_at: Option<OffsetDateTime>,
    pub last_sync_error: Option<String>,
    pub consecutive_sync_failures: i32,
    pub next_sync_retry_at: Option<OffsetDateTime>,
    pub created_at: OffsetDateTime,
    pub updated_at: OffsetDateTime,
}

#[derive(Debug)]
pub struct CreateProvider {
    pub name: String,
    pub url: String,
    pub username: String,
    pub password: String,
    pub preferred_format: Option<String>,
    pub sync_interval_secs: Option<i32>,
}

#[derive(Debug)]
pub struct UpdateProvider {
    pub name: Option<String>,
    pub url: Option<String>,
    pub username: Option<String>,
    pub password: Option<String>,
    pub preferred_format: Option<String>,
    pub sync_interval_secs: Option<i32>,
    pub enabled: Option<bool>,
    pub account_info: Option<Value>,
}

// =============================================================================
// Operations
// =============================================================================

/// Create a new IPTV provider.
pub async fn create<'e, E>(exec: E, input: CreateProvider) -> Result<ProviderRow>
where
    E: Executor<'e, Database = Postgres>,
{
    let id = Uuid::now_v7();
    let r = sqlx::query!(
        r#"
        INSERT INTO iptv_providers (id, name, url, username, password, preferred_format, sync_interval_secs)
        VALUES ($1, $2, $3, $4, $5, COALESCE($6, 'm3u8'), COALESCE($7, 3600))
        RETURNING id, name, url, username, password, preferred_format, sync_interval_secs,
                  enabled, account_info, last_sync_at, last_sync_error,
                  consecutive_sync_failures, next_sync_retry_at, created_at, updated_at
        "#,
        id,
        input.name,
        input.url,
        input.username,
        input.password,
        input.preferred_format,
        input.sync_interval_secs,
    )
    .fetch_one(exec)
    .await?;

    Ok(ProviderRow {
        id: r.id,
        name: r.name,
        url: r.url,
        username: r.username,
        password: r.password,
        preferred_format: r.preferred_format,
        sync_interval_secs: r.sync_interval_secs,
        enabled: r.enabled,
        account_info: r.account_info,
        last_sync_at: r.last_sync_at,
        last_sync_error: r.last_sync_error,
        consecutive_sync_failures: r.consecutive_sync_failures,
        next_sync_retry_at: r.next_sync_retry_at,
        created_at: r.created_at,
        updated_at: r.updated_at,
    })
}

/// Get an IPTV provider by ID.
pub async fn get_by_id<'e, E>(exec: E, id: Uuid) -> Result<Option<ProviderRow>>
where
    E: Executor<'e, Database = Postgres>,
{
    let row = sqlx::query!(
        r#"
        SELECT id, name, url, username, password, preferred_format, sync_interval_secs,
               enabled, account_info, last_sync_at, last_sync_error,
               consecutive_sync_failures, next_sync_retry_at, created_at, updated_at
        FROM iptv_providers
        WHERE id = $1
        "#,
        id,
    )
    .fetch_optional(exec)
    .await?;

    Ok(row.map(|r| ProviderRow {
        id: r.id,
        name: r.name,
        url: r.url,
        username: r.username,
        password: r.password,
        preferred_format: r.preferred_format,
        sync_interval_secs: r.sync_interval_secs,
        enabled: r.enabled,
        account_info: r.account_info,
        last_sync_at: r.last_sync_at,
        last_sync_error: r.last_sync_error,
        consecutive_sync_failures: r.consecutive_sync_failures,
        next_sync_retry_at: r.next_sync_retry_at,
        created_at: r.created_at,
        updated_at: r.updated_at,
    }))
}

/// List all IPTV providers.
pub async fn list_all<'e, E>(exec: E) -> Result<Vec<ProviderRow>>
where
    E: Executor<'e, Database = Postgres>,
{
    let rows = sqlx::query!(
        r#"
        SELECT id, name, url, username, password, preferred_format, sync_interval_secs,
               enabled, account_info, last_sync_at, last_sync_error,
               consecutive_sync_failures, next_sync_retry_at, created_at, updated_at
        FROM iptv_providers
        ORDER BY name ASC
        "#,
    )
    .fetch_all(exec)
    .await?;

    Ok(rows
        .into_iter()
        .map(|r| ProviderRow {
            id: r.id,
            name: r.name,
            url: r.url,
            username: r.username,
            password: r.password,
            preferred_format: r.preferred_format,
            sync_interval_secs: r.sync_interval_secs,
            enabled: r.enabled,
            account_info: r.account_info,
            last_sync_at: r.last_sync_at,
            last_sync_error: r.last_sync_error,
            consecutive_sync_failures: r.consecutive_sync_failures,
            next_sync_retry_at: r.next_sync_retry_at,
            created_at: r.created_at,
            updated_at: r.updated_at,
        })
        .collect())
}

/// Update an IPTV provider.
pub async fn update<'e, E>(exec: E, id: Uuid, input: UpdateProvider) -> Result<bool>
where
    E: Executor<'e, Database = Postgres>,
{
    let result = sqlx::query!(
        r#"
        UPDATE iptv_providers SET
            name = COALESCE($2, name),
            url = COALESCE($3, url),
            username = COALESCE($4, username),
            password = COALESCE($5, password),
            preferred_format = COALESCE($6, preferred_format),
            sync_interval_secs = COALESCE($7, sync_interval_secs),
            enabled = COALESCE($8, enabled),
            account_info = COALESCE($9, account_info),
            updated_at = NOW()
        WHERE id = $1
        "#,
        id,
        input.name,
        input.url,
        input.username,
        input.password,
        input.preferred_format,
        input.sync_interval_secs,
        input.enabled,
        input.account_info,
    )
    .execute(exec)
    .await?;

    Ok(result.rows_affected() > 0)
}

/// Delete an IPTV provider by ID.
pub async fn delete<'e, E>(exec: E, id: Uuid) -> Result<bool>
where
    E: Executor<'e, Database = Postgres>,
{
    let result = sqlx::query!(
        r#"
        DELETE FROM iptv_providers WHERE id = $1
        "#,
        id,
    )
    .execute(exec)
    .await?;

    Ok(result.rows_affected() > 0)
}

/// Update the sync status of a provider (success path — clears failure backoff).
pub async fn update_sync_status<'e, E>(
    exec: E,
    id: Uuid,
    last_sync_at: OffsetDateTime,
    last_sync_error: Option<&str>,
) -> Result<bool>
where
    E: Executor<'e, Database = Postgres>,
{
    let result = sqlx::query!(
        r#"
        UPDATE iptv_providers SET
            last_sync_at = $2,
            last_sync_error = $3,
            consecutive_sync_failures = 0,
            next_sync_retry_at = NULL,
            updated_at = NOW()
        WHERE id = $1
        "#,
        id,
        last_sync_at,
        last_sync_error,
    )
    .execute(exec)
    .await?;

    Ok(result.rows_affected() > 0)
}

/// Record a sync failure with exponential backoff.
///
/// Backoff schedule: 10m, 30m, 1h, 2h, 4h, capped at 4h.
pub async fn record_sync_failure<'e, E>(exec: E, id: Uuid, error: &str) -> Result<bool>
where
    E: Executor<'e, Database = Postgres>,
{
    let result = sqlx::query!(
        r#"
        UPDATE iptv_providers SET
            last_sync_error = $2,
            consecutive_sync_failures = consecutive_sync_failures + 1,
            next_sync_retry_at = NOW() + make_interval(secs =>
                LEAST(
                    600.0 * POWER(3.0, LEAST(consecutive_sync_failures, 5)::double precision),
                    14400.0
                )
            ),
            updated_at = NOW()
        WHERE id = $1
        "#,
        id,
        error,
    )
    .execute(exec)
    .await?;

    Ok(result.rows_affected() > 0)
}

/// List providers that are due for a sync.
pub async fn list_due_for_sync<'e, E>(exec: E) -> Result<Vec<ProviderRow>>
where
    E: Executor<'e, Database = Postgres>,
{
    let rows = sqlx::query!(
        r#"
        SELECT id, name, url, username, password, preferred_format, sync_interval_secs,
               enabled, account_info, last_sync_at, last_sync_error,
               consecutive_sync_failures, next_sync_retry_at, created_at, updated_at
        FROM iptv_providers
        WHERE enabled = true
          AND (next_sync_retry_at IS NULL OR next_sync_retry_at <= NOW())
          AND (last_sync_at IS NULL
               OR last_sync_at + make_interval(secs => sync_interval_secs::double precision) < NOW())
        ORDER BY last_sync_at ASC NULLS FIRST
        "#,
    )
    .fetch_all(exec)
    .await?;

    Ok(rows
        .into_iter()
        .map(|r| ProviderRow {
            id: r.id,
            name: r.name,
            url: r.url,
            username: r.username,
            password: r.password,
            preferred_format: r.preferred_format,
            sync_interval_secs: r.sync_interval_secs,
            enabled: r.enabled,
            account_info: r.account_info,
            last_sync_at: r.last_sync_at,
            last_sync_error: r.last_sync_error,
            consecutive_sync_failures: r.consecutive_sync_failures,
            next_sync_retry_at: r.next_sync_retry_at,
            created_at: r.created_at,
            updated_at: r.updated_at,
        })
        .collect())
}
