use crate::AppState;
use crate::events::LibraryEvent;
use crate::jobs;
use crate::repositories::{
    authors, book_files, books, entitlements, genres, ingest_events, series,
};
use crate::routes::IngestAuth;
use crate::types::EntitlementScope;
use axum::{
    Json,
    body::Bytes,
    extract::{Path, State},
    http::StatusCode,
    response::IntoResponse,
};
use serde::Deserialize;
use std::collections::HashMap;
use std::path::{Component, PathBuf};
use time::{OffsetDateTime, format_description::well_known::Rfc3339};
use tokio::fs;
use uuid::Uuid;

/// Validates that a path segment doesn't allow directory traversal.
pub(crate) fn validate_path_segment(segment: &str) -> Result<(), &'static str> {
    for component in std::path::Path::new(segment).components() {
        match component {
            Component::ParentDir => return Err("path traversal not allowed"),
            Component::RootDir => return Err("absolute paths not allowed"),
            Component::Prefix(_) => return Err("path prefix not allowed"),
            _ => {}
        }
    }
    Ok(())
}

#[derive(Deserialize)]
pub struct BookUpsertRequest {
    pub title: String,
    pub description: Option<String>,
    pub language: Option<String>,
    pub publisher: Option<String>,
    pub pubdate: Option<String>,
    pub series_name: Option<String>,
    pub series_index: Option<f64>,
    pub authors: Option<Vec<String>>,
    pub identifiers: Option<HashMap<String, String>>,
    pub genres: Option<Vec<String>>,
    pub page_count: Option<i32>,
    pub rating: Option<f64>,
    pub rating_count: Option<i32>,
}

async fn fetch_author_ids(state: &AppState, book_id: Uuid) -> Vec<Uuid> {
    authors::get_book_author_ids(&state.pool, book_id)
        .await
        .unwrap_or_default()
}

pub(crate) async fn record_event(
    state: &AppState,
    source: &str,
    book_id: Option<Uuid>,
    status: &str,
    message: Option<String>,
    device_id: Option<Uuid>,
) {
    if let Err(e) = ingest_events::create(
        &state.pool,
        source,
        book_id,
        status,
        message.as_deref(),
        device_id,
    )
    .await
    {
        tracing::error!(error = %e, source = source, "failed to record ingest event");
    }
}

