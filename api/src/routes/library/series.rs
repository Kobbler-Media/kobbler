//! Series-related route handlers.

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
    PaginationDirection, books, series as series_repo, series::SeriesInput, shelf_items,
};
use crate::routes::AuthUser;

use super::helpers::{
    NameCursor, book_cover_url, decode_name_cursor, encode_name_cursor, extracted_cover_url,
    record_activity, record_event,
};
use super::types::{
    DebugQuery, LibraryBook, ListSeriesQuery, PaginatedResponse, SeriesCreateRequest,
    SeriesMergeRequest, SeriesRenameRequest, SeriesReorderRequest, SeriesSummary,
    SeriesUpdateRequest,
};

#[derive(Debug, Deserialize)]
pub struct ListSeriesBooksQuery {
    /// Include books not in library (discovered from series tracking)
    #[serde(default)]
    pub include_not_owned: bool,
}

pub async fn get_series(
    State(state): State<AppState>,
    _auth: AuthUser,
    Path(series_id): Path<String>,
    Query(params): Query<DebugQuery>,
) -> impl IntoResponse {
    let series_id = match Uuid::parse_str(&series_id) {
        Ok(id) => id,
        Err(_) => return (StatusCode::BAD_REQUEST, "invalid series id").into_response(),
    };

    match series_repo::get_by_id(&state.pool, series_id).await {
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
            AxumJson(SeriesSummary {
                id: row.id,
                name: row.name,
                updated_at: row.updated_at,
                book_count: row.book_count,
                wanted_count: row.wanted_count,
                authors: row.authors,
                cover_urls,
                identifiers: row.identifiers.unwrap_or(serde_json::json!({})),
                image_url: endpoint_url(
                    &row.images,
                    "cover",
                    "/api/v1/library/series/",
                    row.id,
                    "/image",
                ),
                description: row.description,
                auto_want_mode: row.auto_want_mode.unwrap_or_else(|| "none".to_string()),
                total_book_count: row.total_book_count,
                is_monitored: row.is_monitored,
                monitoring_started_at: row.monitoring_started_at,
                metadata,
            })
            .into_response()
        }
        Ok(None) => (StatusCode::NOT_FOUND, "series not found").into_response(),
        Err(e) => {
            tracing::error!(error = %e, "Failed to get series");
            (StatusCode::INTERNAL_SERVER_ERROR, "database error").into_response()
        }
    }
}

