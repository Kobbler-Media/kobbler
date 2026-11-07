//! Movie files repository operations.

use super::Result;
use crate::file_link_mode::FileLinkMode;
use serde_json::Value;
use sqlx::{Executor, Postgres};
use time::OffsetDateTime;
use uuid::Uuid;

// =============================================================================
// Types
// =============================================================================

/// Input for creating a movie file.
#[derive(Debug, Default)]
pub struct MovieFileInput {
    pub movie_id: Uuid,
    pub file_path: String,
    pub file_size: Option<i64>,
    pub file_fingerprint: Option<String>,
    pub format: String,
    pub quality: Option<Value>,
    pub languages: Option<Vec<String>>,
    pub subtitles: Option<Vec<String>>,
    pub source_type: Option<String>,
    pub source_metadata: Option<Value>,
    // Video metadata
    pub video_codec: Option<String>,
    pub video_width: Option<i32>,
    pub video_height: Option<i32>,
    pub video_bitrate_kbps: Option<i32>,
    pub framerate: Option<f64>,
    pub bit_depth: Option<i16>,
    pub is_hdr: bool,
    pub hdr_format: Option<String>,
    pub is_interlaced: bool,
    pub duration_seconds: Option<f64>,
    pub container: Option<String>,
    pub overall_bitrate_kbps: Option<i32>,
}

/// Row returned from movie_files queries.
#[derive(Debug)]
pub struct MovieFileRow {
    pub id: Uuid,
    pub movie_id: Uuid,
    pub file_path: String,
    pub file_size: Option<i64>,
    pub file_fingerprint: Option<String>,
    pub format: String,
    pub quality: Option<Value>,
    pub languages: Option<Vec<String>>,
    pub subtitles: Option<Vec<String>>,
    pub source_type: Option<String>,
    pub source_metadata: Option<Value>,
    pub available: bool,
    pub created_at: OffsetDateTime,
    // Video metadata
    pub video_codec: Option<String>,
    pub video_width: Option<i32>,
    pub video_height: Option<i32>,
    pub video_bitrate_kbps: Option<i32>,
    pub framerate: Option<f64>,
    pub bit_depth: Option<i16>,
    pub is_hdr: bool,
    pub hdr_format: Option<String>,
    pub is_interlaced: bool,
    pub dv_profile: Option<i16>,
    pub dv_bl_compat_id: Option<i16>,
    pub duration_seconds: Option<f64>,
    pub container: Option<String>,
    pub overall_bitrate_kbps: Option<i32>,
    pub keyframes: Option<Value>,
}

// =============================================================================
// CRUD
// =============================================================================

/// Create a new movie file record.
pub async fn create<'e, E>(exec: E, input: MovieFileInput) -> Result<Uuid>
where
    E: Executor<'e, Database = Postgres>,
{
    let id = Uuid::now_v7();

    let row = sqlx::query_scalar!(
        r#"
        INSERT INTO movie_files (id, movie_id, file_path, file_size, format, quality,
                                 languages, subtitles, source_type, source_metadata,
                                 video_codec, video_width, video_height, video_bitrate_kbps,
                                 framerate, bit_depth, is_hdr, hdr_format, is_interlaced,
                                 duration_seconds, container, overall_bitrate_kbps, file_fingerprint)
        VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10,
                $11, $12, $13, $14, $15, $16, $17, $18, $19, $20, $21, $22, $23)
        RETURNING id
        "#,
        id,
        input.movie_id,
        &input.file_path,
        input.file_size,
        &input.format,
        input.quality as Option<Value>,
        input.languages.as_deref() as Option<&[String]>,
        input.subtitles.as_deref() as Option<&[String]>,
        input.source_type.as_deref(),
        input.source_metadata as Option<Value>,
        input.video_codec.as_deref(),
        input.video_width,
        input.video_height,
        input.video_bitrate_kbps,
        input.framerate,
        input.bit_depth,
        input.is_hdr,
        input.hdr_format.as_deref(),
        input.is_interlaced,
        input.duration_seconds,
        input.container.as_deref(),
        input.overall_bitrate_kbps,
        input.file_fingerprint.as_deref(),
    )
    .fetch_one(exec)
    .await?;

    Ok(row)
}

