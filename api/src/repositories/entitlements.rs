//! Entitlements repository operations.

use super::Result;
use crate::types::EntitlementScope;
use sqlx::{Executor, PgPool, Postgres};
use uuid::Uuid;

/// Create an entitlement with specific scope.
/// Uses ON CONFLICT to prevent duplicate rows.
pub async fn create<'e, E>(
    exec: E,
    book_id: Uuid,
    scope_type: EntitlementScope,
    scope_id: Option<Uuid>,
) -> Result<Uuid>
where
    E: Executor<'e, Database = Postgres>,
{
    let id = Uuid::now_v7();
    sqlx::query!(
        r#"
        INSERT INTO entitlements (id, book_id, scope_type, scope_id, is_removed)
        VALUES ($1, $2, $3, $4, FALSE)
        ON CONFLICT (book_id, scope_type, scope_id) DO NOTHING
        "#,
        id,
        book_id,
        scope_type as EntitlementScope,
        scope_id,
    )
    .execute(exec)
    .await?;
    Ok(id)
}

/// Check if an entitlement exists for a book and device.
pub async fn exists_for_device<'e, E>(exec: E, book_id: Uuid, device_id: Uuid) -> Result<bool>
where
    E: Executor<'e, Database = Postgres>,
{
    let exists: bool = sqlx::query_scalar!(
        r#"SELECT EXISTS(SELECT 1 FROM entitlements WHERE book_id = $1 AND scope_type = 'DEVICE' AND scope_id = $2) as "exists!""#,
        book_id,
        device_id,
    )
    .fetch_one(exec)
    .await?;
    Ok(exists)
}

/// Delete all entitlements for a book.
pub async fn delete_by_book<'e, E>(exec: E, book_id: Uuid) -> Result<u64>
where
    E: Executor<'e, Database = Postgres>,
{
    let result = sqlx::query!("DELETE FROM entitlements WHERE book_id = $1", book_id)
        .execute(exec)
        .await?;
    Ok(result.rows_affected())
}

/// Touch entitlements for multiple books (update updated_at).
pub async fn touch_by_books<'e, E>(exec: E, book_ids: &[Uuid]) -> Result<()>
where
    E: Executor<'e, Database = Postgres>,
{
    sqlx::query!(
        "UPDATE entitlements SET updated_at = NOW() WHERE book_id = ANY($1)",
        book_ids,
    )
    .execute(exec)
    .await?;
    Ok(())
}

/// Mark an entitlement as removed for a device.
pub async fn mark_removed_for_device<'e, E>(exec: E, book_id: Uuid, device_id: Uuid) -> Result<bool>
where
    E: Executor<'e, Database = Postgres>,
{
    let result = sqlx::query!(
        "UPDATE entitlements SET is_removed = TRUE, updated_at = NOW() WHERE book_id = $1 AND scope_type = 'DEVICE' AND scope_id = $2",
        book_id,
        device_id,
    )
    .execute(exec)
    .await?;
    Ok(result.rows_affected() > 0)
}

/// Unmark an entitlement as removed for a device.
pub async fn unmark_removed_for_device<'e, E>(
    exec: E,
    book_id: Uuid,
    device_id: Uuid,
) -> Result<bool>
where
    E: Executor<'e, Database = Postgres>,
{
    let result = sqlx::query!(
        "UPDATE entitlements SET is_removed = FALSE, updated_at = NOW() WHERE book_id = $1 AND scope_type = 'DEVICE' AND scope_id = $2",
        book_id,
        device_id,
    )
    .execute(exec)
    .await?;
    Ok(result.rows_affected() > 0)
}

/// Mark all entitlements as removed for a book.
pub async fn mark_removed_by_book<'e, E>(exec: E, book_id: Uuid) -> Result<u64>
where
    E: Executor<'e, Database = Postgres>,
{
    let result = sqlx::query!(
        "UPDATE entitlements SET is_removed = TRUE, updated_at = NOW() WHERE book_id = $1",
        book_id,
    )
    .execute(exec)
    .await?;
    Ok(result.rows_affected())
}

/// Mark all entitlements as removed for multiple books.
pub async fn mark_removed_by_books<'e, E>(exec: E, book_ids: &[Uuid]) -> Result<u64>
where
    E: Executor<'e, Database = Postgres>,
{
    let result = sqlx::query!(
        "UPDATE entitlements SET is_removed = TRUE, updated_at = NOW() WHERE book_id = ANY($1)",
        book_ids,
    )
    .execute(exec)
    .await?;
    Ok(result.rows_affected())
}

