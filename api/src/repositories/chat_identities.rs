//! Chat identity bindings and pairing codes.

use super::Result;
use sqlx::{Executor, Postgres};
use time::OffsetDateTime;
use uuid::Uuid;

#[derive(Debug, Clone)]
pub struct ChatIdentity {
    pub id: Uuid,
    pub transport: String,
    pub address: Option<String>,
    pub external_id: Option<String>,
    pub user_id: Uuid,
    pub max_role: String,
    pub enabled: bool,
    pub created_at: OffsetDateTime,
    pub last_seen_at: Option<OffsetDateTime>,
}

#[derive(Debug, Clone)]
pub struct ChatIdentityWithUser {
    pub identity: ChatIdentity,
    pub user_email: String,
    pub user_display_name: Option<String>,
    pub user_role: String,
}

#[derive(Debug, Clone)]
pub struct NewChatIdentity<'a> {
    pub transport: &'a str,
    pub address: Option<&'a str>,
    pub external_id: Option<&'a str>,
    pub user_id: Uuid,
    pub max_role: &'a str,
    pub enabled: bool,
}

#[derive(Debug, Clone, Default)]
pub struct ChatIdentityPatch<'a> {
    pub address: Option<Option<&'a str>>,
    pub external_id: Option<Option<&'a str>>,
    pub user_id: Option<Uuid>,
    pub max_role: Option<&'a str>,
    pub enabled: Option<bool>,
}

pub async fn create<'e, E>(exec: E, input: &NewChatIdentity<'_>) -> Result<ChatIdentity>
where
    E: Executor<'e, Database = Postgres>,
{
    let id = Uuid::now_v7();
    let row = sqlx::query!(
        r#"
        INSERT INTO chat_identities (id, transport, address, external_id, user_id, max_role, enabled)
        VALUES ($1, $2, $3, $4, $5, $6, $7)
        RETURNING id, transport, address, external_id, user_id, max_role, enabled, created_at, last_seen_at
        "#,
        id,
        input.transport,
        input.address,
        input.external_id,
        input.user_id,
        input.max_role,
        input.enabled,
    )
    .fetch_one(exec)
    .await?;

    Ok(ChatIdentity {
        id: row.id,
        transport: row.transport,
        address: row.address,
        external_id: row.external_id,
        user_id: row.user_id,
        max_role: row.max_role,
        enabled: row.enabled,
        created_at: row.created_at,
        last_seen_at: row.last_seen_at,
    })
}

pub async fn get<'e, E>(exec: E, id: Uuid) -> Result<Option<ChatIdentity>>
where
    E: Executor<'e, Database = Postgres>,
{
    let row = sqlx::query!(
        r#"
        SELECT id, transport, address, external_id, user_id, max_role, enabled, created_at, last_seen_at
        FROM chat_identities
        WHERE id = $1
        "#,
        id,
    )
    .fetch_optional(exec)
    .await?;

    Ok(row.map(|r| ChatIdentity {
        id: r.id,
        transport: r.transport,
        address: r.address,
        external_id: r.external_id,
        user_id: r.user_id,
        max_role: r.max_role,
        enabled: r.enabled,
        created_at: r.created_at,
        last_seen_at: r.last_seen_at,
    }))
}

pub async fn list<'e, E>(exec: E) -> Result<Vec<ChatIdentityWithUser>>
where
    E: Executor<'e, Database = Postgres>,
{
    let rows = sqlx::query!(
        r#"
        SELECT ci.id, ci.transport, ci.address, ci.external_id, ci.user_id, ci.max_role,
               ci.enabled, ci.created_at, ci.last_seen_at,
               u.email AS user_email, u.display_name AS user_display_name, u.role AS user_role
        FROM chat_identities ci
        JOIN users u ON u.id = ci.user_id
        ORDER BY ci.created_at DESC, ci.id DESC
        "#,
    )
    .fetch_all(exec)
    .await?;

    Ok(rows
        .into_iter()
        .map(|r| ChatIdentityWithUser {
            identity: ChatIdentity {
                id: r.id,
                transport: r.transport,
                address: r.address,
                external_id: r.external_id,
                user_id: r.user_id,
                max_role: r.max_role,
                enabled: r.enabled,
                created_at: r.created_at,
                last_seen_at: r.last_seen_at,
            },
            user_email: r.user_email,
            user_display_name: r.user_display_name,
            user_role: r.user_role,
        })
        .collect())
}

