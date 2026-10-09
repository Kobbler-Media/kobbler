//! IPTV sync log repository operations.

use super::Result;
use sqlx::{Executor, Postgres};
use time::OffsetDateTime;
use uuid::Uuid;

// =============================================================================
// Types
// =============================================================================

#[derive(Debug)]
pub struct SyncLogRow {
    pub id: Uuid,
    pub provider_id: Uuid,
    pub started_at: OffsetDateTime,
    pub completed_at: Option<OffsetDateTime>,
    pub status: String,
    pub channels_matched: i32,
    pub sources_added: i32,
    pub sources_removed: i32,
    pub unmatched_count: i32,
    pub epg_entries: i32,
    pub error_message: Option<String>,
}

#[derive(Debug)]
pub struct SyncStats {
    pub channels_matched: i32,
    pub sources_added: i32,
    pub sources_removed: i32,
    pub unmatched_count: i32,
    pub epg_entries: i32,
}

// =============================================================================
// Operations
// =============================================================================

/// Create a new sync log entry (status = 'running').
pub async fn create<'e, E>(exec: E, provider_id: Uuid) -> Result<Uuid>
where
    E: Executor<'e, Database = Postgres>,
{
    let id = Uuid::now_v7();
    sqlx::query!(
        r#"
        INSERT INTO iptv_sync_log (id, provider_id, started_at)
        VALUES ($1, $2, NOW())
        "#,
        id,
        provider_id,
    )
    .execute(exec)
    .await?;

    Ok(id)
}

/// Mark a sync log entry as completed with stats.
pub async fn complete<'e, E>(exec: E, id: Uuid, stats: SyncStats) -> Result<bool>
where
    E: Executor<'e, Database = Postgres>,
{
    let result = sqlx::query!(
        r#"
        UPDATE iptv_sync_log SET
            completed_at = NOW(),
            status = 'completed',
            channels_matched = $2,
            sources_added = $3,
            sources_removed = $4,
            unmatched_count = $5,
            epg_entries = $6
        WHERE id = $1
        "#,
        id,
        stats.channels_matched,
        stats.sources_added,
        stats.sources_removed,
        stats.unmatched_count,
        stats.epg_entries,
    )
    .execute(exec)
    .await?;

    Ok(result.rows_affected() > 0)
}

/// Mark a sync log entry as failed with an error message.
pub async fn fail<'e, E>(exec: E, id: Uuid, error: &str) -> Result<bool>
where
    E: Executor<'e, Database = Postgres>,
{
    let result = sqlx::query!(
        r#"
        UPDATE iptv_sync_log SET
            completed_at = NOW(),
            status = 'failed',
            error_message = $2
        WHERE id = $1
        "#,
        id,
        error,
    )
    .execute(exec)
    .await?;

    Ok(result.rows_affected() > 0)
}

/// List sync log entries for a provider, most recent first.
pub async fn list_by_provider<'e, E>(
    exec: E,
    provider_id: Uuid,
    limit: i64,
) -> Result<Vec<SyncLogRow>>
where
    E: Executor<'e, Database = Postgres>,
{
    let rows = sqlx::query!(
        r#"
        SELECT id, provider_id, started_at, completed_at, status, channels_matched,
               sources_added, sources_removed, unmatched_count, epg_entries, error_message
        FROM iptv_sync_log
        WHERE provider_id = $1
        ORDER BY started_at DESC
        LIMIT $2
        "#,
        provider_id,
        limit,
    )
    .fetch_all(exec)
    .await?;

    Ok(rows
        .into_iter()
        .map(|r| SyncLogRow {
            id: r.id,
            provider_id: r.provider_id,
            started_at: r.started_at,
            completed_at: r.completed_at,
            status: r.status,
            channels_matched: r.channels_matched,
            sources_added: r.sources_added,
            sources_removed: r.sources_removed,
            unmatched_count: r.unmatched_count,
            epg_entries: r.epg_entries,
            error_message: r.error_message,
        })
        .collect())
}