/// Unmark all entitlements as removed for a book.
pub async fn unmark_removed_by_book<'e, E>(exec: E, book_id: Uuid) -> Result<u64>
where
    E: Executor<'e, Database = Postgres>,
{
    let result = sqlx::query!(
        "UPDATE entitlements SET is_removed = FALSE, updated_at = NOW() WHERE book_id = $1",
        book_id,
    )
    .execute(exec)
    .await?;
    Ok(result.rows_affected())
}

/// Create an entitlement if it doesn't already exist (ON CONFLICT DO NOTHING).
/// Returns the ID if created, None if already existed.
pub async fn create_if_not_exists<'e, E>(
    exec: E,
    book_id: Uuid,
    scope_type: EntitlementScope,
    scope_id: Option<Uuid>,
) -> Result<Option<Uuid>>
where
    E: Executor<'e, Database = Postgres>,
{
    let id = Uuid::now_v7();
    let result: Option<Uuid> = sqlx::query_scalar!(
        r#"
        INSERT INTO entitlements (id, book_id, scope_type, scope_id, is_removed)
        VALUES ($1, $2, $3, $4, FALSE)
        ON CONFLICT (book_id, scope_type, scope_id) DO NOTHING
        RETURNING id
        "#,
        id,
        book_id,
        scope_type as EntitlementScope,
        scope_id,
    )
    .fetch_optional(exec)
    .await?;

    Ok(result)
}

/// Create an entitlement or restore it if it was removed.
/// If entitlement exists and is_removed=true, restores it.
/// If entitlement doesn't exist, creates it.
pub async fn create_or_restore<'e, E>(
    exec: E,
    book_id: Uuid,
    scope_type: EntitlementScope,
    scope_id: Option<Uuid>,
) -> Result<Uuid>
where
    E: Executor<'e, Database = Postgres>,
{
    let id = Uuid::new_v4();
    let result: Uuid = sqlx::query_scalar!(
        r#"
        WITH updated AS (
            UPDATE entitlements
            SET is_removed = FALSE, updated_at = NOW()
            WHERE book_id = $1 AND scope_type = $2 AND scope_id IS NOT DISTINCT FROM $3
            RETURNING id
        ),
        inserted AS (
            INSERT INTO entitlements (id, book_id, scope_type, scope_id, is_removed)
            SELECT $4, $1, $2, $3, FALSE
            WHERE NOT EXISTS (SELECT 1 FROM updated)
            ON CONFLICT (book_id, scope_type, scope_id) DO NOTHING
            RETURNING id
        )
        SELECT id AS "id!" FROM updated
        UNION ALL
        SELECT id AS "id!" FROM inserted
        "#,
        book_id,
        scope_type as EntitlementScope,
        scope_id,
        id,
    )
    .fetch_one(exec)
    .await?;

    Ok(result)
}

/// Row returned from sync query.
#[derive(Debug)]
pub struct SyncEntitlementRow {
    pub book_id: Uuid,
    pub is_removed: bool,
    pub entitlement_created_at: time::OffsetDateTime,
    pub entitlement_updated_at: time::OffsetDateTime,
    pub title: String,
    pub description: Option<String>,
    pub language: Option<String>,
    pub publisher: Option<String>,
    pub pubdate: Option<time::OffsetDateTime>,
    pub series_id: Option<Uuid>,
    pub series_name: Option<String>,
    pub series_index: Option<f64>,
    pub identifiers: Option<serde_json::Value>,
    pub page_count: Option<i32>,
    pub rating: Option<f64>,
    pub book_created_at: time::OffsetDateTime,
    pub book_updated_at: time::OffsetDateTime,
    pub files: serde_json::Value,
    pub authors: serde_json::Value,
    pub reading_state: Option<serde_json::Value>,
}

