//! CRUD operations for wanted books.

use axum::{
    Json,
    extract::{Path, Query, State},
    http::StatusCode,
    response::IntoResponse,
};
use uuid::Uuid;

use crate::AppState;
use crate::events::LibraryEvent;
use crate::images::endpoint_url;
use crate::repositories::{authors, books, series, wanted_attempts};
use crate::routes::AuthUser;

use super::{
    CreateWantedRequest, ListWantedQuery, ListWantedResponse, WantedAttemptResponse,
    WantedDetailResponse, WantedItemResponse, WantedStatusCounts,
};

/// GET /api/v1/admin/wanted
/// List wanted books (not in library, want_status in pending/wanted)
pub async fn list_wanted(
    State(state): State<AppState>,
    auth: AuthUser,
    Query(query): Query<ListWantedQuery>,
) -> impl IntoResponse {
    if let Err(e) = auth.require_contributor() {
        return e.into_response();
    }
    // Status counts
    let counts = match wanted_attempts::get_status_counts(&state.pool).await {
        Ok(c) => WantedStatusCounts {
            pending: c.pending,
            wanted: c.wanted,
            searching: c.searching,
            downloading: c.downloading,
            ingesting: c.ingesting,
            watching: c.watching,
            search_failed: c.search_failed,
            queued: c.queued,
            kobload_error: c.kobload_error,
            needs_review: c.needs_review,
            completed: c.completed,
            failed: c.failed,
        },
        Err(e) => {
            tracing::error!(error = %e, "Failed to get status counts");
            return (StatusCode::INTERNAL_SERVER_ERROR, "database error").into_response();
        }
    };

    // List items
    match books::list_wanted(
        &state.pool,
        query.search.as_deref(),
        query.status.as_deref(),
    )
    .await
    {
        Ok(rows) => {
            let items = rows
                .into_iter()
                .map(|row| WantedItemResponse {
                    id: row.id,
                    title: row.title,
                    author: row.author,
                    series: row.series_name,
                    series_index: row.series_index,
                    identifiers: row.identifiers,
                    cover_url: endpoint_url(
                        &row.images,
                        "cover",
                        "/api/v1/library/books/",
                        row.id,
                        "/cover",
                    ),
                    want_status: row.want_status,
                    attempt_status: row.attempt_status,
                    last_attempt_at: row.last_attempt_at,
                    created_at: row.created_at,
                    updated_at: row.updated_at,
                })
                .collect::<Vec<_>>();
            Json(ListWantedResponse {
                items,
                status_counts: counts,
            })
            .into_response()
        }
        Err(e) => {
            tracing::error!(error = %e, "Failed to list wanted items");
            (StatusCode::INTERNAL_SERVER_ERROR, "database error").into_response()
        }
    }
}

