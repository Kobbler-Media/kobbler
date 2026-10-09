//! Media root repository operations.

use super::{RepositoryError, Result};
use serde::Serialize;
use sqlx::{Executor, Postgres, Row};
use time::OffsetDateTime;
use uuid::Uuid;

// =============================================================================
// Types
// =============================================================================

/// Input for creating a media root.
#[derive(Debug)]
pub struct MediaRootInput {
    pub path: String,
    pub name: String,
    pub media_type: String,
    pub auto_add: bool,
    pub naming_template: Option<String>,
    pub default_entitlement_scope: Option<String>,
    pub write_sidecars: bool,
    pub write_images: bool,
    pub is_default: bool,
}

/// Input for updating a media root.
#[derive(Debug, Default)]
pub struct MediaRootUpdate {
    pub path: Option<String>,
    pub name: Option<String>,
    pub enabled: Option<bool>,
    pub auto_add: Option<bool>,
    pub naming_template: Option<Option<String>>,
    pub default_entitlement_scope: Option<String>,
    pub write_sidecars: Option<bool>,
    pub write_images: Option<bool>,
    pub is_default: Option<bool>,
    pub max_concurrent_io: Option<i32>,
    pub scan_interval_seconds: Option<Option<i32>>,
}

/// Media root row from the database.
#[derive(Debug, Clone, Serialize)]
pub struct MediaRootRow {
    pub id: Uuid,
    pub path: String,
    pub name: String,
    pub media_type: String,
    pub enabled: bool,
    pub auto_add: bool,
    pub scan_status: String,
    pub last_scan_at: Option<OffsetDateTime>,
    pub scan_error: Option<String>,
    pub error_count: i32,
    pub next_retry_at: Option<OffsetDateTime>,
    pub naming_template: Option<String>,
    pub default_entitlement_scope: String,
    pub write_sidecars: bool,
    pub write_images: bool,
    pub is_default: bool,
    pub created_at: OffsetDateTime,
    pub updated_at: OffsetDateTime,
    pub max_concurrent_io: i32,
    pub scan_interval_seconds: Option<i32>,
    pub last_scan_completed_at: Option<OffsetDateTime>,
}

/// Lightweight row for enabled-directory listing.
#[derive(Debug)]
pub struct EnabledMediaRoot {
    pub id: Uuid,
    pub path: String,
    pub name: String,
    pub media_type: String,
    pub auto_add: bool,
    pub error_count: i32,
    pub next_retry_at: Option<OffsetDateTime>,
    pub naming_template: Option<String>,
    pub write_sidecars: bool,
    pub write_images: bool,
    pub is_default: bool,
    pub scan_interval_seconds: Option<i32>,
    pub last_scan_completed_at: Option<OffsetDateTime>,
}

/// Stats for a media root.
#[derive(Debug, Serialize)]
pub struct MediaRootStats {
    pub total_files: i64,
    pub added: i64,
    pub review: i64,
    pub ambiguous: i64,
    pub unmatched: i64,
    pub ignored: i64,
    pub pending: i64,
    pub identified: i64,
    pub unavailable: i64,
}

// =============================================================================
// CRUD
// =============================================================================

/// Create a new media root.
pub async fn create<'e, E>(exec: E, input: MediaRootInput) -> Result<Uuid>
where
    E: Executor<'e, Database = Postgres>,
{
    if input.path.trim().is_empty() {
        return Err(RepositoryError::Validation(
            "path cannot be empty".to_string(),
        ));
    }
    if !matches!(
        input.media_type.as_str(),
        "tv" | "movie" | "book" | "sport" | "audiobook"
    ) {
        return Err(RepositoryError::Validation(
            "media_type must be 'tv', 'movie', 'book', 'sport', or 'audiobook'".to_string(),
        ));
    }

    let id = Uuid::now_v7();
    let scope = input
        .default_entitlement_scope
        .as_deref()
        .unwrap_or("all_users");
    sqlx::query(
        r#"
        INSERT INTO media_roots (id, path, name, media_type, auto_add, naming_template, default_entitlement_scope, write_sidecars, write_images, is_default)
        VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10)
        "#,
    )
    .bind(id)
    .bind(&input.path)
    .bind(&input.name)
    .bind(&input.media_type)
    .bind(input.auto_add)
    .bind(&input.naming_template)
    .bind(scope)
    .bind(input.write_sidecars)
    .bind(input.write_images)
    .bind(input.is_default)
    .execute(exec)
    .await?;

    Ok(id)
}

