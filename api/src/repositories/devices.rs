//! Devices repository operations.

use super::Result;
use crate::types::DeviceType;
use sqlx::{Executor, Postgres};
use uuid::Uuid;

/// Input for creating a device.
#[derive(Debug)]
pub struct DeviceInput {
    pub user_id: Uuid,
    pub token: String,
    pub name: String,
    pub device_type: DeviceType,
}

/// Create a new device.
pub async fn create<'e, E>(exec: E, input: DeviceInput) -> Result<Uuid>
where
    E: Executor<'e, Database = Postgres>,
{
    let id = Uuid::now_v7();
    sqlx::query!(
        "INSERT INTO devices (id, user_id, token, name, device_type) VALUES ($1, $2, $3, $4, $5)",
        id,
        input.user_id,
        input.token,
        input.name,
        input.device_type as DeviceType,
    )
    .execute(exec)
    .await?;
    Ok(id)
}

/// Update device token.
pub async fn update_token<'e, E>(exec: E, id: Uuid, token: &str) -> Result<bool>
where
    E: Executor<'e, Database = Postgres>,
{
    let result = sqlx::query!("UPDATE devices SET token = $2 WHERE id = $1", id, token,)
        .execute(exec)
        .await?;
    Ok(result.rows_affected() > 0)
}

/// Update device name, kobo_api_url, and/or auto_sync_new_books.
pub async fn update<'e, E>(
    exec: E,
    id: Uuid,
    name: Option<&str>,
    kobo_api_url: Option<&str>,
    auto_sync_new_books: Option<bool>,
) -> Result<bool>
where
    E: Executor<'e, Database = Postgres>,
{
    let result = sqlx::query!(
        r#"UPDATE devices SET
            name = COALESCE($2, name),
            kobo_api_url = COALESCE($3, kobo_api_url),
            auto_sync_new_books = COALESCE($4, auto_sync_new_books)
        WHERE id = $1"#,
        id,
        name,
        kobo_api_url,
        auto_sync_new_books,
    )
    .execute(exec)
    .await?;
    Ok(result.rows_affected() > 0)
}

/// Delete a device.
pub async fn delete<'e, E>(exec: E, id: Uuid) -> Result<bool>
where
    E: Executor<'e, Database = Postgres>,
{
    let result = sqlx::query!("DELETE FROM devices WHERE id = $1", id)
        .execute(exec)
        .await?;
    Ok(result.rows_affected() > 0)
}

/// Get device ID by token.
pub async fn get_id_by_token<'e, E>(exec: E, token: &str) -> Result<Option<Uuid>>
where
    E: Executor<'e, Database = Postgres>,
{
    let id: Option<Uuid> = sqlx::query_scalar!("SELECT id FROM devices WHERE token = $1", token,)
        .fetch_optional(exec)
        .await?;
    Ok(id)
}

/// Get device ID and kobo_api_url by token.
pub async fn get_by_token<'e, E>(exec: E, token: &str) -> Result<Option<(Uuid, Option<String>)>>
where
    E: Executor<'e, Database = Postgres>,
{
    let row = sqlx::query!(
        "SELECT id, kobo_api_url FROM devices WHERE token = $1",
        token,
    )
    .fetch_optional(exec)
    .await?;
    Ok(row.map(|r| (r.id, r.kobo_api_url)))
}

/// Get the user ID for a device.
pub async fn get_user_id<'e, E>(exec: E, id: Uuid) -> Result<Option<Uuid>>
where
    E: Executor<'e, Database = Postgres>,
{
    let user_id: Option<Uuid> =
        sqlx::query_scalar!("SELECT user_id FROM devices WHERE id = $1", id,)
            .fetch_optional(exec)
            .await?;
    Ok(user_id)
}

/// Get all device IDs for a user.
pub async fn list_ids_for_user<'e, E>(exec: E, user_id: Uuid) -> Result<Vec<Uuid>>
where
    E: Executor<'e, Database = Postgres>,
{
    let ids = sqlx::query_scalar!("SELECT id FROM devices WHERE user_id = $1", user_id)
        .fetch_all(exec)
        .await?;
    Ok(ids)
}

