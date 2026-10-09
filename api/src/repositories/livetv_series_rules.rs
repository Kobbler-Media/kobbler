use super::Result;
use sqlx::{Executor, Postgres};
use time::{OffsetDateTime, Time};
use uuid::Uuid;

#[derive(Debug, Clone)]
pub struct SeriesRuleRow {
    pub id: Uuid,
    pub series_id: String,
    pub title: String,
    pub enabled: bool,
    pub channel_ids: Vec<Uuid>,
    pub weekdays: Vec<i16>,
    pub window_start: Option<Time>,
    pub window_end: Option<Time>,
    pub new_only: bool,
    pub min_season: Option<i32>,
    pub keep_count: Option<i32>,
    pub keep_until: String,
    pub keep_until_date: Option<OffsetDateTime>,
    pub padding_before_secs: i32,
    pub padding_after_secs: i32,
    pub on_clash: String,
    pub priority: i32,
    pub created_at: OffsetDateTime,
    pub updated_at: OffsetDateTime,
}

#[derive(Debug)]
pub struct CreateSeriesRule {
    pub series_id: String,
    pub title: String,
    pub enabled: bool,
    pub channel_ids: Vec<Uuid>,
    pub weekdays: Vec<i16>,
    pub window_start: Option<Time>,
    pub window_end: Option<Time>,
    pub new_only: bool,
    pub min_season: Option<i32>,
    pub keep_count: Option<i32>,
    pub keep_until: String,
    pub keep_until_date: Option<OffsetDateTime>,
    pub padding_before_secs: i32,
    pub padding_after_secs: i32,
    pub on_clash: String,
    pub priority: i32,
}

#[derive(Debug, Default)]
pub struct UpdateSeriesRule {
    pub title: Option<String>,
    pub enabled: Option<bool>,
    pub channel_ids: Option<Vec<Uuid>>,
    pub weekdays: Option<Vec<i16>>,
    pub window_start: Option<Option<Time>>,
    pub window_end: Option<Option<Time>>,
    pub new_only: Option<bool>,
    pub min_season: Option<Option<i32>>,
    pub keep_count: Option<Option<i32>>,
    pub keep_until: Option<String>,
    pub keep_until_date: Option<Option<OffsetDateTime>>,
    pub padding_before_secs: Option<i32>,
    pub padding_after_secs: Option<i32>,
    pub on_clash: Option<String>,
    pub priority: Option<i32>,
}

macro_rules! to_row {
    ($r:expr) => {
        SeriesRuleRow {
            id: $r.id,
            series_id: $r.series_id,
            title: $r.title,
            enabled: $r.enabled,
            channel_ids: $r.channel_ids,
            weekdays: $r.weekdays,
            window_start: $r.window_start,
            window_end: $r.window_end,
            new_only: $r.new_only,
            min_season: $r.min_season,
            keep_count: $r.keep_count,
            keep_until: $r.keep_until,
            keep_until_date: $r.keep_until_date,
            padding_before_secs: $r.padding_before_secs,
            padding_after_secs: $r.padding_after_secs,
            on_clash: $r.on_clash,
            priority: $r.priority,
            created_at: $r.created_at,
            updated_at: $r.updated_at,
        }
    };
}

pub async fn create<'e, E>(exec: E, input: CreateSeriesRule) -> Result<SeriesRuleRow>
where
    E: Executor<'e, Database = Postgres>,
{
    let id = Uuid::now_v7();
    let r = sqlx::query!(
        r#"
        INSERT INTO livetv_series_rules
            (id, series_id, title, enabled, channel_ids, weekdays, window_start, window_end,
             new_only, min_season, keep_count, keep_until, keep_until_date,
             padding_before_secs, padding_after_secs, on_clash, priority)
        VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13,$14,$15,$16,$17)
        RETURNING id, series_id, title, enabled, channel_ids, weekdays, window_start, window_end,
                  new_only, min_season, keep_count, keep_until, keep_until_date,
                  padding_before_secs, padding_after_secs, on_clash, priority,
                  created_at, updated_at
        "#,
        id,
        input.series_id,
        input.title,
        input.enabled,
        &input.channel_ids,
        &input.weekdays,
        input.window_start,
        input.window_end,
        input.new_only,
        input.min_season,
        input.keep_count,
        input.keep_until,
        input.keep_until_date,
        input.padding_before_secs,
        input.padding_after_secs,
        input.on_clash,
        input.priority,
    )
    .fetch_one(exec)
    .await?;
    Ok(to_row!(r))
}

