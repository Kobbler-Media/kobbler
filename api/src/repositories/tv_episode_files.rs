//! TV episode files repository operations.
//!
//! Episode-to-file mapping uses the junction table `tv_episode_file_episodes`
//! so one physical file can map to multiple episodes (multi-episode files).

use super::Result;
use crate::file_link_mode::FileLinkMode;
use serde_json::Value;
use sqlx::{Executor, Postgres};
use time::OffsetDateTime;
use uuid::Uuid;

// =============================================================================
// Types
// =============================================================================

/// Input for creating an episode file.
#[derive(Debug)]
pub struct EpisodeFileInput {
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
    pub is_interlaced: bool,
    pub hdr_format: Option<String>,
    pub duration_seconds: Option<f64>,
    pub container: Option<String>,
    pub overall_bitrate_kbps: Option<i32>,
}

/// Row returned from tv_episode_files queries.
#[derive(Debug)]
pub struct EpisodeFileRow {
    pub id: Uuid,
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
    pub duration_seconds: Option<f64>,
    pub container: Option<String>,
    pub overall_bitrate_kbps: Option<i32>,
    pub keyframes: Option<Value>,
}

// =============================================================================
// Junction table: tv_episode_file_episodes
// =============================================================================

/// Replace all episode links for a file with the given set, superseding whatever
/// was on those episodes before. Equivalent to `link_episodes_with_mode` in
/// `FileLinkMode::Replace`, which is what every path except a regrab wants.
pub async fn link_episodes(
    pool: &sqlx::PgPool,
    file_id: Uuid,
    episode_ids: &[Uuid],
) -> Result<Vec<String>> {
    link_episodes_with_mode(pool, file_id, episode_ids, FileLinkMode::Replace).await
}

/// Replace all episode links for a file with the given set.
/// Uses an internal transaction to ensure atomicity.
///
/// In `Replace` mode any existing files exclusive to the target episodes (not
/// shared with other episodes) are deleted first, so the new file replaces them;
/// their paths come back for physical cleanup. In `AddCopy` mode — a regrab —
/// nothing is deleted and the episode ends up with two files, newest preferred.
pub async fn link_episodes_with_mode(
    pool: &sqlx::PgPool,
    file_id: Uuid,
    episode_ids: &[Uuid],
    mode: FileLinkMode,
) -> Result<Vec<String>> {
    let mut tx = pool.begin().await?;

    let replaced = if mode.is_add_copy() {
        Vec::new()
    } else {
        replace_exclusive_files_for_episodes(&mut tx, file_id, episode_ids).await?
    };

    sqlx::query("DELETE FROM tv_episode_file_episodes WHERE episode_file_id = $1")
        .bind(file_id)
        .execute(&mut *tx)
        .await?;

    sqlx::query(
        "INSERT INTO tv_episode_file_episodes (episode_file_id, episode_id) \
         SELECT $1, unnest($2::uuid[]) \
         ON CONFLICT DO NOTHING",
    )
    .bind(file_id)
    .bind(episode_ids)
    .execute(&mut *tx)
    .await?;

    tx.commit().await?;
    Ok(replaced)
}

/// Find files exclusively linked to the given episodes (not shared with other
/// episodes via multi-episode files) and delete them, returning their paths.
///
/// A file is "exclusive" if its total junction row count equals 1, meaning it
/// serves only the one episode we're about to replace.
async fn replace_exclusive_files_for_episodes(
    tx: &mut sqlx::Transaction<'_, Postgres>,
    new_file_id: Uuid,
    episode_ids: &[Uuid],
) -> Result<Vec<String>> {
    let rows: Vec<(Uuid, String)> = sqlx::query_as(
        r#"
        SELECT DISTINCT tef.id, tef.file_path
        FROM tv_episode_file_episodes tefe
        JOIN tv_episode_files tef ON tef.id = tefe.episode_file_id
        WHERE tefe.episode_id = ANY($1)
          AND tef.id != $2
          AND (SELECT COUNT(*) FROM tv_episode_file_episodes sub WHERE sub.episode_file_id = tef.id) = 1
        "#,
    )
    .bind(episode_ids)
    .bind(new_file_id)
    .fetch_all(&mut **tx)
    .await?;

    if rows.is_empty() {
        return Ok(Vec::new());
    }

    let ids: Vec<Uuid> = rows.iter().map(|(id, _)| *id).collect();
    let paths: Vec<String> = rows.into_iter().map(|(_, path)| path).collect();

    for (id, path) in ids.iter().zip(paths.iter()) {
        tracing::info!(old_file_id = %id, old_path = %path, new_file_id = %new_file_id, "Replacing exclusive episode file with new file");
    }

    sqlx::query("DELETE FROM tv_episode_files WHERE id = ANY($1)")
        .bind(&ids)
        .execute(&mut **tx)
        .await?;

    Ok(paths)
}

