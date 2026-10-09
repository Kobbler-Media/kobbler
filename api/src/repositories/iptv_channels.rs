//! IPTV channels repository operations.

use super::Result;
use sqlx::{Executor, Postgres};
use time::OffsetDateTime;
use uuid::Uuid;

// =============================================================================
// Types
// =============================================================================

#[derive(Debug)]
pub struct ChannelRow {
    pub id: Uuid,
    pub name: String,
    pub normalized_name: String,
    pub external_id: Option<String>,
    pub language: Option<String>,
    pub country: Option<String>,
    pub category: Option<String>,
    pub icon_url: Option<String>,
    pub epg_channel_id: Option<String>,
    pub enabled: bool,
    pub display_order: i32,
    pub created_at: OffsetDateTime,
    pub updated_at: OffsetDateTime,
}

#[derive(Debug, sqlx::FromRow)]
pub struct ChannelWithSourceCount {
    pub id: Uuid,
    pub name: String,
    pub normalized_name: String,
    pub external_id: Option<String>,
    pub language: Option<String>,
    pub country: Option<String>,
    pub category: Option<String>,
    pub icon_url: Option<String>,
    pub epg_channel_id: Option<String>,
    pub enabled: bool,
    pub display_order: i32,
    pub created_at: OffsetDateTime,
    pub updated_at: OffsetDateTime,
    pub source_count: i64,
}

#[derive(Debug)]
pub struct CreateChannel {
    pub name: String,
    pub normalized_name: String,
    pub external_id: Option<String>,
    pub language: Option<String>,
    pub country: Option<String>,
    pub category: Option<String>,
    pub icon_url: Option<String>,
    pub epg_channel_id: Option<String>,
}

#[derive(Debug)]
pub struct UpdateChannel {
    pub name: Option<String>,
    pub normalized_name: Option<String>,
    pub external_id: Option<String>,
    pub language: Option<String>,
    pub country: Option<String>,
    pub category: Option<String>,
    pub icon_url: Option<String>,
    pub epg_channel_id: Option<String>,
    pub enabled: Option<bool>,
    pub display_order: Option<i32>,
}

// =============================================================================
// Operations
// =============================================================================

/// Create a new channel.
pub async fn create<'e, E>(exec: E, input: CreateChannel) -> Result<ChannelRow>
where
    E: Executor<'e, Database = Postgres>,
{
    let id = Uuid::now_v7();
    let r = sqlx::query!(
        r#"
        INSERT INTO iptv_channels (id, name, normalized_name, external_id, language, country,
                                   category, icon_url, epg_channel_id)
        VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9)
        RETURNING id, name, normalized_name, external_id, language, country, category,
                  icon_url, epg_channel_id, enabled, display_order, created_at, updated_at
        "#,
        id,
        input.name,
        input.normalized_name,
        input.external_id,
        input.language,
        input.country,
        input.category,
        input.icon_url,
        input.epg_channel_id,
    )
    .fetch_one(exec)
    .await?;

    Ok(ChannelRow {
        id: r.id,
        name: r.name,
        normalized_name: r.normalized_name,
        external_id: r.external_id,
        language: r.language,
        country: r.country,
        category: r.category,
        icon_url: r.icon_url,
        epg_channel_id: r.epg_channel_id,
        enabled: r.enabled,
        display_order: r.display_order,
        created_at: r.created_at,
        updated_at: r.updated_at,
    })
}

/// Get a channel by ID.
pub async fn get_by_id<'e, E>(exec: E, id: Uuid) -> Result<Option<ChannelRow>>
where
    E: Executor<'e, Database = Postgres>,
{
    let row = sqlx::query!(
        r#"
        SELECT id, name, normalized_name, external_id, language, country, category,
               icon_url, epg_channel_id, enabled, display_order, created_at, updated_at
        FROM iptv_channels
        WHERE id = $1
        "#,
        id,
    )
    .fetch_optional(exec)
    .await?;

    Ok(row.map(|r| ChannelRow {
        id: r.id,
        name: r.name,
        normalized_name: r.normalized_name,
        external_id: r.external_id,
        language: r.language,
        country: r.country,
        category: r.category,
        icon_url: r.icon_url,
        epg_channel_id: r.epg_channel_id,
        enabled: r.enabled,
        display_order: r.display_order,
        created_at: r.created_at,
        updated_at: r.updated_at,
    }))
}

