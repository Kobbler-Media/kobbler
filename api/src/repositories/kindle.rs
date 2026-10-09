//! Kindle account, target, and delivery repository operations.

use super::Result;
use sqlx::{Executor, Postgres};
use uuid::Uuid;

// =============================================================================
// Kindle Accounts
// =============================================================================

/// Create a kindle account for a user.
pub async fn create_account<'e, E>(exec: E, user_id: Uuid, credentials_json: &str) -> Result<Uuid>
where
    E: Executor<'e, Database = Postgres>,
{
    let id = Uuid::now_v7();
    sqlx::query!(
        "INSERT INTO kindle_accounts (id, user_id, credentials_json) VALUES ($1, $2, $3)",
        id,
        user_id,
        credentials_json
    )
    .execute(exec)
    .await?;
    Ok(id)
}

/// Get kindle account for a user.
pub async fn get_account_by_user<'e, E>(exec: E, user_id: Uuid) -> Result<Option<(Uuid, String)>>
where
    E: Executor<'e, Database = Postgres>,
{
    let row = sqlx::query!(
        "SELECT id, credentials_json FROM kindle_accounts WHERE user_id = $1",
        user_id
    )
    .fetch_optional(exec)
    .await?;
    Ok(row.map(|r| (r.id, r.credentials_json)))
}

/// Update credentials for a kindle account.
pub async fn update_credentials<'e, E>(
    exec: E,
    account_id: Uuid,
    credentials_json: &str,
) -> Result<bool>
where
    E: Executor<'e, Database = Postgres>,
{
    let result = sqlx::query!(
        "UPDATE kindle_accounts SET credentials_json = $2 WHERE id = $1",
        account_id,
        credentials_json
    )
    .execute(exec)
    .await?;
    Ok(result.rows_affected() > 0)
}

/// Delete a kindle account (cascades to targets and deliveries).
pub async fn delete_account<'e, E>(exec: E, account_id: Uuid) -> Result<bool>
where
    E: Executor<'e, Database = Postgres>,
{
    let result = sqlx::query!("DELETE FROM kindle_accounts WHERE id = $1", account_id)
        .execute(exec)
        .await?;
    Ok(result.rows_affected() > 0)
}

// =============================================================================
// Kindle Targets
// =============================================================================

/// A Kindle target device.
pub struct KindleTarget {
    pub id: Uuid,
    pub kindle_account_id: Uuid,
    pub device_id: Option<Uuid>,
    pub device_serial: String,
    pub device_name: Option<String>,
    pub enabled: bool,
}

/// List all targets for a kindle account.
pub async fn list_targets<'e, E>(exec: E, account_id: Uuid) -> Result<Vec<KindleTarget>>
where
    E: Executor<'e, Database = Postgres>,
{
    let rows = sqlx::query_as!(
        KindleTarget,
        "SELECT id, kindle_account_id, device_id, device_serial, device_name, enabled FROM kindle_targets WHERE kindle_account_id = $1 ORDER BY device_name NULLS LAST",
        account_id,
    )
    .fetch_all(exec)
    .await?;

    Ok(rows)
}

/// List enabled targets for a kindle account.
pub async fn list_enabled_targets<'e, E>(exec: E, account_id: Uuid) -> Result<Vec<KindleTarget>>
where
    E: Executor<'e, Database = Postgres>,
{
    let rows = sqlx::query_as!(
        KindleTarget,
        "SELECT id, kindle_account_id, device_id, device_serial, device_name, enabled FROM kindle_targets WHERE kindle_account_id = $1 AND enabled = TRUE",
        account_id,
    )
    .fetch_all(exec)
    .await?;

    Ok(rows)
}

/// Upsert a kindle target (insert or update name if serial exists).
pub async fn upsert_target<'e, E>(
    exec: E,
    account_id: Uuid,
    device_serial: &str,
    device_name: Option<&str>,
    device_id: Option<Uuid>,
) -> Result<Uuid>
where
    E: Executor<'e, Database = Postgres>,
{
    let id = Uuid::now_v7();
    let row_id: Uuid = sqlx::query_scalar!(
        r#"
        INSERT INTO kindle_targets (id, kindle_account_id, device_serial, device_name, device_id)
        VALUES ($1, $2, $3, $4, $5)
        ON CONFLICT (kindle_account_id, device_serial)
        DO UPDATE SET
            device_name = COALESCE(EXCLUDED.device_name, kindle_targets.device_name),
            device_id = COALESCE(EXCLUDED.device_id, kindle_targets.device_id)
        RETURNING id
        "#,
        id,
        account_id,
        device_serial,
        device_name as Option<&str>,
        device_id as Option<Uuid>,
    )
    .fetch_one(exec)
    .await?;
    Ok(row_id)
}

/// Update enabled status for a target.
pub async fn update_target_enabled<'e, E>(exec: E, target_id: Uuid, enabled: bool) -> Result<bool>
where
    E: Executor<'e, Database = Postgres>,
{
    let result = sqlx::query!(
        "UPDATE kindle_targets SET enabled = $2 WHERE id = $1",
        target_id,
        enabled
    )
    .execute(exec)
    .await?;
    Ok(result.rows_affected() > 0)
}