/// Get all episode IDs linked to a file.
pub async fn get_episode_ids<'e, E>(exec: E, file_id: Uuid) -> Result<Vec<Uuid>>
where
    E: Executor<'e, Database = Postgres>,
{
    let rows = sqlx::query_scalar(
        "SELECT episode_id FROM tv_episode_file_episodes WHERE episode_file_id = $1",
    )
    .bind(file_id)
    .fetch_all(exec)
    .await?;
    Ok(rows)
}

// =============================================================================
// CRUD
// =============================================================================

/// Create a new episode file record.
pub async fn create<'e, E>(exec: E, input: EpisodeFileInput) -> Result<Uuid>
where
    E: Executor<'e, Database = Postgres>,
{
    let id = Uuid::now_v7();

    sqlx::query(
        r#"
        INSERT INTO tv_episode_files (id, file_path, file_size, format, quality,
                                      languages, subtitles, source_type, source_metadata,
                                      video_codec, video_width, video_height, video_bitrate_kbps,
                                      framerate, bit_depth, is_hdr, hdr_format, is_interlaced,
                                      duration_seconds, container, overall_bitrate_kbps, file_fingerprint)
        VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9,
                $10, $11, $12, $13, $14, $15, $16, $17, $18, $19, $20, $21, $22)
        "#,
    )
    .bind(id)
    .bind(&input.file_path)
    .bind(input.file_size)
    .bind(&input.format)
    .bind(&input.quality)
    .bind(input.languages.as_deref())
    .bind(input.subtitles.as_deref())
    .bind(input.source_type.as_deref())
    .bind(&input.source_metadata)
    .bind(input.video_codec.as_deref())
    .bind(input.video_width)
    .bind(input.video_height)
    .bind(input.video_bitrate_kbps)
    .bind(input.framerate)
    .bind(input.bit_depth)
    .bind(input.is_hdr)
    .bind(input.hdr_format.as_deref())
    .bind(input.is_interlaced)
    .bind(input.duration_seconds)
    .bind(input.container.as_deref())
    .bind(input.overall_bitrate_kbps)
    .bind(input.file_fingerprint.as_deref())
    .execute(exec)
    .await?;

    Ok(id)
}