/// Check if a device exists.
pub async fn exists<'e, E>(exec: E, id: Uuid) -> Result<bool>
where
    E: Executor<'e, Database = Postgres>,
{
    let exists: bool = sqlx::query_scalar!(
        r#"SELECT EXISTS(SELECT 1 FROM devices WHERE id = $1) as "exists!""#,
        id,
    )
    .fetch_one(exec)
    .await?;
    Ok(exists)
}

/// Device context for Kobo operations.
#[derive(Debug, Clone)]
pub struct DeviceContext {
    pub user_id: Uuid,
    pub device_id: Uuid,
    pub device_name: Option<String>,
}

/// Look up device context by token (for Kobo sync).
pub async fn get_context_by_token<'e, E>(exec: E, token: &str) -> Result<Option<DeviceContext>>
where
    E: Executor<'e, Database = Postgres>,
{
    let row = sqlx::query!(
        "SELECT id, user_id, name FROM devices WHERE token = $1",
        token,
    )
    .fetch_optional(exec)
    .await?;

    Ok(row.map(|r| DeviceContext {
        device_id: r.id,
        user_id: r.user_id,
        device_name: r.name,
    }))
}

/// Device summary for listing.
pub struct DeviceSummary {
    pub id: Uuid,
    pub name: Option<String>,
    pub user_id: Uuid,
}

/// List devices (basic summary). `user_id = None` lists every user's devices —
/// only admins may pass None.
pub async fn list_summary<'e, E>(exec: E, user_id: Option<Uuid>) -> Result<Vec<DeviceSummary>>
where
    E: Executor<'e, Database = Postgres>,
{
    let rows = sqlx::query!(
        r#"SELECT id, name, user_id FROM devices
           WHERE device_type != 'web'
             AND ($1::uuid IS NULL OR user_id = $1)
           ORDER BY created_at DESC"#,
        user_id,
    )
    .fetch_all(exec)
    .await?;

    Ok(rows
        .into_iter()
        .map(|r| DeviceSummary {
            id: r.id,
            name: r.name,
            user_id: r.user_id,
        })
        .collect())
}

/// Full device row with sync state.
#[derive(Debug)]
pub struct DeviceWithSyncState {
    pub id: Uuid,
    pub name: Option<String>,
    pub user_id: Uuid,
    pub token: String,
    pub kobo_api_url: Option<String>,
    pub last_sync_at: Option<time::OffsetDateTime>,
    pub device_type: DeviceType,
    pub auto_sync_new_books: bool,
}

/// List all devices with sync state.
pub async fn list_with_sync_state<'e, E>(exec: E) -> Result<Vec<DeviceWithSyncState>>
where
    E: Executor<'e, Database = Postgres>,
{
    let rows = sqlx::query!(
        r#"
        SELECT d.id, d.name, d.user_id, d.token, d.kobo_api_url,
               d.device_type as "device_type: DeviceType", s.last_sync_at,
               d.auto_sync_new_books
        FROM devices d
        LEFT JOIN device_sync_state s ON s.device_id = d.id
        WHERE d.device_type != 'web'
        ORDER BY d.created_at DESC
        "#,
    )
    .fetch_all(exec)
    .await?;

    Ok(rows
        .into_iter()
        .map(|r| DeviceWithSyncState {
            id: r.id,
            name: r.name,
            user_id: r.user_id,
            token: r.token,
            kobo_api_url: r.kobo_api_url,
            last_sync_at: r.last_sync_at,
            device_type: r.device_type,
            auto_sync_new_books: r.auto_sync_new_books,
        })
        .collect())
}

/// Get the web reader device for a user, if one exists.
pub async fn get_web_reader_for_user<'e, E>(exec: E, user_id: Uuid) -> Result<Option<Uuid>>
where
    E: Executor<'e, Database = Postgres>,
{
    let id: Option<Uuid> = sqlx::query_scalar!(
        "SELECT id FROM devices WHERE user_id = $1 AND device_type = 'web'",
        user_id,
    )
    .fetch_optional(exec)
    .await?;
    Ok(id)
}