/// Create or update a movie file record (upsert on file_path).
///
/// If a record already exists for this file_path, updates metadata fields
/// but preserves the existing movie_id link and id.
pub async fn create_or_update<'e, E>(exec: E, input: MovieFileInput) -> Result<Uuid>
where
    E: Executor<'e, Database = Postgres>,
{
    let id = Uuid::now_v7();

    let row = sqlx::query_scalar!(
        r#"
        INSERT INTO movie_files (id, movie_id, file_path, file_size, format, quality,
                                 languages, subtitles, source_type, source_metadata,
                                 video_codec, video_width, video_height, video_bitrate_kbps,
                                 framerate, bit_depth, is_hdr, hdr_format, is_interlaced,
                                 duration_seconds, container, overall_bitrate_kbps, file_fingerprint)
        VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10,
                $11, $12, $13, $14, $15, $16, $17, $18, $19, $20, $21, $22, $23)
        ON CONFLICT (file_path) DO UPDATE SET
            file_size = COALESCE(EXCLUDED.file_size, movie_files.file_size),
            file_fingerprint = COALESCE(EXCLUDED.file_fingerprint, movie_files.file_fingerprint),
            format = EXCLUDED.format,
            quality = COALESCE(EXCLUDED.quality, movie_files.quality),
            languages = COALESCE(EXCLUDED.languages, movie_files.languages),
            subtitles = COALESCE(EXCLUDED.subtitles, movie_files.subtitles),
            source_type = COALESCE(EXCLUDED.source_type, movie_files.source_type),
            source_metadata = COALESCE(EXCLUDED.source_metadata, movie_files.source_metadata),
            -- See the matching note in tv_episode_files::create_or_update: the
            -- 'unreadable' sentinel takes a file out of list_needing_probe and
            -- nothing clears it, so a replacement at the same path would stay
            -- unprobed forever. A change in size means this is a different
            -- file, so the earlier verdict no longer applies.
            video_codec = CASE
                WHEN movie_files.video_codec = 'unreadable'
                 AND EXCLUDED.file_size IS DISTINCT FROM movie_files.file_size
                THEN NULL
                ELSE COALESCE(EXCLUDED.video_codec, movie_files.video_codec)
            END,
            video_width = COALESCE(EXCLUDED.video_width, movie_files.video_width),
            video_height = COALESCE(EXCLUDED.video_height, movie_files.video_height),
            video_bitrate_kbps = COALESCE(EXCLUDED.video_bitrate_kbps, movie_files.video_bitrate_kbps),
            framerate = COALESCE(EXCLUDED.framerate, movie_files.framerate),
            bit_depth = COALESCE(EXCLUDED.bit_depth, movie_files.bit_depth),
            is_hdr = EXCLUDED.is_hdr,
            hdr_format = COALESCE(EXCLUDED.hdr_format, movie_files.hdr_format),
            is_interlaced = EXCLUDED.is_interlaced,
            -- A content change at the same path invalidates every byte-exact /
            -- probe-derived cache keyed by this row id. "Changed" = the scanner's
            -- content fingerprint differs (scan path) OR file_size differs (kobload
            -- ingest, which has no fingerprint) — each import path supplies exactly
            -- one of the two. Leaving caches stale maps resume-seeks to the wrong
            -- segment (endless player load), shows old trickplay/LUFS/skip markers,
            -- and force-transcodes the wrong byte windows. media_cache_version = -1
            -- forces media_analyze to regen instead of short-circuiting on the
            -- stale on-disk manifest; duration takes EXCLUDED (NULL → reprobed).
            duration_seconds = CASE
                WHEN (EXCLUDED.file_fingerprint IS NOT NULL
                      AND EXCLUDED.file_fingerprint IS DISTINCT FROM movie_files.file_fingerprint)
                     OR (EXCLUDED.file_size IS NOT NULL
                         AND EXCLUDED.file_size IS DISTINCT FROM movie_files.file_size)
                THEN EXCLUDED.duration_seconds
                ELSE COALESCE(EXCLUDED.duration_seconds, movie_files.duration_seconds)
            END,
            container = COALESCE(EXCLUDED.container, movie_files.container),
            overall_bitrate_kbps = COALESCE(EXCLUDED.overall_bitrate_kbps, movie_files.overall_bitrate_kbps),
            keyframes = CASE
                WHEN (EXCLUDED.file_fingerprint IS NOT NULL
                      AND EXCLUDED.file_fingerprint IS DISTINCT FROM movie_files.file_fingerprint)
                     OR (EXCLUDED.file_size IS NOT NULL
                         AND EXCLUDED.file_size IS DISTINCT FROM movie_files.file_size)
                THEN NULL
                ELSE movie_files.keyframes
            END,
            lufs = CASE
                WHEN (EXCLUDED.file_fingerprint IS NOT NULL
                      AND EXCLUDED.file_fingerprint IS DISTINCT FROM movie_files.file_fingerprint)
                     OR (EXCLUDED.file_size IS NOT NULL
                         AND EXCLUDED.file_size IS DISTINCT FROM movie_files.file_size)
                THEN NULL
                ELSE movie_files.lufs
            END,
            corrupt_ranges = CASE
                WHEN (EXCLUDED.file_fingerprint IS NOT NULL
                      AND EXCLUDED.file_fingerprint IS DISTINCT FROM movie_files.file_fingerprint)
                     OR (EXCLUDED.file_size IS NOT NULL
                         AND EXCLUDED.file_size IS DISTINCT FROM movie_files.file_size)
                THEN '[]'::jsonb
                ELSE movie_files.corrupt_ranges
            END,
            -- -1 sentinel = "invalidated by replace", distinct from the default
            -- 0 (legacy / never-analyzed) so media_analyze only force-regenerates
            -- genuinely-replaced files. See db_media_cache_invalidated.
            media_cache_version = CASE
                WHEN (EXCLUDED.file_fingerprint IS NOT NULL
                      AND EXCLUDED.file_fingerprint IS DISTINCT FROM movie_files.file_fingerprint)
                     OR (EXCLUDED.file_size IS NOT NULL
                         AND EXCLUDED.file_size IS DISTINCT FROM movie_files.file_size)
                THEN -1
                ELSE movie_files.media_cache_version
            END,
            available = TRUE
        RETURNING id
        "#,
        id,
        input.movie_id,
        &input.file_path,
        input.file_size,
        &input.format,
        input.quality as Option<Value>,
        input.languages.as_deref() as Option<&[String]>,
        input.subtitles.as_deref() as Option<&[String]>,
        input.source_type.as_deref(),
        input.source_metadata as Option<Value>,
        input.video_codec.as_deref(),
        input.video_width,
        input.video_height,
        input.video_bitrate_kbps,
        input.framerate,
        input.bit_depth,
        input.is_hdr,
        input.hdr_format.as_deref(),
        input.is_interlaced,
        input.duration_seconds,
        input.container.as_deref(),
        input.overall_bitrate_kbps,
        input.file_fingerprint.as_deref(),
    )
    .fetch_one(exec)
    .await?;

    Ok(row)
}