/// Create or update an episode file record (upsert on file_path).
///
/// If a record already exists for this file_path, updates metadata fields
/// but preserves the existing id. Episode links are managed separately via
/// `link_episodes`.
pub async fn create_or_update<'e, E>(exec: E, input: EpisodeFileInput) -> Result<Uuid>
where
    E: Executor<'e, Database = Postgres>,
{
    let id = Uuid::now_v7();

    let row: Uuid = sqlx::query_scalar(
        r#"
        INSERT INTO tv_episode_files (id, file_path, file_size, format, quality,
                                      languages, subtitles, source_type, source_metadata,
                                      video_codec, video_width, video_height, video_bitrate_kbps,
                                      framerate, bit_depth, is_hdr, hdr_format, is_interlaced,
                                      duration_seconds, container, overall_bitrate_kbps, file_fingerprint)
        VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9,
                $10, $11, $12, $13, $14, $15, $16, $17, $18, $19, $20, $21, $22)
        ON CONFLICT (file_path) DO UPDATE SET
            file_size = COALESCE(EXCLUDED.file_size, tv_episode_files.file_size),
            file_fingerprint = COALESCE(EXCLUDED.file_fingerprint, tv_episode_files.file_fingerprint),
            format = EXCLUDED.format,
            quality = COALESCE(EXCLUDED.quality, tv_episode_files.quality),
            languages = COALESCE(EXCLUDED.languages, tv_episode_files.languages),
            subtitles = COALESCE(EXCLUDED.subtitles, tv_episode_files.subtitles),
            source_type = COALESCE(EXCLUDED.source_type, tv_episode_files.source_type),
            source_metadata = COALESCE(EXCLUDED.source_metadata, tv_episode_files.source_metadata),
            -- A file read once and found corrupt is quarantined by writing the
            -- 'unreadable' sentinel into video_codec, which takes it out of
            -- list_needing_probe (that wants a NULL codec). Nothing clears the
            -- sentinel, so replacing the file at the same path would leave the
            -- new copy permanently unprobed: no duration, so never analyzed, so
            -- no cache, so its season's skip detection defers forever. A change
            -- in size means this is not the file we judged -- drop the verdict
            -- and let the probe walker pick it up again. Same size re-scans are
            -- untouched, and a real codec is never cleared.
            video_codec = CASE
                WHEN tv_episode_files.video_codec = 'unreadable'
                 AND EXCLUDED.file_size IS DISTINCT FROM tv_episode_files.file_size
                THEN NULL
                ELSE COALESCE(EXCLUDED.video_codec, tv_episode_files.video_codec)
            END,
            video_width = COALESCE(EXCLUDED.video_width, tv_episode_files.video_width),
            video_height = COALESCE(EXCLUDED.video_height, tv_episode_files.video_height),
            video_bitrate_kbps = COALESCE(EXCLUDED.video_bitrate_kbps, tv_episode_files.video_bitrate_kbps),
            framerate = COALESCE(EXCLUDED.framerate, tv_episode_files.framerate),
            bit_depth = COALESCE(EXCLUDED.bit_depth, tv_episode_files.bit_depth),
            is_hdr = EXCLUDED.is_hdr,
            hdr_format = COALESCE(EXCLUDED.hdr_format, tv_episode_files.hdr_format),
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
                      AND EXCLUDED.file_fingerprint IS DISTINCT FROM tv_episode_files.file_fingerprint)
                     OR (EXCLUDED.file_size IS NOT NULL
                         AND EXCLUDED.file_size IS DISTINCT FROM tv_episode_files.file_size)
                THEN EXCLUDED.duration_seconds
                ELSE COALESCE(EXCLUDED.duration_seconds, tv_episode_files.duration_seconds)
            END,
            container = COALESCE(EXCLUDED.container, tv_episode_files.container),
            overall_bitrate_kbps = COALESCE(EXCLUDED.overall_bitrate_kbps, tv_episode_files.overall_bitrate_kbps),
            keyframes = CASE
                WHEN (EXCLUDED.file_fingerprint IS NOT NULL
                      AND EXCLUDED.file_fingerprint IS DISTINCT FROM tv_episode_files.file_fingerprint)
                     OR (EXCLUDED.file_size IS NOT NULL
                         AND EXCLUDED.file_size IS DISTINCT FROM tv_episode_files.file_size)
                THEN NULL
                ELSE tv_episode_files.keyframes
            END,
            lufs = CASE
                WHEN (EXCLUDED.file_fingerprint IS NOT NULL
                      AND EXCLUDED.file_fingerprint IS DISTINCT FROM tv_episode_files.file_fingerprint)
                     OR (EXCLUDED.file_size IS NOT NULL
                         AND EXCLUDED.file_size IS DISTINCT FROM tv_episode_files.file_size)
                THEN NULL
                ELSE tv_episode_files.lufs
            END,
            corrupt_ranges = CASE
                WHEN (EXCLUDED.file_fingerprint IS NOT NULL
                      AND EXCLUDED.file_fingerprint IS DISTINCT FROM tv_episode_files.file_fingerprint)
                     OR (EXCLUDED.file_size IS NOT NULL
                         AND EXCLUDED.file_size IS DISTINCT FROM tv_episode_files.file_size)
                THEN '[]'::jsonb
                ELSE tv_episode_files.corrupt_ranges
            END,
            -- -1 sentinel = "invalidated by replace", distinct from the default
            -- 0 (legacy / never-analyzed) so media_analyze only force-regenerates
            -- genuinely-replaced files. See db_media_cache_invalidated.
            media_cache_version = CASE
                WHEN (EXCLUDED.file_fingerprint IS NOT NULL
                      AND EXCLUDED.file_fingerprint IS DISTINCT FROM tv_episode_files.file_fingerprint)
                     OR (EXCLUDED.file_size IS NOT NULL
                         AND EXCLUDED.file_size IS DISTINCT FROM tv_episode_files.file_size)
                THEN -1
                ELSE tv_episode_files.media_cache_version
            END,
            available = TRUE
        RETURNING id
        "#,
    )
    .bind(id)
    .bind(&input.file_path)
    .bind(input.file_size)
    .bind(&input.format)
    .bind(&input.quality)
    .bind(input.languages.as_deref())
    .bind(input.subtitles.as_deref())
    .bind(input.source_type.as_deref())
    .bind(&input.source_metadata)
    .bind(input.video_codec.as_deref())
    .bind(input.video_width)
    .bind(input.video_height)
    .bind(input.video_bitrate_kbps)
    .bind(input.framerate)
    .bind(input.bit_depth)
    .bind(input.is_hdr)
    .bind(input.hdr_format.as_deref())
    .bind(input.is_interlaced)
    .bind(input.duration_seconds)
    .bind(input.container.as_deref())
    .bind(input.overall_bitrate_kbps)
    .bind(input.file_fingerprint.as_deref())
    .fetch_one(exec)
    .await?;

    Ok(row)
}

