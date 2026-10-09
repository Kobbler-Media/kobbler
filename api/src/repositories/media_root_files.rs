//! Media root file repository operations.

use super::Result;
use serde::Serialize;
use serde_json::{Value, json};
use sqlx::{Executor, PgPool, Postgres, Row};
use std::collections::HashMap;
use time::OffsetDateTime;
use uuid::Uuid;

// =============================================================================
// Types
// =============================================================================

/// Input for creating a media root file record.
#[derive(Debug)]
pub struct MediaRootFileInput {
    pub media_root_id: Uuid,
    pub file_path: String,
    pub file_name: String,
    pub file_size: Option<i64>,
    pub file_mtime: Option<OffsetDateTime>,
    pub file_fingerprint: Option<String>,
}

/// Convert a filesystem `SystemTime` to an `OffsetDateTime` truncated to
/// microsecond precision. PostgreSQL `TIMESTAMPTZ` stores microseconds, so
/// without truncation a round-trip (write → read → compare) will always fail
/// on the nanosecond remainder.
pub fn mtime_from_system_time(t: std::time::SystemTime) -> OffsetDateTime {
    let odt = OffsetDateTime::from(t);
    odt.replace_nanosecond(odt.nanosecond() / 1000 * 1000)
        .unwrap_or(odt)
}

/// A lightweight media root file row (no match_candidates / sidecar data).
#[derive(Debug, Serialize)]
pub struct MediaRootFileLiteRow {
    pub id: Uuid,
    pub media_root_id: Uuid,
    pub file_path: String,
    pub file_name: String,
    pub file_size: Option<i64>,
    pub match_status: String,
    pub entity_type: Option<String>,
    pub entity_id: Option<Uuid>,
    pub interpretation: Option<Value>,
    pub available: bool,
    pub created_at: OffsetDateTime,
}

/// Summary of an entity group (e.g. a TV show, movie title, book series).
///
/// Groups are match-based: review/ambiguous files are grouped by their top
/// match candidate's `provider:provider_id`, and added files are grouped by
/// their `entity_type:entity_id`.
#[derive(Debug, Serialize)]
pub struct EntityGroupSummary {
    pub group_key: String,
    pub label: String,
    pub year: Option<i32>,
    pub file_count: i64,
    pub review_count: i64,
    pub unmatched_count: i64,
    pub ambiguous_count: i64,
    pub added_count: i64,
    pub poster_url: Option<String>,
    pub subgroups: Vec<EntitySubgroupSummary>,
    /// Overall status: "review", "ambiguous", "added", or "mixed".
    pub group_status: String,
    /// Authors (books/audiobooks only).
    pub authors: Option<Vec<String>>,
    /// Series name, if part of a series.
    pub series_name: Option<String>,
    /// Position within series.
    pub series_index: Option<f64>,
    /// Match provider name (e.g. "hardcover", "tmdb").
    pub provider: Option<String>,
    /// Match provider ID.
    pub provider_id: Option<String>,
    /// Minimum confidence across files in the group (weakest link).
    pub confidence: Option<f64>,
    /// Cover/poster URL from candidate or entity.
    pub cover_url: Option<String>,
}

/// Summary of a subgroup within an entity group (e.g. a TV season).
#[derive(Debug, Serialize)]
pub struct EntitySubgroupSummary {
    pub season: i32,
    pub label: String,
    pub file_count: i64,
    pub review_count: i64,
}

/// A media root file row.
#[derive(Debug, Serialize)]
pub struct MediaRootFileRow {
    pub id: Uuid,
    pub media_root_id: Uuid,
    pub file_path: String,
    pub file_name: String,
    pub file_size: Option<i64>,
    pub file_mtime: Option<OffsetDateTime>,
    pub file_fingerprint: Option<String>,
    pub match_status: String,
    pub entity_type: Option<String>,
    pub entity_id: Option<Uuid>,
    pub interpretation: Option<Value>,
    pub match_candidates: Option<Value>,
    pub sidecar_metadata: Option<Value>,
    pub sidecar_images: Option<Value>,
    pub available: bool,
    pub created_at: OffsetDateTime,
    pub updated_at: OffsetDateTime,
}

fn row_to_file(r: sqlx::postgres::PgRow) -> MediaRootFileRow {
    MediaRootFileRow {
        id: r.get("id"),
        media_root_id: r.get("media_root_id"),
        file_path: r.get("file_path"),
        file_name: r.get("file_name"),
        file_size: r.get("file_size"),
        file_mtime: r.get("file_mtime"),
        file_fingerprint: r.get("file_fingerprint"),
        match_status: r.get("match_status"),
        entity_type: r.get("entity_type"),
        entity_id: r.get("entity_id"),
        interpretation: r.get("interpretation"),
        match_candidates: r.get("match_candidates"),
        sidecar_metadata: r.get("sidecar_metadata"),
        sidecar_images: r.get("sidecar_images"),
        available: r.get("available"),
        created_at: r.get("created_at"),
        updated_at: r.get("updated_at"),
    }
}

const SELECT_COLS: &str = r#"
    id, media_root_id, file_path, file_name, file_size, file_mtime, file_fingerprint,
    match_status, entity_type, entity_id, interpretation, match_candidates,
    sidecar_metadata, sidecar_images, available, created_at, updated_at
"#;

const SELECT_COLS_LITE: &str = r#"
    id, media_root_id, file_path, file_name, file_size,
    match_status, entity_type, entity_id, interpretation,
    available, created_at
"#;

/// Same columns but qualified with `mrf.` for use in JOINed queries.
const SELECT_COLS_LITE_MRF: &str = r#"
    mrf.id, mrf.media_root_id, mrf.file_path, mrf.file_name, mrf.file_size,
    mrf.match_status, mrf.entity_type, mrf.entity_id, mrf.interpretation,
    mrf.available, mrf.created_at
"#;

fn row_to_file_lite(r: sqlx::postgres::PgRow) -> MediaRootFileLiteRow {
    MediaRootFileLiteRow {
        id: r.get("id"),
        media_root_id: r.get("media_root_id"),
        file_path: r.get("file_path"),
        file_name: r.get("file_name"),
        file_size: r.get("file_size"),
        match_status: r.get("match_status"),
        entity_type: r.get("entity_type"),
        entity_id: r.get("entity_id"),
        interpretation: r.get("interpretation"),
        available: r.get("available"),
        created_at: r.get("created_at"),
    }
}

// =============================================================================
// CRUD
// =============================================================================