/// Delete all existing files for a movie except the given file.
/// Returns file paths of deleted records for physical cleanup.
/// Equivalent to `replace_other_files_for_movie` in `FileLinkMode::Replace`,
/// which is what every path except a regrab wants.
pub async fn delete_other_files_for_movie(
    pool: &sqlx::PgPool,
    movie_id: Uuid,
    keep_file_id: Uuid,
) -> Result<Vec<String>> {
    replace_other_files_for_movie(pool, movie_id, keep_file_id, FileLinkMode::Replace).await
}

/// Settle the other files for a movie against the newly ingested one.
///
/// In `Replace` mode they are deleted and their paths returned for physical
/// cleanup. In `AddCopy` mode — a regrab — nothing is deleted and the movie ends
/// up with two files, newest preferred.
pub async fn replace_other_files_for_movie(
    pool: &sqlx::PgPool,
    movie_id: Uuid,
    keep_file_id: Uuid,
    mode: FileLinkMode,
) -> Result<Vec<String>> {
    if mode.is_add_copy() {
        return Ok(Vec::new());
    }

    let paths: Vec<String> = sqlx::query_scalar(
        "DELETE FROM movie_files WHERE movie_id = $1 AND id != $2 RETURNING file_path",
    )
    .bind(movie_id)
    .bind(keep_file_id)
    .fetch_all(pool)
    .await?;

    for path in &paths {
        tracing::info!(
            movie_id = %movie_id,
            old_path = %path,
            keep_file_id = %keep_file_id,
            "Replacing movie file with new file"
        );
    }

    Ok(paths)
}