/// POST /api/v1/admin/wanted
/// Create a wanted book (book not in library that should be acquired)
pub async fn create_wanted(
    State(state): State<AppState>,
    auth: AuthUser,
    Json(payload): Json<CreateWantedRequest>,
) -> impl IntoResponse {
    if let Err(e) = auth.require_contributor() {
        return e.into_response();
    }
    let identifiers = payload.identifiers.clone().unwrap_or(serde_json::json!({}));

    // Find or create author using repository
    let author_id = if let Some(ref author_name) = payload.author {
        let author_name = author_name.trim();
        if !author_name.is_empty() {
            match authors::get_or_create_by_name(&state.pool, author_name).await {
                Ok(id) => Some(id),
                Err(e) => {
                    tracing::warn!(error = %e, "Failed to create/find author");
                    None
                }
            }
        } else {
            None
        }
    } else {
        None
    };

    // Find or create series using repository
    let series_id = if let Some(ref series_name) = payload.series {
        let author_ids = author_id.as_ref().map(|id| vec![*id]).unwrap_or_default();
        series::get_or_create_with_author_overlap(&state.pool, series_name, &author_ids, None)
            .await
            .ok()
            .flatten()
    } else {
        None
    };

    // Convert cover_url to images map
    let images = payload.cover_url.as_ref().map(|url| {
        let mut map = crate::images::ImageMap::new();
        map.insert(
            "cover".to_string(),
            crate::images::ImageEntry {
                source_url: Some(url.clone()),
                ..Default::default()
            },
        );
        map
    });

    // Create the book record using repository
    let want_format = payload
        .want_format
        .clone()
        .unwrap_or_else(|| "ebook".to_string());
    let input = books::DiscoveredBookInput {
        title: payload.title.clone(),
        identifiers: Some(identifiers.clone()),
        images,
        want_status: Some("wanted".to_string()),
        want_format: Some(want_format.clone()),
        ..Default::default()
    };

    let id = match books::create_discovered_book(&state.pool, input).await {
        Ok(id) => id,
        Err(e) => {
            tracing::error!(error = %e, "Failed to create wanted book");
            return (StatusCode::INTERNAL_SERVER_ERROR, "database error").into_response();
        }
    };

    // Link to series via junction table
    if let Some(sid) = series_id {
        let _ =
            crate::repositories::book_series::add(&state.pool, id, sid, payload.series_index, true)
                .await;
    }

    // Link to author
    if let Some(aid) = author_id {
        let _ = authors::link_book(&state.pool, id, aid).await;
    }

    // Publish NATS event to notify Kobload of new watch pattern
    let pattern_payload = build_watch_pattern_payload(
        id,
        &payload.title,
        payload.author.as_deref(),
        payload.series.as_deref(),
        payload.series_index,
        identifiers.get("isbn10").and_then(|v| v.as_str()),
        identifiers.get("isbn13").and_then(|v| v.as_str()),
        identifiers.get("hardcover").and_then(|v| v.as_str()),
        &want_format,
        &state,
    )
    .await;

    if let Some(pattern_payload) = pattern_payload {
        let _ = state
            .nats
            .publisher()
            .enqueue_job("kobload", "watch_pattern_upsert", pattern_payload)
            .await;
    }

    Json(serde_json::json!({ "id": id })).into_response()
}

/// GET /api/v1/admin/wanted/{id}
pub async fn get_wanted(
    State(state): State<AppState>,
    auth: AuthUser,
    Path(id): Path<Uuid>,
) -> impl IntoResponse {
    if let Err(e) = auth.require_contributor() {
        return e.into_response();
    }
    let item_row = match books::get_wanted_item(&state.pool, id).await {
        Ok(Some(row)) => row,
        Ok(None) => return (StatusCode::NOT_FOUND, "book not found").into_response(),
        Err(e) => {
            tracing::error!(error = %e, "Failed to fetch book");
            return (StatusCode::INTERNAL_SERVER_ERROR, "database error").into_response();
        }
    };

    let item = WantedItemResponse {
        id: item_row.id,
        title: item_row.title,
        author: item_row.author,
        series: item_row.series_name,
        series_index: item_row.series_index,
        identifiers: item_row.identifiers,
        cover_url: endpoint_url(
            &item_row.images,
            "cover",
            "/api/v1/library/books/",
            item_row.id,
            "/cover",
        ),
        want_status: item_row.want_status,
        attempt_status: item_row.attempt_status,
        last_attempt_at: item_row.last_attempt_at,
        created_at: item_row.created_at,
        updated_at: item_row.updated_at,
    };

    let attempts = match wanted_attempts::list_by_book(&state.pool, id).await {
        Ok(rows) => rows
            .into_iter()
            .map(|row| WantedAttemptResponse {
                id: row.id,
                book_id: row.book_id,
                movie_id: row.movie_id,
                source: row.source,
                external_id: row.external_id,
                download_client_id: row.download_client_id,
                download_url: row.download_url,
                status: row.status,
                message: row.message,
                created_at: row.created_at,
            })
            .collect::<Vec<_>>(),
        Err(e) => {
            tracing::error!(error = %e, "Failed to fetch wanted attempts");
            return (StatusCode::INTERNAL_SERVER_ERROR, "database error").into_response();
        }
    };

    Json(WantedDetailResponse { item, attempts }).into_response()
}

