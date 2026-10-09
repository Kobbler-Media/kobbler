//! IPTV recordings repository operations.

use super::Result;
use sqlx::{Executor, Postgres};
use time::OffsetDateTime;
use uuid::Uuid;

// =============================================================================
// Types
// =============================================================================

#[derive(Debug)]
pub struct RecordingRow {
    pub id: Uuid,
    pub channel_id: Uuid,
    pub epg_id: Option<Uuid>,
    pub title: String,
    pub status: String,
    pub start_time: OffsetDateTime,
    pub end_time: OffsetDateTime,
    pub pre_padding_secs: i32,
    pub post_padding_secs: i32,
    pub file_path: Option<String>,
    pub file_size_bytes: Option<i64>,
    pub error_message: Option<String>,
    pub created_at: OffsetDateTime,
    pub duration_seconds: Option<f64>,
    pub video_codec: Option<String>,
    pub container: Option<String>,
}

#[derive(Debug)]
pub struct CreateRecording {
    pub channel_id: Uuid,
    pub epg_id: Option<Uuid>,
    pub title: String,
    pub start_time: OffsetDateTime,
    pub end_time: OffsetDateTime,
    pub pre_padding_secs: Option<i32>,
    pub post_padding_secs: Option<i32>,
}

// =============================================================================
// Operations
// =============================================================================

/// Create a new recording.
pub async fn create<'e, E>(exec: E, input: CreateRecording) -> Result<RecordingRow>
where
    E: Executor<'e, Database = Postgres>,
{
    let id = Uuid::now_v7();
    let r = sqlx::query!(
        r#"
        INSERT INTO iptv_recordings (id, channel_id, epg_id, title, start_time, end_time,
                                     pre_padding_secs, post_padding_secs)
        VALUES ($1, $2, $3, $4, $5, $6, COALESCE($7, 60), COALESCE($8, 120))
        RETURNING id, channel_id, epg_id, title, status, start_time, end_time,
                  pre_padding_secs, post_padding_secs, file_path, file_size_bytes,
                  error_message, created_at, duration_seconds, video_codec, container
        "#,
        id,
        input.channel_id,
        input.epg_id,
        input.title,
        input.start_time,
        input.end_time,
        input.pre_padding_secs,
        input.post_padding_secs,
    )
    .fetch_one(exec)
    .await?;

    Ok(RecordingRow {
        id: r.id,
        channel_id: r.channel_id,
        epg_id: r.epg_id,
        title: r.title,
        status: r.status,
        start_time: r.start_time,
        end_time: r.end_time,
        pre_padding_secs: r.pre_padding_secs,
        post_padding_secs: r.post_padding_secs,
        file_path: r.file_path,
        file_size_bytes: r.file_size_bytes,
        error_message: r.error_message,
        created_at: r.created_at,
        duration_seconds: r.duration_seconds,
        video_codec: r.video_codec,
        container: r.container,
    })
}

/// Get a recording by ID.
pub async fn get_by_id<'e, E>(exec: E, id: Uuid) -> Result<Option<RecordingRow>>
where
    E: Executor<'e, Database = Postgres>,
{
    let row = sqlx::query!(
        r#"
        SELECT id, channel_id, epg_id, title, status, start_time, end_time,
               pre_padding_secs, post_padding_secs, file_path, file_size_bytes,
               error_message, created_at, duration_seconds, video_codec, container
        FROM iptv_recordings
        WHERE id = $1
        "#,
        id,
    )
    .fetch_optional(exec)
    .await?;

    Ok(row.map(|r| RecordingRow {
        id: r.id,
        channel_id: r.channel_id,
        epg_id: r.epg_id,
        title: r.title,
        status: r.status,
        start_time: r.start_time,
        end_time: r.end_time,
        pre_padding_secs: r.pre_padding_secs,
        post_padding_secs: r.post_padding_secs,
        file_path: r.file_path,
        file_size_bytes: r.file_size_bytes,
        error_message: r.error_message,
        created_at: r.created_at,
        duration_seconds: r.duration_seconds,
        video_codec: r.video_codec,
        container: r.container,
    }))
}

/// List recordings filtered by status (pass None to list all).
pub async fn list_with_filters<'e, E>(exec: E, status: Option<&str>) -> Result<Vec<RecordingRow>>
where
    E: Executor<'e, Database = Postgres>,
{
    let rows = sqlx::query!(
        r#"
        SELECT id, channel_id, epg_id, title, status, start_time, end_time,
               pre_padding_secs, post_padding_secs, file_path, file_size_bytes,
               error_message, created_at, duration_seconds, video_codec, container
        FROM iptv_recordings
        WHERE ($1::text IS NULL OR status = $1)
        ORDER BY start_time DESC
        "#,
        status,
    )
    .fetch_all(exec)
    .await?;

    Ok(rows
        .into_iter()
        .map(|r| RecordingRow {
            id: r.id,
            channel_id: r.channel_id,
            epg_id: r.epg_id,
            title: r.title,
            status: r.status,
            start_time: r.start_time,
            end_time: r.end_time,
            pre_padding_secs: r.pre_padding_secs,
            post_padding_secs: r.post_padding_secs,
            file_path: r.file_path,
            file_size_bytes: r.file_size_bytes,
            error_message: r.error_message,
            created_at: r.created_at,
            duration_seconds: r.duration_seconds,
            video_codec: r.video_codec,
            container: r.container,
        })
        .collect())
}

