//! Reading states repository operations.

use super::Result;
use serde_json::Value;
use sqlx::{Executor, PgPool, Postgres};
use time::OffsetDateTime;
use uuid::Uuid;

/// Delete all reading states for a book.
pub async fn delete_by_book<'e, E>(exec: E, book_id: Uuid) -> Result<u64>
where
    E: Executor<'e, Database = Postgres>,
{
    let result = sqlx::query!("DELETE FROM reading_states WHERE book_id = $1", book_id)
        .execute(exec)
        .await?;
    Ok(result.rows_affected())
}

/// Delete all reading states for a device.
pub async fn delete_by_device<'e, E>(exec: E, device_id: Uuid) -> Result<u64>
where
    E: Executor<'e, Database = Postgres>,
{
    let result = sqlx::query!("DELETE FROM reading_states WHERE device_id = $1", device_id)
        .execute(exec)
        .await?;
    Ok(result.rows_affected())
}

/// Get reading state ID for a specific user, device, and book.
pub async fn get_id<'e, E>(
    exec: E,
    user_id: Uuid,
    device_id: Uuid,
    book_id: Uuid,
) -> Result<Option<Uuid>>
where
    E: Executor<'e, Database = Postgres>,
{
    let id: Option<Uuid> = sqlx::query_scalar!(
        "SELECT id FROM reading_states WHERE user_id = $1 AND device_id = $2 AND book_id = $3",
        user_id,
        device_id,
        book_id,
    )
    .fetch_optional(exec)
    .await?;
    Ok(id)
}

/// Full reading state row for Kobo sync.
#[derive(Debug, Clone, serde::Deserialize)]
pub struct ReadingStateRow {
    #[serde(deserialize_with = "crate::serde_pg::uuid")]
    pub id: Uuid,
    #[serde(deserialize_with = "crate::serde_pg::uuid")]
    pub book_id: Uuid,
    #[serde(deserialize_with = "crate::serde_pg::uuid")]
    pub device_id: Uuid,
    #[serde(deserialize_with = "crate::serde_pg::timestamp")]
    pub created_at: OffsetDateTime,
    #[serde(deserialize_with = "crate::serde_pg::timestamp")]
    pub updated_at: OffsetDateTime,
    #[serde(deserialize_with = "crate::serde_pg::timestamp")]
    pub priority_timestamp: OffsetDateTime,
    pub status: String,
    #[serde(deserialize_with = "crate::serde_pg::timestamp")]
    pub status_last_modified: OffsetDateTime,
    pub times_started: i32,
    #[serde(default, deserialize_with = "crate::serde_pg::opt_timestamp")]
    pub last_time_started: Option<OffsetDateTime>,
    pub spent_reading_minutes: Option<i32>,
    pub remaining_time_minutes: Option<i32>,
    #[serde(deserialize_with = "crate::serde_pg::timestamp")]
    pub stats_last_modified: OffsetDateTime,
    pub progress_percent: Option<f64>,
    pub content_source_progress_percent: Option<f64>,
    pub location_value: Option<String>,
    pub location_type: Option<String>,
    pub location_source: Option<String>,
    #[serde(deserialize_with = "crate::serde_pg::timestamp")]
    pub bookmark_last_modified: OffsetDateTime,
    #[serde(default)]
    pub client_payload: Option<Value>,
}

/// Get full reading state by ID with all child tables joined.
pub async fn get_full_by_id<'e, E>(
    exec: E,
    state_id: Uuid,
    book_id: Uuid,
) -> Result<Option<ReadingStateRow>>
where
    E: Executor<'e, Database = Postgres>,
{
    let row = sqlx::query!(
        r#"
        SELECT rs.id, rs.device_id, rs.created_at, rs.updated_at, rs.priority_timestamp, rs.client_payload,
               st.status, st.last_modified_at as status_last_modified, st.times_started_reading, st.last_time_started_reading,
               stats.spent_reading_minutes, stats.remaining_time_minutes, stats.last_modified_at as stats_last_modified,
               bm.progress_percent, bm.content_source_progress_percent, bm.location_value, bm.location_type, bm.location_source,
               bm.last_modified_at as bookmark_last_modified
        FROM reading_states rs
        JOIN reading_status st ON st.reading_state_id = rs.id
        JOIN reading_statistics stats ON stats.reading_state_id = rs.id
        JOIN reading_bookmark bm ON bm.reading_state_id = rs.id
        WHERE rs.id = $1
        "#,
        state_id,
    )
    .fetch_optional(exec)
    .await?;

    Ok(row.map(|r| ReadingStateRow {
        id: r.id,
        book_id,
        device_id: r.device_id,
        created_at: r.created_at,
        updated_at: r.updated_at,
        priority_timestamp: r.priority_timestamp,
        status: r.status,
        status_last_modified: r.status_last_modified,
        times_started: r.times_started_reading,
        last_time_started: r.last_time_started_reading,
        spent_reading_minutes: r.spent_reading_minutes,
        remaining_time_minutes: r.remaining_time_minutes,
        stats_last_modified: r.stats_last_modified,
        progress_percent: r.progress_percent,
        content_source_progress_percent: r.content_source_progress_percent,
        location_value: r.location_value,
        location_type: r.location_type,
        location_source: r.location_source,
        bookmark_last_modified: r.bookmark_last_modified,
        client_payload: r.client_payload,
    }))
}

