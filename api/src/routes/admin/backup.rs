//! Backup and restore route handlers.

use crate::streaming::byte_range::file_stream;
use axum::{
    Json,
    body::Body,
    extract::{Query, State},
    http::{HeaderMap, HeaderValue, StatusCode},
    response::{IntoResponse, Response},
};
use serde::{Deserialize, Serialize};
use std::path::Path as StdPath;
use time::OffsetDateTime;

use crate::jobs::{self, JobContext};
use crate::repositories;
use crate::routes::AuthUser;
use crate::{AppState, backup};

// ========== Backup & Restore ==========

#[derive(Deserialize)]
pub struct BackupQuery {
    pub include_proxy_blobs: Option<bool>,
}

/// Download a backup of the entire system as a tar.gz archive.
pub async fn download_backup(
    State(state): State<AppState>,
    auth: AuthUser,
    Query(query): Query<BackupQuery>,
) -> impl IntoResponse {
    if let Err(e) = auth.require_admin() {
        return e.into_response();
    }
    let include_proxy_blobs = query.include_proxy_blobs.unwrap_or(false);

    // Create backup in temp directory
    let temp_dir = match tempfile::tempdir() {
        Ok(dir) => dir,
        Err(e) => {
            tracing::error!(error = %e, "Failed to create temp directory for backup");
            return (StatusCode::INTERNAL_SERVER_ERROR, "failed to create backup").into_response();
        }
    };

    let backup_filename = backup::backup_filename();
    let backup_path = temp_dir.path().join(&backup_filename);

    match backup::create_backup(
        &state.pool,
        &state.config.storage_root,
        &backup_path,
        include_proxy_blobs,
    )
    .await
    {
        Ok(_manifest) => {
            // Read the backup file and stream it
            let file = match tokio::fs::File::open(&backup_path).await {
                Ok(f) => f,
                Err(e) => {
                    tracing::error!(error = %e, "Failed to open backup file");
                    return (StatusCode::INTERNAL_SERVER_ERROR, "failed to read backup")
                        .into_response();
                }
            };

            let file_size = match file.metadata().await {
                Ok(m) => m.len(),
                Err(e) => {
                    tracing::error!(error = %e, "Failed to get backup file size");
                    return (StatusCode::INTERNAL_SERVER_ERROR, "failed to read backup")
                        .into_response();
                }
            };

            let stream = file_stream(file);
            let body = Body::from_stream(stream);

            let mut response_headers = HeaderMap::new();
            response_headers.insert("Content-Type", HeaderValue::from_static("application/gzip"));
            if let Ok(hv) =
                HeaderValue::from_str(&format!("attachment; filename=\"{}\"", backup_filename))
            {
                response_headers.insert("Content-Disposition", hv);
            }
            if let Ok(hv) = HeaderValue::from_str(&file_size.to_string()) {
                response_headers.insert("Content-Length", hv);
            }

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
        Err(e) => {
            tracing::error!(error = %e, "Failed to create backup");
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("backup failed: {}", e),
            )
                .into_response()
        }
    }
}

#[derive(Serialize)]
pub struct RestoreResponse {
    pub success: bool,
    pub message: String,
    pub stats: backup::RestoreStats,
}

/// Restore from a backup archive.
/// Accepts the tar.gz file as the request body.
pub async fn restore_backup(
    State(state): State<AppState>,
    auth: AuthUser,
    body: axum::body::Bytes,
) -> impl IntoResponse {
    if let Err(e) = auth.require_admin() {
        return e.into_response();
    }
    if body.is_empty() {
        return (StatusCode::BAD_REQUEST, "no backup file provided").into_response();
    }

    // Write body to temp file
    let temp_dir = match tempfile::tempdir() {
        Ok(dir) => dir,
        Err(e) => {
            tracing::error!(error = %e, "Failed to create temp directory for restore");
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                "failed to process backup",
            )
                .into_response();
        }
    };

    let backup_path = temp_dir.path().join("restore.tar.gz");
    if let Err(e) = tokio::fs::write(&backup_path, &body).await {
        tracing::error!(error = %e, "Failed to write backup file");
        return (
            StatusCode::INTERNAL_SERVER_ERROR,
            "failed to process backup",
        )
            .into_response();
    }

    match backup::restore_backup(&state.pool, &state.config.storage_root, &backup_path).await {
        Ok(stats) => {
            // Reload runtime config after restore
            if let Err(e) = state.runtime_config.reload().await {
                tracing::warn!(error = %e, "Failed to reload runtime config after restore");
            }

            Json(RestoreResponse {
                success: true,
                message: "Restore completed successfully".to_string(),
                stats,
            })
            .into_response()
        }
        Err(e) => {
            tracing::error!(error = %e, "Failed to restore backup");
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("restore failed: {}", e),
            )
                .into_response()
        }
    }
}