pub async fn update<'e, E>(
    exec: E,
    id: Uuid,
    patch: &ChatIdentityPatch<'_>,
) -> Result<Option<ChatIdentity>>
where
    E: Executor<'e, Database = Postgres>,
{
    let row = sqlx::query!(
        r#"
        UPDATE chat_identities SET
            address     = CASE WHEN $2 THEN $3 ELSE address END,
            external_id = CASE WHEN $4 THEN $5 ELSE external_id END,
            user_id     = COALESCE($6, user_id),
            max_role    = COALESCE($7, max_role),
            enabled     = COALESCE($8, enabled)
        WHERE id = $1
        RETURNING id, transport, address, external_id, user_id, max_role, enabled, created_at, last_seen_at
        "#,
        id,
        patch.address.is_some(),
        patch.address.flatten(),
        patch.external_id.is_some(),
        patch.external_id.flatten(),
        patch.user_id,
        patch.max_role,
        patch.enabled,
    )
    .fetch_optional(exec)
    .await?;

    Ok(row.map(|r| ChatIdentity {
        id: r.id,
        transport: r.transport,
        address: r.address,
        external_id: r.external_id,
        user_id: r.user_id,
        max_role: r.max_role,
        enabled: r.enabled,
        created_at: r.created_at,
        last_seen_at: r.last_seen_at,
    }))
}

pub async fn delete<'e, E>(exec: E, id: Uuid) -> Result<bool>
where
    E: Executor<'e, Database = Postgres>,
{
    let result = sqlx::query!("DELETE FROM chat_identities WHERE id = $1", id)
        .execute(exec)
        .await?;
    Ok(result.rows_affected() > 0)
}

/// A chat identity joined to its owner's *current* role.
///
/// `user_role` is read at resolve time and never cached — demoting someone in the
/// Users page must lower their chat privileges on the very next message.
#[derive(Debug, Clone)]
pub struct ResolvedIdentity {
    pub id: Uuid,
    pub user_id: Uuid,
    pub user_role: String,
    pub user_is_kids: bool,
    pub max_role: String,
    pub enabled: bool,
    pub external_id: Option<String>,
}

/// Look up an identity by transport resource name or by address (case-insensitively).
///
/// `external_id` wins over `address` when both match different rows, because it is the
/// key the transport itself vouches for.
///
/// The third clause covers a transport with no human-readable address at all: Discord
/// hands over a numeric user id and never an email, so an admin binding someone ahead of
/// time has nothing to type into `address` *except* that id. Without it the binding
/// looks correct in the UI and silently never matches. The `mark_seen` that follows a
/// hit promotes the id into `external_id`, so this path is only ever taken once.
pub async fn resolve<'e, E>(
    exec: E,
    transport: &str,
    external_id: Option<&str>,
    address: Option<&str>,
) -> Result<Option<ResolvedIdentity>>
where
    E: Executor<'e, Database = Postgres>,
{
    let row = sqlx::query!(
        r#"
        SELECT ci.id, ci.user_id, u.role AS user_role, u.is_kids AS user_is_kids, ci.max_role,
               ci.enabled, ci.external_id
        FROM chat_identities ci
        JOIN users u ON u.id = ci.user_id
        WHERE ci.transport = $1
          AND (
                ($2::text IS NOT NULL AND ci.external_id = $2)
             OR ($3::text IS NOT NULL AND lower(ci.address) = lower($3))
             OR ($2::text IS NOT NULL AND lower(ci.address) = lower($2))
          )
        ORDER BY ($2::text IS NOT NULL AND ci.external_id = $2) DESC, ci.created_at ASC, ci.id ASC
        LIMIT 1
        "#,
        transport,
        external_id,
        address,
    )
    .fetch_optional(exec)
    .await?;

    Ok(row.map(|r| ResolvedIdentity {
        id: r.id,
        user_id: r.user_id,
        user_role: r.user_role,
        user_is_kids: r.user_is_kids,
        max_role: r.max_role,
        enabled: r.enabled,
        external_id: r.external_id,
    }))
}