/// Get a single episode file by ID.
pub async fn get<'e, E>(exec: E, id: Uuid) -> Result<Option<EpisodeFileRow>>
where
    E: Executor<'e, Database = Postgres>,
{
    let row = sqlx::query!(
        r#"
        SELECT id, file_path, file_size, file_fingerprint, format, quality,
               languages, subtitles, source_type, source_metadata, available, created_at,
               video_codec, video_width, video_height, video_bitrate_kbps,
               framerate, bit_depth, is_hdr, hdr_format, is_interlaced,
               duration_seconds, container, overall_bitrate_kbps, keyframes
        FROM tv_episode_files
        WHERE id = $1
        "#,
        id,
    )
    .fetch_optional(exec)
    .await?;

    Ok(row.map(|r| EpisodeFileRow {
        id: r.id,
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
        duration_seconds: r.duration_seconds,
        container: r.container,
        overall_bitrate_kbps: r.overall_bitrate_kbps,
        keyframes: r.keyframes,
    }))
}

/// The episodes of a season that currently have an available file.
pub async fn episodes_with_files<'e, E>(exec: E, season_id: Uuid) -> Result<Vec<Uuid>>
where
    E: Executor<'e, Database = Postgres>,
{
    let ids = sqlx::query_scalar!(
        r#"
        SELECT DISTINCT tefe.episode_id AS "episode_id!"
        FROM tv_episode_file_episodes tefe
        JOIN tv_episode_files tef ON tef.id = tefe.episode_file_id
        JOIN tv_episodes e ON e.id = tefe.episode_id
        WHERE e.season_id = $1 AND tef.available
        "#,
        season_id,
    )
    .fetch_all(exec)
    .await?;

    Ok(ids)
}

/// List all files for an episode (via junction table).
pub async fn list_by_episode<'e, E>(exec: E, episode_id: Uuid) -> Result<Vec<EpisodeFileRow>>
where
    E: Executor<'e, Database = Postgres>,
{
    let rows = sqlx::query!(
        r#"
        SELECT tef.id, tef.file_path, tef.file_size, tef.file_fingerprint, tef.format, tef.quality,
               tef.languages, tef.subtitles, tef.source_type, tef.source_metadata,
               tef.available, tef.created_at,
               tef.video_codec, tef.video_width, tef.video_height, tef.video_bitrate_kbps,
               tef.framerate, tef.bit_depth, tef.is_hdr, tef.hdr_format, tef.is_interlaced,
               tef.duration_seconds, tef.container, tef.overall_bitrate_kbps, tef.keyframes
        FROM tv_episode_files tef
        JOIN tv_episode_file_episodes tefe ON tefe.episode_file_id = tef.id
        WHERE tefe.episode_id = $1
        ORDER BY tef.created_at DESC
        "#,
        episode_id,
    )
    .fetch_all(exec)
    .await?;

    Ok(rows
        .into_iter()
        .map(|r| EpisodeFileRow {
            id: r.id,
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
            duration_seconds: r.duration_seconds,
            container: r.container,
            overall_bitrate_kbps: r.overall_bitrate_kbps,
            keyframes: r.keyframes,
        })
        .collect())
}

/// Delete an episode file by ID, returning the linked episode IDs.
/// Junction rows are auto-deleted via CASCADE.
pub async fn delete(pool: &sqlx::PgPool, id: Uuid) -> Result<Option<Vec<Uuid>>> {
    // Fetch linked episodes before deleting (CASCADE will remove junction rows)
    let episode_ids: Vec<Uuid> = sqlx::query_scalar(
        "SELECT episode_id FROM tv_episode_file_episodes WHERE episode_file_id = $1",
    )
    .bind(id)
    .fetch_all(pool)
    .await?;

    let result = sqlx::query("DELETE FROM tv_episode_files WHERE id = $1")
        .bind(id)
        .execute(pool)
        .await?;

    if result.rows_affected() > 0 {
        Ok(Some(episode_ids))
    } else {
        Ok(None)
    }
}

/// Count files for an episode (via junction table).
pub async fn count_by_episode<'e, E>(exec: E, episode_id: Uuid) -> Result<i64>
where
    E: Executor<'e, Database = Postgres>,
{
    let count: i64 = sqlx::query_scalar(
        r#"SELECT COUNT(*) AS "count!" FROM tv_episode_file_episodes WHERE episode_id = $1"#,
    )
    .bind(episode_id)
    .fetch_one(exec)
    .await?;
    Ok(count)
}

/// Replace all episode links for a file, used for moving a file between episodes.
/// Returns false if the file doesn't exist.
pub async fn set_episodes(
    pool: &sqlx::PgPool,
    file_id: Uuid,
    episode_ids: &[Uuid],
) -> Result<bool> {
    let exists: bool =
        sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM tv_episode_files WHERE id = $1)")
            .bind(file_id)
            .fetch_one(pool)
            .await?;

    if !exists {
        return Ok(false);
    }

    let replaced = link_episodes(pool, file_id, episode_ids).await?;
    super::cleanup_replaced_files(&replaced).await;
    Ok(true)
}

