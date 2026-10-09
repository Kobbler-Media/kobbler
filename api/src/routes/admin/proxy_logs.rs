//! Proxy log route handlers.

use axum::{
    body::Body,
    extract::{Json, Path, State},
    http::{HeaderMap, HeaderValue, StatusCode},
    response::{IntoResponse, Response},
};
use serde::{Deserialize, Serialize};
use sqlx::{QueryBuilder, Row};
use time::OffsetDateTime;
use tokio_util::io::ReaderStream;
use uuid::Uuid;

use std::collections::HashMap;

use crate::kobo_handler::entity_scan;
use crate::repositories;
use crate::routes::AuthUser;
use crate::{AppState, blob_storage::BlobStorage};

// ========== Constants ==========

/// Column list for proxy log queries (list endpoint - excludes blob paths)
const PROXY_LOG_COLUMNS: &str = r#"id, device_id, request_method, request_path, request_query,
               request_headers, request_body, request_body_size,
               upstream_status, upstream_headers, upstream_body, upstream_body_size,
               response_status, response_headers, response_body, response_body_size,
               decision, upstream_url, upstream_duration_ms, total_duration_ms,
               severity, notes, status, created_at,
               request_blob_path IS NOT NULL as has_request_blob,
               response_blob_path IS NOT NULL as has_response_blob,
               entity_refs, has_alert, alert_message"#;

/// Column list for proxy log export (includes actual blob paths)
const PROXY_LOG_COLUMNS_WITH_BLOBS: &str = r#"id, device_id, request_method, request_path, request_query,
               request_headers, request_body, request_body_size,
               upstream_status, upstream_headers, upstream_body, upstream_body_size,
               response_status, response_headers, response_body, response_body_size,
               decision, upstream_url, upstream_duration_ms, total_duration_ms,
               severity, notes, status, created_at,
               request_blob_path, response_blob_path,
               entity_refs, has_alert, alert_message"#;

// ========== Types ==========

#[derive(Deserialize)]
pub struct ProxyLogsQuery {
    pub limit: Option<i64>,
    pub offset: Option<i64>,
    pub device_id: Option<Uuid>,
    pub severity: Option<String>,
    pub path: Option<String>,
    pub method: Option<String>,
    pub has_alert: Option<bool>,
}

#[derive(Serialize)]
pub struct ProxyLogEntry {
    pub id: Uuid,
    pub device_id: Option<Uuid>,
    pub request_method: String,
    pub request_path: String,
    pub request_query: Option<String>,
    pub request_headers: Option<serde_json::Value>,
    pub request_body: Option<String>,
    pub request_body_size: i64,
    pub upstream_status: Option<i16>,
    pub upstream_headers: Option<serde_json::Value>,
    pub upstream_body: Option<String>,
    pub upstream_body_size: i64,
    pub response_status: Option<i16>,
    pub response_headers: Option<serde_json::Value>,
    pub response_body: Option<String>,
    pub response_body_size: i64,
    pub decision: String,
    pub upstream_url: Option<String>,
    pub upstream_duration_ms: Option<i32>,
    pub total_duration_ms: Option<i32>,
    pub severity: String,
    pub notes: Option<String>,
    /// Log status: 'in_progress', 'complete', 'error'
    pub status: String,
    #[serde(with = "time::serde::rfc3339")]
    pub created_at: OffsetDateTime,
    /// Whether a request blob was captured (path not exposed, just presence)
    pub has_request_blob: bool,
    /// Whether a response blob was captured (path not exposed, just presence)
    pub has_response_blob: bool,
    /// Resolved entity references found in path/bodies
    pub entity_refs: serde_json::Value,
    /// Whether this log entry has an alert condition
    pub has_alert: bool,
    /// Descriptive alert message explaining why this entry is flagged
    pub alert_message: Option<String>,
}

#[derive(Serialize)]
pub struct ProxyLogsResponse {
    pub logs: Vec<ProxyLogEntry>,
    pub total: i64,
}

/// Query parameters for proxy log export.
#[derive(Deserialize)]
pub struct ExportLogsQuery {
    pub since: Option<String>,       // ISO timestamp
    pub device_id: Option<Uuid>,     // Filter to specific device
    pub include_blobs: Option<bool>, // Include blob files (default true)
}

#[derive(Deserialize)]
pub struct BlobQuery {
    pub blob_type: String, // "request" or "response"
}

// ========== Helper Functions ==========

