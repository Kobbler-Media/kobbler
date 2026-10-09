use super::Result;
use sqlx::{Executor, Postgres};
use time::OffsetDateTime;
use uuid::Uuid;

#[derive(Debug, Clone)]
pub struct SessionRow {
    pub id: Uuid,
    pub session_id: String,
    pub channel_id: Uuid,
    pub lease_id: Option<Uuid>,
    pub user_id: Option<Uuid>,
    pub mode: String,
    pub buffer_dir: String,
    pub started_at: OffsetDateTime,
    pub ended_at: Option<OffsetDateTime>,
    pub last_access: OffsetDateTime,
    pub recording_id: Option<Uuid>,
}

#[derive(Debug)]
pub struct CreateSession {
    pub session_id: String,
    pub channel_id: Uuid,
    pub lease_id: Option<Uuid>,
    pub user_id: Option<Uuid>,
    pub mode: String,
    pub buffer_dir: String,
}

pub async fn create<'e, E>(exec: E, input: CreateSession) -> Result<SessionRow>
where
    E: Executor<'e, Database = Postgres>,
{
    let id = Uuid::now_v7();
    let r = sqlx::query!(
        r#"
        INSERT INTO livetv_sessions (id, session_id, channel_id, lease_id, user_id, mode, buffer_dir)
        VALUES ($1, $2, $3, $4, $5, $6, $7)
        RETURNING id, session_id, channel_id, lease_id, user_id, mode, buffer_dir,
                  started_at, ended_at, last_access, recording_id
        "#,
        id,
        input.session_id,
        input.channel_id,
        input.lease_id,
        input.user_id,
        input.mode,
        input.buffer_dir,
    )
    .fetch_one(exec)
    .await?;
    Ok(SessionRow {
        id: r.id,
        session_id: r.session_id,
        channel_id: r.channel_id,
        lease_id: r.lease_id,
        user_id: r.user_id,
        mode: r.mode,
        buffer_dir: r.buffer_dir,
        started_at: r.started_at,
        ended_at: r.ended_at,
        last_access: r.last_access,
        recording_id: r.recording_id,
    })
}

pub async fn get_by_session_id<'e, E>(exec: E, session_id: &str) -> Result<Option<SessionRow>>
where
    E: Executor<'e, Database = Postgres>,
{
    let row = sqlx::query!(
        r#"
        SELECT id, session_id, channel_id, lease_id, user_id, mode, buffer_dir,
               started_at, ended_at, last_access, recording_id
        FROM livetv_sessions WHERE session_id = $1
        "#,
        session_id,
    )
    .fetch_optional(exec)
    .await?;
    Ok(row.map(|r| SessionRow {
        id: r.id,
        session_id: r.session_id,
        channel_id: r.channel_id,
        lease_id: r.lease_id,
        user_id: r.user_id,
        mode: r.mode,
        buffer_dir: r.buffer_dir,
        started_at: r.started_at,
        ended_at: r.ended_at,
        last_access: r.last_access,
        recording_id: r.recording_id,
    }))
}

pub async fn touch<'e, E>(exec: E, session_id: &str) -> Result<()>
where
    E: Executor<'e, Database = Postgres>,
{
    sqlx::query!(
        "UPDATE livetv_sessions SET last_access = NOW() WHERE session_id = $1",
        session_id
    )
    .execute(exec)
    .await?;
    Ok(())
}

pub async fn end<'e, E>(exec: E, session_id: &str) -> Result<bool>
where
    E: Executor<'e, Database = Postgres>,
{
    let res = sqlx::query!(
        "UPDATE livetv_sessions SET ended_at = NOW() WHERE session_id = $1 AND ended_at IS NULL",
        session_id
    )
    .execute(exec)
    .await?;
    Ok(res.rows_affected() > 0)
}

pub async fn end_all_active<'e, E>(exec: E) -> Result<u64>
where
    E: Executor<'e, Database = Postgres>,
{
    let res = sqlx::query!("UPDATE livetv_sessions SET ended_at = NOW() WHERE ended_at IS NULL")
        .execute(exec)
        .await?;
    Ok(res.rows_affected())
}

pub async fn set_recording<'e, E>(exec: E, session_id: &str, recording_id: Uuid) -> Result<()>
where
    E: Executor<'e, Database = Postgres>,
{
    sqlx::query!(
        "UPDATE livetv_sessions SET recording_id = $2 WHERE session_id = $1",
        session_id,
        recording_id
    )
    .execute(exec)
    .await?;
    Ok(())
}

/// Re-point an existing session at a new channel/lease/buffer (inline channel switch).
pub async fn retune<'e, E>(
    exec: E,
    session_id: &str,
    channel_id: Uuid,
    lease_id: Option<Uuid>,
    mode: &str,
    buffer_dir: &str,
) -> Result<()>
where
    E: Executor<'e, Database = Postgres>,
{
    sqlx::query!(
        r#"
        UPDATE livetv_sessions
        SET channel_id = $2, lease_id = $3, mode = $4, buffer_dir = $5,
            started_at = NOW(), last_access = NOW(), recording_id = NULL
        WHERE session_id = $1
        "#,
        session_id,
        channel_id,
        lease_id,
        mode,
        buffer_dir,
    )
    .execute(exec)
    .await?;
    Ok(())
}

pub async fn all_buffer_dirs<'e, E>(exec: E) -> Result<Vec<String>>
where
    E: Executor<'e, Database = Postgres>,
{
    let rows = sqlx::query!("SELECT DISTINCT buffer_dir FROM livetv_sessions")
        .fetch_all(exec)
        .await?;
    Ok(rows.into_iter().map(|r| r.buffer_dir).collect())
}

pub async fn active_buffer_dirs<'e, E>(exec: E) -> Result<Vec<String>>
where
    E: Executor<'e, Database = Postgres>,
{
    let rows =
        sqlx::query!("SELECT DISTINCT buffer_dir FROM livetv_sessions WHERE ended_at IS NULL")
            .fetch_all(exec)
            .await?;
    Ok(rows.into_iter().map(|r| r.buffer_dir).collect())
}

/// Buffers promoted to a recording survive their session — the segments are the
/// recording.
pub async fn recorded_buffer_dirs<'e, E>(exec: E) -> Result<Vec<String>>
where
    E: Executor<'e, Database = Postgres>,
{
    let rows = sqlx::query!(
        "SELECT DISTINCT buffer_dir FROM livetv_sessions WHERE recording_id IS NOT NULL"
    )
    .fetch_all(exec)
    .await?;
    Ok(rows.into_iter().map(|r| r.buffer_dir).collect())
}
