//! IPTV watch history repository operations.

use super::Result;
use sqlx::{Executor, Postgres};
use time::OffsetDateTime;
use uuid::Uuid;

// =============================================================================
// Types
// =============================================================================

#[derive(Debug)]
pub struct WatchHistoryRow {
    pub user_id: Uuid,
    pub channel_id: Uuid,
    pub watch_count: i32,
    pub last_watched_at: OffsetDateTime,
    pub duration_secs: i32,
}

// =============================================================================
// Operations
// =============================================================================

/// Record a watch event, upserting the watch count and last_watched_at.
pub async fn record_watch<'e, E>(
    exec: E,
    user_id: Uuid,
    channel_id: Uuid,
    duration_secs: i32,
) -> Result<()>
where
    E: Executor<'e, Database = Postgres>,
{
    sqlx::query!(
        r#"
        INSERT INTO iptv_watch_history (user_id, channel_id, watch_count, last_watched_at, duration_secs)
        VALUES ($1, $2, 1, NOW(), $3)
        ON CONFLICT (user_id, channel_id)
        DO UPDATE SET
            watch_count = iptv_watch_history.watch_count + 1,
            last_watched_at = NOW(),
            duration_secs = iptv_watch_history.duration_secs + EXCLUDED.duration_secs
        "#,
        user_id,
        channel_id,
        duration_secs,
    )
    .execute(exec)
    .await?;

    Ok(())
}

/// List recent watch history for a user, ordered by last_watched_at DESC.
pub async fn list_recent<'e, E>(exec: E, user_id: Uuid, limit: i64) -> Result<Vec<WatchHistoryRow>>
where
    E: Executor<'e, Database = Postgres>,
{
    let rows = sqlx::query!(
        r#"
        SELECT user_id, channel_id, watch_count, last_watched_at, duration_secs
        FROM iptv_watch_history
        WHERE user_id = $1
        ORDER BY last_watched_at DESC
        LIMIT $2
        "#,
        user_id,
        limit,
    )
    .fetch_all(exec)
    .await?;

    Ok(rows
        .into_iter()
        .map(|r| WatchHistoryRow {
            user_id: r.user_id,
            channel_id: r.channel_id,
            watch_count: r.watch_count,
            last_watched_at: r.last_watched_at,
            duration_secs: r.duration_secs,
        })
        .collect())
}
