//! Read/write helpers for `tv_episode_cached_segments` and
//! `movie_cached_segments`.
//!
//! These are the global, id-keyed cache of skip-segments produced by
//! cross-episode / movie detect. Debrid sessions UPSERT here after a
//! successful analyze + detect so the *next* debrid stream of the same
//! episode/movie reuses the result without re-running detect (which means
//! intro/credits offsets are available the moment the SSE channel opens —
//! no minutes-long analyze wait on re-streams).
//!
//! Distinct from the per-file `episode_skip_segments` / `movie_skip_segments`
//! tables, which are keyed by `episode_file_id` / `movie_file_id` and serve
//! the in-library playback path. The per-file tables are higher-confidence
//! (they reflect the exact bytes of the user's local file); these
//! id-keyed ones are best-effort across releases of the same episode/movie.

use sqlx::PgPool;
use time::OffsetDateTime;
use uuid::Uuid;

use crate::ffmpeg::persist::ChapterSkipSegment;

/// Cached skip-segments row returned by [`get_episode`] / [`get_movie`].
#[derive(Debug, Clone)]
pub struct CachedSegments {
    pub segments: Vec<ChapterSkipSegment>,
    pub pipeline_version: i32,
    pub generated_at: OffsetDateTime,
}

pub async fn get_episode(
    pool: &PgPool,
    episode_id: Uuid,
) -> Result<Option<CachedSegments>, sqlx::Error> {
    let row = sqlx::query!(
        r#"
        SELECT segments, pipeline_version, generated_at
        FROM tv_episode_cached_segments
        WHERE episode_id = $1
        "#,
        episode_id,
    )
    .fetch_optional(pool)
    .await?;
    let Some(row) = row else { return Ok(None) };
    let segments: Vec<ChapterSkipSegment> = serde_json::from_value(row.segments).map_err(|e| {
        sqlx::Error::Decode(Box::new(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            format!("cached_segments JSON decode: {e}"),
        )))
    })?;
    Ok(Some(CachedSegments {
        segments,
        pipeline_version: row.pipeline_version,
        generated_at: row.generated_at,
    }))
}

pub async fn upsert_episode(
    pool: &PgPool,
    episode_id: Uuid,
    segments: &[ChapterSkipSegment],
    pipeline_version: i32,
) -> Result<(), sqlx::Error> {
    let json = serde_json::to_value(segments).map_err(|e| {
        sqlx::Error::Encode(Box::new(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            format!("cached_segments JSON encode: {e}"),
        )))
    })?;
    sqlx::query!(
        r#"
        INSERT INTO tv_episode_cached_segments (episode_id, segments, pipeline_version, generated_at)
        VALUES ($1, $2, $3, NOW())
        ON CONFLICT (episode_id) DO UPDATE
            SET segments = EXCLUDED.segments,
                pipeline_version = EXCLUDED.pipeline_version,
                generated_at = NOW()
            WHERE tv_episode_cached_segments.pipeline_version <= EXCLUDED.pipeline_version
        "#,
        episode_id,
        json,
        pipeline_version,
    )
    .execute(pool)
    .await?;
    Ok(())
}

pub async fn get_movie(
    pool: &PgPool,
    movie_id: Uuid,
) -> Result<Option<CachedSegments>, sqlx::Error> {
    let row = sqlx::query!(
        r#"
        SELECT segments, pipeline_version, generated_at
        FROM movie_cached_segments
        WHERE movie_id = $1
        "#,
        movie_id,
    )
    .fetch_optional(pool)
    .await?;
    let Some(row) = row else { return Ok(None) };
    let segments: Vec<ChapterSkipSegment> = serde_json::from_value(row.segments).map_err(|e| {
        sqlx::Error::Decode(Box::new(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            format!("cached_segments JSON decode: {e}"),
        )))
    })?;
    Ok(Some(CachedSegments {
        segments,
        pipeline_version: row.pipeline_version,
        generated_at: row.generated_at,
    }))
}

pub async fn upsert_movie(
    pool: &PgPool,
    movie_id: Uuid,
    segments: &[ChapterSkipSegment],
    pipeline_version: i32,
) -> Result<(), sqlx::Error> {
    let json = serde_json::to_value(segments).map_err(|e| {
        sqlx::Error::Encode(Box::new(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            format!("cached_segments JSON encode: {e}"),
        )))
    })?;
    sqlx::query!(
        r#"
        INSERT INTO movie_cached_segments (movie_id, segments, pipeline_version, generated_at)
        VALUES ($1, $2, $3, NOW())
        ON CONFLICT (movie_id) DO UPDATE
            SET segments = EXCLUDED.segments,
                pipeline_version = EXCLUDED.pipeline_version,
                generated_at = NOW()
            WHERE movie_cached_segments.pipeline_version <= EXCLUDED.pipeline_version
        "#,
        movie_id,
        json,
        pipeline_version,
    )
    .execute(pool)
    .await?;
    Ok(())
}

/// What detect last found for any release of this title, if from a pipeline that can
/// still be trusted. Timed to that release, so only an estimate for another.
pub async fn current(
    pool: &PgPool,
    media_type: &str,
    media_id: Uuid,
) -> Result<Vec<ChapterSkipSegment>, sqlx::Error> {
    let cached = match media_type {
        "episode" => get_episode(pool, media_id).await?,
        "movie" => get_movie(pool, media_id).await?,
        _ => None,
    };
    Ok(cached
        .filter(|c| c.pipeline_version >= crate::media_cache::MIN_SUPPORTED_PIPELINE_VERSION)
        .map(|c| c.segments)
        .unwrap_or_default())
}
