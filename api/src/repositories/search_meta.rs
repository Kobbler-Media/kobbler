//! Repository for search index metadata.

use sqlx::{Executor, Postgres};

use super::Result;

/// Get a metadata value by key.
pub async fn get<'e, E>(exec: E, key: &str) -> Result<Option<String>>
where
    E: Executor<'e, Database = Postgres>,
{
    let value: Option<String> =
        sqlx::query_scalar!(r#"SELECT value FROM search_index_meta WHERE key = $1"#, key)
            .fetch_optional(exec)
            .await?;

    Ok(value)
}

/// Set a metadata value.
pub async fn set<'e, E>(exec: E, key: &str, value: &str) -> Result<()>
where
    E: Executor<'e, Database = Postgres>,
{
    sqlx::query!(
        r#"
        INSERT INTO search_index_meta (key, value, updated_at)
        VALUES ($1, $2, NOW())
        ON CONFLICT (key) DO UPDATE SET value = $2, updated_at = NOW()
        "#,
        key,
        value,
    )
    .execute(exec)
    .await?;

    Ok(())
}

/// Get the stored schema version.
pub async fn get_schema_version<'e, E>(exec: E) -> Result<i32>
where
    E: Executor<'e, Database = Postgres>,
{
    let value = get(exec, "schema_version").await?;
    Ok(value.and_then(|v| v.parse().ok()).unwrap_or(0))
}

/// Set the schema version.
pub async fn set_schema_version<'e, E>(exec: E, version: i32) -> Result<()>
where
    E: Executor<'e, Database = Postgres>,
{
    set(exec, "schema_version", &version.to_string()).await
}

/// Record that a full reindex was completed.
pub async fn record_reindex<'e, E>(exec: E) -> Result<()>
where
    E: Executor<'e, Database = Postgres>,
{
    let timestamp = chrono::Utc::now().to_rfc3339();
    set(exec, "last_reindex", &timestamp).await
}
