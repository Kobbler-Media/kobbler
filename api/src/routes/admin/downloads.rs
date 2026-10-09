//! Downloads routes - proxies to Kobload for download information with media enrichment.

use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    response::{IntoResponse, Json},
};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use uuid::Uuid;

use crate::routes::ingest::{
    KobloadCallbackRequest, KobloadFileGroup, KobloadFileInfo, process_kobload_ingest,
};
use crate::{AppState, routes::AuthUser};

/// Query parameters for listing downloads.
#[derive(Debug, Deserialize)]
pub struct ListDownloadsQuery {
    /// Number of items to return (default: 20, max: 100)
    #[serde(default = "default_limit")]
    pub limit: i64,
    /// Pagination cursor from previous response
    pub cursor: Option<String>,
    /// Filter by status (comma-separated)
    pub status: Option<String>,
}

fn default_limit() -> i64 {
    20
}

/// Media info for enrichment (includes IDs for linking)
#[derive(Debug, Clone, Serialize)]
pub struct MediaInfo {
    pub id: Uuid,
    pub title: String,
    pub cover_url: Option<String>,
    pub author: Option<String>,
    pub author_id: Option<Uuid>,
    pub series: Option<String>,
    pub series_id: Option<Uuid>,
    pub series_index: Option<f64>,
    /// Additional context line (e.g. "S01E05 - Pilot" for TV episodes)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub subtitle: Option<String>,
}

/// Response for paginated download list with media enrichment.
#[derive(Debug, Serialize)]
pub struct EnrichedDownloadsResponse {
    pub items: Vec<Value>,
    pub next_cursor: Option<String>,
}

/// Get current downloads from Kobload with media enrichment.
pub async fn list_downloads(
    State(state): State<AppState>,
    auth: AuthUser,
    Query(query): Query<ListDownloadsQuery>,
) -> impl IntoResponse {
    if let Err(e) = auth.require_contributor() {
        return e.into_response();
    }
    let config = state.runtime_config.get().await;

    let Some(base_url) = config.kobload_base_url.filter(|s| !s.is_empty()) else {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(serde_json::json!({ "error": "Kobload not configured" })),
        )
            .into_response();
    };

    let Some(api_key) = config.kobload_api_key.filter(|s| !s.is_empty()) else {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(serde_json::json!({ "error": "Kobload API key not configured" })),
        )
            .into_response();
    };

    // Build URL with query params
    let mut url = format!("{}/api/v1/downloads", base_url.trim_end_matches('/'));
    let mut params = vec![format!("limit={}", query.limit)];
    if let Some(ref cursor) = query.cursor {
        params.push(format!("cursor={}", urlencoding::encode(cursor)));
    }
    if let Some(ref status) = query.status {
        params.push(format!("status={}", urlencoding::encode(status)));
    }
    if !params.is_empty() {
        url = format!("{}?{}", url, params.join("&"));
    }

    let client = crate::http::new_client();
    let response = client
        .get(&url)
        .header("Authorization", format!("Bearer {}", api_key))
        .send()
        .await;

    match response {
        Ok(resp) => {
            if resp.status().is_success() {
                match resp.json::<Value>().await {
                    Ok(data) => {
                        // Enrich downloads with media info
                        let enriched = enrich_downloads(&state, data).await;
                        Json(enriched).into_response()
                    }
                    Err(e) => (
                        StatusCode::BAD_GATEWAY,
                        Json(serde_json::json!({ "error": format!("Failed to parse Kobload response: {}", e) })),
                    )
                        .into_response(),
                }
            } else {
                (
                    StatusCode::BAD_GATEWAY,
                    Json(serde_json::json!({ "error": format!("Kobload returned status: {}", resp.status()) })),
                )
                    .into_response()
            }
        }
        Err(e) => (
            StatusCode::BAD_GATEWAY,
            Json(serde_json::json!({ "error": format!("Failed to connect to Kobload: {}", e) })),
        )
            .into_response(),
    }
}

/// Extract a UUID from a metadata field, checking both top-level and original_metadata.
fn metadata_uuid(item: &Value, field: &str) -> Option<Uuid> {
    let meta = item.get("metadata")?;
    // Check top-level metadata first, then fall back to original_metadata
    meta.get(field)
        .or_else(|| meta.get("original_metadata").and_then(|om| om.get(field)))
        .and_then(|v| v.as_str())
        .and_then(|s| s.parse::<Uuid>().ok())
}

