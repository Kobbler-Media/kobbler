//! Ingest queue CRUD and approve/reject operations.

use crate::AppState;
use crate::repositories::ingest_queue;
use crate::routes::AuthUser;
use crate::routes::library::types::AuthorRef;
use crate::watcher::{
    FileMode, IngestEntitlements, IngestOptions, IngestQueueItem, IngestQueueStatus,
    process_queue_item,
};
use axum::{
    Json as AxumJson,
    extract::{Json, Path, State},
    http::StatusCode,
    response::IntoResponse,
};
use serde::{Deserialize, Serialize};
use std::path::{Path as StdPath, PathBuf};
use time::OffsetDateTime;
use tokio::fs;
use uuid::Uuid;

// ============================================================================
// Types
// ============================================================================

/// Ingest-specific metadata sidecar (file provenance, pending entities, conversion state).
#[derive(Serialize)]
pub struct IngestInfo {
    pub file_path: String,
    pub file_name: String,
    pub file_size: i64,
    pub file_hash: Option<String>,
    pub source_format: String,
    pub has_cover: bool,
    pub conversion_error: Option<String>,
    pub update_target_book_id: Option<Uuid>,
    pub pending_authors: Option<serde_json::Value>,
    pub pending_series: Option<serde_json::Value>,
    pub pending_series_index: Option<f64>,
    pub source_metadata: Option<serde_json::Value>,
}

/// Book-shaped response for ingest queue items, matching LibraryBook field names
/// with an additional `ingest_info` sidecar for ingest-specific data.
#[derive(Serialize)]
pub struct IngestBookResponse {
    pub id: Uuid,
    pub title: String,
    pub authors: Vec<AuthorRef>,
    pub description: Option<String>,
    pub publisher: Option<String>,
    #[serde(with = "time::serde::rfc3339::option")]
    pub pubdate: Option<OffsetDateTime>,
    pub series_id: Option<Uuid>,
    pub series_name: Option<String>,
    pub series_index: Option<f64>,
    pub language: Option<String>,
    pub formats: Vec<String>,
    pub synced: bool,
    pub pending_sync: bool,
    #[serde(with = "time::serde::rfc3339")]
    pub created_at: OffsetDateTime,
    #[serde(with = "time::serde::rfc3339")]
    pub updated_at: OffsetDateTime,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cover_url: Option<String>,
    pub identifiers: serde_json::Value,
    pub is_in_library: Option<bool>,
    pub ingest_status: Option<String>,
    pub page_count: Option<i32>,
    pub rating: Option<f64>,
    #[serde(default)]
    pub genres: Vec<String>,
    pub ingest_info: IngestInfo,
}

impl From<IngestQueueItem> for IngestBookResponse {
    fn from(item: IngestQueueItem) -> Self {
        // Build authors: merge linked_authors (real UUIDs) with pending_authors names (nil UUID)
        let mut authors: Vec<AuthorRef> = Vec::new();
        let mut seen_names: std::collections::HashSet<String> = std::collections::HashSet::new();

        // Add linked authors (real DB records)
        if let Some(ref linked) = item.linked_authors
            && let Ok(refs) = serde_json::from_value::<Vec<AuthorRef>>(linked.clone())
        {
            for author_ref in refs {
                seen_names.insert(author_ref.name.to_lowercase());
                authors.push(author_ref);
            }
        }

        // Add pending authors with null ID, dedup by name
        if let Some(ref pending) = item.pending_authors
            && let Some(arr) = pending.as_array()
        {
            for obj in arr {
                if let Some(name) = obj.get("name").and_then(|n| n.as_str())
                    && !seen_names.contains(&name.to_lowercase())
                {
                    seen_names.insert(name.to_lowercase());
                    authors.push(AuthorRef {
                        id: None,
                        name: name.to_string(),
                    });
                }
            }
        }

        let cover_url = if item.has_cover {
            Some(format!("/api/v1/admin/ingest-queue/{}/cover", item.id))
        } else {
            None
        };

        let title = item.title.clone().unwrap_or_else(|| item.file_name.clone());
        let updated_at = item.updated_at.unwrap_or(item.created_at);
        let status = item.status.clone();

        Self {
            id: item.id,
            title,
            authors,
            description: item.description.clone(),
            publisher: item.publisher.clone(),
            pubdate: item.pubdate,
            series_id: item.series_id,
            series_name: item.series_name.clone(),
            series_index: item.series_index,
            language: item.language.clone(),
            formats: vec![item.source_format.to_uppercase()],
            synced: false,
            pending_sync: false,
            created_at: item.created_at,
            updated_at,
            cover_url,
            identifiers: item.identifiers.clone(),
            is_in_library: Some(false),
            ingest_status: Some(status),
            page_count: item.page_count,
            rating: item.rating,
            genres: item.genres,
            ingest_info: IngestInfo {
                file_path: item.file_path,
                file_name: item.file_name,
                file_size: item.file_size,
                file_hash: item.file_hash,
                source_format: item.source_format,
                has_cover: item.has_cover,
                conversion_error: item.conversion_error,
                update_target_book_id: item.update_target_book_id,
                pending_authors: item.pending_authors,
                pending_series: item.pending_series,
                pending_series_index: item.pending_series_index,
                source_metadata: item.source_metadata,
            },
        }
    }
}