/// Stamp `last_seen_at` and learn the transport's stable id if we did not have it.
pub async fn mark_seen<'e, E>(exec: E, id: Uuid, external_id: Option<&str>) -> Result<()>
where
    E: Executor<'e, Database = Postgres>,
{
    sqlx::query!(
        r#"
        UPDATE chat_identities
        SET last_seen_at = NOW(),
            external_id = COALESCE(external_id, $2)
        WHERE id = $1
        "#,
        id,
        external_id,
    )
    .execute(exec)
    .await?;
    Ok(())
}

// ============================================================================
// Pairing codes
// ============================================================================

#[derive(Debug, Clone)]
pub struct PairingCode {
    pub id: Uuid,
    pub transport: String,
    pub external_id: String,
    pub code: String,
    pub display_name: Option<String>,
    pub space: Option<String>,
    pub thread: Option<String>,
    pub created_at: OffsetDateTime,
    pub expires_at: OffsetDateTime,
    pub consumed_at: Option<OffsetDateTime>,
    pub identity_id: Option<Uuid>,
}

fn map_pairing(r: PairingRow) -> PairingCode {
    PairingCode {
        id: r.id,
        transport: r.transport,
        external_id: r.external_id,
        code: r.code,
        display_name: r.display_name,
        space: r.space,
        thread: r.thread,
        created_at: r.created_at,
        expires_at: r.expires_at,
        consumed_at: r.consumed_at,
        identity_id: r.identity_id,
    }
}

struct PairingRow {
    id: Uuid,
    transport: String,
    external_id: String,
    code: String,
    display_name: Option<String>,
    space: Option<String>,
    thread: Option<String>,
    created_at: OffsetDateTime,
    expires_at: OffsetDateTime,
    consumed_at: Option<OffsetDateTime>,
    identity_id: Option<Uuid>,
}

#[derive(Debug, Clone)]
pub struct NewPairingCode<'a> {
    pub transport: &'a str,
    pub external_id: &'a str,
    pub code: &'a str,
    pub display_name: Option<&'a str>,
    pub space: Option<&'a str>,
    pub thread: Option<&'a str>,
    pub ttl_seconds: i64,
}

pub async fn insert_pairing_code<'e, E>(exec: E, input: &NewPairingCode<'_>) -> Result<PairingCode>
where
    E: Executor<'e, Database = Postgres>,
{
    let id = Uuid::now_v7();
    let row = sqlx::query_as!(
        PairingRow,
        r#"
        INSERT INTO chat_pairing_codes
            (id, transport, external_id, code, display_name, space, thread, expires_at)
        VALUES ($1, $2, $3, $4, $5, $6, $7, NOW() + make_interval(secs => $8::double precision))
        RETURNING id, transport, external_id, code, display_name, space, thread,
                  created_at, expires_at, consumed_at, identity_id
        "#,
        id,
        input.transport,
        input.external_id,
        input.code,
        input.display_name,
        input.space,
        input.thread,
        input.ttl_seconds as f64,
    )
    .fetch_one(exec)
    .await?;
    Ok(map_pairing(row))
}

/// The sender's outstanding, unexpired code, if any.
pub async fn active_pairing_code<'e, E>(
    exec: E,
    transport: &str,
    external_id: &str,
) -> Result<Option<PairingCode>>
where
    E: Executor<'e, Database = Postgres>,
{
    let row = sqlx::query_as!(
        PairingRow,
        r#"
        SELECT id, transport, external_id, code, display_name, space, thread,
               created_at, expires_at, consumed_at, identity_id
        FROM chat_pairing_codes
        WHERE transport = $1 AND external_id = $2
          AND consumed_at IS NULL AND expires_at > NOW()
        ORDER BY created_at DESC, id DESC
        LIMIT 1
        "#,
        transport,
        external_id,
    )
    .fetch_optional(exec)
    .await?;
    Ok(row.map(map_pairing))
}