/// Get a single movie file by ID.
pub async fn get<'e, E>(exec: E, id: Uuid) -> Result<Option<MovieFileRow>>
where
    E: Executor<'e, Database = Postgres>,
{
    let row = sqlx::query!(
        r#"
        SELECT id, movie_id, file_path, file_size, file_fingerprint, format, quality,
               languages, subtitles, source_type, source_metadata, available, created_at,
               video_codec, video_width, video_height, video_bitrate_kbps,
               framerate, bit_depth, is_hdr, hdr_format, is_interlaced, dv_profile, dv_bl_compat_id,
               duration_seconds, container, overall_bitrate_kbps, keyframes
        FROM movie_files
        WHERE id = $1
        "#,
        id,
    )
    .fetch_optional(exec)
    .await?;

    Ok(row.map(|r| MovieFileRow {
        id: r.id,
        movie_id: r.movie_id,
        file_path: r.file_path,
        file_size: r.file_size,
        file_fingerprint: r.file_fingerprint,
        format: r.format,
        quality: r.quality,
        languages: r.languages,
        subtitles: r.subtitles,
        source_type: r.source_type,
        source_metadata: r.source_metadata,
        available: r.available,
        created_at: r.created_at,
        video_codec: r.video_codec,
        video_width: r.video_width,
        video_height: r.video_height,
        video_bitrate_kbps: r.video_bitrate_kbps,
        framerate: r.framerate,
        bit_depth: r.bit_depth,
        is_hdr: r.is_hdr,
        hdr_format: r.hdr_format,
        is_interlaced: r.is_interlaced,
        dv_profile: r.dv_profile,
        dv_bl_compat_id: r.dv_bl_compat_id,
        duration_seconds: r.duration_seconds,
        container: r.container,
        overall_bitrate_kbps: r.overall_bitrate_kbps,
        keyframes: r.keyframes,
    }))
}

/// List all files for a movie.
pub async fn list_by_movie<'e, E>(exec: E, movie_id: Uuid) -> Result<Vec<MovieFileRow>>
where
    E: Executor<'e, Database = Postgres>,
{
    let rows = sqlx::query!(
        r#"
        SELECT id, movie_id, file_path, file_size, file_fingerprint, format, quality,
               languages, subtitles, source_type, source_metadata, available, created_at,
               video_codec, video_width, video_height, video_bitrate_kbps,
               framerate, bit_depth, is_hdr, hdr_format, is_interlaced, dv_profile, dv_bl_compat_id,
               duration_seconds, container, overall_bitrate_kbps, keyframes
        FROM movie_files
        WHERE movie_id = $1
        ORDER BY created_at DESC
        "#,
        movie_id,
    )
    .fetch_all(exec)
    .await?;

    Ok(rows
        .into_iter()
        .map(|r| MovieFileRow {
            id: r.id,
            movie_id: r.movie_id,
            file_path: r.file_path,
            file_size: r.file_size,
            file_fingerprint: r.file_fingerprint,
            format: r.format,
            quality: r.quality,
            languages: r.languages,
            subtitles: r.subtitles,
            source_type: r.source_type,
            source_metadata: r.source_metadata,
            available: r.available,
            created_at: r.created_at,
            video_codec: r.video_codec,
            video_width: r.video_width,
            video_height: r.video_height,
            video_bitrate_kbps: r.video_bitrate_kbps,
            framerate: r.framerate,
            bit_depth: r.bit_depth,
            is_hdr: r.is_hdr,
            hdr_format: r.hdr_format,
            is_interlaced: r.is_interlaced,
            dv_profile: r.dv_profile,
            dv_bl_compat_id: r.dv_bl_compat_id,
            duration_seconds: r.duration_seconds,
            container: r.container,
            overall_bitrate_kbps: r.overall_bitrate_kbps,
            keyframes: r.keyframes,
        })
        .collect())
}

/// Delete a movie file by ID, returning the movie_id if a row was deleted.
pub async fn delete<'e, E>(exec: E, id: Uuid) -> Result<Option<Uuid>>
where
    E: Executor<'e, Database = Postgres>,
{
    let row = sqlx::query_scalar!(
        "DELETE FROM movie_files WHERE id = $1 RETURNING movie_id",
        id,
    )
    .fetch_optional(exec)
    .await?;

    Ok(row)
}

