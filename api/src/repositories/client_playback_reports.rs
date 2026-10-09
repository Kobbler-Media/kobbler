use super::Result;
use sqlx::{Executor, Postgres};
use time::OffsetDateTime;
use uuid::Uuid;

#[derive(Debug, Clone)]
pub struct ReportRow {
    pub id: Uuid,
    pub user_id: Option<Uuid>,
    pub client_kind: String,
    pub app_version: Option<String>,
    pub device_label: Option<String>,
    pub media_type: Option<String>,
    pub media_id: Option<String>,
    pub stream_url: Option<String>,
    pub session_id: Option<String>,
    pub detail: String,
    pub created_at: OffsetDateTime,
}

#[derive(Debug)]
pub struct CreateReport {
    pub user_id: Option<Uuid>,
    pub client_kind: String,
    pub app_version: Option<String>,
    pub device_label: Option<String>,
    pub media_type: Option<String>,
    pub media_id: Option<String>,
    pub stream_url: Option<String>,
    pub session_id: Option<String>,
    pub detail: String,
}

pub async fn create<'e, E>(exec: E, input: CreateReport) -> Result<Uuid>
where
    E: Executor<'e, Database = Postgres>,
{
    let id = Uuid::now_v7();
    let r = sqlx::query!(
        r#"
        INSERT INTO client_playback_reports (
            id, user_id, client_kind, app_version, device_label,
            media_type, media_id, stream_url, session_id, detail)
        VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10)
        RETURNING id
        "#,
        id,
        input.user_id,
        input.client_kind,
        input.app_version,
        input.device_label,
        input.media_type,
        input.media_id,
        input.stream_url,
        input.session_id,
        input.detail,
    )
    .fetch_one(exec)
    .await?;
    Ok(r.id)
}

pub async fn list_recent<'e, E>(exec: E, limit: i64) -> Result<Vec<ReportRow>>
where
    E: Executor<'e, Database = Postgres>,
{
    let rows = sqlx::query!(
        r#"
        SELECT id, user_id, client_kind, app_version, device_label, media_type,
               media_id, stream_url, session_id, detail, created_at
        FROM client_playback_reports
        ORDER BY created_at DESC
        LIMIT $1
        "#,
        limit,
    )
    .fetch_all(exec)
    .await?;

    Ok(rows
        .into_iter()
        .map(|r| ReportRow {
            id: r.id,
            user_id: r.user_id,
            client_kind: r.client_kind,
            app_version: r.app_version,
            device_label: r.device_label,
            media_type: r.media_type,
            media_id: r.media_id,
            stream_url: r.stream_url,
            session_id: r.session_id,
            detail: r.detail,
            created_at: r.created_at,
        })
        .collect())
}