/// Get a media root by ID.
pub async fn get_by_id<'e, E>(exec: E, id: Uuid) -> Result<Option<MediaRootRow>>
where
    E: Executor<'e, Database = Postgres>,
{
    let row = sqlx::query(
        r#"
        SELECT id, path, name, media_type, enabled, auto_add,
               scan_status, last_scan_at, scan_error, error_count, next_retry_at,
               naming_template, default_entitlement_scope, write_sidecars, write_images,
               is_default, created_at, updated_at, max_concurrent_io,
               scan_interval_seconds, last_scan_completed_at
        FROM media_roots WHERE id = $1
        "#,
    )
    .bind(id)
    .fetch_optional(exec)
    .await?;

    Ok(row.map(|r| MediaRootRow {
        id: r.get("id"),
        path: r.get("path"),
        name: r.get("name"),
        media_type: r.get("media_type"),
        enabled: r.get("enabled"),
        auto_add: r.get("auto_add"),
        scan_status: r.get("scan_status"),
        last_scan_at: r.get("last_scan_at"),
        scan_error: r.get("scan_error"),
        error_count: r.get("error_count"),
        next_retry_at: r.get("next_retry_at"),
        naming_template: r.get("naming_template"),
        default_entitlement_scope: r.get("default_entitlement_scope"),
        write_sidecars: r.get("write_sidecars"),
        write_images: r.get("write_images"),
        is_default: r.get("is_default"),
        created_at: r.get("created_at"),
        updated_at: r.get("updated_at"),
        max_concurrent_io: r.get("max_concurrent_io"),
        scan_interval_seconds: r.get("scan_interval_seconds"),
        last_scan_completed_at: r.get("last_scan_completed_at"),
    }))
}

/// Get a media root by ID, returning the enabled subset of fields.
pub async fn get_enabled_by_id<'e, E>(exec: E, id: Uuid) -> Result<Option<EnabledMediaRoot>>
where
    E: Executor<'e, Database = Postgres>,
{
    let row = sqlx::query(
        r#"
        SELECT id, path, name, media_type, auto_add, error_count, next_retry_at,
               naming_template, write_sidecars, write_images, is_default,
               scan_interval_seconds, last_scan_completed_at
        FROM media_roots WHERE id = $1 AND enabled = TRUE
        "#,
    )
    .bind(id)
    .fetch_optional(exec)
    .await?;

    Ok(row.map(|r| EnabledMediaRoot {
        id: r.get("id"),
        path: r.get("path"),
        name: r.get("name"),
        media_type: r.get("media_type"),
        auto_add: r.get("auto_add"),
        error_count: r.get("error_count"),
        next_retry_at: r.get("next_retry_at"),
        naming_template: r.get("naming_template"),
        write_sidecars: r.get("write_sidecars"),
        write_images: r.get("write_images"),
        is_default: r.get("is_default"),
        scan_interval_seconds: r.get("scan_interval_seconds"),
        last_scan_completed_at: r.get("last_scan_completed_at"),
    }))
}