/// Count files for a movie.
pub async fn count_by_movie<'e, E>(exec: E, movie_id: Uuid) -> Result<i64>
where
    E: Executor<'e, Database = Postgres>,
{
    let count: i64 = sqlx::query_scalar!(
        r#"SELECT COUNT(*) AS "count!" FROM movie_files WHERE movie_id = $1"#,
        movie_id,
    )
    .fetch_one(exec)
    .await?;
    Ok(count)
}

/// Move a movie file to a different movie.
pub async fn move_to_movie<'e, E>(exec: E, file_id: Uuid, new_movie_id: Uuid) -> Result<bool>
where
    E: Executor<'e, Database = Postgres>,
{
    let result = sqlx::query!(
        "UPDATE movie_files SET movie_id = $2 WHERE id = $1",
        file_id,
        new_movie_id,
    )
    .execute(exec)
    .await?;

    Ok(result.rows_affected() > 0)
}

/// Set the availability of a movie file.
pub async fn set_available<'e, E>(exec: E, id: Uuid, available: bool) -> Result<bool>
where
    E: Executor<'e, Database = Postgres>,
{
    let result = sqlx::query!(
        "UPDATE movie_files SET available = $2 WHERE id = $1",
        id,
        available,
    )
    .execute(exec)
    .await?;

    Ok(result.rows_affected() > 0)
}

/// The available file the player should use for this movie: the **newest**.
/// See `tv_episode_files::get_preferred_available`.
pub async fn get_preferred_available<'e, E>(exec: E, movie_id: Uuid) -> Result<Option<MovieFileRow>>
where
    E: Executor<'e, Database = Postgres>,
{
    let row = sqlx::query!(
        r#"
        SELECT id, movie_id, file_path, file_size, file_fingerprint, format, quality,
               languages, subtitles, source_type, source_metadata, available, created_at,
               video_codec, video_width, video_height, video_bitrate_kbps,
               framerate, bit_depth, is_hdr, hdr_format, is_interlaced, dv_profile, dv_bl_compat_id,
               duration_seconds, container, overall_bitrate_kbps, keyframes
        FROM movie_files
        WHERE movie_id = $1 AND available = TRUE
        ORDER BY created_at DESC, id DESC
        LIMIT 1
        "#,
        movie_id,
    )
    .fetch_optional(exec)
    .await?;

    Ok(row.map(|r| MovieFileRow {
        id: r.id,
        movie_id: r.movie_id,
        file_path: r.file_path,
        file_size: r.file_size,
        file_fingerprint: r.file_fingerprint,
        format: r.format,
        quality: r.quality,
        languages: r.languages,
        subtitles: r.subtitles,
        source_type: r.source_type,
        source_metadata: r.source_metadata,
        available: r.available,
        created_at: r.created_at,
        video_codec: r.video_codec,
        video_width: r.video_width,
        video_height: r.video_height,
        video_bitrate_kbps: r.video_bitrate_kbps,
        framerate: r.framerate,
        bit_depth: r.bit_depth,
        is_hdr: r.is_hdr,
        hdr_format: r.hdr_format,
        is_interlaced: r.is_interlaced,
        dv_profile: r.dv_profile,
        dv_bl_compat_id: r.dv_bl_compat_id,
        duration_seconds: r.duration_seconds,
        container: r.container,
        overall_bitrate_kbps: r.overall_bitrate_kbps,
        keyframes: r.keyframes,
    }))
}

/// Input for updating video metadata fields from ffprobe.
#[derive(Debug)]
pub struct VideoMetadataUpdate {
    pub video_codec: Option<String>,
    pub video_width: Option<i32>,
    pub video_height: Option<i32>,
    pub video_bitrate_kbps: Option<i32>,
    pub framerate: Option<f64>,
    pub bit_depth: Option<i16>,
    pub is_hdr: bool,
    pub hdr_format: Option<String>,
    pub is_interlaced: bool,
    pub dv_profile: Option<i16>,
    pub dv_bl_compat_id: Option<i16>,
    pub duration_seconds: Option<f64>,
    pub container: Option<String>,
    pub overall_bitrate_kbps: Option<i32>,
}