#[derive(Deserialize)]
pub struct ListQueueParams {
    pub status: Option<String>,
    pub search: Option<String>,
    pub limit: Option<i64>,
    pub offset: Option<i64>,
}

/// Re-export StatusCounts from repository for API responses.
pub use ingest_queue::StatusCounts;

/// A group of ingest queue items with the same base filename (different formats)
#[derive(Serialize)]
pub struct IngestQueueGroupResponse {
    /// Unique key for this group (directory|basename)
    pub group_key: String,
    /// All items in this group (different format files)
    pub items: Vec<IngestBookResponse>,
    /// Available formats in this group (e.g., ["EPUB", "MOBI"])
    pub formats: Vec<String>,
    /// ID of the primary (preferred) item
    pub primary_item_id: Uuid,
    /// Computed display status for the group
    pub display_status: String,
}

#[derive(Serialize)]
pub struct ListGroupedQueueResponse {
    pub groups: Vec<IngestQueueGroupResponse>,
    pub total_groups: i64,
    pub status_counts: StatusCounts,
}

#[derive(Serialize)]
pub struct ApproveResponse {
    pub book_id: Uuid,
}

#[derive(Deserialize, Default)]
pub struct ApproveRequest {
    /// Override the entitlement scope. Values: "all_users", "user", "device"
    /// If device_ids is provided, this is ignored and DEVICE scope is used.
    pub scope_type: Option<String>,
    /// Specific device IDs to create entitlements for. When provided,
    /// creates DEVICE scope entitlements for each device instead of ALL_USERS.
    pub device_ids: Option<Vec<Uuid>>,
}

#[derive(Deserialize)]
pub struct BatchRejectRequest {
    pub ids: Vec<Uuid>,
}

#[derive(Serialize)]
pub struct BatchRejectResponse {
    pub rejected_count: usize,
}

#[derive(Serialize)]
pub struct UploadResponse {
    pub id: Uuid,
}

// ============================================================================
// Internal Helpers
// ============================================================================

/// Compute group key from file path (directory + base filename without extension)
fn compute_group_key(file_path: &str) -> String {
    use std::path::Path;
    let path = Path::new(file_path);
    let dir = path
        .parent()
        .map(|p| p.to_string_lossy())
        .unwrap_or_default();
    let stem = path
        .file_stem()
        .map(|s| s.to_string_lossy())
        .unwrap_or_default();
    format!("{}|{}", dir, stem)
}

/// Get format priority for sorting (lower = preferred).
/// KEPUB is preferred since Kobo devices use it natively and it avoids conversion.
fn format_priority(format: &str) -> u8 {
    match format.to_uppercase().as_str() {
        "KEPUB" => 1,
        "EPUB" => 2,
        "MOBI" => 3,
        "AZW3" => 4,
        "AZW" => 5,
        "PDF" => 6,
        "CBZ" => 7,
        "CBR" => 8,
        _ => 99,
    }
}

/// Compute display status for a group (prioritize active states)
fn compute_group_status(items: &[IngestBookResponse]) -> String {
    // Priority: extracting > converting > error > pending > approved > rejected
    let status_priority = |s: &str| -> u8 {
        match s {
            "extracting" => 1,
            "converting" => 2,
            "error" => 3,
            "pending" => 4,
            "approved" => 5,
            "rejected" => 6,
            _ => 99,
        }
    };

    items
        .iter()
        .filter_map(|i| i.ingest_status.as_deref())
        .min_by_key(|s| status_priority(s))
        .map(String::from)
        .unwrap_or_else(|| "pending".to_string())
}

// ============================================================================
// Handlers
// ============================================================================

