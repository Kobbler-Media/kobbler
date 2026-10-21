use super::Result;
use sqlx::{Executor, Postgres};
use uuid::Uuid;

#[derive(Debug, Clone)]
pub struct Keep {
    pub session_id: Uuid,
    pub job_id: Option<Uuid>,
    pub state: String,
    pub progress: f64,
    pub error: Option<String>,
}

pub async fn get<'e, E>(exec: E, session_id: Uuid) -> Result<Option<Keep>>
where
    E: Executor<'e, Database = Postgres>,
{
    let row = sqlx::query_as!(
        Keep,
        "SELECT session_id, job_id, state, progress, error FROM debrid_keeps WHERE session_id = $1",
        session_id,
    )
    .fetch_optional(exec)
    .await?;
    Ok(row)
}

/// Claim the keep for a session: `true` when this call created it or revived a failed
/// one, so it is the one to start the job.
pub async fn begin<'e, E>(exec: E, session_id: Uuid) -> Result<bool>
where
    E: Executor<'e, Database = Postgres>,
{
    let started = sqlx::query_scalar!(
        r#"
        INSERT INTO debrid_keeps (session_id) VALUES ($1)
        ON CONFLICT (session_id) DO UPDATE
            SET state = 'queued', progress = 0, error = NULL, job_id = NULL, updated_at = NOW()
            WHERE debrid_keeps.state = 'failed'
        RETURNING session_id
        "#,
        session_id,
    )
    .fetch_optional(exec)
    .await?;
    Ok(started.is_some())
}

pub async fn set_job<'e, E>(exec: E, session_id: Uuid, job_id: Uuid) -> Result<()>
where
    E: Executor<'e, Database = Postgres>,
{
    sqlx::query!(
        "UPDATE debrid_keeps SET job_id = $2, updated_at = NOW() WHERE session_id = $1",
        session_id,
        job_id,
    )
    .execute(exec)
    .await?;
    Ok(())
}

pub async fn set_state<'e, E>(
    exec: E,
    session_id: Uuid,
    state: &str,
    progress: f64,
    error: Option<&str>,
) -> Result<()>
where
    E: Executor<'e, Database = Postgres>,
{
    sqlx::query!(
        r#"
        UPDATE debrid_keeps
        SET state = $2, progress = $3, error = $4, updated_at = NOW()
        WHERE session_id = $1
        "#,
        session_id,
        state,
        progress.clamp(0.0, 1.0),
        error,
    )
    .execute(exec)
    .await?;
    Ok(())
}

pub async fn set_done<'e, E>(exec: E, session_id: Uuid, file_path: &str) -> Result<()>
where
    E: Executor<'e, Database = Postgres>,
{
    sqlx::query!(
        r#"
        UPDATE debrid_keeps
        SET state = 'done', progress = 1, error = NULL, file_path = $2, updated_at = NOW()
        WHERE session_id = $1
        "#,
        session_id,
        file_path,
    )
    .execute(exec)
    .await?;
    Ok(())
}