/// Resolve an episode ID by cross-referencing release_interpretation_json (parsed episode
/// numbers) with the episode_map in metadata (episode identifier → UUID).
fn resolve_episode_id_from_release(item: &Value) -> Option<Uuid> {
    let ri = item.get("release_interpretation_json")?;
    let parsed: nats_jobs::ParsedRelease = serde_json::from_value(ri.clone()).ok()?;
    let tv = parsed.media.as_tv()?;

    let ep_num = tv.episodes.first()?;

    // Get the episode_map from metadata (top-level or original_metadata)
    let meta = item.get("metadata")?;
    let ep_map = meta
        .get("episode_map")
        .or_else(|| {
            meta.get("original_metadata")
                .and_then(|om| om.get("episode_map"))
        })?
        .as_object()?;

    let ep_id_str = ep_map.get(&ep_num.to_string())?.as_str()?;
    ep_id_str.parse::<Uuid>().ok()
}

/// Count episodes from the `episode_map` metadata field.
fn metadata_episode_count(item: &Value) -> Option<usize> {
    let meta = item.get("metadata")?;
    meta.get("episode_map")
        .or_else(|| {
            meta.get("original_metadata")
                .and_then(|om| om.get("episode_map"))
        })
        .and_then(|v| v.as_object())
        .map(|obj| obj.len())
}

/// Enrich kobload downloads with media info from metadata (books, movies, TV shows).
pub(crate) async fn enrich_downloads(state: &AppState, data: Value) -> EnrichedDownloadsResponse {
    let items = data
        .get("items")
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();
    let next_cursor = data
        .get("next_cursor")
        .and_then(|v| v.as_str())
        .map(String::from);

    // Collect IDs by type
    let mut book_ids = Vec::new();
    let mut movie_ids = Vec::new();
    let mut episode_ids = Vec::new();
    let mut season_ids = Vec::new();
    let mut show_ids = Vec::new();
    // Track episode count per season from episode_map metadata
    let mut season_episode_counts: std::collections::HashMap<Uuid, usize> =
        std::collections::HashMap::new();

    for item in &items {
        if let Some(id) = metadata_uuid(item, "book_id") {
            book_ids.push(id);
        }
        if let Some(id) = metadata_uuid(item, "movie_id") {
            movie_ids.push(id);
        }
        if let Some(id) = metadata_uuid(item, "episode_id") {
            episode_ids.push(id);
        } else if let Some(id) = resolve_episode_id_from_release(item) {
            // No explicit episode_id in metadata, but we can resolve one by
            // cross-referencing the parsed release (episode number) with the
            // episode_map (episode identifier → UUID).
            episode_ids.push(id);
        }
        if let Some(id) = metadata_uuid(item, "season_id") {
            season_ids.push(id);
            if let Some(count) = metadata_episode_count(item) {
                season_episode_counts.insert(id, count);
            }
        }
        if let Some(id) = metadata_uuid(item, "show_id") {
            show_ids.push(id);
        }
    }

    // Look up info for each type
    let book_map = fetch_book_info_by_ids(&state.pool, &book_ids).await;
    let movie_map = fetch_movie_info_by_ids(&state.pool, &movie_ids).await;
    let episode_map = fetch_episode_info_by_ids(&state.pool, &episode_ids).await;
    let season_map =
        fetch_season_info_by_ids(&state.pool, &season_ids, &season_episode_counts).await;
    let show_map = fetch_show_info_by_ids(&state.pool, &show_ids).await;

    // Enrich each download
    let enriched_items: Vec<Value> = items
        .into_iter()
        .map(|download| {
            let book_id = metadata_uuid(&download, "book_id");
            let movie_id = metadata_uuid(&download, "movie_id");
            let episode_id = metadata_uuid(&download, "episode_id")
                .or_else(|| resolve_episode_id_from_release(&download));
            let season_id = metadata_uuid(&download, "season_id");
            let show_id = metadata_uuid(&download, "show_id");

            let mut obj = match download {
                Value::Object(map) => map,
                _ => Map::new(),
            };

            // Populate "media" field from whichever entity type matches.
            // Prefer episode over season over show (more specific = better subtitle).
            let info = book_id
                .and_then(|id| book_map.get(&id).cloned())
                .or_else(|| movie_id.and_then(|id| movie_map.get(&id).cloned()))
                .or_else(|| episode_id.and_then(|id| episode_map.get(&id).cloned()))
                .or_else(|| season_id.and_then(|id| season_map.get(&id).cloned()))
                .or_else(|| show_id.and_then(|id| show_map.get(&id).cloned()));

            obj.insert(
                "media".to_string(),
                info.map(|i| serde_json::to_value(i).unwrap_or(Value::Null))
                    .unwrap_or(Value::Null),
            );

            Value::Object(obj)
        })
        .collect();

    EnrichedDownloadsResponse {
        items: enriched_items,
        next_cursor,
    }
}

