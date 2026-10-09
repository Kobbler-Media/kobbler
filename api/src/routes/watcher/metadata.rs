//! Metadata extraction and cover operations.

use crate::AppState;
use crate::repositories::ingest_queue;
use crate::routes::AuthUser;
use crate::watcher::IngestQueueItem;
use axum::{
    Json as AxumJson,
    extract::{Path, State},
    http::{HeaderMap, StatusCode},
    response::IntoResponse,
};
use serde::Serialize;
use time::OffsetDateTime;
use uuid::Uuid;

// ============================================================================
// Handlers
// ============================================================================

/// Get the cover image from an ingest queue item (extracted from ebook).
pub async fn get_ingest_queue_cover(
    State(state): State<AppState>,
    _auth: AuthUser,
    Path(id): Path<Uuid>,
) -> impl IntoResponse {
    match ingest_queue::get_cover(&state.pool, id).await {
        Ok(Some(cover)) => {
            let mut headers = HeaderMap::new();
            headers.insert(
                "Content-Type",
                crate::images::detect_content_type(&cover).parse().unwrap(),
            );
            headers.insert("Cache-Control", "public, max-age=86400".parse().unwrap());
            (headers, cover).into_response()
        }
        Ok(None) => (StatusCode::NOT_FOUND, "queue item or cover not found").into_response(),
        Err(e) => {
            tracing::error!("Failed to get ingest queue cover: {}", e);
            (StatusCode::INTERNAL_SERVER_ERROR, "database error").into_response()
        }
    }
}

/// Extract metadata from the source file using ebook-meta CLI.
pub async fn extract_metadata(
    State(state): State<AppState>,
    _auth: AuthUser,
    Path(id): Path<Uuid>,
) -> impl IntoResponse {
    use crate::convert::EbookMeta;
    use std::path::Path as StdPath;

    // Get the queue item
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

    let config = state.runtime_config.get().await;
    let ebook_meta = EbookMeta::with_config_path(config.ebook_meta_path);
    let file_path = StdPath::new(&item.file_path);

    // Extract metadata
    match ebook_meta.extract(file_path).await {
        Ok(metadata) => {
            // Update the queue item with extracted metadata
            let authors: Option<Vec<String>> = if metadata.authors.is_empty() {
                None
            } else {
                Some(metadata.authors)
            };

            // Parse pubdate string to OffsetDateTime
            let pubdate: Option<OffsetDateTime> = metadata.pubdate.as_ref().and_then(|s| {
                time::OffsetDateTime::parse(s, &time::format_description::well_known::Rfc3339)
                    .ok()
                    .or_else(|| {
                        // Try parsing just a date (YYYY-MM-DD)
                        time::Date::parse(
                            s,
                            &time::format_description::parse_borrowed::<2>("[year]-[month]-[day]")
                                .ok()?,
                        )
                        .ok()
                        .map(|d| d.with_hms(0, 0, 0).unwrap().assume_utc())
                    })
            });

            // Update book metadata fields
            {
                use crate::repositories::{book_ingest_source, books};

                let identifiers = metadata
                    .isbn
                    .as_ref()
                    .map(|isbn| serde_json::json!({"isbn": isbn}));

                let update = books::BookUpdate {
                    title: metadata.title.clone(),
                    description: metadata.description.clone(),
                    language: metadata.language.clone(),
                    publisher: metadata.publisher.clone(),
                    pubdate,
                    identifiers,
                    ..Default::default()
                };

                if let Err(e) = books::update(&state.pool, id, update).await {
                    tracing::error!("Failed to update queue item with metadata: {}", e);
                }

                // Update pending authors/series in ingest source
                let pending_authors = authors.as_ref().map(|a| {
                    serde_json::json!(
                        a.iter()
                            .map(|name| serde_json::json!({"name": name}))
                            .collect::<Vec<_>>()
                    )
                });
                let pending_series = metadata
                    .series
                    .as_ref()
                    .map(|s| serde_json::json!({"name": s}));

                if pending_authors.is_some() || pending_series.is_some() {
                    let _ = book_ingest_source::update(
                        &state.pool,
                        id,
                        book_ingest_source::BookIngestSourceUpdate {
                            pending_authors,
                            pending_series,
                            pending_series_index: metadata.series_index,
                            ..Default::default()
                        },
                    )
                    .await;
                }
            }

            // Also extract and save cover image
            let temp_dir = std::env::temp_dir().join(format!("kobbler-cover-{}", id));
            if let Ok(cover_path) = ebook_meta.extract_cover(file_path, &temp_dir).await {
                if let Ok(cover_bytes) = tokio::fs::read(&cover_path).await {
                    let _ = ingest_queue::update_cover(&state.pool, id, &cover_bytes).await;
                    let _ = tokio::fs::remove_file(&cover_path).await;
                }
                let _ = tokio::fs::remove_dir_all(&temp_dir).await;
            }

            #[derive(Serialize)]
            struct ExtractResponse {
                title: Option<String>,
                authors: Vec<String>,
                isbn: Option<String>,
                description: Option<String>,
                publisher: Option<String>,
                series: Option<String>,
                series_index: Option<f64>,
                language: Option<String>,
                pubdate: Option<String>,
            }

            AxumJson(ExtractResponse {
                title: metadata.title,
                authors: authors.unwrap_or_default(),
                isbn: metadata.isbn,
                description: metadata.description,
                publisher: metadata.publisher,
                series: metadata.series,
                series_index: metadata.series_index,
                language: metadata.language,
                pubdate: metadata.pubdate,
            })
            .into_response()
        }
        Err(e) => {
            tracing::error!("Failed to extract metadata: {}", e);
            (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response()
        }
    }
}
