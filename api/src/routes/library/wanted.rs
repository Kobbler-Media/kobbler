//! Wanted-related route handlers for marking books as wanted or skipped.

use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    response::IntoResponse,
};
use serde_json::json;
use uuid::Uuid;

use crate::AppState;
use crate::events::LibraryEvent;
use crate::file_link_mode::AcquireQuery;
use crate::jobs::{JobContext, enqueue};
use crate::repositories::books;
use crate::routes::AuthUser;

/// POST /api/v1/library/books/{book_id}/want
/// Mark a book as wanted and trigger a search via Kobload.
/// For books not in library: sets want_status to 'wanted'.
/// For books already in library: leaves want_status unchanged but still
/// enqueues the search job (for replacement/upgrade scenarios).
pub async fn want_book(
    State(state): State<AppState>,
    auth: AuthUser,
    Path(book_id): Path<Uuid>,
    Query(acquire): Query<AcquireQuery>,
) -> impl IntoResponse {
    // Check the book exists and whether it's in library
    let in_library =
        match sqlx::query_scalar!("SELECT (EXISTS (SELECT 1 FROM book_files bf WHERE bf.book_id = books.id) OR EXISTS (SELECT 1 FROM audiobook_files af WHERE af.book_id = books.id)) AS \"is_in_library!\" FROM books WHERE id = $1", book_id)
            .fetch_optional(&state.pool)
            .await
        {
            Ok(Some(v)) => v,
            Ok(None) => {
                return (StatusCode::NOT_FOUND, "book not found").into_response();
            }
            Err(e) => {
                tracing::error!(error = %e, book_id = %book_id, "failed to check book");
                return (StatusCode::INTERNAL_SERVER_ERROR, "database error").into_response();
            }
        };

    if !in_library {
        // Not in library: set want_status to 'wanted'
        if let Err(e) =
            books::set_want_status_if_not_in_library(&state.pool, book_id, "wanted").await
        {
            tracing::error!(error = %e, book_id = %book_id, "failed to want book");
            return (StatusCode::INTERNAL_SERVER_ERROR, "database error").into_response();
        }
        let _ = state
            .library_events
            .send(LibraryEvent::library_updated(Some(book_id)));
    }

    // Enqueue book_want job regardless of library status
    if let Err(e) = enqueue_book_want_job(&state, book_id, auth.user_id, acquire.manual).await {
        tracing::warn!(book_id = %book_id, error = %e, "Failed to enqueue book_want job");
    }
    StatusCode::NO_CONTENT.into_response()
}

/// Enqueue a book_want job to search and create a watch pattern.
/// This triggers immediate search (via want_request to Kobload) plus ongoing monitoring.
async fn enqueue_book_want_job(
    state: &AppState,
    book_id: Uuid,
    user_id: Uuid,
    manual: bool,
) -> Result<(), String> {
    // Get book title for context description
    let title = books::get_wanted_metadata(&state.pool, book_id)
        .await
        .ok()
        .flatten()
        .map(|m| m.title)
        .unwrap_or_else(|| "Unknown".to_string());

    // Enqueue book_want job
    enqueue(
        &state.pool,
        state.nats.publisher(),
        "book_want",
        json!({ "book_id": book_id, "user_id": user_id, "manual": manual }),
        {
            let display = crate::jobs::fetch_job_display(&state.pool, "book", book_id).await;
            JobContext::for_entity("book", book_id, format!("Searching for: {}", title))
                .with_display_opt(display)
        },
    )
    .await
    .map_err(|e| format!("Failed to enqueue job: {}", e))?;

    tracing::info!(book_id = %book_id, "Book want job enqueued");
    Ok(())
}

/// POST /api/v1/library/authors/{author_id}/want-pending
/// Mark all pending books for an author as wanted
pub async fn want_all_pending_for_author(
    State(state): State<AppState>,
    auth: AuthUser,
    Path(author_id): Path<Uuid>,
) -> impl IntoResponse {
    match books::want_all_pending_for_author(&state.pool, author_id).await {
        Ok(book_ids) => {
            let wanted_count = book_ids.len();

            // Enqueue book_want jobs for each newly wanted book
            for book_id in &book_ids {
                let _ = state
                    .library_events
                    .send(LibraryEvent::library_updated(Some(*book_id)));
                if let Err(e) = enqueue_book_want_job(&state, *book_id, auth.user_id, false).await {
                    tracing::warn!(book_id = %book_id, error = %e, "Failed to enqueue book_want job");
                }
            }

            tracing::info!(author_id = %author_id, wanted_count, "Bulk wanted all pending books for author");
            axum::Json(serde_json::json!({ "wanted_count": wanted_count })).into_response()
        }
        Err(e) => {
            tracing::error!(error = %e, author_id = %author_id, "failed to want all books for author");
            (StatusCode::INTERNAL_SERVER_ERROR, "database error").into_response()
        }
    }
}