pub async fn list_ingest_queue(
    State(state): State<AppState>,
    auth: AuthUser,
    axum::extract::Query(params): axum::extract::Query<ListQueueParams>,
) -> impl IntoResponse {
    if let Err(e) = auth.require_contributor() {
        return e.into_response();
    }
    let limit = params.limit.unwrap_or(50) as usize;
    let offset = params.offset.unwrap_or(0) as usize;

    // Fetch items using repository
    let filters = ingest_queue::ListFilters {
        status: params.status,
        search: params.search,
    };
    let items_result = ingest_queue::list_with_filters(&state.pool, filters).await;

    // Fetch status counts using repository
    let status_counts = ingest_queue::get_status_counts(&state.pool)
        .await
        .unwrap_or_default();

    match items_result {
        Ok(items) => {
            // Convert to response items
            let response_items: Vec<IngestBookResponse> =
                items.into_iter().map(IngestBookResponse::from).collect();

            // Group items by group_key
            let mut groups_map: std::collections::HashMap<String, Vec<IngestBookResponse>> =
                std::collections::HashMap::new();

            for item in response_items {
                let group_key = compute_group_key(&item.ingest_info.file_path);
                groups_map.entry(group_key).or_default().push(item);
            }

            // Convert map to sorted vector of groups
            let mut groups: Vec<IngestQueueGroupResponse> = groups_map
                .into_iter()
                .map(|(group_key, mut items)| {
                    // Sort items by format priority (EPUB first)
                    items.sort_by_key(|i| format_priority(&i.ingest_info.source_format));

                    let formats: Vec<String> = items
                        .iter()
                        .map(|i| i.ingest_info.source_format.to_uppercase())
                        .collect();

                    let primary_item_id = items.first().map(|i| i.id).unwrap_or_default();
                    let display_status = compute_group_status(&items);

                    IngestQueueGroupResponse {
                        group_key,
                        items,
                        formats,
                        primary_item_id,
                        display_status,
                    }
                })
                .collect();

            // Sort groups by most recent item (first item's created_at since items are sorted by created_at DESC)
            groups.sort_by(|a, b| {
                let a_time = a.items.first().map(|i| i.created_at);
                let b_time = b.items.first().map(|i| i.created_at);
                b_time.cmp(&a_time) // Descending (most recent first)
            });

            let total_groups = groups.len() as i64;

            // Apply pagination to groups
            let paginated_groups: Vec<IngestQueueGroupResponse> =
                groups.into_iter().skip(offset).take(limit).collect();

            let response = ListGroupedQueueResponse {
                groups: paginated_groups,
                total_groups,
                status_counts,
            };
            AxumJson(response).into_response()
        }
        Err(e) => {
            tracing::error!("Failed to list ingest queue: {}", e);
            (StatusCode::INTERNAL_SERVER_ERROR, "database error").into_response()
        }
    }
}

pub async fn get_ingest_queue_item(
    State(state): State<AppState>,
    auth: AuthUser,
    Path(id): Path<Uuid>,
) -> impl IntoResponse {
    if let Err(e) = auth.require_contributor() {
        return e.into_response();
    }
    match ingest_queue::get_by_id_full(&state.pool, id).await {
        Ok(Some(item)) => AxumJson(IngestBookResponse::from(item)).into_response(),
        Ok(None) => (StatusCode::NOT_FOUND, "queue item not found").into_response(),
        Err(e) => {
            tracing::error!("Failed to get ingest queue item: {}", e);
            (StatusCode::INTERNAL_SERVER_ERROR, "database error").into_response()
        }
    }
}

pub async fn delete_ingest_queue_item(
    State(state): State<AppState>,
    auth: AuthUser,
    Path(id): Path<Uuid>,
) -> impl IntoResponse {
    if let Err(e) = auth.require_contributor() {
        return e.into_response();
    }
    match ingest_queue::delete(&state.pool, id).await {
        Ok(false) => (StatusCode::NOT_FOUND, "queue item not found").into_response(),
        Ok(true) => StatusCode::NO_CONTENT.into_response(),
        Err(e) => {
            tracing::error!("Failed to delete ingest queue item: {}", e);
            (StatusCode::INTERNAL_SERVER_ERROR, "database error").into_response()
        }
    }
}

