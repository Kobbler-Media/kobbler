//! Movie skip segments repository (intro/credits detection results).

use super::Result;
use sqlx::{Executor, Postgres, Row};
use uuid::Uuid;

#[derive(Debug, serde::Serialize)]
pub struct MovieSkipSegmentRow {
    pub id: Uuid,
    pub movie_file_id: Uuid,
    pub segment_type: String,
    pub start_seconds: f64,
    pub end_seconds: f64,
    pub source: String,
    pub confidence: Option<f64>,
    pub config_tag: Option<String>,
}

#[allow(clippy::too_many_arguments)]
pub async fn upsert<'e, E>(
    exec: E,
    movie_file_id: Uuid,
    segment_type: &str,
    start_seconds: f64,
    end_seconds: f64,
    source: &str,
    confidence: Option<f64>,
    config_tag: Option<&str>,
) -> Result<Uuid>
where
    E: Executor<'e, Database = Postgres>,
{
    let id = Uuid::now_v7();
    let row = sqlx::query(
        r#"
        INSERT INTO movie_skip_segments (id, movie_file_id, segment_type, start_seconds, end_seconds, source, confidence, config_tag)
        VALUES ($1, $2, $3, $4, $5, $6, $7, $8)
        ON CONFLICT (movie_file_id, segment_type, start_seconds) DO UPDATE
            SET end_seconds = EXCLUDED.end_seconds,
                source = EXCLUDED.source,
                confidence = EXCLUDED.confidence,
                config_tag = EXCLUDED.config_tag
        WHERE movie_skip_segments.source != 'manual'
        RETURNING id
        "#,
    )
    .bind(id)
    .bind(movie_file_id)
    .bind(segment_type)
    .bind(start_seconds)
    .bind(end_seconds)
    .bind(source)
    .bind(confidence)
    .bind(config_tag)
    .fetch_one(exec)
    .await?;
    Ok(row.get("id"))
}

/// Like `upsert`, but always overwrites — even existing manual entries.
/// Used by the manual override endpoint.
#[allow(clippy::too_many_arguments)]
pub async fn force_upsert<'e, E>(
    exec: E,
    movie_file_id: Uuid,
    segment_type: &str,
    start_seconds: f64,
    end_seconds: f64,
    source: &str,
    confidence: Option<f64>,
    config_tag: Option<&str>,
) -> Result<Uuid>
where
    E: Executor<'e, Database = Postgres>,
{
    let id = Uuid::now_v7();
    let row = sqlx::query(
        r#"
        INSERT INTO movie_skip_segments (id, movie_file_id, segment_type, start_seconds, end_seconds, source, confidence, config_tag)
        VALUES ($1, $2, $3, $4, $5, $6, $7, $8)
        ON CONFLICT (movie_file_id, segment_type, start_seconds) DO UPDATE
            SET end_seconds = EXCLUDED.end_seconds,
                source = EXCLUDED.source,
                confidence = EXCLUDED.confidence,
                config_tag = EXCLUDED.config_tag
        RETURNING id
        "#,
    )
    .bind(id)
    .bind(movie_file_id)
    .bind(segment_type)
    .bind(start_seconds)
    .bind(end_seconds)
    .bind(source)
    .bind(confidence)
    .bind(config_tag)
    .fetch_one(exec)
    .await?;
    Ok(row.get("id"))
}

pub async fn list_by_movie_file<'e, E>(exec: E, file_id: Uuid) -> Result<Vec<MovieSkipSegmentRow>>
where
    E: Executor<'e, Database = Postgres>,
{
    let rows = sqlx::query(
        r#"
        SELECT id, movie_file_id, segment_type, start_seconds, end_seconds, source, confidence, config_tag
        FROM movie_skip_segments
        WHERE movie_file_id = $1 AND NOT (start_seconds = 0 AND end_seconds = 0)
        ORDER BY start_seconds
        "#,
    )
    .bind(file_id)
    .fetch_all(exec)
    .await?;

    Ok(rows
        .into_iter()
        .map(|r| MovieSkipSegmentRow {
            id: r.get("id"),
            movie_file_id: r.get("movie_file_id"),
            segment_type: r.get("segment_type"),
            start_seconds: r.get("start_seconds"),
            end_seconds: r.get("end_seconds"),
            source: r.get("source"),
            confidence: r.get("confidence"),
            config_tag: r.get("config_tag"),
        })
        .collect())
}

pub async fn delete_by_movie_file<'e, E>(exec: E, file_id: Uuid) -> Result<u64>
where
    E: Executor<'e, Database = Postgres>,
{
    let result = sqlx::query("DELETE FROM movie_skip_segments WHERE movie_file_id = $1")
        .bind(file_id)
        .execute(exec)
        .await?;
    Ok(result.rows_affected())
}

/// Suppress a segment type: delete all existing rows for this type and insert
/// a (0,0) suppression marker. The marker prevents re-detection from re-creating
/// the segment.
pub async fn suppress<'e, E>(exec: E, movie_file_id: Uuid, segment_type: &str) -> Result<()>
where
    E: Executor<'e, Database = Postgres>,
{
    sqlx::query(
        r#"
        WITH deleted AS (
            DELETE FROM movie_skip_segments
            WHERE movie_file_id = $1 AND segment_type = $2
        )
        INSERT INTO movie_skip_segments (id, movie_file_id, segment_type, start_seconds, end_seconds, source, confidence, config_tag)
        VALUES ($3, $1, $2, 0, 0, 'manual', NULL, NULL)
        ON CONFLICT (movie_file_id, segment_type, start_seconds) DO UPDATE
            SET end_seconds = 0, source = 'manual', confidence = NULL, config_tag = NULL
        "#,
    )
    .bind(movie_file_id)
    .bind(segment_type)
    .bind(Uuid::now_v7())
    .execute(exec)
    .await?;
    Ok(())
}

/// Delete the chapter-sourced segment for a specific file and segment type.
/// Used when a detected segment with `chapter_confirmed` supersedes the chapter marker.
pub async fn delete_chapter_for_type<'e, E>(
    exec: E,
    movie_file_id: Uuid,
    segment_type: &str,
) -> Result<u64>
where
    E: Executor<'e, Database = Postgres>,
{
    let result = sqlx::query(
        "DELETE FROM movie_skip_segments WHERE movie_file_id = $1 AND segment_type = $2 AND source = 'chapter'",
    )
    .bind(movie_file_id)
    .bind(segment_type)
    .execute(exec)
    .await?;
    Ok(result.rows_affected())
}

/// Delete all auto-detected segments for a set of movie files, preserving
/// manual overrides and chapter-sourced segments.
pub async fn delete_auto_detected<'e, E>(exec: E, movie_file_ids: &[Uuid]) -> Result<u64>
where
    E: Executor<'e, Database = Postgres>,
{
    let result = sqlx::query(
        "DELETE FROM movie_skip_segments WHERE movie_file_id = ANY($1) AND source NOT IN ('chapter', 'manual')",
    )
    .bind(movie_file_ids)
    .execute(exec)
    .await?;
    Ok(result.rows_affected())
}