// =============================================================================
// Kindle Deliveries
// =============================================================================

/// Record a delivery.
pub async fn create_delivery<'e, E>(
    exec: E,
    target_id: Uuid,
    book_id: Uuid,
    book_file_id: Uuid,
) -> Result<Uuid>
where
    E: Executor<'e, Database = Postgres>,
{
    let id = Uuid::now_v7();
    sqlx::query!(
        r#"
        INSERT INTO kindle_deliveries (id, kindle_target_id, book_id, book_file_id)
        VALUES ($1, $2, $3, $4)
        ON CONFLICT (kindle_target_id, book_id) DO UPDATE SET
            book_file_id = EXCLUDED.book_file_id,
            delivered_at = NOW()
        "#,
        id,
        target_id,
        book_id,
        book_file_id,
    )
    .execute(exec)
    .await?;
    Ok(id)
}

/// Check if a book has been delivered to a target.
pub async fn is_delivered<'e, E>(exec: E, target_id: Uuid, book_id: Uuid) -> Result<bool>
where
    E: Executor<'e, Database = Postgres>,
{
    let exists: bool = sqlx::query_scalar!(
        r#"SELECT EXISTS(SELECT 1 FROM kindle_deliveries WHERE kindle_target_id = $1 AND book_id = $2) AS "exists!""#,
        target_id,
        book_id,
    )
    .fetch_one(exec)
    .await?;
    Ok(exists)
}

/// List deliveries for a book.
pub struct KindleDeliveryInfo {
    pub id: Uuid,
    pub target_id: Uuid,
    pub device_name: Option<String>,
    pub delivered_at: time::OffsetDateTime,
}

pub async fn list_deliveries_for_book<'e, E>(
    exec: E,
    book_id: Uuid,
) -> Result<Vec<KindleDeliveryInfo>>
where
    E: Executor<'e, Database = Postgres>,
{
    let rows = sqlx::query!(
        r#"
        SELECT kd.id, kd.kindle_target_id, kt.device_name, kd.delivered_at
        FROM kindle_deliveries kd
        JOIN kindle_targets kt ON kt.id = kd.kindle_target_id
        WHERE kd.book_id = $1
        ORDER BY kd.delivered_at DESC
        "#,
        book_id,
    )
    .fetch_all(exec)
    .await?;

    Ok(rows
        .into_iter()
        .map(|r| KindleDeliveryInfo {
            id: r.id,
            target_id: r.kindle_target_id,
            device_name: r.device_name,
            delivered_at: r.delivered_at,
        })
        .collect())
}

/// Get credentials for a kindle account by account ID.
pub async fn get_credentials<'e, E>(exec: E, account_id: Uuid) -> Result<Option<String>>
where
    E: Executor<'e, Database = Postgres>,
{
    let row = sqlx::query_scalar!(
        "SELECT credentials_json FROM kindle_accounts WHERE id = $1",
        account_id
    )
    .fetch_optional(exec)
    .await?;
    Ok(row)
}

/// Get a single target by its ID.
pub async fn get_target<'e, E>(exec: E, target_id: Uuid) -> Result<Option<KindleTarget>>
where
    E: Executor<'e, Database = Postgres>,
{
    let row = sqlx::query_as!(
        KindleTarget,
        "SELECT id, kindle_account_id, device_id, device_serial, device_name, enabled FROM kindle_targets WHERE id = $1",
        target_id,
    )
    .fetch_optional(exec)
    .await?;
    Ok(row)
}

/// List enabled targets linked to a specific device.
pub async fn list_enabled_targets_for_device<'e, E>(
    exec: E,
    device_id: Uuid,
) -> Result<Vec<KindleTarget>>
where
    E: Executor<'e, Database = Postgres>,
{
    let rows = sqlx::query_as!(
        KindleTarget,
        "SELECT id, kindle_account_id, device_id, device_serial, device_name, enabled FROM kindle_targets WHERE device_id = $1 AND enabled = TRUE",
        device_id,
    )
    .fetch_all(exec)
    .await?;

    Ok(rows)
}

/// List all enabled kindle targets across all accounts (for ALL_USERS entitlements).
pub async fn list_all_enabled_targets<'e, E>(exec: E) -> Result<Vec<KindleTarget>>
where
    E: Executor<'e, Database = Postgres>,
{
    let rows = sqlx::query_as!(
        KindleTarget,
        "SELECT id, kindle_account_id, device_id, device_serial, device_name, enabled FROM kindle_targets WHERE enabled = TRUE AND device_id IS NOT NULL",
    )
    .fetch_all(exec)
    .await?;

    Ok(rows)
}

