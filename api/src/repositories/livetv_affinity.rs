use super::Result;
use sqlx::{Executor, Postgres};
use time::OffsetDateTime;
use uuid::Uuid;

#[derive(Debug, Clone)]
pub struct AffinityRow {
    pub id: Uuid,
    pub user_id: Uuid,
    pub series_id: Option<String>,
    pub channel_id: Option<Uuid>,
    pub genre: Option<String>,
    pub weekday: Option<i16>,
    pub hour: Option<i16>,
    pub watch_count: i32,
    pub total_secs: i64,
    pub mean_completion: f64,
    pub last_watched_at: Option<OffsetDateTime>,
    pub updated_at: OffsetDateTime,
}

#[derive(Debug)]
pub struct UpsertAffinity {
    pub series_id: Option<String>,
    pub channel_id: Option<Uuid>,
    pub genre: Option<String>,
    pub weekday: Option<i16>,
    pub hour: Option<i16>,
    pub watch_count: i32,
    pub total_secs: i64,
    pub mean_completion: f64,
    pub last_watched_at: Option<OffsetDateTime>,
}

macro_rules! to_row {
    ($r:expr) => {
        AffinityRow {
            id: $r.id,
            user_id: $r.user_id,
            series_id: $r.series_id,
            channel_id: $r.channel_id,
            genre: $r.genre,
            weekday: $r.weekday,
            hour: $r.hour,
            watch_count: $r.watch_count,
            total_secs: $r.total_secs,
            mean_completion: $r.mean_completion,
            last_watched_at: $r.last_watched_at,
            updated_at: $r.updated_at,
        }
    };
}

/// The rollup is a full recompute over the retained event window, so this SETs
/// the counters rather than incrementing them — an incremental upsert would
/// double-count every event on the next scheduled run.
pub async fn upsert<'e, E>(exec: E, user_id: Uuid, input: UpsertAffinity) -> Result<()>
where
    E: Executor<'e, Database = Postgres>,
{
    let id = Uuid::now_v7();
    sqlx::query!(
        r#"
        INSERT INTO livetv_affinity
            (id, user_id, series_id, channel_id, genre, weekday, hour,
             watch_count, total_secs, mean_completion, last_watched_at, updated_at)
        VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11, NOW())
        ON CONFLICT (user_id,
                     COALESCE(series_id, ''),
                     COALESCE(channel_id, '00000000-0000-0000-0000-000000000000'::uuid),
                     COALESCE(genre, ''),
                     COALESCE(weekday, -1),
                     COALESCE(hour, -1))
        DO UPDATE SET
            watch_count     = EXCLUDED.watch_count,
            total_secs      = EXCLUDED.total_secs,
            mean_completion = EXCLUDED.mean_completion,
            last_watched_at = EXCLUDED.last_watched_at,
            updated_at      = NOW()
        "#,
        id,
        user_id,
        input.series_id,
        input.channel_id,
        input.genre,
        input.weekday,
        input.hour,
        input.watch_count,
        input.total_secs,
        input.mean_completion,
        input.last_watched_at,
    )
    .execute(exec)
    .await?;
    Ok(())
}

pub async fn series_affinity<'e, E>(
    exec: E,
    user_id: Uuid,
    series_ids: &[String],
) -> Result<Vec<AffinityRow>>
where
    E: Executor<'e, Database = Postgres>,
{
    let rows = sqlx::query!(
        r#"
        SELECT id, user_id, series_id, channel_id, genre, weekday, hour,
               watch_count, total_secs, mean_completion, last_watched_at, updated_at
        FROM livetv_affinity
        WHERE user_id = $1 AND series_id = ANY($2)
        "#,
        user_id,
        series_ids
    )
    .fetch_all(exec)
    .await?;
    Ok(rows.into_iter().map(|r| to_row!(r)).collect())
}

pub async fn channel_affinity<'e, E>(
    exec: E,
    user_id: Uuid,
    channel_ids: &[Uuid],
) -> Result<Vec<AffinityRow>>
where
    E: Executor<'e, Database = Postgres>,
{
    let rows = sqlx::query!(
        r#"
        SELECT id, user_id, series_id, channel_id, genre, weekday, hour,
               watch_count, total_secs, mean_completion, last_watched_at, updated_at
        FROM livetv_affinity
        WHERE user_id = $1 AND series_id IS NULL AND genre IS NULL AND weekday IS NULL
          AND channel_id = ANY($2)
        "#,
        user_id,
        channel_ids
    )
    .fetch_all(exec)
    .await?;
    Ok(rows.into_iter().map(|r| to_row!(r)).collect())
}

pub async fn genre_affinity<'e, E>(
    exec: E,
    user_id: Uuid,
    genres: &[String],
) -> Result<Vec<AffinityRow>>
where
    E: Executor<'e, Database = Postgres>,
{
    let rows = sqlx::query!(
        r#"
        SELECT id, user_id, series_id, channel_id, genre, weekday, hour,
               watch_count, total_secs, mean_completion, last_watched_at, updated_at
        FROM livetv_affinity
        WHERE user_id = $1 AND genre = ANY($2)
        "#,
        user_id,
        genres
    )
    .fetch_all(exec)
    .await?;
    Ok(rows.into_iter().map(|r| to_row!(r)).collect())
}

pub async fn delete_for_user<'e, E>(exec: E, user_id: Uuid) -> Result<u64>
where
    E: Executor<'e, Database = Postgres>,
{
    let r = sqlx::query!("DELETE FROM livetv_affinity WHERE user_id = $1", user_id)
        .execute(exec)
        .await?;
    Ok(r.rows_affected())
}

/// Rows for users whose events have all aged out never come back through the
/// per-user recompute, so they are swept here instead of lingering forever.
pub async fn delete_orphaned<'e, E>(exec: E) -> Result<u64>
where
    E: Executor<'e, Database = Postgres>,
{
    let r = sqlx::query!(
        r#"
        DELETE FROM livetv_affinity a
        WHERE NOT EXISTS (SELECT 1 FROM livetv_watch_events w WHERE w.user_id = a.user_id)
        "#
    )
    .execute(exec)
    .await?;
    Ok(r.rows_affected())
}

pub async fn distinct_users<'e, E>(exec: E) -> Result<Vec<Uuid>>
where
    E: Executor<'e, Database = Postgres>,
{
    let rows = sqlx::query!(
        r#"SELECT DISTINCT user_id AS "user_id!" FROM livetv_watch_events WHERE user_id IS NOT NULL"#
    )
    .fetch_all(exec)
    .await?;
    Ok(rows.into_iter().map(|r| r.user_id).collect())
}

/// Every time-of-day grain for a user, so a caller can say how concentrated
/// their viewing is in the slot that is running right now.
pub async fn slot_totals<'e, E>(exec: E, user_id: Uuid) -> Result<Vec<(i16, i16, i32)>>
where
    E: Executor<'e, Database = Postgres>,
{
    let rows = sqlx::query!(
        r#"
        SELECT weekday AS "weekday!", hour AS "hour!", watch_count
        FROM livetv_affinity
        WHERE user_id = $1 AND series_id IS NULL AND channel_id IS NULL AND genre IS NULL
          AND weekday IS NOT NULL AND hour IS NOT NULL
        "#,
        user_id
    )
    .fetch_all(exec)
    .await?;
    Ok(rows
        .into_iter()
        .map(|r| (r.weekday, r.hour, r.watch_count))
        .collect())
}
