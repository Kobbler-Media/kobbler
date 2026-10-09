//! Search and download handlers for library releases via Kobload.

use std::collections::HashMap;

use axum::{
    Json as AxumJson,
    extract::{Json, Query, State},
    http::StatusCode,
    response::IntoResponse,
};
use serde::{Deserialize, Serialize};
use time::{OffsetDateTime, format_description::well_known::Rfc3339};
use uuid::Uuid;

use crate::AppState;
use crate::kobload::CreateDownloadRequest as KobloadDownloadRequest;
use crate::repositories::{authors, book_files, books, series, wanted_attempts};
use crate::routes::AuthUser;
use crate::routes::helpers::build_kobload_client;

// =============================================================================
// Types
// =============================================================================

/// Query parameters for library release search
#[derive(Debug, Deserialize)]
pub struct LibrarySearchQuery {
    pub q: String,
    /// Optional book ID to check for previously downloaded releases
    pub book_id: Option<Uuid>,
    /// Category scope for the search. `None`/"default" scopes to books;
    /// "any" drops the category filter so Other/Misc listings are visible.
    pub scope: Option<String>,
}

/// Information about a previously downloaded release
#[derive(Debug, Serialize)]
pub struct PreviousDownloadInfo {
    #[serde(with = "time::serde::rfc3339")]
    pub downloaded_at: OffsetDateTime,
    pub book_id: Uuid,
    pub book_title: String,
    pub file_id: Uuid,
}

/// Search results with optional download history
#[derive(Debug, Serialize)]
pub struct LibrarySearchResponse {
    pub results: Vec<serde_json::Value>,
}

/// Request to download a release to the library
#[derive(Debug, Deserialize)]
pub struct DownloadToLibraryRequest {
    /// The indexer release to download
    pub release: serde_json::Value,
    /// Existing book to add the file to
    pub book_id: Option<Uuid>,
    /// Create a new book with this metadata
    pub create_book: Option<CreateBookForDownload>,
}

#[derive(Debug, Deserialize)]
pub struct CreateBookForDownload {
    pub title: String,
    pub author: Option<String>,
    pub series: Option<String>,
    pub series_index: Option<f64>,
}

// =============================================================================
// Handlers
// =============================================================================