/// Fetch media info for a list of book IDs
async fn fetch_book_info_by_ids(
    pool: &sqlx::PgPool,
    book_ids: &[Uuid],
) -> std::collections::HashMap<Uuid, MediaInfo> {
    if book_ids.is_empty() {
        return std::collections::HashMap::new();
    }

    // Query books directly with series and author info
    let rows = sqlx::query!(
        r#"
        SELECT
            b.id AS "book_id!",
            b.title AS "title!",
            CASE WHEN (b.images->'cover'->>'source_url' IS NOT NULL OR b.images->'cover'->>'cached_at' IS NOT NULL)
                 THEN '/api/v1/library/books/' || b.id || '/cover'
                 ELSE NULL
            END as cover_url,
            bs.series_id AS "series_id?",
            bs.series_index AS "series_index?",
            s.name AS "series_name?",
            -- Two independent subqueries over the same co-authors: both must
            -- order by author_id (the codebase-wide stand-in for a primary
            -- flag) or the name can end up linked to the other author's page.
            (SELECT a.id FROM book_authors ba
             JOIN authors a ON a.id = ba.author_id
             WHERE ba.book_id = b.id
             ORDER BY ba.author_id
             LIMIT 1) AS author_id,
            (SELECT a.name FROM book_authors ba
             JOIN authors a ON a.id = ba.author_id
             WHERE ba.book_id = b.id
             ORDER BY ba.author_id
             LIMIT 1) AS author
        FROM books b
        LEFT JOIN book_series bs ON bs.book_id = b.id AND bs.is_primary = TRUE
        LEFT JOIN series s ON s.id = bs.series_id
        WHERE b.id = ANY($1)
        "#,
        book_ids,
    )
    .fetch_all(pool)
    .await;

    match rows {
        Ok(rows) => rows
            .into_iter()
            .map(|row| {
                (
                    row.book_id,
                    MediaInfo {
                        id: row.book_id,
                        title: row.title,
                        cover_url: row.cover_url,
                        author: row.author,
                        author_id: row.author_id,
                        series: row.series_name,
                        series_id: row.series_id,
                        series_index: row.series_index,
                        subtitle: None,
                    },
                )
            })
            .collect(),
        Err(e) => {
            tracing::warn!(error = %e, "Failed to fetch media info for books");
            std::collections::HashMap::new()
        }
    }
}

/// Fetch media info for a list of movie IDs.
async fn fetch_movie_info_by_ids(
    pool: &sqlx::PgPool,
    movie_ids: &[Uuid],
) -> std::collections::HashMap<Uuid, MediaInfo> {
    if movie_ids.is_empty() {
        return std::collections::HashMap::new();
    }

    let rows = sqlx::query!(
        r#"
        SELECT id, title,
            CASE WHEN (images->'poster'->>'source_url' IS NOT NULL OR images->'poster'->>'cached_at' IS NOT NULL)
                 THEN '/api/v1/admin/movies/' || id || '/poster'
                 ELSE NULL
            END as poster_url,
            year
        FROM movies
        WHERE id = ANY($1)
        "#,
        movie_ids,
    )
    .fetch_all(pool)
    .await;

    match rows {
        Ok(rows) => rows
            .into_iter()
            .map(|row| {
                (
                    row.id,
                    MediaInfo {
                        id: row.id,
                        title: row.title,
                        cover_url: row.poster_url,
                        author: row.year.map(|y| y.to_string()),
                        author_id: None,
                        series: None,
                        series_id: None,
                        series_index: None,
                        subtitle: None,
                    },
                )
            })
            .collect(),
        Err(e) => {
            tracing::warn!(error = %e, "Failed to fetch movie info for downloads");
            std::collections::HashMap::new()
        }
    }
}

