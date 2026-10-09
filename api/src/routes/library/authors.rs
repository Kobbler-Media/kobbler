//! Author-related route handlers.

use axum::{
    Json as AxumJson,
    extract::{Json, Path, Query, State},
    http::StatusCode,
    response::IntoResponse,
};
use serde::Deserialize;
use uuid::Uuid;

use crate::AppState;
use crate::events::LibraryEvent;
use crate::images::{ImageEntry, endpoint_url};
use crate::repositories::{
    PaginationDirection, authors as authors_repo, authors::AuthorInput, books, shelf_items,
};
use crate::routes::AuthUser;

use super::helpers::{
    NameCursor, book_cover_url, decode_name_cursor, encode_name_cursor, extracted_cover_url,
    record_activity, record_event,
};
use super::types::{
    AuthorCreateRequest, AuthorMergeRequest, AuthorRenameRequest, AuthorSummary,
    AuthorUpdateRequest, DebugQuery, LibraryBook, ListAuthorsQuery, PaginatedResponse,
};

#[derive(Debug, Deserialize)]
pub struct ListAuthorBooksQuery {
    /// Include books not in library (discovered from author tracking)
    #[serde(default)]
    pub include_not_owned: bool,
}

pub async fn get_author(
    State(state): State<AppState>,
    _auth: AuthUser,
    Path(author_id): Path<String>,
    Query(params): Query<DebugQuery>,
) -> impl IntoResponse {
    let author_id = match Uuid::parse_str(&author_id) {
        Ok(id) => id,
        Err(_) => return (StatusCode::BAD_REQUEST, "invalid author id").into_response(),
    };

    match authors_repo::get_by_id(&state.pool, author_id).await {
        Ok(Some(row)) => {
            let cover_urls: Vec<String> = row
                .sample_book_ids
                .iter()
                .map(|id| format!("/api/v1/library/books/{}/cover", id))
                .collect();
            let metadata = if params.debug.unwrap_or(false) {
                let m = row.metadata;
                if m.as_object().is_some_and(|o| !o.is_empty()) {
                    Some(m)
                } else {
                    None
                }
            } else {
                None
            };
            AxumJson(AuthorSummary {
                id: row.id,
                name: row.name,
                book_count: row.book_count,
                series_count: row.series_count,
                total_book_count: row.total_book_count,
                total_series_count: row.total_series_count,
                wanted_count: row.wanted_count,
                cover_urls,
                identifiers: row.identifiers,
                image_url: endpoint_url(
                    &row.images,
                    "profile",
                    "/api/v1/library/authors/",
                    row.id,
                    "/image",
                ),
                bio: row.bio,
                is_monitored: row.is_monitored,
                auto_want_mode: row.auto_want_mode.unwrap_or_else(|| "none".to_string()),
                known_book_count: row.known_book_count,
                top_series_names: row.top_series_names,
                metadata,
            })
            .into_response()
        }
        Ok(None) => (StatusCode::NOT_FOUND, "author not found").into_response(),
        Err(e) => {
            tracing::error!(error = %e, "Failed to get author");
            (StatusCode::INTERNAL_SERVER_ERROR, "database error").into_response()
        }
    }
}

