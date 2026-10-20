//! Metadata cache repository operations.

use super::Result;
use serde_json::Value;
use sqlx::{Executor, Postgres};
use time::OffsetDateTime;
use uuid::Uuid;

/// Get cached results for a provider and query (returns raw JSON).
pub async fn get<'e, E>(exec: E, provider_type: &str, query_key: &str) -> Result<Option<Value>>
where
    E: Executor<'e, Database = Postgres>,
{
    let response: Option<Value> = sqlx::query_scalar!(
        r#"
        SELECT response
        FROM metadata_cache
        WHERE provider_type = $1 AND query_key = $2 AND expires_at > NOW()
        "#,
        provider_type,
        query_key,
    )
    .fetch_optional(exec)
    .await?;

    Ok(response)
}

/// Store results in cache (upserts).
pub async fn set<'e, E>(
    exec: E,
    provider_type: &str,
    query_key: &str,
    response: &Value,
    expires_at: OffsetDateTime,
) -> Result<()>
where
    E: Executor<'e, Database = Postgres>,
{
    let id = Uuid::new_v4();
    sqlx::query!(
        r#"
        INSERT INTO metadata_cache (id, provider_type, query_key, response, expires_at)
        VALUES ($1, $2, $3, $4, $5)
        ON CONFLICT (provider_type, query_key)
        DO UPDATE SET response = $4, expires_at = $5
        "#,
        id,
        provider_type,
        query_key,
        response,
        expires_at,
    )
    .execute(exec)
    .await?;

    Ok(())
}

/// Clean up expired cache entries.
pub async fn cleanup_expired<'e, E>(exec: E) -> Result<u64>
where
    E: Executor<'e, Database = Postgres>,
{
    let result = sqlx::query!("DELETE FROM metadata_cache WHERE expires_at < NOW()")
        .execute(exec)
        .await?;

    Ok(result.rows_affected())
}

/// Delete cache entries matching a provider type and key prefix.
pub async fn delete_by_prefix<'e, E>(exec: E, provider_type: &str, key_prefix: &str) -> Result<u64>
where
    E: Executor<'e, Database = Postgres>,
{
    let pattern = format!("{key_prefix}%");
    let result = sqlx::query!(
        "DELETE FROM metadata_cache WHERE provider_type = $1 AND query_key LIKE $2",
        provider_type,
        pattern,
    )
    .execute(exec)
    .await?;
    Ok(result.rows_affected())
}

pub async fn get_many<'e, E>(
    exec: E,
    provider_type: &str,
    query_keys: &[String],
) -> Result<Vec<(String, Value)>>
where
    E: Executor<'e, Database = Postgres>,
{
    let rows = sqlx::query!(
        r#"
        SELECT query_key, response
        FROM metadata_cache
        WHERE provider_type = $1 AND query_key = ANY($2) AND expires_at > NOW()
        "#,
        provider_type,
        query_keys,
    )
    .fetch_all(exec)
    .await?;

    Ok(rows
        .into_iter()
        .map(|r| (r.query_key, r.response))
        .collect())
}

/// Load all non-expired cache entries for a given provider type.
pub async fn list_by_provider<'e, E>(exec: E, provider_type: &str) -> Result<Vec<(String, Value)>>
where
    E: Executor<'e, Database = Postgres>,
{
    let rows: Vec<(String, Value)> = sqlx::query_as(
        "SELECT query_key, response FROM metadata_cache WHERE provider_type = $1 AND expires_at > NOW() ORDER BY query_key",
    )
    .bind(provider_type)
    .fetch_all(exec)
    .await?;

    Ok(rows)
}

/// Delete all metadata cache entries.
pub async fn delete_all<'e, E>(exec: E) -> Result<u64>
where
    E: Executor<'e, Database = Postgres>,
{
    let result = sqlx::query!("DELETE FROM metadata_cache")
        .execute(exec)
        .await?;
    Ok(result.rows_affected())
}
