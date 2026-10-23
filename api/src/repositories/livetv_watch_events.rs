use super::Result;
use sqlx::{Executor, Postgres};
use time::OffsetDateTime;
use uuid::Uuid;

#[derive(Debug)]
pub struct CreateWatchEvent {
    pub user_id: Option<Uuid>,
    pub channel_id: Uuid,
    pub guide_id: Option<Uuid>,
    pub series_id: Option<String>,
    pub title: Option<String>,
    pub episode_key: Option<String>,
    pub started_at: OffsetDateTime,
    pub ended_at: Option<OffsetDateTime>,
    pub watched_secs: i32,
    pub programme_secs: Option<i32>,
    pub source: String,
}

pub async fn create<'e, E>(exec: E, input: CreateWatchEvent) -> Result<Uuid>
where
    E: Executor<'e, Database = Postgres>,
{
    let id = Uuid::now_v7();
    sqlx::query!(
        r#"
        INSERT INTO livetv_watch_events
            (id, user_id, channel_id, guide_id, series_id, title, episode_key,
             started_at, ended_at, watched_secs, programme_secs, source)
        VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12)
        "#,
        id,
        input.user_id,
        input.channel_id,
        input.guide_id,
        input.series_id,
        input.title,
        input.episode_key,
        input.started_at,
        input.ended_at,
        input.watched_secs,
        input.programme_secs,
        input.source,
    )
    .execute(exec)
    .await?;
    Ok(id)
}

pub async fn count_for_channel<'e, E>(exec: E, channel_id: Uuid) -> Result<i64>
where
    E: Executor<'e, Database = Postgres>,
{
    let r = sqlx::query!(
        r#"SELECT COUNT(*) AS "count!" FROM livetv_watch_events WHERE channel_id = $1"#,
        channel_id
    )
    .fetch_one(exec)
    .await?;
    Ok(r.count)
}

#[derive(Debug)]
pub struct WatchFactRow {
    pub user_id: Uuid,
    pub channel_id: Uuid,
    pub series_id: Option<String>,
    pub genres: Vec<String>,
    pub started_at: OffsetDateTime,
    pub watched_secs: i32,
    pub programme_secs: Option<i32>,
}

pub async fn facts_for_user<'e, E>(
    exec: E,
    user_id: Uuid,
    since: OffsetDateTime,
) -> Result<Vec<WatchFactRow>>
where
    E: Executor<'e, Database = Postgres>,
{
    let rows = sqlx::query!(
        r#"
        SELECT w.channel_id, w.series_id, w.started_at, w.watched_secs, w.programme_secs,
               g.genres AS "genres?"
        FROM livetv_watch_events w
        LEFT JOIN livetv_guide g ON g.id = w.guide_id
        WHERE w.user_id = $1 AND w.started_at >= $2
        ORDER BY w.started_at
        "#,
        user_id,
        since
    )
    .fetch_all(exec)
    .await?;
    Ok(rows
        .into_iter()
        .map(|r| WatchFactRow {
            user_id,
            channel_id: r.channel_id,
            series_id: r.series_id,
            genres: r.genres.unwrap_or_default(),
            started_at: r.started_at,
            watched_secs: r.watched_secs,
            programme_secs: r.programme_secs,
        })
        .collect())
}

pub async fn prune_older_than<'e, E>(exec: E, days: i64) -> Result<u64>
where
    E: Executor<'e, Database = Postgres>,
{
    let cutoff = OffsetDateTime::now_utc() - time::Duration::days(days);
    let r = sqlx::query!(
        "DELETE FROM livetv_watch_events WHERE started_at < $1",
        cutoff
    )
    .execute(exec)
    .await?;
    Ok(r.rows_affected())
}

/// Per-airing completion ratios, newest first — the evidence behind "you have
/// watched 14 of the last 16".
pub async fn recent_series_completions<'e, E>(
    exec: E,
    user_id: Uuid,
    series_id: &str,
    limit: i64,
) -> Result<Vec<(f64, OffsetDateTime)>>
where
    E: Executor<'e, Database = Postgres>,
{
    let rows = sqlx::query!(
        r#"
        SELECT (watched_secs::float8 / programme_secs::float8) AS "completion!",
               started_at
        FROM livetv_watch_events
        WHERE user_id = $1 AND series_id = $2
          AND programme_secs IS NOT NULL AND programme_secs > 0
        ORDER BY started_at DESC
        LIMIT $3
        "#,
        user_id,
        series_id,
        limit
    )
    .fetch_all(exec)
    .await?;
    Ok(rows
        .into_iter()
        .map(|r| (r.completion.clamp(0.0, 1.0), r.started_at))
        .collect())
}

pub async fn series_watch_starts<'e, E>(
    exec: E,
    user_id: Uuid,
    series_ids: &[String],
    since: OffsetDateTime,
) -> Result<Vec<(String, OffsetDateTime)>>
where
    E: Executor<'e, Database = Postgres>,
{
    let rows = sqlx::query!(
        r#"
        SELECT series_id AS "series_id!", started_at
        FROM livetv_watch_events
        WHERE user_id = $1 AND series_id = ANY($2) AND started_at >= $3
        "#,
        user_id,
        series_ids,
        since
    )
    .fetch_all(exec)
    .await?;
    Ok(rows
        .into_iter()
        .map(|r| (r.series_id, r.started_at))
        .collect())
}