pub async fn update<'e, E>(
    exec: E,
    id: Uuid,
    input: UpdateSeriesRule,
) -> Result<Option<SeriesRuleRow>>
where
    E: Executor<'e, Database = Postgres>,
{
    let row = sqlx::query!(
        r#"
        UPDATE livetv_series_rules SET
            title = COALESCE($2, title),
            enabled = COALESCE($3, enabled),
            channel_ids = COALESCE($4, channel_ids),
            weekdays = COALESCE($5, weekdays),
            window_start = CASE WHEN $6::bool THEN $7 ELSE window_start END,
            window_end = CASE WHEN $8::bool THEN $9 ELSE window_end END,
            new_only = COALESCE($10, new_only),
            min_season = CASE WHEN $11::bool THEN $12 ELSE min_season END,
            keep_count = CASE WHEN $13::bool THEN $14 ELSE keep_count END,
            keep_until = COALESCE($15, keep_until),
            keep_until_date = CASE WHEN $16::bool THEN $17 ELSE keep_until_date END,
            padding_before_secs = COALESCE($18, padding_before_secs),
            padding_after_secs = COALESCE($19, padding_after_secs),
            on_clash = COALESCE($20, on_clash),
            priority = COALESCE($21, priority),
            updated_at = NOW()
        WHERE id = $1
        RETURNING id, series_id, title, enabled, channel_ids, weekdays, window_start, window_end,
                  new_only, min_season, keep_count, keep_until, keep_until_date,
                  padding_before_secs, padding_after_secs, on_clash, priority,
                  created_at, updated_at
        "#,
        id,
        input.title,
        input.enabled,
        input.channel_ids.as_deref(),
        input.weekdays.as_deref(),
        input.window_start.is_some(),
        input.window_start.flatten(),
        input.window_end.is_some(),
        input.window_end.flatten(),
        input.new_only,
        input.min_season.is_some(),
        input.min_season.flatten(),
        input.keep_count.is_some(),
        input.keep_count.flatten(),
        input.keep_until,
        input.keep_until_date.is_some(),
        input.keep_until_date.flatten(),
        input.padding_before_secs,
        input.padding_after_secs,
        input.on_clash,
        input.priority,
    )
    .fetch_optional(exec)
    .await?;
    Ok(row.map(|r| to_row!(r)))
}

pub async fn get_by_id<'e, E>(exec: E, id: Uuid) -> Result<Option<SeriesRuleRow>>
where
    E: Executor<'e, Database = Postgres>,
{
    let row = sqlx::query!(
        r#"
        SELECT id, series_id, title, enabled, channel_ids, weekdays, window_start, window_end,
               new_only, min_season, keep_count, keep_until, keep_until_date,
               padding_before_secs, padding_after_secs, on_clash, priority, created_at, updated_at
        FROM livetv_series_rules WHERE id = $1
        "#,
        id
    )
    .fetch_optional(exec)
    .await?;
    Ok(row.map(|r| to_row!(r)))
}

pub async fn list_all<'e, E>(exec: E, enabled_only: bool) -> Result<Vec<SeriesRuleRow>>
where
    E: Executor<'e, Database = Postgres>,
{
    let rows = sqlx::query!(
        r#"
        SELECT id, series_id, title, enabled, channel_ids, weekdays, window_start, window_end,
               new_only, min_season, keep_count, keep_until, keep_until_date,
               padding_before_secs, padding_after_secs, on_clash, priority, created_at, updated_at
        FROM livetv_series_rules
        WHERE (NOT $1::bool OR enabled)
        ORDER BY priority DESC, title ASC
        "#,
        enabled_only,
    )
    .fetch_all(exec)
    .await?;
    Ok(rows.into_iter().map(|r| to_row!(r)).collect())
}

pub async fn delete<'e, E>(exec: E, id: Uuid) -> Result<bool>
where
    E: Executor<'e, Database = Postgres>,
{
    let res = sqlx::query!("DELETE FROM livetv_series_rules WHERE id = $1", id)
        .execute(exec)
        .await?;
    Ok(res.rows_affected() > 0)
}