/// Set the availability of an episode file.
pub async fn set_available<'e, E>(exec: E, id: Uuid, available: bool) -> Result<bool>
where
    E: Executor<'e, Database = Postgres>,
{
    let result = sqlx::query!(
        "UPDATE tv_episode_files SET available = $2 WHERE id = $1",
        id,
        available,
    )
    .execute(exec)
    .await?;

    Ok(result.rows_affected() > 0)
}

/// The available file the player should use for this episode: the **newest**.
///
/// A regrab adds a second file rather than replacing one, and the decision is
/// that the new copy wins. Subtitle sidecars and NFO paths resolve through here
/// too, so they land beside the file that actually plays.
pub async fn get_preferred_available<'e, E>(
    exec: E,
    episode_id: Uuid,
) -> Result<Option<EpisodeFileRow>>
where
    E: Executor<'e, Database = Postgres>,
{
    let row = sqlx::query!(
        r#"
        SELECT tef.id, tef.file_path, tef.file_size, tef.file_fingerprint, tef.format, tef.quality,
               tef.languages, tef.subtitles, tef.source_type, tef.source_metadata,
               tef.available, tef.created_at,
               tef.video_codec, tef.video_width, tef.video_height, tef.video_bitrate_kbps,
               tef.framerate, tef.bit_depth, tef.is_hdr, tef.hdr_format, tef.is_interlaced,
               tef.duration_seconds, tef.container, tef.overall_bitrate_kbps, tef.keyframes
        FROM tv_episode_files tef
        JOIN tv_episode_file_episodes tefe ON tefe.episode_file_id = tef.id
        WHERE tefe.episode_id = $1 AND tef.available = TRUE
        ORDER BY tef.created_at DESC, tef.id DESC
        LIMIT 1
        "#,
        episode_id,
    )
    .fetch_optional(exec)
    .await?;

    Ok(row.map(|r| EpisodeFileRow {
        id: r.id,
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
    pub duration_seconds: Option<f64>,
    pub container: Option<String>,
    pub overall_bitrate_kbps: Option<i32>,
}

/// Update video metadata fields on an existing episode file.
pub async fn update_video_metadata<'e, E>(
    exec: E,
    id: Uuid,
    meta: &VideoMetadataUpdate,
) -> Result<bool>
where
    E: Executor<'e, Database = Postgres>,
{
    let result = sqlx::query(
        r#"
        UPDATE tv_episode_files SET
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
            overall_bitrate_kbps = $13
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
    .execute(exec)
    .await?;

    Ok(result.rows_affected() > 0)
}

/// Lightweight id + file_path pair for subtitle discovery.
pub struct EpisodeFileRef {
    pub id: Uuid,
    pub file_path: String,
}

/// List (id, file_path) for all episode files whose path starts with a prefix.
pub async fn list_refs_by_path_prefix<'e, E>(
    exec: E,
    path_prefix: &str,
) -> Result<Vec<EpisodeFileRef>>
where
    E: Executor<'e, Database = Postgres>,
{
    let pattern = format!("{path_prefix}%");
    let rows = sqlx::query!(
        "SELECT id, file_path FROM tv_episode_files WHERE file_path LIKE $1 AND available = TRUE",
        pattern,
    )
    .fetch_all(exec)
    .await?;

    Ok(rows
        .into_iter()
        .map(|r| EpisodeFileRef {
            id: r.id,
            file_path: r.file_path,
        })
        .collect())
}

/// Update cached keyframe timestamps for an episode file.
pub async fn update_keyframes<'e, E>(exec: E, id: Uuid, keyframes: &Value) -> Result<bool>
where
    E: Executor<'e, Database = Postgres>,
{
    let result = sqlx::query("UPDATE tv_episode_files SET keyframes = $2 WHERE id = $1")
        .bind(id)
        .bind(keyframes)
        .execute(exec)
        .await?;

    Ok(result.rows_affected() > 0)
}

/// Update the LUFS loudness measurement for an episode file.
pub async fn update_lufs<'e, E>(exec: E, id: Uuid, lufs: Option<f64>) -> Result<bool>
where
    E: Executor<'e, Database = Postgres>,
{
    let result = sqlx::query("UPDATE tv_episode_files SET lufs = $2 WHERE id = $1")
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
    let result =
        sqlx::query("UPDATE tv_episode_files SET video_codec = 'unreadable' WHERE id = $1")
            .bind(id)
            .execute(exec)
            .await?;
    Ok(result.rows_affected() > 0)
}

/// Find video files that have never been probed (no video_codec metadata).
/// These need a full media_probe before extraction can run.
/// Joins against media_root_files to exclude files whose source has been
/// marked unavailable by the scanner.
pub async fn list_needing_probe<'e, E>(exec: E) -> Result<Vec<Uuid>>
where
    E: Executor<'e, Database = Postgres>,
{
    let rows = sqlx::query_scalar(
        "SELECT ef.id FROM tv_episode_files ef \
         JOIN media_root_files mrf ON mrf.file_path = ef.file_path AND mrf.available = TRUE \
         WHERE ef.video_codec IS NULL AND ef.available = TRUE",
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
        "SELECT ef.id FROM tv_episode_files ef \
         JOIN media_root_files mrf ON mrf.file_path = ef.file_path AND mrf.available = TRUE \
         WHERE ef.keyframes IS NULL AND ef.video_codec IS NOT NULL AND ef.video_codec != 'unreadable' AND ef.available = TRUE \
         AND NOT ( \
             ef.container IN ('mov,mp4,m4a,3gp,3g2,mj2', 'mp4') \
             AND ef.video_codec = 'h264' \
             AND EXISTS (SELECT 1 FROM audio_tracks at2 \
                         WHERE at2.episode_file_id = ef.id \
                         AND at2.codec IN ('aac','opus','flac')) \
         )",
    )
    .fetch_all(exec)
    .await?;

    Ok(rows)
}

/// Bump `media_cache_version` + `media_cache_generated_at` after a successful
/// `media_analyze` pass.
pub async fn set_media_cache_version<'e, E>(exec: E, id: Uuid, version: i32) -> Result<()>
where
    E: Executor<'e, Database = Postgres>,
{
    sqlx::query!(
        "UPDATE tv_episode_files SET media_cache_version = $2, media_cache_generated_at = NOW() WHERE id = $1",
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
        r#"SELECT id AS "id!", file_path AS "file_path!" FROM tv_episode_files WHERE available = TRUE"#,
    )
    .fetch_all(exec)
    .await?;
    Ok(rows.into_iter().map(|r| (r.id, r.file_path)).collect())
}

/// Of `ids`, those whose probe concluded the media itself is unreadable.
///
/// `media_probe` writes the `'unreadable'` sentinel into `video_codec` only
/// after `is_file()` succeeded and ffprobe still could not parse the file --
/// a root that is unmounted or offline fails the `is_file` guard and errors
/// the job instead, so this specifically means "we read it and it is broken",
/// not "we could not reach it".
pub async fn list_unreadable<'e, E>(exec: E, ids: &[Uuid]) -> Result<Vec<Uuid>>
where
    E: Executor<'e, Database = Postgres>,
{
    if ids.is_empty() {
        return Ok(Vec::new());
    }
    let rows = sqlx::query_scalar!(
        "SELECT id FROM tv_episode_files WHERE id = ANY($1) AND video_codec = 'unreadable'",
        ids,
    )
    .fetch_all(exec)
    .await?;
    Ok(rows)
}

/// Stamp `skip_detect_ran_at` for every file whose detection just completed.
///
/// Called once per season, with the whole sibling set, because detection is
/// season-scoped: one job produces (or deliberately declines to produce)
/// segments for every episode it looked at, so every one of them has been
/// detected as of now. Stamping only the file that happened to trigger the job
/// would leave its siblings looking stale forever.
pub async fn mark_skip_detect_ran<'e, E>(exec: E, ids: &[Uuid]) -> Result<u64>
where
    E: Executor<'e, Database = Postgres>,
{
    if ids.is_empty() {
        return Ok(0);
    }
    let res = sqlx::query!(
        "UPDATE tv_episode_files SET skip_detect_ran_at = NOW() WHERE id = ANY($1)",
        ids,
    )
    .execute(exec)
    .await?;
    Ok(res.rows_affected())
}

/// Current media-cache pipeline version recorded for a file (None if the row is
/// gone). 0 means the cache was invalidated and must be regenerated.
pub async fn get_media_cache_version<'e, E>(exec: E, id: Uuid) -> Result<Option<i32>>
where
    E: Executor<'e, Database = Postgres>,
{
    let v = sqlx::query_scalar!(
        "SELECT media_cache_version FROM tv_episode_files WHERE id = $1",
        id,
    )
    .fetch_optional(exec)
    .await?;
    Ok(v)
}

/// Find available episode files whose media cache is older than the current
/// pipeline version. Used by the V2 backfill walker.
pub async fn list_needing_media_cache<'e, E>(exec: E, current_version: i32) -> Result<Vec<Uuid>>
where
    E: Executor<'e, Database = Postgres>,
{
    let rows = sqlx::query_scalar!(
        r#"SELECT ef.id FROM tv_episode_files ef
         JOIN media_root_files mrf ON mrf.file_path = ef.file_path AND mrf.available = true
         WHERE ef.media_cache_version < $1
           AND ef.duration_seconds IS NOT NULL
           AND ef.available = true
         ORDER BY ef.created_at DESC"#,
        current_version,
    )
    .fetch_all(exec)
    .await?;
    Ok(rows)
}

/// Every available episode file with a known duration, regardless of
/// `media_cache_version`. Counterpart to [`list_needing_media_cache`] for the
/// "re-run skip detection on everything" path.
pub async fn list_all_with_media(
    exec: impl Executor<'_, Database = Postgres>,
) -> Result<Vec<Uuid>> {
    let rows = sqlx::query_scalar!(
        r#"SELECT ef.id FROM tv_episode_files ef
         JOIN media_root_files mrf ON mrf.file_path = ef.file_path AND mrf.available = true
         WHERE ef.duration_seconds IS NOT NULL
           AND ef.available = true
         ORDER BY ef.created_at DESC"#,
    )
    .fetch_all(exec)
    .await?;
    Ok(rows)
}

/// List all episode file IDs for a season (via episodes → junction → files).
/// One representative episode file per season, for enqueueing season-scoped
/// skip detection.
///
/// Detection covers a whole season per job, so the batch path wants one job per
/// season rather than one per file. `stale_only` restricts to seasons holding at
/// least one file whose cache is newer than its last detection (or that has
/// never been detected); `false` returns every season with a usable cache, which
/// is what a forced re-run wants.
pub async fn list_season_detect_targets<'e, E>(
    exec: E,
    stale_only: bool,
) -> Result<Vec<(Uuid, Uuid)>>
where
    E: Executor<'e, Database = Postgres>,
{
    let rows = sqlx::query!(
        r#"
        SELECT DISTINCT ON (te.season_id)
               te.season_id AS "season_id!",
               tef.id       AS "file_id!"
          FROM tv_episode_files tef
          JOIN tv_episode_file_episodes tefe ON tefe.episode_file_id = tef.id
          JOIN tv_episodes te ON te.id = tefe.episode_id
         WHERE tef.available = TRUE
           AND tef.media_cache_generated_at IS NOT NULL
           AND (
                NOT $1::bool
             OR tef.skip_detect_ran_at IS NULL
             OR tef.skip_detect_ran_at < tef.media_cache_generated_at
           )
         ORDER BY te.season_id, tef.id
        "#,
        stale_only,
    )
    .fetch_all(exec)
    .await?;
    Ok(rows.into_iter().map(|r| (r.season_id, r.file_id)).collect())
}

/// Season a given episode file belongs to, if any.
///
/// A file can be linked to several episodes (multi-episode files), which all
/// belong to the same season in practice, so the first match is the answer.
pub async fn season_id_for_file<'e, E>(exec: E, file_id: Uuid) -> Result<Option<Uuid>>
where
    E: Executor<'e, Database = Postgres>,
{
    let id = sqlx::query_scalar!(
        r#"
        SELECT te.season_id
          FROM tv_episode_files tef
          JOIN tv_episode_file_episodes tefe ON tefe.episode_file_id = tef.id
          JOIN tv_episodes te ON te.id = tefe.episode_id
         WHERE tef.id = $1
         LIMIT 1
        "#,
        file_id,
    )
    .fetch_optional(exec)
    .await?;
    Ok(id)
}

pub async fn list_file_ids_by_season<'e, E>(exec: E, season_id: Uuid) -> Result<Vec<Uuid>>
where
    E: Executor<'e, Database = Postgres>,
{
    let rows = sqlx::query_scalar!(
        r#"
        SELECT DISTINCT tef.id
        FROM tv_episode_files tef
        JOIN tv_episode_file_episodes tefe ON tefe.episode_file_id = tef.id
        JOIN tv_episodes e ON e.id = tefe.episode_id
        WHERE e.season_id = $1 AND tef.available = TRUE
        "#,
        season_id,
    )
    .fetch_all(exec)
    .await?;

    Ok(rows)
}

// =============================================================================
// Library quality
// =============================================================================

/// Lightweight quality info for library-awareness in download decisions.
pub struct EpisodeLibraryQuality {
    pub episode_id: Uuid,
    /// Best video height across available files. `None` if all files are unprobed.
    pub video_height: Option<i32>,
}

/// Batch-fetch the best video_height for episodes that have available files.
///
/// Returns one entry per episode that has at least one available file.
/// `video_height` is `None` when files exist but none have been probed yet.
/// Episodes with no available files at all are omitted from the result.
pub async fn get_library_quality_batch<'e, E>(
    exec: E,
    episode_ids: &[Uuid],
) -> Result<Vec<EpisodeLibraryQuality>>
where
    E: Executor<'e, Database = Postgres>,
{
    let rows = sqlx::query_as!(
        EpisodeLibraryQuality,
        r#"SELECT tefe.episode_id, MAX(tef.video_height) AS "video_height?"
           FROM tv_episode_file_episodes tefe
           JOIN tv_episode_files tef ON tef.id = tefe.episode_file_id
           WHERE tefe.episode_id = ANY($1) AND tef.available = TRUE
           GROUP BY tefe.episode_id"#,
        episode_ids,
    )
    .fetch_all(exec)
    .await?;

    Ok(rows)
}

