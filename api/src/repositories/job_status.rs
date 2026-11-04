//! Job status repository operations.

use super::Result;
use serde_json::Value;
use sqlx::{Executor, Postgres};
use uuid::Uuid;

/// Input for creating a job status entry.
#[derive(Debug)]
pub struct JobStatusInput {
    pub id: Uuid,
    pub job_type: String,
    pub status: String,
    pub payload: Value,
    pub entity_type: Option<String>,
    pub entity_id: Option<Uuid>,
    pub description: Option<String>,
    pub display: Option<Value>,
    pub scheduled: bool,
}

/// Create a job status entry.
pub async fn create<'e, E>(exec: E, input: JobStatusInput) -> Result<()>
where
    E: Executor<'e, Database = Postgres>,
{
    sqlx::query!(
        r#"
        INSERT INTO job_status (id, job_type, status, payload, entity_type, entity_id, description, display, scheduled, created_at)
        VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, NOW())
        "#,
        input.id,
        &input.job_type,
        &input.status,
        &input.payload,
        input.entity_type.as_deref(),
        input.entity_id,
        input.description.as_deref(),
        input.display.as_ref(),
        input.scheduled,
    )
    .execute(exec)
    .await?;
    Ok(())
}

/// Create a job status entry if it doesn't already exist.
pub async fn create_if_not_exists<'e, E>(exec: E, input: JobStatusInput) -> Result<()>
where
    E: Executor<'e, Database = Postgres>,
{
    sqlx::query!(
        r#"
        INSERT INTO job_status (id, job_type, status, payload, entity_type, entity_id, description, display, scheduled, created_at)
        VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, NOW())
        ON CONFLICT (id) DO NOTHING
        "#,
        input.id,
        &input.job_type,
        &input.status,
        &input.payload,
        input.entity_type.as_deref(),
        input.entity_id,
        input.description.as_deref(),
        input.display.as_ref(),
        input.scheduled,
    )
    .execute(exec)
    .await?;
    Ok(())
}

/// Update job progress and result.
pub async fn update_progress<'e, E>(
    exec: E,
    id: Uuid,
    progress: i32,
    result: Option<&Value>,
) -> Result<()>
where
    E: Executor<'e, Database = Postgres>,
{
    sqlx::query!(
        r#"
        UPDATE job_status
        SET progress = $2, result = $3
        WHERE id = $1
        "#,
        id,
        progress as i16,
        result,
    )
    .execute(exec)
    .await?;
    Ok(())
}

pub async fn latest_active_of_type<'e, E>(
    exec: E,
    job_type: &str,
) -> Result<Option<nats_jobs::JobStatusRow>>
where
    E: Executor<'e, Database = Postgres>,
{
    let row = sqlx::query_as!(
        nats_jobs::JobStatusRow,
        r#"
        SELECT id, job_type, status, payload, result, error, progress,
               entity_type, entity_id, description, display, scheduled,
               created_at, started_at, completed_at
        FROM job_status
        WHERE job_type = $1 AND status IN ('pending', 'running', 'retrying')
        ORDER BY created_at DESC
        LIMIT 1
        "#,
        job_type,
    )
    .fetch_optional(exec)
    .await?;
    Ok(row)
}

/// Get a job by ID.
pub async fn get_by_id<'e, E>(exec: E, id: Uuid) -> Result<Option<nats_jobs::JobStatusRow>>
where
    E: Executor<'e, Database = Postgres>,
{
    let row = sqlx::query_as!(
        nats_jobs::JobStatusRow,
        r#"
        SELECT id, job_type, status, payload, result, error, progress,
               entity_type, entity_id, description, display, scheduled,
               created_at, started_at, completed_at
        FROM job_status WHERE id = $1
        "#,
        id,
    )
    .fetch_optional(exec)
    .await?;
    Ok(row)
}

/// Update job status with result.
pub async fn update_status<'e, E>(
    exec: E,
    id: Uuid,
    status: &str,
    result: Option<&Value>,
    error: Option<&str>,
) -> Result<()>
where
    E: Executor<'e, Database = Postgres>,
{
    sqlx::query!(
        r#"
        UPDATE job_status
        SET status = $2,
            result = COALESCE($3, result),
            error = COALESCE($4, error),
            completed_at = CASE WHEN $2 IN ('completed', 'failed') THEN NOW() ELSE completed_at END
        WHERE id = $1
        "#,
        id,
        status,
        result,
        error,
    )
    .execute(exec)
    .await?;
    Ok(())
}

/// Delete a job by ID.
pub async fn delete<'e, E>(exec: E, id: Uuid) -> Result<bool>
where
    E: Executor<'e, Database = Postgres>,
{
    let result = sqlx::query!("DELETE FROM job_status WHERE id = $1", id)
        .execute(exec)
        .await?;
    Ok(result.rows_affected() > 0)
}

/// Cancel all pending jobs for a given entity.
pub async fn cancel_by_entity(
    pool: &sqlx::PgPool,
    entity_type: &str,
    entity_id: Uuid,
) -> Result<u64> {
    let result = sqlx::query(
        r#"
        UPDATE job_status
        SET status = 'cancelled', completed_at = NOW()
        WHERE entity_type = $1 AND entity_id = $2 AND status IN ('pending', 'running')
        "#,
    )
    .bind(entity_type)
    .bind(entity_id)
    .execute(pool)
    .await?;
    Ok(result.rows_affected())
}

/// Delete all finished (non-pending, non-running) jobs.
pub async fn delete_finished<'e, E>(exec: E) -> Result<u64>
where
    E: Executor<'e, Database = Postgres>,
{
    let result = sqlx::query!("DELETE FROM job_status WHERE status NOT IN ('pending', 'running')")
        .execute(exec)
        .await?;
    Ok(result.rows_affected())
}