/// Get the most recent reading state for a user+book across ALL devices.
/// This is used for cross-device sync - when Device B syncs, it gets the most
/// recent state from ANY of the user's devices, not just its own.
pub async fn get_most_recent_for_user_book<'e, E>(
    exec: E,
    user_id: Uuid,
    book_id: Uuid,
) -> Result<Option<ReadingStateRow>>
where
    E: Executor<'e, Database = Postgres>,
{
    let row = sqlx::query!(
        r#"
        SELECT rs.id, rs.device_id, rs.created_at, rs.updated_at, rs.priority_timestamp, rs.client_payload,
               st.status, st.last_modified_at as status_last_modified, st.times_started_reading, st.last_time_started_reading,
               stats.spent_reading_minutes, stats.remaining_time_minutes, stats.last_modified_at as stats_last_modified,
               bm.progress_percent, bm.content_source_progress_percent, bm.location_value, bm.location_type, bm.location_source,
               bm.last_modified_at as bookmark_last_modified
        FROM reading_states rs
        JOIN reading_status st ON st.reading_state_id = rs.id
        JOIN reading_statistics stats ON stats.reading_state_id = rs.id
        JOIN reading_bookmark bm ON bm.reading_state_id = rs.id
        WHERE rs.user_id = $1 AND rs.book_id = $2
        ORDER BY
            CASE
                WHEN st.status = 'Reading' THEN 0
                WHEN COALESCE(bm.progress_percent, 0) > 0 THEN 1
                WHEN st.status = 'Finished' THEN 2
                ELSE 3
            END,
            rs.updated_at DESC
        LIMIT 1
        "#,
        user_id,
        book_id,
    )
    .fetch_optional(exec)
    .await?;

    Ok(row.map(|r| ReadingStateRow {
        id: r.id,
        book_id,
        device_id: r.device_id,
        created_at: r.created_at,
        updated_at: r.updated_at,
        priority_timestamp: r.priority_timestamp,
        status: r.status,
        status_last_modified: r.status_last_modified,
        times_started: r.times_started_reading,
        last_time_started: r.last_time_started_reading,
        spent_reading_minutes: r.spent_reading_minutes,
        remaining_time_minutes: r.remaining_time_minutes,
        stats_last_modified: r.stats_last_modified,
        progress_percent: r.progress_percent,
        content_source_progress_percent: r.content_source_progress_percent,
        location_value: r.location_value,
        location_type: r.location_type,
        location_source: r.location_source,
        bookmark_last_modified: r.bookmark_last_modified,
        client_payload: r.client_payload,
    }))
}

/// List all reading_states rows for (user, book) across every device, each with
/// its child tables joined. Used by the Kobo GET resolver to pick which bytes
/// to serve the requesting device.
pub async fn list_for_user_book<'e, E>(
    exec: E,
    user_id: Uuid,
    book_id: Uuid,
) -> Result<Vec<ReadingStateRow>>
where
    E: Executor<'e, Database = Postgres>,
{
    let rows = sqlx::query!(
        r#"
        SELECT rs.id, rs.device_id, rs.created_at, rs.updated_at, rs.priority_timestamp, rs.client_payload,
               st.status, st.last_modified_at as status_last_modified, st.times_started_reading, st.last_time_started_reading,
               stats.spent_reading_minutes, stats.remaining_time_minutes, stats.last_modified_at as stats_last_modified,
               bm.progress_percent, bm.content_source_progress_percent, bm.location_value, bm.location_type, bm.location_source,
               bm.last_modified_at as bookmark_last_modified
        FROM reading_states rs
        JOIN reading_status st ON st.reading_state_id = rs.id
        JOIN reading_statistics stats ON stats.reading_state_id = rs.id
        JOIN reading_bookmark bm ON bm.reading_state_id = rs.id
        WHERE rs.user_id = $1 AND rs.book_id = $2
        ORDER BY
            CASE
                WHEN st.status = 'Reading' THEN 0
                WHEN COALESCE(bm.progress_percent, 0) > 0 THEN 1
                WHEN st.status = 'Finished' THEN 2
                ELSE 3
            END,
            rs.updated_at DESC
        "#,
        user_id,
        book_id,
    )
    .fetch_all(exec)
    .await?;

    Ok(rows
        .into_iter()
        .map(|r| ReadingStateRow {
            id: r.id,
            book_id,
            device_id: r.device_id,
            created_at: r.created_at,
            updated_at: r.updated_at,
            priority_timestamp: r.priority_timestamp,
            status: r.status,
            status_last_modified: r.status_last_modified,
            times_started: r.times_started_reading,
            last_time_started: r.last_time_started_reading,
            spent_reading_minutes: r.spent_reading_minutes,
            remaining_time_minutes: r.remaining_time_minutes,
            stats_last_modified: r.stats_last_modified,
            progress_percent: r.progress_percent,
            content_source_progress_percent: r.content_source_progress_percent,
            location_value: r.location_value,
            location_type: r.location_type,
            location_source: r.location_source,
            bookmark_last_modified: r.bookmark_last_modified,
            client_payload: r.client_payload,
        })
        .collect())
}

