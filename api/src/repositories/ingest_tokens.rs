//! Ingest tokens repository operations.

use super::Result;
use sqlx::{Executor, Postgres};
use uuid::Uuid;

/// Create a new ingest token.
pub async fn create<'e, E>(exec: E, token: &str, name: &str) -> Result<Uuid>
where
    E: Executor<'e, Database = Postgres>,
{
    let id = Uuid::now_v7();
    sqlx::query!(
        "INSERT INTO ingest_tokens (id, token, name) VALUES ($1, $2, $3)",
        id,
        token,
        name,
    )
    .execute(exec)
    .await?;
    Ok(id)
}

/// Revoke an ingest token.
pub async fn revoke<'e, E>(exec: E, id: Uuid) -> Result<bool>
where
    E: Executor<'e, Database = Postgres>,
{
    let result = sqlx::query!("UPDATE ingest_tokens SET revoked = TRUE WHERE id = $1", id,)
        .execute(exec)
        .await?;
    Ok(result.rows_affected() > 0)
}

/// Update the name of an ingest token.
pub async fn update_name<'e, E>(exec: E, id: Uuid, name: &str) -> Result<bool>
where
    E: Executor<'e, Database = Postgres>,
{
    let result = sqlx::query!("UPDATE ingest_tokens SET name = $2 WHERE id = $1", id, name,)
        .execute(exec)
        .await?;
    Ok(result.rows_affected() > 0)
}

/// Check if a token is valid (exists and not revoked).
pub async fn is_valid<'e, E>(exec: E, token: &str) -> Result<bool>
where
    E: Executor<'e, Database = Postgres>,
{
    let exists: bool = sqlx::query_scalar!(
        r#"SELECT EXISTS(SELECT 1 FROM ingest_tokens WHERE token = $1 AND revoked = FALSE) as "exists!""#,
        token,
    )
    .fetch_one(exec)
    .await?;
    Ok(exists)
}

/// Check if a token exists (regardless of revoked status).
pub async fn token_exists<'e, E>(exec: E, token: &str) -> Result<bool>
where
    E: Executor<'e, Database = Postgres>,
{
    let exists: bool = sqlx::query_scalar!(
        r#"SELECT EXISTS(SELECT 1 FROM ingest_tokens WHERE token = $1) as "exists!""#,
        token,
    )
    .fetch_one(exec)
    .await?;
    Ok(exists)
}

/// Ingest token summary for listing.
pub struct IngestTokenSummary {
    pub id: Uuid,
    pub token: String,
    pub name: Option<String>,
    pub created_at: time::OffsetDateTime,
    pub revoked: bool,
}

/// List all ingest tokens.
pub async fn list_all<'e, E>(exec: E) -> Result<Vec<IngestTokenSummary>>
where
    E: Executor<'e, Database = Postgres>,
{
    let rows = sqlx::query!(
        "SELECT id, token, name, created_at, revoked FROM ingest_tokens ORDER BY created_at DESC",
    )
    .fetch_all(exec)
    .await?;

    Ok(rows
        .into_iter()
        .map(|row| IngestTokenSummary {
            id: row.id,
            token: row.token,
            name: row.name,
            created_at: row.created_at,
            revoked: row.revoked,
        })
        .collect())
}