/// POST /api/v1/library/series/{series_id}/want-pending
/// Mark all pending books for a series as wanted
pub async fn want_all_pending_for_series(
    State(state): State<AppState>,
    auth: AuthUser,
    Path(series_id): Path<Uuid>,
) -> impl IntoResponse {
    match books::want_all_pending_for_series(&state.pool, series_id).await {
        Ok(book_ids) => {
            let wanted_count = book_ids.len();

            // Enqueue book_want jobs for each newly wanted book
            for book_id in &book_ids {
                let _ = state
                    .library_events
                    .send(LibraryEvent::library_updated(Some(*book_id)));
                if let Err(e) = enqueue_book_want_job(&state, *book_id, auth.user_id, false).await {
                    tracing::warn!(book_id = %book_id, error = %e, "Failed to enqueue book_want job");
                }
            }

            tracing::info!(series_id = %series_id, wanted_count, "Bulk wanted all pending books for series");
            axum::Json(serde_json::json!({ "wanted_count": wanted_count })).into_response()
        }
        Err(e) => {
            tracing::error!(error = %e, series_id = %series_id, "failed to want all books for series");
            (StatusCode::INTERNAL_SERVER_ERROR, "database error").into_response()
        }
    }
}

/// DELETE /api/v1/library/authors/{author_id}/want-pending
/// Drop every outstanding want on an author's books.
///
/// The inverse of `want_all_pending_for_author`. Books that are already in the
/// library, skipped, or merely pending are left alone. Unlike TV, no auto-want
/// mode has to be switched off: the book auto-want only assigns a want status
/// when a book is first discovered, so it never re-wants one of these.
pub async fn unwant_all_for_author(
    State(state): State<AppState>,
    _auth: AuthUser,
    Path(author_id): Path<Uuid>,
) -> impl IntoResponse {
    match books::unwant_all_for_author(&state.pool, author_id).await {
        Ok(book_ids) => {
            let unwanted_count = book_ids.len();
            for book_id in &book_ids {
                let _ = state
                    .library_events
                    .send(LibraryEvent::library_updated(Some(*book_id)));
                crate::wanted::book_source::delete_book_patterns(state.nats.publisher(), *book_id)
                    .await;
            }

            tracing::info!(author_id = %author_id, unwanted_count, "Bulk unwanted books for author");
            axum::Json(serde_json::json!({ "unwanted_count": unwanted_count })).into_response()
        }
        Err(e) => {
            tracing::error!(error = %e, author_id = %author_id, "failed to unwant books for author");
            (StatusCode::INTERNAL_SERVER_ERROR, "database error").into_response()
        }
    }
}

/// DELETE /api/v1/library/series/{series_id}/want-pending
/// Drop every outstanding want on a series' books.
/// See [`unwant_all_for_author`] for what is left untouched.
pub async fn unwant_all_for_series(
    State(state): State<AppState>,
    _auth: AuthUser,
    Path(series_id): Path<Uuid>,
) -> impl IntoResponse {
    match books::unwant_all_for_series(&state.pool, series_id).await {
        Ok(book_ids) => {
            let unwanted_count = book_ids.len();
            for book_id in &book_ids {
                let _ = state
                    .library_events
                    .send(LibraryEvent::library_updated(Some(*book_id)));
                crate::wanted::book_source::delete_book_patterns(state.nats.publisher(), *book_id)
                    .await;
            }

            tracing::info!(series_id = %series_id, unwanted_count, "Bulk unwanted books for series");
            axum::Json(serde_json::json!({ "unwanted_count": unwanted_count })).into_response()
        }
        Err(e) => {
            tracing::error!(error = %e, series_id = %series_id, "failed to unwant books for series");
            (StatusCode::INTERNAL_SERVER_ERROR, "database error").into_response()
        }
    }
}

/// POST /api/v1/library/books/{book_id}/skip
/// Mark a book as skipped (don't want it)
pub async fn skip_book(
    State(state): State<AppState>,
    _auth: AuthUser,
    Path(book_id): Path<Uuid>,
) -> impl IntoResponse {
    match books::set_want_status_if_not_in_library(&state.pool, book_id, "skipped").await {
        Ok(true) => {
            let _ = state
                .library_events
                .send(LibraryEvent::library_updated(Some(book_id)));
            crate::wanted::book_source::delete_book_patterns(state.nats.publisher(), book_id).await;
            StatusCode::NO_CONTENT.into_response()
        }
        Ok(false) => (
            StatusCode::NOT_FOUND,
            "book not found or already in library",
        )
            .into_response(),
        Err(e) => {
            tracing::error!(error = %e, book_id = %book_id, "failed to skip book");
            (StatusCode::INTERNAL_SERVER_ERROR, "database error").into_response()
        }
    }
}
