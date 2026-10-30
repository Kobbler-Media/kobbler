use super::Result;
use sqlx::{Executor, Postgres};
use uuid::Uuid;

#[derive(Debug, Clone)]
pub struct OfflineExport {
    pub id: Uuid,
    pub media_type: String,
    pub file_id: Uuid,
    pub quality: String,
    pub audio_stream_index: i32,
    pub state: String,
    pub progress: f64,
    pub size_bytes: Option<i64>,
    pub error: Option<String>,
}

/// The export for this rendition, inserted as queued when there was none. The flag says
/// whether this call inserted it.
pub async fn find_or_create<'e, E>(
    exec: E,
    media_type: &str,
    file_id: Uuid,
    quality: &str,
    audio_stream_index: i32,
) -> Result<(OfflineExport, bool)>
where
    E: Executor<'e, Database = Postgres>,
{
    // xmax is 0 only on a row this statement inserted; the conflict branch updates it.
    let row = sqlx::query!(
        r#"
        INSERT INTO offline_exports (media_type, file_id, quality, audio_stream_index)
        VALUES ($1, $2, $3, $4)
        ON CONFLICT (file_id, quality, audio_stream_index)
        DO UPDATE SET last_accessed_at = NOW()
        RETURNING id, media_type, file_id, quality, audio_stream_index, state, progress,
                  size_bytes, error, (xmax = 0) AS "inserted!"
        "#,
        media_type,
        file_id,
        quality,
        audio_stream_index,
    )
    .fetch_one(exec)
    .await?;
    Ok((
        OfflineExport {
            id: row.id,
            media_type: row.media_type,
            file_id: row.file_id,
            quality: row.quality,
            audio_stream_index: row.audio_stream_index,
            state: row.state,
            progress: row.progress,
            size_bytes: row.size_bytes,
            error: row.error,
        },
        row.inserted,
    ))
}

pub async fn get<'e, E>(exec: E, id: Uuid) -> Result<Option<OfflineExport>>
where
    E: Executor<'e, Database = Postgres>,
{
    Ok(sqlx::query_as!(
        OfflineExport,
        "SELECT id, media_type, file_id, quality, audio_stream_index, state, progress, \
         size_bytes, error FROM offline_exports WHERE id = $1",
        id,
    )
    .fetch_optional(exec)
    .await?)
}

pub async fn touch<'e, E>(exec: E, id: Uuid) -> Result<Option<OfflineExport>>
where
    E: Executor<'e, Database = Postgres>,
{
    Ok(sqlx::query_as!(
        OfflineExport,
        "UPDATE offline_exports SET last_accessed_at = NOW() WHERE id = $1 \
         RETURNING id, media_type, file_id, quality, audio_stream_index, state, progress, \
         size_bytes, error",
        id,
    )
    .fetch_optional(exec)
    .await?)
}

/// Start the rendition over under a new id: a client part-way through downloading the old
/// file must not be handed different bytes under the id it already holds.
pub async fn requeue<'e, E>(exec: E, id: Uuid) -> Result<Option<OfflineExport>>
where
    E: Executor<'e, Database = Postgres>,
{
    Ok(sqlx::query_as!(
        OfflineExport,
        "UPDATE offline_exports \
         SET id = gen_random_uuid(), state = 'queued', progress = 0, size_bytes = NULL, \
             error = NULL, updated_at = NOW(), last_accessed_at = NOW() \
         WHERE id = $1 \
         RETURNING id, media_type, file_id, quality, audio_stream_index, state, progress, \
         size_bytes, error",
        id,
    )
    .fetch_optional(exec)
    .await?)
}

/// Bump a queued or running export nobody has moved for `quiet_minutes`, so its job can be
/// sent again; true when it was that stale.
pub async fn revive_if_quiet<'e, E>(exec: E, id: Uuid, quiet_minutes: i32) -> Result<bool>
where
    E: Executor<'e, Database = Postgres>,
{
    let revived = sqlx::query_scalar!(
        "UPDATE offline_exports SET updated_at = NOW() \
         WHERE id = $1 AND state IN ('queued', 'running') \
           AND updated_at < NOW() - make_interval(mins => $2) \
         RETURNING id",
        id,
        quiet_minutes,
    )
    .fetch_optional(exec)
    .await?;
    Ok(revived.is_some())
}

/// Take a queued export for producing. `running` is claimable too: that is a job
/// redelivered after the worker died under it.
pub async fn claim<'e, E>(exec: E, id: Uuid) -> Result<Option<OfflineExport>>
where
    E: Executor<'e, Database = Postgres>,
{
    Ok(sqlx::query_as!(
        OfflineExport,
        "UPDATE offline_exports \
         SET state = 'running', progress = 0, error = NULL, updated_at = NOW() \
         WHERE id = $1 AND state IN ('queued', 'running') \
         RETURNING id, media_type, file_id, quality, audio_stream_index, state, progress, \
         size_bytes, error",
        id,
    )
    .fetch_optional(exec)
    .await?)
}

pub async fn set_progress<'e, E>(exec: E, id: Uuid, progress: f64) -> Result<()>
where
    E: Executor<'e, Database = Postgres>,
{
    sqlx::query!(
        "UPDATE offline_exports SET progress = $2, updated_at = NOW() \
         WHERE id = $1 AND state = 'running'",
        id,
        progress,
    )
    .execute(exec)
    .await?;
    Ok(())
}

pub async fn mark_ready<'e, E>(exec: E, id: Uuid, size_bytes: i64) -> Result<()>
where
    E: Executor<'e, Database = Postgres>,
{
    sqlx::query!(
        "UPDATE offline_exports \
         SET state = 'ready', progress = 1, size_bytes = $2, error = NULL, updated_at = NOW() \
         WHERE id = $1",
        id,
        size_bytes,
    )
    .execute(exec)
    .await?;
    Ok(())
}

pub async fn mark_failed<'e, E>(exec: E, id: Uuid, error: &str) -> Result<()>
where
    E: Executor<'e, Database = Postgres>,
{
    sqlx::query!(
        "UPDATE offline_exports \
         SET state = 'failed', size_bytes = NULL, error = $2, updated_at = NOW() \
         WHERE id = $1",
        id,
        error,
    )
    .execute(exec)
    .await?;
    Ok(())
}

/// Delete exports nobody has asked for in `idle_days`, and failures older than
/// `failed_days`, returning their ids. One being produced is left to finish.
pub async fn delete_expired<'e, E>(exec: E, idle_days: i32, failed_days: i32) -> Result<Vec<Uuid>>
where
    E: Executor<'e, Database = Postgres>,
{
    Ok(sqlx::query_scalar!(
        "DELETE FROM offline_exports \
         WHERE state <> 'running' \
           AND (last_accessed_at < NOW() - make_interval(days => $1) \
                OR (state = 'failed' AND updated_at < NOW() - make_interval(days => $2))) \
         RETURNING id",
        idle_days,
        failed_days,
    )
    .fetch_all(exec)
    .await?)
}

pub async fn all_ids<'e, E>(exec: E) -> Result<Vec<Uuid>>
where
    E: Executor<'e, Database = Postgres>,
{
    Ok(sqlx::query_scalar!("SELECT id FROM offline_exports")
        .fetch_all(exec)
        .await?)
}
