//! Book-related route handlers.

use axum::{
    Json as AxumJson,
    extract::{Json, Path, Query, State},
    http::StatusCode,
    response::IntoResponse,
};
use std::path::PathBuf;
use tokio::fs;
use uuid::Uuid;

use crate::AppState;
use crate::events::LibraryEvent;
use crate::kobload::WantRequest;
use crate::repositories::{
    PaginationDirection, audiobook_files, authors, book_files, book_ingest_source, books,
    entitlements, genres, ingest_queue, media_root_files, reading_states, series, shelf_items,
};
use crate::routes::AuthUser;
use crate::routes::helpers::build_kobload_client;
use crate::wanted::watch_sync::SOURCE_SYSTEM;

use super::helpers::{
    Cursor, NameCursor, book_cover_url, decode_cursor, decode_name_cursor, encode_cursor,
    encode_name_cursor, extracted_cover_url, fetch_author_ids, normalize_optional_string,
    parse_flexible_date, parse_search_query, record_activity, record_event,
};
use super::types::{
    AudioFileInfo, AudioSummary, AvailableLettersQuery, AvailableLettersResponse,
    BookCreateRequest, BookFileInfo, BookFilesResponse, BookUpdate, DebugQuery, FirstByLetterQuery,
    FirstByLetterResponse, LibraryBook, ListBooksQuery, PaginatedResponse,
};

/// Batch request for book operations
#[derive(serde::Deserialize)]
pub struct BookBatchRequest {
    pub book_ids: Vec<Uuid>,
}