/// Upsert a file by path (insert or update mtime/size if changed).
///
/// When size or mtime change, stale metadata (interpretation, entity link,
/// match candidates, sidecar data) is cleared and match_status reset to
/// 'pending' so the file gets re-interpreted. However, if a fingerprint is
/// provided and matches the existing fingerprint, the content hasn't actually
/// changed (e.g. `touch`, backup restore) so metadata is preserved.
///
/// Returns (id, is_new).
pub async fn upsert_by_path<'e, E>(exec: E, input: MediaRootFileInput) -> Result<(Uuid, bool)>
where
    E: Executor<'e, Database = Postgres>,
{
    let id = Uuid::now_v7();
    // "content_changed" = (size OR mtime differs) AND fingerprint doesn't prove same content.
    // When fingerprint is NULL on either side, we conservatively treat it as changed.
    let row = sqlx::query(
        r#"
        INSERT INTO media_root_files (id, media_root_id, file_path, file_name, file_size, file_mtime, file_fingerprint)
        VALUES ($1, $2, $3, $4, $5, $6, $7)
        ON CONFLICT (file_path) DO UPDATE SET
            file_size = EXCLUDED.file_size,
            file_mtime = EXCLUDED.file_mtime,
            file_fingerprint = COALESCE(EXCLUDED.file_fingerprint, media_root_files.file_fingerprint),
            available = TRUE,
            match_status = CASE
                WHEN (media_root_files.file_mtime IS DISTINCT FROM EXCLUDED.file_mtime
                      OR media_root_files.file_size IS DISTINCT FROM EXCLUDED.file_size)
                     AND (media_root_files.file_fingerprint IS NULL
                          OR EXCLUDED.file_fingerprint IS NULL
                          OR media_root_files.file_fingerprint IS DISTINCT FROM EXCLUDED.file_fingerprint)
                THEN 'pending'
                ELSE media_root_files.match_status
            END,
            interpretation = CASE
                WHEN (media_root_files.file_mtime IS DISTINCT FROM EXCLUDED.file_mtime
                      OR media_root_files.file_size IS DISTINCT FROM EXCLUDED.file_size)
                     AND (media_root_files.file_fingerprint IS NULL
                          OR EXCLUDED.file_fingerprint IS NULL
                          OR media_root_files.file_fingerprint IS DISTINCT FROM EXCLUDED.file_fingerprint)
                THEN NULL ELSE media_root_files.interpretation END,
            match_candidates = CASE
                WHEN (media_root_files.file_mtime IS DISTINCT FROM EXCLUDED.file_mtime
                      OR media_root_files.file_size IS DISTINCT FROM EXCLUDED.file_size)
                     AND (media_root_files.file_fingerprint IS NULL
                          OR EXCLUDED.file_fingerprint IS NULL
                          OR media_root_files.file_fingerprint IS DISTINCT FROM EXCLUDED.file_fingerprint)
                THEN NULL ELSE media_root_files.match_candidates END,
            entity_type = CASE
                WHEN (media_root_files.file_mtime IS DISTINCT FROM EXCLUDED.file_mtime
                      OR media_root_files.file_size IS DISTINCT FROM EXCLUDED.file_size)
                     AND (media_root_files.file_fingerprint IS NULL
                          OR EXCLUDED.file_fingerprint IS NULL
                          OR media_root_files.file_fingerprint IS DISTINCT FROM EXCLUDED.file_fingerprint)
                THEN NULL ELSE media_root_files.entity_type END,
            entity_id = CASE
                WHEN (media_root_files.file_mtime IS DISTINCT FROM EXCLUDED.file_mtime
                      OR media_root_files.file_size IS DISTINCT FROM EXCLUDED.file_size)
                     AND (media_root_files.file_fingerprint IS NULL
                          OR EXCLUDED.file_fingerprint IS NULL
                          OR media_root_files.file_fingerprint IS DISTINCT FROM EXCLUDED.file_fingerprint)
                THEN NULL ELSE media_root_files.entity_id END,
            sidecar_metadata = CASE
                WHEN (media_root_files.file_mtime IS DISTINCT FROM EXCLUDED.file_mtime
                      OR media_root_files.file_size IS DISTINCT FROM EXCLUDED.file_size)
                     AND (media_root_files.file_fingerprint IS NULL
                          OR EXCLUDED.file_fingerprint IS NULL
                          OR media_root_files.file_fingerprint IS DISTINCT FROM EXCLUDED.file_fingerprint)
                THEN NULL ELSE media_root_files.sidecar_metadata END,
            sidecar_images = CASE
                WHEN (media_root_files.file_mtime IS DISTINCT FROM EXCLUDED.file_mtime
                      OR media_root_files.file_size IS DISTINCT FROM EXCLUDED.file_size)
                     AND (media_root_files.file_fingerprint IS NULL
                          OR EXCLUDED.file_fingerprint IS NULL
                          OR media_root_files.file_fingerprint IS DISTINCT FROM EXCLUDED.file_fingerprint)
                THEN NULL ELSE media_root_files.sidecar_images END,
            updated_at = NOW()
        RETURNING id, (xmax = 0) AS is_new
        "#,
    )
    .bind(id)
    .bind(input.media_root_id)
    .bind(&input.file_path)
    .bind(&input.file_name)
    .bind(input.file_size)
    .bind(input.file_mtime)
    .bind(&input.file_fingerprint)
    .fetch_one(exec)
    .await?;

    Ok((row.get("id"), row.get("is_new")))
}

/// Create or upgrade a media_root_files entry directly as 'added' with entity linkage.
///
/// Used by ingest paths that place files directly into a media root so they're
/// immediately visible to the media root system (reorganize, sidecar writing, etc.)
/// without waiting for a scan cycle.
///
/// Race-safe: if a scan already created a 'pending' entry, this upgrades it to 'added'.
/// If this runs first, the scan's `upsert_by_path` preserves the 'added' status.
pub async fn upsert_added(
    pool: &PgPool,
    media_root_id: Uuid,
    file_path: &str,
    file_name: &str,
    file_size: Option<i64>,
    entity_type: &str,
    entity_id: Uuid,
) -> Result<Uuid> {
    let id = Uuid::now_v7();
    let row = sqlx::query(
        r#"
        INSERT INTO media_root_files
            (id, media_root_id, file_path, file_name, file_size, match_status, entity_type, entity_id)
        VALUES ($1, $2, $3, $4, $5, 'added', $6, $7)
        ON CONFLICT (file_path) DO UPDATE SET
            file_size = EXCLUDED.file_size,
            match_status = 'added',
            entity_type = EXCLUDED.entity_type,
            entity_id = EXCLUDED.entity_id,
            available = TRUE,
            updated_at = NOW()
        RETURNING id
        "#,
    )
    .bind(id)
    .bind(media_root_id)
    .bind(file_path)
    .bind(file_name)
    .bind(file_size)
    .bind(entity_type)
    .bind(entity_id)
    .fetch_one(pool)
    .await?;

    Ok(row.get("id"))
}

/// If an entity file row (audiobook_files / book_files / movie_files /
/// tv_episode_files) already references this exact path, mark the
/// media_root_files row as `added` and link to that entity. Used by the scan
/// path so previously-matched files don't get dragged back into review when
/// their interpretation/match data is cleared (e.g. fingerprint reset).
///
/// Returns Ok(true) when the row was upgraded.
pub async fn attach_to_existing_entity_if_present<'e, E>(
    exec: E,
    id: Uuid,
    file_path: &str,
    media_type: &str,
) -> Result<bool>
where
    E: Executor<'e, Database = Postgres>,
{
    // Per-media-type lookup keeps the planner honest (one indexed equality
    // probe each, not a four-way UNION). `media_type` is from a hardcoded enum
    // upstream, not user input, so the literal SQL match is fine.
    let lookup_sql = match media_type {
        "book" | "audiobook" => {
            r#"
            SELECT 'book'::text AS et, book_id AS eid FROM audiobook_files
              WHERE file_path = $2 AND available = TRUE
            UNION ALL
            SELECT 'book', book_id FROM book_files
              WHERE file_path = $2 AND available = TRUE
            LIMIT 1
            "#
        }
        "movie" => {
            r#"
            SELECT 'movie'::text AS et, movie_id AS eid FROM movie_files
              WHERE file_path = $2 AND available = TRUE
            LIMIT 1
            "#
        }
        "tv" => {
            r#"
            SELECT 'tv_episode'::text AS et, tefe.episode_id AS eid
              FROM tv_episode_files ef
              JOIN tv_episode_file_episodes tefe ON tefe.episode_file_id = ef.id
              WHERE ef.file_path = $2 AND ef.available = TRUE
            LIMIT 1
            "#
        }
        _ => return Ok(false),
    };

    let sql = format!(
        r#"
        WITH found AS ({lookup_sql})
        UPDATE media_root_files SET
            match_status = 'added',
            entity_type = (SELECT et FROM found),
            entity_id = (SELECT eid FROM found),
            match_candidates = NULL,
            updated_at = NOW()
        WHERE id = $1
          AND match_status NOT IN ('added', 'ignored')
          AND EXISTS (SELECT 1 FROM found)
        "#
    );

    let res = sqlx::query(sqlx::AssertSqlSafe(sql.as_str()))
        .bind(id)
        .bind(file_path)
        .execute(exec)
        .await?;
    Ok(res.rows_affected() > 0)
}

/// Known state for a single file: (size, mtime, fingerprint).
pub type KnownFileState = (Option<i64>, Option<OffsetDateTime>, Option<String>);

