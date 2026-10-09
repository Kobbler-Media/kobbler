//! Shelf-related route handlers.

use axum::{
    Json as AxumJson,
    extract::{Json, Path, Query, State},
    http::StatusCode,
    response::IntoResponse,
};
use uuid::Uuid;

use crate::AppState;
use crate::repositories::{books, shelf_items, shelves};
use crate::routes::AuthUser;

use super::helpers::{
    NameCursor, book_cover_url, decode_name_cursor, encode_name_cursor, extracted_cover_url,
    max_dt, record_event,
};
use super::types::{
    LibraryBook, PaginatedResponse, ShelfCreateRequest, ShelfItemsRequest, ShelfQuery,
    ShelfSummary, ShelfUpdateRequest,
};

async fn verify_shelf_ownership(
    pool: &sqlx::PgPool,
    shelf_id: Uuid,
    auth: &AuthUser,
) -> Result<(), (StatusCode, &'static str)> {
    if auth.role == "admin" {
        return Ok(());
    }
    match shelves::get_user_id(pool, shelf_id).await {
        Ok(Some(owner_id)) if owner_id == auth.user_id => Ok(()),
        Ok(Some(_)) => Err((StatusCode::FORBIDDEN, "not your shelf")),
        Ok(None) => Err((StatusCode::NOT_FOUND, "shelf not found")),
        Err(_) => Err((StatusCode::INTERNAL_SERVER_ERROR, "database error")),
    }
}

pub async fn list_shelves(
    State(state): State<AppState>,
    auth: AuthUser,
    Query(query): Query<ShelfQuery>,
) -> impl IntoResponse {
    // Always use requester's own user_id - no cross-user access until permissions are implemented
    let user_id = auth.user_id;

    let runtime_config = state.runtime_config.get().await;
    let limit = query
        .limit
        .unwrap_or(runtime_config.default_page_size)
        .min(runtime_config.max_page_size);
    let cursor = query.cursor.as_deref().and_then(decode_name_cursor);

    let filter = shelves::ShelfListFilter {
        user_id,
        cursor: cursor.map(|c| (c.name, c.id)),
        limit,
    };

    // Only count on first page (no cursor)
    let total_count = if query.cursor.is_none() {
        shelves::count_with_filters(&state.pool, &filter).await.ok()
    } else {
        None
    };

    let rows = match shelves::list_with_filters(&state.pool, &filter).await {
        Ok(rows) => rows,
        Err(e) => {
            tracing::error!(error = %e, "Failed to list shelves");
            return (StatusCode::INTERNAL_SERVER_ERROR, "database error").into_response();
        }
    };

    let has_more = rows.len() as i64 > limit;
    let rows: Vec<_> = if has_more {
        rows.into_iter().take(limit as usize).collect()
    } else {
        rows
    };

    let shelves: Vec<ShelfSummary> = rows
        .into_iter()
        .map(|row| {
            let last_modified = row
                .item_last_modified
                .map(|dt| max_dt(dt, row.updated_at))
                .unwrap_or(row.updated_at);
            let cover_urls: Vec<String> = row
                .sample_book_ids
                .iter()
                .map(|id| format!("/api/v1/library/books/{}/cover", id))
                .collect();
            ShelfSummary {
                id: row.id,
                name: row.name,
                user_id: row.user_id,
                book_count: row.book_count,
                last_modified,
                cover_urls,
            }
        })
        .collect();

    let next_cursor = if has_more {
        shelves.last().map(|s| {
            encode_name_cursor(&NameCursor {
                id: s.id,
                name: s.name.clone(),
            })
        })
    } else {
        None
    };

    AxumJson(PaginatedResponse {
        items: shelves,
        next_cursor,
        prev_cursor: None,
        total_count,
    })
    .into_response()
}

