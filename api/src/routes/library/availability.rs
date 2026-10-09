//! Availability-related route handlers.

use axum::{
    Json as AxumJson,
    extract::{Path, State},
    http::StatusCode,
    response::IntoResponse,
};
use uuid::Uuid;

use crate::AppState;
use crate::events::LibraryEvent;
use crate::jobs::handlers::file_availability::check_file_availability;
use crate::repositories::{book_files, books, entitlements};
use crate::routes::AuthUser;

use super::helpers::record_event;
use super::types::{BookRecommendation, UnavailableBook, UnavailableBooksResponse};

/// List books that have unavailable (missing) files
pub async fn list_unavailable_books(
    State(state): State<AppState>,
    _auth: AuthUser,
) -> impl IntoResponse {
    // Query books that have at least one unavailable file
    let rows = match books::list_unavailable(&state.pool).await {
        Ok(rows) => rows,
        Err(e) => {
            tracing::error!(error = %e, "Failed to list unavailable books");
            return (StatusCode::INTERNAL_SERVER_ERROR, "database error").into_response();
        }
    };

    let total_count = rows.len() as i64;

    let items: Vec<UnavailableBook> = rows
        .into_iter()
        .map(|row| UnavailableBook {
            id: row.id,
            title: row.title,
            authors: row.authors,
            unavailable_files: row.unavailable_files,
            total_files: row.total_files,
            unavailable_since: row.unavailable_since,
            cover_url: Some(format!("/api/v1/library/books/{}/cover", row.id)),
        })
        .collect();

    AxumJson(UnavailableBooksResponse { items, total_count }).into_response()
}

/// Delete books that have all files unavailable
pub async fn delete_unavailable_books(
    State(state): State<AppState>,
    auth: AuthUser,
) -> impl IntoResponse {
    if let Err(e) = auth.require_admin() {
        return e.into_response();
    }
    // Find books where ALL files are unavailable
    let book_ids = match books::list_fully_unavailable_ids(&state.pool).await {
        Ok(ids) => ids,
        Err(e) => {
            tracing::error!(error = %e, "Failed to list fully unavailable books");
            return (StatusCode::INTERNAL_SERVER_ERROR, "database error").into_response();
        }
    };

    if book_ids.is_empty() {
        return AxumJson(serde_json::json!({
            "deleted": 0,
            "book_ids": []
        }))
        .into_response();
    }

    let mut tx = match state.pool.begin().await {
        Ok(tx) => tx,
        Err(e) => {
            tracing::error!(error = %e, "failed to begin transaction for delete unavailable books");
            return (StatusCode::INTERNAL_SERVER_ERROR, "database error").into_response();
        }
    };

    // Mark entitlements as removed (so Kobo devices delete the books)
    if let Err(e) = entitlements::mark_removed_by_books(&mut *tx, &book_ids).await {
        tracing::error!(error = %e, "failed to mark entitlements as removed");
        return (StatusCode::INTERNAL_SERVER_ERROR, "failed to delete books").into_response();
    }

    // Delete book files
    if let Err(e) = book_files::delete_by_books(&mut *tx, &book_ids).await {
        tracing::error!(error = %e, "failed to delete book files");
        return (StatusCode::INTERNAL_SERVER_ERROR, "failed to delete books").into_response();
    }

    if let Err(e) = tx.commit().await {
        tracing::error!(error = %e, "failed to commit delete unavailable books transaction");
        return (StatusCode::INTERNAL_SERVER_ERROR, "database error").into_response();
    }

    let deleted = book_ids.len();
    for book_id in &book_ids {
        record_event(
            &state,
            "book.delete_unavailable",
            Some(*book_id),
            "ok",
            Some("All files unavailable".to_string()),
            None,
        )
        .await;
        let _ = state
            .library_events
            .send(LibraryEvent::library_updated(Some(*book_id)));
    }

    AxumJson(serde_json::json!({
        "deleted": deleted,
        "book_ids": book_ids
    }))
    .into_response()
}

/// Check availability of files for a specific book
pub async fn check_book_availability(
    State(state): State<AppState>,
    _auth: AuthUser,
    Path(book_id): Path<String>,
) -> impl IntoResponse {
    let book_id = match Uuid::parse_str(&book_id) {
        Ok(id) => id,
        Err(_) => return (StatusCode::BAD_REQUEST, "invalid book id").into_response(),
    };

    // Verify book exists
    match books::exists(&state.pool, book_id).await {
        Ok(false) => return (StatusCode::NOT_FOUND, "book not found").into_response(),
        Err(e) => {
            tracing::error!(error = %e, "failed to check book exists");
            return (StatusCode::INTERNAL_SERVER_ERROR, "database error").into_response();
        }
        Ok(true) => {}
    }

    match check_file_availability(&state.pool, Some(book_id)).await {
        Ok(result) => {
            record_event(
                &state,
                "availability.check",
                Some(book_id),
                "ok",
                None,
                None,
            )
            .await;
            AxumJson::<serde_json::Value>(result).into_response()
        }
        Err(e) => {
            tracing::error!(error = %e, book_id = %book_id, "failed to check book availability");
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                "failed to check availability",
            )
                .into_response()
        }
    }
}

/// Get recommendations for a book (next in series, same author)
pub async fn get_book_recommendations(
    State(state): State<AppState>,
    _auth: AuthUser,
    Path(book_id): Path<String>,
) -> impl IntoResponse {
    let book_id = match Uuid::parse_str(&book_id) {
        Ok(id) => id,
        Err(_) => return (StatusCode::BAD_REQUEST, "invalid book id").into_response(),
    };

    // Verify book exists
    match books::exists(&state.pool, book_id).await {
        Ok(false) => return (StatusCode::NOT_FOUND, "book not found").into_response(),
        Err(e) => {
            tracing::error!(error = %e, "failed to check book exists");
            return (StatusCode::INTERNAL_SERVER_ERROR, "database error").into_response();
        }
        Ok(true) => {}
    }

    // Use a dummy user/device ID for admin preview (we want all entitled books)
    // In production the recommendations respect entitlements, but for admin preview
    // we use a nil UUID which won't match any specific user/device scope
    let dummy_id = Uuid::nil();

    let candidates =
        crate::kobo::get_recommendations(&state.pool, book_id, dummy_id, dummy_id, 10).await;

    let runtime_config = state.runtime_config.get().await;
    let api_base = state
        .config
        .public_proxy_url(&runtime_config.public_proxy_url);

    let recommendations: Vec<BookRecommendation> = candidates
        .into_iter()
        .map(|c| {
            let reason = match c.reason {
                crate::kobo::recommendations::RecommendationReason::NextInSeries => {
                    "next_in_series"
                }
                crate::kobo::recommendations::RecommendationReason::SameAuthor => "same_author",
            };
            BookRecommendation {
                id: c.book_id.to_string(),
                title: c.title,
                authors: c.authors,
                series_name: c.series_name,
                series_index: c.series_index,
                cover_url: Some(format!(
                    "{}/api/v1/library/books/{}/cover",
                    api_base, c.book_id
                )),
                reason: reason.to_string(),
                progress_percent: c.progress_percent,
            }
        })
        .collect();

    AxumJson(recommendations).into_response()
}