/// Get or create the web reader device for a user.
/// Uses the partial unique index to ensure at most one per user.
pub async fn get_or_create_web_reader(pool: &sqlx::PgPool, user_id: Uuid) -> Result<Uuid> {
    // Try to find existing first (fast path)
    if let Some(id) = get_web_reader_for_user(pool, user_id).await? {
        return Ok(id);
    }

    let id = Uuid::now_v7();
    let token = format!("web-reader-{}", Uuid::now_v7());

    // INSERT with ON CONFLICT to handle race conditions
    let device_id: Uuid = sqlx::query_scalar!(
        r#"
        INSERT INTO devices (id, user_id, token, name, device_type)
        VALUES ($1, $2, $3, 'Web Reader', 'web')
        ON CONFLICT (user_id) WHERE device_type = 'web'
        DO UPDATE SET id = devices.id
        RETURNING id
        "#,
        id,
        user_id,
        token,
    )
    .fetch_one(pool)
    .await?;

    Ok(device_id)
}

/// Check if any devices of the given type exist.
pub async fn has_devices_of_type<'e, E>(exec: E, device_type: DeviceType) -> Result<bool>
where
    E: Executor<'e, Database = Postgres>,
{
    let exists: bool = sqlx::query_scalar!(
        r#"SELECT EXISTS(SELECT 1 FROM devices WHERE device_type = $1) as "exists!""#,
        device_type as DeviceType,
    )
    .fetch_one(exec)
    .await?;
    Ok(exists)
}

/// Get distinct device types that have at least one device.
pub async fn active_device_types<'e, E>(exec: E) -> Result<Vec<DeviceType>>
where
    E: Executor<'e, Database = Postgres>,
{
    let rows: Vec<DeviceType> = sqlx::query_scalar!(
        r#"SELECT DISTINCT device_type as "device_type: DeviceType" FROM devices"#,
    )
    .fetch_all(exec)
    .await?;
    Ok(rows)
}

/// Get a single device with sync state by ID.
pub async fn get_with_sync_state<'e, E>(exec: E, id: Uuid) -> Result<Option<DeviceWithSyncState>>
where
    E: Executor<'e, Database = Postgres>,
{
    let row = sqlx::query!(
        r#"
        SELECT d.id, d.name, d.user_id, d.token, d.kobo_api_url,
               d.device_type as "device_type: DeviceType", s.last_sync_at,
               d.auto_sync_new_books
        FROM devices d
        LEFT JOIN device_sync_state s ON s.device_id = d.id
        WHERE d.id = $1
        "#,
        id,
    )
    .fetch_optional(exec)
    .await?;

    Ok(row.map(|r| DeviceWithSyncState {
        id: r.id,
        name: r.name,
        user_id: r.user_id,
        token: r.token,
        kobo_api_url: r.kobo_api_url,
        last_sync_at: r.last_sync_at,
        device_type: r.device_type,
        auto_sync_new_books: r.auto_sync_new_books,
    }))
}

// ========== Device Activity Queries ==========

/// Mini book info for activity displays.
pub struct MiniBookInfo {
    pub id: Uuid,
    pub title: String,
    pub cover_url: Option<String>,
}

/// Mini book with reading progress.
pub struct MiniBookWithProgress {
    pub id: Uuid,
    pub title: String,
    pub cover_url: Option<String>,
    pub progress_percent: i32,
}

/// Mini book with sync timestamp.
pub struct MiniBookWithSyncTime {
    pub id: Uuid,
    pub title: String,
    pub cover_url: Option<String>,
    pub synced_at: time::OffsetDateTime,
}

