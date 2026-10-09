//! Tier 3 profile facts.
//!
//! Every read and every write is scoped by `user_id` in SQL rather than filtered
//! afterwards: a fact is a statement about a person and must not leave Postgres
//! for anyone else. The same rule Agent D's `chat_turn_logs` split enforces for
//! transcripts.

use super::Result;
use sqlx::PgPool;
use time::OffsetDateTime;
use uuid::Uuid;

/// How many facts a person keeps. Past this it stops being a profile and starts
/// being a log, which is the failure mode the design names explicitly.
pub const MAX_FACTS_PER_USER: i64 = 20;

pub const KINDS: [&str; 3] = ["prefers", "likes", "avoids"];

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProfileFact {
    pub id: Uuid,
    pub user_id: Uuid,
    pub kind: String,
    pub value: String,
    pub source_id: Option<Uuid>,
    pub created_at: OffsetDateTime,
}

pub fn valid_kind(kind: &str) -> bool {
    KINDS.contains(&kind)
}

/// Add one fact. A re-extraction of something already known is a no-op rather
/// than a duplicate row, and returns `None`.
pub async fn insert(
    pool: &PgPool,
    user_id: Uuid,
    kind: &str,
    value: &str,
    source_id: Option<Uuid>,
) -> Result<Option<ProfileFact>> {
    let row = sqlx::query_as!(
        ProfileFact,
        r#"
        INSERT INTO chat_profile_facts (id, user_id, kind, value, source_id)
        VALUES ($1, $2, $3, $4, $5)
        ON CONFLICT DO NOTHING
        RETURNING id, user_id, kind, value, source_id, created_at
        "#,
        Uuid::now_v7(),
        user_id,
        kind,
        value,
        source_id,
    )
    .fetch_optional(pool)
    .await?;
    Ok(row)
}

/// Newest first — the order the eviction cap and the UI both read in.
pub async fn list_for_user(pool: &PgPool, user_id: Uuid) -> Result<Vec<ProfileFact>> {
    let rows = sqlx::query_as!(
        ProfileFact,
        r#"
        SELECT id, user_id, kind, value, source_id, created_at
        FROM chat_profile_facts
        WHERE user_id = $1
        ORDER BY created_at DESC, id DESC
        "#,
        user_id,
    )
    .fetch_all(pool)
    .await?;
    Ok(rows)
}

/// Oldest first, capped — what goes into every prompt.
pub async fn for_prompt(pool: &PgPool, user_id: Uuid) -> Result<Vec<ProfileFact>> {
    let rows = sqlx::query_as!(
        ProfileFact,
        r#"
        SELECT id, user_id, kind, value, source_id, created_at
        FROM chat_profile_facts
        WHERE user_id = $1
        ORDER BY created_at ASC, id ASC
        LIMIT $2
        "#,
        user_id,
        MAX_FACTS_PER_USER,
    )
    .fetch_all(pool)
    .await?;
    Ok(rows)
}

/// Correct a fact. Scoped by owner, so another user's id simply matches nothing.
pub async fn update(
    pool: &PgPool,
    id: Uuid,
    owner_id: Uuid,
    kind: &str,
    value: &str,
) -> Result<Option<ProfileFact>> {
    let row = sqlx::query_as!(
        ProfileFact,
        r#"
        UPDATE chat_profile_facts
        SET kind = $3, value = $4
        WHERE id = $1 AND user_id = $2
        RETURNING id, user_id, kind, value, source_id, created_at
        "#,
        id,
        owner_id,
        kind,
        value,
    )
    .fetch_optional(pool)
    .await?;
    Ok(row)
}

pub async fn delete(pool: &PgPool, id: Uuid, owner_id: Uuid) -> Result<bool> {
    let deleted = sqlx::query!(
        "DELETE FROM chat_profile_facts WHERE id = $1 AND user_id = $2",
        id,
        owner_id,
    )
    .execute(pool)
    .await?
    .rows_affected();
    Ok(deleted > 0)
}

/// Evict oldest beyond the cap. Returns how many went.
pub async fn trim_to_cap(pool: &PgPool, user_id: Uuid, cap: i64) -> Result<u64> {
    let deleted = sqlx::query!(
        r#"
        DELETE FROM chat_profile_facts
        WHERE id IN (
            SELECT id FROM chat_profile_facts
            WHERE user_id = $1
            ORDER BY created_at DESC, id DESC
            OFFSET $2
        )
        "#,
        user_id,
        cap,
    )
    .execute(pool)
    .await?
    .rows_affected();
    Ok(deleted)
}

pub async fn count_for_user(pool: &PgPool, user_id: Uuid) -> Result<i64> {
    let count = sqlx::query_scalar!(
        "SELECT COUNT(*) FROM chat_profile_facts WHERE user_id = $1",
        user_id,
    )
    .fetch_one(pool)
    .await?
    .unwrap_or(0);
    Ok(count)
}