/// Parse path filter string into terms (space-separated, ! prefix for negation)
/// e.g., "library !sync !init" -> [(false, "library"), (true, "sync"), (true, "init")]
fn parse_path_filter(path: &str) -> Vec<(bool, String)> {
    path.split_whitespace()
        .filter(|t| !t.is_empty())
        .filter_map(|t| {
            if let Some(rest) = t.strip_prefix('!') {
                if !rest.is_empty() {
                    Some((true, rest.to_string())) // negated
                } else {
                    None
                }
            } else {
                Some((false, t.to_string())) // positive
            }
        })
        .collect()
}

/// Map a database row to a ProxyLogEntry struct
fn row_to_proxy_log_entry(row: &sqlx::postgres::PgRow) -> ProxyLogEntry {
    ProxyLogEntry {
        id: row.get("id"),
        device_id: row.get("device_id"),
        request_method: row.get("request_method"),
        request_path: row.get("request_path"),
        request_query: row.get("request_query"),
        request_headers: row.get("request_headers"),
        request_body: row.get("request_body"),
        request_body_size: row.get("request_body_size"),
        upstream_status: row.get("upstream_status"),
        upstream_headers: row.get("upstream_headers"),
        upstream_body: row.get("upstream_body"),
        upstream_body_size: row.get("upstream_body_size"),
        response_status: row.get("response_status"),
        response_headers: row.get("response_headers"),
        response_body: row.get("response_body"),
        response_body_size: row.get("response_body_size"),
        decision: row.get("decision"),
        upstream_url: row.get("upstream_url"),
        upstream_duration_ms: row.get("upstream_duration_ms"),
        total_duration_ms: row.get("total_duration_ms"),
        severity: row.get("severity"),
        notes: row.get("notes"),
        status: row.get("status"),
        created_at: row.get("created_at"),
        has_request_blob: row.get("has_request_blob"),
        has_response_blob: row.get("has_response_blob"),
        entity_refs: row.get("entity_refs"),
        has_alert: row.get("has_alert"),
        alert_message: row.get("alert_message"),
    }
}

// ========== Handlers ==========

pub async fn list_proxy_logs(
    State(state): State<AppState>,
    _auth: AuthUser,
    axum::extract::Query(query): axum::extract::Query<ProxyLogsQuery>,
) -> impl IntoResponse {
    let limit = query.limit.unwrap_or(100).min(1000);
    let offset = query.offset.unwrap_or(0);
    let path_terms = query
        .path
        .as_ref()
        .map(|p| parse_path_filter(p))
        .unwrap_or_default();

    // Build data query
    let mut data_query: QueryBuilder<sqlx::Postgres> = QueryBuilder::new(format!(
        "SELECT {} FROM proxy_logs WHERE 1=1",
        PROXY_LOG_COLUMNS
    ));

    // Build count query in parallel
    let mut count_query: QueryBuilder<sqlx::Postgres> =
        QueryBuilder::new("SELECT COUNT(*) as count FROM proxy_logs WHERE 1=1");

    // Apply filters to both queries
    if let Some(device_id) = query.device_id {
        data_query.push(" AND device_id = ").push_bind(device_id);
        count_query.push(" AND device_id = ").push_bind(device_id);
    }

    if let Some(severity) = &query.severity {
        data_query
            .push(" AND severity = ")
            .push_bind(severity.clone());
        count_query.push(" AND severity = ").push_bind(severity);
    }

    for (negated, term) in &path_terms {
        let op = if *negated { " NOT LIKE " } else { " LIKE " };
        let pattern = format!("%{}%", term);
        data_query
            .push(" AND request_path")
            .push(op)
            .push_bind(pattern.clone());
        count_query
            .push(" AND request_path")
            .push(op)
            .push_bind(pattern);
    }

    if let Some(method) = &query.method {
        data_query
            .push(" AND request_method = ")
            .push_bind(method.clone());
        count_query.push(" AND request_method = ").push_bind(method);
    }

    if let Some(has_alert) = query.has_alert {
        data_query.push(" AND has_alert = ").push_bind(has_alert);
        count_query.push(" AND has_alert = ").push_bind(has_alert);
    }

    // Add ordering and pagination to data query
    data_query
        .push(" ORDER BY created_at DESC LIMIT ")
        .push_bind(limit)
        .push(" OFFSET ")
        .push_bind(offset);

    // Execute count query
    let total: i64 = match count_query.build().fetch_one(&state.pool).await {
        Ok(row) => row.get("count"),
        Err(e) => {
            tracing::error!(error = %e, "failed to count proxy logs");
            return (StatusCode::INTERNAL_SERVER_ERROR, "database error").into_response();
        }
    };

    // Execute data query
    let rows = match data_query.build().fetch_all(&state.pool).await {
        Ok(rows) => rows,
        Err(e) => {
            tracing::error!(error = %e, "failed to list proxy logs");
            return (StatusCode::INTERNAL_SERVER_ERROR, "database error").into_response();
        }
    };

    let logs = rows.iter().map(row_to_proxy_log_entry).collect();
    Json(ProxyLogsResponse { logs, total }).into_response()
}