/// How many codes this sender has been issued inside the window — the rate-limit input.
pub async fn count_recent_pairing_codes<'e, E>(
    exec: E,
    transport: &str,
    external_id: &str,
    window_seconds: i64,
) -> Result<i64>
where
    E: Executor<'e, Database = Postgres>,
{
    let count = sqlx::query_scalar!(
        r#"
        SELECT COUNT(*) AS "count!"
        FROM chat_pairing_codes
        WHERE transport = $1 AND external_id = $2
          AND created_at > NOW() - make_interval(secs => $3::double precision)
        "#,
        transport,
        external_id,
        window_seconds as f64,
    )
    .fetch_one(exec)
    .await?;
    Ok(count)
}

/// Pending approvals for the admin screen: unconsumed and still valid.
pub async fn list_pending_pairing_codes<'e, E>(exec: E) -> Result<Vec<PairingCode>>
where
    E: Executor<'e, Database = Postgres>,
{
    let rows = sqlx::query_as!(
        PairingRow,
        r#"
        SELECT id, transport, external_id, code, display_name, space, thread,
               created_at, expires_at, consumed_at, identity_id
        FROM chat_pairing_codes
        WHERE consumed_at IS NULL AND expires_at > NOW()
        ORDER BY created_at DESC, id DESC
        "#,
    )
    .fetch_all(exec)
    .await?;
    Ok(rows.into_iter().map(map_pairing).collect())
}

/// Claim a code atomically. Returns `None` if it is unknown, expired or already used.
pub async fn claim_pairing_code<'e, E>(
    exec: E,
    transport: &str,
    code: &str,
    identity_id: Uuid,
) -> Result<Option<PairingCode>>
where
    E: Executor<'e, Database = Postgres>,
{
    let row = sqlx::query_as!(
        PairingRow,
        r#"
        UPDATE chat_pairing_codes
        SET consumed_at = NOW(), identity_id = $3
        WHERE transport = $1 AND code = $2 AND consumed_at IS NULL AND expires_at > NOW()
        RETURNING id, transport, external_id, code, display_name, space, thread,
                  created_at, expires_at, consumed_at, identity_id
        "#,
        transport,
        code,
        identity_id,
    )
    .fetch_optional(exec)
    .await?;
    Ok(row.map(map_pairing))
}

pub async fn get_pairing_code<'e, E>(exec: E, id: Uuid) -> Result<Option<PairingCode>>
where
    E: Executor<'e, Database = Postgres>,
{
    let row = sqlx::query_as!(
        PairingRow,
        r#"
        SELECT id, transport, external_id, code, display_name, space, thread,
               created_at, expires_at, consumed_at, identity_id
        FROM chat_pairing_codes
        WHERE id = $1
        "#,
        id,
    )
    .fetch_optional(exec)
    .await?;
    Ok(row.map(map_pairing))
}

pub async fn delete_pairing_code<'e, E>(exec: E, id: Uuid) -> Result<bool>
where
    E: Executor<'e, Database = Postgres>,
{
    let result = sqlx::query!("DELETE FROM chat_pairing_codes WHERE id = $1", id)
        .execute(exec)
        .await?;
    Ok(result.rows_affected() > 0)
}

/// Drop codes that can never be approved again, so the unique index on `code` stays free.
pub async fn prune_pairing_codes<'e, E>(exec: E, older_than_seconds: i64) -> Result<u64>
where
    E: Executor<'e, Database = Postgres>,
{
    let result = sqlx::query!(
        r#"
        DELETE FROM chat_pairing_codes
        WHERE created_at < NOW() - make_interval(secs => $1::double precision)
          AND (consumed_at IS NOT NULL OR expires_at <= NOW())
        "#,
        older_than_seconds as f64,
    )
    .execute(exec)
    .await?;
    Ok(result.rows_affected())
}
