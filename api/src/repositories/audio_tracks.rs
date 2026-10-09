//! Audio tracks repository operations.

use super::Result;
use sqlx::{Executor, Postgres};
use uuid::Uuid;

// =============================================================================
// Types
// =============================================================================

/// Input for creating an audio track.
#[derive(Debug)]
pub struct AudioTrackInput {
    pub movie_file_id: Option<Uuid>,
    pub episode_file_id: Option<Uuid>,
    pub track_index: i32,
    pub codec: String,
    pub channels: i16,
    pub bitrate_kbps: Option<i32>,
    pub sample_rate: Option<i32>,
    pub language: Option<String>,
    pub title: Option<String>,
    pub is_default: bool,
}

/// Row returned from audio_tracks queries.
#[derive(Debug, serde::Serialize)]
pub struct AudioTrackRow {
    pub id: Uuid,
    pub movie_file_id: Option<Uuid>,
    pub episode_file_id: Option<Uuid>,
    pub track_index: i32,
    pub codec: String,
    pub channels: i16,
    pub bitrate_kbps: Option<i32>,
    pub sample_rate: Option<i32>,
    pub language: Option<String>,
    pub title: Option<String>,
    pub is_default: bool,
}

// =============================================================================
// CRUD
// =============================================================================

/// Create a single audio track record.
pub async fn create<'e, E>(exec: E, input: &AudioTrackInput) -> Result<Uuid>
where
    E: Executor<'e, Database = Postgres>,
{
    let id = Uuid::now_v7();
    sqlx::query_scalar!(
        r#"
        INSERT INTO audio_tracks (id, movie_file_id, episode_file_id, track_index,
                                  codec, channels, bitrate_kbps, sample_rate,
                                  language, title, is_default)
        VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11)
        RETURNING id
        "#,
        id,
        input.movie_file_id,
        input.episode_file_id,
        input.track_index,
        &input.codec,
        input.channels,
        input.bitrate_kbps,
        input.sample_rate,
        input.language.as_deref(),
        input.title.as_deref(),
        input.is_default,
    )
    .fetch_one(exec)
    .await?;

    Ok(id)
}

/// Batch-insert audio tracks (uses a transaction internally).
pub async fn create_batch(pool: &sqlx::PgPool, inputs: &[AudioTrackInput]) -> Result<Vec<Uuid>> {
    let mut tx = pool.begin().await?;
    let mut ids = Vec::with_capacity(inputs.len());
    for input in inputs {
        let id = create(&mut *tx, input).await?;
        ids.push(id);
    }
    tx.commit().await?;
    Ok(ids)
}

/// List audio tracks for a movie file.
pub async fn list_by_movie_file<'e, E>(exec: E, movie_file_id: Uuid) -> Result<Vec<AudioTrackRow>>
where
    E: Executor<'e, Database = Postgres>,
{
    let rows = sqlx::query!(
        r#"
        SELECT id, movie_file_id, episode_file_id, track_index,
               codec, channels, bitrate_kbps, sample_rate,
               language, title, is_default
        FROM audio_tracks
        WHERE movie_file_id = $1
        ORDER BY track_index
        "#,
        movie_file_id,
    )
    .fetch_all(exec)
    .await?;

    Ok(rows
        .into_iter()
        .map(|r| AudioTrackRow {
            id: r.id,
            movie_file_id: r.movie_file_id,
            episode_file_id: r.episode_file_id,
            track_index: r.track_index,
            codec: r.codec,
            channels: r.channels,
            bitrate_kbps: r.bitrate_kbps,
            sample_rate: r.sample_rate,
            language: r.language,
            title: r.title,
            is_default: r.is_default,
        })
        .collect())
}

/// List audio tracks for an episode file.
pub async fn list_by_episode_file<'e, E>(
    exec: E,
    episode_file_id: Uuid,
) -> Result<Vec<AudioTrackRow>>
where
    E: Executor<'e, Database = Postgres>,
{
    let rows = sqlx::query!(
        r#"
        SELECT id, movie_file_id, episode_file_id, track_index,
               codec, channels, bitrate_kbps, sample_rate,
               language, title, is_default
        FROM audio_tracks
        WHERE episode_file_id = $1
        ORDER BY track_index
        "#,
        episode_file_id,
    )
    .fetch_all(exec)
    .await?;

    Ok(rows
        .into_iter()
        .map(|r| AudioTrackRow {
            id: r.id,
            movie_file_id: r.movie_file_id,
            episode_file_id: r.episode_file_id,
            track_index: r.track_index,
            codec: r.codec,
            channels: r.channels,
            bitrate_kbps: r.bitrate_kbps,
            sample_rate: r.sample_rate,
            language: r.language,
            title: r.title,
            is_default: r.is_default,
        })
        .collect())
}

/// Delete all audio tracks for a movie file.
pub async fn delete_by_movie_file<'e, E>(exec: E, movie_file_id: Uuid) -> Result<u64>
where
    E: Executor<'e, Database = Postgres>,
{
    let result = sqlx::query!(
        "DELETE FROM audio_tracks WHERE movie_file_id = $1",
        movie_file_id,
    )
    .execute(exec)
    .await?;
    Ok(result.rows_affected())
}

/// Delete all audio tracks for an episode file.
pub async fn delete_by_episode_file<'e, E>(exec: E, episode_file_id: Uuid) -> Result<u64>
where
    E: Executor<'e, Database = Postgres>,
{
    let result = sqlx::query!(
        "DELETE FROM audio_tracks WHERE episode_file_id = $1",
        episode_file_id,
    )
    .execute(exec)
    .await?;
    Ok(result.rows_affected())
}