/// File ID + path pair for bulk cleanup.
pub struct FilePathEntry {
    pub id: Uuid,
    pub file_path: String,
}

/// List all episode file paths for a TV show (across all seasons/episodes).
pub async fn list_paths_by_show<'e, E>(exec: E, show_id: Uuid) -> Result<Vec<FilePathEntry>>
where
    E: Executor<'e, Database = Postgres>,
{
    let rows = sqlx::query_as!(
        FilePathEntry,
        r#"
        SELECT DISTINCT tef.id, tef.file_path
        FROM tv_episode_files tef
        JOIN tv_episode_file_episodes tefe ON tefe.episode_file_id = tef.id
        JOIN tv_episodes te ON te.id = tefe.episode_id
        JOIN tv_seasons ts ON ts.id = te.season_id
        WHERE ts.show_id = $1
        "#,
        show_id,
    )
    .fetch_all(exec)
    .await?;

    Ok(rows)
}

/// List all episode file paths for a season (across all episodes).
pub async fn list_paths_by_season<'e, E>(exec: E, season_id: Uuid) -> Result<Vec<FilePathEntry>>
where
    E: Executor<'e, Database = Postgres>,
{
    let rows = sqlx::query_as!(
        FilePathEntry,
        r#"
        SELECT DISTINCT tef.id, tef.file_path
        FROM tv_episode_files tef
        JOIN tv_episode_file_episodes tefe ON tefe.episode_file_id = tef.id
        JOIN tv_episodes te ON te.id = tefe.episode_id
        WHERE te.season_id = $1
        "#,
        season_id,
    )
    .fetch_all(exec)
    .await?;

    Ok(rows)
}

