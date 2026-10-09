use super::basic::{record_event, validate_path_segment};
use crate::AppState;
use crate::events::LibraryEvent;
use crate::jobs;
use crate::repositories::{authors, book_files, books, series};
use crate::routes::IngestAuth;
use axum::{
    Json,
    extract::{Multipart, Path, State},
    http::StatusCode,
    response::IntoResponse,
};
use serde::Deserialize;
use std::collections::HashMap;
use std::path::PathBuf;
use time::{OffsetDateTime, format_description::well_known::Rfc3339};
use tokio::fs;
use uuid::Uuid;

/// Atomic ingest metadata (matches BookUpsertRequest but parsed from multipart)
#[derive(Deserialize)]
pub struct AtomicIngestMetadata {
    pub title: String,
    pub description: Option<String>,
    pub language: Option<String>,
    pub publisher: Option<String>,
    pub pubdate: Option<String>,
    pub series_name: Option<String>,
    pub series_index: Option<f64>,
    pub authors: Option<Vec<String>>,
    pub identifiers: Option<HashMap<String, String>>,
}

/// Atomic book ingestion - single request with metadata, cover, and file.
/// Wraps all operations in a transaction for atomicity.
pub async fn atomic_ingest(
    State(state): State<AppState>,
    _auth: IngestAuth,
    Path(book_id): Path<String>,
    mut multipart: Multipart,
) -> impl IntoResponse {
    let book_id = match Uuid::parse_str(&book_id) {
        Ok(id) => id,
        Err(_) => return (StatusCode::BAD_REQUEST, "invalid book id").into_response(),
    };

    // Parse multipart fields
    let mut metadata: Option<AtomicIngestMetadata> = None;
    let mut cover_bytes: Option<Vec<u8>> = None;
    let mut file_bytes: Option<Vec<u8>> = None;
    let mut file_format: Option<String> = None;

    while let Ok(Some(field)) = multipart.next_field().await {
        let name = field.name().unwrap_or("").to_string();
        match name.as_str() {
            "metadata" => {
                let data = match field.bytes().await {
                    Ok(d) => d,
                    Err(_) => {
                        return (StatusCode::BAD_REQUEST, "failed to read metadata field")
                            .into_response();
                    }
                };
                metadata = match serde_json::from_slice(&data) {
                    Ok(m) => Some(m),
                    Err(_) => {
                        return (StatusCode::BAD_REQUEST, "invalid metadata JSON").into_response();
                    }
                };
            }
            "cover" => {
                cover_bytes = match field.bytes().await {
                    Ok(d) => Some(d.to_vec()),
                    Err(_) => {
                        return (StatusCode::BAD_REQUEST, "failed to read cover field")
                            .into_response();
                    }
                };
            }
            "file" => {
                // Get format from filename or content-type
                let filename = field.file_name().map(|s| s.to_string());
                let content_type = field.content_type().map(|s| s.to_string());

                // Try to extract format from filename extension (only if filename contains a dot)
                file_format = filename
                    .as_ref()
                    .filter(|f| f.contains('.'))
                    .and_then(|f| f.rsplit('.').next())
                    .map(|ext| ext.to_uppercase())
                    .or_else(|| {
                        // Fall back to content type
                        content_type.and_then(|ct| {
                            if ct.contains("epub") {
                                Some("KEPUB".to_string())
                            } else {
                                None
                            }
                        })
                    });

                file_bytes = match field.bytes().await {
                    Ok(d) => Some(d.to_vec()),
                    Err(_) => {
                        return (StatusCode::BAD_REQUEST, "failed to read file field")
                            .into_response();
                    }
                };
            }
            "format" => {
                // Explicit format field
                if let Ok(d) = field.bytes().await
                    && let Ok(s) = String::from_utf8(d.to_vec())
                {
                    file_format = Some(s.trim().to_uppercase());
                }
            }
            _ => {}
        }
    }

    // Validate required fields
    let metadata = match metadata {
        Some(m) => m,
        None => return (StatusCode::BAD_REQUEST, "metadata field required").into_response(),
    };
    let file_bytes = match file_bytes {
        Some(f) => f,
        None => return (StatusCode::BAD_REQUEST, "file field required").into_response(),
    };
    let file_format = match file_format {
        Some(f) => f,
        None => return (StatusCode::BAD_REQUEST, "file format required").into_response(),
    };

    // Validate file_format to prevent path traversal
    if let Err(e) = validate_path_segment(&file_format) {
        return (StatusCode::BAD_REQUEST, e).into_response();
    }

    // Parse pubdate
    let pubdate = match &metadata.pubdate {
        Some(value) => match OffsetDateTime::parse(value, &Rfc3339) {
            Ok(dt) => Some(dt),
            Err(_) => return (StatusCode::BAD_REQUEST, "invalid pubdate").into_response(),
        },
        None => None,
    };

    // Start transaction
    let mut tx = match state.pool.begin().await {
        Ok(tx) => tx,
        Err(e) => {
            tracing::error!(error = %e, "failed to begin transaction for atomic ingest");
            return (StatusCode::INTERNAL_SERVER_ERROR, "database error").into_response();
        }
    };

    // Track files we write so we can clean up on failure
    let mut written_files: Vec<PathBuf> = Vec::new();

    // Insert/update book
    let series_name = series::normalize_name(metadata.series_name);
    let identifiers_json = metadata
        .identifiers
        .as_ref()
        .and_then(|ids| serde_json::to_value(ids).ok());

    let input = books::BookInput {
        id: Some(book_id),
        title: metadata.title.clone(),
        description: metadata.description.clone(),
        language: metadata.language.clone(),
        publisher: metadata.publisher.clone(),
        pubdate,
        identifiers: identifiers_json,
        ..Default::default()
    };

    if let Err(e) = books::upsert_library_book(&mut *tx, input).await {
        tracing::error!(error = %e, book_id = %book_id, "failed to upsert book during atomic ingest");
        return (StatusCode::INTERNAL_SERVER_ERROR, "failed to upsert book").into_response();
    }

    // Handle authors
    let mut author_ids = Vec::new();
    if let Some(author_list) = metadata.authors {
        if let Err(e) = authors::clear_book_authors(&mut *tx, book_id).await {
            tracing::error!(error = %e, book_id = %book_id, "failed to delete existing book_authors during atomic ingest");
        }

        for author_name in author_list {
            let author_name = author_name.trim();
            if author_name.is_empty() {
                continue;
            }

            match authors::get_or_create_by_name(&state.pool, author_name).await {
                Ok(author_id) => {
                    if let Err(e) = authors::link_book(&mut *tx, book_id, author_id).await {
                        tracing::error!(error = %e, book_id = %book_id, author_id = %author_id, "failed to insert book_author during atomic ingest");
                    }
                    author_ids.push(author_id);
                }
                Err(e) => {
                    tracing::error!(error = %e, author_name = %author_name, "failed to get or create author during atomic ingest");
                }
            }
        }
    }

    // Resolve series (create if needed) — actual junction link happens after tx commits
    let mut series_id_for_index = None;
    if let Some(ref name) = series_name {
        match series::get_or_create_by_name(&state.pool, name).await {
            Ok(series_id) => {
                series_id_for_index = Some(series_id);
            }
            Err(e) => {
                tracing::error!(error = %e, book_id = %book_id, "failed to get or create series during atomic ingest");
            }
        }
    }

    // Write cover file (if provided)
    if let Some(cover) = cover_bytes {
        let mut cover_path = PathBuf::from(&state.config.storage_root);
        cover_path.push("covers");
        if let Err(e) = fs::create_dir_all(&cover_path).await {
            tracing::error!(error = %e, "failed to create covers directory");
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                "failed to create directory",
            )
                .into_response();
        }
        cover_path.push(format!("{}.jpg", book_id));

        if let Err(e) = fs::write(&cover_path, cover).await {
            tracing::error!(error = %e, "failed to write cover during atomic ingest");
            // Clean up and return error
            for path in &written_files {
                let _ = fs::remove_file(path).await;
            }
            return (StatusCode::INTERNAL_SERVER_ERROR, "failed to write cover").into_response();
        }
        written_files.push(cover_path);
    }

    // Write book file
    let mut book_path = PathBuf::from(&state.config.storage_root);
    book_path.push("books");
    book_path.push(book_id.to_string());
    if let Err(e) = fs::create_dir_all(&book_path).await {
        tracing::error!(error = %e, "failed to create book directory");
        for path in &written_files {
            let _ = fs::remove_file(path).await;
        }
        return (
            StatusCode::INTERNAL_SERVER_ERROR,
            "failed to create directory",
        )
            .into_response();
    }
    book_path.push(file_format.to_lowercase());

    let file_size = file_bytes.len() as i64;
    if let Err(e) = fs::write(&book_path, &file_bytes).await {
        tracing::error!(error = %e, "failed to write book file during atomic ingest");
        for path in &written_files {
            let _ = fs::remove_file(path).await;
        }
        return (StatusCode::INTERNAL_SERVER_ERROR, "failed to write file").into_response();
    }
    written_files.push(book_path.clone());

    // Insert book_files record
    let input = book_files::BookFileInput {
        book_id,
        format: file_format.clone(),
        size_bytes: file_size,
        file_path: book_path.to_string_lossy().to_string(),
        source_type: Some("calibre_plugin".to_string()),
        ..Default::default()
    };
    if let Err(e) = book_files::upsert(&mut *tx, input).await {
        tracing::error!(error = %e, book_id = %book_id, format = %file_format, "failed to insert book_files record during atomic ingest");
        for path in &written_files {
            let _ = fs::remove_file(path).await;
        }
        return (StatusCode::INTERNAL_SERVER_ERROR, "failed to record file").into_response();
    }

    // Commit transaction
    if let Err(e) = tx.commit().await {
        tracing::error!(error = %e, book_id = %book_id, "failed to commit atomic ingest transaction");
        for path in &written_files {
            let _ = fs::remove_file(path).await;
        }
        return (StatusCode::INTERNAL_SERVER_ERROR, "failed to commit").into_response();
    }

    // Link book to series via junction table (after tx commit so book FK exists)
    if let Some(series_id) = series_id_for_index
        && let Err(e) =
            series::set_book_series(&state.pool, book_id, Some(series_id), metadata.series_index)
                .await
    {
        tracing::error!(error = %e, book_id = %book_id, "failed to update book series info during atomic ingest");
    }

    // Create per-device entitlements for auto-sync devices
    crate::auto_sync::create_auto_sync_entitlements(&state.pool, state.nats.publisher(), book_id)
        .await;

    // Enqueue metadata_write to embed DB metadata into the file
    jobs::enqueue_metadata_write(
        &state.pool,
        state.nats.publisher(),
        book_id,
        &metadata.title,
    )
    .await;

    // Success - record events
    record_event(&state, "ingest.atomic", Some(book_id), "ok", None, None).await;
    crate::routes::library::helpers::record_activity(
        &state.pool,
        "imported",
        Some("book"),
        Some(book_id),
        &metadata.title,
        None,
        Some(&format!("/api/v1/library/books/{}/cover", book_id)),
        Some(&format!("/library/books/{}", book_id)),
        None,
    )
    .await;

    // Index book, authors, and series for search
    let nats = state.nats.publisher();
    jobs::enqueue_book_index(&state.pool, nats, book_id).await;
    for author_id in &author_ids {
        jobs::enqueue_author_index(&state.pool, nats, *author_id).await;
    }
    if let Some(series_id) = series_id_for_index {
        jobs::enqueue_series_index(&state.pool, nats, series_id).await;
    }

    let _ = state
        .library_events
        .send(LibraryEvent::library_updated(Some(book_id)));

    // Clean up Kobload watch pattern if this book was wanted
    crate::wanted::on_book_acquired(&state.runtime_config, &state.library_events, book_id).await;

    (
        StatusCode::CREATED,
        Json(serde_json::json!({
            "book_id": book_id
        })),
    )
        .into_response()
}