pub async fn upsert_book(
    State(state): State<AppState>,
    _auth: IngestAuth,
    Path(book_id): Path<String>,
    Json(payload): Json<BookUpsertRequest>,
) -> impl IntoResponse {
    let book_id = match Uuid::parse_str(&book_id) {
        Ok(id) => id,
        Err(_) => return (StatusCode::BAD_REQUEST, "invalid book id").into_response(),
    };

    let pubdate = match &payload.pubdate {
        Some(value) => match OffsetDateTime::parse(value, &Rfc3339) {
            Ok(dt) => Some(dt),
            Err(_) => return (StatusCode::BAD_REQUEST, "invalid pubdate").into_response(),
        },
        None => None,
    };

    let series_name = series::normalize_name(payload.series_name);
    let identifiers_json = payload
        .identifiers
        .as_ref()
        .and_then(|ids| serde_json::to_value(ids).ok());

    let input = books::BookInput {
        id: Some(book_id),
        title: payload.title.clone(),
        description: payload.description.clone(),
        language: payload.language.clone(),
        publisher: payload.publisher.clone(),
        pubdate,
        identifiers: identifiers_json,
        page_count: payload.page_count,
        rating: payload.rating,
        rating_count: payload.rating_count,
        ..Default::default()
    };

    if let Err(e) = books::upsert_library_book(&state.pool, input).await {
        tracing::error!(error = %e, book_id = %book_id, "failed to upsert book during ingest");
        return (StatusCode::INTERNAL_SERVER_ERROR, "failed to upsert book").into_response();
    }

    // Handle genres if provided
    if let Some(genre_list) = &payload.genres
        && let Err(e) = genres::set_book_genres(&state.pool, book_id, genre_list).await
    {
        tracing::error!(error = %e, book_id = %book_id, "failed to set book genres during ingest");
    }

    let mut author_ids = Vec::new();
    if let Some(author_list) = payload.authors {
        if let Err(e) = authors::clear_book_authors(&state.pool, book_id).await {
            tracing::error!(error = %e, book_id = %book_id, "failed to delete existing book_authors during ingest");
        }

        for author_name in author_list {
            let author_name = author_name.trim();
            if author_name.is_empty() {
                continue;
            }

            match authors::get_or_create_by_name(&state.pool, author_name).await {
                Ok(author_id) => {
                    if let Err(e) = authors::link_book(&state.pool, book_id, author_id).await {
                        tracing::error!(error = %e, book_id = %book_id, author_id = %author_id, "failed to insert book_author during ingest");
                    }
                    author_ids.push(author_id);
                }
                Err(e) => {
                    tracing::error!(error = %e, author_name = %author_name, "failed to get or create author during ingest");
                }
            }
        }
    } else {
        author_ids = fetch_author_ids(&state, book_id).await;
    }

    // Handle series assignment
    let mut series_id_for_index = None;
    if let Some(ref name) = series_name {
        match series::get_or_create_with_author_overlap(&state.pool, name, &author_ids, None).await
        {
            Ok(Some(series_id)) => {
                series_id_for_index = Some(series_id);
                if let Err(e) = series::set_book_series(
                    &state.pool,
                    book_id,
                    Some(series_id),
                    payload.series_index,
                )
                .await
                {
                    tracing::error!(error = %e, book_id = %book_id, "failed to update book series info during ingest");
                }
            }
            Ok(None) => {}
            Err(e) => {
                tracing::error!(error = %e, book_id = %book_id, "failed to get or create series during ingest");
            }
        }
    }

    record_event(&state, "ingest.book", Some(book_id), "ok", None, None).await;
    crate::routes::library::helpers::record_activity_for_entity(
        &state.pool,
        "imported",
        book_id,
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

    StatusCode::NO_CONTENT.into_response()
}

pub async fn upload_cover(
    State(state): State<AppState>,
    _auth: IngestAuth,
    Path(book_id): Path<String>,
    body: Bytes,
) -> impl IntoResponse {
    let book_id = match Uuid::parse_str(&book_id) {
        Ok(id) => id,
        Err(_) => return (StatusCode::BAD_REQUEST, "invalid book id").into_response(),
    };

    let mut path = PathBuf::from(&state.config.storage_root);
    path.push("covers");
    let _ = fs::create_dir_all(&path).await;
    path.push(format!("{}.jpg", book_id));

    if fs::write(&path, body).await.is_err() {
        return (StatusCode::INTERNAL_SERVER_ERROR, "failed to write cover").into_response();
    }

    record_event(&state, "ingest.cover", Some(book_id), "ok", None, None).await;
    let _ = state
        .library_events
        .send(LibraryEvent::library_updated(Some(book_id)));
    StatusCode::NO_CONTENT.into_response()
}

pub async fn upload_file(
    State(state): State<AppState>,
    _auth: IngestAuth,
    Path((book_id, format)): Path<(String, String)>,
    body: Bytes,
) -> impl IntoResponse {
    let book_id = match Uuid::parse_str(&book_id) {
        Ok(id) => id,
        Err(_) => return (StatusCode::BAD_REQUEST, "invalid book id").into_response(),
    };

    // Validate format to prevent path traversal
    if let Err(e) = validate_path_segment(&format) {
        return (StatusCode::BAD_REQUEST, e).into_response();
    }

    let mut path = PathBuf::from(&state.config.storage_root);
    path.push("books");
    path.push(book_id.to_string());
    let _ = fs::create_dir_all(&path).await;
    path.push(format.to_lowercase());

    if fs::write(&path, &body).await.is_err() {
        return (StatusCode::INTERNAL_SERVER_ERROR, "failed to write file").into_response();
    }

    let size_bytes = body.len() as i64;
    let input = book_files::BookFileInput {
        book_id,
        format: format.to_uppercase(),
        size_bytes,
        file_path: path.to_string_lossy().to_string(),
        source_type: Some("calibre_plugin".to_string()),
        ..Default::default()
    };
    if let Err(e) = book_files::upsert(&state.pool, input).await {
        tracing::error!(error = %e, book_id = %book_id, format = %format, "failed to insert book_files record during ingest");
        return (StatusCode::INTERNAL_SERVER_ERROR, "failed to record file").into_response();
    }

    // Enqueue metadata_write to embed DB metadata into the file
    if let Ok(Some(book)) = books::get_title(&state.pool, book_id).await {
        jobs::enqueue_metadata_write(&state.pool, state.nats.publisher(), book_id, &book).await;
    }

    record_event(
        &state,
        "ingest.file",
        Some(book_id),
        "ok",
        Some(format.to_uppercase()),
        None,
    )
    .await;
    let _ = state
        .library_events
        .send(LibraryEvent::library_updated(Some(book_id)));
    StatusCode::NO_CONTENT.into_response()
}

#[derive(Deserialize)]
pub struct EntitlementRequest {
    pub book_id: Uuid,
    pub is_removed: Option<bool>,
    /// Scope type: "ALL_USERS", "USER", or "DEVICE"
    pub scope_type: Option<String>,
    /// UUID of user or device (required for USER/DEVICE scope)
    pub scope_id: Option<Uuid>,
}

pub async fn create_entitlement(
    State(state): State<AppState>,
    _auth: IngestAuth,
    Json(payload): Json<EntitlementRequest>,
) -> impl IntoResponse {
    // Parse and validate scope_type
    let scope_type: EntitlementScope = match payload.scope_type.as_deref() {
        Some(s) => match s.parse() {
            Ok(scope) => scope,
            Err(_) => return (StatusCode::BAD_REQUEST, "invalid scope_type").into_response(),
        },
        None => EntitlementScope::AllUsers,
    };

    // Validate scope_id is provided for USER/DEVICE scopes
    if scope_type != EntitlementScope::AllUsers && payload.scope_id.is_none() {
        return (
            StatusCode::BAD_REQUEST,
            "scope_id required for USER/DEVICE scope",
        )
            .into_response();
    }

    if let Err(e) =
        entitlements::create(&state.pool, payload.book_id, scope_type, payload.scope_id).await
    {
        tracing::error!(error = %e, book_id = %payload.book_id, "failed to create entitlement");
        return (StatusCode::BAD_REQUEST, "failed to create entitlement").into_response();
    }

    record_event(
        &state,
        "ingest.entitlement",
        Some(payload.book_id),
        "ok",
        None,
        None,
    )
    .await;
    let _ = state
        .library_events
        .send(LibraryEvent::library_updated(Some(payload.book_id)));
    StatusCode::CREATED.into_response()
}
