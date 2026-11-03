//! Cover-related route handlers.

use axum::{
    extract::{Json, Path, Query, State},
    http::{HeaderMap, StatusCode},
    response::IntoResponse,
};
use std::path::PathBuf;
use tokio::fs;
use uuid::Uuid;

use crate::AppState;
use crate::images::{self, ImageType};
use crate::repositories::books;
use crate::routes::{AuthUser, extract_bearer, extract_session_cookie, require_admin_token};

use super::types::{CoverFromUrlRequest, CoverQuery};

/// Serve cover image for a book.
/// Supports authentication via query param, bearer token, or session cookie.
pub async fn cover(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(query): Query<CoverQuery>,
    Path(book_id): Path<String>,
) -> impl IntoResponse {
    let token = query
        .token
        .as_deref()
        .map(|value| value.trim())
        .filter(|value| !value.is_empty())
        .map(str::to_string)
        .or_else(|| extract_bearer(&headers))
        .or_else(|| extract_session_cookie(&headers));

    if let Some(token) = token {
        if require_admin_token(&state, &token).await.is_none() {
            return (StatusCode::UNAUTHORIZED, "unauthorized").into_response();
        }
    } else {
        return (StatusCode::UNAUTHORIZED, "unauthorized").into_response();
    }

    let book_id = match Uuid::parse_str(&book_id) {
        Ok(id) => id,
        Err(_) => return (StatusCode::BAD_REQUEST, "invalid book id").into_response(),
    };

    match images::load_book_cover(&state.pool, &state.config.storage_root, book_id).await {
        Some(bytes) => serve_cover_bytes(bytes),
        None => (StatusCode::NOT_FOUND, "cover not found").into_response(),
    }
}

/// Serve the extracted cover image for a book (from ebook file).
/// Uses the same auth pattern as `cover()`.
pub async fn extracted_cover(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(query): Query<CoverQuery>,
    Path(book_id): Path<String>,
) -> impl IntoResponse {
    let token = query
        .token
        .as_deref()
        .map(|value| value.trim())
        .filter(|value| !value.is_empty())
        .map(str::to_string)
        .or_else(|| extract_bearer(&headers))
        .or_else(|| extract_session_cookie(&headers));

    if let Some(token) = token {
        if require_admin_token(&state, &token).await.is_none() {
            return (StatusCode::UNAUTHORIZED, "unauthorized").into_response();
        }
    } else {
        return (StatusCode::UNAUTHORIZED, "unauthorized").into_response();
    }

    let book_id = match Uuid::parse_str(&book_id) {
        Ok(id) => id,
        Err(_) => return (StatusCode::BAD_REQUEST, "invalid book id").into_response(),
    };

    let extracted_path = images::local_image_path(
        &state.config.storage_root,
        ImageType::BookExtracted,
        book_id,
    );
    match fs::read(&extracted_path).await {
        Ok(bytes) => serve_cover_bytes(bytes),
        Err(_) => (StatusCode::NOT_FOUND, "no extracted cover").into_response(),
    }
}

/// Copy the extracted cover to the primary cover location (`covers/{book_id}.jpg`).
/// This makes it the highest-priority cover for the book.
pub async fn use_extracted_cover(
    State(state): State<AppState>,
    auth: AuthUser,
    Path(book_id): Path<Uuid>,
) -> impl IntoResponse {
    if let Err(e) = auth.require_contributor() {
        return e.into_response();
    }
    // Verify the book exists
    match books::exists(&state.pool, book_id).await {
        Ok(false) => return (StatusCode::NOT_FOUND, "book not found").into_response(),
        Err(e) => {
            tracing::error!(error = %e, "Failed to check book existence");
            return (StatusCode::INTERNAL_SERVER_ERROR, "database error").into_response();
        }
        Ok(true) => {}
    }

    // Read extracted cover
    let extracted_path = images::local_image_path(
        &state.config.storage_root,
        ImageType::BookExtracted,
        book_id,
    );
    let cover_bytes = match fs::read(&extracted_path).await {
        Ok(bytes) => bytes,
        Err(_) => return (StatusCode::NOT_FOUND, "no extracted cover").into_response(),
    };

    // Write to primary cover location
    let mut path = PathBuf::from(&state.config.storage_root);
    path.push("covers");
    if let Err(e) = fs::create_dir_all(&path).await {
        tracing::error!("Failed to create covers directory: {}", e);
        return (StatusCode::INTERNAL_SERVER_ERROR, "failed to save cover").into_response();
    }
    path.push(format!("{}.jpg", book_id));

    if let Err(e) = fs::write(&path, &cover_bytes).await {
        tracing::error!("Failed to write cover: {}", e);
        return (StatusCode::INTERNAL_SERVER_ERROR, "failed to save cover").into_response();
    }

    let _ = images::mark_cached(&state.pool, "books", book_id, "cover").await;

    StatusCode::NO_CONTENT.into_response()
}