/// Fetch media info for a list of TV episode IDs (joins through season → show).
async fn fetch_episode_info_by_ids(
    pool: &sqlx::PgPool,
    episode_ids: &[Uuid],
) -> std::collections::HashMap<Uuid, MediaInfo> {
    if episode_ids.is_empty() {
        return std::collections::HashMap::new();
    }

    let rows = sqlx::query!(
        r#"
        SELECT
            e.id AS episode_id,
            e.title AS "episode_title?",
            e.episode_identifier,
            s.season_identifier,
            sh.id AS show_id,
            sh.title AS show_title,
            CASE WHEN (sh.images->'poster'->>'source_url' IS NOT NULL OR sh.images->'poster'->>'cached_at' IS NOT NULL)
                 THEN '/api/v1/admin/tv/shows/' || sh.id || '/poster'
                 ELSE NULL
            END as poster_url
        FROM tv_episodes e
        JOIN tv_seasons s ON s.id = e.season_id
        JOIN tv_shows sh ON sh.id = s.show_id
        WHERE e.id = ANY($1)
        "#,
        episode_ids,
    )
    .fetch_all(pool)
    .await;

    match rows {
        Ok(rows) => rows
            .into_iter()
            .map(|row| {
                let subtitle = {
                    let mut s = format!("S{}E{}", row.season_identifier, row.episode_identifier);
                    if let Some(ref ep_title) = row.episode_title {
                        s.push_str(" - ");
                        s.push_str(ep_title);
                    }
                    s
                };
                (
                    row.episode_id,
                    MediaInfo {
                        // Use show_id so the frontend links to the show page
                        id: row.show_id,
                        title: row.show_title,
                        cover_url: row.poster_url,
                        author: None,
                        author_id: None,
                        series: None,
                        series_id: None,
                        series_index: None,
                        subtitle: Some(subtitle),
                    },
                )
            })
            .collect(),
        Err(e) => {
            tracing::warn!(error = %e, "Failed to fetch episode info for downloads");
            std::collections::HashMap::new()
        }
    }
}

/// Fetch media info for a list of TV season IDs (joins through season → show).
async fn fetch_season_info_by_ids(
    pool: &sqlx::PgPool,
    season_ids: &[Uuid],
    episode_counts: &std::collections::HashMap<Uuid, usize>,
) -> std::collections::HashMap<Uuid, MediaInfo> {
    if season_ids.is_empty() {
        return std::collections::HashMap::new();
    }

    let rows = sqlx::query!(
        r#"
        SELECT
            s.id AS season_id,
            s.season_identifier,
            sh.id AS show_id,
            sh.title AS show_title,
            CASE WHEN (sh.images->'poster'->>'source_url' IS NOT NULL OR sh.images->'poster'->>'cached_at' IS NOT NULL)
                 THEN '/api/v1/admin/tv/shows/' || sh.id || '/poster'
                 ELSE NULL
            END as poster_url
        FROM tv_seasons s
        JOIN tv_shows sh ON sh.id = s.show_id
        WHERE s.id = ANY($1)
        "#,
        season_ids,
    )
    .fetch_all(pool)
    .await;

    match rows {
        Ok(rows) => rows
            .into_iter()
            .map(|row| {
                let mut subtitle = format!("Season {}", row.season_identifier);
                if let Some(&count) = episode_counts.get(&row.season_id) {
                    subtitle.push_str(&format!(
                        " · {} episode{}",
                        count,
                        if count == 1 { "" } else { "s" }
                    ));
                }
                (
                    row.season_id,
                    MediaInfo {
                        id: row.show_id,
                        title: row.show_title,
                        cover_url: row.poster_url,
                        author: None,
                        author_id: None,
                        series: None,
                        series_id: None,
                        series_index: None,
                        subtitle: Some(subtitle),
                    },
                )
            })
            .collect(),
        Err(e) => {
            tracing::warn!(error = %e, "Failed to fetch season info for downloads");
            std::collections::HashMap::new()
        }
    }
}

/// Fetch media info for a list of TV show IDs.
async fn fetch_show_info_by_ids(
    pool: &sqlx::PgPool,
    show_ids: &[Uuid],
) -> std::collections::HashMap<Uuid, MediaInfo> {
    if show_ids.is_empty() {
        return std::collections::HashMap::new();
    }

    let rows = sqlx::query!(
        r#"
        SELECT id, title,
            CASE WHEN (images->'poster'->>'source_url' IS NOT NULL OR images->'poster'->>'cached_at' IS NOT NULL)
                 THEN '/api/v1/admin/tv/shows/' || id || '/poster'
                 ELSE NULL
            END as poster_url
        FROM tv_shows
        WHERE id = ANY($1)
        "#,
        show_ids,
    )
    .fetch_all(pool)
    .await;

    match rows {
        Ok(rows) => rows
            .into_iter()
            .map(|row| {
                (
                    row.id,
                    MediaInfo {
                        id: row.id,
                        title: row.title,
                        cover_url: row.poster_url,
                        author: None,
                        author_id: None,
                        series: None,
                        series_id: None,
                        series_index: None,
                        subtitle: None,
                    },
                )
            })
            .collect(),
        Err(e) => {
            tracing::warn!(error = %e, "Failed to fetch TV show info for downloads");
            std::collections::HashMap::new()
        }
    }
}