/// List entitlements for Kobo sync since a timestamp.
///
/// Deduplicates by book_id using scope priority: DEVICE > USER > ALL_USERS.
/// When a book has entitlements at multiple scopes, only the most specific
/// (highest priority) scope is returned. The `updated_at` timestamp is the
/// MAX across all visible entitlements so any scope change triggers a sync.
pub async fn list_for_sync<'e, E>(
    exec: E,
    user_id: Uuid,
    device_id: Uuid,
    since: time::OffsetDateTime,
    limit: i64,
) -> Result<Vec<SyncEntitlementRow>>
where
    E: Executor<'e, Database = Postgres>,
{
    let rows = sqlx::query!(
        r#"
        WITH visible_entitlements AS (
            -- All entitlements visible to this device, with scope priority
            SELECT e.*,
                   CASE e.scope_type
                       WHEN 'DEVICE'    THEN 1
                       WHEN 'USER'      THEN 2
                       WHEN 'ALL_USERS' THEN 3
                   END AS scope_priority
            FROM entitlements e
            WHERE (
                e.scope_type = 'ALL_USERS'
                OR (e.scope_type = 'USER' AND e.scope_id = $1)
                OR (e.scope_type = 'DEVICE' AND e.scope_id = $2)
            )
        ),
        deduped AS (
            -- Pick the most specific scope per book.
            -- Use the winning scope's is_removed and created_at,
            -- but MAX updated_at across all scopes so any change triggers sync.
            SELECT DISTINCT ON (ve.book_id)
                ve.book_id,
                ve.is_removed,
                ve.created_at,
                (SELECT MAX(v2.updated_at) FROM visible_entitlements v2 WHERE v2.book_id = ve.book_id) AS updated_at
            FROM visible_entitlements ve
            ORDER BY ve.book_id, ve.scope_priority ASC
        )
        SELECT d.book_id, d.is_removed,
               d.created_at AS entitlement_created_at, d.updated_at AS "entitlement_updated_at!",
               b.title, b.description, b.language, b.publisher, b.pubdate,
               bs.series_id AS "series_id?", s.name AS "series_name?", bs.series_index AS "series_index?", b.identifiers,
               b.page_count, b.rating, b.created_at AS book_created_at, b.updated_at AS book_updated_at,
               COALESCE(
                   (SELECT jsonb_agg(jsonb_build_object('format', bf.format, 'size_bytes', bf.size_bytes, 'content_revision', bf.content_revision))
                    FROM book_files bf WHERE bf.book_id = d.book_id),
                   '[]'::jsonb
               ) AS "files!",
               COALESCE(
                   (SELECT jsonb_agg(a.name ORDER BY a.name)
                    FROM book_authors ba JOIN authors a ON a.id = ba.author_id
                    WHERE ba.book_id = d.book_id),
                   '[]'::jsonb
               ) AS "authors!",
               (SELECT jsonb_build_object(
                   'id', rs.id, 'book_id', rs.book_id, 'device_id', rs.device_id,
                   'created_at', rs.created_at, 'updated_at', rs.updated_at,
                   'priority_timestamp', rs.priority_timestamp,
                   'client_payload', rs.client_payload,
                   'status', st.status, 'status_last_modified', st.last_modified_at,
                   'times_started', st.times_started_reading, 'last_time_started', st.last_time_started_reading,
                   'spent_reading_minutes', stats.spent_reading_minutes,
                   'remaining_time_minutes', stats.remaining_time_minutes,
                   'stats_last_modified', stats.last_modified_at,
                   'progress_percent', bm.progress_percent,
                   'content_source_progress_percent', bm.content_source_progress_percent,
                   'location_value', bm.location_value, 'location_type', bm.location_type,
                   'location_source', bm.location_source,
                   'kobo_location_value', bm.kobo_location_value,
                   'kobo_location_source', bm.kobo_location_source,
                   'bookmark_last_modified', bm.last_modified_at
               )
               FROM reading_states rs
               JOIN reading_status st ON st.reading_state_id = rs.id
               JOIN reading_statistics stats ON stats.reading_state_id = rs.id
               JOIN reading_bookmark bm ON bm.reading_state_id = rs.id
               WHERE rs.user_id = $1 AND rs.book_id = d.book_id
               ORDER BY CASE WHEN st.status = 'Reading' THEN 0
                             WHEN COALESCE(bm.progress_percent, 0) > 0 THEN 1
                             WHEN st.status = 'Finished' THEN 2 ELSE 3 END,
                        rs.updated_at DESC
               LIMIT 1
              ) AS reading_state
        FROM deduped d
        JOIN books b ON b.id = d.book_id
        LEFT JOIN book_series bs ON bs.book_id = b.id AND bs.is_primary = TRUE
        LEFT JOIN series s ON s.id = bs.series_id
        WHERE (
            d.updated_at > $3 OR d.created_at > $3
            OR b.updated_at > $3 OR b.created_at > $3
        )
        AND (
            -- Include if explicitly removed (need to tell device to delete)
            d.is_removed = TRUE
            -- Or if book has no files yet (metadata-only / newly created)
            OR NOT EXISTS (
                SELECT 1 FROM book_files bf
                WHERE bf.book_id = d.book_id
            )
            -- Or if at least one file is available
            OR EXISTS (
                SELECT 1 FROM book_files bf
                WHERE bf.book_id = d.book_id AND bf.available = TRUE
            )
        )
        -- Only sync approved library books (not items still in ingest queue)
        AND b.ingest_status IS NULL
        ORDER BY GREATEST(d.updated_at, b.updated_at) ASC, b.id ASC
        LIMIT $4
        "#,
        user_id,
        device_id,
        since,
        limit,
    )
    .fetch_all(exec)
    .await?;

    Ok(rows
        .into_iter()
        .map(|r| SyncEntitlementRow {
            book_id: r.book_id,
            is_removed: r.is_removed,
            entitlement_created_at: r.entitlement_created_at,
            entitlement_updated_at: r.entitlement_updated_at,
            title: r.title,
            description: r.description,
            language: r.language,
            publisher: r.publisher,
            pubdate: r.pubdate,
            series_id: r.series_id,
            series_name: r.series_name,
            series_index: r.series_index,
            identifiers: Some(r.identifiers),
            page_count: r.page_count,
            rating: r.rating,
            book_created_at: r.book_created_at,
            book_updated_at: r.book_updated_at,
            files: r.files,
            authors: r.authors,
            reading_state: r.reading_state,
        })
        .collect())
}