/// Find a channel by normalized name, language, and country.
pub async fn find_by_normalized_name<'e, E>(
    exec: E,
    normalized_name: &str,
    language: Option<&str>,
    country: Option<&str>,
) -> Result<Option<ChannelRow>>
where
    E: Executor<'e, Database = Postgres>,
{
    let row = sqlx::query!(
        r#"
        SELECT id, name, normalized_name, external_id, language, country, category,
               icon_url, epg_channel_id, enabled, display_order, created_at, updated_at
        FROM iptv_channels
        WHERE normalized_name = $1
          AND language IS NOT DISTINCT FROM $2
          AND country IS NOT DISTINCT FROM $3
        "#,
        normalized_name,
        language,
        country,
    )
    .fetch_optional(exec)
    .await?;

    Ok(row.map(|r| ChannelRow {
        id: r.id,
        name: r.name,
        normalized_name: r.normalized_name,
        external_id: r.external_id,
        language: r.language,
        country: r.country,
        category: r.category,
        icon_url: r.icon_url,
        epg_channel_id: r.epg_channel_id,
        enabled: r.enabled,
        display_order: r.display_order,
        created_at: r.created_at,
        updated_at: r.updated_at,
    }))
}

/// List all channels with their source counts.
pub async fn list_with_source_counts<'e, E>(exec: E) -> Result<Vec<ChannelWithSourceCount>>
where
    E: Executor<'e, Database = Postgres>,
{
    let rows = sqlx::query!(
        r#"
        SELECT c.id, c.name, c.normalized_name, c.external_id, c.language, c.country,
               c.category, c.icon_url, c.epg_channel_id, c.enabled, c.display_order,
               c.created_at, c.updated_at,
               COUNT(s.id) AS "source_count!"
        FROM iptv_channels c
        LEFT JOIN iptv_sources s ON s.channel_id = c.id
        GROUP BY c.id
        ORDER BY c.display_order ASC, c.name ASC
        "#,
    )
    .fetch_all(exec)
    .await?;

    Ok(rows
        .into_iter()
        .map(|r| ChannelWithSourceCount {
            id: r.id,
            name: r.name,
            normalized_name: r.normalized_name,
            external_id: r.external_id,
            language: r.language,
            country: r.country,
            category: r.category,
            icon_url: r.icon_url,
            epg_channel_id: r.epg_channel_id,
            enabled: r.enabled,
            display_order: r.display_order,
            created_at: r.created_at,
            updated_at: r.updated_at,
            source_count: r.source_count,
        })
        .collect())
}

/// List channels by specific IDs with source counts.
pub async fn list_by_ids_with_source_counts(
    pool: &sqlx::PgPool,
    ids: &[Uuid],
) -> Result<Vec<ChannelWithSourceCount>> {
    if ids.is_empty() {
        return Ok(Vec::new());
    }

    let rows: Vec<ChannelWithSourceCount> = sqlx::query_as(
        r#"
        SELECT c.id, c.name, c.normalized_name, c.external_id, c.language, c.country,
               c.category, c.icon_url, c.epg_channel_id, c.enabled, c.display_order,
               c.created_at, c.updated_at,
               COUNT(s.id) AS source_count
        FROM iptv_channels c
        LEFT JOIN iptv_sources s ON s.channel_id = c.id
        WHERE c.id = ANY($1)
        GROUP BY c.id
        ORDER BY c.display_order ASC, c.name ASC
        "#,
    )
    .bind(ids)
    .fetch_all(pool)
    .await?;

    Ok(rows)
}

