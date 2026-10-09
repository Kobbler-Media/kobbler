//! Genre-related route handlers.

use axum::{
    Json as AxumJson,
    extract::{Path, State},
    http::StatusCode,
    response::IntoResponse,
};
use uuid::Uuid;

use crate::AppState;
use crate::repositories::{books, genres, shelf_items};
use crate::routes::AuthUser;

use super::helpers::{book_cover_url, extracted_cover_url};
use super::types::{GenreSummary, LibraryBook};

/// List all genres with book counts
pub async fn list_genres(State(state): State<AppState>, _auth: AuthUser) -> impl IntoResponse {
    match genres::list_with_counts(&state.pool).await {
        Ok(rows) => {
            let genres: Vec<GenreSummary> = rows
                .into_iter()
                .map(|row| GenreSummary {
                    id: row.id,
                    name: row.name,
                    book_count: row.book_count,
                })
                .collect();
            AxumJson(genres).into_response()
        }
        Err(e) => {
            tracing::error!(error = %e, "Failed to list genres");
            (StatusCode::INTERNAL_SERVER_ERROR, "database error").into_response()
        }
    }
}

/// List books by genre
pub async fn list_genre_books(
    State(state): State<AppState>,
    auth: AuthUser,
    Path(genre_id): Path<String>,
) -> impl IntoResponse {
    let genre_id = match Uuid::parse_str(&genre_id) {
        Ok(id) => id,
        Err(_) => return (StatusCode::BAD_REQUEST, "invalid genre id").into_response(),
    };

    let rows = match books::list_by_genre(&state.pool, genre_id, auth.user_id).await {
        Ok(rows) => rows,
        Err(e) => {
            tracing::error!(error = %e, "Failed to list books by genre");
            return (StatusCode::INTERNAL_SERVER_ERROR, "database error").into_response();
        }
    };

    let book_ids: Vec<_> = rows.iter().map(|r| r.id).collect();
    let mut shelf_map = shelf_items::list_shelves_for_books(&state.pool, auth.user_id, &book_ids)
        .await
        .unwrap_or_default();

    let books: Vec<LibraryBook> = rows
        .into_iter()
        .map(|row| {
            let shelves = shelf_map.remove(&row.id).unwrap_or_default();
            LibraryBook {
                id: row.id,
                title: row.title,
                authors: serde_json::from_value(row.authors).unwrap_or_default(),
                description: row.description,
                publisher: row.publisher,
                pubdate: row.pubdate,
                series_id: row.series_id,
                series_name: row.series_name,
                series_index: row.series_index,
                language: row.language,
                formats: row.formats,
                synced: row.synced,
                pending_sync: row.pending_sync,
                created_at: row.created_at,
                updated_at: row.updated_at,
                cover_url: book_cover_url(&row.images, row.id),
                extracted_cover_url: extracted_cover_url(&row.images, row.id),
                identifiers: row.identifiers,
                is_in_library: Some(row.is_in_library),
                want_status: Some(row.want_status),
                page_count: row.page_count,
                rating: row.rating,
                rating_count: row.rating_count,
                genres: row.genres,
                shelves,
                provider_metadata: None,
                narrator: row.narrator,
                duration_seconds: row.duration_seconds,
                is_abridged: row.is_abridged,
                want_format: row.want_format,
                has_audiobook: row.has_audiobook,
                media_root_id: row.media_root_id,
                series_memberships: None,
            }
        })
        .collect();

    AxumJson(books).into_response()
}