pub async fn create_shelf(
    State(state): State<AppState>,
    auth: AuthUser,
    Json(payload): Json<ShelfCreateRequest>,
) -> impl IntoResponse {
    let user_id = auth.user_id;
    let name = payload.name.trim();
    if name.is_empty() {
        return (StatusCode::BAD_REQUEST, "name required").into_response();
    }

    match shelves::create(&state.pool, user_id, name).await {
        Ok(shelf_id) => {
            record_event(
                &state,
                "shelf.create",
                None,
                "ok",
                Some(name.to_string()),
                None,
            )
            .await;
            AxumJson(shelf_id).into_response()
        }
        Err(_) => (StatusCode::CONFLICT, "shelf exists").into_response(),
    }
}

pub async fn rename_shelf(
    State(state): State<AppState>,
    auth: AuthUser,
    Path(shelf_id): Path<String>,
    Json(payload): Json<ShelfUpdateRequest>,
) -> impl IntoResponse {
    let shelf_id = match Uuid::parse_str(&shelf_id) {
        Ok(id) => id,
        Err(_) => return (StatusCode::BAD_REQUEST, "invalid shelf id").into_response(),
    };
    if let Err(e) = verify_shelf_ownership(&state.pool, shelf_id, &auth).await {
        return e.into_response();
    }
    let name = payload.name.trim();
    if name.is_empty() {
        return (StatusCode::BAD_REQUEST, "name required").into_response();
    }

    match shelves::rename(&state.pool, shelf_id, name).await {
        Ok(_) => {
            record_event(
                &state,
                "shelf.rename",
                None,
                "ok",
                Some(name.to_string()),
                None,
            )
            .await;
            StatusCode::NO_CONTENT.into_response()
        }
        Err(_) => (StatusCode::CONFLICT, "failed to rename").into_response(),
    }
}

pub async fn delete_shelf(
    State(state): State<AppState>,
    auth: AuthUser,
    Path(shelf_id): Path<String>,
) -> impl IntoResponse {
    let shelf_id = match Uuid::parse_str(&shelf_id) {
        Ok(id) => id,
        Err(_) => return (StatusCode::BAD_REQUEST, "invalid shelf id").into_response(),
    };
    if let Err(e) = verify_shelf_ownership(&state.pool, shelf_id, &auth).await {
        return e.into_response();
    }

    let (user_id, name) = match shelves::get_user_and_name(&state.pool, shelf_id).await {
        Ok(Some(data)) => data,
        Ok(None) => return (StatusCode::NOT_FOUND, "not found").into_response(),
        Err(e) => {
            tracing::error!(error = %e, shelf_id = %shelf_id, "failed to fetch shelf");
            return (StatusCode::INTERNAL_SERVER_ERROR, "database error").into_response();
        }
    };

    let mut tx = match state.pool.begin().await {
        Ok(tx) => tx,
        Err(e) => {
            tracing::error!(error = %e, "failed to begin transaction for shelf delete");
            return (StatusCode::INTERNAL_SERVER_ERROR, "database error").into_response();
        }
    };

    if let Err(e) = shelves::archive(&mut *tx, shelf_id, user_id, &name).await {
        tracing::error!(error = %e, shelf_id = %shelf_id, "failed to archive shelf");
        return (StatusCode::INTERNAL_SERVER_ERROR, "failed to delete shelf").into_response();
    }

    if let Err(e) = shelves::delete(&mut *tx, shelf_id).await {
        tracing::error!(error = %e, shelf_id = %shelf_id, "failed to delete shelf");
        return (StatusCode::INTERNAL_SERVER_ERROR, "failed to delete shelf").into_response();
    }

    if let Err(e) = tx.commit().await {
        tracing::error!(error = %e, "failed to commit shelf delete transaction");
        return (StatusCode::INTERNAL_SERVER_ERROR, "database error").into_response();
    }

    record_event(&state, "shelf.delete", None, "ok", Some(name), None).await;
    StatusCode::NO_CONTENT.into_response()
}