pub async fn clear_proxy_logs(State(state): State<AppState>, _auth: AuthUser) -> impl IntoResponse {
    // Get all log IDs to delete blob files
    let log_ids = match repositories::proxy_logs::get_all_ids(&state.pool).await {
        Ok(ids) => ids,
        Err(e) => {
            tracing::error!(error = %e, "Failed to query proxy log IDs for cleanup");
            return (StatusCode::INTERNAL_SERVER_ERROR, "failed to clear logs").into_response();
        }
    };

    // Delete blob files first
    if !log_ids.is_empty() {
        let blob_storage = BlobStorage::new(&state.config.storage_root);
        blob_storage.delete_for_logs(&log_ids).await;
    }

    match repositories::proxy_logs::delete_all(&state.pool).await {
        Ok(deleted) => Json(serde_json::json!({"deleted": deleted})).into_response(),
        Err(e) => {
            tracing::error!(error = %e, "Failed to clear proxy logs");
            (StatusCode::INTERNAL_SERVER_ERROR, "failed to clear logs").into_response()
        }
    }
}

/// Export proxy logs as a ZIP file.
pub async fn export_proxy_logs(
    State(state): State<AppState>,
    _auth: AuthUser,
    axum::extract::Query(query): axum::extract::Query<ExportLogsQuery>,
) -> impl IntoResponse {
    let include_blobs = query.include_blobs.unwrap_or(true);

    // Build query using QueryBuilder
    let mut qb: QueryBuilder<sqlx::Postgres> = QueryBuilder::new(format!(
        "SELECT {} FROM proxy_logs WHERE 1=1",
        PROXY_LOG_COLUMNS_WITH_BLOBS
    ));

    if let Some(device_id) = query.device_id {
        qb.push(" AND device_id = ").push_bind(device_id);
    }

    if let Some(ref since) = query.since {
        qb.push(" AND created_at >= ").push_bind(since.clone());
    }

    qb.push(" ORDER BY created_at DESC");

    let rows = match qb.build().fetch_all(&state.pool).await {
        Ok(rows) => rows,
        Err(e) => {
            tracing::error!(error = %e, "failed to export proxy logs");
            return (StatusCode::INTERNAL_SERVER_ERROR, "database error").into_response();
        }
    };

    // Build JSON log entries
    let logs: Vec<serde_json::Value> = rows
        .iter()
        .map(|row| {
            serde_json::json!({
                "id": row.get::<Uuid, _>("id").to_string(),
                "device_id": row.get::<Option<Uuid>, _>("device_id").map(|u| u.to_string()),
                "request_method": row.get::<String, _>("request_method"),
                "request_path": row.get::<String, _>("request_path"),
                "request_query": row.get::<Option<String>, _>("request_query"),
                "request_headers": row.get::<Option<serde_json::Value>, _>("request_headers"),
                "request_body": row.get::<Option<String>, _>("request_body"),
                "request_body_size": row.get::<i64, _>("request_body_size"),
                "upstream_status": row.get::<Option<i16>, _>("upstream_status"),
                "upstream_headers": row.get::<Option<serde_json::Value>, _>("upstream_headers"),
                "upstream_body": row.get::<Option<String>, _>("upstream_body"),
                "upstream_body_size": row.get::<i64, _>("upstream_body_size"),
                "response_status": row.get::<Option<i16>, _>("response_status"),
                "response_headers": row.get::<Option<serde_json::Value>, _>("response_headers"),
                "response_body": row.get::<Option<String>, _>("response_body"),
                "response_body_size": row.get::<i64, _>("response_body_size"),
                "decision": row.get::<String, _>("decision"),
                "upstream_url": row.get::<Option<String>, _>("upstream_url"),
                "upstream_duration_ms": row.get::<Option<i32>, _>("upstream_duration_ms"),
                "total_duration_ms": row.get::<Option<i32>, _>("total_duration_ms"),
                "severity": row.get::<String, _>("severity"),
                "notes": row.get::<Option<String>, _>("notes"),
                "status": row.get::<String, _>("status"),
                "entity_refs": row.get::<serde_json::Value, _>("entity_refs"),
                "has_alert": row.get::<bool, _>("has_alert"),
                "alert_message": row.get::<Option<String>, _>("alert_message"),
                "created_at": row.get::<OffsetDateTime, _>("created_at").to_string(),
            })
        })
        .collect();

    // Create ZIP in memory
    let mut zip_buffer = std::io::Cursor::new(Vec::new());
    {
        let mut zip = zip::ZipWriter::new(&mut zip_buffer);
        let options = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Deflated);

        // Add logs.json
        if let Err(e) = zip.start_file("logs.json", options) {
            tracing::error!(error = %e, "Failed to start logs.json in ZIP");
            return (StatusCode::INTERNAL_SERVER_ERROR, "failed to create export").into_response();
        }
        let logs_json = match serde_json::to_vec_pretty(&logs) {
            Ok(json) => json,
            Err(e) => {
                tracing::error!(error = %e, "Failed to serialize logs to JSON");
                return (StatusCode::INTERNAL_SERVER_ERROR, "failed to create export")
                    .into_response();
            }
        };
        if let Err(e) = std::io::Write::write_all(&mut zip, &logs_json) {
            tracing::error!(error = %e, "Failed to write logs.json");
            return (StatusCode::INTERNAL_SERVER_ERROR, "failed to create export").into_response();
        }

        // Add blob files if requested
        if include_blobs {
            for row in &rows {
                let log_id: Uuid = row.get("id");

                // Request blob
                if let Some(request_path) = row.get::<Option<String>, _>("request_blob_path") {
                    let blob_filename = format!("request_blobs/{}_request.bin", log_id);
                    if let Ok(data) = std::fs::read(&request_path)
                        && zip.start_file(&blob_filename, options).is_ok()
                    {
                        let _ = std::io::Write::write_all(&mut zip, &data);
                    }
                }

                // Response blob
                if let Some(response_path) = row.get::<Option<String>, _>("response_blob_path") {
                    let blob_filename = format!("response_blobs/{}_response.bin", log_id);
                    if let Ok(data) = std::fs::read(&response_path)
                        && zip.start_file(&blob_filename, options).is_ok()
                    {
                        let _ = std::io::Write::write_all(&mut zip, &data);
                    }
                }
            }
        }

        if let Err(e) = zip.finish() {
            tracing::error!(error = %e, "Failed to finish ZIP");
            return (StatusCode::INTERNAL_SERVER_ERROR, "failed to create export").into_response();
        }
    }

    let zip_bytes = zip_buffer.into_inner();
    let timestamp = time::OffsetDateTime::now_utc()
        .format(&time::format_description::well_known::Iso8601::DEFAULT)
        .unwrap_or_else(|_| "unknown".to_string())
        .replace(':', "-");
    let filename = format!("proxy-logs-{}.zip", timestamp);

    let mut response_headers = HeaderMap::new();
    response_headers.insert("Content-Type", HeaderValue::from_static("application/zip"));
    if let Ok(hv) = HeaderValue::from_str(&format!("attachment; filename=\"{}\"", filename)) {
        response_headers.insert("Content-Disposition", hv);
    }
    if let Ok(hv) = HeaderValue::from_str(&zip_bytes.len().to_string()) {
        response_headers.insert("Content-Length", hv);
    }

    (response_headers, zip_bytes).into_response()
}

