//! Reading status and rating route handlers.
//!
//! Provides endpoints for managing user's reading status and ratings for books.

use axum::{
    extract::{Json, Path, State},
    http::StatusCode,
    response::IntoResponse,
};
use serde::{Deserialize, Serialize};
use time::OffsetDateTime;
use uuid::Uuid;

use crate::AppState;
use crate::repositories::{self, books::LibraryBookRow, reading_states, user_book_data};
use crate::routes::AuthUser;
use crate::sync_providers::SyncAction;

// ========== Response Types ==========

/// Combined reading data response for a book.
#[derive(Serialize)]
pub struct ReadingDataResponse {
    /// User's personal reading status: 'want_to_read', 'reading', 'finished', or null
    pub status: Option<String>,
    /// Progress percent from device sync (0-100)
    pub progress_percent: Option<f64>,
    /// Total spent reading minutes across all devices
    pub spent_reading_minutes: Option<i32>,
    /// Book page count for calculating page from percent
    pub page_count: Option<i32>,
    /// When the user started reading
    #[serde(with = "time::serde::rfc3339::option")]
    pub started_at: Option<OffsetDateTime>,
    /// When the user finished reading
    #[serde(with = "time::serde::rfc3339::option")]
    pub finished_at: Option<OffsetDateTime>,
    /// User's rating (1-5)
    pub rating: Option<i32>,
    /// Source of rating: 'manual', provider name, or 'kobo'
    pub rating_source: Option<String>,
    /// Community rating from book metadata
    pub community_rating: Option<f64>,
    /// Community rating count
    pub community_rating_count: Option<i32>,
}

// ========== Request Types ==========

/// Request to update reading status/progress/rating.
#[derive(Deserialize)]
pub struct UpdateReadingRequest {
    /// Reading status: 'want_to_read', 'reading', 'finished', or null to clear
    pub status: Option<String>,
    /// Progress percent (0-100)
    pub progress_percent: Option<f64>,
    /// Page number (converted to percent using book's page_count)
    pub page: Option<i32>,
    /// Rating (1-5) or null to clear
    pub rating: Option<i32>,
}

/// Request to log reading time.
#[derive(Deserialize)]
pub struct LogReadingTimeRequest {
    /// Minutes of reading time to add
    pub minutes: i32,
}

// ========== Handlers ==========

/// Get reading data for a book (current user).
pub async fn get_reading_data(
    State(state): State<AppState>,
    auth: AuthUser,
    Path(book_id): Path<String>,
) -> impl IntoResponse {
    let book_id = match Uuid::parse_str(&book_id) {
        Ok(id) => id,
        Err(_) => return (StatusCode::BAD_REQUEST, "invalid book id").into_response(),
    };

    // Get book for community rating and page count
    let book = match repositories::books::get_by_id(&state.pool, book_id).await {
        Ok(Some(b)) => b,
        Ok(None) => return (StatusCode::NOT_FOUND, "book not found").into_response(),
        Err(e) => {
            tracing::error!(error = %e, "failed to get book");
            return (StatusCode::INTERNAL_SERVER_ERROR, "database error").into_response();
        }
    };

    // Get user book data (rating, personal status)
    let user_data = user_book_data::get(&state.pool, auth.user_id, book_id)
        .await
        .ok()
        .flatten();

    // Get aggregated reading state from devices
    let reading_data =
        reading_states::get_aggregated_for_user_book(&state.pool, auth.user_id, book_id)
            .await
            .ok()
            .flatten();

    // Get total reading time across all devices
    let total_reading_time =
        reading_states::get_total_reading_time(&state.pool, auth.user_id, book_id)
            .await
            .ok()
            .flatten();

    let response = ReadingDataResponse {
        status: user_data
            .as_ref()
            .and_then(|d| d.reading_status.clone())
            .or_else(|| {
                // Fall back to device reading state status
                reading_data.as_ref().map(|r| match r.status.as_str() {
                    "ReadyToRead" => "want_to_read".to_string(),
                    "Reading" => "reading".to_string(),
                    "Finished" => "finished".to_string(),
                    _ => r.status.clone(),
                })
            }),
        progress_percent: reading_data.as_ref().and_then(|r| r.progress_percent),
        spent_reading_minutes: total_reading_time,
        page_count: book.page_count,
        started_at: user_data.as_ref().and_then(|d| d.started_at),
        finished_at: user_data.as_ref().and_then(|d| d.finished_at),
        rating: user_data.as_ref().and_then(|d| d.rating),
        rating_source: user_data.as_ref().and_then(|d| d.rating_source.clone()),
        community_rating: book.rating,
        community_rating_count: book.rating_count,
    };

    Json(response).into_response()
}

