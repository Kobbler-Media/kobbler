//! Activity log repository operations.

use super::Result;
use sqlx::{Executor, Postgres};
use time::OffsetDateTime;
use uuid::Uuid;

/// Input for creating an activity log entry.
pub struct CreateActivity<'a> {
    pub event_kind: &'a str,
    pub entity_type: Option<&'a str>,
    pub entity_id: Option<Uuid>,
    pub title: &'a str,
    pub subtitle: Option<&'a str>,
    pub image_url: Option<&'a str>,
    pub url: Option<&'a str>,
    pub user_id: Option<Uuid>,
}

/// Create an activity log entry.
pub async fn create<'e, E>(exec: E, input: &CreateActivity<'_>) -> Result<Uuid>
where
    E: Executor<'e, Database = Postgres>,
{
    let id = Uuid::now_v7();
    sqlx::query!(
        r#"INSERT INTO activity_log (id, event_kind, entity_type, entity_id, title, subtitle, image_url, url, user_id)
           VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9)"#,
        id,
        input.event_kind,
        input.entity_type,
        input.entity_id,
        input.title,
        input.subtitle,
        input.image_url,
        input.url,
        input.user_id,
    )
    .execute(exec)
    .await?;
    Ok(id)
}

/// Activity log row for listing.
#[derive(Debug)]
pub struct ActivityLogRow {
    pub id: Uuid,
    pub event_kind: String,
    pub entity_type: Option<String>,
    pub entity_id: Option<Uuid>,
    pub title: String,
    pub subtitle: Option<String>,
    pub image_url: Option<String>,
    pub url: Option<String>,
    pub created_at: OffsetDateTime,
}

/// Count total activity log entries.
pub async fn count<'e, E>(exec: E) -> Result<i64>
where
    E: Executor<'e, Database = Postgres>,
{
    let count = sqlx::query_scalar!(r#"SELECT COUNT(*) as "count!" FROM activity_log"#)
        .fetch_one(exec)
        .await?;
    Ok(count)
}

/// List activity log entries with optional cursor pagination.
pub async fn list<'e, E>(
    exec: E,
    cursor_ts: Option<OffsetDateTime>,
    cursor_id: Option<Uuid>,
    limit: i64,
) -> Result<Vec<ActivityLogRow>>
where
    E: Executor<'e, Database = Postgres>,
{
    let rows = if let (Some(ts), Some(id)) = (cursor_ts, cursor_id) {
        sqlx::query!(
            r#"
            SELECT id, event_kind, entity_type, entity_id, title, subtitle, image_url, url, created_at
            FROM activity_log
            WHERE (created_at, id) < ($1, $2)
            ORDER BY created_at DESC, id DESC
            LIMIT $3
            "#,
            ts,
            id,
            limit,
        )
        .fetch_all(exec)
        .await?
        .into_iter()
        .map(|r| ActivityLogRow {
            id: r.id,
            event_kind: r.event_kind,
            entity_type: r.entity_type,
            entity_id: r.entity_id,
            title: r.title,
            subtitle: r.subtitle,
            image_url: r.image_url,
            url: r.url,
            created_at: r.created_at,
        })
        .collect()
    } else {
        sqlx::query!(
            r#"
            SELECT id, event_kind, entity_type, entity_id, title, subtitle, image_url, url, created_at
            FROM activity_log
            ORDER BY created_at DESC, id DESC
            LIMIT $1
            "#,
            limit,
        )
        .fetch_all(exec)
        .await?
        .into_iter()
        .map(|r| ActivityLogRow {
            id: r.id,
            event_kind: r.event_kind,
            entity_type: r.entity_type,
            entity_id: r.entity_id,
            title: r.title,
            subtitle: r.subtitle,
            image_url: r.image_url,
            url: r.url,
            created_at: r.created_at,
        })
        .collect()
    };

    Ok(rows)
}

/// Delete activity log entries older than the given timestamp.
pub async fn cleanup_before<'e, E>(exec: E, before: OffsetDateTime) -> Result<u64>
where
    E: Executor<'e, Database = Postgres>,
{
    let result = sqlx::query!("DELETE FROM activity_log WHERE created_at < $1", before)
        .execute(exec)
        .await?;
    Ok(result.rows_affected())
}