/// GET /api/v1/library/search/releases
/// Search releases via Kobload (independent of wanted system)
pub async fn search_library_releases(
    State(state): State<AppState>,
    _auth: AuthUser,
    Query(query): Query<LibrarySearchQuery>,
) -> impl IntoResponse {
    let runtime = state.runtime_config.get().await;
    let client = match build_kobload_client(&runtime) {
        Ok(client) => client,
        Err((status, message)) => return (status, message).into_response(),
    };

    // Scope to books unless the caller asked for "any", which drops the
    // category filter so releases filed under Other/Misc show up too.
    let media_type = match query.scope.as_deref() {
        Some("any") => None,
        _ => Some("ebooks".to_string()),
    };

    // Search Kobload
    let results = client.search_scoped(&query.q, None, None, media_type).await;

    let mut results = match results {
        Ok(r) => r,
        Err(err) => {
            tracing::error!(error = ?err, "Failed to search Kobload");
            return (
                StatusCode::BAD_GATEWAY,
                format!("Kobload search failed: {err}"),
            )
                .into_response();
        }
    };

    // Check for previously downloaded releases if book_id provided
    if query.book_id.is_some() {
        // Fetch all book files with their source_metadata to check for matches
        let files = match book_files::list_with_kobload_source(&state.pool).await {
            Ok(files) => files,
            Err(e) => {
                tracing::warn!(error = %e, "Failed to fetch kobload source files");
                Vec::new()
            }
        };

        // Build lookup maps for matching
        let mut hash_map: HashMap<String, (Uuid, Uuid, String, OffsetDateTime)> = HashMap::new();
        let mut title_size_map: HashMap<(String, i64), (Uuid, Uuid, String, OffsetDateTime)> =
            HashMap::new();

        for file in files {
            let file_id = file.id;
            let book_id = file.book_id;
            let book_title = file.book_title;
            let added_at = file.added_at;
            let source_metadata = file.source_metadata;

            if let Some(info_hash) = source_metadata
                .get("info_hash")
                .and_then(|v| v.as_str())
                .map(|s| s.to_lowercase())
            {
                hash_map.insert(info_hash, (file_id, book_id, book_title.clone(), added_at));
            }

            if let (Some(release_title), Some(size)) = (
                source_metadata
                    .get("release_title")
                    .and_then(|v| v.as_str()),
                source_metadata.get("size").and_then(|v| v.as_i64()),
            ) {
                title_size_map.insert(
                    (release_title.to_string(), size),
                    (file_id, book_id, book_title, added_at),
                );
            }
        }

        // Annotate results with previous download info
        for result in &mut results {
            let protocol = result
                .get("protocol")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_lowercase();

            let mut previous_download: Option<PreviousDownloadInfo> = None;

            if protocol == "torrent" {
                // Match by info_hash
                if let Some(info_hash) = result
                    .get("info_hash")
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_lowercase())
                    && let Some((file_id, book_id, book_title, downloaded_at)) =
                        hash_map.get(&info_hash)
                {
                    previous_download = Some(PreviousDownloadInfo {
                        downloaded_at: *downloaded_at,
                        book_id: *book_id,
                        book_title: book_title.clone(),
                        file_id: *file_id,
                    });
                }
            } else {
                // Match by release_title + size (usenet or other protocols)
                let title = result.get("title").and_then(|v| v.as_str());
                let size = result.get("size").and_then(|v| v.as_i64());

                if let (Some(title), Some(size)) = (title, size)
                    && let Some((file_id, book_id, book_title, downloaded_at)) =
                        title_size_map.get(&(title.to_string(), size))
                {
                    previous_download = Some(PreviousDownloadInfo {
                        downloaded_at: *downloaded_at,
                        book_id: *book_id,
                        book_title: book_title.clone(),
                        file_id: *file_id,
                    });
                }
            }

            if let Some(info) = previous_download
                && let Some(obj) = result.as_object_mut()
            {
                obj.insert(
                    "previously_downloaded".to_string(),
                    serde_json::json!({
                        "downloaded_at": info.downloaded_at.format(&Rfc3339).unwrap_or_default(),
                        "book_id": info.book_id,
                        "book_title": info.book_title,
                        "file_id": info.file_id
                    }),
                );
            }
        }
    }

    // Annotate results with rejected release info if book_id provided
    if let Some(book_id) = query.book_id {
        let rejected = books::get_rejected_releases(&state.pool, book_id)
            .await
            .unwrap_or(serde_json::json!([]));
        annotate_rejected_releases(&mut results, &rejected);
    }

    AxumJson(LibrarySearchResponse { results }).into_response()
}

/// Check if a search result matches any rejected release entry.
pub fn annotate_rejected_releases(results: &mut [serde_json::Value], rejected: &serde_json::Value) {
    let entries = match rejected.as_array() {
        Some(arr) if !arr.is_empty() => arr,
        _ => return,
    };

    for result in results.iter_mut() {
        let result_title = result.get("title").and_then(|v| v.as_str());
        let result_hash = result
            .get("info_hash")
            .and_then(|v| v.as_str())
            .map(|s| s.to_lowercase());

        for entry in entries {
            let rej_title = entry.get("release_title").and_then(|v| v.as_str());
            let rej_hash = entry
                .get("info_hash")
                .and_then(|v| v.as_str())
                .map(|s| s.to_lowercase());
            let reason = entry
                .get("reason")
                .and_then(|v| v.as_str())
                .unwrap_or("rejected");

            let matched = (rej_hash.is_some() && rej_hash == result_hash)
                || (rej_title.is_some() && rej_title == result_title);

            if matched {
                if let Some(obj) = result.as_object_mut() {
                    obj.insert(
                        "rejected_release".to_string(),
                        serde_json::json!({
                            "reason": reason,
                            "release_title": rej_title,
                        }),
                    );
                }
                break;
            }
        }
    }
}