/// DELETE /api/v1/admin/wanted/{id}
pub async fn delete_wanted(
    State(state): State<AppState>,
    auth: AuthUser,
    Path(id): Path<Uuid>,
) -> impl IntoResponse {
    if let Err(e) = auth.require_contributor() {
        return e.into_response();
    }
    // For books not in library, we can either:
    // 1. Set want_status to 'skipped'
    // 2. Actually delete the book record
    // Let's use option 1 for now to preserve history

    let result = books::set_want_status_if_not_in_library(&state.pool, id, "skipped").await;

    match result {
        Ok(true) => {
            let _ = state
                .library_events
                .send(LibraryEvent::library_updated(Some(id)));
            // Notify Kobload to remove watch pattern
            let _ = state
                .nats
                .publisher()
                .enqueue_job(
                    "kobload",
                    "watch_pattern_delete",
                    serde_json::json!({ "external_id": id.to_string() }),
                )
                .await;
            StatusCode::NO_CONTENT.into_response()
        }
        Ok(false) => (
            StatusCode::NOT_FOUND,
            "book not found or already in library",
        )
            .into_response(),
        Err(e) => {
            tracing::error!(error = %e, "Failed to delete wanted book");
            (StatusCode::INTERNAL_SERVER_ERROR, "database error").into_response()
        }
    }
}

// =============================================================================
// Watch Pattern Payload Helper
// =============================================================================

#[allow(clippy::too_many_arguments)]
async fn build_watch_pattern_payload(
    book_id: Uuid,
    title: &str,
    author: Option<&str>,
    series: Option<&str>,
    series_index: Option<f64>,
    isbn10: Option<&str>,
    isbn13: Option<&str>,
    hardcover_id: Option<&str>,
    want_format: &str,
    state: &AppState,
) -> Option<serde_json::Value> {
    let runtime = state.runtime_config.get().await;

    // Use ingest URL directly so downloaded files are processed
    let callback_url = runtime.kobload_ingest_url.clone();

    let callback_url = callback_url?;

    let min_score = runtime.wanted_min_score.unwrap_or(0.65);
    let identifiers = nats_jobs::BookIdentifiers {
        isbn10: isbn10.map(String::from),
        isbn13: isbn13.map(String::from),
        hardcover_id: hardcover_id.map(String::from),
        goodreads_id: None,
        openlibrary_id: None,
    };
    let alternate_titles = crate::wanted::book_titles::book_alternate_titles(
        &crate::metadata::ProviderRegistry::new(state.pool.clone()),
        &crate::wanted::book_titles::BookRef {
            title,
            series,
            position: series_index,
            hardcover_id,
        },
        crate::wanted::book_titles::Lookup::Fetch,
    )
    .await;
    let media_want = match want_format {
        "audiobook" => nats_jobs::MediaWant::Audiobook {
            title: title.to_string(),
            author: author.map(String::from),
            narrator: None,
            isbn: identifiers.isbn13.clone().or(identifiers.isbn10.clone()),
            format_preferences: vec!["m4b".to_string(), "flac".to_string(), "mp3".to_string()],
            identifiers: Some(identifiers),
            series: series.map(String::from),
            series_position: series_index.map(|idx| idx.to_string()),
            release_date: None,
            alternate_titles: alternate_titles.clone(),
        },
        _ => nats_jobs::MediaWant::Book {
            title: title.to_string(),
            author: author.map(String::from),
            isbn: identifiers.isbn13.clone().or(identifiers.isbn10.clone()),
            format_preferences: crate::routes::ingest::EBOOK_EXTENSIONS
                .iter()
                .map(|s| (*s).to_string())
                .collect(),
            identifiers: Some(identifiers),
            series: series.map(String::from),
            series_position: series_index.map(|idx| idx.to_string()),
            release_date: None,
            alternate_titles: alternate_titles.clone(),
        },
    };

    let metadata = serde_json::json!({
        "book_id": book_id,
        "title": title,
        "author": author,
        "series": series,
        "series_index": series_index,
        "want_format": want_format,
    });

    // Build proper WatchPatternUpsert payload
    let upsert = nats_jobs::WatchPatternUpsert {
        source_system: "kobbler".to_string(),
        source_id: book_id.to_string(),
        callback_url,
        patterns: Vec::new(),
        media_want: Some(media_want),
        match_targets: Vec::new(),
        match_mode: "fuzzy".to_string(),
        min_score: Some(min_score),
        auto_download: true,
        metadata: Some(metadata),
        enabled: true,
    };

    serde_json::to_value(&upsert).ok()
}