/// List channel IDs, icon URLs, and last cache attempt time for channels that have icons.
pub async fn list_icon_urls<'e, E>(exec: E) -> Result<Vec<(Uuid, String, Option<OffsetDateTime>)>>
where
    E: Executor<'e, Database = Postgres>,
{
    let rows = sqlx::query!(
        r#"
        SELECT id, icon_url AS "icon_url!", icon_cache_attempted_at
        FROM iptv_channels
        WHERE icon_url IS NOT NULL
        "#,
    )
    .fetch_all(exec)
    .await?;

    Ok(rows
        .into_iter()
        .map(|r| (r.id, r.icon_url, r.icon_cache_attempted_at))
        .collect())
}

/// Stamp `icon_cache_attempted_at` to now for the given channel.
pub async fn mark_icon_cache_attempted<'e, E>(exec: E, id: Uuid) -> Result<()>
where
    E: Executor<'e, Database = Postgres>,
{
    sqlx::query!(
        r#"UPDATE iptv_channels SET icon_cache_attempted_at = NOW() WHERE id = $1"#,
        id,
    )
    .execute(exec)
    .await?;
    Ok(())
}

/// Update a channel.
pub async fn update<'e, E>(exec: E, id: Uuid, input: UpdateChannel) -> Result<bool>
where
    E: Executor<'e, Database = Postgres>,
{
    let result = sqlx::query!(
        r#"
        UPDATE iptv_channels SET
            name = COALESCE($2, name),
            normalized_name = COALESCE($3, normalized_name),
            external_id = COALESCE($4, external_id),
            language = COALESCE($5, language),
            country = COALESCE($6, country),
            category = COALESCE($7, category),
            icon_url = COALESCE($8, icon_url),
            epg_channel_id = COALESCE($9, epg_channel_id),
            enabled = COALESCE($10, enabled),
            display_order = COALESCE($11, display_order),
            updated_at = NOW()
        WHERE id = $1
        "#,
        id,
        input.name,
        input.normalized_name,
        input.external_id,
        input.language,
        input.country,
        input.category,
        input.icon_url,
        input.epg_channel_id,
        input.enabled,
        input.display_order,
    )
    .execute(exec)
    .await?;

    Ok(result.rows_affected() > 0)
}

/// Delete a channel by ID.
pub async fn delete<'e, E>(exec: E, id: Uuid) -> Result<bool>
where
    E: Executor<'e, Database = Postgres>,
{
    let result = sqlx::query!(
        r#"
        DELETE FROM iptv_channels WHERE id = $1
        "#,
        id,
    )
    .execute(exec)
    .await?;

    Ok(result.rows_affected() > 0)
}

/// Delete channels that have no remaining sources.
/// Returns the number of deleted channels.
pub async fn delete_orphaned<'e, E>(exec: E) -> Result<u64>
where
    E: Executor<'e, Database = Postgres>,
{
    let result = sqlx::query!(
        r#"
        DELETE FROM iptv_channels
        WHERE id NOT IN (SELECT DISTINCT channel_id FROM iptv_sources)
        "#,
    )
    .execute(exec)
    .await?;

    Ok(result.rows_affected())
}

/// Set the enabled status of a channel.
pub async fn set_enabled<'e, E>(exec: E, id: Uuid, enabled: bool) -> Result<bool>
where
    E: Executor<'e, Database = Postgres>,
{
    let result = sqlx::query!(
        r#"
        UPDATE iptv_channels SET enabled = $2, updated_at = NOW() WHERE id = $1
        "#,
        id,
        enabled,
    )
    .execute(exec)
    .await?;

    Ok(result.rows_affected() > 0)
}

/// Count enabled channels.
pub async fn count_enabled<'e, E>(exec: E) -> Result<i64>
where
    E: Executor<'e, Database = Postgres>,
{
    let count = sqlx::query_scalar!(
        r#"
        SELECT COUNT(*) AS "count!" FROM iptv_channels WHERE enabled = true
        "#,
    )
    .fetch_one(exec)
    .await?;

    Ok(count)
}