pub async fn list_series(
    State(state): State<AppState>,
    _auth: AuthUser,
    Query(params): Query<ListSeriesQuery>,
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

    // When format filter is active, skip MeiliSearch (it can't filter series by book format)
    // and fall back to ILIKE search which works with the SQL-level format filter.
    let use_meili = params.format.is_none();
    let (search_ids, search) = if let Some(ref query) = params.search {
        if !query.trim().is_empty() {
            if use_meili {
                // Use search backend for fuzzy search with attribute filters
                use crate::search::SeriesSearchFilters;
                let filters = SeriesSearchFilters {
                    is_monitored: params.is_monitored,
                };
                match state
                    .search
                    .search_series(query, filters, search_limit, search_offset)
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

    // Handle "around" parameter for bidirectional pagination (jump to specific series)
    if let Some(ref around_id) = params.around {
        let series_id = match Uuid::parse_str(around_id) {
            Ok(id) => id,
            Err(_) => return (StatusCode::BAD_REQUEST, "invalid around id").into_response(),
        };

        // Get the series name to use as cursor position
        let series_name = match series_repo::get_name(&state.pool, series_id).await {
            Ok(Some(name)) => name,
            Ok(None) => return (StatusCode::NOT_FOUND, "series not found").into_response(),
            Err(e) => {
                tracing::error!(error = %e, "Failed to get series name");
                return (StatusCode::INTERNAL_SERVER_ERROR, "database error").into_response();
            }
        };

        // Get the target series data
        let target_row = match series_repo::get_by_id(&state.pool, series_id).await {
            Ok(Some(row)) => row,
            Ok(None) => return (StatusCode::NOT_FOUND, "series not found").into_response(),
            Err(e) => {
                tracing::error!(error = %e, "Failed to get series");
                return (StatusCode::INTERNAL_SERVER_ERROR, "database error").into_response();
            }
        };

        // Query items BEFORE the target (backward direction)
        let half_limit = limit / 2;
        let before_filter = series_repo::SeriesListFilter {
            cursor: Some((series_name.clone(), series_id)),
            direction: PaginationDirection::Backward,
            search: search.clone(),
            search_ids: search_ids.clone(),
            is_monitored: params.is_monitored,
            include_no_books: params.include_no_books,
            format: params.format.clone(),
            limit: half_limit + 1, // +1 to detect if there are more
        };
        let before_rows = match series_repo::list_with_filters(&state.pool, &before_filter).await {
            Ok(rows) => rows,
            Err(e) => {
                tracing::error!(error = %e, "Failed to list series (before)");
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
        let after_filter = series_repo::SeriesListFilter {
            cursor: Some((series_name, series_id)), // Items after this position
            direction: PaginationDirection::Forward,
            search,
            search_ids,
            is_monitored: params.is_monitored,
            include_no_books: params.include_no_books,
            format: params.format.clone(),
            limit: after_limit + 1, // +1 to detect if there are more
        };
        let after_rows = match series_repo::list_with_filters(&state.pool, &after_filter).await {
            Ok(rows) => rows,
            Err(e) => {
                tracing::error!(error = %e, "Failed to list series (after)");
                return (StatusCode::INTERNAL_SERVER_ERROR, "database error").into_response();
            }
        };

        let has_next = after_rows.len() as i64 > after_limit;
        let after_rows: Vec<_> = after_rows.into_iter().take(after_limit as usize).collect();

        // Combine: before + target + after
        let mut combined = before_rows;
        combined.push(target_row);
        combined.extend(after_rows);

        let series: Vec<SeriesSummary> = combined
            .into_iter()
            .map(|row| {
                let cover_urls: Vec<String> = row
                    .sample_book_ids
                    .iter()
                    .map(|id| format!("/api/v1/library/books/{}/cover", id))
                    .collect();
                SeriesSummary {
                    id: row.id,
                    name: row.name,
                    updated_at: row.updated_at,
                    book_count: row.book_count,
                    wanted_count: row.wanted_count,
                    authors: row.authors,
                    cover_urls,
                    identifiers: row.identifiers.unwrap_or(serde_json::json!({})),
                    image_url: endpoint_url(
                        &row.images,
                        "cover",
                        "/api/v1/library/series/",
                        row.id,
                        "/image",
                    ),
                    description: row.description,
                    auto_want_mode: row.auto_want_mode.unwrap_or_else(|| "none".to_string()),
                    total_book_count: row.total_book_count,
                    is_monitored: row.is_monitored,
                    monitoring_started_at: row.monitoring_started_at,
                    metadata: None,
                }
            })
            .collect();

        let prev_cursor = if has_prev {
            series.first().map(|s| {
                encode_name_cursor(&NameCursor {
                    id: s.id,
                    name: s.name.clone(),
                })
            })
        } else {
            None
        };

        let next_cursor = if has_next {
            series.last().map(|s| {
                encode_name_cursor(&NameCursor {
                    id: s.id,
                    name: s.name.clone(),
                })
            })
        } else {
            None
        };

        return AxumJson(PaginatedResponse {
            items: series,
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
    let cursor = params.cursor.as_deref().and_then(decode_name_cursor);

    let filter = series_repo::SeriesListFilter {
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
        series_repo::count_with_filters(&state.pool, &filter)
            .await
            .ok()
    } else {
        None
    };

    let rows = match series_repo::list_with_filters(&state.pool, &filter).await {
        Ok(rows) => rows,
        Err(e) => {
            tracing::error!(error = %e, "Failed to list series");
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

    let series: Vec<SeriesSummary> = rows
        .into_iter()
        .map(|row| {
            let cover_urls: Vec<String> = row
                .sample_book_ids
                .iter()
                .map(|id| format!("/api/v1/library/books/{}/cover", id))
                .collect();
            SeriesSummary {
                id: row.id,
                name: row.name,
                updated_at: row.updated_at,
                book_count: row.book_count,
                wanted_count: row.wanted_count,
                authors: row.authors,
                cover_urls,
                identifiers: row.identifiers.unwrap_or(serde_json::json!({})),
                image_url: endpoint_url(
                    &row.images,
                    "cover",
                    "/api/v1/library/series/",
                    row.id,
                    "/image",
                ),
                description: row.description,
                auto_want_mode: row.auto_want_mode.unwrap_or_else(|| "none".to_string()),
                total_book_count: row.total_book_count,
                is_monitored: row.is_monitored,
                monitoring_started_at: row.monitoring_started_at,
                metadata: None,
            }
        })
        .collect();

    // For backward pagination, the cursor points to the first item (to get more older items)
    // For forward pagination, the cursor points to the last item (to get more newer items)
    // For search pagination, we use offset-based pagination (cursor is next offset)
    let (next_cursor, prev_cursor) = if is_search_mode {
        // Search mode: return next offset as cursor marker
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
            series.first().map(|s| {
                encode_name_cursor(&NameCursor {
                    id: s.id,
                    name: s.name.clone(),
                })
            })
        } else {
            None
        };
        (None, prev)
    } else {
        let next = if has_more {
            series.last().map(|s| {
                encode_name_cursor(&NameCursor {
                    id: s.id,
                    name: s.name.clone(),
                })
            })
        } else {
            None
        };
        (next, None)
    };

    AxumJson(PaginatedResponse {
        items: series,
        next_cursor,
        prev_cursor,
        total_count,
    })
    .into_response()
}

pub async fn create_series(
    State(state): State<AppState>,
    auth: AuthUser,
    Json(payload): Json<SeriesCreateRequest>,
) -> impl IntoResponse {
    if let Err(e) = auth.require_contributor() {
        return e.into_response();
    }
    let name = payload.name.trim();
    if name.is_empty() {
        return (StatusCode::BAD_REQUEST, "name required").into_response();
    }

    // Use full SeriesInput with all fields
    let images = payload.image_url.map(|url| {
        let mut m = std::collections::HashMap::new();
        m.insert(
            "cover".to_string(),
            ImageEntry {
                source_url: Some(url),
                cached_at: None,
            },
        );
        m
    });
    let input = SeriesInput {
        name: name.to_string(),
        identifiers: payload.identifiers,
        images,
        description: payload.description,
        total_book_count: payload.total_book_count,
        is_monitored: payload.is_monitored,
        auto_want_mode: payload.auto_want_mode,
    };

    match series_repo::upsert(&state.pool, input).await {
        Ok(series_id) => {
            record_event(
                &state,
                "series.create",
                None,
                "ok",
                Some(name.to_string()),
                None,
            )
            .await;
            // Trigger search index update
            let _ = state
                .library_events
                .send(LibraryEvent::SeriesUpdated { series_id });

            // If monitored, enqueue enrichment job to discover books
            if payload.is_monitored.unwrap_or(false) {
                let _ = state
                    .nats
                    .publisher()
                    .enqueue_job(
                        "kobbler",
                        "series_enrich",
                        serde_json::json!({ "series_id": series_id }),
                    )
                    .await;
                tracing::info!(series_id = %series_id, name = %name, "Series created with monitoring, enrichment job queued");
            }

            AxumJson(serde_json::json!({ "id": series_id })).into_response()
        }
        Err(e) => {
            tracing::error!(error = %e, series = %name, "failed to create series");
            (StatusCode::INTERNAL_SERVER_ERROR, "failed to create series").into_response()
        }
    }
}

pub async fn rename_series(
    State(state): State<AppState>,
    auth: AuthUser,
    Path(series_id): Path<String>,
    Json(payload): Json<SeriesRenameRequest>,
) -> impl IntoResponse {
    if let Err(e) = auth.require_contributor() {
        return e.into_response();
    }
    let series_id = match Uuid::parse_str(&series_id) {
        Ok(id) => id,
        Err(_) => return (StatusCode::BAD_REQUEST, "invalid series id").into_response(),
    };
    let name = payload.name.trim();
    if name.is_empty() {
        return (StatusCode::BAD_REQUEST, "name required").into_response();
    }

    match series_repo::rename(&state.pool, series_id, name).await {
        Ok(true) => {
            record_event(
                &state,
                "series.rename",
                None,
                "ok",
                Some(name.to_string()),
                None,
            )
            .await;
            // Trigger search index update for series and affected books
            let _ = state
                .library_events
                .send(LibraryEvent::SeriesUpdated { series_id });
            // Books also need reindexing since they store series name
            if let Ok(book_ids) = books::get_ids_by_series(&state.pool, series_id).await {
                for book_id in book_ids {
                    let _ = state
                        .library_events
                        .send(LibraryEvent::library_updated(Some(book_id)));
                }
            }
            StatusCode::NO_CONTENT.into_response()
        }
        Ok(false) => (StatusCode::NOT_FOUND, "series not found").into_response(),
        Err(_) => (StatusCode::CONFLICT, "series name exists").into_response(),
    }
}

/// PUT /api/v1/library/series/{series_id}
/// Update all fields for a series
pub async fn update_series(
    State(state): State<AppState>,
    auth: AuthUser,
    Path(series_id): Path<String>,
    Json(payload): Json<SeriesUpdateRequest>,
) -> impl IntoResponse {
    if let Err(e) = auth.require_contributor() {
        return e.into_response();
    }
    let series_id = match Uuid::parse_str(&series_id) {
        Ok(id) => id,
        Err(_) => return (StatusCode::BAD_REQUEST, "invalid series id").into_response(),
    };

    // Get current series to fill in missing fields
    let current = match series_repo::get_by_id(&state.pool, series_id).await {
        Ok(Some(series)) => series,
        Ok(None) => return (StatusCode::NOT_FOUND, "series not found").into_response(),
        Err(e) => {
            tracing::error!(error = %e, "failed to get series");
            return (StatusCode::INTERNAL_SERVER_ERROR, "database error").into_response();
        }
    };

    // Update name if provided (and different from current)
    if let Some(ref new_name) = payload.name {
        let trimmed = new_name.trim();
        if !trimmed.is_empty()
            && trimmed != current.name
            && let Err(e) = series_repo::rename(&state.pool, series_id, trimmed).await
        {
            tracing::error!(error = %e, "failed to rename series");
            return (StatusCode::CONFLICT, "series name exists").into_response();
        }
    }

    // Update metadata (identifiers, images, description, total_book_count)
    let identifiers = payload.identifiers.unwrap_or_else(|| serde_json::json!({}));
    let images = payload.image_url.map(|url| {
        let mut m = std::collections::HashMap::new();
        m.insert(
            "cover".to_string(),
            ImageEntry {
                source_url: Some(url),
                cached_at: None,
            },
        );
        m
    });
    if let Err(e) = series_repo::apply_metadata(
        &state.pool,
        series_id,
        identifiers,
        images,
        payload.description.as_deref(),
        payload.total_book_count,
    )
    .await
    {
        tracing::error!(error = %e, "failed to update series metadata");
        return (StatusCode::INTERNAL_SERVER_ERROR, "failed to update series").into_response();
    }

    // Update monitoring settings
    if (payload.is_monitored.is_some() || payload.auto_want_mode.is_some())
        && let Err(e) = series_repo::update_monitoring(
            &state.pool,
            series_id,
            payload.is_monitored,
            payload.auto_want_mode.as_deref(),
        )
        .await
    {
        tracing::error!(error = %e, "failed to update series monitoring");
        return (StatusCode::INTERNAL_SERVER_ERROR, "failed to update series").into_response();
    }

    let name = payload.name.unwrap_or(current.name);
    record_event(&state, "series.update", None, "ok", Some(name), None).await;

    // Trigger search index update for series and affected books
    let _ = state
        .library_events
        .send(LibraryEvent::SeriesUpdated { series_id });
    if let Ok(book_ids) = books::get_ids_by_series(&state.pool, series_id).await {
        for book_id in book_ids {
            let _ = state
                .library_events
                .send(LibraryEvent::library_updated(Some(book_id)));
        }
    }

    StatusCode::NO_CONTENT.into_response()
}

pub async fn merge_series(
    State(state): State<AppState>,
    auth: AuthUser,
    Json(payload): Json<SeriesMergeRequest>,
) -> impl IntoResponse {
    if let Err(e) = auth.require_contributor() {
        return e.into_response();
    }
    if payload.source_id == payload.target_id {
        return (StatusCode::BAD_REQUEST, "source and target must differ").into_response();
    }

    let target_name = match series_repo::get_name(&state.pool, payload.target_id).await {
        Ok(Some(name)) => name,
        Ok(None) => return (StatusCode::NOT_FOUND, "target not found").into_response(),
        Err(e) => {
            tracing::error!(error = %e, "failed to get target series name");
            return (StatusCode::INTERNAL_SERVER_ERROR, "database error").into_response();
        }
    };

    let mut tx = match state.pool.begin().await {
        Ok(tx) => tx,
        Err(e) => {
            tracing::error!(error = %e, "failed to begin transaction for series merge");
            return (StatusCode::INTERNAL_SERVER_ERROR, "database error").into_response();
        }
    };

    // Get affected book IDs before the move (they may end up in target or stay as-is)
    let source_book_ids = books::get_ids_by_series(&state.pool, payload.source_id)
        .await
        .unwrap_or_default();

    if let Err(e) =
        books::move_books_to_series(&mut *tx, payload.source_id, payload.target_id).await
    {
        tracing::error!(error = %e, source_id = %payload.source_id, target_id = %payload.target_id, "failed to update books in series merge");
        return (StatusCode::INTERNAL_SERVER_ERROR, "failed to merge series").into_response();
    }

    // Delete source series (CASCADE cleans up any remaining junction rows)
    if let Err(e) = series_repo::delete(&mut *tx, payload.source_id).await {
        tracing::error!(error = %e, source_id = %payload.source_id, "failed to delete source series in merge");
        return (StatusCode::INTERNAL_SERVER_ERROR, "failed to merge series").into_response();
    }

    // Auto-select primary for books that came from the source series
    {
        use crate::repositories::book_series;
        for &book_id in &source_book_ids {
            let _ = book_series::auto_select_primary(&mut *tx, book_id, false).await;
        }
    }

    // Get affected book IDs (now in target series)
    let affected_book_ids = books::get_ids_by_series(&mut *tx, payload.target_id)
        .await
        .unwrap_or_default();

    if let Err(e) = tx.commit().await {
        tracing::error!(error = %e, "failed to commit series merge transaction");
        return (StatusCode::INTERNAL_SERVER_ERROR, "database error").into_response();
    }

    record_event(
        &state,
        "series.merge",
        None,
        "ok",
        Some(target_name.clone()),
        None,
    )
    .await;
    record_activity(
        &state.pool,
        "merged",
        Some("series"),
        Some(payload.target_id),
        &target_name,
        Some("Series merged"),
        None,
        None,
        Some(auth.user_id),
    )
    .await;

    // Trigger search index updates:
    // 1. Source series will be deleted from index (index call handles missing entity)
    let _ = state.library_events.send(LibraryEvent::SeriesUpdated {
        series_id: payload.source_id,
    });
    // 2. Target series needs reindex
    let _ = state.library_events.send(LibraryEvent::SeriesUpdated {
        series_id: payload.target_id,
    });
    // 3. Affected books need reindex (series_id changed)
    for book_id in affected_book_ids {
        let _ = state
            .library_events
            .send(LibraryEvent::library_updated(Some(book_id)));
    }

    StatusCode::NO_CONTENT.into_response()
}

pub async fn delete_series(
    State(state): State<AppState>,
    auth: AuthUser,
    Path(series_id): Path<String>,
) -> impl IntoResponse {
    if let Err(e) = auth.require_contributor() {
        return e.into_response();
    }
    let series_id = match Uuid::parse_str(&series_id) {
        Ok(id) => id,
        Err(_) => return (StatusCode::BAD_REQUEST, "invalid series id").into_response(),
    };

    let name = match series_repo::get_name(&state.pool, series_id).await {
        Ok(Some(name)) => name,
        Ok(None) => return (StatusCode::NOT_FOUND, "not found").into_response(),
        Err(e) => {
            tracing::error!(error = %e, "failed to get series name");
            return (StatusCode::INTERNAL_SERVER_ERROR, "database error").into_response();
        }
    };

    // Get affected book IDs before deletion
    let affected_book_ids = books::get_ids_by_series(&state.pool, series_id)
        .await
        .unwrap_or_default();

    let mut tx = match state.pool.begin().await {
        Ok(tx) => tx,
        Err(e) => {
            tracing::error!(error = %e, "failed to begin transaction for series delete");
            return (StatusCode::INTERNAL_SERVER_ERROR, "database error").into_response();
        }
    };

    // Delete the series (CASCADE removes book_series junction rows)
    if let Err(e) = series_repo::delete(&mut *tx, series_id).await {
        tracing::error!(error = %e, series_id = %series_id, "failed to delete series");
        return (StatusCode::INTERNAL_SERVER_ERROR, "failed to delete series").into_response();
    }

    // Auto-select new primary for books that lost their primary series
    {
        use crate::repositories::book_series;
        for &book_id in &affected_book_ids {
            let _ = book_series::auto_select_primary(&mut *tx, book_id, true).await;
        }
    }

    if let Err(e) = tx.commit().await {
        tracing::error!(error = %e, "failed to commit series delete transaction");
        return (StatusCode::INTERNAL_SERVER_ERROR, "database error").into_response();
    }

    record_event(&state, "series.delete", None, "ok", Some(name), None).await;

    // Trigger search index updates:
    // 1. Series will be deleted from index (index call handles missing entity)
    let _ = state
        .library_events
        .send(LibraryEvent::SeriesUpdated { series_id });
    // 2. Affected books need reindex (series_id cleared)
    for book_id in affected_book_ids {
        let _ = state
            .library_events
            .send(LibraryEvent::library_updated(Some(book_id)));
    }

    StatusCode::NO_CONTENT.into_response()
}

pub async fn reorder_series(
    State(state): State<AppState>,
    auth: AuthUser,
    Path(series_id): Path<String>,
    Json(payload): Json<SeriesReorderRequest>,
) -> impl IntoResponse {
    if let Err(e) = auth.require_contributor() {
        return e.into_response();
    }
    let series_id = match Uuid::parse_str(&series_id) {
        Ok(id) => id,
        Err(_) => return (StatusCode::BAD_REQUEST, "invalid series id").into_response(),
    };

    match books::update_series_index(
        &state.pool,
        payload.book_id,
        series_id,
        payload.series_index,
    )
    .await
    {
        Ok(_) => {
            record_event(
                &state,
                "series.reorder",
                Some(payload.book_id),
                "ok",
                None,
                None,
            )
            .await;
            StatusCode::NO_CONTENT.into_response()
        }
        Err(_) => (StatusCode::BAD_REQUEST, "failed to update series order").into_response(),
    }
}

pub async fn list_series_books(
    State(state): State<AppState>,
    auth: AuthUser,
    Path(series_id): Path<String>,
    Query(params): Query<ListSeriesBooksQuery>,
) -> impl IntoResponse {
    let series_id = match Uuid::parse_str(&series_id) {
        Ok(id) => id,
        Err(_) => return (StatusCode::BAD_REQUEST, "invalid series id").into_response(),
    };

    let rows = match books::list_by_series(
        &state.pool,
        series_id,
        auth.user_id,
        params.include_not_owned,
    )
    .await
    {
        Ok(rows) => rows,
        Err(e) => {
            tracing::error!(error = %e, "Failed to list series books");
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