/// Update reading status/progress/rating for a book.
pub async fn update_reading_data(
    State(state): State<AppState>,
    auth: AuthUser,
    Path(book_id): Path<String>,
    Json(payload): Json<UpdateReadingRequest>,
) -> impl IntoResponse {
    let book_id = match Uuid::parse_str(&book_id) {
        Ok(id) => id,
        Err(_) => return (StatusCode::BAD_REQUEST, "invalid book id").into_response(),
    };

    // Verify book exists
    let book = match repositories::books::get_by_id(&state.pool, book_id).await {
        Ok(Some(b)) => b,
        Ok(None) => return (StatusCode::NOT_FOUND, "book not found").into_response(),
        Err(e) => {
            tracing::error!(error = %e, "failed to get book");
            return (StatusCode::INTERNAL_SERVER_ERROR, "database error").into_response();
        }
    };

    // Update reading status if provided
    if let Some(status) = &payload.status {
        let valid_statuses = ["want_to_read", "reading", "finished"];
        if !status.is_empty() && !valid_statuses.contains(&status.as_str()) {
            return (StatusCode::BAD_REQUEST, "invalid status").into_response();
        }

        let status_value = if status.is_empty() {
            None
        } else {
            Some(status.as_str())
        };

        if let Err(e) =
            user_book_data::update_reading_status(&state.pool, auth.user_id, book_id, status_value)
                .await
        {
            tracing::error!(error = %e, "failed to update reading status");
            return (StatusCode::INTERNAL_SERVER_ERROR, "failed to update status").into_response();
        }

        // Queue sync for status change to all enabled providers
        let action = match status.as_str() {
            "reading" => Some(SyncAction::Started),
            "finished" => Some(SyncAction::Finished),
            _ => None,
        };
        if let Some(action) = action {
            queue_sync_for_providers(&state, auth.user_id, book_id, &book, action).await;
        }
    }

    // Update progress if provided (page or percent)
    let progress_percent = if let Some(page) = payload.page {
        // Convert page to percent using book's page count
        if let Some(total_pages) = book.page_count {
            if total_pages > 0 {
                Some((page as f64 / total_pages as f64) * 100.0)
            } else {
                None
            }
        } else {
            None
        }
    } else {
        payload.progress_percent
    };

    if let Some(percent) = progress_percent {
        let percent = percent.clamp(0.0, 100.0);

        // Get or create a reading state for this user+book.
        // Books synced from external providers (e.g. Hardcover) only have user_book_data,
        // not reading_states, so we create one on first local progress update.
        let state_id =
            match reading_states::get_aggregated_for_user_book(&state.pool, auth.user_id, book_id)
                .await
            {
                Ok(Some(agg)) => Some(agg.reading_state_id),
                _ => {
                    // No reading state exists — create one via the web reader device
                    let device_id = match repositories::devices::get_or_create_web_reader(
                        &state.pool,
                        auth.user_id,
                    )
                    .await
                    {
                        Ok(id) => id,
                        Err(e) => {
                            tracing::error!(error = %e, "failed to get/create web reader device");
                            return (
                                StatusCode::INTERNAL_SERVER_ERROR,
                                "failed to create reading state",
                            )
                                .into_response();
                        }
                    };
                    match reading_states::upsert_with_children(
                        &state.pool,
                        auth.user_id,
                        device_id,
                        book_id,
                    )
                    .await
                    {
                        Ok(id) => id,
                        Err(e) => {
                            tracing::error!(error = %e, "failed to create reading state");
                            None
                        }
                    }
                }
            };

        if let Some(state_id) = state_id {
            if let Err(e) = reading_states::update_bookmark(
                &state.pool,
                state_id,
                Some(percent),
                None,
                None,
                None,
                None,
            )
            .await
            {
                tracing::error!(error = %e, "failed to update bookmark");
            }

            // Touch the reading state
            let _ = reading_states::touch(&state.pool, state_id).await;
        }

        // Auto-complete: if progress >= 95% and no explicit status was provided, mark as finished
        if percent >= 95.0 && payload.status.is_none() {
            let _ = user_book_data::update_reading_status(
                &state.pool,
                auth.user_id,
                book_id,
                Some("finished"),
            )
            .await;
            queue_sync_for_providers(&state, auth.user_id, book_id, &book, SyncAction::Finished)
                .await;
        }

        // Queue sync for progress update to all enabled providers
        queue_sync_for_providers(
            &state,
            auth.user_id,
            book_id,
            &book,
            SyncAction::Progress {
                percent,
                page_count: book.page_count,
                started_at: Some(chrono::Utc::now().format("%Y-%m-%d").to_string()),
            },
        )
        .await;
    }

    // Update rating if provided
    if let Some(rating) = payload.rating {
        // rating = 0 clears the rating, rating 1-5 sets it, anything else is invalid
        if rating != 0 && !(1..=5).contains(&rating) {
            return (StatusCode::BAD_REQUEST, "rating must be 1-5").into_response();
        }

        let rating_value = if rating == 0 { None } else { Some(rating) };

        if let Err(e) = user_book_data::update_rating(
            &state.pool,
            auth.user_id,
            book_id,
            rating_value,
            "manual",
        )
        .await
        {
            tracing::error!(error = %e, "failed to update rating");
            return (StatusCode::INTERNAL_SERVER_ERROR, "failed to update rating").into_response();
        }

        // Queue sync for rating to all enabled providers
        if let Some(rating) = rating_value {
            queue_sync_for_providers(
                &state,
                auth.user_id,
                book_id,
                &book,
                SyncAction::Rating { stars: rating },
            )
            .await;
        }
    }

    // Re-index book in search (reading_progress may have changed)
    crate::jobs::enqueue_book_index(&state.pool, state.nats.publisher(), book_id).await;

    StatusCode::NO_CONTENT.into_response()
}

