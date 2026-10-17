//! Playback token repository operations.
//!
//! Short-lived tokens for cookie-free authentication, used by Cast receivers
//! and external players that cannot send session cookies.

use super::Result;
use sqlx::{Executor, Postgres};
use time::OffsetDateTime;
use uuid::Uuid;

/// Create a playback token with a specific expiry time.
pub async fn create<'e, E>(
    exec: E,
    user_id: Uuid,
    token: &str,
    expires_at: OffsetDateTime,
) -> Result<Uuid>
where
    E: Executor<'e, Database = Postgres>,
{
    let id = Uuid::now_v7();
    sqlx::query!(
        "INSERT INTO playback_tokens (id, user_id, token, expires_at) VALUES ($1, $2, $3, $4)",
        id,
        user_id,
        token,
        expires_at,
    )
    .execute(exec)
    .await?;
    Ok(id)
}

/// Validate a playback token and return the associated (user_id, role).
/// Returns None if token is missing, expired, or invalid.
pub async fn validate<'e, E>(exec: E, token: &str) -> Result<Option<(Uuid, String, bool)>>
where
    E: Executor<'e, Database = Postgres>,
{
    let row = sqlx::query!(
        r#"SELECT pt.user_id, u.role, u.is_kids
        FROM playback_tokens pt
        JOIN users u ON u.id = pt.user_id
        WHERE pt.token = $1 AND pt.expires_at > NOW()"#,
        token,
    )
    .fetch_optional(exec)
    .await?;
    Ok(row.map(|r| (r.user_id, r.role, r.is_kids)))
}

/// Revoke a playback token by deleting it.
pub async fn revoke<'e, E>(exec: E, token: &str) -> Result<bool>
where
    E: Executor<'e, Database = Postgres>,
{
    let result = sqlx::query!("DELETE FROM playback_tokens WHERE token = $1", token)
        .execute(exec)
        .await?;
    Ok(result.rows_affected() > 0)
}

/// Delete all expired playback tokens. Returns the count of deleted rows.
pub async fn delete_expired<'e, E>(exec: E) -> Result<u64>
where
    E: Executor<'e, Database = Postgres>,
{
    let result = sqlx::query!("DELETE FROM playback_tokens WHERE expires_at < NOW()")
        .execute(exec)
        .await?;
    Ok(result.rows_affected())
}
