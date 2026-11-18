use super::Result;
use crate::corruption::CorruptRange;
use crate::ffmpeg::persist::ChapterSkipSegment;
use crate::routes::admin::streaming::ChapterResponse;
use sqlx::{Executor, PgPool, Postgres};
use uuid::Uuid;

#[derive(Debug, Clone)]
pub struct DebridFile {
    pub id: Uuid,
    pub cache_path: String,
    pub media_type: String,
    pub media_id: Uuid,
}

/// The row for the download at `cache_path`, made on first sight. The media it plays as
/// follows the latest session to name it.
pub async fn upsert<'e, E>(
    exec: E,
    cache_path: &str,
    media_type: &str,
    media_id: Uuid,
) -> Result<DebridFile>
where
    E: Executor<'e, Database = Postgres>,
{
    let row = sqlx::query_as!(
        DebridFile,
        r#"
        INSERT INTO debrid_files (cache_path, media_type, media_id)
        VALUES ($1, $2, $3)
        ON CONFLICT (cache_path) DO UPDATE
            SET media_type = EXCLUDED.media_type, media_id = EXCLUDED.media_id
        RETURNING id, cache_path, media_type, media_id
        "#,
        cache_path,
        media_type,
        media_id,
    )
    .fetch_one(exec)
    .await?;
    Ok(row)
}

pub async fn at<'e, E>(exec: E, cache_path: &str) -> Result<Option<DebridFile>>
where
    E: Executor<'e, Database = Postgres>,
{
    let row = sqlx::query_as!(
        DebridFile,
        "SELECT id, cache_path, media_type, media_id FROM debrid_files WHERE cache_path = $1",
        cache_path,
    )
    .fetch_optional(exec)
    .await?;
    Ok(row)
}

/// Chapters as probed from the file a session plays, once it has a download to name.
pub async fn record_session_chapters<'e, E>(
    exec: E,
    session_id: Uuid,
    chapters: &[ChapterResponse],
) -> Result<()>
where
    E: Executor<'e, Database = Postgres>,
{
    let Some(chapters) = chapters_json(chapters) else {
        return Ok(());
    };
    sqlx::query!(
        r#"
        INSERT INTO debrid_files (cache_path, media_type, media_id, chapters)
        SELECT cache_path, media_type, media_id, $2
        FROM debrid_stream_sessions
        WHERE id = $1 AND cache_path IS NOT NULL
        ON CONFLICT (cache_path) DO UPDATE SET chapters = EXCLUDED.chapters, updated_at = NOW()
        "#,
        session_id,
        chapters,
    )
    .execute(exec)
    .await?;
    Ok(())
}

pub async fn set_chapters<'e, E>(exec: E, id: Uuid, chapters: &[ChapterResponse]) -> Result<()>
where
    E: Executor<'e, Database = Postgres>,
{
    sqlx::query!(
        "UPDATE debrid_files SET chapters = COALESCE($2, chapters), updated_at = NOW() WHERE id = $1",
        id,
        chapters_json(chapters),
    )
    .execute(exec)
    .await?;
    Ok(())
}

fn chapters_json(chapters: &[ChapterResponse]) -> Option<serde_json::Value> {
    (!chapters.is_empty())
        .then(|| serde_json::to_value(chapters).ok())
        .flatten()
}

pub async fn chapters<'e, E>(exec: E, id: Uuid) -> Result<Vec<ChapterResponse>>
where
    E: Executor<'e, Database = Postgres>,
{
    let chapters = sqlx::query_scalar!("SELECT chapters FROM debrid_files WHERE id = $1", id)
        .fetch_optional(exec)
        .await?
        .flatten();
    Ok(chapters
        .and_then(|c| serde_json::from_value(c).ok())
        .unwrap_or_default())
}

pub async fn set_skip_segments<'e, E>(
    exec: E,
    id: Uuid,
    segments: &[ChapterSkipSegment],
) -> Result<()>
where
    E: Executor<'e, Database = Postgres>,
{
    let segments = serde_json::to_value(segments).unwrap_or_default();
    sqlx::query!(
        "UPDATE debrid_files SET skip_segments = $2, updated_at = NOW() WHERE id = $1",
        id,
        segments,
    )
    .execute(exec)
    .await?;
    Ok(())
}

/// What analysis found in the download at `cache_path`, for a session about to play it.
pub async fn skip_segments_at<'e, E>(exec: E, cache_path: &str) -> Result<Vec<ChapterSkipSegment>>
where
    E: Executor<'e, Database = Postgres>,
{
    let segments = sqlx::query_scalar!(
        "SELECT skip_segments FROM debrid_files WHERE cache_path = $1",
        cache_path,
    )
    .fetch_optional(exec)
    .await?
    .flatten();
    Ok(segments
        .and_then(|s| serde_json::from_value(s).ok())
        .unwrap_or_default())
}