/// Load known file state for a media root (path → size, mtime, fingerprint).
///
/// Used by the scan handler to skip processing unchanged files.
pub async fn get_known_file_state(
    pool: &PgPool,
    media_root_id: Uuid,
) -> Result<HashMap<String, KnownFileState>> {
    let rows = sqlx::query(
        "SELECT file_path, file_size, file_mtime, file_fingerprint FROM media_root_files \
         WHERE media_root_id = $1 AND available = TRUE",
    )
    .bind(media_root_id)
    .fetch_all(pool)
    .await?;

    let mut map = HashMap::with_capacity(rows.len());
    for row in rows {
        let path: String = row.get("file_path");
        let size: Option<i64> = row.get("file_size");
        let mtime: Option<OffsetDateTime> = row.get("file_mtime");
        let fingerprint: Option<String> = row.get("file_fingerprint");
        map.insert(path, (size, mtime, fingerprint));
    }
    Ok(map)
}

/// Get a file by ID.
pub async fn get_by_id<'e, E>(exec: E, id: Uuid) -> Result<Option<MediaRootFileRow>>
where
    E: Executor<'e, Database = Postgres>,
{
    let row = sqlx::query(sqlx::AssertSqlSafe(format!(
        "SELECT {} FROM media_root_files WHERE id = $1",
        SELECT_COLS
    )))
    .bind(id)
    .fetch_optional(exec)
    .await?;

    Ok(row.map(row_to_file))
}

/// Get files linked to a specific entity (movie, tv_episode, book).
pub async fn get_by_entity<'e, E>(
    exec: E,
    entity_type: &str,
    entity_id: Uuid,
) -> Result<Vec<MediaRootFileRow>>
where
    E: Executor<'e, Database = Postgres>,
{
    let rows = sqlx::query(sqlx::AssertSqlSafe(format!(
        "SELECT {} FROM media_root_files WHERE entity_type = $1 AND entity_id = $2",
        SELECT_COLS
    )))
    .bind(entity_type)
    .bind(entity_id)
    .fetch_all(exec)
    .await?;

    Ok(rows.into_iter().map(row_to_file).collect())
}

/// List files for a media root with optional status filter.
pub async fn list_for_root<'e, E>(
    exec: E,
    media_root_id: Uuid,
    status_filter: Option<&str>,
    limit: i64,
    offset: i64,
) -> Result<Vec<MediaRootFileRow>>
where
    E: Executor<'e, Database = Postgres>,
{
    let rows = sqlx::query(sqlx::AssertSqlSafe(format!(
        r#"
        SELECT {} FROM media_root_files
        WHERE media_root_id = $1
          AND ($2::text IS NULL OR match_status = $2)
          AND available = TRUE
        ORDER BY file_path
        LIMIT $3 OFFSET $4
        "#,
        SELECT_COLS
    )))
    .bind(media_root_id)
    .bind(status_filter)
    .bind(limit)
    .bind(offset)
    .fetch_all(exec)
    .await?;

    Ok(rows.into_iter().map(row_to_file).collect())
}

/// Get pending files for a root (for interpretation).
pub async fn get_pending<'e, E>(
    exec: E,
    media_root_id: Uuid,
    limit: i64,
) -> Result<Vec<MediaRootFileRow>>
where
    E: Executor<'e, Database = Postgres>,
{
    let rows = sqlx::query(sqlx::AssertSqlSafe(format!(
        r#"
        SELECT {} FROM media_root_files
        WHERE media_root_id = $1 AND match_status = 'pending' AND available = TRUE
        ORDER BY file_path
        LIMIT $2
        "#,
        SELECT_COLS
    )))
    .bind(media_root_id)
    .bind(limit)
    .fetch_all(exec)
    .await?;

    Ok(rows.into_iter().map(row_to_file).collect())
}

/// Get identified files for a root (for matching).
pub async fn get_identified<'e, E>(
    exec: E,
    media_root_id: Uuid,
    limit: i64,
) -> Result<Vec<MediaRootFileRow>>
where
    E: Executor<'e, Database = Postgres>,
{
    let rows = sqlx::query(sqlx::AssertSqlSafe(format!(
        r#"
        SELECT {} FROM media_root_files
        WHERE media_root_id = $1 AND match_status = 'identified' AND available = TRUE
        ORDER BY file_path
        LIMIT $2
        "#,
        SELECT_COLS
    )))
    .bind(media_root_id)
    .bind(limit)
    .fetch_all(exec)
    .await?;

    Ok(rows.into_iter().map(row_to_file).collect())
}

// =============================================================================
// Status updates
// =============================================================================

/// Set sidecar metadata on a file.
pub async fn set_sidecar_metadata<'e, E>(exec: E, id: Uuid, sidecar_metadata: &Value) -> Result<()>
where
    E: Executor<'e, Database = Postgres>,
{
    sqlx::query(
        r#"
        UPDATE media_root_files SET
            sidecar_metadata = $2,
            updated_at = NOW()
        WHERE id = $1
        "#,
    )
    .bind(id)
    .bind(sidecar_metadata)
    .execute(exec)
    .await?;
    Ok(())
}

/// Set sidecar images on a file.
pub async fn set_sidecar_images<'e, E>(exec: E, id: Uuid, sidecar_images: &Value) -> Result<()>
where
    E: Executor<'e, Database = Postgres>,
{
    sqlx::query(
        r#"
        UPDATE media_root_files SET
            sidecar_images = $2,
            updated_at = NOW()
        WHERE id = $1
        "#,
    )
    .bind(id)
    .bind(sidecar_images)
    .execute(exec)
    .await?;
    Ok(())
}

/// Set interpretation result on a file.
pub async fn set_interpreted<'e, E>(exec: E, id: Uuid, interpretation: &Value) -> Result<()>
where
    E: Executor<'e, Database = Postgres>,
{
    sqlx::query(
        r#"
        UPDATE media_root_files SET
            match_status = 'identified',
            interpretation = $2,
            updated_at = NOW()
        WHERE id = $1
        "#,
    )
    .bind(id)
    .bind(interpretation)
    .execute(exec)
    .await?;
    Ok(())
}

/// Set a file as added to library (linked to an entity).
pub async fn set_added<'e, E>(exec: E, id: Uuid, entity_type: &str, entity_id: Uuid) -> Result<()>
where
    E: Executor<'e, Database = Postgres>,
{
    sqlx::query(
        r#"
        UPDATE media_root_files SET
            match_status = 'added',
            entity_type = $2,
            entity_id = $3,
            match_candidates = NULL,
            updated_at = NOW()
        WHERE id = $1
        "#,
    )
    .bind(id)
    .bind(entity_type)
    .bind(entity_id)
    .execute(exec)
    .await?;
    Ok(())
}

/// Set multiple files as added in a single query.
///
/// Each entry is (file_id, entity_type, entity_id).
pub async fn set_added_batch(pool: &sqlx::PgPool, updates: &[(Uuid, &str, Uuid)]) -> Result<()> {
    if updates.is_empty() {
        return Ok(());
    }
    let file_ids: Vec<Uuid> = updates.iter().map(|(id, _, _)| *id).collect();
    let entity_types: Vec<&str> = updates.iter().map(|(_, et, _)| *et).collect();
    let entity_ids: Vec<Uuid> = updates.iter().map(|(_, _, eid)| *eid).collect();

    sqlx::query(
        r#"
        UPDATE media_root_files AS mrf SET
            match_status = 'added',
            entity_type = v.entity_type,
            entity_id = v.entity_id,
            match_candidates = NULL,
            updated_at = NOW()
        FROM (
            SELECT unnest($1::uuid[]) AS id,
                   unnest($2::text[]) AS entity_type,
                   unnest($3::uuid[]) AS entity_id
        ) v
        WHERE mrf.id = v.id
        "#,
    )
    .bind(&file_ids)
    .bind(&entity_types)
    .bind(&entity_ids)
    .execute(pool)
    .await?;
    Ok(())
}

/// Set a file as ambiguous with candidates.
pub async fn set_ambiguous<'e, E>(exec: E, id: Uuid, candidates: &Value) -> Result<()>
where
    E: Executor<'e, Database = Postgres>,
{
    sqlx::query(
        r#"
        UPDATE media_root_files SET
            match_status = 'ambiguous',
            match_candidates = $2,
            updated_at = NOW()
        WHERE id = $1
        "#,
    )
    .bind(id)
    .bind(candidates)
    .execute(exec)
    .await?;
    Ok(())
}