/// Count pending entitlements for Kobo sync since a timestamp.
///
/// Uses the same deduplication logic as `list_for_sync` to ensure the count
/// matches the actual number of items that would be returned.
pub async fn count_pending_sync<'e, E>(
    exec: E,
    user_id: Uuid,
    device_id: Uuid,
    since: time::OffsetDateTime,
) -> Result<i64>
where
    E: Executor<'e, Database = Postgres>,
{
    let count: i64 = sqlx::query_scalar!(
        r#"
        WITH visible_entitlements AS (
            SELECT e.*,
                   CASE e.scope_type
                       WHEN 'DEVICE'    THEN 1
                       WHEN 'USER'      THEN 2
                       WHEN 'ALL_USERS' THEN 3
                   END AS scope_priority
            FROM entitlements e
            WHERE (
                e.scope_type = 'ALL_USERS'
                OR (e.scope_type = 'USER' AND e.scope_id = $1)
                OR (e.scope_type = 'DEVICE' AND e.scope_id = $2)
            )
        ),
        deduped AS (
            SELECT DISTINCT ON (ve.book_id)
                ve.book_id,
                ve.is_removed,
                ve.created_at,
                (SELECT MAX(v2.updated_at) FROM visible_entitlements v2 WHERE v2.book_id = ve.book_id) AS updated_at
            FROM visible_entitlements ve
            ORDER BY ve.book_id, ve.scope_priority ASC
        )
        SELECT COUNT(*) AS "count!"
        FROM deduped d
        JOIN books b ON b.id = d.book_id
        WHERE (
            d.updated_at > $3 OR d.created_at > $3
            OR b.updated_at > $3 OR b.created_at > $3
        )
        AND (
            d.is_removed = TRUE
            OR NOT EXISTS (
                SELECT 1 FROM book_files bf
                WHERE bf.book_id = d.book_id
            )
            OR EXISTS (
                SELECT 1 FROM book_files bf
                WHERE bf.book_id = d.book_id AND bf.available = TRUE
            )
        )
        AND b.ingest_status IS NULL
        "#,
        user_id,
        device_id,
        since,
    )
    .fetch_one(exec)
    .await?;

    Ok(count)
}

/// Book summary for device entitlements listing.
#[derive(Debug)]
pub struct DeviceBookSummary {
    pub id: Uuid,
    pub title: String,
}

/// Device entitlement status for a book.
#[derive(Debug)]
pub struct BookDeviceStatus {
    pub device_id: Uuid,
    pub device_name: Option<String>,
    pub device_type: String,
    pub entitled: bool,
    pub is_removed: bool,
    pub scope_type: Option<String>,
}