pub async fn download_proxy_log_blob(
    State(state): State<AppState>,
    _auth: AuthUser,
    Path(log_id): Path<Uuid>,
    axum::extract::Query(query): axum::extract::Query<BlobQuery>,
) -> impl IntoResponse {
    // Get the log entry to check blob path and original headers
    let paths = match repositories::proxy_logs::get_blob_paths(&state.pool, log_id).await {
        Ok(Some(paths)) => paths,
        Ok(None) => return (StatusCode::NOT_FOUND, "log not found").into_response(),
        Err(e) => {
            tracing::error!(error = %e, "Failed to query proxy log");
            return (StatusCode::INTERNAL_SERVER_ERROR, "database error").into_response();
        }
    };

    let (blob_path, original_headers): (Option<String>, Option<serde_json::Value>) =
        match query.blob_type.as_str() {
            "request" => (paths.request_blob_path, None),
            "response" => (paths.response_blob_path, paths.original_response_headers),
            _ => {
                return (
                    StatusCode::BAD_REQUEST,
                    "blob_type must be 'request' or 'response'",
                )
                    .into_response();
            }
        };

    let blob_path = match blob_path {
        Some(p) => p,
        None => return (StatusCode::NOT_FOUND, "blob not captured").into_response(),
    };

    // Open file for streaming - don't load into memory
    let file_path = std::path::Path::new(&blob_path);
    let file = match tokio::fs::File::open(file_path).await {
        Ok(f) => f,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return (StatusCode::NOT_FOUND, "blob file not found").into_response();
        }
        Err(e) => {
            tracing::error!(error = %e, "Failed to open blob file");
            return (StatusCode::INTERNAL_SERVER_ERROR, "failed to read blob").into_response();
        }
    };

    // Get file size for Content-Length
    let file_size = match file.metadata().await {
        Ok(m) => m.len(),
        Err(e) => {
            tracing::error!(error = %e, "Failed to get blob file metadata");
            return (StatusCode::INTERNAL_SERVER_ERROR, "failed to read blob").into_response();
        }
    };

    // Build response headers from original headers if available (for response blobs)
    // Headers are stored as object: {"Header": "value"} or {"Header": ["val1", "val2"]} for duplicates
    let mut response_headers = HeaderMap::new();

    if let Some(headers_json) = original_headers
        && let Some(headers_obj) = headers_json.as_object()
    {
        // Try to preserve Content-Type and Content-Disposition
        for (key, value) in headers_obj {
            // Value can be string or array of strings
            let first_value = match value {
                serde_json::Value::String(s) => Some(s.as_str()),
                serde_json::Value::Array(arr) => arr.first().and_then(|v| v.as_str()),
                _ => None,
            };

            if let Some(val) = first_value {
                if key.eq_ignore_ascii_case("content-type")
                    && let Ok(hv) = HeaderValue::from_str(val)
                {
                    response_headers.insert("Content-Type", hv);
                } else if key.eq_ignore_ascii_case("content-disposition")
                    && let Ok(hv) = HeaderValue::from_str(val)
                {
                    response_headers.insert("Content-Disposition", hv);
                }
            }
        }
    }

    // Set defaults if not present
    if !response_headers.contains_key("Content-Type") {
        response_headers.insert(
            "Content-Type",
            HeaderValue::from_static("application/octet-stream"),
        );
    }
    if !response_headers.contains_key("Content-Disposition") {
        let filename = format!("{}_{}.bin", log_id, query.blob_type);
        if let Ok(hv) = HeaderValue::from_str(&format!("attachment; filename=\"{}\"", filename)) {
            response_headers.insert("Content-Disposition", hv);
        }
    }

    // Add Content-Length header
    if let Ok(hv) = HeaderValue::from_str(&file_size.to_string()) {
        response_headers.insert("Content-Length", hv);
    }

    // Stream the file instead of buffering in memory
    let stream = ReaderStream::new(file);
    let body = Body::from_stream(stream);

    Response::builder()
        .status(StatusCode::OK)
        .body(body)
        .map(|mut response| {
            *response.headers_mut() = response_headers;
            response
        })
        .unwrap_or_else(|_| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                "failed to build response",
            )
                .into_response()
        })
}

// ========== UUID Resolution ==========

/// Maximum number of UUIDs to resolve in a single API call.
const MAX_RESOLVE_UUIDS: usize = 100;

#[derive(Deserialize)]
pub struct ResolveUuidsQuery {
    pub ids: String,
}

pub async fn resolve_uuids(
    State(state): State<AppState>,
    _auth: AuthUser,
    axum::extract::Query(query): axum::extract::Query<ResolveUuidsQuery>,
) -> impl IntoResponse {
    let uuids: Vec<Uuid> = query
        .ids
        .split(',')
        .filter(|s| !s.is_empty())
        .filter_map(|s| Uuid::parse_str(s.trim()).ok())
        .take(MAX_RESOLVE_UUIDS)
        .collect();

    let resolved = entity_scan::batch_resolve(&state.pool, &uuids).await;

    // Build response: { "entities": { "uuid": { "entity_type": "...", "name": "..." } | null } }
    let mut entities: HashMap<String, Option<entity_scan::ResolvedEntity>> = HashMap::new();
    for uuid in &uuids {
        entities.insert(uuid.to_string(), resolved.get(uuid).cloned());
    }

    Json(serde_json::json!({ "entities": entities })).into_response()
}