/// Set a file as review (confident match, awaiting user confirmation).
pub async fn set_review<'e, E>(exec: E, id: Uuid, candidates: &Value) -> Result<()>
where
    E: Executor<'e, Database = Postgres>,
{
    sqlx::query(
        r#"
        UPDATE media_root_files SET
            match_status = 'review',
            match_candidates = $2,
            updated_at = NOW()
        WHERE id = $1
        "#,
    )
    .bind(id)
    .bind(candidates)
    .execute(exec)
    .await?;
    Ok(())
}

/// Set a file as unmatched.
pub async fn set_unmatched<'e, E>(exec: E, id: Uuid) -> Result<()>
where
    E: Executor<'e, Database = Postgres>,
{
    sqlx::query(
        r#"
        UPDATE media_root_files SET
            match_status = 'unmatched',
            updated_at = NOW()
        WHERE id = $1
        "#,
    )
    .bind(id)
    .execute(exec)
    .await?;
    Ok(())
}

/// Set a file as ignored.
pub async fn set_ignored<'e, E>(exec: E, id: Uuid) -> Result<()>
where
    E: Executor<'e, Database = Postgres>,
{
    sqlx::query(
        r#"
        UPDATE media_root_files SET
            match_status = 'ignored',
            updated_at = NOW()
        WHERE id = $1
        "#,
    )
    .bind(id)
    .execute(exec)
    .await?;
    Ok(())
}

/// Reset a file for re-matching.
pub async fn reset_for_rematch<'e, E>(exec: E, id: Uuid) -> Result<()>
where
    E: Executor<'e, Database = Postgres>,
{
    sqlx::query(
        r#"
        UPDATE media_root_files SET
            match_status = 'pending',
            entity_type = NULL,
            entity_id = NULL,
            interpretation = NULL,
            match_candidates = NULL,
            updated_at = NOW()
        WHERE id = $1
        "#,
    )
    .bind(id)
    .execute(exec)
    .await?;
    Ok(())
}

/// Mark files not in the given set as unavailable.
pub async fn mark_missing_unavailable<'e, E>(
    exec: E,
    media_root_id: Uuid,
    existing_paths: &[String],
) -> Result<u64>
where
    E: Executor<'e, Database = Postgres>,
{
    let result = sqlx::query(
        r#"
        UPDATE media_root_files SET
            available = FALSE,
            updated_at = NOW()
        WHERE media_root_id = $1
          AND available = TRUE
          AND file_path != ALL($2)
        "#,
    )
    .bind(media_root_id)
    .bind(existing_paths)
    .execute(exec)
    .await?;

    Ok(result.rows_affected())
}

/// Propagate unavailability from media_root_files to entity file tables.
///
/// When media_root_scan marks files as unavailable, the corresponding rows in
/// movie_files, tv_episode_files, book_files, and audiobook_files still have
/// `available = TRUE`. This function bulk-updates them to match.
///
/// Returns the total number of entity file rows marked unavailable.
pub async fn propagate_unavailable_to_entity_files(
    pool: &PgPool,
    media_root_id: Uuid,
) -> Result<u64> {
    let mut total = 0u64;

    // movie_files: join on file_path
    let r = sqlx::query(
        r#"
        UPDATE movie_files mf SET available = FALSE
        WHERE mf.available = TRUE
          AND EXISTS (
            SELECT 1 FROM media_root_files mrf
            WHERE mrf.file_path = mf.file_path
              AND mrf.media_root_id = $1
              AND mrf.available = FALSE
          )
        "#,
    )
    .bind(media_root_id)
    .execute(pool)
    .await?;
    total += r.rows_affected();

    // tv_episode_files: join on file_path
    let r = sqlx::query(
        r#"
        UPDATE tv_episode_files ef SET available = FALSE
        WHERE ef.available = TRUE
          AND EXISTS (
            SELECT 1 FROM media_root_files mrf
            WHERE mrf.file_path = ef.file_path
              AND mrf.media_root_id = $1
              AND mrf.available = FALSE
          )
        "#,
    )
    .bind(media_root_id)
    .execute(pool)
    .await?;
    total += r.rows_affected();

    let r = sqlx::query(
        r#"
        UPDATE book_files bf SET
            available = FALSE,
            last_checked_at = NOW(),
            unavailable_since = COALESCE(bf.unavailable_since, NOW())
        WHERE bf.available = TRUE
          AND EXISTS (
            SELECT 1 FROM media_root_files mrf
            WHERE mrf.file_path = bf.file_path
              AND mrf.media_root_id = $1
              AND mrf.available = FALSE
          )
        "#,
    )
    .bind(media_root_id)
    .execute(pool)
    .await?;
    total += r.rows_affected();

    // audiobook_files: join on file_path (or source_path as fallback)
    let r = sqlx::query(
        r#"
        UPDATE audiobook_files af SET
            available = FALSE,
            last_checked_at = NOW(),
            unavailable_since = COALESCE(af.unavailable_since, NOW())
        WHERE af.available = TRUE
          AND EXISTS (
            SELECT 1 FROM media_root_files mrf
            WHERE (mrf.file_path = af.file_path OR mrf.file_path = af.source_path)
              AND mrf.media_root_id = $1
              AND mrf.available = FALSE
          )
        "#,
    )
    .bind(media_root_id)
    .execute(pool)
    .await?;
    total += r.rows_affected();

    Ok(total)
}

/// Update file_path for a matched file (used by reorganize).
pub async fn update_file_path<'e, E>(
    exec: E,
    id: Uuid,
    new_path: &str,
    new_name: &str,
) -> Result<()>
where
    E: Executor<'e, Database = Postgres>,
{
    sqlx::query(
        r#"
        UPDATE media_root_files SET
            file_path = $2,
            file_name = $3,
            updated_at = NOW()
        WHERE id = $1
        "#,
    )
    .bind(id)
    .bind(new_path)
    .bind(new_name)
    .execute(exec)
    .await?;
    Ok(())
}

/// Find a file in the same media root with the same fingerprint and size but
/// at a different path. Used to detect renames/moves: if a new file appears
/// whose fingerprint matches an existing record at a different path, the file
/// was likely moved rather than being a genuinely new file.
///
/// Returns the old record's id and transferable metadata if a match is found.
pub async fn find_renamed_source(
    pool: &PgPool,
    media_root_id: Uuid,
    fingerprint: &str,
    file_size: i64,
    new_file_path: &str,
) -> Result<Option<RenamedSourceRecord>> {
    let row = sqlx::query(
        r#"
        SELECT id, file_path, match_status, entity_type, entity_id,
               interpretation, match_candidates, sidecar_metadata, sidecar_images
        FROM media_root_files
        WHERE media_root_id = $1
          AND file_fingerprint = $2
          AND file_size = $3
          AND file_path != $4
          AND available = FALSE
        ORDER BY updated_at DESC
        LIMIT 1
        "#,
    )
    .bind(media_root_id)
    .bind(fingerprint)
    .bind(file_size)
    .bind(new_file_path)
    .fetch_optional(pool)
    .await?;

    Ok(row.map(|r| RenamedSourceRecord {
        id: r.get("id"),
        file_path: r.get("file_path"),
        match_status: r.get("match_status"),
        entity_type: r.get("entity_type"),
        entity_id: r.get("entity_id"),
        interpretation: r.get("interpretation"),
        match_candidates: r.get("match_candidates"),
        sidecar_metadata: r.get("sidecar_metadata"),
        sidecar_images: r.get("sidecar_images"),
    }))
}

/// Record returned by `find_renamed_source`.
pub struct RenamedSourceRecord {
    pub id: Uuid,
    pub file_path: String,
    pub match_status: String,
    pub entity_type: Option<String>,
    pub entity_id: Option<Uuid>,
    pub interpretation: Option<Value>,
    pub match_candidates: Option<Value>,
    pub sidecar_metadata: Option<Value>,
    pub sidecar_images: Option<Value>,
}

