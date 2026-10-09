use super::Result;
use sqlx::{Executor, Postgres};
use time::OffsetDateTime;
use uuid::Uuid;

#[derive(Debug, Clone)]
pub struct LeaseRow {
    pub id: Uuid,
    pub tuner_id: Uuid,
    pub channel_id: Uuid,
    pub tuner_index: Option<i32>,
    pub holder_kind: String,
    pub session_id: Option<String>,
    pub recording_id: Option<Uuid>,
    pub user_id: Option<Uuid>,
    pub buffer_dir: Option<String>,
    pub acquired_at: OffsetDateTime,
    pub expires_at: Option<OffsetDateTime>,
    pub released_at: Option<OffsetDateTime>,
}

#[derive(Debug)]
pub struct CreateLease {
    pub id: Uuid,
    pub tuner_id: Uuid,
    pub channel_id: Uuid,
    pub tuner_index: Option<i32>,
    pub holder_kind: String,
    pub session_id: Option<String>,
    pub recording_id: Option<Uuid>,
    pub user_id: Option<Uuid>,
    pub buffer_dir: Option<String>,
}

pub async fn create<'e, E>(exec: E, input: CreateLease) -> Result<LeaseRow>
where
    E: Executor<'e, Database = Postgres>,
{
    let r = sqlx::query!(
        r#"
        INSERT INTO livetv_tuner_leases
            (id, tuner_id, channel_id, tuner_index, holder_kind, session_id,
             recording_id, user_id, buffer_dir)
        VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9)
        RETURNING id, tuner_id, channel_id, tuner_index, holder_kind, session_id,
                  recording_id, user_id, buffer_dir, acquired_at, expires_at, released_at
        "#,
        input.id,
        input.tuner_id,
        input.channel_id,
        input.tuner_index,
        input.holder_kind,
        input.session_id,
        input.recording_id,
        input.user_id,
        input.buffer_dir,
    )
    .fetch_one(exec)
    .await?;
    Ok(map(
        r.id,
        r.tuner_id,
        r.channel_id,
        r.tuner_index,
        r.holder_kind,
        r.session_id,
        r.recording_id,
        r.user_id,
        r.buffer_dir,
        r.acquired_at,
        r.expires_at,
        r.released_at,
    ))
}

#[allow(clippy::too_many_arguments)]
fn map(
    id: Uuid,
    tuner_id: Uuid,
    channel_id: Uuid,
    tuner_index: Option<i32>,
    holder_kind: String,
    session_id: Option<String>,
    recording_id: Option<Uuid>,
    user_id: Option<Uuid>,
    buffer_dir: Option<String>,
    acquired_at: OffsetDateTime,
    expires_at: Option<OffsetDateTime>,
    released_at: Option<OffsetDateTime>,
) -> LeaseRow {
    LeaseRow {
        id,
        tuner_id,
        channel_id,
        tuner_index,
        holder_kind,
        session_id,
        recording_id,
        user_id,
        buffer_dir,
        acquired_at,
        expires_at,
        released_at,
    }
}

pub async fn release<'e, E>(exec: E, id: Uuid) -> Result<bool>
where
    E: Executor<'e, Database = Postgres>,
{
    let res = sqlx::query!(
        "UPDATE livetv_tuner_leases SET released_at = NOW() WHERE id = $1 AND released_at IS NULL",
        id
    )
    .execute(exec)
    .await?;
    Ok(res.rows_affected() > 0)
}

pub async fn release_all_active<'e, E>(exec: E) -> Result<u64>
where
    E: Executor<'e, Database = Postgres>,
{
    let res = sqlx::query!(
        "UPDATE livetv_tuner_leases SET released_at = NOW() WHERE released_at IS NULL"
    )
    .execute(exec)
    .await?;
    Ok(res.rows_affected())
}

pub async fn promote_to_recording<'e, E>(exec: E, id: Uuid, recording_id: Uuid) -> Result<()>
where
    E: Executor<'e, Database = Postgres>,
{
    sqlx::query!(
        "UPDATE livetv_tuner_leases SET holder_kind = 'recording', recording_id = $2 WHERE id = $1",
        id,
        recording_id
    )
    .execute(exec)
    .await?;
    Ok(())
}

pub async fn touch<'e, E>(exec: E, id: Uuid) -> Result<()>
where
    E: Executor<'e, Database = Postgres>,
{
    sqlx::query!(
        "UPDATE livetv_tuner_leases SET last_heartbeat = NOW() WHERE id = $1",
        id
    )
    .execute(exec)
    .await?;
    Ok(())
}

pub async fn list_active<'e, E>(exec: E) -> Result<Vec<LeaseRow>>
where
    E: Executor<'e, Database = Postgres>,
{
    let rows = sqlx::query!(
        r#"
        SELECT id, tuner_id, channel_id, tuner_index, holder_kind, session_id,
               recording_id, user_id, buffer_dir, acquired_at, expires_at, released_at
        FROM livetv_tuner_leases
        WHERE released_at IS NULL
        ORDER BY acquired_at
        "#,
    )
    .fetch_all(exec)
    .await?;
    Ok(rows
        .into_iter()
        .map(|r| {
            map(
                r.id,
                r.tuner_id,
                r.channel_id,
                r.tuner_index,
                r.holder_kind,
                r.session_id,
                r.recording_id,
                r.user_id,
                r.buffer_dir,
                r.acquired_at,
                r.expires_at,
                r.released_at,
            )
        })
        .collect())
}