/// Get books pending sync for a device (entitled but updated since last sync).
pub async fn get_pending_sync_books<'e, E>(
    exec: E,
    user_id: Uuid,
    device_id: Uuid,
    since: time::OffsetDateTime,
    limit: i64,
) -> Result<Vec<MiniBookInfo>>
where
    E: Executor<'e, Database = Postgres>,
{
    let rows = sqlx::query!(
        r#"
        SELECT DISTINCT b.id, b.title, b.images->'cover'->>'source_url' AS cover_url
        FROM entitlements e
        JOIN books b ON b.id = e.book_id
        WHERE (
            e.scope_type = 'ALL_USERS'
            OR (e.scope_type = 'USER' AND e.scope_id = $1)
            OR (e.scope_type = 'DEVICE' AND e.scope_id = $2)
        )
        AND e.is_removed = FALSE
        AND b.ingest_status IS NULL
        AND (e.updated_at > $3 OR e.created_at > $3 OR b.updated_at > $3)
        AND EXISTS (
            SELECT 1 FROM book_files bf WHERE bf.book_id = b.id AND bf.available = TRUE
        )
        ORDER BY b.title
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
        .map(|r| MiniBookInfo {
            id: r.id,
            title: r.title,
            cover_url: r.cover_url,
        })
        .collect())
}

/// Get recently read books for a user (across all devices).
pub async fn get_recently_read_books<'e, E>(
    exec: E,
    user_id: Uuid,
    limit: i64,
) -> Result<Vec<MiniBookWithProgress>>
where
    E: Executor<'e, Database = Postgres>,
{
    let rows = sqlx::query!(
        r#"
        SELECT DISTINCT ON (b.id) b.id, b.title, b.images->'cover'->>'source_url' AS cover_url,
               COALESCE(bm.progress_percent, 0)::int AS "progress_percent!",
               rs.updated_at
        FROM reading_states rs
        JOIN reading_bookmark bm ON bm.reading_state_id = rs.id
        JOIN books b ON b.id = rs.book_id
        WHERE rs.user_id = $1
          AND COALESCE(bm.progress_percent, 0) > 0
        ORDER BY b.id, rs.updated_at DESC
        "#,
        user_id,
    )
    .fetch_all(exec)
    .await?;

    // Sort by updated_at and take limit (need to do this after DISTINCT ON)
    let mut results: Vec<_> = rows
        .into_iter()
        .map(|r| {
            (
                MiniBookWithProgress {
                    id: r.id,
                    title: r.title,
                    cover_url: r.cover_url,
                    progress_percent: r.progress_percent,
                },
                r.updated_at,
            )
        })
        .collect();

    results.sort_by_key(|b| std::cmp::Reverse(b.1));
    Ok(results
        .into_iter()
        .take(limit as usize)
        .map(|(b, _)| b)
        .collect())
}

/// Get recently synced books for a device (based on entitlement update time).
pub async fn get_recently_synced_books<'e, E>(
    exec: E,
    user_id: Uuid,
    device_id: Uuid,
    limit: i64,
) -> Result<Vec<MiniBookWithSyncTime>>
where
    E: Executor<'e, Database = Postgres>,
{
    let rows = sqlx::query!(
        r#"
        SELECT b.id, b.title, b.images->'cover'->>'source_url' AS cover_url,
               GREATEST(e.updated_at, b.updated_at) AS "synced_at!"
        FROM entitlements e
        JOIN books b ON b.id = e.book_id
        WHERE (
            e.scope_type = 'ALL_USERS'
            OR (e.scope_type = 'USER' AND e.scope_id = $1)
            OR (e.scope_type = 'DEVICE' AND e.scope_id = $2)
        )
        AND e.is_removed = FALSE
        AND b.ingest_status IS NULL
        AND EXISTS (
            SELECT 1 FROM book_files bf WHERE bf.book_id = b.id AND bf.available = TRUE
        )
        ORDER BY GREATEST(e.updated_at, b.updated_at) DESC
        LIMIT $3
        "#,
        user_id,
        device_id,
        limit,
    )
    .fetch_all(exec)
    .await?;

    Ok(rows
        .into_iter()
        .map(|r| MiniBookWithSyncTime {
            id: r.id,
            title: r.title,
            cover_url: r.cover_url,
            synced_at: r.synced_at,
        })
        .collect())
}

/// List device IDs that have auto-sync enabled.
pub async fn list_auto_sync_device_ids<'e, E>(exec: E) -> Result<Vec<Uuid>>
where
    E: Executor<'e, Database = Postgres>,
{
    let ids = sqlx::query_scalar!(
        "SELECT id FROM devices WHERE auto_sync_new_books = TRUE AND device_type != 'web'"
    )
    .fetch_all(exec)
    .await?;
    Ok(ids)
}