/// Transfer metadata from an old record (at a previous path) to a new record
/// (at the current path), then delete the old record. Used after rename
/// detection to preserve entity associations across file moves.
///
/// Also updates the file path in entity file tables (`tv_episode_files`,
/// `movie_files`, `book_files`, `audiobook_files`, `external_subtitles`)
/// so they stay in sync with the actual file location on disk.
pub async fn transfer_from_renamed(
    pool: &PgPool,
    new_id: Uuid,
    new_file_path: &str,
    source: &RenamedSourceRecord,
) -> Result<()> {
    let mut tx = pool.begin().await?;

    sqlx::query(
        r#"
        UPDATE media_root_files SET
            match_status = $2,
            entity_type = $3,
            entity_id = $4,
            interpretation = $5,
            match_candidates = $6,
            sidecar_metadata = $7,
            sidecar_images = $8,
            updated_at = NOW()
        WHERE id = $1
        "#,
    )
    .bind(new_id)
    .bind(&source.match_status)
    .bind(&source.entity_type)
    .bind(source.entity_id)
    .bind(&source.interpretation)
    .bind(&source.match_candidates)
    .bind(&source.sidecar_metadata)
    .bind(&source.sidecar_images)
    .execute(&mut *tx)
    .await?;

    // Update file path in entity file tables so they track the new location.
    // Each table has UNIQUE(file_path). If a duplicate already exists at the
    // new path (e.g. the resolve pipeline raced ahead), delete it first so the
    // old record (which carries source_metadata/provenance) can take its place.
    let old_path = &source.file_path;
    for table in &[
        "tv_episode_files",
        "movie_files",
        "book_files",
        "audiobook_files",
        "external_subtitles",
    ] {
        // Remove any stale duplicate at the new path before updating
        sqlx::query(sqlx::AssertSqlSafe(format!(
            "DELETE FROM {table} WHERE file_path = $1"
        )))
        .bind(new_file_path)
        .execute(&mut *tx)
        .await?;
        sqlx::query(sqlx::AssertSqlSafe(format!(
            "UPDATE {table} SET file_path = $1 WHERE file_path = $2"
        )))
        .bind(new_file_path)
        .bind(old_path)
        .execute(&mut *tx)
        .await?;
    }

    sqlx::query("DELETE FROM media_root_files WHERE id = $1")
        .bind(source.id)
        .execute(&mut *tx)
        .await?;

    tx.commit().await?;
    Ok(())
}

/// Count total available files for a root with optional status filter.
pub async fn count_for_root<'e, E>(
    exec: E,
    media_root_id: Uuid,
    status_filter: Option<&str>,
) -> Result<i64>
where
    E: Executor<'e, Database = Postgres>,
{
    let row = sqlx::query(
        r#"
        SELECT COUNT(*) AS cnt
        FROM media_root_files
        WHERE media_root_id = $1
          AND ($2::text IS NULL OR match_status = $2)
          AND available = TRUE
        "#,
    )
    .bind(media_root_id)
    .bind(status_filter)
    .fetch_one(exec)
    .await?;

    Ok(row.get::<i64, _>("cnt"))
}

/// Count files by match_status for a root.
pub async fn count_by_status<'e, E>(exec: E, media_root_id: Uuid) -> Result<Vec<(String, i64)>>
where
    E: Executor<'e, Database = Postgres>,
{
    let rows = sqlx::query(
        r#"
        SELECT match_status, COUNT(*) AS cnt
        FROM media_root_files
        WHERE media_root_id = $1 AND available = TRUE
        GROUP BY match_status
        "#,
    )
    .bind(media_root_id)
    .fetch_all(exec)
    .await?;

    Ok(rows
        .into_iter()
        .map(|r| (r.get::<String, _>("match_status"), r.get::<i64, _>("cnt")))
        .collect())
}

// =============================================================================
// Lite projections (no match_candidates / sidecar data)
// =============================================================================

/// List files for a media root with optional status and directory filters (lite projection).
///
/// When `directory` is provided, only files directly inside that directory are returned
/// (not files in subdirectories). An empty string means root-level files only.
pub async fn list_for_root_lite(
    pool: &PgPool,
    media_root_id: Uuid,
    status_filter: Option<&str>,
    directory: Option<&str>,
    root_path: &str,
    limit: i64,
    offset: i64,
) -> Result<Vec<MediaRootFileLiteRow>> {
    // None = no directory filter (all files). Some("") = root-level files only.
    // Some("foo/bar") = files directly inside foo/bar/ only.
    // Prepend root_path so prefix matches absolute file_path values.
    let root_prefix = format!("{}/", root_path.trim_end_matches('/'));
    let dir_prefix = match directory {
        Some(d) => {
            let d = d.strip_suffix('/').unwrap_or(d);
            if d.is_empty() {
                Some(root_prefix)
            } else {
                Some(format!("{root_prefix}{d}/"))
            }
        }
        None => None,
    };

    // $5 is the prefix (e.g. "ShowA/" or "" for root).
    // The substring/position check ensures we only get direct children:
    // strip the prefix, then verify the remainder has no '/'.
    let rows = sqlx::query(sqlx::AssertSqlSafe(format!(
        r#"
        SELECT {SELECT_COLS_LITE} FROM media_root_files
        WHERE media_root_id = $1
          AND ($2::text IS NULL OR match_status = $2)
          AND ($5::text IS NULL OR (
              file_path LIKE $5 || '%'
              AND position('/' in substring(file_path from length($5) + 1)) = 0
          ))
          AND available = TRUE
        ORDER BY file_path
        LIMIT $3 OFFSET $4
        "#
    )))
    .bind(media_root_id)
    .bind(status_filter)
    .bind(limit)
    .bind(offset)
    .bind(dir_prefix)
    .fetch_all(pool)
    .await?;

    Ok(rows.into_iter().map(row_to_file_lite).collect())
}

/// Directory entry for hierarchical directory browsing.
#[derive(Debug, Serialize)]
pub struct DirectoryEntry {
    pub name: String,
    pub path: String,
    pub file_count: i64,
    pub review_count: i64,
    pub ambiguous_count: i64,
    pub unmatched_count: i64,
    pub added_count: i64,
}

/// List file IDs within a directory (recursively) for a media root.
///
/// `directory` is a relative path like "Author/Book". Files whose relative path
/// starts with `directory/` are included.
pub async fn list_file_ids_in_directory(
    pool: &PgPool,
    media_root_id: Uuid,
    directory: &str,
    root_path: &str,
) -> Result<Vec<Uuid>> {
    let abs_prefix = format!(
        "{}/{}/",
        root_path.trim_end_matches('/'),
        directory.trim_matches('/')
    );
    let rows: Vec<(Uuid,)> = sqlx::query_as(
        "SELECT id FROM media_root_files WHERE media_root_id = $1 AND available = TRUE AND file_path LIKE $2 || '%'",
    )
    .bind(media_root_id)
    .bind(&abs_prefix)
    .fetch_all(pool)
    .await?;
    Ok(rows.into_iter().map(|(id,)| id).collect())
}

/// List immediate subdirectories (or files at root level) for a media root.
///
/// If `parent` is None, returns top-level directories.
/// If `parent` is Some(path), returns children of that directory.
pub async fn list_directories(
    pool: &PgPool,
    media_root_id: Uuid,
    parent: Option<&str>,
    root_path: &str,
) -> Result<Vec<DirectoryEntry>> {
    // File paths are absolute (e.g. /mnt/media/Author/Book/file.mp3).
    // Strip the root_path prefix to get relative paths before splitting.
    // rel_path = substring(file_path from len(root_path) + 2) strips root + trailing slash.
    let root_len = root_path.trim_end_matches('/').len() as i32;

    let (prefix, depth) = match parent {
        Some(p) if !p.is_empty() => {
            let normalized = p.strip_suffix('/').unwrap_or(p);
            let depth = normalized.chars().filter(|c| *c == '/').count() as i32 + 1;
            (format!("{normalized}/"), depth)
        }
        _ => (String::new(), 0),
    };

    let rows = sqlx::query(
        r#"
        WITH rel AS (
            SELECT substring(file_path from $4 + 2) AS rel_path,
                   match_status
            FROM media_root_files
            WHERE media_root_id = $1
              AND available = TRUE
        )
        SELECT
            split_part(rel_path, '/', $3 + 1) AS dir_name,
            COUNT(*) AS file_count,
            COUNT(*) FILTER (WHERE match_status = 'review') AS review_count,
            COUNT(*) FILTER (WHERE match_status = 'ambiguous') AS ambiguous_count,
            COUNT(*) FILTER (WHERE match_status = 'unmatched') AS unmatched_count,
            COUNT(*) FILTER (WHERE match_status = 'added') AS added_count
        FROM rel
        WHERE ($2::text = '' OR rel_path LIKE $2 || '%')
          AND split_part(rel_path, '/', $3 + 1) != ''
          AND split_part(rel_path, '/', $3 + 2) != ''
        GROUP BY dir_name
        ORDER BY dir_name
        "#,
    )
    .bind(media_root_id)
    .bind(&prefix)
    .bind(depth)
    .bind(root_len)
    .fetch_all(pool)
    .await?;

    Ok(rows
        .into_iter()
        .map(|r| {
            let name: String = r.get("dir_name");
            let path = if prefix.is_empty() {
                name.clone()
            } else {
                format!("{prefix}{name}")
            };
            DirectoryEntry {
                name,
                path,
                file_count: r.get("file_count"),
                review_count: r.get("review_count"),
                ambiguous_count: r.get("ambiguous_count"),
                unmatched_count: r.get("unmatched_count"),
                added_count: r.get("added_count"),
            }
        })
        .collect())
}