/// Persist the raw Kobo-shape body for a reading_states row so future GETs can echo it verbatim.
pub async fn set_client_payload<'e, E>(exec: E, state_id: Uuid, payload: &Value) -> Result<()>
where
    E: Executor<'e, Database = Postgres>,
{
    sqlx::query!(
        "UPDATE reading_states SET client_payload = $1 WHERE id = $2",
        payload,
        state_id,
    )
    .execute(exec)
    .await?;
    Ok(())
}

/// Upsert a reading state atomically. Returns the state ID.
/// If a reading state already exists, returns that ID without modification.
/// If new, creates the state and all child records (status, statistics, bookmark).
pub async fn upsert_with_children(
    pool: &PgPool,
    user_id: Uuid,
    device_id: Uuid,
    book_id: Uuid,
) -> Result<Option<Uuid>> {
    let id = Uuid::new_v4();

    let mut tx = pool.begin().await?;

    // Upsert reading state atomically - ON CONFLICT returns existing id
    let state_id: Uuid = sqlx::query_scalar!(
        r#"
        INSERT INTO reading_states (id, book_id, user_id, device_id)
        VALUES ($1, $2, $3, $4)
        ON CONFLICT (book_id, user_id, device_id) DO UPDATE SET id = reading_states.id
        RETURNING id
        "#,
        id,
        book_id,
        user_id,
        device_id,
    )
    .fetch_one(&mut *tx)
    .await?;

    // If we got back a different id, we hit the existing record - nothing more to do
    let is_new = state_id == id;

    if is_new {
        // Create child records for new reading state
        sqlx::query!(
            "INSERT INTO reading_status (reading_state_id, status, times_started_reading) VALUES ($1, $2, $3)",
            state_id,
            "ReadyToRead",
            0i32,
        )
        .execute(&mut *tx)
        .await?;

        sqlx::query!(
            "INSERT INTO reading_statistics (reading_state_id) VALUES ($1)",
            state_id
        )
        .execute(&mut *tx)
        .await?;

        sqlx::query!(
            "INSERT INTO reading_bookmark (reading_state_id) VALUES ($1)",
            state_id
        )
        .execute(&mut *tx)
        .await?;
    }

    tx.commit().await?;

    Ok(Some(state_id))
}

/// Update reading status for a state.
pub async fn update_status<'e, E>(exec: E, state_id: Uuid, status: &str) -> Result<()>
where
    E: Executor<'e, Database = Postgres>,
{
    sqlx::query!(
        "UPDATE reading_status SET status = $1, last_modified_at = NOW() WHERE reading_state_id = $2",
        status,
        state_id,
    )
    .execute(exec)
    .await?;
    Ok(())
}

/// Update reading statistics for a state.
pub async fn update_statistics<'e, E>(
    exec: E,
    state_id: Uuid,
    spent_reading_minutes: Option<i32>,
    remaining_time_minutes: Option<i32>,
) -> Result<()>
where
    E: Executor<'e, Database = Postgres>,
{
    sqlx::query!(
        r#"
        UPDATE reading_statistics SET
            spent_reading_minutes = $1,
            remaining_time_minutes = $2,
            last_modified_at = NOW()
        WHERE reading_state_id = $3
        "#,
        spent_reading_minutes,
        remaining_time_minutes,
        state_id,
    )
    .execute(exec)
    .await?;
    Ok(())
}

