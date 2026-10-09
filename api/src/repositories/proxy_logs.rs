//! Proxy logs repository operations.

use super::Result;
use serde_json::Value;
use sqlx::{Executor, Postgres};
use uuid::Uuid;

/// Input for creating an initial proxy log entry (inflight request).
#[derive(Debug)]
pub struct ProxyLogInitialInput<'a> {
    pub id: Uuid,
    pub device_id: Option<Uuid>,
    pub method: &'a str,
    pub path: &'a str,
    pub query: Option<&'a str>,
    pub request_headers: Option<Value>,
    pub decision: &'a str,
}

/// Create an initial proxy log entry for inflight request.
pub async fn create_initial<'e, E>(exec: E, input: &ProxyLogInitialInput<'_>) -> Result<()>
where
    E: Executor<'e, Database = Postgres>,
{
    sqlx::query!(
        r#"
        INSERT INTO proxy_logs (
            id, device_id, request_method, request_path, request_query,
            request_headers, decision, status
        ) VALUES ($1, $2, $3, $4, $5, $6, $7, 'in_progress')
        "#,
        input.id,
        input.device_id as Option<Uuid>,
        input.method,
        input.path,
        input.query as Option<&str>,
        input.request_headers.as_ref() as Option<&Value>,
        input.decision,
    )
    .execute(exec)
    .await?;
    Ok(())
}

/// Input for completing a proxy log entry.
#[derive(Debug)]
pub struct ProxyLogCompleteInput<'a> {
    pub request_body: Option<&'a str>,
    pub request_body_size: i64,
    pub upstream_status: Option<i16>,
    pub upstream_headers: Option<Value>,
    pub upstream_body: Option<&'a str>,
    pub upstream_body_size: i64,
    pub response_status: i16,
    pub response_headers: Option<Value>,
    pub response_body: Option<&'a str>,
    pub response_body_size: i64,
    pub upstream_url: Option<&'a str>,
    pub upstream_duration_ms: Option<i32>,
    pub total_duration_ms: i32,
    pub severity: &'a str,
    pub notes: Option<&'a str>,
    pub request_blob_path: Option<&'a str>,
    pub response_blob_path: Option<&'a str>,
    pub original_response_headers: Option<Value>,
    pub log_status: &'a str,
    pub entity_refs: Value,
    pub has_alert: bool,
    pub alert_message: Option<&'a str>,
}

/// Complete a proxy log entry with response data.
pub async fn complete<'e, E>(exec: E, id: Uuid, input: &ProxyLogCompleteInput<'_>) -> Result<()>
where
    E: Executor<'e, Database = Postgres>,
{
    sqlx::query!(
        r#"
        UPDATE proxy_logs SET
            request_body = $2,
            request_body_size = $3,
            upstream_status = $4,
            upstream_headers = $5,
            upstream_body = $6,
            upstream_body_size = $7,
            response_status = $8,
            response_headers = $9,
            response_body = $10,
            response_body_size = $11,
            upstream_url = $12,
            upstream_duration_ms = $13,
            total_duration_ms = $14,
            severity = $15,
            notes = $16,
            request_blob_path = $17,
            response_blob_path = $18,
            original_response_headers = $19,
            status = $20,
            entity_refs = $21,
            has_alert = $22,
            alert_message = $23
        WHERE id = $1
        "#,
        id,
        input.request_body as Option<&str>,
        input.request_body_size,
        input.upstream_status as Option<i16>,
        input.upstream_headers.as_ref() as Option<&Value>,
        input.upstream_body as Option<&str>,
        input.upstream_body_size,
        input.response_status,
        input.response_headers.as_ref() as Option<&Value>,
        input.response_body as Option<&str>,
        input.response_body_size,
        input.upstream_url as Option<&str>,
        input.upstream_duration_ms as Option<i32>,
        input.total_duration_ms,
        input.severity,
        input.notes as Option<&str>,
        input.request_blob_path as Option<&str>,
        input.response_blob_path as Option<&str>,
        input.original_response_headers.as_ref() as Option<&Value>,
        input.log_status,
        input.entity_refs,
        input.has_alert,
        input.alert_message as Option<&str>,
    )
    .execute(exec)
    .await?;
    Ok(())
}

/// Delete all proxy logs.
pub async fn delete_all<'e, E>(exec: E) -> Result<u64>
where
    E: Executor<'e, Database = Postgres>,
{
    let result = sqlx::query!("DELETE FROM proxy_logs").execute(exec).await?;
    Ok(result.rows_affected())
}

/// Get all proxy log IDs.
pub async fn get_all_ids<'e, E>(exec: E) -> Result<Vec<Uuid>>
where
    E: Executor<'e, Database = Postgres>,
{
    let ids: Vec<Uuid> = sqlx::query_scalar!("SELECT id FROM proxy_logs")
        .fetch_all(exec)
        .await?;
    Ok(ids)
}

/// Delete old proxy logs (older than specified days).
pub async fn delete_older_than_days<'e, E>(exec: E, days: i32) -> Result<u64>
where
    E: Executor<'e, Database = Postgres>,
{
    let result = sqlx::query!(
        "DELETE FROM proxy_logs WHERE created_at < NOW() - make_interval(days => $1)",
        days
    )
    .execute(exec)
    .await?;
    Ok(result.rows_affected())
}

/// Get IDs of logs older than specified days (for batched deletion).
pub async fn get_old_ids<'e, E>(exec: E, days: i32, limit: i64) -> Result<Vec<Uuid>>
where
    E: Executor<'e, Database = Postgres>,
{
    let ids: Vec<Uuid> = sqlx::query_scalar!(
        "SELECT id FROM proxy_logs WHERE created_at < NOW() - make_interval(days => $1) ORDER BY created_at ASC LIMIT $2",
        days,
        limit,
    )
    .fetch_all(exec)
    .await?;

    Ok(ids)
}

/// List all IDs of logs older than specified days.
pub async fn list_old_ids<'e, E>(exec: E, days: i32) -> Result<Vec<Uuid>>
where
    E: Executor<'e, Database = Postgres>,
{
    let ids: Vec<Uuid> = sqlx::query_scalar!(
        "SELECT id FROM proxy_logs WHERE created_at < NOW() - make_interval(days => $1)",
        days,
    )
    .fetch_all(exec)
    .await?;
    Ok(ids)
}

/// Delete logs by IDs.
pub async fn delete_by_ids<'e, E>(exec: E, ids: &[Uuid]) -> Result<u64>
where
    E: Executor<'e, Database = Postgres>,
{
    let result = sqlx::query!("DELETE FROM proxy_logs WHERE id = ANY($1)", ids)
        .execute(exec)
        .await?;
    Ok(result.rows_affected())
}

/// Blob paths for a proxy log entry.
#[derive(Debug)]
pub struct ProxyLogBlobPaths {
    pub request_blob_path: Option<String>,
    pub response_blob_path: Option<String>,
    pub original_response_headers: Option<Value>,
}

/// Get blob paths for a proxy log entry.
pub async fn get_blob_paths<'e, E>(exec: E, id: Uuid) -> Result<Option<ProxyLogBlobPaths>>
where
    E: Executor<'e, Database = Postgres>,
{
    let row = sqlx::query_as!(
        ProxyLogBlobPaths,
        "SELECT request_blob_path, response_blob_path, original_response_headers FROM proxy_logs WHERE id = $1",
        id,
    )
    .fetch_optional(exec)
    .await?;

    Ok(row)
}
