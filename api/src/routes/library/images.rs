//! Image route handlers for authors and series.

use axum::{
    extract::{Path, Query, State},
    http::{HeaderMap, StatusCode},
    response::IntoResponse,
};
use tokio::fs;
use uuid::Uuid;

use crate::AppState;
use crate::images::{self, ImageType};
use crate::repositories::{authors, series};
use crate::routes::{extract_bearer, extract_session_cookie, require_admin_token};

use super::types::CoverQuery;

/// Helper to serve image bytes with correct content type
fn serve_image_bytes(bytes: Vec<u8>) -> axum::response::Response {
    let content_type = infer::get(&bytes)
        .map(|t| t.mime_type())
        .unwrap_or("application/octet-stream");
    (StatusCode::OK, [("Content-Type", content_type)], bytes).into_response()
}

/// GET /api/v1/library/authors/{author_id}/image
/// Get the cached image for an author
pub async fn get_author_image(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(query): Query<CoverQuery>,
    Path(author_id): Path<String>,
) -> impl IntoResponse {
    // Auth check (same pattern as cover endpoint)
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

    let author_id = match Uuid::parse_str(&author_id) {
        Ok(id) => id,
        Err(_) => return (StatusCode::BAD_REQUEST, "invalid author id").into_response(),
    };

    // Check if cached image exists
    let path = images::local_image_path(&state.config.storage_root, ImageType::Author, author_id);

    if let Ok(bytes) = fs::read(&path).await {
        return serve_image_bytes(bytes);
    }

    // Try to lazy-cache from the images JSONB source_url
    if let Ok(Some(url)) = authors::get_image_url(&state.pool, author_id).await {
        if let Ok(cached_path) = images::cache_image(
            &state.config.storage_root,
            ImageType::Author,
            author_id,
            &url,
        )
        .await
        {
            let _ = images::mark_cached(&state.pool, "authors", author_id, "profile").await;
            if let Ok(bytes) = fs::read(&cached_path).await {
                return serve_image_bytes(bytes);
            }
        }
        // Fall back to redirect if caching failed
        return (
            StatusCode::TEMPORARY_REDIRECT,
            [("Location", url.as_str())],
            "",
        )
            .into_response();
    }

    (StatusCode::NOT_FOUND, "image not found").into_response()
}

/// GET /api/v1/library/series/{series_id}/image
/// Get the cached image for a series
pub async fn get_series_image(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(query): Query<CoverQuery>,
    Path(series_id): Path<String>,
) -> impl IntoResponse {
    // Auth check (same pattern as cover endpoint)
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

    let series_id = match Uuid::parse_str(&series_id) {
        Ok(id) => id,
        Err(_) => return (StatusCode::BAD_REQUEST, "invalid series id").into_response(),
    };

    // Check if cached image exists
    let path = images::local_image_path(&state.config.storage_root, ImageType::Series, series_id);

    if let Ok(bytes) = fs::read(&path).await {
        return serve_image_bytes(bytes);
    }

    // Try to lazy-cache from the images JSONB source_url
    if let Ok(Some(url)) = series::get_image_url(&state.pool, series_id).await {
        if let Ok(cached_path) = images::cache_image(
            &state.config.storage_root,
            ImageType::Series,
            series_id,
            &url,
        )
        .await
        {
            let _ = images::mark_cached(&state.pool, "series", series_id, "cover").await;
            if let Ok(bytes) = fs::read(&cached_path).await {
                return serve_image_bytes(bytes);
            }
        }
        // Fall back to redirect if caching failed
        return (
            StatusCode::TEMPORARY_REDIRECT,
            [("Location", url.as_str())],
            "",
        )
            .into_response();
    }

    (StatusCode::NOT_FOUND, "image not found").into_response()
}