pub async fn list_author_books(
    State(state): State<AppState>,
    auth: AuthUser,
    Path(author_id): Path<String>,
    Query(params): Query<ListAuthorBooksQuery>,
) -> impl IntoResponse {
    let author_id = match Uuid::parse_str(&author_id) {
        Ok(id) => id,
        Err(_) => return (StatusCode::BAD_REQUEST, "invalid author id").into_response(),
    };

    let rows = match books::list_by_author(
        &state.pool,
        author_id,
        auth.user_id,
        params.include_not_owned,
    )
    .await
    {
        Ok(rows) => rows,
        Err(e) => {
            tracing::error!(error = %e, "Failed to list author books");
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
        .collect::<Vec<_>>();

    AxumJson(books).into_response()
}

pub async fn list_authors(
    State(state): State<AppState>,
    _auth: AuthUser,
    Query(params): Query<ListAuthorsQuery>,
) -> impl IntoResponse {
    let runtime_config = state.runtime_config.get().await;
    let limit = params
        .limit
        .unwrap_or(runtime_config.default_page_size)
        .min(runtime_config.max_page_size);

    // Try MeiliSearch first for typo-tolerant search, fall back to ILIKE
    // When searching, we use offset-based pagination with MeiliSearch
    let search_offset = params.search_offset.unwrap_or(0);
    let search_limit = limit as usize + 1; // +1 to detect if there are more results

    // Track if MeiliSearch has more results (for setting next_cursor in search mode)
    let mut search_has_more = false;

    // When format filter is active, skip MeiliSearch (it can't filter authors by book format)
    // and fall back to ILIKE search which works with the SQL-level format filter.
    let use_meili = params.format.is_none();
    let (search_ids, search) = if let Some(ref query) = params.search {
        if !query.trim().is_empty() {
            if use_meili {
                // Use search backend for fuzzy search with attribute filters
                use crate::search::AuthorSearchFilters;
                let filters = AuthorSearchFilters {
                    is_monitored: params.is_monitored,
                };
                match state
                    .search
                    .search_authors(query, filters, search_limit, search_offset)
                    .await
                {
                    Ok(mut ids) => {
                        // Check if MeiliSearch has more results
                        search_has_more = ids.len() > limit as usize;
                        // Truncate to limit for database query
                        ids.truncate(limit as usize);
                        (Some(ids), None)
                    }
                    Err(e) => {
                        // Fall back to ILIKE search on error
                        tracing::warn!(error = %e, "Search backend failed, falling back to ILIKE");
                        (None, Some(query.clone()))
                    }
                }
            } else {
                // Format filter active — use ILIKE for SQL-level filtering
                (None, Some(query.clone()))
            }
        } else {
            (None, None)
        }
    } else {
        (None, None)
    };

    // Handle "around" parameter for bidirectional pagination (jump to specific author)
    if let Some(ref around_id) = params.around {
        let author_id = match Uuid::parse_str(around_id) {
            Ok(id) => id,
            Err(_) => return (StatusCode::BAD_REQUEST, "invalid around id").into_response(),
        };

        // Get the author's name to use as cursor position
        let author_name = match authors_repo::get_name(&state.pool, author_id).await {
            Ok(Some(name)) => name,
            Ok(None) => return (StatusCode::NOT_FOUND, "author not found").into_response(),
            Err(e) => {
                tracing::error!(error = %e, "Failed to get author name");
                return (StatusCode::INTERNAL_SERVER_ERROR, "database error").into_response();
            }
        };

        // Get the target author data
        let target_row = match authors_repo::get_by_id(&state.pool, author_id).await {
            Ok(Some(row)) => row,
            Ok(None) => return (StatusCode::NOT_FOUND, "author not found").into_response(),
            Err(e) => {
                tracing::error!(error = %e, "Failed to get author");
                return (StatusCode::INTERNAL_SERVER_ERROR, "database error").into_response();
            }
        };

        // Query items BEFORE the target (backward direction)
        let half_limit = limit / 2;
        let before_filter = authors_repo::AuthorListFilter {
            cursor: Some((author_name.clone(), author_id)),
            direction: PaginationDirection::Backward,
            search: search.clone(),
            search_ids: search_ids.clone(),
            is_monitored: params.is_monitored,
            include_no_books: params.include_no_books,
            format: params.format.clone(),
            limit: half_limit + 1, // +1 to detect if there are more
        };
        let before_rows = match authors_repo::list_with_filters(&state.pool, &before_filter).await {
            Ok(rows) => rows,
            Err(e) => {
                tracing::error!(error = %e, "Failed to list authors (before)");
                return (StatusCode::INTERNAL_SERVER_ERROR, "database error").into_response();
            }
        };
        let has_prev = before_rows.len() as i64 > half_limit;
        // Reverse the before rows to get correct order (they come in DESC, we need ASC)
        let before_rows: Vec<_> = before_rows
            .into_iter()
            .take(half_limit as usize)
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
            .collect();

        // Query items AFTER the target (forward order, using target as cursor)
        let after_limit = limit - half_limit - 1; // -1 because we have the target itself
        let after_filter = authors_repo::AuthorListFilter {
            cursor: Some((author_name, author_id)), // Items after this position
            direction: PaginationDirection::Forward,
            search,
            search_ids,
            is_monitored: params.is_monitored,
            include_no_books: params.include_no_books,
            format: params.format.clone(),
            limit: after_limit + 1, // +1 to detect if there are more
        };
        let after_rows = match authors_repo::list_with_filters(&state.pool, &after_filter).await {
            Ok(rows) => rows,
            Err(e) => {
                tracing::error!(error = %e, "Failed to list authors (after)");
                return (StatusCode::INTERNAL_SERVER_ERROR, "database error").into_response();
            }
        };

        let has_next = after_rows.len() as i64 > after_limit;
        let after_rows: Vec<_> = after_rows.into_iter().take(after_limit as usize).collect();

        // Combine: before + target + after
        let mut combined = before_rows;
        combined.push(target_row);
        combined.extend(after_rows);

        let authors: Vec<AuthorSummary> = combined
            .into_iter()
            .map(|row| {
                let cover_urls: Vec<String> = row
                    .sample_book_ids
                    .iter()
                    .map(|id| format!("/api/v1/library/books/{}/cover", id))
                    .collect();
                AuthorSummary {
                    id: row.id,
                    name: row.name,
                    book_count: row.book_count,
                    series_count: row.series_count,
                    total_book_count: row.total_book_count,
                    total_series_count: row.total_series_count,
                    wanted_count: row.wanted_count,
                    cover_urls,
                    identifiers: row.identifiers,
                    image_url: endpoint_url(
                        &row.images,
                        "profile",
                        "/api/v1/library/authors/",
                        row.id,
                        "/image",
                    ),
                    bio: row.bio,
                    is_monitored: row.is_monitored,
                    auto_want_mode: row.auto_want_mode.unwrap_or_else(|| "none".to_string()),
                    known_book_count: row.known_book_count,
                    top_series_names: row.top_series_names,
                    metadata: None,
                }
            })
            .collect();

        let prev_cursor = if has_prev {
            authors.first().map(|a| {
                encode_name_cursor(&NameCursor {
                    id: a.id,
                    name: a.name.clone(),
                })
            })
        } else {
            None
        };

        let next_cursor = if has_next {
            authors.last().map(|a| {
                encode_name_cursor(&NameCursor {
                    id: a.id,
                    name: a.name.clone(),
                })
            })
        } else {
            None
        };

        return AxumJson(PaginatedResponse {
            items: authors,
            next_cursor,
            prev_cursor,
            total_count: None, // Skip count for around queries
        })
        .into_response();
    }

    // Parse direction parameter
    let is_backward = params
        .direction
        .as_deref()
        .map(|d| d.eq_ignore_ascii_case("backward"))
        .unwrap_or(false);
    let direction = if is_backward {
        PaginationDirection::Backward
    } else {
        PaginationDirection::Forward
    };

    // Normal cursor-based pagination
    let cursor = params.cursor.as_deref().and_then(decode_name_cursor);

    let filter = authors_repo::AuthorListFilter {
        cursor: cursor.map(|c| (c.name, c.id)),
        direction,
        search,
        search_ids,
        is_monitored: params.is_monitored,
        include_no_books: params.include_no_books,
        format: params.format.clone(),
        limit: limit + 1, // +1 to detect if there are more
    };

    // Only count on first page (no cursor)
    let total_count = if params.cursor.is_none() {
        authors_repo::count_with_filters(&state.pool, &filter)
            .await
            .ok()
    } else {
        None
    };

    let rows = match authors_repo::list_with_filters(&state.pool, &filter).await {
        Ok(rows) => rows,
        Err(e) => {
            tracing::error!(error = %e, "Failed to list authors");
            return (StatusCode::INTERNAL_SERVER_ERROR, "database error").into_response();
        }
    };

    // Determine if we're in search mode
    let is_search_mode = params.search.as_ref().is_some_and(|q| !q.trim().is_empty());

    // In search mode, use search_has_more from MeiliSearch; otherwise check DB result count
    let has_more = if is_search_mode {
        search_has_more
    } else {
        rows.len() as i64 > limit
    };
    let rows: Vec<_> = rows.into_iter().take(limit as usize).collect();

    // For backward pagination, reverse the results to get correct display order
    let rows: Vec<_> = if is_backward {
        rows.into_iter().rev().collect()
    } else {
        rows
    };

    let authors: Vec<AuthorSummary> = rows
        .into_iter()
        .map(|row| {
            let cover_urls: Vec<String> = row
                .sample_book_ids
                .iter()
                .map(|id| format!("/api/v1/library/books/{}/cover", id))
                .collect();
            AuthorSummary {
                id: row.id,
                name: row.name,
                book_count: row.book_count,
                series_count: row.series_count,
                total_book_count: row.total_book_count,
                total_series_count: row.total_series_count,
                wanted_count: row.wanted_count,
                cover_urls,
                identifiers: row.identifiers,
                image_url: endpoint_url(
                    &row.images,
                    "profile",
                    "/api/v1/library/authors/",
                    row.id,
                    "/image",
                ),
                bio: row.bio,
                is_monitored: row.is_monitored,
                auto_want_mode: row.auto_want_mode.unwrap_or_else(|| "none".to_string()),
                known_book_count: row.known_book_count,
                top_series_names: row.top_series_names,
                metadata: None,
            }
        })
        .collect();

    // For search mode, use offset-based cursors; otherwise use name-based cursors
    // For backward pagination, the cursor points to the first item (to get more older items)
    // For forward pagination, the cursor points to the last item (to get more newer items)
    let (next_cursor, prev_cursor) = if is_search_mode {
        // Search mode: use offset-based pagination
        let next = if has_more {
            Some(format!("search_offset:{}", search_offset + limit as usize))
        } else {
            None
        };
        let prev = if search_offset > 0 {
            Some(format!(
                "search_offset:{}",
                search_offset.saturating_sub(limit as usize)
            ))
        } else {
            None
        };
        (next, prev)
    } else if is_backward {
        let prev = if has_more {
            authors.first().map(|a| {
                encode_name_cursor(&NameCursor {
                    id: a.id,
                    name: a.name.clone(),
                })
            })
        } else {
            None
        };
        (None, prev)
    } else {
        let next = if has_more {
            authors.last().map(|a| {
                encode_name_cursor(&NameCursor {
                    id: a.id,
                    name: a.name.clone(),
                })
            })
        } else {
            None
        };
        (next, None)
    };

    AxumJson(PaginatedResponse {
        items: authors,
        next_cursor,
        prev_cursor,
        total_count,
    })
    .into_response()
}

pub async fn create_author(
    State(state): State<AppState>,
    auth: AuthUser,
    Json(payload): Json<AuthorCreateRequest>,
) -> impl IntoResponse {
    if let Err(e) = auth.require_contributor() {
        return e.into_response();
    }
    let name = payload.name.trim();
    if name.is_empty() {
        return (StatusCode::BAD_REQUEST, "name required").into_response();
    }

    // Check before moving identifiers into input
    let has_hardcover = payload
        .identifiers
        .as_ref()
        .and_then(|v| v.get("hardcover"))
        .is_some();
    let is_monitored = payload.is_monitored.unwrap_or(false);

    // Use repository function that handles all fields
    let images = payload.image_url.map(|url| {
        let mut m = std::collections::HashMap::new();
        m.insert(
            "profile".to_string(),
            ImageEntry {
                source_url: Some(url),
                cached_at: None,
            },
        );
        m
    });
    let input = AuthorInput {
        name: name.to_string(),
        identifiers: payload.identifiers,
        images,
        bio: payload.bio,
        is_monitored: payload.is_monitored,
        auto_want_mode: payload.auto_want_mode,
        known_book_count: None,
    };

    match authors_repo::upsert(&state.pool, input).await {
        Ok(author_id) => {
            record_event(
                &state,
                "author.create",
                None,
                "ok",
                Some(name.to_string()),
                None,
            )
            .await;
            // Trigger search index update
            let _ = state
                .library_events
                .send(LibraryEvent::AuthorUpdated { author_id });

            // Enqueue enrichment if monitored or has a hardcover ID
            if is_monitored || has_hardcover {
                let _ = state
                    .nats
                    .publisher()
                    .enqueue_job(
                        "kobbler",
                        "author_enrich",
                        serde_json::json!({ "author_id": author_id }),
                    )
                    .await;
                tracing::info!(author_id = %author_id, name = %name, "Author created, enrichment job queued");
            }

            AxumJson(serde_json::json!({ "id": author_id })).into_response()
        }
        Err(e) => {
            tracing::error!(author = %name, error = %e, "failed to create author");
            (StatusCode::INTERNAL_SERVER_ERROR, "failed to create author").into_response()
        }
    }
}

pub async fn rename_author(
    State(state): State<AppState>,
    auth: AuthUser,
    Path(author_id): Path<String>,
    Json(payload): Json<AuthorRenameRequest>,
) -> impl IntoResponse {
    if let Err(e) = auth.require_contributor() {
        return e.into_response();
    }
    let author_id = match Uuid::parse_str(&author_id) {
        Ok(id) => id,
        Err(_) => return (StatusCode::BAD_REQUEST, "invalid author id").into_response(),
    };
    let name = payload.name.trim();
    if name.is_empty() {
        return (StatusCode::BAD_REQUEST, "name required").into_response();
    }

    match authors_repo::rename(&state.pool, author_id, name).await {
        Ok(true) => {
            record_event(
                &state,
                "author.rename",
                None,
                "ok",
                Some(name.to_string()),
                None,
            )
            .await;
            // Trigger search index update for author and affected books
            let _ = state
                .library_events
                .send(LibraryEvent::AuthorUpdated { author_id });
            // Books also need reindexing since they store author names
            if let Ok(book_ids) = authors_repo::get_book_ids(&state.pool, author_id).await {
                for book_id in book_ids {
                    let _ = state
                        .library_events
                        .send(LibraryEvent::library_updated(Some(book_id)));
                }
            }
            StatusCode::NO_CONTENT.into_response()
        }
        Ok(false) => (StatusCode::NOT_FOUND, "author not found").into_response(),
        Err(_) => (StatusCode::CONFLICT, "author name exists").into_response(),
    }
}

/// PUT /api/v1/library/authors/{author_id}
/// Update all fields for an author
pub async fn update_author(
    State(state): State<AppState>,
    auth: AuthUser,
    Path(author_id): Path<String>,
    Json(payload): Json<AuthorUpdateRequest>,
) -> impl IntoResponse {
    if let Err(e) = auth.require_contributor() {
        return e.into_response();
    }
    let author_id = match Uuid::parse_str(&author_id) {
        Ok(id) => id,
        Err(_) => return (StatusCode::BAD_REQUEST, "invalid author id").into_response(),
    };

    // Get current author to fill in missing fields
    let current = match authors_repo::get_by_id(&state.pool, author_id).await {
        Ok(Some(author)) => author,
        Ok(None) => return (StatusCode::NOT_FOUND, "author not found").into_response(),
        Err(e) => {
            tracing::error!(error = %e, "failed to get author");
            return (StatusCode::INTERNAL_SERVER_ERROR, "database error").into_response();
        }
    };

    // Check if hardcover ID is being newly added (before moving payload fields)
    let had_hardcover = current.identifiers.get("hardcover").is_some();
    let new_has_hardcover = payload
        .identifiers
        .as_ref()
        .and_then(|v| v.get("hardcover"))
        .is_some();

    // Build input with provided values or current values
    let name = payload.name.unwrap_or(current.name);
    let images = if let Some(url) = payload.image_url {
        let mut m = std::collections::HashMap::new();
        m.insert(
            "profile".to_string(),
            ImageEntry {
                source_url: Some(url),
                cached_at: None,
            },
        );
        Some(m)
    } else if !current.images.is_empty() {
        Some(current.images)
    } else {
        None
    };
    let input = AuthorInput {
        name: name.clone(),
        identifiers: payload.identifiers,
        images,
        bio: payload.bio.or(current.bio),
        is_monitored: payload.is_monitored.or(Some(current.is_monitored)),
        auto_want_mode: payload.auto_want_mode.or(current.auto_want_mode),
        known_book_count: current.known_book_count,
    };

    match authors_repo::update_metadata(&state.pool, author_id, input).await {
        Ok(()) => {
            record_event(
                &state,
                "author.update",
                None,
                "ok",
                Some(name.clone()),
                None,
            )
            .await;
            // Trigger search index update for author and affected books
            let _ = state
                .library_events
                .send(LibraryEvent::AuthorUpdated { author_id });
            // Books also need reindexing since they store author names
            if let Ok(book_ids) = authors_repo::get_book_ids(&state.pool, author_id).await {
                for book_id in book_ids {
                    let _ = state
                        .library_events
                        .send(LibraryEvent::library_updated(Some(book_id)));
                }
            }

            // If hardcover ID was just added, enqueue enrichment
            if !had_hardcover && new_has_hardcover {
                let _ = state
                    .nats
                    .publisher()
                    .enqueue_job(
                        "kobbler",
                        "author_enrich",
                        serde_json::json!({ "author_id": author_id }),
                    )
                    .await;
                tracing::info!(author_id = %author_id, name = %name, "Hardcover ID added, enrichment job queued");
            }

            StatusCode::NO_CONTENT.into_response()
        }
        Err(e) => {
            tracing::error!(error = %e, "failed to update author");
            (StatusCode::INTERNAL_SERVER_ERROR, "failed to update author").into_response()
        }
    }
}

pub async fn merge_authors(
    State(state): State<AppState>,
    auth: AuthUser,
    Json(payload): Json<AuthorMergeRequest>,
) -> impl IntoResponse {
    if let Err(e) = auth.require_contributor() {
        return e.into_response();
    }
    if payload.source_id == payload.target_id {
        return (StatusCode::BAD_REQUEST, "source and target must differ").into_response();
    }

    let target_name = match authors_repo::get_name(&state.pool, payload.target_id).await {
        Ok(Some(name)) => name,
        Ok(None) => return (StatusCode::NOT_FOUND, "target not found").into_response(),
        Err(e) => {
            tracing::error!(error = %e, "failed to get target author name");
            return (StatusCode::INTERNAL_SERVER_ERROR, "database error").into_response();
        }
    };

    let mut tx = match state.pool.begin().await {
        Ok(tx) => tx,
        Err(e) => {
            tracing::error!(error = %e, "failed to begin transaction for author merge");
            return (StatusCode::INTERNAL_SERVER_ERROR, "database error").into_response();
        }
    };

    if let Err(e) =
        authors_repo::copy_book_links(&mut *tx, payload.source_id, payload.target_id).await
    {
        tracing::error!(error = %e, source_id = %payload.source_id, target_id = %payload.target_id, "failed to copy book authors in merge");
        return (StatusCode::INTERNAL_SERVER_ERROR, "failed to merge authors").into_response();
    }

    if let Err(e) = authors_repo::unlink_all_books(&mut *tx, payload.source_id).await {
        tracing::error!(error = %e, source_id = %payload.source_id, "failed to delete source book authors in merge");
        return (StatusCode::INTERNAL_SERVER_ERROR, "failed to merge authors").into_response();
    }

    if let Err(e) = authors_repo::delete(&mut *tx, payload.source_id).await {
        tracing::error!(error = %e, source_id = %payload.source_id, "failed to delete source author in merge");
        return (StatusCode::INTERNAL_SERVER_ERROR, "failed to merge authors").into_response();
    }

    // Get affected book IDs before commit (they were linked to source)
    let affected_book_ids = authors_repo::get_book_ids(&state.pool, payload.target_id)
        .await
        .unwrap_or_default();

    if let Err(e) = tx.commit().await {
        tracing::error!(error = %e, "failed to commit author merge transaction");
        return (StatusCode::INTERNAL_SERVER_ERROR, "database error").into_response();
    }

    record_event(
        &state,
        "author.merge",
        None,
        "ok",
        Some(target_name.clone()),
        None,
    )
    .await;
    record_activity(
        &state.pool,
        "merged",
        Some("author"),
        Some(payload.target_id),
        &target_name,
        Some("Authors merged"),
        None,
        None,
        Some(auth.user_id),
    )
    .await;

    // Trigger search index updates:
    // 1. Source author will be deleted from index (index call handles missing entity)
    let _ = state.library_events.send(LibraryEvent::AuthorUpdated {
        author_id: payload.source_id,
    });
    // 2. Target author needs reindex
    let _ = state.library_events.send(LibraryEvent::AuthorUpdated {
        author_id: payload.target_id,
    });
    // 3. Affected books need reindex (author_ids changed)
    for book_id in affected_book_ids {
        let _ = state
            .library_events
            .send(LibraryEvent::library_updated(Some(book_id)));
    }

    StatusCode::NO_CONTENT.into_response()
}

pub async fn delete_author(
    State(state): State<AppState>,
    auth: AuthUser,
    Path(author_id): Path<String>,
) -> impl IntoResponse {
    if let Err(e) = auth.require_contributor() {
        return e.into_response();
    }
    let author_id = match Uuid::parse_str(&author_id) {
        Ok(id) => id,
        Err(_) => return (StatusCode::BAD_REQUEST, "invalid author id").into_response(),
    };

    let name = match authors_repo::get_name(&state.pool, author_id).await {
        Ok(Some(name)) => name,
        Ok(None) => return (StatusCode::NOT_FOUND, "not found").into_response(),
        Err(e) => {
            tracing::error!(error = %e, "failed to get author name");
            return (StatusCode::INTERNAL_SERVER_ERROR, "database error").into_response();
        }
    };

    // Get affected book IDs before deletion
    let affected_book_ids = authors_repo::get_book_ids(&state.pool, author_id)
        .await
        .unwrap_or_default();

    let mut tx = match state.pool.begin().await {
        Ok(tx) => tx,
        Err(e) => {
            tracing::error!(error = %e, "failed to begin transaction for author delete");
            return (StatusCode::INTERNAL_SERVER_ERROR, "database error").into_response();
        }
    };

    // Remove author from books (delete book_authors entries)
    if let Err(e) = authors_repo::unlink_all_books(&mut *tx, author_id).await {
        tracing::error!(error = %e, author_id = %author_id, "failed to delete book_authors for author delete");
        return (StatusCode::INTERNAL_SERVER_ERROR, "failed to delete author").into_response();
    }

    // Delete the author
    if let Err(e) = authors_repo::delete(&mut *tx, author_id).await {
        tracing::error!(error = %e, author_id = %author_id, "failed to delete author");
        return (StatusCode::INTERNAL_SERVER_ERROR, "failed to delete author").into_response();
    }

    if let Err(e) = tx.commit().await {
        tracing::error!(error = %e, "failed to commit author delete transaction");
        return (StatusCode::INTERNAL_SERVER_ERROR, "database error").into_response();
    }

    record_event(&state, "author.delete", None, "ok", Some(name), None).await;

    // Trigger search index updates:
    // 1. Author will be deleted from index (index call handles missing entity)
    let _ = state
        .library_events
        .send(LibraryEvent::AuthorUpdated { author_id });
    // 2. Affected books need reindex (author_ids changed)
    for book_id in affected_book_ids {
        let _ = state
            .library_events
            .send(LibraryEvent::library_updated(Some(book_id)));
    }

    StatusCode::NO_CONTENT.into_response()
}