/// Get all devices with their entitlement status for a specific book.
pub async fn list_book_devices(
    pool: &PgPool,
    book_id: Uuid,
) -> Result<(Vec<BookDeviceStatus>, bool)> {
    // Check if there's an ALL_USERS scope entitlement (not removed)
    let all_users: bool = sqlx::query_scalar!(
        r#"
        SELECT EXISTS(
            SELECT 1 FROM entitlements
            WHERE book_id = $1 AND scope_type = 'ALL_USERS' AND is_removed = FALSE
        ) as "exists!"
        "#,
        book_id,
    )
    .fetch_one(pool)
    .await?;

    // Get all devices with their DEVICE scope entitlement status for this book
    let rows = sqlx::query!(
        r#"
        SELECT
            d.id AS device_id,
            d.name AS device_name,
            d.device_type,
            CASE WHEN e.id IS NOT NULL THEN TRUE ELSE FALSE END AS "entitled!",
            COALESCE(e.is_removed, FALSE) AS "is_removed!",
            e.scope_type AS "scope_type?"
        FROM devices d
        LEFT JOIN entitlements e ON e.scope_id = d.id
            AND e.book_id = $1
            AND e.scope_type = 'DEVICE'
        WHERE d.device_type != 'web'
        ORDER BY d.name NULLS LAST, d.id
        "#,
        book_id,
    )
    .fetch_all(pool)
    .await?;

    let statuses = rows
        .into_iter()
        .map(|r| BookDeviceStatus {
            device_id: r.device_id,
            device_name: r.device_name,
            device_type: r.device_type,
            entitled: r.entitled,
            is_removed: r.is_removed,
            scope_type: r.scope_type,
        })
        .collect();

    Ok((statuses, all_users))
}

/// Delete the ALL_USERS entitlement for a book (marks as removed).
pub async fn mark_removed_all_users<'e, E>(exec: E, book_id: Uuid) -> Result<bool>
where
    E: Executor<'e, Database = Postgres>,
{
    let result = sqlx::query!(
        "UPDATE entitlements SET is_removed = TRUE, updated_at = NOW() WHERE book_id = $1 AND scope_type = 'ALL_USERS'",
        book_id,
    )
    .execute(exec)
    .await?;
    Ok(result.rows_affected() > 0)
}

/// Check if a book has an active ALL_USERS entitlement.
pub async fn has_all_users<'e, E>(exec: E, book_id: Uuid) -> Result<bool>
where
    E: Executor<'e, Database = Postgres>,
{
    let exists: bool = sqlx::query_scalar!(
        r#"SELECT EXISTS(SELECT 1 FROM entitlements WHERE book_id = $1 AND scope_type = 'ALL_USERS' AND is_removed = FALSE) as "exists!""#,
        book_id,
    )
    .fetch_one(exec)
    .await?;
    Ok(exists)
}

/// Create DEVICE entitlements for all devices except the excluded one.
/// Used when converting from ALL_USERS to per-device entitlements.
pub async fn create_device_entitlements_for_all_except<'e, E>(
    exec: E,
    book_id: Uuid,
    excluded_device_id: Uuid,
) -> Result<u64>
where
    E: Executor<'e, Database = Postgres>,
{
    let result = sqlx::query!(
        r#"
        INSERT INTO entitlements (id, book_id, scope_type, scope_id, is_removed)
        SELECT gen_random_uuid(), $1, 'DEVICE', d.id, FALSE
        FROM devices d
        WHERE d.id != $2 AND d.device_type != 'web'
        ON CONFLICT (book_id, scope_type, scope_id) DO NOTHING
        "#,
        book_id,
        excluded_device_id,
    )
    .execute(exec)
    .await?;
    Ok(result.rows_affected())
}

/// List books entitled to a device (DEVICE scope entitlements).
pub async fn list_device_books<'e, E>(exec: E, device_id: Uuid) -> Result<Vec<DeviceBookSummary>>
where
    E: Executor<'e, Database = Postgres>,
{
    let rows = sqlx::query!(
        r#"
        SELECT DISTINCT b.id, b.title
        FROM books b
        JOIN entitlements e ON e.book_id = b.id
        WHERE e.scope_type = 'DEVICE' AND e.scope_id = $1 AND e.is_removed = FALSE
        ORDER BY b.title
        "#,
        device_id,
    )
    .fetch_all(exec)
    .await?;

    Ok(rows
        .into_iter()
        .map(|r| DeviceBookSummary {
            id: r.id,
            title: r.title,
        })
        .collect())
}

