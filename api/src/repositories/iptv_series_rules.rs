//! IPTV series rules repository operations.

use super::Result;
use sqlx::{Executor, Postgres};
use time::OffsetDateTime;
use uuid::Uuid;

// =============================================================================
// Types
// =============================================================================

#[derive(Debug)]
pub struct SeriesRuleRow {
    pub id: Uuid,
    pub title_pattern: String,
    pub channel_id: Option<Uuid>,
    pub record_new_only: bool,
    pub keep_count: Option<i32>,
    pub pre_padding_secs: i32,
    pub post_padding_secs: i32,
    pub enabled: bool,
    pub created_at: OffsetDateTime,
}

#[derive(Debug)]
pub struct CreateSeriesRule {
    pub title_pattern: String,
    pub channel_id: Option<Uuid>,
    pub record_new_only: Option<bool>,
    pub keep_count: Option<i32>,
    pub pre_padding_secs: Option<i32>,
    pub post_padding_secs: Option<i32>,
}

#[derive(Debug)]
pub struct UpdateSeriesRule {
    pub title_pattern: Option<String>,
    pub channel_id: Option<Uuid>,
    pub record_new_only: Option<bool>,
    pub keep_count: Option<i32>,
    pub pre_padding_secs: Option<i32>,
    pub post_padding_secs: Option<i32>,
    pub enabled: Option<bool>,
}

// =============================================================================
// Operations
// =============================================================================

/// Create a new series rule.
pub async fn create<'e, E>(exec: E, input: CreateSeriesRule) -> Result<SeriesRuleRow>
where
    E: Executor<'e, Database = Postgres>,
{
    let id = Uuid::now_v7();
    let r = sqlx::query!(
        r#"
        INSERT INTO iptv_series_rules (id, title_pattern, channel_id, record_new_only,
                                       keep_count, pre_padding_secs, post_padding_secs)
        VALUES ($1, $2, $3, COALESCE($4, true), $5, COALESCE($6, 60), COALESCE($7, 120))
        RETURNING id, title_pattern, channel_id, record_new_only, keep_count,
                  pre_padding_secs, post_padding_secs, enabled, created_at
        "#,
        id,
        input.title_pattern,
        input.channel_id,
        input.record_new_only,
        input.keep_count,
        input.pre_padding_secs,
        input.post_padding_secs,
    )
    .fetch_one(exec)
    .await?;

    Ok(SeriesRuleRow {
        id: r.id,
        title_pattern: r.title_pattern,
        channel_id: r.channel_id,
        record_new_only: r.record_new_only,
        keep_count: r.keep_count,
        pre_padding_secs: r.pre_padding_secs,
        post_padding_secs: r.post_padding_secs,
        enabled: r.enabled,
        created_at: r.created_at,
    })
}

/// Get a series rule by ID.
pub async fn get_by_id<'e, E>(exec: E, id: Uuid) -> Result<Option<SeriesRuleRow>>
where
    E: Executor<'e, Database = Postgres>,
{
    let row = sqlx::query!(
        r#"
        SELECT id, title_pattern, channel_id, record_new_only, keep_count,
               pre_padding_secs, post_padding_secs, enabled, created_at
        FROM iptv_series_rules
        WHERE id = $1
        "#,
        id,
    )
    .fetch_optional(exec)
    .await?;

    Ok(row.map(|r| SeriesRuleRow {
        id: r.id,
        title_pattern: r.title_pattern,
        channel_id: r.channel_id,
        record_new_only: r.record_new_only,
        keep_count: r.keep_count,
        pre_padding_secs: r.pre_padding_secs,
        post_padding_secs: r.post_padding_secs,
        enabled: r.enabled,
        created_at: r.created_at,
    }))
}

/// List all series rules.
pub async fn list_all<'e, E>(exec: E) -> Result<Vec<SeriesRuleRow>>
where
    E: Executor<'e, Database = Postgres>,
{
    let rows = sqlx::query!(
        r#"
        SELECT id, title_pattern, channel_id, record_new_only, keep_count,
               pre_padding_secs, post_padding_secs, enabled, created_at
        FROM iptv_series_rules
        ORDER BY created_at ASC
        "#,
    )
    .fetch_all(exec)
    .await?;

    Ok(rows
        .into_iter()
        .map(|r| SeriesRuleRow {
            id: r.id,
            title_pattern: r.title_pattern,
            channel_id: r.channel_id,
            record_new_only: r.record_new_only,
            keep_count: r.keep_count,
            pre_padding_secs: r.pre_padding_secs,
            post_padding_secs: r.post_padding_secs,
            enabled: r.enabled,
            created_at: r.created_at,
        })
        .collect())
}

/// Update a series rule.
pub async fn update<'e, E>(exec: E, id: Uuid, input: UpdateSeriesRule) -> Result<bool>
where
    E: Executor<'e, Database = Postgres>,
{
    let result = sqlx::query!(
        r#"
        UPDATE iptv_series_rules SET
            title_pattern = COALESCE($2, title_pattern),
            channel_id = COALESCE($3, channel_id),
            record_new_only = COALESCE($4, record_new_only),
            keep_count = COALESCE($5, keep_count),
            pre_padding_secs = COALESCE($6, pre_padding_secs),
            post_padding_secs = COALESCE($7, post_padding_secs),
            enabled = COALESCE($8, enabled)
        WHERE id = $1
        "#,
        id,
        input.title_pattern,
        input.channel_id,
        input.record_new_only,
        input.keep_count,
        input.pre_padding_secs,
        input.post_padding_secs,
        input.enabled,
    )
    .execute(exec)
    .await?;

    Ok(result.rows_affected() > 0)
}

/// Delete a series rule by ID.
pub async fn delete<'e, E>(exec: E, id: Uuid) -> Result<bool>
where
    E: Executor<'e, Database = Postgres>,
{
    let result = sqlx::query!(
        r#"
        DELETE FROM iptv_series_rules WHERE id = $1
        "#,
        id,
    )
    .execute(exec)
    .await?;

    Ok(result.rows_affected() > 0)
}

/// List enabled series rules.
pub async fn list_enabled<'e, E>(exec: E) -> Result<Vec<SeriesRuleRow>>
where
    E: Executor<'e, Database = Postgres>,
{
    let rows = sqlx::query!(
        r#"
        SELECT id, title_pattern, channel_id, record_new_only, keep_count,
               pre_padding_secs, post_padding_secs, enabled, created_at
        FROM iptv_series_rules
        WHERE enabled = true
        ORDER BY created_at ASC
        "#,
    )
    .fetch_all(exec)
    .await?;

    Ok(rows
        .into_iter()
        .map(|r| SeriesRuleRow {
            id: r.id,
            title_pattern: r.title_pattern,
            channel_id: r.channel_id,
            record_new_only: r.record_new_only,
            keep_count: r.keep_count,
            pre_padding_secs: r.pre_padding_secs,
            post_padding_secs: r.post_padding_secs,
            enabled: r.enabled,
            created_at: r.created_at,
        })
        .collect())
}