pub async fn approve_ingest_item(
    State(state): State<AppState>,
    auth: AuthUser,
    Path(id): Path<Uuid>,
    body: Option<Json<ApproveRequest>>,
) -> impl IntoResponse {
    if let Err(e) = auth.require_contributor() {
        return e.into_response();
    }
    let request = body.map(|b| b.0).unwrap_or_default();

    // Get the queue item (now the book itself with ingest_status set)
    let item: IngestQueueItem = match ingest_queue::get_by_id(&state.pool, id).await {
        Ok(Some(item)) => item,
        Ok(None) => {
            return (StatusCode::NOT_FOUND, "queue item not found").into_response();
        }
        Err(e) => {
            tracing::error!("Failed to get ingest queue item: {}", e);
            return (StatusCode::INTERNAL_SERVER_ERROR, "database error").into_response();
        }
    };

    if item.status() != IngestQueueStatus::Pending {
        return (StatusCode::BAD_REQUEST, "item not in pending status").into_response();
    }

    let options = IngestOptions {
        file_mode: FileMode::Copy,
        entitlements: match request.device_ids {
            Some(device_ids) => IngestEntitlements::Devices(device_ids),
            None => IngestEntitlements::AutoSync,
        },
    };
    let book_id = match process_queue_item(
        &state.pool,
        &state.config.storage_root,
        &state.library_events,
        state.nats.publisher(),
        &state.runtime_config,
        id,
        options,
    )
    .await
    {
        Ok(book_id) => book_id,
        Err(e) => {
            tracing::error!(queue_id = %id, error = %e, "Failed to approve queue item");
            return (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response();
        }
    };

    let _ = state.ingest_queue_events.send(crate::IngestQueueEvent {
        item_id: id,
        status: "approved".to_string(),
    });

    AxumJson(ApproveResponse { book_id }).into_response()
}

pub async fn reject_ingest_item(
    State(state): State<AppState>,
    auth: AuthUser,
    Path(id): Path<Uuid>,
) -> impl IntoResponse {
    if let Err(e) = auth.require_contributor() {
        return e.into_response();
    }
    match ingest_queue::reject(&state.pool, id).await {
        Ok(true) => StatusCode::NO_CONTENT.into_response(),
        Ok(false) => (StatusCode::NOT_FOUND, "queue item not found").into_response(),
        Err(e) => {
            tracing::error!("Failed to reject ingest queue item: {}", e);
            (StatusCode::INTERNAL_SERVER_ERROR, "database error").into_response()
        }
    }
}

pub async fn batch_reject_ingest_items(
    State(state): State<AppState>,
    auth: AuthUser,
    Json(payload): Json<BatchRejectRequest>,
) -> impl IntoResponse {
    if let Err(e) = auth.require_contributor() {
        return e.into_response();
    }
    if payload.ids.is_empty() {
        return AxumJson(BatchRejectResponse { rejected_count: 0 }).into_response();
    }

    match ingest_queue::batch_reject(&state.pool, &payload.ids).await {
        Ok(count) => {
            let rejected_count = count as usize;
            tracing::info!("Batch rejected {} ingest queue items", rejected_count);
            AxumJson(BatchRejectResponse { rejected_count }).into_response()
        }
        Err(e) => {
            tracing::error!("Failed to batch reject ingest queue items: {}", e);
            (StatusCode::INTERNAL_SERVER_ERROR, "database error").into_response()
        }
    }
}

/// Restore a rejected ingest queue item back to pending status.
pub async fn restore_ingest_item(
    State(state): State<AppState>,
    auth: AuthUser,
    Path(id): Path<Uuid>,
) -> impl IntoResponse {
    if let Err(e) = auth.require_contributor() {
        return e.into_response();
    }
    match ingest_queue::restore(&state.pool, id).await {
        Ok(false) => (
            StatusCode::NOT_FOUND,
            "queue item not found or not rejected",
        )
            .into_response(),
        Ok(true) => StatusCode::NO_CONTENT.into_response(),
        Err(e) => {
            tracing::error!("Failed to restore ingest queue item: {}", e);
            (StatusCode::INTERNAL_SERVER_ERROR, "database error").into_response()
        }
    }
}

/// Upload a book file directly to the ingest queue.
pub async fn upload_book(
    State(state): State<AppState>,
    auth: AuthUser,
    mut multipart: axum::extract::Multipart,
) -> impl IntoResponse {
    if let Err(e) = auth.require_contributor() {
        return e.into_response();
    }
    // Extract the book file from multipart
    let mut file_bytes: Option<Vec<u8>> = None;
    let mut file_name: Option<String> = None;

    while let Ok(Some(field)) = multipart.next_field().await {
        if field.name() == Some("file") {
            file_name = field.file_name().map(|s| s.to_string());
            match field.bytes().await {
                Ok(bytes) => file_bytes = Some(bytes.to_vec()),
                Err(e) => {
                    tracing::warn!("Failed to read file bytes: {}", e);
                    return (StatusCode::BAD_REQUEST, "failed to read file").into_response();
                }
            }
            break;
        }
    }

    let file_bytes = match file_bytes {
        Some(bytes) => bytes,
        None => return (StatusCode::BAD_REQUEST, "no file provided").into_response(),
    };

    let file_name = match file_name {
        Some(name) if !name.is_empty() => name,
        _ => return (StatusCode::BAD_REQUEST, "file name required").into_response(),
    };

    // Determine format from extension
    let extension = StdPath::new(&file_name)
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_lowercase())
        .unwrap_or_default();

    if !["epub", "kepub", "mobi", "azw", "azw3", "pdf"].contains(&extension.as_str()) {
        return (
            StatusCode::BAD_REQUEST,
            "unsupported file format - must be epub, kepub, mobi, azw, azw3, or pdf",
        )
            .into_response();
    }

    // Create uploads directory
    let uploads_dir = PathBuf::from(&state.config.storage_root).join("uploads");
    if let Err(e) = fs::create_dir_all(&uploads_dir).await {
        tracing::error!("Failed to create uploads directory: {}", e);
        return (StatusCode::INTERNAL_SERVER_ERROR, "failed to save file").into_response();
    }

    // Generate unique ID and save file
    let id = Uuid::new_v4();
    let safe_name = file_name.replace(['/', '\\', '\0'], "_");
    let stored_name = format!("{}_{}", id, safe_name);
    let file_path = uploads_dir.join(&stored_name);

    if let Err(e) = fs::write(&file_path, &file_bytes).await {
        tracing::error!("Failed to write uploaded file: {}", e);
        return (StatusCode::INTERNAL_SERVER_ERROR, "failed to save file").into_response();
    }

    let file_size = file_bytes.len() as i64;
    let file_path_str = file_path.to_string_lossy().to_string();

    // Calculate file hash for deduplication.
    //
    // SHA-256 over a fully-buffered upload is CPU-bound and scales with the file,
    // so it goes to the blocking pool: on a runtime worker a large upload stalls
    // every other in-flight request while it hashes.
    let file_hash = {
        let bytes = file_bytes;
        tokio::task::spawn_blocking(move || {
            use sha2::{Digest, Sha256};
            let mut hasher = Sha256::new();
            hasher.update(&bytes);
            hex::encode(hasher.finalize())
        })
        .await
        .expect("upload hash task panicked")
    };

    // Create ingest queue entry
    let upload_input = ingest_queue::UploadInput {
        file_path: file_path_str,
        file_name: file_name.clone(),
        file_size,
        file_hash,
        source_format: extension,
    };

    match ingest_queue::create_for_upload(&state.pool, upload_input).await {
        Ok(Some(created_id)) => {
            tracing::info!("Uploaded book {} to ingest queue", file_name);
            AxumJson(UploadResponse { id: created_id }).into_response()
        }
        Ok(None) => {
            // Duplicate file - clean up the uploaded file
            let _ = fs::remove_file(&file_path).await;
            (StatusCode::CONFLICT, "duplicate file already in queue").into_response()
        }
        Err(e) => {
            tracing::error!("Failed to create ingest queue entry: {}", e);
            let _ = fs::remove_file(&file_path).await;
            (StatusCode::INTERNAL_SERVER_ERROR, "database error").into_response()
        }
    }
}