/// Update video metadata fields on an existing movie file.
pub async fn update_video_metadata<'e, E>(
    exec: E,
    id: Uuid,
    meta: &VideoMetadataUpdate,
) -> Result<bool>
where
    E: Executor<'e, Database = Postgres>,
{
    // Uses runtime query to avoid sqlx cache regeneration requirement
    let result = sqlx::query(
        r#"
        UPDATE movie_files SET
            video_codec = $2,
            video_width = $3,
            video_height = $4,
            video_bitrate_kbps = $5,
            framerate = $6,
            bit_depth = $7,
            is_hdr = $8,
            hdr_format = $9,
            is_interlaced = $10,
            duration_seconds = $11,
            container = $12,
            overall_bitrate_kbps = $13,
            dv_profile = $14,
            dv_bl_compat_id = $15
        WHERE id = $1
        "#,
    )
    .bind(id)
    .bind(meta.video_codec.as_deref())
    .bind(meta.video_width)
    .bind(meta.video_height)
    .bind(meta.video_bitrate_kbps)
    .bind(meta.framerate)
    .bind(meta.bit_depth)
    .bind(meta.is_hdr)
    .bind(meta.hdr_format.as_deref())
    .bind(meta.is_interlaced)
    .bind(meta.duration_seconds)
    .bind(meta.container.as_deref())
    .bind(meta.overall_bitrate_kbps)
    .bind(meta.dv_profile)
    .bind(meta.dv_bl_compat_id)
    .execute(exec)
    .await?;

    Ok(result.rows_affected() > 0)
}

/// Lightweight id + file_path pair for subtitle discovery.
pub struct MovieFileRef {
    pub id: Uuid,
    pub file_path: String,
}

/// List (id, file_path) for all movie files whose path starts with a prefix.
pub async fn list_refs_by_path_prefix<'e, E>(
    exec: E,
    path_prefix: &str,
) -> Result<Vec<MovieFileRef>>
where
    E: Executor<'e, Database = Postgres>,
{
    let pattern = format!("{path_prefix}%");
    let rows = sqlx::query!(
        "SELECT id, file_path FROM movie_files WHERE file_path LIKE $1 AND available = TRUE",
        pattern,
    )
    .fetch_all(exec)
    .await?;

    Ok(rows
        .into_iter()
        .map(|r| MovieFileRef {
            id: r.id,
            file_path: r.file_path,
        })
        .collect())
}

/// Update cached keyframe timestamps for a movie file.
pub async fn update_keyframes<'e, E>(exec: E, id: Uuid, keyframes: &Value) -> Result<bool>
where
    E: Executor<'e, Database = Postgres>,
{
    let result = sqlx::query("UPDATE movie_files SET keyframes = $2 WHERE id = $1")
        .bind(id)
        .bind(keyframes)
        .execute(exec)
        .await?;

    Ok(result.rows_affected() > 0)
}

/// Update the LUFS loudness measurement for a movie file.
pub async fn update_lufs<'e, E>(exec: E, id: Uuid, lufs: Option<f64>) -> Result<bool>
where
    E: Executor<'e, Database = Postgres>,
{
    let result = sqlx::query("UPDATE movie_files SET lufs = $2 WHERE id = $1")
        .bind(id)
        .bind(lufs)
        .execute(exec)
        .await?;
    Ok(result.rows_affected() > 0)
}

/// Mark a file as unreadable by ffprobe so it is excluded from future probes.
/// Sets video_codec to 'unreadable', which causes list_needing_probe
/// (WHERE video_codec IS NULL) to skip it.
pub async fn mark_unreadable<'e, E>(exec: E, id: Uuid) -> Result<bool>
where
    E: Executor<'e, Database = Postgres>,
{
    let result = sqlx::query("UPDATE movie_files SET video_codec = 'unreadable' WHERE id = $1")
        .bind(id)
        .execute(exec)
        .await?;
    Ok(result.rows_affected() > 0)
}

/// Find video files that have never been probed (no video_codec metadata).
/// These need a full media_probe before extraction can run.
/// Joins against media_root_files to exclude files whose source has been
/// marked unavailable by the scanner, even if propagation hasn't run yet.
pub async fn list_needing_probe<'e, E>(exec: E) -> Result<Vec<Uuid>>
where
    E: Executor<'e, Database = Postgres>,
{
    let rows = sqlx::query_scalar(
        "SELECT mf.id FROM movie_files mf \
         JOIN media_root_files mrf ON mrf.file_path = mf.file_path AND mrf.available = TRUE \
         WHERE mf.video_codec IS NULL AND mf.available = TRUE",
    )
    .fetch_all(exec)
    .await?;

    Ok(rows)
}

