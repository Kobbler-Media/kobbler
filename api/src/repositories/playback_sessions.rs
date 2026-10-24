use super::Result;
use sqlx::{Executor, Postgres};
use uuid::Uuid;

#[derive(Debug, Clone)]
pub struct StoredSession {
    pub user_id: Uuid,
    pub request: serde_json::Value,
    pub progress: Option<serde_json::Value>,
}

#[derive(Debug, Clone)]
pub struct ExpiredSession {
    pub id: Uuid,
    pub dir: String,
}

pub async fn insert<'e, E>(
    exec: E,
    id: Uuid,
    user_id: Uuid,
    dir: &str,
    request: &serde_json::Value,
) -> Result<()>
where
    E: Executor<'e, Database = Postgres>,
{
    sqlx::query!(
        "INSERT INTO playback_sessions (id, user_id, dir, request) VALUES ($1, $2, $3, $4)",
        id,
        user_id,
        dir,
        request,
    )
    .execute(exec)
    .await?;
    Ok(())
}

pub async fn get<'e, E>(exec: E, id: Uuid) -> Result<Option<StoredSession>>
where
    E: Executor<'e, Database = Postgres>,
{
    let row = sqlx::query_as!(
        StoredSession,
        "SELECT user_id, request, progress FROM playback_sessions WHERE id = $1",
        id,
    )
    .fetch_optional(exec)
    .await?;
    Ok(row)
}

/// Record how far a session has got, and when it was last used — `idle_secs` before now.
pub async fn save_progress<'e, E>(
    exec: E,
    id: Uuid,
    progress: &serde_json::Value,
    idle_secs: f64,
) -> Result<()>
where
    E: Executor<'e, Database = Postgres>,
{
    sqlx::query!(
        "UPDATE playback_sessions \
         SET progress = $2, last_activity_at = NOW() - make_interval(secs => $3) \
         WHERE id = $1",
        id,
        progress,
        idle_secs,
    )
    .execute(exec)
    .await?;
    Ok(())
}

/// Delete the row, returning the session directory it named.
pub async fn delete<'e, E>(exec: E, id: Uuid) -> Result<Option<String>>
where
    E: Executor<'e, Database = Postgres>,
{
    let dir = sqlx::query_scalar!(
        "DELETE FROM playback_sessions WHERE id = $1 RETURNING dir",
        id
    )
    .fetch_optional(exec)
    .await?;
    Ok(dir)
}

pub async fn take_idle_since<'e, E>(exec: E, idle_secs: f64) -> Result<Vec<ExpiredSession>>
where
    E: Executor<'e, Database = Postgres>,
{
    let rows = sqlx::query_as!(
        ExpiredSession,
        "DELETE FROM playback_sessions \
         WHERE last_activity_at < NOW() - make_interval(secs => $1) \
         RETURNING id, dir",
        idle_secs,
    )
    .fetch_all(exec)
    .await?;
    Ok(rows)
}
