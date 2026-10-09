//! Device sync state repository operations.

use super::Result;
use sqlx::{Executor, Postgres};
use time::OffsetDateTime;
use uuid::Uuid;

/// Delete sync state for a device.
pub async fn delete_by_device<'e, E>(exec: E, device_id: Uuid) -> Result<u64>
where
    E: Executor<'e, Database = Postgres>,
{
    let result = sqlx::query!(
        "DELETE FROM device_sync_state WHERE device_id = $1",
        device_id
    )
    .execute(exec)
    .await?;
    Ok(result.rows_affected())
}

/// Get last sync timestamp for a device.
pub async fn get_last_sync_at<'e, E>(exec: E, device_id: Uuid) -> Result<Option<OffsetDateTime>>
where
    E: Executor<'e, Database = Postgres>,
{
    let row = sqlx::query_scalar!(
        "SELECT last_sync_at FROM device_sync_state WHERE device_id = $1",
        device_id,
    )
    .fetch_optional(exec)
    .await?;
    Ok(row.flatten())
}

/// Upsert last sync timestamp for a device.
pub async fn upsert_last_sync_at<'e, E>(
    exec: E,
    device_id: Uuid,
    sync_time: OffsetDateTime,
) -> Result<()>
where
    E: Executor<'e, Database = Postgres>,
{
    sqlx::query!(
        r#"
        INSERT INTO device_sync_state (device_id, last_sync_at)
        VALUES ($1, $2)
        ON CONFLICT (device_id) DO UPDATE SET last_sync_at = EXCLUDED.last_sync_at
        "#,
        device_id,
        sync_time,
    )
    .execute(exec)
    .await?;
    Ok(())
}