/// Find video files that have been probed but are missing keyframe data.
/// These can go directly to media_extract (metadata already exists).
pub async fn list_needing_extract<'e, E>(exec: E) -> Result<Vec<Uuid>>
where
    E: Executor<'e, Database = Postgres>,
{
    let rows = sqlx::query_scalar(
        "SELECT mf.id FROM movie_files mf \
         JOIN media_root_files mrf ON mrf.file_path = mf.file_path AND mrf.available = TRUE \
         WHERE mf.keyframes IS NULL AND mf.video_codec IS NOT NULL AND mf.video_codec != 'unreadable' AND mf.available = TRUE \
         AND NOT ( \
             mf.container IN ('mov,mp4,m4a,3gp,3g2,mj2', 'mp4') \
             AND mf.video_codec = 'h264' \
             AND EXISTS (SELECT 1 FROM audio_tracks at2 \
                         WHERE at2.movie_file_id = mf.id \
                         AND at2.codec IN ('aac','opus','flac')) \
         )",
    )
    .fetch_all(exec)
    .await?;

    Ok(rows)
}

/// Bump `media_cache_version` + `media_cache_generated_at` after a successful
/// `media_analyze` pass, so the backfill walker stops re-queuing this file.
pub async fn set_media_cache_version<'e, E>(exec: E, id: Uuid, version: i32) -> Result<()>
where
    E: Executor<'e, Database = Postgres>,
{
    sqlx::query!(
        "UPDATE movie_files SET media_cache_version = $2, media_cache_generated_at = NOW() WHERE id = $1",
        id,
        version,
    )
    .execute(exec)
    .await?;
    Ok(())
}

/// Every available file with its path, for whole-library sweeps.
pub async fn list_available_with_paths<'e, E>(exec: E) -> Result<Vec<(Uuid, String)>>
where
    E: Executor<'e, Database = Postgres>,
{
    let rows = sqlx::query!(
        r#"SELECT id AS "id!", file_path AS "file_path!" FROM movie_files WHERE available = TRUE"#,
    )
    .fetch_all(exec)
    .await?;
    Ok(rows.into_iter().map(|r| (r.id, r.file_path)).collect())
}

/// Movie files eligible for skip detection. See
/// [`crate::repositories::tv_episode_files::list_season_detect_targets`] for
/// what `stale_only` means; movies have no season, so this is simply per file.
pub async fn list_detect_targets<'e, E>(exec: E, stale_only: bool) -> Result<Vec<Uuid>>
where
    E: Executor<'e, Database = Postgres>,
{
    let rows = sqlx::query_scalar!(
        r#"
        SELECT id
          FROM movie_files
         WHERE available = TRUE
           AND media_cache_generated_at IS NOT NULL
           AND (
                NOT $1::bool
             OR skip_detect_ran_at IS NULL
             OR skip_detect_ran_at < media_cache_generated_at
           )
        "#,
        stale_only,
    )
    .fetch_all(exec)
    .await?;
    Ok(rows)
}

/// Stamp `skip_detect_ran_at` after detection completes for a movie file.
pub async fn mark_skip_detect_ran<'e, E>(exec: E, id: Uuid) -> Result<()>
where
    E: Executor<'e, Database = Postgres>,
{
    sqlx::query!(
        "UPDATE movie_files SET skip_detect_ran_at = NOW() WHERE id = $1",
        id,
    )
    .execute(exec)
    .await?;
    Ok(())
}

/// Current media-cache pipeline version recorded for a file (None if the row is
/// gone). 0 means the cache was invalidated and must be regenerated.
pub async fn get_media_cache_version<'e, E>(exec: E, id: Uuid) -> Result<Option<i32>>
where
    E: Executor<'e, Database = Postgres>,
{
    let v = sqlx::query_scalar!(
        "SELECT media_cache_version FROM movie_files WHERE id = $1",
        id,
    )
    .fetch_optional(exec)
    .await?;
    Ok(v)
}