/// List all media roots.
pub async fn list_all<'e, E>(exec: E) -> Result<Vec<MediaRootRow>>
where
    E: Executor<'e, Database = Postgres>,
{
    let rows = sqlx::query(
        r#"
        SELECT id, path, name, media_type, enabled, auto_add,
               scan_status, last_scan_at, scan_error, error_count, next_retry_at,
               naming_template, default_entitlement_scope, write_sidecars, write_images,
               is_default, created_at, updated_at, max_concurrent_io,
               scan_interval_seconds, last_scan_completed_at
        FROM media_roots ORDER BY created_at
        "#,
    )
    .fetch_all(exec)
    .await?;

    Ok(rows
        .into_iter()
        .map(|r| MediaRootRow {
            id: r.get("id"),
            path: r.get("path"),
            name: r.get("name"),
            media_type: r.get("media_type"),
            enabled: r.get("enabled"),
            auto_add: r.get("auto_add"),
            scan_status: r.get("scan_status"),
            last_scan_at: r.get("last_scan_at"),
            scan_error: r.get("scan_error"),
            error_count: r.get("error_count"),
            next_retry_at: r.get("next_retry_at"),
            naming_template: r.get("naming_template"),
            default_entitlement_scope: r.get("default_entitlement_scope"),
            write_sidecars: r.get("write_sidecars"),
            write_images: r.get("write_images"),
            is_default: r.get("is_default"),
            created_at: r.get("created_at"),
            updated_at: r.get("updated_at"),
            max_concurrent_io: r.get("max_concurrent_io"),
            scan_interval_seconds: r.get("scan_interval_seconds"),
            last_scan_completed_at: r.get("last_scan_completed_at"),
        })
        .collect())
}

/// List all enabled media roots (for scan check handler).
pub async fn get_all_enabled<'e, E>(exec: E) -> Result<Vec<EnabledMediaRoot>>
where
    E: Executor<'e, Database = Postgres>,
{
    let rows = sqlx::query(
        r#"
        SELECT id, path, name, media_type, auto_add, error_count, next_retry_at,
               naming_template, write_sidecars, write_images, is_default,
               scan_interval_seconds, last_scan_completed_at
        FROM media_roots WHERE enabled = TRUE ORDER BY created_at
        "#,
    )
    .fetch_all(exec)
    .await?;

    Ok(rows
        .into_iter()
        .map(|r| EnabledMediaRoot {
            id: r.get("id"),
            path: r.get("path"),
            name: r.get("name"),
            media_type: r.get("media_type"),
            auto_add: r.get("auto_add"),
            error_count: r.get("error_count"),
            next_retry_at: r.get("next_retry_at"),
            naming_template: r.get("naming_template"),
            write_sidecars: r.get("write_sidecars"),
            write_images: r.get("write_images"),
            is_default: r.get("is_default"),
            scan_interval_seconds: r.get("scan_interval_seconds"),
            last_scan_completed_at: r.get("last_scan_completed_at"),
        })
        .collect())
}

/// Update a media root.
pub async fn update<'e, E>(exec: E, id: Uuid, input: MediaRootUpdate) -> Result<bool>
where
    E: Executor<'e, Database = Postgres>,
{
    let result = sqlx::query(
        r#"
        UPDATE media_roots SET
            path = COALESCE($2, path),
            name = COALESCE($3, name),
            enabled = COALESCE($4, enabled),
            auto_add = COALESCE($5, auto_add),
            naming_template = CASE WHEN $6 THEN $7 ELSE naming_template END,
            default_entitlement_scope = COALESCE($8, default_entitlement_scope),
            write_sidecars = COALESCE($9, write_sidecars),
            write_images = COALESCE($10, write_images),
            is_default = COALESCE($11, is_default),
            max_concurrent_io = COALESCE($12, max_concurrent_io),
            scan_interval_seconds = CASE WHEN $13 THEN $14 ELSE scan_interval_seconds END,
            updated_at = NOW()
        WHERE id = $1
        "#,
    )
    .bind(id)
    .bind(input.path)
    .bind(input.name)
    .bind(input.enabled)
    .bind(input.auto_add)
    .bind(input.naming_template.is_some()) // flag: should we update naming_template?
    .bind(input.naming_template.flatten()) // the actual value (could be NULL)
    .bind(input.default_entitlement_scope)
    .bind(input.write_sidecars)
    .bind(input.write_images)
    .bind(input.is_default)
    .bind(input.max_concurrent_io)
    .bind(input.scan_interval_seconds.is_some())
    .bind(input.scan_interval_seconds.flatten())
    .execute(exec)
    .await?;

    Ok(result.rows_affected() > 0)
}

