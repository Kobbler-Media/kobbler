//! Auth tokens repository operations.

use super::Result;
use sha2::{Digest, Sha256};
use sqlx::{Executor, Postgres};
use time::OffsetDateTime;
use uuid::Uuid;

/// SHA256-hash a token for storage/lookup.
fn hash_token(token: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(token.as_bytes());
    hex::encode(hasher.finalize())
}

pub struct SessionRow {
    pub id: Uuid,
    pub user_id: Uuid,
    pub email: String,
    pub display_name: Option<String>,
    pub device_name: Option<String>,
    pub client_kind: Option<String>,
    pub scope: String,
    pub created_at: OffsetDateTime,
    pub expires_at: OffsetDateTime,
    pub last_used_at: Option<OffsetDateTime>,
    pub created_by: Option<Uuid>,
    pub created_by_email: Option<String>,
    pub revoked: bool,
}

/// Create an auth token. The token is SHA256-hashed before storage.
pub async fn create<'e, E>(exec: E, user_id: Uuid, token: &str, scope: &str) -> Result<Uuid>
where
    E: Executor<'e, Database = Postgres>,
{
    let id = Uuid::now_v7();
    let token_hash = hash_token(token);
    sqlx::query!(
        "INSERT INTO auth_tokens (id, user_id, token_hash, scope, expires_at) VALUES ($1, $2, $3, $4, NOW() + INTERVAL '30 days')",
        id,
        user_id,
        token_hash,
        scope,
    )
    .execute(exec)
    .await?;
    Ok(id)
}

pub async fn create_with_expiry_days<'e, E>(
    exec: E,
    user_id: Uuid,
    token: &str,
    scope: &str,
    days: i32,
    device_name: Option<&str>,
    created_by: Option<Uuid>,
) -> Result<Uuid>
where
    E: Executor<'e, Database = Postgres>,
{
    let id = Uuid::now_v7();
    let token_hash = hash_token(token);
    sqlx::query!(
        "INSERT INTO auth_tokens (id, user_id, token_hash, scope, expires_at, device_name, created_by)
         VALUES ($1, $2, $3, $4, NOW() + make_interval(days => $5), $6, $7)",
        id,
        user_id,
        token_hash,
        scope,
        days,
        device_name,
        created_by,
    )
    .execute(exec)
    .await?;
    Ok(id)
}

/// Validate an admin token and return the associated user_id and role.
pub async fn validate_admin_token<'e, E>(exec: E, token: &str) -> Result<Option<(Uuid, String)>>
where
    E: Executor<'e, Database = Postgres>,
{
    let token_hash = hash_token(token);
    let row = sqlx::query!(
        r#"SELECT t.user_id, u.role
        FROM auth_tokens t
        JOIN users u ON u.id = t.user_id
        WHERE t.token_hash = $1 AND t.revoked = FALSE AND t.scope = 'admin' AND t.expires_at > NOW()"#,
        token_hash,
    )
    .fetch_optional(exec)
    .await?;
    Ok(row.map(|r| (r.user_id, r.role)))
}

/// None when the token is unknown, revoked, or already expired - an expired token must go
/// back through pairing rather than resurrect itself here.
pub async fn touch<'e, E>(
    exec: E,
    token: &str,
    days: i32,
    device_name: Option<&str>,
    client_kind: Option<&str>,
) -> Result<Option<OffsetDateTime>>
where
    E: Executor<'e, Database = Postgres>,
{
    let token_hash = hash_token(token);
    let row = sqlx::query!(
        r#"UPDATE auth_tokens
           SET expires_at = NOW() + make_interval(days => $2),
               last_used_at = NOW(),
               device_name = COALESCE($3, device_name),
               client_kind = COALESCE($4, client_kind)
           WHERE token_hash = $1 AND revoked = FALSE AND expires_at > NOW()
           RETURNING expires_at"#,
        token_hash,
        days,
        device_name,
        client_kind,
    )
    .fetch_optional(exec)
    .await?;
    Ok(row.map(|r| r.expires_at))
}

pub async fn id_by_token<'e, E>(exec: E, token: &str) -> Result<Option<Uuid>>
where
    E: Executor<'e, Database = Postgres>,
{
    let token_hash = hash_token(token);
    let row = sqlx::query!(
        "SELECT id FROM auth_tokens WHERE token_hash = $1",
        token_hash,
    )
    .fetch_optional(exec)
    .await?;
    Ok(row.map(|r| r.id))
}

/// `user_id` of None lists every user's sessions (admin view).
pub async fn list_sessions<'e, E>(exec: E, user_id: Option<Uuid>) -> Result<Vec<SessionRow>>
where
    E: Executor<'e, Database = Postgres>,
{
    let rows = sqlx::query!(
        r#"SELECT t.id, t.user_id, u.email, u.display_name as "display_name?",
                  t.device_name as "device_name?", t.client_kind as "client_kind?",
                  t.scope, t.created_at, t.expires_at,
                  t.last_used_at as "last_used_at?",
                  t.created_by as "created_by?", c.email as "created_by_email?",
                  t.revoked
           FROM auth_tokens t
           JOIN users u ON u.id = t.user_id
           LEFT JOIN users c ON c.id = t.created_by
           WHERE t.revoked = FALSE AND t.expires_at > NOW()
             AND ($1::uuid IS NULL OR t.user_id = $1)
           ORDER BY COALESCE(t.last_used_at, t.created_at) DESC"#,
        user_id,
    )
    .fetch_all(exec)
    .await?;
    Ok(rows
        .into_iter()
        .map(|r| SessionRow {
            id: r.id,
            user_id: r.user_id,
            email: r.email,
            display_name: r.display_name,
            device_name: r.device_name,
            client_kind: r.client_kind,
            scope: r.scope,
            created_at: r.created_at,
            expires_at: r.expires_at,
            last_used_at: r.last_used_at,
            created_by: r.created_by,
            created_by_email: r.created_by_email,
            revoked: r.revoked,
        })
        .collect())
}

pub async fn owner_of<'e, E>(exec: E, id: Uuid) -> Result<Option<Uuid>>
where
    E: Executor<'e, Database = Postgres>,
{
    let row = sqlx::query!("SELECT user_id FROM auth_tokens WHERE id = $1", id)
        .fetch_optional(exec)
        .await?;
    Ok(row.map(|r| r.user_id))
}

pub async fn revoke_by_id<'e, E>(exec: E, id: Uuid) -> Result<bool>
where
    E: Executor<'e, Database = Postgres>,
{
    let result = sqlx::query!(
        "UPDATE auth_tokens SET revoked = TRUE WHERE id = $1 AND revoked = FALSE",
        id,
    )
    .execute(exec)
    .await?;
    Ok(result.rows_affected() > 0)
}

/// Revoke an auth token by its raw (unhashed) value.
pub async fn revoke_by_token<'e, E>(exec: E, token: &str) -> Result<bool>
where
    E: Executor<'e, Database = Postgres>,
{
    let token_hash = hash_token(token);
    let result = sqlx::query!(
        "UPDATE auth_tokens SET revoked = TRUE WHERE token_hash = $1 AND revoked = FALSE",
        token_hash,
    )
    .execute(exec)
    .await?;
    Ok(result.rows_affected() > 0)
}