/// POST /api/v1/library/search/download
/// Download a release to an existing book or create a new one
pub async fn download_to_library(
    State(state): State<AppState>,
    auth: AuthUser,
    Json(payload): Json<DownloadToLibraryRequest>,
) -> impl IntoResponse {
    // Determine target book ID
    let book_id = if let Some(id) = payload.book_id {
        // Verify the book exists
        match books::exists(&state.pool, id).await {
            Ok(true) => id,
            Ok(false) => return (StatusCode::NOT_FOUND, "book not found").into_response(),
            Err(e) => {
                tracing::error!(error = %e, "Failed to check book exists");
                return (StatusCode::INTERNAL_SERVER_ERROR, "database error").into_response();
            }
        }
    } else if let Some(ref create_book) = payload.create_book {
        // Create a new book
        // Find or create author
        let author_id = if let Some(ref author_name) = create_book.author {
            let author_name = author_name.trim();
            if !author_name.is_empty() {
                authors::get_or_create_by_name(&state.pool, author_name)
                    .await
                    .ok()
            } else {
                None
            }
        } else {
            None
        };

        // Find or create series
        let series_id = if let Some(ref series_name) = create_book.series {
            let author_ids = author_id.as_ref().map(|id| vec![*id]).unwrap_or_default();
            series::get_or_create_with_author_overlap(&state.pool, series_name, &author_ids, None)
                .await
                .ok()
                .flatten()
        } else {
            None
        };

        // Create the book record using repository (starts as not in library - will be set true on ingest)
        let input = books::DiscoveredBookInput {
            title: create_book.title.clone(),
            want_status: Some("wanted".to_string()),
            ..Default::default()
        };

        let new_id = match books::create_discovered_book(&state.pool, input).await {
            Ok(id) => id,
            Err(e) => {
                tracing::error!(error = %e, "Failed to create book for download");
                return (StatusCode::INTERNAL_SERVER_ERROR, "failed to create book")
                    .into_response();
            }
        };

        // Link to series via junction table
        if let Some(sid) = series_id {
            let _ = crate::repositories::book_series::add(
                &state.pool,
                new_id,
                sid,
                create_book.series_index,
                true,
            )
            .await;
        }

        // Link to author
        if let Some(aid) = author_id {
            let _ = authors::link_book(&state.pool, new_id, aid).await;
        }

        new_id
    } else {
        return (
            StatusCode::BAD_REQUEST,
            "must provide either book_id or create_book",
        )
            .into_response();
    };

    // Get book metadata using repository
    let metadata = match books::get_wanted_metadata(&state.pool, book_id).await {
        Ok(Some(m)) => m,
        Ok(None) => {
            return (StatusCode::NOT_FOUND, "book not found").into_response();
        }
        Err(e) => {
            tracing::error!(error = %e, "Failed to fetch book details");
            return (StatusCode::INTERNAL_SERVER_ERROR, "database error").into_response();
        }
    };

    let title = metadata.title;
    let author = metadata.author;
    let series = metadata.series_name;
    let series_index = metadata.series_index;
    let identifiers = metadata.identifiers;

    // Extract release info
    let release = payload.release.clone();
    let protocol = release
        .get("protocol")
        .and_then(|v| v.as_str())
        .map(|v| v.to_lowercase())
        .ok_or((StatusCode::BAD_REQUEST, "release protocol missing"));

    let protocol = match protocol {
        Ok(value) => value,
        Err((status, message)) => return (status, message).into_response(),
    };

    let download_url = release
        .get("download_url")
        .and_then(|v| v.as_str())
        .map(|v| v.to_string())
        .ok_or((StatusCode::BAD_REQUEST, "release download_url missing"));

    let download_url = match download_url {
        Ok(value) => value,
        Err((status, message)) => return (status, message).into_response(),
    };

    let runtime = state.runtime_config.get().await;

    let client = match build_kobload_client(&runtime) {
        Ok(client) => client,
        Err((status, message)) => return (status, message).into_response(),
    };

    let callback_url = match runtime.kobload_ingest_url.as_deref() {
        Some(value) if !value.trim().is_empty() => value.trim().to_string(),
        _ => {
            return (StatusCode::BAD_REQUEST, "Kobload Ingest URL not configured").into_response();
        }
    };

    let provider = identifiers
        .get("hardcover")
        .map(|_| "hardcover")
        .unwrap_or("manual");

    let metadata = serde_json::json!({
        "book_id": book_id,
        "provider": provider,
        "title": title,
        "author": author,
        "series": series,
        "series_index": series_index,
        "release_title": release.get("title").cloned().unwrap_or(serde_json::Value::Null),
        "release_file_name": release.get("file_name").cloned().unwrap_or(serde_json::Value::Null),
    });

    let name = release
        .get("file_name")
        .and_then(|v| v.as_str())
        .or_else(|| release.get("title").and_then(|v| v.as_str()))
        .unwrap_or("download")
        .to_string();

    let request = KobloadDownloadRequest {
        source: "indexer".to_string(),
        protocol: protocol.clone(),
        name,
        download_url: download_url.clone(),
        callback_url,
        auth_token: None,
        metadata,
        release: Some(release.clone()),
    };

    let (status, message, download_id) = match client.create_download(&request).await {
        Ok(response) => (response.status, None, Some(response.id)),
        Err(err) => (
            "failed".to_string(),
            Some(format!("Kobload error: {err}")),
            None,
        ),
    };

    // Create wanted_attempt record for tracking
    let mut tx = match state.pool.begin().await {
        Ok(tx) => tx,
        Err(e) => {
            tracing::error!(error = %e, "Failed to begin transaction");
            return (StatusCode::INTERNAL_SERVER_ERROR, "database error").into_response();
        }
    };

    let attempt_input = wanted_attempts::WantedAttemptInput {
        book_id: Some(book_id),
        movie_id: None,
        episode_id: None,
        source: "indexer".to_string(),
        external_id: download_id.map(|id| id.to_string()),
        download_client_id: None,
        download_url: Some(download_url.clone()),
        status: status.clone(),
        message: message.clone(),
        user_id: Some(auth.user_id),
    };

    let attempt_id = match wanted_attempts::create(&mut *tx, attempt_input).await {
        Ok(id) => id,
        Err(e) => {
            tracing::error!(error = %e, "Failed to insert wanted attempt");
            return (StatusCode::INTERNAL_SERVER_ERROR, "database error").into_response();
        }
    };

    // Update book's updated_at timestamp
    if let Err(e) = books::touch(&mut *tx, book_id).await {
        tracing::error!(error = %e, "Failed to update book timestamp");
        return (StatusCode::INTERNAL_SERVER_ERROR, "database error").into_response();
    }

    if let Err(e) = tx.commit().await {
        tracing::error!(error = %e, "Failed to commit transaction");
        return (StatusCode::INTERNAL_SERVER_ERROR, "database error").into_response();
    }

    if status == "failed" {
        let error_msg = message.unwrap_or_else(|| "kobload download failed".to_string());
        return (
            StatusCode::BAD_GATEWAY,
            AxumJson(serde_json::json!({ "message": error_msg })),
        )
            .into_response();
    }

    AxumJson(serde_json::json!({
        "book_id": book_id,
        "attempt_id": attempt_id,
        "download_id": download_id
    }))
    .into_response()
}