/// Resolve the media_root_ids associated with a file for a given media type.
/// Returns a deduped Vec — a single movie file belongs to one root; a single
/// episode file can theoretically map to multiple episodes (multi-episode
/// files) spanning one show, so the result is 0 or 1 root in practice but
/// the API is plural for future flexibility.
///
/// Returns an empty Vec when the media type doesn't live under a media root
/// (e.g. IPTV recordings) or when the entity has no media root yet.
pub async fn resolve_media_root_ids_for_file(
    pool: &sqlx::PgPool,
    media_type: &str,
    file_id: Uuid,
) -> Result<Vec<Uuid>> {
    let rows: Vec<Uuid> = match media_type {
        "movie" => sqlx::query_scalar!(
            r#"
            SELECT m.media_root_id AS "media_root_id?: Uuid"
            FROM movie_files mf
            JOIN movies m ON m.id = mf.movie_id
            WHERE mf.id = $1
            "#,
            file_id,
        )
        .fetch_all(pool)
        .await?
        .into_iter()
        .flatten()
        .collect(),
        "episode" => sqlx::query_scalar!(
            r#"
            SELECT DISTINCT s.media_root_id AS "media_root_id?: Uuid"
            FROM tv_episode_file_episodes tefe
            JOIN tv_episodes e ON e.id = tefe.episode_id
            JOIN tv_seasons ss ON ss.id = e.season_id
            JOIN tv_shows s ON s.id = ss.show_id
            WHERE tefe.episode_file_id = $1
            "#,
            file_id,
        )
        .fetch_all(pool)
        .await?
        .into_iter()
        .flatten()
        .collect(),
        _ => Vec::new(),
    };
    let mut ids = rows;
    ids.sort();
    ids.dedup();
    Ok(ids)
}

/// Resolve the media_root_id for a TV season (via its show).
pub async fn resolve_media_root_id_for_season(
    pool: &sqlx::PgPool,
    season_id: Uuid,
) -> Result<Option<Uuid>> {
    let row = sqlx::query_scalar!(
        r#"
        SELECT s.media_root_id AS "media_root_id?: Uuid"
        FROM tv_seasons ss
        JOIN tv_shows s ON s.id = ss.show_id
        WHERE ss.id = $1
        "#,
        season_id,
    )
    .fetch_optional(pool)
    .await?;
    Ok(row.flatten())
}

/// Resolve the media_root_id for a movie file.
pub async fn resolve_media_root_id_for_movie_file(
    pool: &sqlx::PgPool,
    movie_file_id: Uuid,
) -> Result<Option<Uuid>> {
    let row = sqlx::query_scalar!(
        r#"
        SELECT m.media_root_id AS "media_root_id?: Uuid"
        FROM movie_files mf
        JOIN movies m ON m.id = mf.movie_id
        WHERE mf.id = $1
        "#,
        movie_file_id,
    )
    .fetch_optional(pool)
    .await?;
    Ok(row.flatten())
}

/// Get the `max_concurrent_io` cap for a media root.
/// Returns the DB value, or `1` if the row is missing / unreadable.
pub async fn get_max_concurrent_io(pool: &sqlx::PgPool, id: Uuid) -> Result<i32> {
    let row = sqlx::query_scalar!(
        "SELECT max_concurrent_io FROM media_roots WHERE id = $1",
        id,
    )
    .fetch_optional(pool)
    .await?;
    Ok(row.unwrap_or(1))
}