/// Cancel a download in Kobload
pub async fn cancel_download(
    State(state): State<AppState>,
    auth: AuthUser,
    Path(download_id): Path<Uuid>,
) -> impl IntoResponse {
    if let Err(e) = auth.require_contributor() {
        return e.into_response();
    }
    let config = state.runtime_config.get().await;

    let Some(base_url) = config.kobload_base_url.filter(|s| !s.is_empty()) else {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(serde_json::json!({ "error": "Kobload not configured" })),
        )
            .into_response();
    };

    let Some(api_key) = config.kobload_api_key.filter(|s| !s.is_empty()) else {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(serde_json::json!({ "error": "Kobload API key not configured" })),
        )
            .into_response();
    };

    // DELETE with cancel=true to remove from download client
    let url = format!(
        "{}/api/v1/downloads/{}?cancel=true",
        base_url.trim_end_matches('/'),
        download_id
    );

    let client = crate::http::new_client();
    let response = client
        .delete(&url)
        .header("Authorization", format!("Bearer {}", api_key))
        .send()
        .await;

    match response {
        Ok(resp) => {
            if resp.status().is_success() || resp.status() == StatusCode::NO_CONTENT {
                // Also update local wanted_attempt status
                let _ = sqlx::query!(
                    "UPDATE wanted_attempts SET status = 'cancelled', message = 'Cancelled by user' WHERE external_id = $1",
                    download_id.to_string(),
                )
                .execute(&state.pool)
                .await;

                StatusCode::NO_CONTENT.into_response()
            } else {
                (
                    StatusCode::BAD_GATEWAY,
                    Json(serde_json::json!({ "error": format!("Kobload returned status: {}", resp.status()) })),
                )
                    .into_response()
            }
        }
        Err(e) => (
            StatusCode::BAD_GATEWAY,
            Json(serde_json::json!({ "error": format!("Failed to connect to Kobload: {}", e) })),
        )
            .into_response(),
    }
}

/// Retry downloading a book - triggers a new search
pub async fn retry_download(
    State(state): State<AppState>,
    auth: AuthUser,
    Path(download_id): Path<Uuid>,
) -> impl IntoResponse {
    if let Err(e) = auth.require_contributor() {
        return e.into_response();
    }
    // Find the book associated with this download
    let book_id: Option<Uuid> = sqlx::query_scalar!(
        "SELECT book_id FROM wanted_attempts WHERE external_id = $1",
        download_id.to_string(),
    )
    .fetch_optional(&state.pool)
    .await
    .ok()
    .flatten()
    .flatten();

    let Some(book_id) = book_id else {
        return (
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({ "error": "Download not found or not linked to a book" })),
        )
            .into_response();
    };

    // Queue a new search job for this book
    let display = crate::jobs::fetch_job_display(&state.pool, "book", book_id).await;
    let context =
        nats_jobs::JobContext::for_entity("book", book_id, "Retrying download search".to_string())
            .with_display_opt(display);
    match crate::jobs::enqueue(
        &state.pool,
        state.nats.publisher(),
        "search_wanted_book",
        serde_json::json!({ "book_id": book_id }),
        context,
    )
    .await
    {
        Ok(_) => {
            // Update the attempt status
            let _ = sqlx::query!(
                "UPDATE wanted_attempts SET status = 'searching', message = 'Retry search triggered' WHERE external_id = $1",
                download_id.to_string(),
            )
            .execute(&state.pool)
            .await;

            Json(serde_json::json!({ "message": "Search queued", "book_id": book_id }))
                .into_response()
        }
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({ "error": format!("Failed to queue search: {}", e) })),
        )
            .into_response(),
    }
}

/// Response for Kobload download details
#[derive(Debug, Deserialize)]
struct KobloadDownloadDetails {
    #[allow(dead_code)]
    pub id: Uuid,
    pub status: String,
    #[serde(default)]
    pub name: Option<String>,
    pub metadata: serde_json::Value,
    pub files: Option<Vec<KobloadFileInfo>>,
    pub file_groups: Option<Vec<KobloadFileGroup>>,
    pub error: Option<String>,
}

