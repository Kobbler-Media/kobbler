//! Subtitle tracks repository operations (embedded container tracks).

use super::Result;
use sqlx::{Executor, Postgres};
use uuid::Uuid;

// =============================================================================
// Types
// =============================================================================

/// Input for creating a subtitle track.
#[derive(Debug)]
pub struct SubtitleTrackInput {
    pub movie_file_id: Option<Uuid>,
    pub episode_file_id: Option<Uuid>,
    pub track_index: i32,
    pub codec: String,
    pub language: Option<String>,
    pub title: Option<String>,
    pub is_forced: bool,
    pub is_default: bool,
    pub is_hearing_impaired: bool,
}

/// Row returned from subtitle_tracks queries.
#[derive(Debug, serde::Serialize)]
pub struct SubtitleTrackRow {
    pub id: Uuid,
    pub movie_file_id: Option<Uuid>,
    pub episode_file_id: Option<Uuid>,
    pub track_index: i32,
    pub codec: String,
    pub language: Option<String>,
    pub title: Option<String>,
    pub is_forced: bool,
    pub is_default: bool,
    pub is_hearing_impaired: bool,
}

// =============================================================================
// CRUD
// =============================================================================

/// Create a single subtitle track record.
pub async fn create<'e, E>(exec: E, input: &SubtitleTrackInput) -> Result<Uuid>
where
    E: Executor<'e, Database = Postgres>,
{
    let id = Uuid::now_v7();
    sqlx::query_scalar!(
        r#"
        INSERT INTO subtitle_tracks (id, movie_file_id, episode_file_id, track_index,
                                     codec, language, title,
                                     is_forced, is_default, is_hearing_impaired)
        VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10)
        RETURNING id
        "#,
        id,
        input.movie_file_id,
        input.episode_file_id,
        input.track_index,
        &input.codec,
        input.language.as_deref(),
        input.title.as_deref(),
        input.is_forced,
        input.is_default,
        input.is_hearing_impaired,
    )
    .fetch_one(exec)
    .await?;

    Ok(id)
}

/// Batch-insert subtitle tracks.
pub async fn create_batch(pool: &sqlx::PgPool, inputs: &[SubtitleTrackInput]) -> Result<Vec<Uuid>> {
    let mut tx = pool.begin().await?;
    let mut ids = Vec::with_capacity(inputs.len());
    for input in inputs {
        let id = create(&mut *tx, input).await?;
        ids.push(id);
    }
    tx.commit().await?;
    Ok(ids)
}

/// List subtitle tracks for a movie file.
pub async fn list_by_movie_file<'e, E>(
    exec: E,
    movie_file_id: Uuid,
) -> Result<Vec<SubtitleTrackRow>>
where
    E: Executor<'e, Database = Postgres>,
{
    let rows = sqlx::query!(
        r#"
        SELECT id, movie_file_id, episode_file_id, track_index,
               codec, language, title, is_forced, is_default, is_hearing_impaired
        FROM subtitle_tracks
        WHERE movie_file_id = $1
        ORDER BY track_index
        "#,
        movie_file_id,
    )
    .fetch_all(exec)
    .await?;

    Ok(rows
        .into_iter()
        .map(|r| SubtitleTrackRow {
            id: r.id,
            movie_file_id: r.movie_file_id,
            episode_file_id: r.episode_file_id,
            track_index: r.track_index,
            codec: r.codec,
            language: r.language,
            title: r.title,
            is_forced: r.is_forced,
            is_default: r.is_default,
            is_hearing_impaired: r.is_hearing_impaired,
        })
        .collect())
}

/// List subtitle tracks for an episode file.
pub async fn list_by_episode_file<'e, E>(
    exec: E,
    episode_file_id: Uuid,
) -> Result<Vec<SubtitleTrackRow>>
where
    E: Executor<'e, Database = Postgres>,
{
    let rows = sqlx::query!(
        r#"
        SELECT id, movie_file_id, episode_file_id, track_index,
               codec, language, title, is_forced, is_default, is_hearing_impaired
        FROM subtitle_tracks
        WHERE episode_file_id = $1
        ORDER BY track_index
        "#,
        episode_file_id,
    )
    .fetch_all(exec)
    .await?;

    Ok(rows
        .into_iter()
        .map(|r| SubtitleTrackRow {
            id: r.id,
            movie_file_id: r.movie_file_id,
            episode_file_id: r.episode_file_id,
            track_index: r.track_index,
            codec: r.codec,
            language: r.language,
            title: r.title,
            is_forced: r.is_forced,
            is_default: r.is_default,
            is_hearing_impaired: r.is_hearing_impaired,
        })
        .collect())
}

/// Find forced subtitle tracks, optionally filtering by language.
pub async fn find_forced<'e, E>(
    exec: E,
    movie_file_id: Option<Uuid>,
    episode_file_id: Option<Uuid>,
    language: Option<&str>,
) -> Result<Vec<SubtitleTrackRow>>
where
    E: Executor<'e, Database = Postgres>,
{
    let rows = sqlx::query!(
        r#"
        SELECT id, movie_file_id, episode_file_id, track_index,
               codec, language, title, is_forced, is_default, is_hearing_impaired
        FROM subtitle_tracks
        WHERE is_forced = TRUE
          AND ($1::uuid IS NULL OR movie_file_id = $1)
          AND ($2::uuid IS NULL OR episode_file_id = $2)
          AND ($3::text IS NULL OR language = $3)
        ORDER BY track_index
        "#,
        movie_file_id,
        episode_file_id,
        language,
    )
    .fetch_all(exec)
    .await?;

    Ok(rows
        .into_iter()
        .map(|r| SubtitleTrackRow {
            id: r.id,
            movie_file_id: r.movie_file_id,
            episode_file_id: r.episode_file_id,
            track_index: r.track_index,
            codec: r.codec,
            language: r.language,
            title: r.title,
            is_forced: r.is_forced,
            is_default: r.is_default,
            is_hearing_impaired: r.is_hearing_impaired,
        })
        .collect())
}

/// Delete all subtitle tracks for a movie file.
pub async fn delete_by_movie_file<'e, E>(exec: E, movie_file_id: Uuid) -> Result<u64>
where
    E: Executor<'e, Database = Postgres>,
{
    let result = sqlx::query!(
        "DELETE FROM subtitle_tracks WHERE movie_file_id = $1",
        movie_file_id,
    )
    .execute(exec)
    .await?;
    Ok(result.rows_affected())
}

/// Delete all subtitle tracks for an episode file.
pub async fn delete_by_episode_file<'e, E>(exec: E, episode_file_id: Uuid) -> Result<u64>
where
    E: Executor<'e, Database = Postgres>,
{
    let result = sqlx::query!(
        "DELETE FROM subtitle_tracks WHERE episode_file_id = $1",
        episode_file_id,
    )
    .execute(exec)
    .await?;
    Ok(result.rows_affected())
}