/// Delete a media root.
pub async fn delete<'e, E>(exec: E, id: Uuid) -> Result<bool>
where
    E: Executor<'e, Database = Postgres>,
{
    let result = sqlx::query("DELETE FROM media_roots WHERE id = $1")
        .bind(id)
        .execute(exec)
        .await?;
    Ok(result.rows_affected() > 0)
}

// =============================================================================
// Default media root
// =============================================================================

/// Get the default enabled media root for a given media type.
pub async fn get_default_for_type<'e, E>(
    exec: E,
    media_type: &str,
) -> Result<Option<EnabledMediaRoot>>
where
    E: Executor<'e, Database = Postgres>,
{
    let row = sqlx::query(
        r#"
        SELECT id, path, name, media_type, auto_add, error_count, next_retry_at,
               naming_template, write_sidecars, write_images, is_default,
               scan_interval_seconds, last_scan_completed_at
        FROM media_roots
        WHERE media_type = $1 AND is_default = TRUE AND enabled = TRUE
        "#,
    )
    .bind(media_type)
    .fetch_optional(exec)
    .await?;

    Ok(row.map(|r| EnabledMediaRoot {
        id: r.get("id"),
        path: r.get("path"),
        name: r.get("name"),
        media_type: r.get("media_type"),
        auto_add: r.get("auto_add"),
        error_count: r.get("error_count"),
        next_retry_at: r.get("next_retry_at"),
        naming_template: r.get("naming_template"),
        write_sidecars: r.get("write_sidecars"),
        write_images: r.get("write_images"),
        is_default: r.get("is_default"),
        scan_interval_seconds: r.get("scan_interval_seconds"),
        last_scan_completed_at: r.get("last_scan_completed_at"),
    }))
}

/// Set a media root as the default for its type.
/// Unsets any existing default for the same type first.
pub async fn set_default(pool: &sqlx::PgPool, id: Uuid) -> Result<bool> {
    let mut tx = pool.begin().await?;

    // Get the media_type of the target root
    let row = sqlx::query("SELECT media_type FROM media_roots WHERE id = $1")
        .bind(id)
        .fetch_optional(&mut *tx)
        .await?;

    let Some(row) = row else {
        return Ok(false);
    };
    let media_type: String = row.get("media_type");

    // Unset existing default for this type
    sqlx::query(
        r#"
        UPDATE media_roots SET is_default = FALSE, updated_at = NOW()
        WHERE media_type = $1 AND is_default = TRUE
        "#,
    )
    .bind(&media_type)
    .execute(&mut *tx)
    .await?;

    // Set the new default
    let result = sqlx::query(
        r#"
        UPDATE media_roots SET is_default = TRUE, updated_at = NOW()
        WHERE id = $1
        "#,
    )
    .bind(id)
    .execute(&mut *tx)
    .await?;

    tx.commit().await?;
    Ok(result.rows_affected() > 0)
}

/// Check if any default exists for the given media type.
pub async fn has_default_for_type<'e, E>(exec: E, media_type: &str) -> Result<bool>
where
    E: Executor<'e, Database = Postgres>,
{
    let row = sqlx::query(
        "SELECT EXISTS(SELECT 1 FROM media_roots WHERE media_type = $1 AND is_default = TRUE) AS has_default",
    )
    .bind(media_type)
    .fetch_one(exec)
    .await?;

    Ok(row.get::<bool, _>("has_default"))
}

// =============================================================================
// Scan state management
// =============================================================================

/// Set scan status to scanning.
pub async fn set_scanning<'e, E>(exec: E, id: Uuid) -> Result<()>
where
    E: Executor<'e, Database = Postgres>,
{
    sqlx::query(
        r#"
        UPDATE media_roots SET scan_status = 'scanning', updated_at = NOW()
        WHERE id = $1
        "#,
    )
    .bind(id)
    .execute(exec)
    .await?;
    Ok(())
}