/// Log reading time for a book.
pub async fn log_reading_time(
    State(state): State<AppState>,
    auth: AuthUser,
    Path(book_id): Path<String>,
    Json(payload): Json<LogReadingTimeRequest>,
) -> impl IntoResponse {
    let book_id = match Uuid::parse_str(&book_id) {
        Ok(id) => id,
        Err(_) => return (StatusCode::BAD_REQUEST, "invalid book id").into_response(),
    };

    if payload.minutes <= 0 {
        return (StatusCode::BAD_REQUEST, "minutes must be positive").into_response();
    }

    // Verify book exists
    if !repositories::books::exists(&state.pool, book_id)
        .await
        .unwrap_or(false)
    {
        return (StatusCode::NOT_FOUND, "book not found").into_response();
    }

    // Add reading time to the most recent reading state
    let updated =
        reading_states::add_reading_time(&state.pool, auth.user_id, book_id, payload.minutes)
            .await
            .unwrap_or(false);

    if !updated {
        tracing::debug!(
            book_id = %book_id,
            user_id = %auth.user_id,
            "no reading state to add time to"
        );
    }

    StatusCode::NO_CONTENT.into_response()
}

// ========== Helpers ==========

/// Queue sync jobs for all enabled sync providers that have this book linked.
/// Uses a nil device UUID since this is from the UI, not a device sync.
async fn queue_sync_for_providers(
    state: &AppState,
    user_id: Uuid,
    book_id: Uuid,
    book: &LibraryBookRow,
    action: SyncAction,
) {
    let registry = crate::sync_providers::SyncProviderRegistry::new();
    let providers = repositories::user_sync_providers::list_enabled_for_user(&state.pool, user_id)
        .await
        .unwrap_or_default();

    for provider_config in providers {
        // Check if this provider has the book linked (via identifier key)
        if let Some(provider) = registry.create_provider(
            &provider_config.provider_type,
            provider_config
                .config
                .get("api_key")
                .and_then(|v| v.as_str())
                .unwrap_or(""),
        ) && provider.is_book_linked(&book.identifiers)
            && let Err(e) = crate::jobs::handlers::queue_sync_job_for_user(
                &state.pool,
                state.nats.publisher(),
                Some(user_id),
                Uuid::nil(),
                book_id,
                &provider_config.provider_type,
                action.clone(),
            )
            .await
        {
            tracing::warn!(
                error = %e,
                book_id = %book_id,
                provider = %provider_config.provider_type,
                action = ?action,
                "failed to queue sync"
            );
        }
    }
}