/// Check if any kindle accounts exist (for conditional UI).
pub async fn has_any_account<'e, E>(exec: E) -> Result<bool>
where
    E: Executor<'e, Database = Postgres>,
{
    let exists: bool =
        sqlx::query_scalar!(r#"SELECT EXISTS(SELECT 1 FROM kindle_accounts) AS "exists!""#)
            .fetch_one(exec)
            .await?;
    Ok(exists)
}

// =============================================================================
// OAuth PKCE Verifier
// =============================================================================

/// Create or update a pending verifier for a user's OAuth flow.
pub async fn set_pending_verifier<'e, E>(exec: E, user_id: Uuid, verifier: &str) -> Result<Uuid>
where
    E: Executor<'e, Database = Postgres>,
{
    let id = Uuid::now_v7();
    let row_id: Uuid = sqlx::query_scalar!(
        r#"
        INSERT INTO kindle_accounts (id, user_id, credentials_json, pending_verifier)
        VALUES ($1, $2, '', $3)
        ON CONFLICT (user_id)
        DO UPDATE SET pending_verifier = EXCLUDED.pending_verifier
        RETURNING id
        "#,
        id,
        user_id,
        verifier,
    )
    .fetch_one(exec)
    .await?;
    Ok(row_id)
}

/// Get the pending verifier for a user's OAuth flow.
pub async fn get_pending_verifier<'e, E>(exec: E, user_id: Uuid) -> Result<Option<String>>
where
    E: Executor<'e, Database = Postgres>,
{
    let row = sqlx::query_scalar!(
        "SELECT pending_verifier FROM kindle_accounts WHERE user_id = $1",
        user_id
    )
    .fetch_optional(exec)
    .await?;
    Ok(row.flatten())
}

/// Clear the pending verifier after OAuth completion.
pub async fn clear_pending_verifier<'e, E>(exec: E, user_id: Uuid) -> Result<()>
where
    E: Executor<'e, Database = Postgres>,
{
    sqlx::query!(
        "UPDATE kindle_accounts SET pending_verifier = NULL WHERE user_id = $1",
        user_id
    )
    .execute(exec)
    .await?;
    Ok(())
}

// =============================================================================
// Device Activity Queries
// =============================================================================

/// Mini book info for activity displays.
pub struct MiniBookInfo {
    pub id: Uuid,
    pub title: String,
    pub cover_url: Option<String>,
}

/// Mini book with delivery timestamp.
pub struct MiniBookWithDeliveryTime {
    pub id: Uuid,
    pub title: String,
    pub cover_url: Option<String>,
    pub delivered_at: time::OffsetDateTime,
}

/// Get books pending delivery for a Kindle device.
/// These are entitled books that haven't been delivered to any enabled target for this device.
pub async fn get_pending_delivery_books<'e, E>(
    exec: E,
    user_id: Uuid,
    device_id: Uuid,
    limit: i64,
) -> Result<Vec<MiniBookInfo>>
where
    E: Executor<'e, Database = Postgres>,
{
    let rows = sqlx::query_as!(
        MiniBookInfo,
        r#"
        SELECT DISTINCT b.id, b.title, b.images->'cover'->>'source_url' as cover_url
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
        -- Not yet delivered to any enabled target for this device
        AND NOT EXISTS (
            SELECT 1 FROM kindle_deliveries kd
            JOIN kindle_targets kt ON kt.id = kd.kindle_target_id
            WHERE kd.book_id = b.id
              AND kt.device_id = $2
              AND kt.enabled = TRUE
        )
        ORDER BY b.title
        LIMIT $3
        "#,
        user_id,
        device_id,
        limit,
    )
    .fetch_all(exec)
    .await?;

    Ok(rows)
}

/// Get recently delivered books for a Kindle device.
/// Returns books and the most recent delivery timestamp.
pub async fn get_recently_delivered_books<'e, E>(
    exec: E,
    device_id: Uuid,
    limit: i64,
) -> Result<(Vec<MiniBookWithDeliveryTime>, Option<time::OffsetDateTime>)>
where
    E: Executor<'e, Database = Postgres>,
{
    let rows = sqlx::query_as!(
        MiniBookWithDeliveryTime,
        r#"
        SELECT DISTINCT ON (b.id) b.id, b.title, b.images->'cover'->>'source_url' as cover_url, kd.delivered_at
        FROM kindle_deliveries kd
        JOIN kindle_targets kt ON kt.id = kd.kindle_target_id
        JOIN books b ON b.id = kd.book_id
        WHERE kt.device_id = $1
        ORDER BY b.id, kd.delivered_at DESC
        "#,
        device_id,
    )
    .fetch_all(exec)
    .await?;

    // Sort by delivery time and take limit
    let mut results: Vec<_> = rows;

    results.sort_by_key(|b| std::cmp::Reverse(b.delivered_at));
    let last_delivery = results.first().map(|b| b.delivered_at);
    let books: Vec<_> = results.into_iter().take(limit as usize).collect();

    Ok((books, last_delivery))
}

/// The user who owns a kindle account, for attributing deliveries.
pub async fn account_owner<'e, E>(exec: E, account_id: Uuid) -> Result<Option<Uuid>>
where
    E: Executor<'e, Database = Postgres>,
{
    let row = sqlx::query_scalar!(
        "SELECT user_id FROM kindle_accounts WHERE id = $1",
        account_id
    )
    .fetch_optional(exec)
    .await?;
    Ok(row)
}