/// Set scan as completed successfully.
pub async fn set_scan_completed<'e, E>(exec: E, id: Uuid) -> Result<()>
where
    E: Executor<'e, Database = Postgres>,
{
    sqlx::query(
        r#"
        UPDATE media_roots SET
            scan_status = 'idle',
            last_scan_at = NOW(),
            last_scan_completed_at = NOW(),
            scan_error = NULL,
            error_count = 0,
            next_retry_at = NULL,
            updated_at = NOW()
        WHERE id = $1
        "#,
    )
    .bind(id)
    .execute(exec)
    .await?;
    Ok(())
}

/// Stamp `last_scan_completed_at = NOW()` without touching scan status.
///
/// Called from the scan handler at partial-batch boundaries so that the
/// `media_root_scan_check` interval gate recognises that the scan is making
/// progress even while continuation re-enqueues are still in flight.
pub async fn touch_last_scan_completed_at<'e, E>(exec: E, id: Uuid) -> Result<()>
where
    E: Executor<'e, Database = Postgres>,
{
    sqlx::query(
        "UPDATE media_roots SET last_scan_completed_at = NOW(), updated_at = NOW() WHERE id = $1",
    )
    .bind(id)
    .execute(exec)
    .await?;
    Ok(())
}

/// Clear error state and backoff (e.g. for manual re-scan).
pub async fn clear_error<'e, E>(exec: E, id: Uuid) -> Result<()>
where
    E: Executor<'e, Database = Postgres>,
{
    sqlx::query(
        r#"
        UPDATE media_roots SET
            scan_status = 'idle',
            scan_error = NULL,
            error_count = 0,
            next_retry_at = NULL,
            updated_at = NOW()
        WHERE id = $1
        "#,
    )
    .bind(id)
    .execute(exec)
    .await?;
    Ok(())
}

/// Set scan as errored with backoff.
pub async fn set_scan_error<'e, E>(
    exec: E,
    id: Uuid,
    error: &str,
    next_retry_at: OffsetDateTime,
) -> Result<()>
where
    E: Executor<'e, Database = Postgres>,
{
    sqlx::query(
        r#"
        UPDATE media_roots SET
            scan_status = 'error',
            scan_error = $2,
            error_count = error_count + 1,
            next_retry_at = $3,
            updated_at = NOW()
        WHERE id = $1
        "#,
    )
    .bind(id)
    .bind(error)
    .bind(next_retry_at)
    .execute(exec)
    .await?;
    Ok(())
}

// =============================================================================
// Availability check
// =============================================================================

/// Media root job types that touch the filesystem and should be cancelled
/// when a root goes down (and re-created by scan_check on recovery).
pub const FS_JOB_TYPES: &[&str] = &[
    "media_root_scan",
    "media_root_interpret",
    "media_root_reorganize",
    "media_root_write_sidecars",
    "media_root_write_images",
    "media_root_book_convert",
];

/// Check if a media root's path is currently accessible.
///
/// - If the path does not exist, sets scan_error with exponential backoff and
///   returns `false`.
/// - If the path exists and the root was previously in a "Directory not found"
///   error state, clears the error and cancels stale pending jobs (the scan
///   scheduler will create fresh ones).
pub async fn check_available(pool: &sqlx::PgPool, root: &MediaRootRow) -> Result<bool> {
    let root_path = std::path::Path::new(&root.path).to_owned();
    let exists = tokio::task::spawn_blocking(move || root_path.exists())
        .await
        .unwrap_or(false);

    if !exists {
        // Only bump error_count if not already in a "Directory not found" error state
        // (avoids double-incrementing when multiple handlers check the same root)
        if root.scan_status != "error"
            || !root
                .scan_error
                .as_deref()
                .is_some_and(|e| e.starts_with("Directory not found:"))
        {
            let next_retry = time::OffsetDateTime::now_utc()
                + time::Duration::minutes(std::cmp::min(60, 2_i64.pow(root.error_count as u32)));
            set_scan_error(
                pool,
                root.id,
                &format!("Directory not found: {}", root.path),
                next_retry,
            )
            .await?;
        }
        return Ok(false);
    }

    // Root is back — clear error if it was a "Directory not found" state
    if root.scan_status == "error"
        && root
            .scan_error
            .as_deref()
            .is_some_and(|e| e.starts_with("Directory not found:"))
    {
        tracing::info!(
            media_root_id = %root.id,
            path = %root.path,
            "Media root path is accessible again, clearing error state"
        );
        clear_error(pool, root.id).await?;

        // Cancel stale pending jobs — the scan_check scheduler will create fresh ones
        let db = nats_jobs::JobStatusDb::new(pool.clone());
        let root_id_str = root.id.to_string();
        for job_type in FS_JOB_TYPES {
            let _ = db
                .cancel_pending_jobs_with_field(
                    job_type,
                    "media_root_id",
                    &root_id_str,
                    "media_root_id",
                )
                .await;
        }
    }

    Ok(true)
}

