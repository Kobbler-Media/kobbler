//! IPTV channel aliases repository operations.

use super::Result;
use sqlx::{Executor, Postgres};
use uuid::Uuid;

// =============================================================================
// Types
// =============================================================================

#[derive(Debug)]
pub struct AliasRow {
    pub id: Uuid,
    pub channel_id: Uuid,
    pub normalized_alias: String,
    pub source: String,
}

// =============================================================================
// Operations
// =============================================================================

/// Create a channel alias.
pub async fn create<'e, E>(
    exec: E,
    channel_id: Uuid,
    normalized_alias: &str,
    source: &str,
) -> Result<Uuid>
where
    E: Executor<'e, Database = Postgres>,
{
    let id = Uuid::now_v7();
    sqlx::query!(
        r#"
        INSERT INTO iptv_channel_aliases (id, channel_id, normalized_alias, source)
        VALUES ($1, $2, $3, $4)
        "#,
        id,
        channel_id,
        normalized_alias,
        source,
    )
    .execute(exec)
    .await?;

    Ok(id)
}

/// Find a channel ID by a normalized alias.
pub async fn find_channel_by_alias<'e, E>(exec: E, normalized_alias: &str) -> Result<Option<Uuid>>
where
    E: Executor<'e, Database = Postgres>,
{
    let row = sqlx::query_scalar!(
        r#"
        SELECT channel_id FROM iptv_channel_aliases WHERE normalized_alias = $1
        "#,
        normalized_alias,
    )
    .fetch_optional(exec)
    .await?;

    Ok(row)
}

/// List all aliases for a channel.
pub async fn list_by_channel<'e, E>(exec: E, channel_id: Uuid) -> Result<Vec<AliasRow>>
where
    E: Executor<'e, Database = Postgres>,
{
    let rows = sqlx::query!(
        r#"
        SELECT id, channel_id, normalized_alias, source
        FROM iptv_channel_aliases
        WHERE channel_id = $1
        ORDER BY normalized_alias ASC
        "#,
        channel_id,
    )
    .fetch_all(exec)
    .await?;

    Ok(rows
        .into_iter()
        .map(|r| AliasRow {
            id: r.id,
            channel_id: r.channel_id,
            normalized_alias: r.normalized_alias,
            source: r.source,
        })
        .collect())
}

/// Delete an alias by ID.
pub async fn delete<'e, E>(exec: E, id: Uuid) -> Result<bool>
where
    E: Executor<'e, Database = Postgres>,
{
    let result = sqlx::query!(
        r#"
        DELETE FROM iptv_channel_aliases WHERE id = $1
        "#,
        id,
    )
    .execute(exec)
    .await?;

    Ok(result.rows_affected() > 0)
}

/// Bulk insert aliases, skipping conflicts.
/// Returns the number of rows inserted.
pub async fn bulk_insert<'e, E>(
    exec: E,
    channel_id: Uuid,
    aliases: &[String],
    source: &str,
) -> Result<u64>
where
    E: Executor<'e, Database = Postgres>,
{
    if aliases.is_empty() {
        return Ok(0);
    }

    let mut ids = Vec::with_capacity(aliases.len());
    let mut channel_ids = Vec::with_capacity(aliases.len());
    let mut sources = Vec::with_capacity(aliases.len());

    for _ in aliases {
        ids.push(Uuid::now_v7());
        channel_ids.push(channel_id);
        sources.push(source.to_string());
    }

    let result = sqlx::query!(
        r#"
        INSERT INTO iptv_channel_aliases (id, channel_id, normalized_alias, source)
        SELECT * FROM UNNEST($1::uuid[], $2::uuid[], $3::text[], $4::text[])
        ON CONFLICT (normalized_alias) DO NOTHING
        "#,
        &ids,
        &channel_ids,
        aliases,
        &sources,
    )
    .execute(exec)
    .await?;

    Ok(result.rows_affected())
}
