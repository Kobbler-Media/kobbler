//! IPTV EPG (Electronic Program Guide) repository operations.

use super::Result;
use serde_json::Value;
use sqlx::{Executor, Postgres};
use time::OffsetDateTime;
use uuid::Uuid;

// =============================================================================
// Types
// =============================================================================

#[derive(Debug, sqlx::FromRow)]
pub struct EpgRow {
    pub id: Uuid,
    pub channel_id: Uuid,
    pub title: Option<String>,
    pub description: Option<String>,
    pub start_time: OffsetDateTime,
    pub stop_time: OffsetDateTime,
    pub categories: Vec<String>,
    pub episode_info: Option<Value>,
    pub created_at: OffsetDateTime,
}

#[derive(Debug)]
pub struct EpgEntry {
    pub channel_id: Uuid,
    pub title: Option<String>,
    pub description: Option<String>,
    pub start_time: OffsetDateTime,
    pub stop_time: OffsetDateTime,
    pub categories: Vec<String>,
    pub episode_info: Option<Value>,
}

// =============================================================================
// Operations
// =============================================================================

/// Bulk upsert EPG entries (batch insert with ON CONFLICT).
/// Returns the number of rows inserted/updated.
pub async fn bulk_upsert<'e, E>(exec: E, entries: &[EpgEntry]) -> Result<u64>
where
    E: Executor<'e, Database = Postgres>,
{
    if entries.is_empty() {
        return Ok(0);
    }

    let mut ids = Vec::with_capacity(entries.len());
    let mut channel_ids = Vec::with_capacity(entries.len());
    let mut titles: Vec<Option<String>> = Vec::with_capacity(entries.len());
    let mut descriptions: Vec<Option<String>> = Vec::with_capacity(entries.len());
    let mut start_times = Vec::with_capacity(entries.len());
    let mut stop_times = Vec::with_capacity(entries.len());
    let mut categories_json: Vec<Value> = Vec::with_capacity(entries.len());
    let mut episode_infos: Vec<Option<Value>> = Vec::with_capacity(entries.len());

    for entry in entries {
        ids.push(Uuid::now_v7());
        channel_ids.push(entry.channel_id);
        titles.push(entry.title.clone());
        descriptions.push(entry.description.clone());
        start_times.push(entry.start_time);
        stop_times.push(entry.stop_time);
        categories_json.push(serde_json::to_value(&entry.categories).unwrap_or_default());
        episode_infos.push(entry.episode_info.clone());
    }

    let result = sqlx::query(
        r#"
        INSERT INTO iptv_epg (id, channel_id, title, description, start_time, stop_time,
                              categories, episode_info)
        SELECT
            u.id, u.channel_id, u.title, u.description, u.start_time, u.stop_time,
            COALESCE(
                (SELECT array_agg(e.value::text) FROM jsonb_array_elements_text(u.cats) e),
                '{}'::text[]
            ),
            u.episode_info
        FROM UNNEST(
            $1::uuid[], $2::uuid[], $3::text[], $4::text[], $5::timestamptz[],
            $6::timestamptz[], $7::jsonb[], $8::jsonb[]
        ) AS u(id, channel_id, title, description, start_time, stop_time, cats, episode_info)
        ON CONFLICT (channel_id, start_time, stop_time)
        DO UPDATE SET
            title = EXCLUDED.title,
            description = EXCLUDED.description,
            categories = EXCLUDED.categories,
            episode_info = EXCLUDED.episode_info
        "#,
    )
    .bind(&ids)
    .bind(&channel_ids)
    .bind(&titles as &[Option<String>])
    .bind(&descriptions as &[Option<String>])
    .bind(&start_times)
    .bind(&stop_times)
    .bind(&categories_json)
    .bind(&episode_infos as &[Option<Value>])
    .execute(exec)
    .await?;

    Ok(result.rows_affected())
}

/// Get the current program for a channel at a given time.
pub async fn get_current_program<'e, E>(
    exec: E,
    channel_id: Uuid,
    now: OffsetDateTime,
) -> Result<Option<EpgRow>>
where
    E: Executor<'e, Database = Postgres>,
{
    let row = sqlx::query!(
        r#"
        SELECT id, channel_id, title, description, start_time, stop_time,
               categories, episode_info, created_at
        FROM iptv_epg
        WHERE channel_id = $1 AND start_time <= $2 AND stop_time > $2
        ORDER BY start_time DESC
        LIMIT 1
        "#,
        channel_id,
        now,
    )
    .fetch_optional(exec)
    .await?;

    Ok(row.map(|r| EpgRow {
        id: r.id,
        channel_id: r.channel_id,
        title: r.title,
        description: r.description,
        start_time: r.start_time,
        stop_time: r.stop_time,
        categories: r.categories,
        episode_info: r.episode_info,
        created_at: r.created_at,
    }))
}

