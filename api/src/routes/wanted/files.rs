//! File-related handlers for wanted books (files, ingest, skip).

use axum::{
    Json,
    extract::{Path, State},
    http::StatusCode,
    response::IntoResponse,
};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::AppState;
use crate::repositories::{books, ingest_queue, wanted_attempts};
use crate::routes::AuthUser;
use crate::routes::helpers::build_kobload_client;
use crate::routes::ingest::KobloadFileInfo;

/// GET /api/v1/admin/wanted/{id}/files
pub async fn get_wanted_files(
    State(state): State<AppState>,
    auth: AuthUser,
    Path(id): Path<Uuid>,
) -> impl IntoResponse {
    if let Err(e) = auth.require_contributor() {
        return e.into_response();
    }
    // Get the latest attempt with files
    let attempt = match wanted_attempts::get_latest_with_files(&state.pool, id).await {
        Ok(Some(row)) => row,
        Ok(None) => {
            return (
                StatusCode::NOT_FOUND,
                "no download attempt with files found",
            )
                .into_response();
        }
        Err(e) => {
            tracing::error!(error = %e, "Failed to fetch attempt");
            return (StatusCode::INTERNAL_SERVER_ERROR, "database error").into_response();
        }
    };

    let (_id, external_id, files_json, _status) = attempt;

    let files: Vec<KobloadFileInfo> = files_json
        .and_then(|v| serde_json::from_value(v).ok())
        .unwrap_or_default();

    #[derive(Debug, Serialize)]
    struct WantedFilesResponse {
        download_id: Option<String>,
        files: Vec<KobloadFileInfo>,
    }

    Json(WantedFilesResponse {
        download_id: external_id,
        files,
    })
    .into_response()
}

#[derive(Debug, Deserialize)]
pub struct IngestFilesRequest {
    pub download_id: String,
    pub files: Vec<FileIngestMetadata>,
}

#[derive(Debug, Deserialize)]
pub struct FileIngestMetadata {
    pub file_name: String,
    #[serde(default)]
    pub skip: bool,
}

/// POST /api/v1/admin/wanted/{id}/ingest-files
pub async fn ingest_selected_files(
    State(state): State<AppState>,
    auth: AuthUser,
    Path(id): Path<Uuid>,
    Json(payload): Json<IngestFilesRequest>,
) -> impl IntoResponse {
    if let Err(e) = auth.require_contributor() {
        return e.into_response();
    }
    let runtime = state.runtime_config.get().await;
    let client = match build_kobload_client(&runtime) {
        Ok(client) => client,
        Err((status, message)) => return (status, message).into_response(),
    };

    // Get book metadata for ingest
    let book_meta = match books::get_wanted_metadata(&state.pool, id).await {
        Ok(Some(meta)) => meta,
        Ok(None) => return (StatusCode::NOT_FOUND, "book not found").into_response(),
        Err(e) => {
            tracing::error!(error = %e, "Failed to fetch book");
            return (StatusCode::INTERNAL_SERVER_ERROR, "database error").into_response();
        }
    };

    let title = book_meta.title;
    let author = book_meta.author;
    let series = book_meta.series_name;
    let series_index = book_meta.series_index;
    let identifiers = book_meta.identifiers;

    let dest_dir = format!("{}/downloads", state.config.storage_root);

    // Download files from Kobload
    let download_id = match Uuid::parse_str(&payload.download_id) {
        Ok(id) => id,
        Err(_) => return (StatusCode::BAD_REQUEST, "invalid download_id").into_response(),
    };

    let files_to_ingest: Vec<_> = payload
        .files
        .iter()
        .filter(|f| !f.skip)
        .map(|f| f.file_name.clone())
        .collect();

    if files_to_ingest.is_empty() {
        // Mark as skipped if no files selected
        let _ = wanted_attempts::update_status_with_message_by_book(
            &state.pool,
            id,
            &payload.download_id,
            "failed",
            Some("All files skipped"),
        )
        .await;
        return (StatusCode::BAD_REQUEST, "no files selected for ingest").into_response();
    }

    let mut ingested_count = 0;
    let mut errors = Vec::new();

    for file_name in &files_to_ingest {
        match client.download_file(download_id, file_name).await {
            Ok(content) => {
                let dest_path = format!("{}/{}", dest_dir, file_name);
                if let Err(e) = tokio::fs::write(&dest_path, &content).await {
                    errors.push(format!("Failed to write {}: {}", file_name, e));
                    continue;
                }

                // Create ingest queue entry
                let ext = std::path::Path::new(file_name)
                    .extension()
                    .and_then(|e| e.to_str())
                    .unwrap_or("")
                    .to_lowercase();

                let provider = identifiers
                    .get("hardcover")
                    .map(|_| "hardcover")
                    .unwrap_or("manual");

                let metadata = serde_json::json!({
                    "book_id": id,
                    "provider": provider,
                    "title": title,
                    "author": author,
                    "series": series,
                    "series_index": series_index,
                });

                // Build pending_authors JSONB
                let pending_authors = author
                    .as_ref()
                    .filter(|a| !a.is_empty())
                    .map(|a| serde_json::json!([{"name": a}]));

                let _ = ingest_queue::create(
                    &state.pool,
                    ingest_queue::IngestQueueInput {
                        file_path: dest_path.clone(),
                        file_name: file_name.clone(),
                        file_size: content.len() as i64,
                        source_format: ext,
                        title: Some(title.clone()),
                        pending_authors,
                        source_metadata: Some(metadata),
                        status: "pending".to_string(),
                        ..Default::default()
                    },
                )
                .await;

                ingested_count += 1;
            }
            Err(e) => {
                errors.push(format!("Failed to download {}: {}", file_name, e));
            }
        }
    }

    // Update attempt status
    let new_status = if ingested_count > 0 {
        "completed"
    } else {
        "failed"
    };
    let message = if errors.is_empty() {
        None
    } else {
        Some(errors.join("; "))
    };

    let _ = wanted_attempts::update_status_with_message_by_book(
        &state.pool,
        id,
        &payload.download_id,
        new_status,
        message.as_deref(),
    )
    .await;

    if ingested_count > 0 {
        Json(serde_json::json!({
            "success": true,
            "ingested": ingested_count,
            "errors": errors,
        }))
        .into_response()
    } else {
        (
            StatusCode::BAD_GATEWAY,
            Json(serde_json::json!({
                "success": false,
                "message": errors.join("; "),
            })),
        )
            .into_response()
    }
}

/// POST /api/v1/admin/wanted/{id}/skip
pub async fn skip_wanted_review(
    State(state): State<AppState>,
    auth: AuthUser,
    Path(id): Path<Uuid>,
) -> impl IntoResponse {
    if let Err(e) = auth.require_contributor() {
        return e.into_response();
    }
    // Mark the latest needs_review attempt as skipped/failed
    match wanted_attempts::skip_latest_needs_review(&state.pool, id).await {
        Ok(true) => StatusCode::NO_CONTENT.into_response(),
        Ok(false) => (StatusCode::NOT_FOUND, "no review to skip").into_response(),
        Err(e) => {
            tracing::error!(error = %e, "Failed to skip review");
            (StatusCode::INTERNAL_SERVER_ERROR, "database error").into_response()
        }
    }
}