pub async fn add_shelf_items(
    State(state): State<AppState>,
    auth: AuthUser,
    Path(shelf_id): Path<String>,
    Json(payload): Json<ShelfItemsRequest>,
) -> impl IntoResponse {
    let shelf_id = match Uuid::parse_str(&shelf_id) {
        Ok(id) => id,
        Err(_) => return (StatusCode::BAD_REQUEST, "invalid shelf id").into_response(),
    };
    if let Err(e) = verify_shelf_ownership(&state.pool, shelf_id, &auth).await {
        return e.into_response();
    }

    let mut tx = match state.pool.begin().await {
        Ok(tx) => tx,
        Err(e) => {
            tracing::error!(error = %e, "failed to begin transaction for add shelf items");
            return (StatusCode::INTERNAL_SERVER_ERROR, "database error").into_response();
        }
    };

    for book_id in payload.book_ids {
        if let Err(e) = shelf_items::add(&mut *tx, shelf_id, book_id).await {
            tracing::error!(error = %e, shelf_id = %shelf_id, book_id = %book_id, "failed to add item to shelf");
        }
    }

    if let Err(e) = shelves::touch(&mut *tx, shelf_id).await {
        tracing::error!(error = %e, shelf_id = %shelf_id, "failed to update shelf timestamp");
    }

    if let Err(e) = tx.commit().await {
        tracing::error!(error = %e, "failed to commit add shelf items transaction");
        return (StatusCode::INTERNAL_SERVER_ERROR, "database error").into_response();
    }

    record_event(&state, "shelf.add_items", None, "ok", None, None).await;
    StatusCode::NO_CONTENT.into_response()
}

pub async fn remove_shelf_items(
    State(state): State<AppState>,
    auth: AuthUser,
    Path(shelf_id): Path<String>,
    Json(payload): Json<ShelfItemsRequest>,
) -> impl IntoResponse {
    let shelf_id = match Uuid::parse_str(&shelf_id) {
        Ok(id) => id,
        Err(_) => return (StatusCode::BAD_REQUEST, "invalid shelf id").into_response(),
    };
    if let Err(e) = verify_shelf_ownership(&state.pool, shelf_id, &auth).await {
        return e.into_response();
    }

    let mut tx = match state.pool.begin().await {
        Ok(tx) => tx,
        Err(e) => {
            tracing::error!(error = %e, "failed to begin transaction for remove shelf items");
            return (StatusCode::INTERNAL_SERVER_ERROR, "database error").into_response();
        }
    };

    for book_id in payload.book_ids {
        if let Err(e) = shelf_items::remove(&mut *tx, shelf_id, book_id).await {
            tracing::error!(error = %e, shelf_id = %shelf_id, book_id = %book_id, "failed to remove item from shelf");
        }
    }

    if let Err(e) = shelves::touch(&mut *tx, shelf_id).await {
        tracing::error!(error = %e, shelf_id = %shelf_id, "failed to update shelf timestamp");
    }

    if let Err(e) = tx.commit().await {
        tracing::error!(error = %e, "failed to commit remove shelf items transaction");
        return (StatusCode::INTERNAL_SERVER_ERROR, "database error").into_response();
    }

    record_event(&state, "shelf.remove_items", None, "ok", None, None).await;
    StatusCode::NO_CONTENT.into_response()
}

pub async fn list_shelf_items(
    State(state): State<AppState>,
    auth: AuthUser,
    Path(shelf_id): Path<String>,
) -> impl IntoResponse {
    let shelf_id = match Uuid::parse_str(&shelf_id) {
        Ok(id) => id,
        Err(_) => return (StatusCode::BAD_REQUEST, "invalid shelf id").into_response(),
    };
    if let Err(e) = verify_shelf_ownership(&state.pool, shelf_id, &auth).await {
        return e.into_response();
    }

    let rows = match books::list_by_shelf(&state.pool, shelf_id, auth.user_id).await {
        Ok(rows) => rows,
        Err(e) => {
            tracing::error!(error = %e, "Failed to list shelf items");
            return (StatusCode::INTERNAL_SERVER_ERROR, "database error").into_response();
        }
    };

    let book_ids: Vec<_> = rows.iter().map(|r| r.id).collect();
    let mut shelf_map = shelf_items::list_shelves_for_books(&state.pool, auth.user_id, &book_ids)
        .await
        .unwrap_or_default();

    let books = rows
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
                is_in_library: None, // Only returned when requested via include_not_owned
                want_status: None,
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
        .collect::<Vec<_>>();

    AxumJson(books).into_response()
}