pub async fn list_books(
    State(state): State<AppState>,
    auth: AuthUser,
    Query(params): Query<ListBooksQuery>,
) -> impl IntoResponse {
    let runtime_config = state.runtime_config.get().await;
    let limit = params
        .limit
        .unwrap_or(runtime_config.default_page_size)
        .min(runtime_config.max_page_size);
    let filter_type = params.filter.as_deref().unwrap_or("all").to_string();

    // Backward compat: has_audiobook=true maps to format=audiobook if format not set
    let format_filter = params.format.clone().or_else(|| {
        if params.has_audiobook == Some(true) {
            Some("audiobook".to_string())
        } else {
            None
        }
    });

    // Parse sort parameter - defaults to title (alphabetical)
    let sort_field = params
        .sort
        .as_deref()
        .map(books::BookSortField::parse)
        .unwrap_or(books::BookSortField::Title);

    // Try MeiliSearch first for typo-tolerant search, fall back to ILIKE
    // When searching, we use offset-based pagination with MeiliSearch
    let search_offset = params.search_offset.unwrap_or(0);
    let search_limit = limit as usize + 1; // +1 to detect if there are more results

    // Track if MeiliSearch has more results (for setting next_cursor in search mode)
    let mut search_has_more = false;

    let (search_ids, search_terms) = if let Some(ref query) = params.search {
        if !query.trim().is_empty() {
            // Use search backend for fuzzy search with attribute filters
            use crate::search::BookSearchFilters;
            let filters = BookSearchFilters {
                is_in_library: params.in_library,
                want_status: params.want_status.clone(),
                exclude_ingest_queue: true, // Library pages should never show ingest queue items
                format: format_filter.clone(),
            };
            match state
                .search
                .search_books(query, filters, search_limit, search_offset)
                .await
            {
                Ok(mut ids) => {
                    // Check if MeiliSearch has more results
                    search_has_more = ids.len() > limit as usize;
                    // Truncate to limit for database query
                    ids.truncate(limit as usize);
                    (Some(ids), vec![])
                }
                Err(e) => {
                    // Fall back to ILIKE search on error
                    tracing::warn!(error = %e, "Search backend failed, falling back to ILIKE");
                    let terms = parse_search_query(query)
                        .into_iter()
                        .map(|t| match t {
                            super::helpers::SearchTerm::Include(pattern) => books::SearchTerm {
                                pattern,
                                negated: false,
                            },
                            super::helpers::SearchTerm::Exclude(pattern) => books::SearchTerm {
                                pattern,
                                negated: true,
                            },
                        })
                        .collect();
                    (None, terms)
                }
            }
        } else {
            (None, vec![])
        }
    } else {
        (None, vec![])
    };

    // Handle "around" parameter for bidirectional pagination (jump to specific book)
    if let Some(ref around_id) = params.around {
        let book_id = match Uuid::parse_str(around_id) {
            Ok(id) => id,
            Err(_) => return (StatusCode::BAD_REQUEST, "invalid around id").into_response(),
        };

        // Get the book's data to use for combining with before/after rows
        let target_row = match books::get_by_id_for_user(&state.pool, book_id, auth.user_id).await {
            Ok(Some(book)) => book,
            Ok(None) => return (StatusCode::NOT_FOUND, "book not found").into_response(),
            Err(e) => {
                tracing::error!(error = %e, "Failed to get book");
                return (StatusCode::INTERNAL_SERVER_ERROR, "database error").into_response();
            }
        };

        // Query items BEFORE the target (earlier in sort order, backward direction)
        // around_before controls how many items appear before the target.
        // Default (None) = centered (half of limit). Set to small value for letter navigation.
        let before_limit = params
            .around_before
            .unwrap_or(limit / 2)
            .min(limit - 1)
            .max(0);
        let cursor = match sort_field {
            books::BookSortField::Title => {
                books::BookCursor::Title(book_sort_key(&target_row), book_id)
            }
            books::BookSortField::CreatedAt => {
                books::BookCursor::CreatedAt(target_row.created_at, book_id)
            }
        };
        let before_filter = books::BookListFilter {
            user_id: auth.user_id,
            cursor: Some(cursor.clone()),
            sort_field,
            direction: PaginationDirection::Backward,
            search_terms: search_terms.clone(),
            search_ids: search_ids.clone(),
            in_library: params.in_library,
            want_status: params.want_status.clone(),
            filter_type: filter_type.clone(),
            has_audiobook: params.has_audiobook,
            format: format_filter.clone(),
            orphaned: params.orphaned,
            limit: before_limit + 1, // +1 to detect if there are more
        };
        let before_rows = match books::list_with_filters(&state.pool, &before_filter).await {
            Ok(rows) => rows,
            Err(e) => {
                tracing::error!(error = %e, "Failed to list books (before)");
                return (StatusCode::INTERNAL_SERVER_ERROR, "database error").into_response();
            }
        };
        let has_prev = before_rows.len() as i64 > before_limit;
        // Reverse the before rows to get correct order (alphabetical), and take only before_limit
        let before_rows: Vec<_> = before_rows
            .into_iter()
            .take(before_limit as usize)
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
            .collect();

        // Query items AFTER the target (later in sort order, forward direction)
        // Query for +1 to detect if there are more items
        let after_limit = limit - before_limit - 1; // -1 because we have the target itself
        let after_filter = books::BookListFilter {
            user_id: auth.user_id,
            cursor: Some(cursor), // Items after this position
            sort_field,
            direction: PaginationDirection::Forward,
            search_terms: search_terms.clone(),
            search_ids: search_ids.clone(),
            in_library: params.in_library,
            want_status: params.want_status.clone(),
            filter_type: filter_type.clone(),
            has_audiobook: params.has_audiobook,
            format: format_filter.clone(),
            orphaned: params.orphaned,
            limit: after_limit + 1, // +1 to detect if there are more
        };
        let after_rows = match books::list_with_filters(&state.pool, &after_filter).await {
            Ok(rows) => rows,
            Err(e) => {
                tracing::error!(error = %e, "Failed to list books (after)");
                return (StatusCode::INTERNAL_SERVER_ERROR, "database error").into_response();
            }
        };

        let has_next = after_rows.len() as i64 > after_limit;
        let after_rows: Vec<_> = after_rows.into_iter().take(after_limit as usize).collect();

        // Combine: before + target + after
        let mut combined = before_rows;
        combined.push(target_row);
        combined.extend(after_rows);

        // Cursors are encoded from the rows: LibraryBook drops the sort key.
        let window_first_cursor = combined.first().map(|r| encode_row_cursor(r, sort_field));
        let window_last_cursor = combined.last().map(|r| encode_row_cursor(r, sort_field));

        let book_ids: Vec<_> = combined.iter().map(|r| r.id).collect();
        let mut shelf_map =
            shelf_items::list_shelves_for_books(&state.pool, auth.user_id, &book_ids)
                .await
                .unwrap_or_default();

        let books_list: Vec<LibraryBook> = combined
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

        let prev_cursor = if has_prev { window_first_cursor } else { None };
        let next_cursor = if has_next { window_last_cursor } else { None };

        return AxumJson(PaginatedResponse {
            items: books_list,
            next_cursor,
            prev_cursor,
            total_count: None,
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
    // Decode cursor based on sort field
    let cursor = params.cursor.as_deref().and_then(|c| match sort_field {
        books::BookSortField::Title => {
            decode_name_cursor(c).map(|nc| books::BookCursor::Title(nc.name, nc.id))
        }
        books::BookSortField::CreatedAt => {
            decode_cursor(c).map(|tc| books::BookCursor::CreatedAt(tc.ts, tc.id))
        }
    });
    let is_first_page = cursor.is_none();

    let filter = books::BookListFilter {
        user_id: auth.user_id,
        cursor,
        sort_field,
        direction,
        search_terms,
        search_ids,
        in_library: params.in_library,
        want_status: params.want_status.clone(),
        filter_type,
        has_audiobook: params.has_audiobook,
        format: format_filter,
        orphaned: params.orphaned,
        limit: limit + 1, // +1 to detect if there are more
    };

    // Only count on first page (no cursor)
    let total_count = if is_first_page {
        books::count_with_filters(&state.pool, &filter).await.ok()
    } else {
        None
    };

    let rows = match books::list_with_filters(&state.pool, &filter).await {
        Ok(rows) => rows,
        Err(e) => {
            tracing::error!(error = %e, "Failed to list books");
            return (StatusCode::INTERNAL_SERVER_ERROR, "database error").into_response();
        }
    };

    // When searching, use MeiliSearch's has_more; otherwise use database row count
    let is_search_mode = params.search.as_ref().is_some_and(|q| !q.trim().is_empty());
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

    // Cursors are encoded from the rows: LibraryBook drops the sort key.
    let page_first_cursor = rows.first().map(|r| encode_row_cursor(r, sort_field));
    let page_last_cursor = rows.last().map(|r| encode_row_cursor(r, sort_field));

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

    // For backward pagination, the cursor points to the first item (earliest in sort order)
    // For forward pagination, the cursor points to the last item (latest in sort order)
    // For search pagination, we use offset-based pagination (cursor is next offset)
    let (next_cursor, prev_cursor) = if is_search_mode {
        // Search mode: return next offset as cursor marker (frontend tracks its own offset but
        // uses next_cursor presence to know if there are more results)
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
        // Going backward: prev_cursor points to first item (to get more items earlier in sort order)
        let prev = if has_more { page_first_cursor } else { None };
        (None, prev)
    } else {
        // Going forward: next_cursor points to last item (to get more items later in sort order)
        let next = if has_more { page_last_cursor } else { None };
        (next, None)
    };

    AxumJson(PaginatedResponse {
        items: books,
        next_cursor,
        prev_cursor,
        total_count,
    })
    .into_response()
}

pub async fn get_book(
    State(state): State<AppState>,
    auth: AuthUser,
    Path(book_id): Path<String>,
    Query(params): Query<DebugQuery>,
) -> impl IntoResponse {
    let book_id = match Uuid::parse_str(&book_id) {
        Ok(id) => id,
        Err(_) => return (StatusCode::BAD_REQUEST, "invalid book id").into_response(),
    };

    match books::get_by_id_for_user(&state.pool, book_id, auth.user_id).await {
        Ok(Some(row)) => {
            let provider_metadata = if params.debug.unwrap_or(false) {
                let meta = row.provider_metadata;
                if meta.as_object().is_some_and(|o| !o.is_empty()) {
                    Some(meta)
                } else {
                    None
                }
            } else {
                None
            };

            // Fetch all series memberships for this book
            let series_memberships =
                crate::repositories::book_series::list_for_book(&state.pool, row.id)
                    .await
                    .ok();

            let shelves = shelf_items::list_shelves_for_books(&state.pool, auth.user_id, &[row.id])
                .await
                .unwrap_or_default()
                .remove(&row.id)
                .unwrap_or_default();

            let book = LibraryBook {
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
                provider_metadata,
                narrator: row.narrator,
                duration_seconds: row.duration_seconds,
                is_abridged: row.is_abridged,
                want_format: row.want_format,
                has_audiobook: row.has_audiobook,
                media_root_id: row.media_root_id,
                series_memberships,
            };
            AxumJson(book).into_response()
        }
        Ok(None) => (StatusCode::NOT_FOUND, "book not found").into_response(),
        Err(e) => {
            tracing::error!(error = %e, "Failed to get book");
            (StatusCode::INTERNAL_SERVER_ERROR, "database error").into_response()
        }
    }
}

pub async fn update_book(
    State(state): State<AppState>,
    auth: AuthUser,
    Path(book_id): Path<String>,
    Json(payload): Json<BookUpdate>,
) -> impl IntoResponse {
    if let Err(e) = auth.require_contributor() {
        return e.into_response();
    }
    let book_id = match Uuid::parse_str(&book_id) {
        Ok(id) => id,
        Err(_) => return (StatusCode::BAD_REQUEST, "invalid book id").into_response(),
    };

    let existing = match books::get_for_edit(&state.pool, book_id).await {
        Ok(Some(data)) => data,
        Ok(None) => return (StatusCode::NOT_FOUND, "book not found").into_response(),
        Err(e) => {
            tracing::error!(error = %e, "Failed to fetch book for edit");
            return (StatusCode::INTERNAL_SERVER_ERROR, "database error").into_response();
        }
    };

    let is_ingest = existing.ingest_status.is_some();

    let existing_title = existing.title;
    let existing_description = existing.description;
    let existing_language = existing.language;
    let existing_publisher = existing.publisher;
    let existing_pubdate = existing.pubdate;
    let existing_series_id = existing.series_id;
    let existing_series_index = existing.series_index;
    let existing_identifiers = existing.identifiers;
    let existing_page_count = existing.page_count;
    let existing_rating = existing.rating;
    let existing_rating_count = existing.rating_count;

    let title = payload
        .title
        .and_then(|t| {
            let trimmed = t.trim();
            if trimmed.is_empty() {
                None
            } else {
                Some(trimmed.to_string())
            }
        })
        .unwrap_or(existing_title);
    let description = normalize_optional_string(payload.description).or(existing_description);
    let language = normalize_optional_string(payload.language).or(existing_language);
    let publisher = normalize_optional_string(payload.publisher).or(existing_publisher);

    // Authors: ingest items use find_by_name (pending if not found),
    // library books use get_or_create (always creates)
    let mut author_ids = if let Some(author_list) = payload.authors.clone() {
        if let Err(e) = authors::clear_book_authors(&state.pool, book_id).await {
            tracing::error!(error = %e, book_id = %book_id, "failed to delete book authors");
        }

        let mut ids = Vec::new();
        let mut pending_authors = Vec::new();

        for author_input in author_list {
            let author_name = author_input.name.trim();
            if author_name.is_empty() {
                continue;
            }

            if is_ingest {
                // Ingest items: link existing authors, store unknown in pending_authors
                if let Ok(Some((author_id, _))) =
                    authors::find_by_name(&state.pool, author_name).await
                {
                    if let Err(e) = authors::link_book(&state.pool, book_id, author_id).await {
                        tracing::error!(error = %e, book_id = %book_id, author_id = %author_id, "failed to link book to author");
                    }
                    ids.push(author_id);
                } else {
                    let mut author_json = serde_json::json!({"name": author_name});
                    if let Some(ref hardcover_id) = author_input.hardcover_id {
                        author_json["hardcover_id"] = serde_json::json!(hardcover_id);
                    }
                    pending_authors.push(author_json);
                }
            } else {
                // Library books: always create if not found
                match authors::get_or_create(
                    &state.pool,
                    author_name,
                    author_input.hardcover_id.as_deref(),
                )
                .await
                {
                    Ok(author_id) => {
                        if let Err(e) = authors::link_book(&state.pool, book_id, author_id).await {
                            tracing::error!(error = %e, book_id = %book_id, author_id = %author_id, "failed to link book to author");
                        }
                        ids.push(author_id);
                    }
                    Err(e) => {
                        tracing::error!(error = %e, author = %author_name, "failed to get or create author");
                    }
                }
            }
        }

        // Store pending authors in ingest source (always set — empty array clears previous)
        if is_ingest {
            let _ = book_ingest_source::update(
                &state.pool,
                book_id,
                book_ingest_source::BookIngestSourceUpdate {
                    pending_authors: Some(serde_json::json!(pending_authors)),
                    ..Default::default()
                },
            )
            .await;
        }

        ids
    } else {
        fetch_author_ids(&state, book_id).await
    };

    author_ids.sort();
    author_ids.dedup();

    // Series: ingest items use case-insensitive lookup (pending if not found),
    // library books use get_or_create_with_author_overlap (always creates)
    let mut series_id = payload.series_id.or(existing_series_id);
    if let Some(raw) = payload.series_name {
        let normalized = series::normalize_name(Some(raw));
        let series_identifiers = payload
            .series_hardcover_id
            .as_deref()
            .filter(|s| !s.is_empty())
            .map(|hc_id| serde_json::json!({"hardcover": hc_id}));

        if is_ingest {
            // Ingest items: case-insensitive lookup, store in pending if not found
            match normalized {
                Some(name) => {
                    // Migration 0031 made series names non-unique on purpose, so
                    // resolve to the oldest, matching series::crud. Trailing id
                    // makes the order total; created_at is not unique.
                    let existing_series: Option<Uuid> = sqlx::query_scalar!(
                        "SELECT id FROM series WHERE LOWER(name) = LOWER($1)
                         ORDER BY created_at ASC, id ASC LIMIT 1",
                        &name,
                    )
                    .fetch_optional(&state.pool)
                    .await
                    .ok()
                    .flatten();

                    if let Some(sid) = existing_series {
                        series_id = Some(sid);
                    } else {
                        // Store in pending_series for creation on approval
                        series_id = None;
                        let mut series_json = serde_json::json!({"name": name});
                        if let Some(ref hc_id) = series_identifiers
                            && let Some(hc_val) = hc_id.get("hardcover")
                        {
                            series_json["hardcover_id"] = hc_val.clone();
                        }
                        let pending_series_index = payload.series_index;
                        let _ = book_ingest_source::update(
                            &state.pool,
                            book_id,
                            book_ingest_source::BookIngestSourceUpdate {
                                pending_series: Some(series_json),
                                pending_series_index,
                                ..Default::default()
                            },
                        )
                        .await;
                    }
                }
                None => {
                    series_id = None;
                    // Clear pending series
                    let _ = book_ingest_source::update(
                        &state.pool,
                        book_id,
                        book_ingest_source::BookIngestSourceUpdate {
                            pending_series: Some(serde_json::json!(null)),
                            ..Default::default()
                        },
                    )
                    .await;
                }
            }
        } else {
            series_id = match normalized {
                Some(name) => series::get_or_create_with_author_overlap(
                    &state.pool,
                    &name,
                    &author_ids,
                    series_identifiers,
                )
                .await
                .ok()
                .flatten(),
                None => None,
            };
        }
    }
    let mut series_index = payload.series_index.or(existing_series_index);
    if series_id.is_none() {
        series_index = None;
    }

    let pubdate = match payload.pubdate {
        Some(raw) => {
            let trimmed = raw.trim();
            if trimmed.is_empty() {
                None
            } else {
                match parse_flexible_date(trimmed) {
                    Some(dt) => Some(dt),
                    None => {
                        return (
                            StatusCode::BAD_REQUEST,
                            Json(serde_json::json!({"message": "Invalid publication date"})),
                        )
                            .into_response();
                    }
                }
            }
        }
        None => existing_pubdate,
    };

    let identifiers = payload.identifiers.unwrap_or(existing_identifiers);
    let page_count = payload.page_count.or(existing_page_count);
    let rating = payload.rating.or(existing_rating);
    let rating_count = payload.rating_count.or(existing_rating_count);

    // Build update using repository
    let update = books::BookUpdate {
        title: Some(title.clone()),
        description,
        language,
        publisher,
        pubdate,
        identifiers: Some(identifiers),
        page_count,
        rating,
        rating_count,
        narrator: normalize_optional_string(payload.narrator),
        is_abridged: payload.is_abridged,
        want_format: normalize_optional_string(payload.want_format),
        ..Default::default()
    };

    if let Err(e) = books::update(&state.pool, book_id, update).await {
        tracing::error!(error = %e, book_id = %book_id, "failed to update book");
        return (StatusCode::INTERNAL_SERVER_ERROR, "failed to update book").into_response();
    }

    // Update series via junction table
    // series_memberships (bulk replace) takes precedence over single series_id/series_name
    if let Some(memberships) = payload.series_memberships {
        let entries: Vec<(uuid::Uuid, Option<f64>, bool)> = memberships
            .iter()
            .map(|m| (m.series_id, m.series_index, m.is_primary))
            .collect();
        if let Err(e) =
            crate::repositories::book_series::set_all(&state.pool, book_id, &entries).await
        {
            tracing::error!(error = %e, book_id = %book_id, "failed to set book series memberships");
        }
    } else if (series_id != existing_series_id || series_index != existing_series_index)
        && let Err(e) = books::set_series(&state.pool, book_id, series_id, series_index).await
    {
        tracing::error!(error = %e, book_id = %book_id, "failed to update book series");
    }

    // Mark as user-edited to prevent background jobs from overwriting
    // (only for library books, not ingest queue items)
    if !is_ingest && let Err(e) = books::mark_user_edited(&state.pool, book_id).await {
        tracing::warn!(error = %e, book_id = %book_id, "failed to mark book as user-edited");
    }

    // Handle genres if provided
    if let Some(genre_list) = payload.genres
        && let Err(e) = genres::set_book_genres(&state.pool, book_id, &genre_list).await
    {
        tracing::error!(error = %e, book_id = %book_id, "failed to set book genres");
    }

    // Handle cover URL: for ingest items, download and store as extracted_cover bytes
    if is_ingest {
        if let Some(ref cover_url) = payload.cover_url
            && !cover_url.is_empty()
        {
            match reqwest::get(cover_url).await {
                Ok(resp) if resp.status().is_success() => match resp.bytes().await {
                    Ok(bytes) => {
                        if let Err(e) =
                            ingest_queue::update_cover(&state.pool, book_id, &bytes).await
                        {
                            tracing::warn!(error = %e, "Failed to store fetched cover");
                        }
                    }
                    Err(e) => {
                        tracing::warn!(error = %e, url = %cover_url, "Failed to read cover bytes");
                    }
                },
                Ok(resp) => {
                    tracing::warn!(status = %resp.status(), url = %cover_url, "Cover URL returned non-success status");
                }
                Err(e) => {
                    tracing::warn!(error = %e, url = %cover_url, "Failed to fetch cover URL");
                }
            }
        }
    } else if let Some(ref cover_url) = payload.cover_url {
        // For library books, store the URL in the images JSONB
        let mut m = std::collections::HashMap::new();
        m.insert(
            "cover".to_string(),
            crate::images::ImageEntry {
                source_url: Some(cover_url.clone()),
                cached_at: None,
            },
        );
        let _ = books::update(
            &state.pool,
            book_id,
            books::BookUpdate {
                images: Some(m),
                ..Default::default()
            },
        )
        .await;
    }

    // Update source_metadata on ingest source if provided
    if is_ingest && let Some(source_metadata) = payload.source_metadata {
        let _ = book_ingest_source::update(
            &state.pool,
            book_id,
            book_ingest_source::BookIngestSourceUpdate {
                source_metadata: Some(source_metadata),
                ..Default::default()
            },
        )
        .await;
    }

    // Queue metadata write job only for library books with ebook files
    // (ingest items don't have ebook files yet)
    if !is_ingest {
        let has_ebook = book_files::has_ebook_format(&state.pool, book_id)
            .await
            .unwrap_or(false);

        if has_ebook {
            let display = crate::jobs::fetch_job_display(&state.pool, "book", book_id).await;
            let context = nats_jobs::JobContext::for_entity(
                "book",
                book_id,
                format!("Writing metadata to '{}'", title),
            )
            .with_display_opt(display);
            if let Err(e) = crate::jobs::enqueue(
                &state.pool,
                state.nats.publisher(),
                "metadata_write",
                serde_json::json!({ "book_id": book_id.to_string() }),
                context,
            )
            .await
            {
                tracing::warn!(error = %e, book_id = %book_id, "Failed to enqueue metadata_write job");
            }
        }
    }

    record_event(&state, "metadata.update", Some(book_id), "ok", None, None).await;
    let _ = state
        .library_events
        .send(LibraryEvent::library_updated(Some(book_id)));
    StatusCode::NO_CONTENT.into_response()
}

/// Set the primary series for a book
pub async fn set_book_primary_series(
    State(state): State<AppState>,
    auth: AuthUser,
    Path(book_id): Path<String>,
    Json(payload): Json<serde_json::Value>,
) -> impl IntoResponse {
    if let Err(e) = auth.require_contributor() {
        return e.into_response();
    }
    let Ok(book_id) = book_id.parse::<uuid::Uuid>() else {
        return (StatusCode::BAD_REQUEST, "invalid book id").into_response();
    };
    let Some(series_id_str) = payload.get("series_id").and_then(|v| v.as_str()) else {
        return (StatusCode::BAD_REQUEST, "series_id required").into_response();
    };
    let Ok(series_id) = series_id_str.parse::<uuid::Uuid>() else {
        return (StatusCode::BAD_REQUEST, "invalid series_id").into_response();
    };
    if let Err(e) =
        crate::repositories::book_series::set_primary(&state.pool, book_id, series_id).await
    {
        tracing::error!(error = %e, "Failed to set primary series");
        return (
            StatusCode::INTERNAL_SERVER_ERROR,
            "failed to set primary series",
        )
            .into_response();
    }
    StatusCode::NO_CONTENT.into_response()
}

/// Delete a book and all associated data
pub async fn delete_book(
    State(state): State<AppState>,
    auth: AuthUser,
    Path(book_id): Path<String>,
) -> impl IntoResponse {
    if let Err(e) = auth.require_contributor() {
        return e.into_response();
    }
    let book_id = match Uuid::parse_str(&book_id) {
        Ok(id) => id,
        Err(_) => return (StatusCode::BAD_REQUEST, "invalid book id").into_response(),
    };

    // Check if book exists and get want_status + title for cleanup / activity log
    let (want_status, book_title): (Option<String>, String) = match sqlx::query!(
        "SELECT want_status, title FROM books WHERE id = $1",
        book_id
    )
    .fetch_optional(&state.pool)
    .await
    {
        Ok(Some(row)) => (Some(row.want_status), row.title),
        Ok(None) => return (StatusCode::NOT_FOUND, "book not found").into_response(),
        Err(e) => {
            tracing::error!(error = %e, "failed to check book existence");
            return (StatusCode::INTERNAL_SERVER_ERROR, "database error").into_response();
        }
    };

    // Clean up watch patterns for books with active want statuses
    if matches!(
        want_status.as_deref(),
        Some("wanted" | "searching" | "downloading" | "watching")
    ) {
        let delete = nats_jobs::WatchPatternDelete {
            source_system: "kobbler".to_string(),
            source_id: book_id.to_string(),
        };
        if let Ok(payload) = serde_json::to_value(&delete) {
            let _ = state
                .nats
                .publisher()
                .enqueue_job("kobload", "watch_pattern_delete", payload)
                .await;
            tracing::info!(book_id = %book_id, "Watch pattern delete queued on book delete");
        }
    }

    // Start a transaction to delete all related data
    let mut tx = match state.pool.begin().await {
        Ok(tx) => tx,
        Err(e) => {
            tracing::error!(error = %e, "failed to begin transaction for delete book");
            return (StatusCode::INTERNAL_SERVER_ERROR, "database error").into_response();
        }
    };

    // Delete all related data in the correct order (respecting foreign key constraints)
    // Note: shelf_items, entitlements, reading_states, book_authors, book_files reference books

    // Delete shelf items
    if let Err(e) = shelf_items::delete_by_book(&mut *tx, book_id).await {
        tracing::error!(error = %e, book_id = %book_id, "failed to delete shelf items");
        let _ = tx.rollback().await;
        return (StatusCode::INTERNAL_SERVER_ERROR, "failed to delete book").into_response();
    }

    // Delete entitlements
    if let Err(e) = entitlements::delete_by_book(&mut *tx, book_id).await {
        tracing::error!(error = %e, book_id = %book_id, "failed to delete entitlements");
        let _ = tx.rollback().await;
        return (StatusCode::INTERNAL_SERVER_ERROR, "failed to delete book").into_response();
    }

    // Delete reading states
    if let Err(e) = reading_states::delete_by_book(&mut *tx, book_id).await {
        tracing::error!(error = %e, book_id = %book_id, "failed to delete reading states");
        let _ = tx.rollback().await;
        return (StatusCode::INTERNAL_SERVER_ERROR, "failed to delete book").into_response();
    }

    // Delete book authors
    if let Err(e) = authors::clear_book_authors(&mut *tx, book_id).await {
        tracing::error!(error = %e, book_id = %book_id, "failed to delete book authors");
        let _ = tx.rollback().await;
        return (StatusCode::INTERNAL_SERVER_ERROR, "failed to delete book").into_response();
    }

    // Delete book files
    if let Err(e) = book_files::delete_by_book(&mut *tx, book_id).await {
        tracing::error!(error = %e, book_id = %book_id, "failed to delete book files");
        let _ = tx.rollback().await;
        return (StatusCode::INTERNAL_SERVER_ERROR, "failed to delete book").into_response();
    }

    // Delete the book itself
    if let Err(e) = books::delete(&mut *tx, book_id).await {
        tracing::error!(error = %e, book_id = %book_id, "failed to delete book");
        let _ = tx.rollback().await;
        return (StatusCode::INTERNAL_SERVER_ERROR, "failed to delete book").into_response();
    }

    // Commit the transaction
    if let Err(e) = tx.commit().await {
        tracing::error!(error = %e, book_id = %book_id, "failed to commit delete transaction");
        return (StatusCode::INTERNAL_SERVER_ERROR, "failed to delete book").into_response();
    }

    // Delete cover from blob storage if it exists
    let covers_dir = PathBuf::from("data/covers");
    let cover_path = covers_dir.join(format!("{}.jpg", book_id));
    if cover_path.exists()
        && let Err(e) = fs::remove_file(&cover_path).await
    {
        tracing::warn!(error = %e, book_id = %book_id, "failed to delete cover file");
        // Continue anyway - cover deletion is not critical
    }

    tracing::info!(book_id = %book_id, "deleted book and all associated data");
    record_event(&state, "book.delete", Some(book_id), "ok", None, None).await;
    record_activity(
        &state.pool,
        "deleted",
        Some("book"),
        Some(book_id),
        &book_title,
        None,
        None,
        None,
        Some(auth.user_id),
    )
    .await;

    // Broadcast library updated event with book_id so search index deletes the entry
    let _ = state
        .library_events
        .send(LibraryEvent::library_updated(Some(book_id)));

    StatusCode::NO_CONTENT.into_response()
}

pub async fn touch_books(
    State(state): State<AppState>,
    auth: AuthUser,
    Json(payload): Json<BookBatchRequest>,
) -> impl IntoResponse {
    if let Err(e) = auth.require_contributor() {
        return e.into_response();
    }
    if payload.book_ids.is_empty() {
        return (StatusCode::BAD_REQUEST, "book_ids required").into_response();
    }

    let mut tx = match state.pool.begin().await {
        Ok(tx) => tx,
        Err(e) => {
            tracing::error!(error = %e, "failed to begin transaction for touch books");
            return (StatusCode::INTERNAL_SERVER_ERROR, "database error").into_response();
        }
    };

    if let Err(e) = books::touch_many(&mut *tx, &payload.book_ids).await {
        tracing::error!(error = %e, "failed to update books timestamp");
        return (StatusCode::INTERNAL_SERVER_ERROR, "failed to touch books").into_response();
    }

    if let Err(e) = entitlements::touch_by_books(&mut *tx, &payload.book_ids).await {
        tracing::error!(error = %e, "failed to update entitlements timestamp");
        return (StatusCode::INTERNAL_SERVER_ERROR, "failed to touch books").into_response();
    }

    if let Err(e) = tx.commit().await {
        tracing::error!(error = %e, "failed to commit touch books transaction");
        return (StatusCode::INTERNAL_SERVER_ERROR, "database error").into_response();
    }

    for book_id in &payload.book_ids {
        record_event(&state, "metadata.touch", Some(*book_id), "ok", None, None).await;
        let _ = state
            .library_events
            .send(LibraryEvent::library_updated(Some(*book_id)));
    }

    StatusCode::NO_CONTENT.into_response()
}

/// GET /api/v1/library/books/{book_id}/files
/// Get all files for a book
pub async fn get_book_files(
    State(state): State<AppState>,
    _auth: AuthUser,
    Path(book_id): Path<Uuid>,
) -> impl IntoResponse {
    // Verify the book exists
    let exists = books::exists(&state.pool, book_id).await.unwrap_or(false);

    if !exists {
        return (StatusCode::NOT_FOUND, "book not found").into_response();
    }

    let file_list = match book_files::list_by_book(&state.pool, book_id).await {
        Ok(list) => list,
        Err(e) => {
            tracing::error!(error = %e, "Failed to fetch book files");
            return (StatusCode::INTERNAL_SERVER_ERROR, "database error").into_response();
        }
    };

    let files: Vec<BookFileInfo> = file_list
        .into_iter()
        .map(|f| {
            let filename = std::path::Path::new(&f.file_path)
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or(&f.file_path)
                .to_string();
            BookFileInfo {
                id: f.id,
                format: f.format,
                size_bytes: Some(f.size_bytes),
                filename,
                file_path: f.file_path,
                source_type: f.source_type,
                source_metadata: f.source_metadata,
                added_at: f.added_at,
                available: f.available,
            }
        })
        .collect();

    // Fetch audiobook files
    let (audio_files, audio_summary) =
        match audiobook_files::list_by_book(&state.pool, book_id).await {
            Ok(af_list) if !af_list.is_empty() => {
                let count = af_list.len();
                let total_size: i64 = af_list.iter().map(|f| f.file_size).sum();
                let total_dur: f64 = af_list.iter().filter_map(|f| f.duration_seconds).sum();
                let format = af_list
                    .first()
                    .map(|f| f.format.clone())
                    .unwrap_or_default();
                let audio_files: Vec<AudioFileInfo> = af_list
                    .into_iter()
                    .map(|f| {
                        let filename = std::path::Path::new(&f.file_path)
                            .file_name()
                            .and_then(|n| n.to_str())
                            .unwrap_or(&f.file_path)
                            .to_string();
                        AudioFileInfo {
                            id: f.id,
                            file_index: f.file_index,
                            format: f.format,
                            filename,
                            file_size: f.file_size,
                            duration_seconds: f.duration_seconds,
                            available: f.available,
                        }
                    })
                    .collect();
                let summary = AudioSummary {
                    count,
                    total_size,
                    total_duration_seconds: if total_dur > 0.0 {
                        Some(total_dur)
                    } else {
                        None
                    },
                    format,
                };
                (audio_files, Some(summary))
            }
            _ => (vec![], None),
        };

    Json(BookFilesResponse {
        files,
        audio_files,
        audio_summary,
    })
    .into_response()
}

/// DELETE /api/v1/library/books/{book_id}/files/{file_id}
/// Delete a specific book file
pub async fn delete_book_file(
    State(state): State<AppState>,
    auth: AuthUser,
    Path((book_id, file_id)): Path<(Uuid, Uuid)>,
) -> impl IntoResponse {
    if let Err(e) = auth.require_contributor() {
        return e.into_response();
    }
    // Verify the file exists and belongs to the book
    let file_info = match book_files::get_by_id_for_book(&state.pool, file_id, book_id).await {
        Ok(Some(info)) => info,
        Ok(None) => return (StatusCode::NOT_FOUND, "file not found").into_response(),
        Err(e) => {
            tracing::error!(error = %e, "Failed to fetch file");
            return (StatusCode::INTERNAL_SERVER_ERROR, "database error").into_response();
        }
    };

    let source_path = file_info.source_path;
    let file_path = file_info.file_path;

    // Start transaction
    let mut tx = match state.pool.begin().await {
        Ok(tx) => tx,
        Err(e) => {
            tracing::error!(error = %e, "Failed to begin transaction");
            return (StatusCode::INTERNAL_SERVER_ERROR, "database error").into_response();
        }
    };

    // Delete the file record
    if let Err(e) = book_files::delete(&mut *tx, file_id).await {
        tracing::error!(error = %e, file_id = %file_id, "Failed to delete file record");
        return (StatusCode::INTERNAL_SERVER_ERROR, "failed to delete file").into_response();
    }

    // Commit transaction
    if let Err(e) = tx.commit().await {
        tracing::error!(error = %e, "Failed to commit transaction");
        return (StatusCode::INTERNAL_SERVER_ERROR, "database error").into_response();
    }

    // Try to delete the physical file(s) - non-critical if this fails
    for path in [&source_path, &file_path].into_iter().flatten() {
        let file_path_buf = PathBuf::from(path);
        if file_path_buf.exists()
            && let Err(e) = fs::remove_file(&file_path_buf).await
        {
            tracing::warn!(error = %e, path = %path, "Failed to delete physical file");
        }
    }

    tracing::info!(book_id = %book_id, file_id = %file_id, "Deleted book file");
    record_event(
        &state,
        "file.delete",
        Some(book_id),
        "ok",
        Some(file_id.to_string()),
        None,
    )
    .await;

    // Broadcast library updated event
    let _ = state
        .library_events
        .send(LibraryEvent::library_updated(Some(book_id)));

    StatusCode::NO_CONTENT.into_response()
}

// =============================================================================
// Move Book File
// =============================================================================

#[derive(serde::Deserialize)]
pub struct MoveBookFileBody {
    pub target_book_id: Uuid,
}

/// POST /api/v1/library/books/{book_id}/files/{file_id}/move
pub async fn move_book_file(
    State(state): State<AppState>,
    _auth: AuthUser,
    Path((book_id, file_id)): Path<(Uuid, Uuid)>,
    Json(body): Json<MoveBookFileBody>,
) -> impl IntoResponse {
    let _ = book_id; // for route grouping; move is by file_id

    // Verify target book exists
    match books::get_by_id(&state.pool, body.target_book_id).await {
        Ok(Some(_)) => {}
        Ok(None) => return (StatusCode::NOT_FOUND, "target book not found").into_response(),
        Err(e) => {
            tracing::error!(error = %e, "Failed to check target book");
            return (StatusCode::INTERNAL_SERVER_ERROR, "database error").into_response();
        }
    }

    // Look up the file path before moving (needed for sidecar cleanup)
    let file_path = book_files::get_path_by_id(&state.pool, file_id)
        .await
        .ok()
        .flatten();

    match book_files::move_to_book(&state.pool, file_id, body.target_book_id).await {
        Ok(true) => {
            // Sync media_root_files entity_id so sidecar writes use the correct book
            if let Some(ref fp) = file_path
                && let Err(e) =
                    media_root_files::reassign_entity_by_path(&state.pool, fp, body.target_book_id)
                        .await
            {
                tracing::warn!(error = %e, "Failed to update media_root_files entity_id");
            }

            // Clean up stale sidecars from the old directory if no other book files remain
            if let Some(ref fp) = file_path {
                let dir = std::path::Path::new(fp).parent();
                if let Some(dir) = dir {
                    let dir_str = dir.to_string_lossy().to_string();
                    if let Ok(0) = book_files::count_in_directory(&state.pool, &dir_str).await {
                        for name in ["metadata.json", "metadata.opf", "cover.jpg"] {
                            let sidecar = dir.join(name);
                            if let Err(e) = fs::remove_file(&sidecar).await
                                && e.kind() != std::io::ErrorKind::NotFound
                            {
                                tracing::warn!(
                                    path = %sidecar.display(),
                                    error = %e,
                                    "Failed to remove stale sidecar"
                                );
                            }
                        }
                    }
                }
            }

            StatusCode::NO_CONTENT.into_response()
        }
        Ok(false) => (StatusCode::NOT_FOUND, "book file not found").into_response(),
        Err(e) => {
            tracing::error!(error = %e, "Failed to move book file");
            (StatusCode::INTERNAL_SERVER_ERROR, "failed to move file").into_response()
        }
    }
}

/// Encode a pagination cursor from a list row.
///
/// Title cursors must carry `COALESCE(sort_title, title)` — the expression the query
/// orders and compares on. Encoding the raw title instead skips or repeats books whose
/// title starts with an article.
fn encode_row_cursor(row: &books::LibraryBookRow, sort_field: books::BookSortField) -> String {
    match sort_field {
        books::BookSortField::Title => encode_name_cursor(&NameCursor {
            id: row.id,
            name: book_sort_key(row),
        }),
        books::BookSortField::CreatedAt => encode_cursor(&Cursor {
            id: row.id,
            ts: row.created_at,
        }),
    }
}

/// The value title-ordered book queries sort on.
fn book_sort_key(row: &books::LibraryBookRow) -> String {
    row.sort_title.clone().unwrap_or_else(|| row.title.clone())
}

/// GET /api/v1/library/books/first-by-letter
/// Find the first book starting with a given letter (for alphabet bar jump navigation).
/// Returns both ID and title so the frontend can create a cursor for forward pagination.
pub async fn first_by_letter(
    State(state): State<AppState>,
    auth: AuthUser,
    Query(params): Query<FirstByLetterQuery>,
) -> impl IntoResponse {
    let letter = params.letter.trim().to_uppercase();
    if letter.is_empty() {
        return (StatusCode::BAD_REQUEST, "letter parameter required").into_response();
    }

    // Validate letter is A-Z or #
    if letter != "#" && (letter.len() != 1 || !letter.chars().next().unwrap().is_ascii_alphabetic())
    {
        return (StatusCode::BAD_REQUEST, "letter must be A-Z or #").into_response();
    }

    let filter_type = params.filter.as_deref().unwrap_or("all");

    match books::first_by_letter(
        &state.pool,
        &letter,
        params.in_library,
        filter_type,
        params.format.as_deref(),
        auth.user_id,
    )
    .await
    {
        Ok(Some(result)) => AxumJson(FirstByLetterResponse {
            id: result.id,
            title: result.title,
        })
        .into_response(),
        Ok(None) => (StatusCode::NOT_FOUND, "no books found for letter").into_response(),
        Err(e) => {
            tracing::error!(error = %e, letter = %letter, "Failed to find first book by letter");
            (StatusCode::INTERNAL_SERVER_ERROR, "database error").into_response()
        }
    }
}

/// GET /api/v1/library/books/available-letters
/// Get letters that have at least one book (for alphabet bar).
pub async fn available_letters(
    State(state): State<AppState>,
    _auth: AuthUser,
    Query(params): Query<AvailableLettersQuery>,
) -> impl IntoResponse {
    use crate::search::BookSearchFilters;

    let filters = BookSearchFilters {
        is_in_library: params.in_library,
        want_status: None,
        exclude_ingest_queue: true, // Library pages should never show ingest queue items
        format: params.format.clone(),
    };

    match state.search.get_available_book_letters(filters).await {
        Ok(letters) => AxumJson(AvailableLettersResponse { letters }).into_response(),
        // The bar is furniture, not an answer to a question: losing the search index
        // should cost typo tolerance, not make every library page's alphabet vanish.
        Err(e) => {
            tracing::debug!(error = %e, "Book letters from search unavailable, using SQL");
            let mut conditions: Vec<&'static str> = vec!["b.ingest_status IS NULL"];
            match params.in_library {
                Some(true) => conditions.push(books::IN_LIBRARY),
                Some(false) => conditions.push(books::NOT_IN_LIBRARY),
                None => {}
            }
            if let Some(condition) = params.format.as_deref().and_then(books::format_condition) {
                conditions.push(condition);
            }
            match crate::repositories::letters::available_letters(
                &state.pool,
                "books b",
                &conditions,
            )
            .await
            {
                Ok(letters) => AxumJson(AvailableLettersResponse { letters }).into_response(),
                Err(e) => {
                    tracing::error!(error = %e, "Failed to get available letters");
                    (StatusCode::INTERNAL_SERVER_ERROR, "database error").into_response()
                }
            }
        }
    }
}

/// GET /api/v1/library/books/facets
/// Contextual genre + want_status counts for drill-down filtering.
pub async fn facets(
    State(state): State<AppState>,
    _auth: AuthUser,
    Query(params): Query<AvailableLettersQuery>,
) -> impl IntoResponse {
    use crate::search::BookSearchFilters;

    let filters = BookSearchFilters {
        is_in_library: params.in_library,
        want_status: None,
        exclude_ingest_queue: true,
        format: params.format.clone(),
    };

    match state.search.get_book_facets(filters).await {
        Ok(facets) => AxumJson(facets).into_response(),
        Err(e) => {
            tracing::error!(error = %e, "Failed to get book facets from search");
            (StatusCode::INTERNAL_SERVER_ERROR, "search error").into_response()
        }
    }
}

/// POST /api/v1/library/books
/// Create a new book (wanted or library book without files)
pub async fn create_book(
    State(state): State<AppState>,
    auth: AuthUser,
    Json(payload): Json<BookCreateRequest>,
) -> impl IntoResponse {
    if let Err(e) = auth.require_contributor() {
        return e.into_response();
    }
    let title = payload.title.trim();
    if title.is_empty() {
        return (StatusCode::BAD_REQUEST, "title required").into_response();
    }

    // Get author IDs first (needed for series overlap detection)
    let mut author_ids = Vec::new();
    if let Some(ref author_list) = payload.authors {
        for author_input in author_list {
            let author_name = author_input.name.trim();
            if author_name.is_empty() {
                continue;
            }
            match authors::get_or_create(
                &state.pool,
                author_name,
                author_input.hardcover_id.as_deref(),
            )
            .await
            {
                Ok(id) => author_ids.push(id),
                Err(e) => {
                    tracing::error!(error = %e, author = %author_name, "Failed to get/create author");
                }
            }
        }
    }

    // Handle series - prefer series_id (from dropdown), fall back to series_name (typed/created)
    let series_id = if let Some(id) = payload.series_id {
        // Direct ID selection from dropdown - use as-is
        Some(id)
    } else if let Some(ref series_name) = payload.series_name {
        let series_name = series::normalize_name(Some(series_name.clone()));
        let series_identifiers = payload
            .series_hardcover_id
            .as_deref()
            .filter(|s| !s.is_empty())
            .map(|hc_id| serde_json::json!({"hardcover": hc_id}));
        if let Some(name) = series_name {
            match series::get_or_create_with_author_overlap(
                &state.pool,
                &name,
                &author_ids,
                series_identifiers,
            )
            .await
            {
                Ok(Some(id)) => Some(id),
                Ok(None) => None,
                Err(e) => {
                    tracing::error!(error = %e, "Failed to get/create series");
                    None
                }
            }
        } else {
            None
        }
    } else {
        None
    };

    // Parse pubdate if provided
    let pubdate = if let Some(ref raw) = payload.pubdate {
        let trimmed = raw.trim();
        if trimmed.is_empty() {
            None
        } else {
            parse_flexible_date(trimmed)
        }
    } else {
        None
    };

    // Build images map from cover_url if provided
    let images = payload.cover_url.as_ref().map(|url| {
        let mut m = std::collections::HashMap::new();
        m.insert(
            "cover".to_string(),
            crate::images::ImageEntry {
                source_url: Some(url.clone()),
                cached_at: None,
            },
        );
        m
    });

    // Create the book
    let book_id = if payload.want {
        // Create as wanted book (is_in_library=false, want_status='wanted')
        let input = books::DiscoveredBookInput {
            title: title.to_string(),
            description: payload.description.clone(),
            identifiers: payload.identifiers.clone(),
            images: images.clone(),
            release_date: None,
            is_released: Some(true),
            want_status: Some("wanted".to_string()),
            discovered_from_author_id: None,
            page_count: payload.page_count,
            rating: payload.rating,
            rating_count: payload.rating_count,
            provider_metadata: None,
            narrator: payload.narrator.clone(),
            want_format: payload.want_format.clone(),
        };
        match books::create_discovered_book(&state.pool, input).await {
            Ok(id) => id,
            Err(e) => {
                tracing::error!(error = %e, "Failed to create wanted book");
                return (StatusCode::INTERNAL_SERVER_ERROR, "failed to create book")
                    .into_response();
            }
        }
    } else {
        // Create as library book without files
        let input = books::BookInput {
            id: None,
            title: title.to_string(),
            description: payload.description.clone(),
            language: payload.language.clone(),
            publisher: payload.publisher.clone(),
            pubdate,
            identifiers: payload.identifiers.clone(),
            page_count: payload.page_count,
            rating: payload.rating,
            rating_count: payload.rating_count,
            images,
            narrator: payload.narrator.clone(),
            want_format: payload.want_format.clone(),
            is_abridged: payload.is_abridged,
        };
        match books::upsert_library_book(&state.pool, input).await {
            Ok(id) => id,
            Err(e) => {
                tracing::error!(error = %e, "Failed to create library book");
                return (StatusCode::INTERNAL_SERVER_ERROR, "failed to create book")
                    .into_response();
            }
        }
    };

    // Link to series via junction table
    if let Some(sid) = series_id
        && let Err(e) = crate::repositories::book_series::add(
            &state.pool,
            book_id,
            sid,
            payload.series_index,
            true,
        )
        .await
    {
        tracing::error!(error = %e, "Failed to link book to series");
    }

    // Link authors to the book
    for author_id in &author_ids {
        if let Err(e) = authors::link_book(&state.pool, book_id, *author_id).await {
            tracing::error!(error = %e, author_id = %author_id, "Failed to link book to author");
        }
    }

    // Set genres if provided
    if let Some(ref genre_list) = payload.genres
        && let Err(e) = genres::set_book_genres(&state.pool, book_id, genre_list).await
    {
        tracing::error!(error = %e, "Failed to set book genres");
    }

    // Set is_abridged if specified (wanted books don't include it in the insert)
    if let Some(true) = payload.is_abridged {
        let update = books::BookUpdate {
            is_abridged: Some(true),
            ..Default::default()
        };
        if let Err(e) = books::update(&state.pool, book_id, update).await {
            tracing::warn!(error = %e, book_id = %book_id, "Failed to set is_abridged on new book");
        }
    }

    // Download cover from URL if provided
    if let Some(ref url) = payload.cover_url
        && !url.is_empty()
    {
        let client = crate::http::new_client();
        if let Err(e) =
            crate::routes::library::covers::save_cover_from_url(&client, book_id, url).await
        {
            tracing::warn!(error = %e, book_id = %book_id, "Failed to download cover from URL");
        }
    }

    // If this is a wanted book, call Kobload to search+watch for it
    if payload.want {
        let runtime = state.runtime_config.get().await;

        // Only proceed if Kobload is configured
        if let Ok(kobload_client) = build_kobload_client(&runtime) {
            let min_score = runtime.wanted_min_score.unwrap_or(0.65);

            // Use ingest URL directly so downloaded files are processed
            let callback_url = runtime.kobload_ingest_url.clone().unwrap_or_default();

            if callback_url.is_empty() {
                tracing::warn!(
                    book_id = %book_id,
                    "Cannot search Kobload for wanted book: ingest URL not configured"
                );
            }

            if !callback_url.is_empty() {
                // Get author name for pattern generation
                let author_name = if let Some(ref authors) = payload.authors {
                    authors.first().map(|a| a.name.clone())
                } else {
                    None
                };

                // Get series info
                let series_name = payload.series_name.clone();
                let series_index = payload.series_index;

                // Extract ISBNs from identifiers
                let isbn10 = payload
                    .identifiers
                    .as_ref()
                    .and_then(|ids| ids.get("isbn10"))
                    .and_then(|v| v.as_str())
                    .map(String::from);
                let isbn13 = payload
                    .identifiers
                    .as_ref()
                    .and_then(|ids| ids.get("isbn13"))
                    .and_then(|v| v.as_str())
                    .map(String::from);

                let hardcover_id = payload
                    .identifiers
                    .as_ref()
                    .and_then(|ids| ids.get("hardcover"))
                    .and_then(|v| v.as_str())
                    .map(String::from);
                let alternate_titles = crate::wanted::book_titles::book_alternate_titles(
                    &crate::metadata::ProviderRegistry::new(state.pool.clone()),
                    &crate::wanted::book_titles::BookRef {
                        title,
                        series: series_name.as_deref(),
                        position: series_index,
                        hardcover_id: hardcover_id.as_deref(),
                    },
                    crate::wanted::book_titles::Lookup::Fetch,
                )
                .await;
                let identifiers = nats_jobs::BookIdentifiers {
                    isbn10,
                    isbn13,
                    hardcover_id,
                    goodreads_id: None,
                    openlibrary_id: None,
                };

                let media_want = nats_jobs::MediaWant::Book {
                    title: title.to_string(),
                    author: author_name.clone(),
                    isbn: identifiers.isbn13.clone().or(identifiers.isbn10.clone()),
                    format_preferences: crate::routes::ingest::EBOOK_EXTENSIONS
                        .iter()
                        .map(|s| (*s).to_string())
                        .collect(),
                    identifiers: Some(identifiers),
                    series: series_name.clone(),
                    series_position: series_index.map(|idx| idx.to_string()),
                    release_date: None,
                    alternate_titles,
                };

                let metadata = serde_json::json!({
                    "book_id": book_id,
                    "title": title,
                    "author": author_name,
                    "series": series_name,
                    "series_index": series_index,
                });

                let want_request = WantRequest {
                    source_system: SOURCE_SYSTEM.to_string(),
                    source_id: book_id.to_string(),
                    callback_url: callback_url.clone(),
                    auth_token: None,
                    query: None,
                    patterns: Vec::new(),
                    media_want: Some(media_want),
                    match_targets: Vec::new(),
                    match_mode: "fuzzy".to_string(),
                    min_score,
                    early_exit_score: 0.95,
                    media_type: None,
                    auto_download: true,
                    limit: 50,
                    upgrade_allowed: Some(false),
                    manual: false,
                    metadata: Some(metadata),
                    excluded_releases: None,
                    search_not_before: None,
                };

                // Call Kobload want API (don't fail the whole request if this fails)
                match kobload_client.want_content(&want_request).await {
                    Ok(response) => {
                        tracing::info!(
                            book_id = %book_id,
                            watch_pattern_id = %response.watch_pattern_id,
                            status = ?response.status,
                            download_id = ?response.download_id,
                            results_found = response.results_found,
                            best_score = response.best_score,
                            "Kobload want request completed"
                        );
                    }
                    Err(e) => {
                        tracing::warn!(
                            error = %e,
                            book_id = %book_id,
                            "Failed to create Kobload want request (book was still created)"
                        );
                    }
                }
            }
        } else {
            tracing::info!(book_id = %book_id, "Skipping Kobload want request: Kobload not configured");
        }
    }

    // Record event
    record_event(
        &state,
        "book.create",
        Some(book_id),
        "ok",
        Some(title.to_string()),
        None,
    )
    .await;

    // Broadcast library update
    let _ = state
        .library_events
        .send(LibraryEvent::library_updated(Some(book_id)));

    // Clean up Kobload watch pattern if this book entered the library (not wanted)
    if !payload.want {
        crate::wanted::on_book_acquired(&state.runtime_config, &state.library_events, book_id)
            .await;
    }

    AxumJson(serde_json::json!({ "id": book_id })).into_response()
}