/// List recordings due to start within `lookahead` of `now` (scheduled, and
/// `start_time - pre_padding - lookahead <= now`). The lookahead lets the
/// scheduler pick up a recording slightly before its capture-start so the
/// handler can wait for the exact moment instead of starting up to one
/// poll-interval late.
pub async fn list_due<'e, E>(
    exec: E,
    now: OffsetDateTime,
    lookahead: std::time::Duration,
) -> Result<Vec<RecordingRow>>
where
    E: Executor<'e, Database = Postgres>,
{
    let lookahead_secs = lookahead.as_secs_f64();
    let rows = sqlx::query!(
        r#"
        SELECT id, channel_id, epg_id, title, status, start_time, end_time,
               pre_padding_secs, post_padding_secs, file_path, file_size_bytes,
               error_message, created_at, duration_seconds, video_codec, container
        FROM iptv_recordings
        WHERE status = 'scheduled'
          AND start_time
              - make_interval(secs => pre_padding_secs::double precision)
              - make_interval(secs => $2::double precision) <= $1
        ORDER BY start_time ASC
        "#,
        now,
        lookahead_secs,
    )
    .fetch_all(exec)
    .await?;

    Ok(rows
        .into_iter()
        .map(|r| RecordingRow {
            id: r.id,
            channel_id: r.channel_id,
            epg_id: r.epg_id,
            title: r.title,
            status: r.status,
            start_time: r.start_time,
            end_time: r.end_time,
            pre_padding_secs: r.pre_padding_secs,
            post_padding_secs: r.post_padding_secs,
            file_path: r.file_path,
            file_size_bytes: r.file_size_bytes,
            error_message: r.error_message,
            created_at: r.created_at,
            duration_seconds: r.duration_seconds,
            video_codec: r.video_codec,
            container: r.container,
        })
        .collect())
}

/// Update the status of a recording.
pub async fn update_status<'e, E>(
    exec: E,
    id: Uuid,
    status: &str,
    file_path: Option<&str>,
    file_size_bytes: Option<i64>,
    error_message: Option<&str>,
) -> Result<bool>
where
    E: Executor<'e, Database = Postgres>,
{
    let result = sqlx::query!(
        r#"
        UPDATE iptv_recordings SET
            status = $2,
            file_path = COALESCE($3, file_path),
            file_size_bytes = COALESCE($4, file_size_bytes),
            error_message = $5
        WHERE id = $1
        "#,
        id,
        status,
        file_path,
        file_size_bytes,
        error_message,
    )
    .execute(exec)
    .await?;

    Ok(result.rows_affected() > 0)
}

/// Atomically transition a recording from `scheduled` to `recording`. Returns
/// true if this call won the claim, false if it was already claimed (a worker
/// running a duplicate enqueue from the lookahead window). Losers must stop —
/// the winner owns the capture.
pub async fn try_claim_for_recording<'e, E>(exec: E, id: Uuid) -> Result<bool>
where
    E: Executor<'e, Database = Postgres>,
{
    let result = sqlx::query!(
        r#"
        UPDATE iptv_recordings SET status = 'recording'
        WHERE id = $1 AND status = 'scheduled'
        "#,
        id,
    )
    .execute(exec)
    .await?;

    Ok(result.rows_affected() > 0)
}

/// Update media metadata after recording finishes (ffprobe results).
pub async fn update_media_metadata<'e, E>(
    exec: E,
    id: Uuid,
    duration_seconds: Option<f64>,
    video_codec: Option<&str>,
    container: Option<&str>,
) -> Result<bool>
where
    E: Executor<'e, Database = Postgres>,
{
    let result = sqlx::query!(
        r#"
        UPDATE iptv_recordings SET
            duration_seconds = COALESCE($2, duration_seconds),
            video_codec = COALESCE($3, video_codec),
            container = COALESCE($4, container)
        WHERE id = $1
        "#,
        id,
        duration_seconds,
        video_codec,
        container,
    )
    .execute(exec)
    .await?;

    Ok(result.rows_affected() > 0)
}

/// Get the configured recording path from iptv_config, if any.
pub async fn get_recording_path<'e, E>(exec: E) -> Result<Option<String>>
where
    E: Executor<'e, Database = Postgres>,
{
    let row = sqlx::query_scalar!(r#"SELECT recording_path FROM iptv_config LIMIT 1"#,)
        .fetch_optional(exec)
        .await?;

    Ok(row.flatten())
}

/// Delete a recording by ID.
pub async fn delete<'e, E>(exec: E, id: Uuid) -> Result<bool>
where
    E: Executor<'e, Database = Postgres>,
{
    let result = sqlx::query!(
        r#"
        DELETE FROM iptv_recordings WHERE id = $1
        "#,
        id,
    )
    .execute(exec)
    .await?;

    Ok(result.rows_affected() > 0)
}