/// List files for a specific entity group (lite projection).
///
/// Group key format: `provider:provider_id` (match-based) or `entity_type:entity_id` (added).
/// Special group_key: `__ungrouped__` (no candidates and no entity).
#[allow(clippy::too_many_arguments)]
pub async fn list_for_group_lite(
    pool: &PgPool,
    media_root_id: Uuid,
    media_type: &str,
    group_key: &str,
    season: Option<i32>,
    status_filter: Option<&str>,
    limit: i64,
    offset: i64,
    root_path: &str,
) -> Result<Vec<MediaRootFileLiteRow>> {
    let (where_clause, order_clause, join_clause) =
        group_filter_clauses(media_type, group_key, season.is_some(), root_path);

    let sql = format!(
        r#"
        SELECT {SELECT_COLS_LITE_MRF} FROM media_root_files mrf
        {join_clause}
        WHERE mrf.media_root_id = $1
          AND mrf.available = TRUE
          AND ($4::text IS NULL OR mrf.match_status = $4)
          AND {where_clause}
        ORDER BY {order_clause}
        LIMIT $5 OFFSET $6
        "#
    );

    let rows = sqlx::query(sqlx::AssertSqlSafe(sql.as_str()))
        .bind(media_root_id)
        .bind(group_key)
        .bind(season)
        .bind(status_filter)
        .bind(limit)
        .bind(offset)
        .fetch_all(pool)
        .await?;

    Ok(rows.into_iter().map(row_to_file_lite).collect())
}

/// Count files for a specific entity group.
#[allow(clippy::too_many_arguments)]
pub async fn count_for_group(
    pool: &PgPool,
    media_root_id: Uuid,
    media_type: &str,
    group_key: &str,
    season: Option<i32>,
    status_filter: Option<&str>,
    root_path: &str,
) -> Result<i64> {
    let (where_clause, _, join_clause) =
        group_filter_clauses(media_type, group_key, season.is_some(), root_path);

    let sql = format!(
        r#"
        SELECT COUNT(*) AS cnt FROM media_root_files mrf
        {join_clause}
        WHERE mrf.media_root_id = $1
          AND mrf.available = TRUE
          AND ($4::text IS NULL OR mrf.match_status = $4)
          AND {where_clause}
        "#
    );

    let row = sqlx::query(sqlx::AssertSqlSafe(sql.as_str()))
        .bind(media_root_id)
        .bind(group_key)
        .bind(season)
        .bind(status_filter)
        .fetch_one(pool)
        .await?;

    Ok(row.get::<i64, _>("cnt"))
}

/// Build WHERE, ORDER BY, and optional JOIN clauses for group-filtered queries.
///
/// Parameters: $2 = group_key (unused in generated SQL — parsed here), $3 = season.
///
/// Group key formats:
/// - `provider:provider_id` — match-candidate-based group (review/ambiguous files)
/// - `entity_type:entity_id` — entity-based group (added files)
/// - `__ungrouped__` — files with no candidates and no entity
///
/// Returns `(where_clause, order_clause, join_clause)`.
fn group_filter_clauses(
    media_type: &str,
    group_key: &str,
    has_season: bool,
    _root_path: &str,
) -> (String, String, String) {
    match group_key {
        "__ungrouped__" => (
            "(mrf.match_candidates IS NULL AND mrf.entity_id IS NULL)".to_string(),
            "mrf.file_path".to_string(),
            String::new(),
        ),
        _ => {
            // Parse "prefix:id" from group_key
            let (prefix, id_part) = group_key.split_once(':').unwrap_or(("", group_key));

            // Entity-based group keys use known entity type prefixes
            let is_entity_group = matches!(
                prefix,
                "book" | "movie" | "tv_show" | "tv_episode" | "sport"
            );

            if is_entity_group {
                let entity_id: Uuid = id_part.parse().unwrap_or_default();

                let (where_clause, join) = if prefix == "tv_show" {
                    let season_filter = if has_season {
                        " AND COALESCE(tsn.season_sort_order, 0) = $3"
                    } else {
                        ""
                    };
                    (
                        format!("tsn.show_id = '{entity_id}'::uuid{season_filter}"),
                        "LEFT JOIN tv_episodes ep ON mrf.entity_id = ep.id AND mrf.entity_type = 'tv_episode' \
                         LEFT JOIN tv_seasons tsn ON ep.season_id = tsn.id".to_string(),
                    )
                } else {
                    (
                        format!(
                            "mrf.entity_type = '{prefix}' AND mrf.entity_id = '{entity_id}'::uuid"
                        ),
                        String::new(),
                    )
                };

                let order = match media_type {
                    "tv" | "sport" => "(mrf.interpretation->>'season')::int NULLS FIRST, \
                        (mrf.interpretation->'episodes'->0)::int NULLS FIRST, mrf.file_path"
                        .to_string(),
                    _ => "mrf.file_path".to_string(),
                };

                (where_clause, order, join)
            } else {
                // Match-candidate-based group: filter by provider + provider_id
                // Sanitize by rejecting any single quotes in user input
                let safe_prefix = prefix.replace('\'', "");
                let safe_id = id_part.replace('\'', "");

                let season_filter = if has_season && (media_type == "tv" || media_type == "sport") {
                    " AND COALESCE((mrf.interpretation->>'season')::int, \
                       (mrf.match_candidates->0->>'season')::int, 0) = $3"
                } else {
                    ""
                };

                let where_clause = format!(
                    "mrf.match_candidates->0->>'provider' = '{safe_prefix}' \
                     AND mrf.match_candidates->0->>'provider_id' = '{safe_id}'{season_filter}"
                );

                let order = match media_type {
                    "tv" | "sport" => "(mrf.interpretation->>'season')::int NULLS FIRST, \
                        (mrf.interpretation->'episodes'->0)::int NULLS FIRST, mrf.file_path"
                        .to_string(),
                    "book" | "audiobook" => {
                        "COALESCE((mrf.match_candidates->0->>'series_index')::float, \
                         (mrf.interpretation->>'series_index')::float) NULLS LAST, mrf.file_path"
                            .to_string()
                    }
                    _ => "mrf.file_path".to_string(),
                };

                (where_clause, order, String::new())
            }
        }
    }
}

// =============================================================================
// Entity grouping
// =============================================================================

/// Compute group_status from status counts.
fn compute_group_status(review: i64, ambiguous: i64, added: i64) -> String {
    if added > 0 && review == 0 && ambiguous == 0 {
        "added".to_string()
    } else if ambiguous > 0 {
        if review > 0 || added > 0 {
            "mixed".to_string()
        } else {
            "ambiguous".to_string()
        }
    } else if review > 0 {
        if added > 0 {
            "mixed".to_string()
        } else {
            "review".to_string()
        }
    } else {
        "mixed".to_string()
    }
}