/// Update reading bookmark for a state.
pub async fn update_bookmark<'e, E>(
    exec: E,
    state_id: Uuid,
    progress_percent: Option<f64>,
    content_source_progress_percent: Option<f64>,
    location_value: Option<&str>,
    location_type: Option<&str>,
    location_source: Option<&str>,
) -> Result<()>
where
    E: Executor<'e, Database = Postgres>,
{
    sqlx::query!(
        r#"
        UPDATE reading_bookmark SET
            progress_percent = $1,
            content_source_progress_percent = $2,
            location_value = $3,
            location_type = $4,
            location_source = $5,
            last_modified_at = NOW()
        WHERE reading_state_id = $6
        "#,
        progress_percent,
        content_source_progress_percent,
        location_value as Option<&str>,
        location_type as Option<&str>,
        location_source as Option<&str>,
        state_id,
    )
    .execute(exec)
    .await?;
    Ok(())
}

/// Touch the updated_at timestamp for a reading state.
pub async fn touch<'e, E>(exec: E, state_id: Uuid) -> Result<()>
where
    E: Executor<'e, Database = Postgres>,
{
    sqlx::query!(
        "UPDATE reading_states SET updated_at = NOW() WHERE id = $1",
        state_id
    )
    .execute(exec)
    .await?;
    Ok(())
}

/// List reading states changed since a timestamp for standalone `ChangedReadingState` emission.
///
/// Excludes books in `exclude_book_ids` (already in entitlement results which carry reading state inline).
/// Only returns for books with an active (non-removed) entitlement visible to this user.
pub async fn list_changed_since(
    pool: &PgPool,
    user_id: Uuid,
    since: OffsetDateTime,
    exclude_book_ids: &[Uuid],
) -> Result<Vec<ReadingStateRow>> {
    let rows = sqlx::query!(
        r#"
        SELECT DISTINCT ON (rs.book_id)
               rs.id, rs.book_id, rs.device_id, rs.created_at, rs.updated_at, rs.priority_timestamp, rs.client_payload,
               st.status, st.last_modified_at as status_last_modified,
               st.times_started_reading, st.last_time_started_reading,
               stats.spent_reading_minutes, stats.remaining_time_minutes,
               stats.last_modified_at as stats_last_modified,
               bm.progress_percent, bm.content_source_progress_percent,
               bm.location_value, bm.location_type, bm.location_source,
               bm.last_modified_at as bookmark_last_modified
        FROM reading_states rs
        JOIN reading_status st ON st.reading_state_id = rs.id
        JOIN reading_statistics stats ON stats.reading_state_id = rs.id
        JOIN reading_bookmark bm ON bm.reading_state_id = rs.id
        WHERE rs.user_id = $1
          AND rs.updated_at > $2
          AND rs.book_id != ALL($3)
          AND EXISTS (
              SELECT 1 FROM entitlements e
              WHERE e.book_id = rs.book_id
                AND e.is_removed = FALSE
                AND (
                    e.scope_type = 'ALL_USERS'
                    OR (e.scope_type = 'USER' AND e.scope_id = $1)
                )
          )
        ORDER BY rs.book_id,
                 CASE WHEN st.status = 'Reading' THEN 0
                      WHEN COALESCE(bm.progress_percent, 0) > 0 THEN 1
                      WHEN st.status = 'Finished' THEN 2
                      ELSE 3 END,
                 rs.updated_at DESC
        "#,
        user_id,
        since,
        exclude_book_ids,
    )
    .fetch_all(pool)
    .await?;

    Ok(rows
        .into_iter()
        .map(|r| ReadingStateRow {
            id: r.id,
            book_id: r.book_id,
            device_id: r.device_id,
            created_at: r.created_at,
            updated_at: r.updated_at,
            priority_timestamp: r.priority_timestamp,
            status: r.status,
            status_last_modified: r.status_last_modified,
            times_started: r.times_started_reading,
            last_time_started: r.last_time_started_reading,
            spent_reading_minutes: r.spent_reading_minutes,
            remaining_time_minutes: r.remaining_time_minutes,
            stats_last_modified: r.stats_last_modified,
            progress_percent: r.progress_percent,
            content_source_progress_percent: r.content_source_progress_percent,
            location_value: r.location_value,
            location_type: r.location_type,
            location_source: r.location_source,
            bookmark_last_modified: r.bookmark_last_modified,
            client_payload: r.client_payload,
        })
        .collect())
}