/// Create DEVICE entitlements for a book on specific devices.
/// Used by auto-sync to create per-device entitlements for opted-in devices.
pub async fn create_for_auto_sync_devices<'e, E>(
    exec: E,
    book_id: Uuid,
    device_ids: &[Uuid],
) -> Result<u64>
where
    E: Executor<'e, Database = Postgres>,
{
    let result = sqlx::query!(
        r#"
        INSERT INTO entitlements (id, book_id, scope_type, scope_id, is_removed)
        SELECT gen_random_uuid(), $1, 'DEVICE', unnest($2::uuid[]), FALSE
        ON CONFLICT (book_id, scope_type, scope_id) DO UPDATE
            SET is_removed = FALSE, updated_at = NOW()
            WHERE entitlements.is_removed = TRUE
        "#,
        book_id,
        device_ids,
    )
    .execute(exec)
    .await?;
    Ok(result.rows_affected())
}

/// Delete all DEVICE-scoped entitlements for a device.
/// Used during device deletion to prevent orphaned entitlements.
pub async fn delete_by_device_scope<'e, E>(exec: E, device_id: Uuid) -> Result<u64>
where
    E: Executor<'e, Database = Postgres>,
{
    let result = sqlx::query!(
        "DELETE FROM entitlements WHERE scope_type = 'DEVICE' AND scope_id = $1",
        device_id,
    )
    .execute(exec)
    .await?;
    Ok(result.rows_affected())
}

/// Books a Kobo is entitled to that still have no KEPUB.
///
/// KEPUB is what the Kobo prefers — `kobo::metadata` offers it over the EPUB
/// whenever one exists, and the koboSpan markup is what carries reading
/// position and the full-page image transform. So the set worth converting is
/// exactly the set some Kobo can see.
///
/// The scope test mirrors `list_for_sync`: ALL_USERS reaches every device, USER
/// reaches that user's devices, DEVICE reaches just that one. A `web` device is
/// deliberately not a reason to convert — the web reader reads the EPUB.
pub async fn list_synced_without_kepub<'e, E>(exec: E) -> Result<Vec<(Uuid, String)>>
where
    E: Executor<'e, Database = Postgres>,
{
    let rows = sqlx::query!(
        r#"
        SELECT DISTINCT e.book_id, b.title
        FROM entitlements e
        JOIN books b ON b.id = e.book_id
        WHERE NOT e.is_removed
          AND b.ingest_status IS NULL
          AND EXISTS (
              SELECT 1 FROM devices d
              WHERE d.device_type = 'kobo'
                AND (
                    e.scope_type = 'ALL_USERS'
                    OR (e.scope_type = 'USER' AND d.user_id = e.scope_id)
                    OR (e.scope_type = 'DEVICE' AND d.id = e.scope_id)
                )
          )
          AND EXISTS (
              SELECT 1 FROM book_files bf
              WHERE bf.book_id = e.book_id AND bf.available
                AND bf.format IN ('EPUB', 'AZW3', 'AZW', 'MOBI')
          )
          AND NOT EXISTS (
              SELECT 1 FROM book_files bf
              WHERE bf.book_id = e.book_id AND bf.format = 'KEPUB'
          )
        ORDER BY b.title
        "#,
    )
    .fetch_all(exec)
    .await?;

    Ok(rows.into_iter().map(|r| (r.book_id, r.title)).collect())
}

/// Whether some Kobo is entitled to this book.
///
/// Same scope rules as [`list_synced_without_kepub`], asked one book at a time
/// so a caller walking a media root can tell which of its files a device will
/// ever ask for.
pub async fn book_reaches_kobo<'e, E>(exec: E, book_id: Uuid) -> Result<bool>
where
    E: Executor<'e, Database = Postgres>,
{
    let reaches: bool = sqlx::query_scalar!(
        r#"
        SELECT EXISTS (
            SELECT 1
            FROM entitlements e
            JOIN devices d ON d.device_type = 'kobo'
            WHERE e.book_id = $1
              AND NOT e.is_removed
              AND (
                  e.scope_type = 'ALL_USERS'
                  OR (e.scope_type = 'USER' AND d.user_id = e.scope_id)
                  OR (e.scope_type = 'DEVICE' AND d.id = e.scope_id)
              )
        ) AS "exists!"
        "#,
        book_id,
    )
    .fetch_one(exec)
    .await?;

    Ok(reaches)
}