/// List entity groups for a media root (server-side, match-based grouping).
///
/// Groups review/ambiguous files by their top match candidate (provider:provider_id),
/// and added files by their library entity (entity_type:entity_id). Uses a UNION
/// because match_candidates is set to NULL when files are added.
pub async fn list_entity_groups(
    pool: &PgPool,
    media_root_id: Uuid,
    media_type: &str,
    _root_path: &str,
) -> Result<Vec<EntityGroupSummary>> {
    let poster_field = if media_type == "book" || media_type == "audiobook" {
        "cover_url"
    } else {
        "poster_url"
    };

    // -------------------------------------------------------------------
    // Arm 1: Files with match_candidates (review / ambiguous)
    // Group by top candidate's provider + provider_id.
    // -------------------------------------------------------------------
    let arm1_sql = format!(
        r#"
        SELECT
            (mrf.match_candidates->0->>'provider') || ':' || (mrf.match_candidates->0->>'provider_id') AS group_key,
            MAX(mrf.match_candidates->0->>'title') AS label,
            MAX((mrf.match_candidates->0->>'year')::int) AS year,
            COUNT(*) AS file_count,
            COUNT(*) FILTER (WHERE mrf.match_status = 'review') AS review_count,
            COUNT(*) FILTER (WHERE mrf.match_status = 'unmatched') AS unmatched_count,
            COUNT(*) FILTER (WHERE mrf.match_status = 'ambiguous') AS ambiguous_count,
            COUNT(*) FILTER (WHERE mrf.match_status = 'added') AS added_count,
            MIN(mrf.match_candidates->0->>'{poster_field}') FILTER (
                WHERE mrf.match_candidates->0->>'{poster_field}' IS NOT NULL
            ) AS poster_url,
            MAX(mrf.match_candidates->0->>'provider') AS provider,
            MAX(mrf.match_candidates->0->>'provider_id') AS provider_id,
            MAX(mrf.match_candidates->0->>'series') AS series_name,
            MIN((mrf.match_candidates->0->>'series_index')::float) AS series_index,
            MAX(mrf.match_candidates->0->>'authors') AS authors_json,
            MIN((mrf.match_candidates->0->>'confidence')::float) AS min_confidence,
            MAX(mrf.match_candidates->0->>'{poster_field}') AS cover_url
        FROM media_root_files mrf
        WHERE mrf.media_root_id = $1
          AND mrf.available = TRUE
          AND mrf.match_candidates IS NOT NULL
          AND jsonb_array_length(mrf.match_candidates) > 0
          AND mrf.match_candidates->0->>'provider' IS NOT NULL
          AND mrf.match_candidates->0->>'provider_id' IS NOT NULL
        GROUP BY mrf.match_candidates->0->>'provider', mrf.match_candidates->0->>'provider_id'
        "#,
        poster_field = poster_field,
    );

    // -------------------------------------------------------------------
    // Arm 2: Added files (entity_id set, match_candidates NULL).
    // Run as separate query to avoid UNION column-alignment complexity.
    // -------------------------------------------------------------------
    let (arm2_select, arm2_join, arm2_group_key) = match media_type {
        "book" | "audiobook" => (
            r#"
            mrf.entity_type || ':' || mrf.entity_id::text AS group_key,
            MAX(b.title) AS label,
            MAX(s.name) AS series_name,
            MIN(bs.series_index) AS series_index,
            EXTRACT(YEAR FROM MAX(b.pubdate))::int AS year,
            MAX(b.images->'cover'->>'source_url') AS cover_url
            "#
            .to_string(),
            "LEFT JOIN books b ON mrf.entity_id = b.id AND mrf.entity_type = 'book' \
             LEFT JOIN book_series bs ON bs.book_id = b.id AND bs.is_primary = TRUE \
             LEFT JOIN series s ON s.id = bs.series_id",
            "mrf.entity_type, mrf.entity_id",
        ),
        "tv" | "sport" => (
            r#"
            'tv_show:' || sh.id::text AS group_key,
            MAX(sh.title) AS label,
            NULL::text AS series_name,
            NULL::float AS series_index,
            MAX(sh.year) AS year,
            MAX(sh.images->'poster'->>'source_url') AS cover_url
            "#
            .to_string(),
            "LEFT JOIN tv_episodes ep ON mrf.entity_id = ep.id AND mrf.entity_type = 'tv_episode' \
             LEFT JOIN tv_seasons tsn ON ep.season_id = tsn.id \
             LEFT JOIN tv_shows sh ON tsn.show_id = sh.id",
            "sh.id",
        ),
        "movie" => (
            r#"
            mrf.entity_type || ':' || mrf.entity_id::text AS group_key,
            MAX(m.title) AS label,
            NULL::text AS series_name,
            NULL::float AS series_index,
            MAX(m.year) AS year,
            MAX(m.images->'poster'->>'source_url') AS cover_url
            "#
            .to_string(),
            "LEFT JOIN movies m ON mrf.entity_id = m.id AND mrf.entity_type = 'movie'",
            "mrf.entity_type, mrf.entity_id",
        ),
        _ => (
            r#"
            mrf.entity_type || ':' || mrf.entity_id::text AS group_key,
            NULL::text AS label,
            NULL::text AS series_name,
            NULL::float AS series_index,
            NULL::int AS year,
            NULL::text AS cover_url
            "#
            .to_string(),
            "",
            "mrf.entity_type, mrf.entity_id",
        ),
    };

    let arm2_sql = format!(
        r#"
        SELECT
            {arm2_select},
            COUNT(*) AS file_count,
            COUNT(*) FILTER (WHERE mrf.match_status = 'review') AS review_count,
            COUNT(*) FILTER (WHERE mrf.match_status = 'unmatched') AS unmatched_count,
            COUNT(*) FILTER (WHERE mrf.match_status = 'ambiguous') AS ambiguous_count,
            COUNT(*) FILTER (WHERE mrf.match_status = 'added') AS added_count
        FROM media_root_files mrf
        {arm2_join}
        WHERE mrf.media_root_id = $1
          AND mrf.available = TRUE
          AND mrf.entity_id IS NOT NULL
          AND mrf.match_candidates IS NULL
        GROUP BY {arm2_group_key}
        "#,
        arm2_select = arm2_select,
        arm2_join = arm2_join,
        arm2_group_key = arm2_group_key,
    );

    // -------------------------------------------------------------------
    // Run both queries
    // -------------------------------------------------------------------
    let (arm1_rows, arm2_rows) = tokio::try_join!(
        sqlx::query(sqlx::AssertSqlSafe(arm1_sql.as_str()))
            .bind(media_root_id)
            .fetch_all(pool),
        sqlx::query(sqlx::AssertSqlSafe(arm2_sql.as_str()))
            .bind(media_root_id)
            .fetch_all(pool),
    )?;

    let mut groups: Vec<EntityGroupSummary> = Vec::new();

    // Process arm1 (match-candidate-based groups)
    for r in arm1_rows {
        let review_count: i64 = r.get("review_count");
        let ambiguous_count: i64 = r.get("ambiguous_count");
        let added_count: i64 = r.get("added_count");

        let authors: Option<Vec<String>> = r
            .get::<Option<String>, _>("authors_json")
            .and_then(|s| serde_json::from_str(&s).ok());

        groups.push(EntityGroupSummary {
            group_key: r.get("group_key"),
            label: r.get::<Option<String>, _>("label").unwrap_or_default(),
            year: r.get("year"),
            file_count: r.get("file_count"),
            review_count,
            unmatched_count: r.get("unmatched_count"),
            ambiguous_count,
            added_count,
            poster_url: r.get("poster_url"),
            subgroups: Vec::new(),
            group_status: compute_group_status(review_count, ambiguous_count, added_count),
            authors,
            series_name: r.get("series_name"),
            series_index: r.get("series_index"),
            provider: r.get("provider"),
            provider_id: r.get("provider_id"),
            confidence: r.get("min_confidence"),
            cover_url: r.get("cover_url"),
        });
    }

    // Process arm2 (entity-based groups for added files)
    for r in arm2_rows {
        let gk: Option<String> = r.get("group_key");
        let Some(group_key) = gk else { continue };
        let review_count: i64 = r.get("review_count");
        let ambiguous_count: i64 = r.get("ambiguous_count");
        let added_count: i64 = r.get("added_count");

        groups.push(EntityGroupSummary {
            group_key,
            label: r.get::<Option<String>, _>("label").unwrap_or_default(),
            year: r.get("year"),
            file_count: r.get("file_count"),
            review_count,
            unmatched_count: r.get("unmatched_count"),
            ambiguous_count,
            added_count,
            poster_url: None,
            subgroups: Vec::new(),
            group_status: compute_group_status(review_count, ambiguous_count, added_count),
            authors: None,
            series_name: r.get("series_name"),
            series_index: r.get("series_index"),
            provider: None,
            provider_id: None,
            confidence: None,
            cover_url: r.get("cover_url"),
        });
    }

    // Sort all groups by label
    groups.sort_by_key(|a| a.label.to_lowercase());

    // For TV/sport, fetch season subgroups and merge them in.
    if (media_type == "tv" || media_type == "sport") && !groups.is_empty() {
        // Arm 1 subgroups: from match_candidates (review/ambiguous)
        let sub_arm1 = r#"
            SELECT
                (mrf.match_candidates->0->>'provider') || ':' || (mrf.match_candidates->0->>'provider_id') AS group_key,
                COALESCE((mrf.interpretation->>'season')::int, (mrf.match_candidates->0->>'season')::int, 0) AS season,
                COUNT(*) AS file_count,
                COUNT(*) FILTER (WHERE mrf.match_status = 'review') AS review_count
            FROM media_root_files mrf
            WHERE mrf.media_root_id = $1
              AND mrf.available = TRUE
              AND mrf.match_candidates IS NOT NULL
              AND jsonb_array_length(mrf.match_candidates) > 0
            GROUP BY mrf.match_candidates->0->>'provider', mrf.match_candidates->0->>'provider_id',
                     COALESCE((mrf.interpretation->>'season')::int, (mrf.match_candidates->0->>'season')::int, 0)
        "#;

        // Arm 2 subgroups: from entity tables (added files)
        let sub_arm2 = r#"
            SELECT
                'tv_show:' || sh.id::text AS group_key,
                COALESCE(tsn.season_sort_order, 0) AS season,
                COUNT(*) AS file_count,
                COUNT(*) FILTER (WHERE mrf.match_status = 'review') AS review_count
            FROM media_root_files mrf
            LEFT JOIN tv_episodes ep ON mrf.entity_id = ep.id AND mrf.entity_type = 'tv_episode'
            LEFT JOIN tv_seasons tsn ON ep.season_id = tsn.id
            LEFT JOIN tv_shows sh ON tsn.show_id = sh.id
            WHERE mrf.media_root_id = $1
              AND mrf.available = TRUE
              AND mrf.entity_id IS NOT NULL
              AND mrf.match_candidates IS NULL
            GROUP BY sh.id, tsn.season_sort_order
        "#;

        let subgroup_sql = format!(
            r#"
            SELECT group_key, season, file_count, review_count
            FROM ({sub_arm1} UNION ALL {sub_arm2}) sub
            WHERE group_key IS NOT NULL
            ORDER BY group_key, season
            "#,
        );

        let subgroup_rows = sqlx::query(sqlx::AssertSqlSafe(subgroup_sql.as_str()))
            .bind(media_root_id)
            .fetch_all(pool)
            .await?;

        let mut subgroup_map: HashMap<String, Vec<EntitySubgroupSummary>> = HashMap::new();
        for r in subgroup_rows {
            let key: String = r.get("group_key");
            let season: i32 = r.get("season");
            let label = if season == 0 {
                "Specials".to_string()
            } else {
                format!("Season {season}")
            };
            subgroup_map
                .entry(key)
                .or_default()
                .push(EntitySubgroupSummary {
                    season,
                    label,
                    file_count: r.get("file_count"),
                    review_count: r.get("review_count"),
                });
        }

        for group in &mut groups {
            if let Some(subs) = subgroup_map.remove(&group.group_key) {
                group.subgroups = subs;
            }
        }
    }

    Ok(groups)
}

