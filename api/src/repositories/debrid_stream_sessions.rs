use super::Result;
use sqlx::{Executor, Postgres};
use uuid::Uuid;

#[derive(Debug, Clone)]
pub struct SessionRelease {
    pub info_hash: Option<String>,
    pub file_id: Option<i32>,
    pub debrid_provider: String,
    pub release_title: String,
    pub filename: String,
}

#[derive(Debug, Clone)]
pub struct PlayedSession {
    pub user_id: Uuid,
    pub media_type: String,
    pub media_id: Uuid,
    pub info_hash: Option<String>,
    pub file_id: Option<i32>,
    pub debrid_provider: String,
    pub release_title: String,
    pub filename: String,
}

#[derive(Debug, Clone)]
pub struct PlayedRelease {
    pub info_hash: String,
    pub file_id: Option<i32>,
    pub release_title: String,
    pub cache_path: Option<String>,
}

pub async fn release<'e, E>(
    exec: E,
    session_id: Uuid,
    owner: Option<Uuid>,
) -> Result<Option<SessionRelease>>
where
    E: Executor<'e, Database = Postgres>,
{
    let row = sqlx::query_as!(
        SessionRelease,
        r#"
        SELECT info_hash, file_id, debrid_provider, release_title, filename
        FROM debrid_stream_sessions
        WHERE id = $1 AND ($2::uuid IS NULL OR user_id = $2)
        "#,
        session_id,
        owner,
    )
    .fetch_optional(exec)
    .await?;
    Ok(row)
}

/// `None` when the session is unknown or had already played.
pub async fn mark_played<'e, E>(exec: E, session_id: Uuid) -> Result<Option<PlayedSession>>
where
    E: Executor<'e, Database = Postgres>,
{
    let row = sqlx::query_as!(
        PlayedSession,
        r#"
        UPDATE debrid_stream_sessions
        SET played_at = NOW()
        WHERE id = $1 AND played_at IS NULL
        RETURNING user_id, media_type, media_id, info_hash, file_id, debrid_provider,
                  release_title, filename
        "#,
        session_id,
    )
    .fetch_optional(exec)
    .await?;
    Ok(row)
}

/// Played releases of one title, local cache first, then most recently played.
pub async fn played_for_title<'e, E>(
    exec: E,
    media_type: &str,
    media_id: Uuid,
) -> Result<Vec<PlayedRelease>>
where
    E: Executor<'e, Database = Postgres>,
{
    let rows = sqlx::query_as!(
        PlayedRelease,
        r#"
        SELECT info_hash AS "info_hash!", file_id, release_title, cache_path
        FROM debrid_stream_sessions
        WHERE media_type = $1 AND media_id = $2
          AND info_hash IS NOT NULL AND played_at IS NOT NULL
        ORDER BY CASE WHEN cache_path IS NOT NULL THEN 0 ELSE 1 END, played_at DESC
        LIMIT 20
        "#,
        media_type,
        media_id,
    )
    .fetch_all(exec)
    .await?;
    Ok(rows)
}

/// What this user last played from another episode of the same show.
pub async fn last_played_in_show<'e, E>(
    exec: E,
    user_id: Uuid,
    episode_id: Uuid,
) -> Result<Option<PlayedRelease>>
where
    E: Executor<'e, Database = Postgres>,
{
    let row = sqlx::query_as!(
        PlayedRelease,
        r#"
        SELECT s.info_hash AS "info_hash!", s.file_id, s.release_title, s.cache_path
        FROM debrid_stream_sessions s
        JOIN tv_episodes e ON e.id = s.media_id
        JOIN tv_seasons se ON se.id = e.season_id
        WHERE s.media_type = 'episode' AND s.user_id = $1 AND s.media_id <> $2
          AND s.info_hash IS NOT NULL AND s.played_at IS NOT NULL
          AND se.show_id = (
              SELECT ts.show_id FROM tv_episodes te
              JOIN tv_seasons ts ON ts.id = te.season_id
              WHERE te.id = $2
          )
        ORDER BY s.played_at DESC
        LIMIT 1
        "#,
        user_id,
        episode_id,
    )
    .fetch_optional(exec)
    .await?;
    Ok(row)
}

/// Whether the row was there to take them: the session's row can be written a moment after
/// the stream it describes has started.
pub async fn set_subtitle_tracks<'e, E>(
    exec: E,
    session_id: Uuid,
    tracks: &serde_json::Value,
) -> Result<bool>
where
    E: Executor<'e, Database = Postgres>,
{
    let done = sqlx::query!(
        "UPDATE debrid_stream_sessions SET subtitle_tracks = $1, updated_at = NOW() WHERE id = $2",
        tracks,
        session_id,
    )
    .execute(exec)
    .await?;
    Ok(done.rows_affected() > 0)
}