// =============================================================================
// Stats
// =============================================================================

/// Get stats for a media root.
pub async fn get_stats<'e, E>(exec: E, media_root_id: Uuid) -> Result<MediaRootStats>
where
    E: Executor<'e, Database = Postgres>,
{
    let row = sqlx::query(
        r#"
        SELECT
            COUNT(*) FILTER (WHERE available = TRUE) AS total_files,
            COUNT(*) FILTER (WHERE available = TRUE AND match_status = 'added') AS added,
            COUNT(*) FILTER (WHERE available = TRUE AND match_status = 'review') AS review,
            COUNT(*) FILTER (WHERE available = TRUE AND match_status = 'ambiguous') AS ambiguous,
            COUNT(*) FILTER (WHERE available = TRUE AND match_status = 'unmatched') AS unmatched,
            COUNT(*) FILTER (WHERE available = TRUE AND match_status = 'ignored') AS ignored,
            COUNT(*) FILTER (WHERE available = TRUE AND match_status = 'pending') AS pending,
            COUNT(*) FILTER (WHERE available = TRUE AND match_status = 'identified') AS identified,
            COUNT(*) FILTER (WHERE available = FALSE) AS unavailable
        FROM media_root_files WHERE media_root_id = $1
        "#,
    )
    .bind(media_root_id)
    .fetch_one(exec)
    .await?;

    Ok(MediaRootStats {
        total_files: row.get::<i64, _>("total_files"),
        added: row.get::<i64, _>("added"),
        review: row.get::<i64, _>("review"),
        ambiguous: row.get::<i64, _>("ambiguous"),
        unmatched: row.get::<i64, _>("unmatched"),
        ignored: row.get::<i64, _>("ignored"),
        pending: row.get::<i64, _>("pending"),
        identified: row.get::<i64, _>("identified"),
        unavailable: row.get::<i64, _>("unavailable"),
    })
}

/// Look up the storage_kind for a media root: `"local"` or `"network"` (default).
/// Used by Phase 5+ ingest dual-write to skip the local-cache copy when the
/// media_root is already on local storage (no NAS round-trip to save).
pub async fn get_storage_kind<'e, E>(exec: E, id: Uuid) -> Result<Option<String>>
where
    E: Executor<'e, Database = Postgres>,
{
    let row = sqlx::query!("SELECT storage_kind FROM media_roots WHERE id = $1", id)
        .fetch_optional(exec)
        .await?;
    Ok(row.map(|r| r.storage_kind))
}

/// Whether ingest dual-write should tee bytes into local cache for files
/// landing on this media root. Returns `true` for network storage (the
/// default) and `false` for local storage. Returns `false` if the media
/// root doesn't exist.
pub async fn dual_write_enabled<'e, E>(exec: E, id: Uuid) -> Result<bool>
where
    E: Executor<'e, Database = Postgres>,
{
    Ok(matches!(
        get_storage_kind(exec, id).await?.as_deref(),
        Some("network") | None
    ))
    // Treat unknown/missing media roots as network — fail open, it's just an
    // opportunistic optimisation.
}