/// List EPG entries for a channel within a time range.
pub async fn list_by_channel_time_range<'e, E>(
    exec: E,
    channel_id: Uuid,
    from: OffsetDateTime,
    to: OffsetDateTime,
) -> Result<Vec<EpgRow>>
where
    E: Executor<'e, Database = Postgres>,
{
    let rows = sqlx::query!(
        r#"
        SELECT id, channel_id, title, description, start_time, stop_time,
               categories, episode_info, created_at
        FROM iptv_epg
        WHERE channel_id = $1 AND start_time < $3 AND stop_time > $2
        ORDER BY start_time ASC
        "#,
        channel_id,
        from,
        to,
    )
    .fetch_all(exec)
    .await?;

    Ok(rows
        .into_iter()
        .map(|r| EpgRow {
            id: r.id,
            channel_id: r.channel_id,
            title: r.title,
            description: r.description,
            start_time: r.start_time,
            stop_time: r.stop_time,
            categories: r.categories,
            episode_info: r.episode_info,
            created_at: r.created_at,
        })
        .collect())
}

/// Batch query: list EPG entries for multiple channels within a time range.
/// Single query replaces N+1 per-channel queries.
pub async fn list_by_channel_ids_time_range(
    pool: &sqlx::PgPool,
    channel_ids: &[Uuid],
    from: OffsetDateTime,
    to: OffsetDateTime,
) -> Result<Vec<EpgRow>> {
    if channel_ids.is_empty() {
        return Ok(Vec::new());
    }

    let rows: Vec<EpgRow> = sqlx::query_as(
        r#"
        SELECT id, channel_id, title, description, start_time, stop_time,
               categories, episode_info, created_at
        FROM iptv_epg
        WHERE channel_id = ANY($1) AND start_time < $3 AND stop_time > $2
        ORDER BY channel_id, start_time ASC
        "#,
    )
    .bind(channel_ids)
    .bind(from)
    .bind(to)
    .fetch_all(pool)
    .await?;

    Ok(rows)
}

/// Prune expired EPG entries (stop_time before the given timestamp).
/// Returns the number of rows deleted.
pub async fn prune_expired<'e, E>(exec: E, before: OffsetDateTime) -> Result<u64>
where
    E: Executor<'e, Database = Postgres>,
{
    let result = sqlx::query!(
        r#"
        DELETE FROM iptv_epg WHERE stop_time < $1
        "#,
        before,
    )
    .execute(exec)
    .await?;

    Ok(result.rows_affected())
}

/// Read EPG retention days from config (defaults to 7).
pub async fn get_retention_days(pool: &sqlx::PgPool) -> i64 {
    sqlx::query_scalar::<_, i32>("SELECT epg_retention_days FROM iptv_config LIMIT 1")
        .fetch_optional(pool)
        .await
        .ok()
        .flatten()
        .unwrap_or(7) as i64
}

/// Tiered EPG pruning: keep longer retention for favourite channels.
/// Favourite channels keep `fav_days` of history, others keep `default_days`.
pub async fn prune_tiered(pool: &sqlx::PgPool, fav_days: i64, default_days: i64) -> Result<u64> {
    let now = time::OffsetDateTime::now_utc();
    let fav_cutoff = now - time::Duration::days(fav_days);
    let default_cutoff = now - time::Duration::days(default_days);

    let result = sqlx::query(
        r#"
        DELETE FROM iptv_epg
        WHERE (
            channel_id IN (SELECT channel_id FROM iptv_favourites)
            AND stop_time < $1
        ) OR (
            channel_id NOT IN (SELECT channel_id FROM iptv_favourites)
            AND stop_time < $2
        )
        "#,
    )
    .bind(fav_cutoff)
    .bind(default_cutoff)
    .execute(pool)
    .await?;

    Ok(result.rows_affected())
}

/// Count EPG entries for a channel.
pub async fn count_by_channel<'e, E>(exec: E, channel_id: Uuid) -> Result<i64>
where
    E: Executor<'e, Database = Postgres>,
{
    let count = sqlx::query_scalar!(
        r#"
        SELECT COUNT(*) AS "count!" FROM iptv_epg WHERE channel_id = $1
        "#,
        channel_id,
    )
    .fetch_one(exec)
    .await?;

    Ok(count)
}