/// Retry the ingest callback for a completed download.
/// This is useful when the initial callback failed (e.g., network issue, timeout).
pub async fn retry_callback(
    State(state): State<AppState>,
    auth: AuthUser,
    Path(download_id): Path<Uuid>,
) -> impl IntoResponse {
    if let Err(e) = auth.require_contributor() {
        return e.into_response();
    }
    let config = state.runtime_config.get().await;

    let Some(base_url) = config.kobload_base_url.filter(|s| !s.is_empty()) else {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(serde_json::json!({ "error": "Kobload not configured" })),
        )
            .into_response();
    };

    let Some(api_key) = config.kobload_api_key.filter(|s| !s.is_empty()) else {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(serde_json::json!({ "error": "Kobload API key not configured" })),
        )
            .into_response();
    };

    // Fetch download details from Kobload
    let url = format!(
        "{}/api/v1/downloads/{}",
        base_url.trim_end_matches('/'),
        download_id
    );

    let client = crate::http::new_client();
    let response = match client
        .get(&url)
        .header("Authorization", format!("Bearer {}", api_key))
        .send()
        .await
    {
        Ok(resp) => resp,
        Err(e) => {
            return (
                StatusCode::BAD_GATEWAY,
                Json(serde_json::json!({ "error": format!("Failed to fetch download from Kobload: {}", e) })),
            )
                .into_response();
        }
    };

    if !response.status().is_success() {
        return (
            StatusCode::BAD_GATEWAY,
            Json(serde_json::json!({ "error": format!("Kobload returned status: {}", response.status()) })),
        )
            .into_response();
    }

    let download: KobloadDownloadDetails = match response.json().await {
        Ok(d) => d,
        Err(e) => {
            return (
                StatusCode::BAD_GATEWAY,
                Json(serde_json::json!({ "error": format!("Failed to parse Kobload response: {}", e) })),
            )
                .into_response();
        }
    };

    // Verify the download is in a completed state
    if download.status != "completed"
        && download.status != "complete"
        && download.status != "downloaded"
    {
        return (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({ "error": format!("Download is not completed (status: {})", download.status) })),
        )
            .into_response();
    }

    // Build the callback payload
    let files = download.files.unwrap_or_default();
    if files.is_empty() {
        return (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({ "error": "Download has no files" })),
        )
            .into_response();
    }

    let payload = KobloadCallbackRequest {
        download_id,
        status: download.status,
        name: download.name.unwrap_or_default(),
        metadata: download.metadata,
        files,
        file_groups: download.file_groups,
        release_interpretation: None, // Not available for manual retries
        error: download.error,
        recoverable_error: None,
        idempotency_key: None, // Manual retry generates fresh callback
        is_upgrade: false,
        quality_score: None,
        replaces_download_id: None,
        release_info: None, // Not available for manual retries
    };

    // Process the ingest directly using the internal implementation
    match process_kobload_ingest(
        &state,
        payload,
        false,
        tokio_util::sync::CancellationToken::new(),
    )
    .await
    {
        Ok(_) => {
            Json(serde_json::json!({ "message": "Callback retry successful" })).into_response()
        }
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({ "error": format!("Ingest failed: {}", e) })),
        )
            .into_response(),
    }
}

/// Path parameters for the reject-download endpoint.
#[derive(Debug, Deserialize)]
pub struct RejectDownloadPath {
    pub media_type: String,
    pub entity_id: Uuid,
}

/// Request body for rejecting a download.
#[derive(Debug, Deserialize)]
pub struct RejectDownloadRequest {
    #[serde(default)]
    pub reason: Option<String>,
}

/// Reject the active download for an entity.
///
/// Looks up the entity's watch pattern on Kobload, finds the active (non-rejected)
/// match with a download, and rejects it. This cancels the download and re-opens
/// the pattern for new matches.
///
/// POST /api/v1/admin/library/{media_type}/{entity_id}/reject-download
pub async fn reject_download(
    State(state): State<AppState>,
    auth: AuthUser,
    Path(path): Path<RejectDownloadPath>,
    Json(body): Json<RejectDownloadRequest>,
) -> impl IntoResponse {
    if let Err(e) = auth.require_contributor() {
        return e.into_response();
    }
    let config = state.runtime_config.get().await;
    let client = match crate::routes::helpers::build_kobload_client(&config) {
        Ok(c) => c,
        Err((status, msg)) => {
            return (status, Json(serde_json::json!({ "error": msg }))).into_response();
        }
    };

    // Validate media_type
    if !matches!(
        path.media_type.as_str(),
        "book" | "movie" | "tv_episode" | "tv_season"
    ) {
        return (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({ "error": format!("Unsupported media_type: {}", path.media_type) })),
        )
            .into_response();
    }

    let entity_id_str = path.entity_id.to_string();
    match client
        .reject_active_download(&entity_id_str, body.reason.as_deref())
        .await
    {
        Ok(Some(match_id)) => {
            tracing::info!(
                media_type = %path.media_type,
                entity_id = %path.entity_id,
                match_id = %match_id,
                "Rejected active download via Kobload"
            );

            // Update local wanted_attempt status if we have one
            let _ = sqlx::query(
                r#"
                UPDATE wanted_attempts
                SET status = 'rejected', message = $1
                WHERE (book_id = $2 OR movie_id = $2 OR episode_id = $2)
                  AND status NOT IN ('completed', 'failed', 'cancelled', 'rejected')
                "#,
            )
            .bind(body.reason.as_deref().unwrap_or("Rejected by user"))
            .bind(path.entity_id)
            .execute(&state.pool)
            .await;

            // Broadcast status update
            let _ = state
                .library_events
                .send(crate::events::LibraryEvent::wanted_status_updated(
                    path.entity_id,
                    "watching".to_string(),
                ));

            Json(serde_json::json!({
                "rejected": true,
                "match_id": match_id,
            }))
            .into_response()
        }
        Ok(None) => (
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({
                "error": "No active download found for this entity",
                "rejected": false,
            })),
        )
            .into_response(),
        Err(e) => {
            tracing::error!(
                media_type = %path.media_type,
                entity_id = %path.entity_id,
                error = %e,
                "Failed to reject download via Kobload"
            );
            (
                StatusCode::BAD_GATEWAY,
                Json(serde_json::json!({ "error": format!("Kobload error: {}", e) })),
            )
                .into_response()
        }
    }
}