/// Helper to serve cover bytes with correct content type
fn serve_cover_bytes(bytes: Vec<u8>) -> axum::response::Response {
    let content_type = infer::get(&bytes)
        .map(|t| t.mime_type())
        .unwrap_or("application/octet-stream");
    (StatusCode::OK, [("Content-Type", content_type)], bytes).into_response()
}

/// Download and save a cover from a URL.
/// Can be called directly without going through the HTTP handler.
pub async fn save_cover_from_url(
    client: &reqwest::Client,
    book_id: Uuid,
    url: &str,
) -> Result<(), String> {
    let response = client
        .get(url)
        .send()
        .await
        .map_err(|e| format!("Failed to fetch cover: {}", e))?;

    if !response.status().is_success() {
        return Err(format!("Cover URL returned status: {}", response.status()));
    }

    let cover_bytes = response
        .bytes()
        .await
        .map_err(|e| format!("Failed to read cover bytes: {}", e))?;

    let mut path = PathBuf::from("data");
    path.push("covers");
    fs::create_dir_all(&path)
        .await
        .map_err(|e| format!("Failed to create covers directory: {}", e))?;
    path.push(format!("{}.jpg", book_id));

    fs::write(&path, &cover_bytes)
        .await
        .map_err(|e| format!("Failed to write cover: {}", e))?;

    Ok(())
}

/// Set cover from a URL.
pub async fn cover_from_url(
    State(state): State<AppState>,
    auth: AuthUser,
    Path(book_id): Path<Uuid>,
    Json(request): Json<CoverFromUrlRequest>,
) -> impl IntoResponse {
    if let Err(e) = auth.require_contributor() {
        return e.into_response();
    }
    // Verify the book exists
    match books::exists(&state.pool, book_id).await {
        Ok(false) => return (StatusCode::NOT_FOUND, "book not found").into_response(),
        Err(e) => {
            tracing::error!(error = %e, "Failed to check book existence");
            return (StatusCode::INTERNAL_SERVER_ERROR, "database error").into_response();
        }
        Ok(true) => {}
    }

    // Fetch the cover from the URL
    let client = crate::http::new_client();

    let response = match client.get(&request.url).send().await {
        Ok(resp) => resp,
        Err(e) => {
            tracing::warn!("Failed to fetch cover from URL: {}", e);
            return (StatusCode::BAD_REQUEST, "failed to fetch cover").into_response();
        }
    };

    if !response.status().is_success() {
        return (StatusCode::BAD_REQUEST, "failed to fetch cover").into_response();
    }

    let cover_bytes = match response.bytes().await {
        Ok(bytes) => bytes.to_vec(),
        Err(e) => {
            tracing::warn!("Failed to read cover bytes: {}", e);
            return (StatusCode::BAD_REQUEST, "failed to read cover").into_response();
        }
    };

    // Save the cover
    let mut path = PathBuf::from(&state.config.storage_root);
    path.push("covers");
    if let Err(e) = fs::create_dir_all(&path).await {
        tracing::error!("Failed to create covers directory: {}", e);
        return (StatusCode::INTERNAL_SERVER_ERROR, "failed to save cover").into_response();
    }
    path.push(format!("{}.jpg", book_id));

    if let Err(e) = fs::write(&path, &cover_bytes).await {
        tracing::error!("Failed to write cover: {}", e);
        return (StatusCode::INTERNAL_SERVER_ERROR, "failed to save cover").into_response();
    }

    let _ = images::mark_cached(&state.pool, "books", book_id, "cover").await;

    StatusCode::NO_CONTENT.into_response()
}

/// Upload a cover file via multipart form.
pub async fn upload_cover(
    State(state): State<AppState>,
    auth: AuthUser,
    Path(book_id): Path<Uuid>,
    mut multipart: axum::extract::Multipart,
) -> impl IntoResponse {
    if let Err(e) = auth.require_contributor() {
        return e.into_response();
    }
    // Verify the book exists
    match books::exists(&state.pool, book_id).await {
        Ok(false) => return (StatusCode::NOT_FOUND, "book not found").into_response(),
        Err(e) => {
            tracing::error!(error = %e, "Failed to check book existence");
            return (StatusCode::INTERNAL_SERVER_ERROR, "database error").into_response();
        }
        Ok(true) => {}
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
        Some(bytes) => bytes,
        None => return (StatusCode::BAD_REQUEST, "no cover file provided").into_response(),
    };

    // Save the cover
    let mut path = PathBuf::from(&state.config.storage_root);
    path.push("covers");
    if let Err(e) = fs::create_dir_all(&path).await {
        tracing::error!("Failed to create covers directory: {}", e);
        return (StatusCode::INTERNAL_SERVER_ERROR, "failed to save cover").into_response();
    }
    path.push(format!("{}.jpg", book_id));

    if let Err(e) = fs::write(&path, &cover_bytes).await {
        tracing::error!("Failed to write cover: {}", e);
        return (StatusCode::INTERNAL_SERVER_ERROR, "failed to save cover").into_response();
    }

    let _ = images::mark_cached(&state.pool, "books", book_id, "cover").await;

    StatusCode::NO_CONTENT.into_response()
}