/// Find available movie files whose media cache is older than the current
/// pipeline version. Used by the V2 backfill walker to find files needing
/// `media_analyze` regen.
pub async fn list_needing_media_cache<'e, E>(exec: E, current_version: i32) -> Result<Vec<Uuid>>
where
    E: Executor<'e, Database = Postgres>,
{
    let rows = sqlx::query_scalar!(
        r#"SELECT mf.id FROM movie_files mf
         JOIN media_root_files mrf ON mrf.file_path = mf.file_path AND mrf.available = true
         WHERE mf.media_cache_version < $1
           AND mf.duration_seconds IS NOT NULL
           AND mf.available = true
         ORDER BY mf.created_at DESC"#,
        current_version,
    )
    .fetch_all(exec)
    .await?;
    Ok(rows)
}

/// Every available movie file with a known duration, regardless of
/// `media_cache_version`. Used by the "re-run skip detection on everything"
/// path, where a warm cache is the norm and the point is to re-run the
/// detectors (and supplement embeddings) rather than re-decode.
pub async fn list_all_with_media(
    exec: impl Executor<'_, Database = Postgres>,
) -> Result<Vec<Uuid>> {
    let rows = sqlx::query_scalar!(
        r#"SELECT mf.id FROM movie_files mf
         JOIN media_root_files mrf ON mrf.file_path = mf.file_path AND mrf.available = true
         WHERE mf.duration_seconds IS NOT NULL
           AND mf.available = true
         ORDER BY mf.created_at DESC"#,
    )
    .fetch_all(exec)
    .await?;
    Ok(rows)
}

// =============================================================================
// Library quality
// =============================================================================

/// Fetch the best video_height for a movie's available files.
///
/// Returns `None` if no available files exist (movie not in library).
/// Returns `Some(None)` if files exist but none have been probed yet.
/// Returns `Some(Some(height))` if at least one file has been probed.
pub async fn get_library_video_height<'e, E>(exec: E, movie_id: Uuid) -> Result<Option<Option<i32>>>
where
    E: Executor<'e, Database = Postgres>,
{
    let row = sqlx::query!(
        r#"SELECT
            COUNT(*) AS "count!",
            MAX(video_height) AS "max_height?"
           FROM movie_files
           WHERE movie_id = $1 AND available = TRUE"#,
        movie_id,
    )
    .fetch_one(exec)
    .await?;

    if row.count == 0 {
        Ok(None) // No available files
    } else {
        Ok(Some(row.max_height)) // Some(None) if all unprobed, Some(Some(h)) if probed
    }
}

// =============================================================================
// corrupt_ranges
// =============================================================================

pub async fn get_corrupt_ranges<'e, E>(
    exec: E,
    id: Uuid,
) -> Result<Option<Vec<crate::corruption::CorruptRange>>>
where
    E: Executor<'e, Database = Postgres>,
{
    let row = sqlx::query!("SELECT corrupt_ranges FROM movie_files WHERE id = $1", id,)
        .fetch_optional(exec)
        .await?;
    match row {
        None => Ok(None),
        Some(r) => Ok(Some(
            serde_json::from_value(r.corrupt_ranges).unwrap_or_default(),
        )),
    }
}

pub async fn set_corrupt_ranges<'e, E>(
    exec: E,
    id: Uuid,
    ranges: Vec<crate::corruption::CorruptRange>,
) -> Result<bool>
where
    E: Executor<'e, Database = Postgres>,
{
    let canonical = crate::corruption::merge(ranges, [], crate::corruption::DEFAULT_MERGE_GAP_SECS);
    let json = serde_json::to_value(&canonical).unwrap_or(serde_json::Value::Array(vec![]));
    let result = sqlx::query!(
        "UPDATE movie_files SET corrupt_ranges = $2 WHERE id = $1",
        id,
        json,
    )
    .execute(exec)
    .await?;
    Ok(result.rows_affected() > 0)
}

pub async fn append_corrupt_range(
    pool: &sqlx::PgPool,
    id: Uuid,
    range: crate::corruption::CorruptRange,
) -> Result<bool> {
    let existing = get_corrupt_ranges(pool, id).await?.unwrap_or_default();
    let merged =
        crate::corruption::merge(existing, [range], crate::corruption::DEFAULT_MERGE_GAP_SECS);
    set_corrupt_ranges(pool, id, merged).await
}