/// Bulk create channels using UNNEST. Skips duplicates via ON CONFLICT DO NOTHING.
/// The caller provides pre-generated IDs. Returns the number of channels actually inserted.
pub async fn bulk_create(
    conn: &mut sqlx::PgConnection,
    channels: &[CreateChannelWithId],
) -> Result<u64> {
    if channels.is_empty() {
        return Ok(0);
    }

    let mut total = 0u64;
    for chunk in channels.chunks(2000) {
        let mut ids = Vec::with_capacity(chunk.len());
        let mut names = Vec::with_capacity(chunk.len());
        let mut normalized_names = Vec::with_capacity(chunk.len());
        let mut external_ids: Vec<Option<String>> = Vec::with_capacity(chunk.len());
        let mut languages: Vec<Option<String>> = Vec::with_capacity(chunk.len());
        let mut countries: Vec<Option<String>> = Vec::with_capacity(chunk.len());
        let mut categories_col: Vec<Option<String>> = Vec::with_capacity(chunk.len());
        let mut icon_urls: Vec<Option<String>> = Vec::with_capacity(chunk.len());

        for ch in chunk {
            ids.push(ch.id);
            names.push(ch.name.clone());
            normalized_names.push(ch.normalized_name.clone());
            external_ids.push(ch.external_id.clone());
            languages.push(ch.language.clone());
            countries.push(ch.country.clone());
            categories_col.push(ch.category.clone());
            icon_urls.push(ch.icon_url.clone());
        }

        let result = sqlx::query(
            r#"
            INSERT INTO iptv_channels (id, name, normalized_name, external_id, language, country, category, icon_url)
            SELECT u.id, u.name, u.normalized_name, u.external_id, u.language, u.country, u.category, u.icon_url
            FROM UNNEST(
                $1::uuid[], $2::text[], $3::text[], $4::text[], $5::text[], $6::text[], $7::text[], $8::text[]
            ) AS u(id, name, normalized_name, external_id, language, country, category, icon_url)
            ON CONFLICT (normalized_name, language, country) DO NOTHING
            "#,
        )
        .bind(&ids)
        .bind(&names)
        .bind(&normalized_names)
        .bind(&external_ids as &[Option<String>])
        .bind(&languages as &[Option<String>])
        .bind(&countries as &[Option<String>])
        .bind(&categories_col as &[Option<String>])
        .bind(&icon_urls as &[Option<String>])
        .execute(&mut *conn)
        .await?;

        total += result.rows_affected();
    }

    Ok(total)
}

/// Channel creation input with a pre-generated ID.
#[derive(Debug)]
pub struct CreateChannelWithId {
    pub id: Uuid,
    pub name: String,
    pub normalized_name: String,
    pub external_id: Option<String>,
    pub language: Option<String>,
    pub country: Option<String>,
    pub category: Option<String>,
    pub icon_url: Option<String>,
}

/// Merge one channel into another: move all sources from `source_id` to `target_id`,
/// then delete the source channel. Returns true if the source channel was deleted.
pub async fn merge(pool: &sqlx::PgPool, source_id: Uuid, target_id: Uuid) -> Result<bool> {
    let mut tx = pool.begin().await?;

    sqlx::query!(
        r#"
        UPDATE iptv_sources SET channel_id = $2 WHERE channel_id = $1
        "#,
        source_id,
        target_id,
    )
    .execute(&mut *tx)
    .await?;

    sqlx::query!(
        r#"
        UPDATE iptv_channel_aliases SET channel_id = $2 WHERE channel_id = $1
        "#,
        source_id,
        target_id,
    )
    .execute(&mut *tx)
    .await?;

    let result = sqlx::query!(
        r#"
        DELETE FROM iptv_channels WHERE id = $1
        "#,
        source_id,
    )
    .execute(&mut *tx)
    .await?;

    tx.commit().await?;

    Ok(result.rows_affected() > 0)
}