/// Aggregated reading data for a user+book across all devices.
/// Uses the most recently updated reading state.
#[derive(Debug)]
pub struct AggregatedReadingData {
    pub reading_state_id: Uuid,
    pub status: String,
    pub progress_percent: Option<f64>,
    pub spent_reading_minutes: Option<i32>,
    pub times_started: i32,
    pub last_time_started: Option<OffsetDateTime>,
    pub updated_at: OffsetDateTime,
}

/// Get the most recent reading state for a user+book across all their devices.
/// This aggregates reading progress so all devices can see the same state.
pub async fn get_aggregated_for_user_book<'e, E>(
    exec: E,
    user_id: Uuid,
    book_id: Uuid,
) -> Result<Option<AggregatedReadingData>>
where
    E: Executor<'e, Database = Postgres>,
{
    let row = sqlx::query!(
        r#"
        SELECT rs.id, st.status, bm.progress_percent, stats.spent_reading_minutes,
               st.times_started_reading, st.last_time_started_reading, rs.updated_at
        FROM reading_states rs
        JOIN reading_status st ON st.reading_state_id = rs.id
        JOIN reading_statistics stats ON stats.reading_state_id = rs.id
        JOIN reading_bookmark bm ON bm.reading_state_id = rs.id
        WHERE rs.user_id = $1 AND rs.book_id = $2
        ORDER BY
            CASE
                WHEN st.status = 'Reading' THEN 0
                WHEN COALESCE(bm.progress_percent, 0) > 0 THEN 1
                WHEN st.status = 'Finished' THEN 2
                ELSE 3
            END,
            rs.updated_at DESC
        LIMIT 1
        "#,
        user_id,
        book_id,
    )
    .fetch_optional(exec)
    .await?;

    Ok(row.map(|r| AggregatedReadingData {
        reading_state_id: r.id,
        status: r.status,
        progress_percent: r.progress_percent,
        spent_reading_minutes: r.spent_reading_minutes,
        times_started: r.times_started_reading,
        last_time_started: r.last_time_started_reading,
        updated_at: r.updated_at,
    }))
}

/// Get total spent reading minutes for a user+book across all devices.
/// This sums up reading time from all device reading states.
pub async fn get_total_reading_time<'e, E>(
    exec: E,
    user_id: Uuid,
    book_id: Uuid,
) -> Result<Option<i32>>
where
    E: Executor<'e, Database = Postgres>,
{
    let total: Option<i64> = sqlx::query_scalar!(
        r#"
        SELECT SUM(stats.spent_reading_minutes)::bigint
        FROM reading_states rs
        JOIN reading_statistics stats ON stats.reading_state_id = rs.id
        WHERE rs.user_id = $1 AND rs.book_id = $2
        "#,
        user_id,
        book_id,
    )
    .fetch_one(exec)
    .await?;

    Ok(total.map(|t| t as i32))
}

/// Add reading time to a specific reading state (by state ID).
pub async fn add_reading_time_by_state<'e, E>(exec: E, state_id: Uuid, minutes: i32) -> Result<bool>
where
    E: Executor<'e, Database = Postgres>,
{
    let result = sqlx::query!(
        r#"
        UPDATE reading_statistics
        SET spent_reading_minutes = COALESCE(spent_reading_minutes, 0) + $2,
            last_modified_at = NOW()
        WHERE reading_state_id = $1
        "#,
        state_id,
        minutes,
    )
    .execute(exec)
    .await?;

    Ok(result.rows_affected() > 0)
}

/// Get the current reading status for a reading state.
pub async fn get_status<'e, E>(exec: E, state_id: Uuid) -> Result<Option<String>>
where
    E: Executor<'e, Database = Postgres>,
{
    let status: Option<String> = sqlx::query_scalar!(
        "SELECT status FROM reading_status WHERE reading_state_id = $1",
        state_id
    )
    .fetch_optional(exec)
    .await?;
    Ok(status)
}

/// Add reading time to the most recent reading state for a user+book.
/// If no reading state exists, this is a no-op.
pub async fn add_reading_time<'e, E>(
    exec: E,
    user_id: Uuid,
    book_id: Uuid,
    minutes: i32,
) -> Result<bool>
where
    E: Executor<'e, Database = Postgres>,
{
    let result = sqlx::query!(
        r#"
        UPDATE reading_statistics
        SET spent_reading_minutes = COALESCE(spent_reading_minutes, 0) + $3,
            last_modified_at = NOW()
        WHERE reading_state_id = (
            SELECT rs.id
            FROM reading_states rs
            WHERE rs.user_id = $1 AND rs.book_id = $2
            ORDER BY rs.updated_at DESC
            LIMIT 1
        )
        "#,
        user_id,
        book_id,
        minutes,
    )
    .execute(exec)
    .await?;

    Ok(result.rows_affected() > 0)
}