/// Upload a cover image for an ingest queue item.
pub async fn upload_ingest_cover(
    State(state): State<AppState>,
    auth: AuthUser,
    Path(id): Path<Uuid>,
    mut multipart: axum::extract::Multipart,
) -> impl IntoResponse {
    if let Err(e) = auth.require_contributor() {
        return e.into_response();
    }
    // Extract the cover file from multipart
    let mut cover_bytes: Option<Vec<u8>> = None;

    while let Ok(Some(field)) = multipart.next_field().await {
        if field.name() == Some("cover") {
            match field.bytes().await {
                Ok(bytes) => cover_bytes = Some(bytes.to_vec()),
                Err(e) => {
                    tracing::warn!("Failed to read cover bytes: {}", e);
                    return (StatusCode::BAD_REQUEST, "failed to read cover").into_response();
                }
            }
            break;
        }
    }

    let cover_bytes = match cover_bytes {
        Some(bytes) if !bytes.is_empty() => bytes,
        _ => return (StatusCode::BAD_REQUEST, "no cover file provided").into_response(),
    };

    match ingest_queue::update_cover(&state.pool, id, &cover_bytes).await {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(e) => {
            tracing::error!("Failed to update ingest cover: {}", e);
            (StatusCode::INTERNAL_SERVER_ERROR, "database error").into_response()
        }
    }
}
