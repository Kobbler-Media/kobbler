use super::Result;
use crate::streaming::loudness::TrackLoudness;
use sqlx::types::Json;
use sqlx::{Executor, Postgres};
use uuid::Uuid;

#[derive(Debug, Clone)]
pub struct SessionRelease {
    pub release_key: Option<String>,
    pub file_id: Option<i32>,
    pub debrid_provider: String,
    pub release_title: String,
    pub filename: String,
}

#[derive(Debug, Clone)]
pub struct PlayedSession {
    pub first: bool,
    pub user_id: Uuid,
    pub media_type: String,
    pub media_id: Uuid,
    pub release_key: Option<String>,
    pub file_id: Option<i32>,
    pub debrid_provider: String,
    pub release_title: String,
    pub filename: String,
}

#[derive(Debug, Clone)]
pub struct PlayedRelease {
    pub release_key: String,
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
        SELECT release_key, file_id, debrid_provider, release_title, filename
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

/// The key kobload gave an NZB the last time a session of it resolved.
pub async fn release_key_of_nzb<'e, E>(exec: E, nzb: &nats_jobs::NzbRef) -> Result<Option<String>>
where
    E: Executor<'e, Database = Postgres>,
{
    let key = sqlx::query_scalar!(
        r#"
        SELECT release_key AS "release_key!"
        FROM debrid_stream_sessions
        WHERE nzb_ref = $1 AND release_key IS NOT NULL
        ORDER BY created_at DESC
        LIMIT 1
        "#,
        Json(nzb) as _,
    )
    .fetch_optional(exec)
    .await?;
    Ok(key)
}

/// The file of the release an earlier session of this title played, so a pack's episode
/// finds its own file again.
pub async fn file_played<'e, E>(
    exec: E,
    release_key: &str,
    media_type: &str,
    media_id: Uuid,
) -> Result<Option<i32>>
where
    E: Executor<'e, Database = Postgres>,
{
    let file_id = sqlx::query_scalar!(
        r#"
        SELECT file_id AS "file_id!"
        FROM debrid_stream_sessions
        WHERE release_key = $1 AND media_type = $2 AND media_id = $3 AND file_id IS NOT NULL
        ORDER BY created_at DESC
        LIMIT 1
        "#,
        release_key,
        media_type,
        media_id,
    )
    .fetch_optional(exec)
    .await?;
    Ok(file_id)
}

/// Stamps every report, so `played_at` orders a title's releases by when each was last
/// watched. `None` when the session is unknown.
pub async fn mark_played<'e, E>(exec: E, session_id: Uuid) -> Result<Option<PlayedSession>>
where
    E: Executor<'e, Database = Postgres>,
{
    let row = sqlx::query_as!(
        PlayedSession,
        r#"
        UPDATE debrid_stream_sessions s
        SET played_at = NOW()
        FROM (SELECT id, played_at FROM debrid_stream_sessions WHERE id = $1 FOR UPDATE) prior
        WHERE s.id = prior.id
        RETURNING prior.played_at IS NULL AS "first!", s.user_id, s.media_type, s.media_id,
                  s.release_key, s.file_id, s.debrid_provider, s.release_title, s.filename
        "#,
        session_id,
    )
    .fetch_optional(exec)
    .await?;
    Ok(row)
}

/// Played releases of one title, the one watched last first.
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
        SELECT release_key AS "release_key!", file_id, release_title, cache_path
        FROM debrid_stream_sessions
        WHERE media_type = $1 AND media_id = $2
          AND release_key IS NOT NULL AND played_at IS NOT NULL
        ORDER BY played_at DESC
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
        SELECT s.release_key AS "release_key!", s.file_id, s.release_title, s.cache_path
        FROM debrid_stream_sessions s
        JOIN tv_episodes e ON e.id = s.media_id
        JOIN tv_seasons se ON se.id = e.season_id
        WHERE s.media_type = 'episode' AND s.user_id = $1 AND s.media_id <> $2
          AND s.release_key IS NOT NULL AND s.played_at IS NOT NULL
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

/// Whether the row was there to take it, as with [`set_subtitle_tracks`].
pub async fn set_picture<'e, E>(
    exec: E,
    session_id: Uuid,
    picture: &serde_json::Value,
) -> Result<bool>
where
    E: Executor<'e, Database = Postgres>,
{
    let done = sqlx::query!(
        "UPDATE debrid_stream_sessions SET picture = $1, updated_at = NOW() WHERE id = $2",
        picture,
        session_id,
    )
    .execute(exec)
    .await?;
    Ok(done.rows_affected() > 0)
}

pub async fn picture<'e, E>(exec: E, session_id: Uuid) -> Result<Option<serde_json::Value>>
where
    E: Executor<'e, Database = Postgres>,
{
    let picture = sqlx::query_scalar!(
        "SELECT picture FROM debrid_stream_sessions WHERE id = $1",
        session_id,
    )
    .fetch_optional(exec)
    .await?;
    Ok(picture.flatten())
}

/// The picture area any earlier session measured for this release.
pub async fn release_picture<'e, E>(
    exec: E,
    release_key: &str,
    file_id: Option<i32>,
) -> Result<Option<serde_json::Value>>
where
    E: Executor<'e, Database = Postgres>,
{
    let picture = sqlx::query_scalar!(
        r#"
        SELECT picture AS "picture!"
        FROM debrid_stream_sessions
        WHERE release_key = $1 AND file_id IS NOT DISTINCT FROM $2 AND picture IS NOT NULL
        ORDER BY updated_at DESC
        LIMIT 1
        "#,
        release_key,
        file_id,
    )
    .fetch_optional(exec)
    .await?;
    Ok(picture)
}

#[derive(Debug, Clone, Default)]
pub struct ReleaseAnalysis {
    pub audio_loudness: Option<Json<Vec<TrackLoudness>>>,
    pub picture: Option<serde_json::Value>,
}

/// Loudness and bars are measured on one file, so only an earlier session of this same
/// release can lend them to a new one.
pub async fn release_analysis<'e, E>(
    exec: E,
    media_type: &str,
    media_id: Uuid,
    release_key: Option<&str>,
    file_id: Option<i32>,
    cache_path: Option<&str>,
) -> Result<ReleaseAnalysis>
where
    E: Executor<'e, Database = Postgres>,
{
    let row = sqlx::query_as!(
        ReleaseAnalysis,
        r#"
        SELECT audio_loudness AS "audio_loudness: Json<Vec<TrackLoudness>>", picture
        FROM debrid_stream_sessions
        WHERE media_type = $1 AND media_id = $2
          AND ((release_key = $3 AND file_id IS NOT DISTINCT FROM $4) OR cache_path = $5)
          AND (audio_loudness IS NOT NULL OR picture IS NOT NULL)
        ORDER BY updated_at DESC
        LIMIT 1
        "#,
        media_type,
        media_id,
        release_key,
        file_id,
        cache_path,
    )
    .fetch_optional(exec)
    .await?;
    Ok(row.unwrap_or_default())
}
