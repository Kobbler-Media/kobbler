//! IPTV programme watches repository operations.

use super::Result;
use serde::Serialize;
use sqlx::{Executor, PgPool, Postgres, Row};
use time::OffsetDateTime;
use uuid::Uuid;

// =============================================================================
// Types
// =============================================================================

#[derive(Debug)]
pub struct WatchRow {
    pub id: Uuid,
    pub user_id: Uuid,
    pub title_pattern: String,
    pub match_mode: String,
    pub enabled: bool,
    pub created_at: OffsetDateTime,
}

#[derive(Debug, Serialize)]
pub struct WatchMatch {
    pub watch_id: Uuid,
    pub channel_id: Uuid,
    pub channel_name: String,
    pub channel_icon_url: Option<String>,
    pub epg_title: String,
    pub start_time: OffsetDateTime,
    pub stop_time: OffsetDateTime,
    pub is_live: bool,
}

// =============================================================================
// Operations
// =============================================================================

/// Create a new programme watch.
pub async fn create<'e, E>(
    exec: E,
    user_id: Uuid,
    title_pattern: &str,
    match_mode: &str,
) -> Result<Uuid>
where
    E: Executor<'e, Database = Postgres>,
{
    let id = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO iptv_programme_watches (id, user_id, title_pattern, match_mode)
         VALUES ($1, $2, $3, $4)",
    )
    .bind(id)
    .bind(user_id)
    .bind(title_pattern)
    .bind(match_mode)
    .execute(exec)
    .await?;

    Ok(id)
}

/// List all watches for a user.
pub async fn list_for_user<'e, E>(exec: E, user_id: Uuid) -> Result<Vec<WatchRow>>
where
    E: Executor<'e, Database = Postgres>,
{
    let rows = sqlx::query(
        "SELECT id, user_id, title_pattern, match_mode, enabled, created_at
         FROM iptv_programme_watches
         WHERE user_id = $1
         ORDER BY created_at ASC",
    )
    .bind(user_id)
    .fetch_all(exec)
    .await?;

    Ok(rows
        .into_iter()
        .map(|r| WatchRow {
            id: r.get("id"),
            user_id: r.get("user_id"),
            title_pattern: r.get("title_pattern"),
            match_mode: r.get("match_mode"),
            enabled: r.get("enabled"),
            created_at: r.get("created_at"),
        })
        .collect())
}

/// Delete a watch by ID (scoped to user).
pub async fn delete<'e, E>(exec: E, id: Uuid, user_id: Uuid) -> Result<bool>
where
    E: Executor<'e, Database = Postgres>,
{
    let result = sqlx::query("DELETE FROM iptv_programme_watches WHERE id = $1 AND user_id = $2")
        .bind(id)
        .bind(user_id)
        .execute(exec)
        .await?;

    Ok(result.rows_affected() > 0)
}

/// List all enabled watches (across all users).
pub async fn list_enabled<'e, E>(exec: E) -> Result<Vec<WatchRow>>
where
    E: Executor<'e, Database = Postgres>,
{
    let rows = sqlx::query(
        "SELECT id, user_id, title_pattern, match_mode, enabled, created_at
         FROM iptv_programme_watches
         WHERE enabled = true
         ORDER BY created_at ASC",
    )
    .fetch_all(exec)
    .await?;

    Ok(rows
        .into_iter()
        .map(|r| WatchRow {
            id: r.get("id"),
            user_id: r.get("user_id"),
            title_pattern: r.get("title_pattern"),
            match_mode: r.get("match_mode"),
            enabled: r.get("enabled"),
            created_at: r.get("created_at"),
        })
        .collect())
}

/// Find EPG programmes matching user watches in the next hour.
///
/// Uses a runtime query since match_mode determines the WHERE condition dynamically.
pub async fn find_matches(pool: &PgPool, user_id: Uuid) -> Result<Vec<WatchMatch>> {
    let rows = sqlx::query(
        r#"
        SELECT
            w.id AS watch_id,
            c.id AS channel_id,
            c.name AS channel_name,
            c.icon_url AS channel_icon_url,
            e.title AS epg_title,
            e.start_time,
            e.stop_time,
            (e.start_time <= NOW() AND e.stop_time > NOW()) AS is_live
        FROM iptv_programme_watches w
        JOIN iptv_epg e ON (
            (w.match_mode = 'contains' AND e.title ILIKE '%' || w.title_pattern || '%')
            OR (w.match_mode = 'exact' AND e.title = w.title_pattern)
            OR (w.match_mode = 'regex' AND e.title ~* w.title_pattern)
        )
        JOIN iptv_channels c ON c.id = e.channel_id
        WHERE w.user_id = $1
          AND w.enabled = true
          AND e.start_time <= NOW() + INTERVAL '1 hour'
          AND e.stop_time > NOW()
        ORDER BY e.start_time ASC
        "#,
    )
    .bind(user_id)
    .fetch_all(pool)
    .await?;

    Ok(rows
        .into_iter()
        .map(|r| WatchMatch {
            watch_id: r.get("watch_id"),
            channel_id: r.get("channel_id"),
            channel_name: r.get("channel_name"),
            channel_icon_url: r.get("channel_icon_url"),
            epg_title: r.get("epg_title"),
            start_time: r.get("start_time"),
            stop_time: r.get("stop_time"),
            is_live: r.get::<Option<bool>, _>("is_live").unwrap_or(false),
        })
        .collect())
}