/// Request body for replacing a file.
#[derive(Debug, Deserialize)]
pub struct ReplaceFileRequest {
    pub file_id: Uuid,
    #[serde(default)]
    pub reason: Option<String>,
}

/// Replace a bad file: delete it, record the rejection, and re-want.
///
/// The re-want is `manual`: the file is already gone by the time it runs, and
/// Kobload's own record of the import outlives Kobbler deleting the file, so
/// without it the want is skipped as "already imported" whenever the pattern
/// survived confirm — a recent episode, or a part-filled season. That leaves the
/// person with no file and no replacement.
///
/// POST /api/v1/admin/library/{media_type}/{entity_id}/replace
pub async fn replace_file(
    State(state): State<AppState>,
    auth: AuthUser,
    Path(path): Path<RejectDownloadPath>,
    Json(body): Json<ReplaceFileRequest>,
) -> impl IntoResponse {
    if let Err(e) = auth.require_contributor() {
        return e.into_response();
    }
    use crate::repositories::{
        book_files, books, movie_files, movies, tv_episode_files, tv_episodes, tv_seasons,
    };

    let media_type = path.media_type.as_str();
    let entity_id = path.entity_id;

    if !matches!(media_type, "movie" | "tv_episode" | "book") {
        return (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({ "error": format!("Unsupported media_type for replace: {}", media_type) })),
        )
            .into_response();
    }

    let mut tx = match state.pool.begin().await {
        Ok(tx) => tx,
        Err(e) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({ "error": format!("Failed to begin transaction: {}", e) })),
            )
                .into_response();
        }
    };

    // Extract release identifiers and file paths before deleting
    let mut file_paths_to_delete: Vec<String> = Vec::new();
    let release_info = match media_type {
        "movie" => {
            let file = movie_files::get(&mut *tx, body.file_id)
                .await
                .ok()
                .flatten();
            if let Some(ref f) = file {
                file_paths_to_delete.push(f.file_path.clone());
            }
            let sm = file.as_ref().and_then(|f| f.source_metadata.as_ref());
            build_rejection_entry(sm, body.reason.as_deref())
        }
        "tv_episode" => {
            let file = tv_episode_files::get(&mut *tx, body.file_id)
                .await
                .ok()
                .flatten();
            if let Some(ref f) = file {
                file_paths_to_delete.push(f.file_path.clone());
            }
            let sm = file.as_ref().and_then(|f| f.source_metadata.as_ref());
            build_rejection_entry(sm, body.reason.as_deref())
        }
        "book" => {
            let row: Option<(Option<serde_json::Value>, String, Option<String>)> = sqlx::query_as(
                "SELECT source_metadata, file_path, source_path FROM book_files WHERE id = $1",
            )
            .bind(body.file_id)
            .fetch_optional(&mut *tx)
            .await
            .ok()
            .flatten();
            if let Some((_, ref path, ref source_path)) = row {
                file_paths_to_delete.push(path.clone());
                if let Some(sp) = source_path {
                    file_paths_to_delete.push(sp.clone());
                }
            }
            let sm = row.as_ref().and_then(|(sm, _, _)| sm.as_ref());
            build_rejection_entry(sm, body.reason.as_deref())
        }
        _ => unreachable!(),
    };

    // Delete the file record
    let delete_ok = match media_type {
        "movie" => match movie_files::delete(&mut *tx, body.file_id).await {
            Ok(Some(movie_id)) => {
                if movie_id != entity_id {
                    return (
                        StatusCode::BAD_REQUEST,
                        Json(serde_json::json!({ "error": "File does not belong to this movie" })),
                    )
                        .into_response();
                }
                true
            }
            _ => false,
        },
        "tv_episode" => match tv_episode_files::delete(&state.pool, body.file_id).await {
            Ok(Some(episode_ids)) => {
                if !episode_ids.contains(&entity_id) {
                    return (
                        StatusCode::BAD_REQUEST,
                        Json(
                            serde_json::json!({ "error": "File does not belong to this episode" }),
                        ),
                    )
                        .into_response();
                }
                true
            }
            _ => false,
        },
        "book" => matches!(book_files::delete(&mut *tx, body.file_id).await, Ok(true)),
        _ => unreachable!(),
    };

    if !delete_ok {
        return (
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({ "error": "File not found" })),
        )
            .into_response();
    }

    // Append to rejected_releases on the entity
    match media_type {
        "movie" => {
            let _ = movies::add_rejected_release(&mut *tx, entity_id, release_info).await;
        }
        "tv_episode" => {
            let _ = tv_episodes::add_rejected_release(&mut *tx, entity_id, release_info).await;
        }
        "book" => {
            let _ = books::add_rejected_release(&mut *tx, entity_id, release_info).await;
        }
        _ => unreachable!(),
    }

    // Set want_status to 'wanted'
    match media_type {
        "movie" => {
            let _ = movies::set_want_status(&mut *tx, entity_id, "wanted").await;
        }
        "tv_episode" => {
            let _ = tv_episodes::set_want_status(&mut *tx, entity_id, "wanted").await;
        }
        "book" => {
            let _ = books::set_want_status(&mut *tx, entity_id, "wanted").await;
        }
        _ => unreachable!(),
    }

    if let Err(e) = tx.commit().await {
        return (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({ "error": format!("Failed to commit: {}", e) })),
        )
            .into_response();
    }

    // Delete physical files from disk (after commit so we don't orphan DB records on failure)
    for file_path in &file_paths_to_delete {
        let path_buf = std::path::PathBuf::from(file_path);
        if path_buf.exists()
            && let Err(e) = tokio::fs::remove_file(&path_buf).await
        {
            tracing::warn!(error = %e, path = %file_path, "Failed to delete physical file during replace");
        }
    }

    // Enqueue the appropriate want job (outside transaction)
    let nats = state.nats.publisher();
    match media_type {
        "movie" => {
            let _ = nats
                .enqueue_job(
                    "kobbler",
                    "movie_want",
                    serde_json::json!({
                        "movie_id": entity_id,
                        "user_id": auth.user_id,
                        "manual": true,
                    }),
                )
                .await;
        }
        "tv_episode" => {
            if let Ok(Some(episode)) = tv_episodes::get(&state.pool, entity_id).await
                && let Ok(Some(season)) = tv_seasons::get(&state.pool, episode.season_id).await
            {
                let _ = nats
                    .enqueue_job(
                        "kobbler",
                        "tv_want",
                        serde_json::json!({
                            "show_id": season.show_id,
                            "season_id": episode.season_id,
                            "episode_ids": [entity_id],
                            "user_id": auth.user_id,
                            "manual": true,
                        }),
                    )
                    .await;
            }
        }
        "book" => {
            let _ = nats
                .enqueue_job(
                    "kobbler",
                    "book_want",
                    serde_json::json!({
                        "book_id": entity_id,
                        "user_id": auth.user_id,
                        "manual": true,
                    }),
                )
                .await;
        }
        _ => unreachable!(),
    }

    let _ = state
        .library_events
        .send(crate::events::LibraryEvent::library_updated(None));

    Json(serde_json::json!({
        "status": "replaced",
        "entity_id": entity_id,
        "media_type": path.media_type,
    }))
    .into_response()
}

/// Build a rejection entry from file source_metadata.
pub(crate) fn build_rejection_entry(
    source_metadata: Option<&serde_json::Value>,
    reason: Option<&str>,
) -> serde_json::Value {
    let release_title = source_metadata
        .and_then(|sm| sm.get("release_title"))
        .and_then(|v| v.as_str());
    let info_hash = source_metadata
        .and_then(|sm| sm.get("info_hash"))
        .and_then(|v| v.as_str());

    let mut entry = serde_json::Map::new();
    if let Some(t) = release_title {
        entry.insert("release_title".into(), serde_json::Value::String(t.into()));
    }
    if let Some(h) = info_hash {
        entry.insert("info_hash".into(), serde_json::Value::String(h.into()));
    }
    entry.insert(
        "reason".into(),
        serde_json::Value::String(reason.unwrap_or("user_replaced").into()),
    );
    serde_json::Value::Object(entry)
}