/// Count files with no match candidates and no entity (ungrouped).
pub async fn count_ungrouped(pool: &PgPool, media_root_id: Uuid) -> Result<i64> {
    let row = sqlx::query(
        r#"
        SELECT COUNT(*) AS cnt
        FROM media_root_files
        WHERE media_root_id = $1
          AND available = TRUE
          AND match_candidates IS NULL
          AND entity_id IS NULL
        "#,
    )
    .bind(media_root_id)
    .fetch_one(pool)
    .await?;
    Ok(row.get::<i64, _>("cnt"))
}

/// Delete a media_root_files entry by its file path.
pub async fn delete_by_path<'e, E>(exec: E, file_path: &str) -> Result<bool>
where
    E: Executor<'e, Database = Postgres>,
{
    let result = sqlx::query("DELETE FROM media_root_files WHERE file_path = $1")
        .bind(file_path)
        .execute(exec)
        .await?;
    Ok(result.rows_affected() > 0)
}

/// Reassign entity_id for a media_root_files entry matching a file path.
/// Used when a book file is moved to a different book to keep media_root_files in sync.
pub async fn reassign_entity_by_path<'e, E>(
    exec: E,
    file_path: &str,
    new_entity_id: Uuid,
) -> Result<bool>
where
    E: Executor<'e, Database = Postgres>,
{
    let result = sqlx::query(
        "UPDATE media_root_files SET entity_id = $2, updated_at = NOW() WHERE file_path = $1",
    )
    .bind(file_path)
    .bind(new_entity_id)
    .execute(exec)
    .await?;
    Ok(result.rows_affected() > 0)
}

/// Reset all files in a media root with a given status for re-interpretation.
/// Returns the number of files reset.
pub async fn reset_for_reinterpret<'e, E>(
    exec: E,
    media_root_id: Uuid,
    status_filter: Option<&str>,
) -> Result<u64>
where
    E: Executor<'e, Database = Postgres>,
{
    let (sql, use_status_bind) = if status_filter.is_some() {
        (
            "UPDATE media_root_files SET \
                match_status = 'pending', interpretation = NULL, match_candidates = NULL, updated_at = NOW() \
             WHERE media_root_id = $1 AND available = TRUE AND match_status = $2"
                .to_string(),
            true,
        )
    } else {
        (
            "UPDATE media_root_files SET \
                match_status = 'pending', interpretation = NULL, match_candidates = NULL, updated_at = NOW() \
             WHERE media_root_id = $1 AND available = TRUE AND match_status NOT IN ('added', 'ignored', 'pending')"
                .to_string(),
            false,
        )
    };

    let mut query = sqlx::query(sqlx::AssertSqlSafe(sql.as_str())).bind(media_root_id);
    if use_status_bind {
        query = query.bind(status_filter.unwrap());
    }
    let result = query.execute(exec).await?;
    Ok(result.rows_affected())
}

// =============================================================================
// Match resolution helpers
// =============================================================================

/// Build a candidates array with the chosen candidate first (marked resolved).
///
/// Used by both the single-file resolve route and the bulk resolve job handler
/// when moving files to "review" status with a user-chosen candidate.
pub fn build_confirmed_candidates(file: &MediaRootFileRow, chosen: &Value) -> Value {
    let existing: Vec<Value> = file
        .match_candidates
        .as_ref()
        .and_then(|v| serde_json::from_value(v.clone()).ok())
        .unwrap_or_default();

    let chosen_provider = chosen["provider"].as_str().unwrap_or("");
    let chosen_id = chosen["provider_id"]
        .as_str()
        .map(|s| s.to_string())
        .unwrap_or_else(|| {
            chosen["provider_id"]
                .as_u64()
                .map(|n| n.to_string())
                .unwrap_or_default()
        });

    let mut confirmed = chosen.clone();
    if let Some(obj) = confirmed.as_object_mut() {
        obj.insert("resolved".to_string(), json!(true));
    }

    let rest: Vec<_> = existing
        .into_iter()
        .filter(|c| {
            let p = c["provider"].as_str().unwrap_or("");
            let id = c["provider_id"]
                .as_str()
                .map(|s| s.to_string())
                .unwrap_or_else(|| {
                    c["provider_id"]
                        .as_u64()
                        .map(|n| n.to_string())
                        .unwrap_or_default()
                });
            !(p == chosen_provider && id == chosen_id)
        })
        .collect();

    let mut result = vec![confirmed];
    result.extend(rest);
    json!(result)
}