// ========== Dev Snapshot (Dev Mode Only) ==========

#[derive(Serialize)]
pub struct DevSnapshotStatus {
    pub exists: bool,
    pub size: Option<u64>,
    #[serde(with = "time::serde::rfc3339::option")]
    pub created_at: Option<OffsetDateTime>,
    pub dev_mode: bool,
}

fn get_dev_snapshot_path(config_path: &str) -> std::path::PathBuf {
    StdPath::new(config_path)
        .parent()
        .unwrap_or(StdPath::new("/config"))
        .join("dev-snapshot.tar.gz")
}

fn is_dev_mode() -> bool {
    std::env::var("DEV_MODE")
        .ok()
        .filter(|s| s == "1")
        .is_some()
}

/// Check if dev snapshot exists and get its status.
pub async fn get_dev_snapshot_status(
    State(state): State<AppState>,
    auth: AuthUser,
) -> impl IntoResponse {
    if let Err(e) = auth.require_admin() {
        return e.into_response();
    }
    let snapshot_path = get_dev_snapshot_path(&state.config.config_path);
    let dev_mode = is_dev_mode();

    if !snapshot_path.exists() {
        return Json(DevSnapshotStatus {
            exists: false,
            size: None,
            created_at: None,
            dev_mode,
        })
        .into_response();
    }

    let metadata = match std::fs::metadata(&snapshot_path) {
        Ok(m) => m,
        Err(e) => {
            tracing::error!(error = %e, "Failed to get snapshot metadata");
            return Json(DevSnapshotStatus {
                exists: true,
                size: None,
                created_at: None,
                dev_mode,
            })
            .into_response();
        }
    };

    let created_at = metadata.modified().ok().and_then(|t| {
        let duration = t.duration_since(std::time::UNIX_EPOCH).ok()?;
        OffsetDateTime::from_unix_timestamp(duration.as_secs() as i64).ok()
    });

    Json(DevSnapshotStatus {
        exists: true,
        size: Some(metadata.len()),
        created_at,
        dev_mode,
    })
    .into_response()
}

/// Save current state as dev default snapshot.
pub async fn save_dev_snapshot(State(state): State<AppState>, auth: AuthUser) -> impl IntoResponse {
    if let Err(e) = auth.require_admin() {
        return e.into_response();
    }
    if !is_dev_mode() {
        return (StatusCode::FORBIDDEN, "dev mode not enabled").into_response();
    }

    let snapshot_path = get_dev_snapshot_path(&state.config.config_path);

    match backup::create_backup(
        &state.pool,
        &state.config.storage_root,
        &snapshot_path,
        false,
    )
    .await
    {
        Ok(manifest) => Json(serde_json::json!({
            "success": true,
            "message": "Dev snapshot saved",
            "books_count": manifest.books_count,
            "files_count": manifest.files_count
        }))
        .into_response(),
        Err(e) => {
            tracing::error!(error = %e, "Failed to save dev snapshot");
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("failed to save snapshot: {}", e),
            )
                .into_response()
        }
    }
}

/// Delete the dev snapshot.
pub async fn delete_dev_snapshot(
    State(state): State<AppState>,
    auth: AuthUser,
) -> impl IntoResponse {
    if let Err(e) = auth.require_admin() {
        return e.into_response();
    }
    if !is_dev_mode() {
        return (StatusCode::FORBIDDEN, "dev mode not enabled").into_response();
    }

    let snapshot_path = get_dev_snapshot_path(&state.config.config_path);

    if !snapshot_path.exists() {
        return (StatusCode::NOT_FOUND, "no dev snapshot exists").into_response();
    }

    match std::fs::remove_file(&snapshot_path) {
        Ok(_) => Json(serde_json::json!({
            "success": true,
            "message": "Dev snapshot deleted"
        }))
        .into_response(),
        Err(e) => {
            tracing::error!(error = %e, "Failed to delete dev snapshot");
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("failed to delete snapshot: {}", e),
            )
                .into_response()
        }
    }
}