/// List the distinct files linked to any of `episode_ids`. A file shared by a
/// double episode appears once, even when both halves are in the set.
pub async fn list_paths_by_episodes<'e, E>(
    exec: E,
    episode_ids: &[Uuid],
) -> Result<Vec<FilePathEntry>>
where
    E: Executor<'e, Database = Postgres>,
{
    let rows = sqlx::query_as!(
        FilePathEntry,
        r#"
        SELECT DISTINCT tef.id, tef.file_path
        FROM tv_episode_files tef
        JOIN tv_episode_file_episodes tefe ON tefe.episode_file_id = tef.id
        WHERE tefe.episode_id = ANY($1)
        "#,
        episode_ids,
    )
    .fetch_all(exec)
    .await?;

    Ok(rows)
}

/// Delete an explicit set of episode files. Junction rows go via CASCADE.
/// Returns the number of rows removed.
pub async fn delete_many<'e, E>(exec: E, ids: &[Uuid]) -> Result<u64>
where
    E: Executor<'e, Database = Postgres>,
{
    let result = sqlx::query!("DELETE FROM tv_episode_files WHERE id = ANY($1)", ids)
        .execute(exec)
        .await?;

    Ok(result.rows_affected())
}

// =============================================================================
// corrupt_ranges
// =============================================================================

/// Read the stored `corrupt_ranges` for an episode file. Returns an empty Vec
/// if the row exists but no ranges are set.
pub async fn get_corrupt_ranges<'e, E>(
    exec: E,
    id: Uuid,
) -> Result<Option<Vec<crate::corruption::CorruptRange>>>
where
    E: Executor<'e, Database = Postgres>,
{
    let row = sqlx::query!(
        "SELECT corrupt_ranges FROM tv_episode_files WHERE id = $1",
        id,
    )
    .fetch_optional(exec)
    .await?;
    match row {
        None => Ok(None),
        Some(r) => Ok(Some(
            serde_json::from_value(r.corrupt_ranges).unwrap_or_default(),
        )),
    }
}

/// Replace the corrupt_ranges array entirely (used by media_analyze after a
/// fresh full-file decode). Input is canonicalised via `merge` so adjacent
/// entries from the analysis run are coalesced before write.
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
        "UPDATE tv_episode_files SET corrupt_ranges = $2 WHERE id = $1",
        id,
        json,
    )
    .execute(exec)
    .await?;
    Ok(result.rows_affected() > 0)
}

/// Append a new range to the existing list, coalescing overlaps (used by
/// Tier 3 `POST /sessions/:id/decode-error`).
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
