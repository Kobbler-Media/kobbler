//! External subtitles repository operations (sidecar files).

use super::Result;
use serde_json::Value;
use sqlx::{Executor, Postgres};
use time::OffsetDateTime;
use uuid::Uuid;

// =============================================================================
// Types
// =============================================================================

/// Input for creating an external subtitle record.
#[derive(Debug)]
pub struct ExternalSubtitleInput {
    pub movie_file_id: Option<Uuid>,
    pub episode_file_id: Option<Uuid>,
    pub file_path: String,
    pub file_size: Option<i64>,
    pub format: String,
    pub language: Option<String>,
    pub title: Option<String>,
    pub is_forced: bool,
    pub is_hearing_impaired: bool,
    pub source: String,
    pub source_id: Option<String>,
    pub source_metadata: Option<Value>,
}

/// Row returned from external_subtitles queries.
#[derive(Debug, serde::Serialize)]
pub struct ExternalSubtitleRow {
    pub id: Uuid,
    pub movie_file_id: Option<Uuid>,
    pub episode_file_id: Option<Uuid>,
    pub file_path: String,
    pub file_size: Option<i64>,
    pub format: String,
    pub language: Option<String>,
    pub title: Option<String>,
    pub is_forced: bool,
    pub is_hearing_impaired: bool,
    pub source: String,
    pub source_id: Option<String>,
    pub source_metadata: Option<Value>,
    pub available: bool,
    pub created_at: OffsetDateTime,
}

// =============================================================================
// CRUD
// =============================================================================

/// Create a new external subtitle record.
pub async fn create<'e, E>(exec: E, input: &ExternalSubtitleInput) -> Result<Uuid>
where
    E: Executor<'e, Database = Postgres>,
{
    let id = Uuid::now_v7();
    sqlx::query_scalar!(
        r#"
        INSERT INTO external_subtitles (id, movie_file_id, episode_file_id, file_path,
                                        file_size, format, language, title,
                                        is_forced, is_hearing_impaired,
                                        source, source_id, source_metadata)
        VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13)
        RETURNING id
        "#,
        id,
        input.movie_file_id,
        input.episode_file_id,
        &input.file_path,
        input.file_size,
        &input.format,
        input.language.as_deref(),
        input.title.as_deref(),
        input.is_forced,
        input.is_hearing_impaired,
        &input.source,
        input.source_id.as_deref(),
        input.source_metadata.as_ref() as Option<&Value>,
    )
    .fetch_one(exec)
    .await?;

    Ok(id)
}

/// Upsert by file path (ON CONFLICT update metadata).
pub async fn upsert_by_path<'e, E>(exec: E, input: &ExternalSubtitleInput) -> Result<Uuid>
where
    E: Executor<'e, Database = Postgres>,
{
    let id = Uuid::now_v7();
    let row = sqlx::query_scalar!(
        r#"
        INSERT INTO external_subtitles (id, movie_file_id, episode_file_id, file_path,
                                        file_size, format, language, title,
                                        is_forced, is_hearing_impaired,
                                        source, source_id, source_metadata, available)
        VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, TRUE)
        ON CONFLICT (file_path)
        DO UPDATE SET
            file_size = EXCLUDED.file_size,
            language = COALESCE(EXCLUDED.language, external_subtitles.language),
            is_forced = EXCLUDED.is_forced,
            is_hearing_impaired = EXCLUDED.is_hearing_impaired,
            available = TRUE
        RETURNING id
        "#,
        id,
        input.movie_file_id,
        input.episode_file_id,
        &input.file_path,
        input.file_size,
        &input.format,
        input.language.as_deref(),
        input.title.as_deref(),
        input.is_forced,
        input.is_hearing_impaired,
        &input.source,
        input.source_id.as_deref(),
        input.source_metadata.as_ref() as Option<&Value>,
    )
    .fetch_one(exec)
    .await?;

    Ok(row)
}

/// Set availability of an external subtitle.
pub async fn set_available<'e, E>(exec: E, id: Uuid, available: bool) -> Result<bool>
where
    E: Executor<'e, Database = Postgres>,
{
    let result = sqlx::query!(
        "UPDATE external_subtitles SET available = $2 WHERE id = $1",
        id,
        available,
    )
    .execute(exec)
    .await?;
    Ok(result.rows_affected() > 0)
}

/// List external subtitles for a movie file.
pub async fn list_by_movie_file<'e, E>(
    exec: E,
    movie_file_id: Uuid,
) -> Result<Vec<ExternalSubtitleRow>>
where
    E: Executor<'e, Database = Postgres>,
{
    let rows = sqlx::query!(
        r#"
        SELECT id, movie_file_id, episode_file_id, file_path, file_size,
               format, language, title, is_forced, is_hearing_impaired,
               source, source_id, source_metadata, available, created_at
        FROM external_subtitles
        WHERE movie_file_id = $1
        ORDER BY created_at
        "#,
        movie_file_id,
    )
    .fetch_all(exec)
    .await?;

    Ok(rows
        .into_iter()
        .map(|r| ExternalSubtitleRow {
            id: r.id,
            movie_file_id: r.movie_file_id,
            episode_file_id: r.episode_file_id,
            file_path: r.file_path,
            file_size: r.file_size,
            format: r.format,
            language: r.language,
            title: r.title,
            is_forced: r.is_forced,
            is_hearing_impaired: r.is_hearing_impaired,
            source: r.source,
            source_id: r.source_id,
            source_metadata: r.source_metadata,
            available: r.available,
            created_at: r.created_at,
        })
        .collect())
}

/// List external subtitles for an episode file.
pub async fn list_by_episode_file<'e, E>(
    exec: E,
    episode_file_id: Uuid,
) -> Result<Vec<ExternalSubtitleRow>>
where
    E: Executor<'e, Database = Postgres>,
{
    let rows = sqlx::query!(
        r#"
        SELECT id, movie_file_id, episode_file_id, file_path, file_size,
               format, language, title, is_forced, is_hearing_impaired,
               source, source_id, source_metadata, available, created_at
        FROM external_subtitles
        WHERE episode_file_id = $1
        ORDER BY created_at
        "#,
        episode_file_id,
    )
    .fetch_all(exec)
    .await?;

    Ok(rows
        .into_iter()
        .map(|r| ExternalSubtitleRow {
            id: r.id,
            movie_file_id: r.movie_file_id,
            episode_file_id: r.episode_file_id,
            file_path: r.file_path,
            file_size: r.file_size,
            format: r.format,
            language: r.language,
            title: r.title,
            is_forced: r.is_forced,
            is_hearing_impaired: r.is_hearing_impaired,
            source: r.source,
            source_id: r.source_id,
            source_metadata: r.source_metadata,
            available: r.available,
            created_at: r.created_at,
        })
        .collect())
}