/// Trigger comprehensive library cleanup (database + optional file cleanup).
/// POST /api/v1/admin/library-cleanup
pub async fn trigger_library_cleanup(
    State(state): State<AppState>,
    auth: AuthUser,
) -> impl IntoResponse {
    if let Err(e) = auth.require_admin() {
        return e.into_response();
    }

    match jobs::enqueue_with_event(
        &state.pool,
        state.nats.publisher(),
        &state.job_events,
        "database_cleanup",
        serde_json::json!({ "include_file_cleanup": true }),
        JobContext::new("Library cleanup (database + files)"),
        false,
    )
    .await
    {
        Ok(job_id) => Json(serde_json::json!({ "job_id": job_id })).into_response(),
        Err(e) => {
            tracing::error!(error = %e, "Failed to enqueue library cleanup");
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                "failed to enqueue cleanup",
            )
                .into_response()
        }
    }
}

/// Clear metadata cache
/// DELETE /api/v1/admin/metadata-cache
pub async fn clear_metadata_cache(
    State(state): State<AppState>,
    auth: AuthUser,
) -> impl IntoResponse {
    if let Err(e) = auth.require_admin() {
        return e.into_response();
    }
    match repositories::metadata_cache::delete_all(&state.pool).await {
        Ok(deleted) => {
            tracing::info!(deleted = %deleted, "Cleared metadata cache");
            Json(serde_json::json!({ "deleted": deleted })).into_response()
        }
        Err(e) => {
            tracing::error!(error = %e, "Failed to clear metadata cache");
            (StatusCode::INTERNAL_SERVER_ERROR, "failed to clear cache").into_response()
        }
    }
}

/// Enqueue metadata_write jobs for all owned library books with ebook files.
/// POST /api/v1/admin/metadata-write-all
pub async fn metadata_write_all(
    State(state): State<AppState>,
    auth: AuthUser,
) -> impl IntoResponse {
    if let Err(e) = auth.require_admin() {
        return e.into_response();
    }
    // Query all owned library books that have at least one ebook file
    let rows = match sqlx::query!(
        r#"
        SELECT DISTINCT b.id, b.title
        FROM books b
        JOIN book_files bf ON bf.book_id = b.id
        WHERE (EXISTS (SELECT 1 FROM book_files bf WHERE bf.book_id = b.id) OR EXISTS (SELECT 1 FROM audiobook_files af WHERE af.book_id = b.id))
        "#,
    )
    .fetch_all(&state.pool)
    .await
    {
        Ok(rows) => rows,
        Err(e) => {
            tracing::error!(error = %e, "Failed to query books for metadata write");
            return (StatusCode::INTERNAL_SERVER_ERROR, "database error").into_response();
        }
    };

    let nats = state.nats.publisher();
    let mut count = 0usize;
    for row in &rows {
        jobs::enqueue_metadata_write(&state.pool, nats, row.id, &row.title).await;
        count += 1;
    }

    tracing::info!(count, "Enqueued metadata_write jobs for all library books");
    Json(serde_json::json!({
        "message": format!("Enqueued {} metadata write jobs", count),
        "count": count
    }))
    .into_response()
}

/// Trigger orphan cleanup (images, covers, book file directories).
/// POST /api/v1/admin/orphan-cleanup
pub async fn trigger_orphan_cleanup(
    State(state): State<AppState>,
    auth: AuthUser,
) -> impl IntoResponse {
    if let Err(e) = auth.require_admin() {
        return e.into_response();
    }

    match jobs::enqueue_with_event(
        &state.pool,
        state.nats.publisher(),
        &state.job_events,
        "orphan_cleanup",
        serde_json::json!({}),
        JobContext::new("Cleaning up orphaned files"),
        false,
    )
    .await
    {
        Ok(job_id) => Json(serde_json::json!({ "job_id": job_id })).into_response(),
        Err(e) => {
            tracing::error!(error = %e, "Failed to enqueue orphan cleanup");
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                "failed to enqueue cleanup",
            )
                .into_response()
        }
    }
}