pub async fn mark_skip_detect_ran<'e, E>(exec: E, ids: &[Uuid]) -> Result<()>
where
    E: Executor<'e, Database = Postgres>,
{
    sqlx::query!(
        "UPDATE debrid_files SET skip_detect_ran_at = NOW() WHERE id = ANY($1)",
        ids,
    )
    .execute(exec)
    .await?;
    Ok(())
}

/// Replace the ranges analysis found, canonicalised as the library's are.
pub async fn set_corrupt_ranges<'e, E>(exec: E, id: Uuid, ranges: Vec<CorruptRange>) -> Result<()>
where
    E: Executor<'e, Database = Postgres>,
{
    let canonical = crate::corruption::merge(ranges, [], crate::corruption::DEFAULT_MERGE_GAP_SECS);
    sqlx::query!(
        "UPDATE debrid_files SET corrupt_ranges = $2 WHERE id = $1",
        id,
        serde_json::to_value(canonical).unwrap_or_default(),
    )
    .execute(exec)
    .await?;
    Ok(())
}

pub async fn corrupt_ranges_at<'e, E>(exec: E, cache_path: &str) -> Result<Vec<CorruptRange>>
where
    E: Executor<'e, Database = Postgres>,
{
    let ranges = sqlx::query_scalar!(
        "SELECT corrupt_ranges FROM debrid_files WHERE cache_path = $1",
        cache_path,
    )
    .fetch_optional(exec)
    .await?;
    Ok(ranges
        .and_then(|r| serde_json::from_value(r).ok())
        .unwrap_or_default())
}

/// A range the player hit at runtime, merged in for the next play of the same file.
pub async fn append_corrupt_range(
    pool: &PgPool,
    cache_path: &str,
    range: CorruptRange,
) -> Result<bool> {
    let mut tx = pool.begin().await?;
    let existing = sqlx::query_scalar!(
        "SELECT corrupt_ranges FROM debrid_files WHERE cache_path = $1 FOR UPDATE",
        cache_path,
    )
    .fetch_optional(&mut *tx)
    .await?;
    let Some(existing) = existing else {
        return Ok(false);
    };
    let existing: Vec<CorruptRange> = serde_json::from_value(existing).unwrap_or_default();
    let merged =
        crate::corruption::merge(existing, [range], crate::corruption::DEFAULT_MERGE_GAP_SECS);
    sqlx::query!(
        "UPDATE debrid_files SET corrupt_ranges = $2 WHERE cache_path = $1",
        cache_path,
        serde_json::to_value(merged).unwrap_or_default(),
    )
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(true)
}

/// Credits start of the file a title was last analysed in, for when the session that
/// played it no longer knows its download.
pub async fn latest_credits_start<'e, E>(
    exec: E,
    media_type: &str,
    media_id: Uuid,
) -> Result<Option<f64>>
where
    E: Executor<'e, Database = Postgres>,
{
    let start = sqlx::query_scalar!(
        r#"
        SELECT (SELECT MIN((seg->>'start_seconds')::float8)
                FROM jsonb_array_elements(f.skip_segments) seg
                WHERE seg->>'segment_type' = 'credits') AS start
        FROM debrid_files f
        WHERE f.media_type = $1 AND f.media_id = $2
          AND jsonb_typeof(f.skip_segments) = 'array'
        ORDER BY f.skip_detect_ran_at DESC NULLS LAST, f.updated_at DESC
        LIMIT 1
        "#,
        media_type,
        media_id,
    )
    .fetch_optional(exec)
    .await?;
    Ok(start.flatten())
}

/// Forget the files of titles that are gone, returning their ids so their analysis can
/// leave the disk with them.
pub async fn delete_for_media<'e, E>(
    exec: E,
    media_type: &str,
    media_ids: &[Uuid],
) -> Result<Vec<Uuid>>
where
    E: Executor<'e, Database = Postgres>,
{
    let ids = sqlx::query_scalar!(
        "DELETE FROM debrid_files WHERE media_type = $1 AND media_id = ANY($2) RETURNING id",
        media_type,
        media_ids,
    )
    .fetch_all(exec)
    .await?;
    Ok(ids)
}

/// Delete the files of titles that are gone; the ids of those left.
pub async fn prune_orphans(pool: &PgPool) -> Result<Vec<Uuid>> {
    let mut tx = pool.begin().await?;
    sqlx::query!(
        r#"
        DELETE FROM debrid_files f
        WHERE (f.media_type = 'episode' AND NOT EXISTS (SELECT 1 FROM tv_episodes e WHERE e.id = f.media_id))
           OR (f.media_type = 'movie' AND NOT EXISTS (SELECT 1 FROM movies m WHERE m.id = f.media_id))
        "#
    )
    .execute(&mut *tx)
    .await?;
    let live = sqlx::query_scalar!("SELECT id FROM debrid_files")
        .fetch_all(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok(live)
}
