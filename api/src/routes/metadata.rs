use axum::{
    Json,
    extract::{Path, Query, State},
    http::StatusCode,
    response::IntoResponse,
};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::AppState;
use crate::metadata::merge::merge_by_priority;
use crate::metadata::types::{Capability, ConfigFieldType, UpdateProviderRequest};
use crate::metadata::{
    MetadataQuery, MetadataResult, ProviderRegistry, SeriesQuery, SeriesSearchResult,
};
use crate::repositories::{ingest_queue, metadata_providers};
use crate::routes::AuthUser;
// tracking providers are created via crate::tracking::create_tracking_provider()

// ============================================================================
// Provider Response Types
// ============================================================================

/// Configuration field with current value for API response
#[derive(Debug, Serialize)]
pub struct ConfigFieldResponse {
    pub key: String,
    pub label: String,
    pub field_type: ConfigFieldType,
    pub required: bool,
    pub description: Option<String>,
    pub help_url: Option<String>,
    pub default: Option<String>,
    pub current_value: Option<serde_json::Value>,
}

/// Full provider info for API response (combines static info + DB state)
#[derive(Debug, Serialize)]
pub struct ProviderResponse {
    pub id: Uuid,
    pub provider_type: String,
    pub enabled: bool,
    pub priority: i32,
    // From static ProviderInfo
    pub display_name: String,
    pub description: String,
    pub website: String,
    pub capabilities: Vec<Capability>,
    // Dynamic config
    pub config_fields: Vec<ConfigFieldResponse>,
}

#[derive(Debug, Deserialize)]
pub struct SearchQuery {
    pub isbn: Option<String>,
    pub title: Option<String>,
    pub author: Option<String>,
    /// ASIN for audiobook lookup (e.g., via Audnexus)
    pub asin: Option<String>,
    /// Optional: filter to only search a specific provider (e.g., "hardcover")
    pub provider: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct SearchResponse {
    pub results: Vec<MetadataResult>,
}

/// Search for book metadata across all enabled providers
pub async fn search_metadata(
    State(state): State<AppState>,
    _auth: AuthUser,
    Query(params): Query<SearchQuery>,
) -> impl IntoResponse {
    let query = MetadataQuery {
        isbn: params.isbn,
        title: params.title,
        authors: params.author.into_iter().collect(),
        asin: params.asin,
        free_text: None,
    };

    if query.isbn.is_none() && query.title.is_none() && query.asin.is_none() {
        return (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({ "error": "Must provide isbn, title, or asin" })),
        )
            .into_response();
    }

    let registry = ProviderRegistry::new(state.pool.clone());
    let outcome = registry.search(&query, params.provider.as_deref()).await;

    Json(SearchResponse {
        results: outcome.results,
    })
    .into_response()
}

// ============================================================================
// Series Search
// ============================================================================

#[derive(Debug, Deserialize)]
pub struct SeriesSearchQuery {
    pub name: String,
    /// Optional: filter to only search a specific provider (e.g., "hardcover")
    pub provider: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct SeriesSearchResponse {
    pub results: Vec<SeriesSearchResult>,
}

/// Search for series by name across all enabled providers that support series search
pub async fn search_series(
    State(state): State<AppState>,
    _auth: AuthUser,
    Query(params): Query<SeriesSearchQuery>,
) -> impl IntoResponse {
    if params.name.is_empty() {
        return (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({ "error": "Must provide series name" })),
        )
            .into_response();
    }

    let query = SeriesQuery::new(&params.name);
    let registry = ProviderRegistry::new(state.pool.clone());

    // Check if any enabled provider supports series search
    let series_search_check = registry
        .check_series_search_availability(params.provider.as_deref())
        .await;

    if let Err(msg) = series_search_check {
        return (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({ "error": msg })),
        )
            .into_response();
    }

    let outcome = registry
        .search_series(&query, params.provider.as_deref())
        .await;

    Json(SeriesSearchResponse {
        results: outcome.results,
    })
    .into_response()
}

// ============================================================================
// Author Search
// ============================================================================

#[derive(Debug, Deserialize)]
pub struct AuthorSearchQuery {
    pub name: String,
    /// Optional: filter to only search a specific provider (e.g., "hardcover")
    pub provider: Option<String>,
}

/// Result from author search
#[derive(Debug, Clone, Serialize)]
pub struct AuthorSearchResult {
    pub provider: String,
    pub external_id: String,
    pub slug: Option<String>,
    pub name: String,
    pub image_url: Option<String>,
    /// Series names this author has written (for identification in search results)
    pub series_names: Vec<String>,
    pub book_count: Option<i32>,
    pub confidence: f64,
}

#[derive(Debug, Serialize)]
pub struct AuthorSearchResponse {
    pub results: Vec<AuthorSearchResult>,
}

/// Search for authors by name
/// GET /api/v1/admin/metadata/search-authors?name=X&provider=Y
pub async fn search_authors(
    State(state): State<AppState>,
    _auth: AuthUser,
    Query(params): Query<AuthorSearchQuery>,
) -> impl IntoResponse {
    if params.name.is_empty() {
        return (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({ "error": "Must provide author name" })),
        )
            .into_response();
    }

    let provider_id = params.provider.as_deref().unwrap_or("hardcover");

    // Get API key from config
    let api_key = match metadata_providers::get_api_key(&state.pool, provider_id).await {
        Ok(Some(key)) if !key.is_empty() => key,
        Ok(_) => {
            return (
                StatusCode::BAD_REQUEST,
                Json(serde_json::json!({ "error": format!("{} API key not configured", provider_id) })),
            )
                .into_response();
        }
        Err(e) => {
            tracing::error!(error = %e, provider = %provider_id, "Failed to get tracking provider API key");
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({ "error": "Database error" })),
            )
                .into_response();
        }
    };

    let Some(tracking_provider) =
        crate::tracking::create_tracking_provider(provider_id, api_key, state.pool.clone())
    else {
        return (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({ "error": format!("Provider '{}' does not support author search", provider_id) })),
        )
            .into_response();
    };

    match tracking_provider.search_authors(&params.name).await {
        Ok(outcome) => {
            let results: Vec<AuthorSearchResult> = outcome
                .data
                .into_iter()
                .map(|r| AuthorSearchResult {
                    provider: provider_id.to_string(),
                    external_id: r.id,
                    slug: r.slug,
                    name: r.name,
                    image_url: r.image_url,
                    series_names: r.series_names,
                    book_count: r.book_count,
                    confidence: 0.9, // High confidence for direct API results
                })
                .collect();
            Json(AuthorSearchResponse { results }).into_response()
        }
        Err(e) => {
            tracing::error!(error = %e, "Author search failed");
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({ "error": format!("Author search failed: {}", e) })),
            )
                .into_response()
        }
    }
}

/// List all metadata providers with their capabilities and config schema
pub async fn list_providers(State(state): State<AppState>, auth: AuthUser) -> impl IntoResponse {
    // Non-admins reach this for the Home provider selector and metadata pickers;
    // config_fields carries provider API keys, so it is admin-only.
    let expose_config = auth.require_admin().is_ok();
    let registry = ProviderRegistry::new(state.pool.clone());
    let stored_providers = registry.get_all_providers().await;

    // Combine stored provider data with static provider info
    let responses: Vec<ProviderResponse> = stored_providers
        .into_iter()
        .filter_map(|stored| {
            // Get static info for this provider type
            let info = registry.get_provider_info(&stored.provider_type)?;

            let config_fields: Vec<ConfigFieldResponse> = if expose_config {
                info.config_fields
                    .iter()
                    .map(|field| ConfigFieldResponse {
                        key: field.key.to_string(),
                        label: field.label.to_string(),
                        field_type: field.field_type,
                        required: field.required,
                        description: field.description.map(String::from),
                        help_url: field.help_url.map(String::from),
                        default: field.default.map(String::from),
                        current_value: stored.config.get(field.key).cloned(),
                    })
                    .collect()
            } else {
                Vec::new()
            };

            Some(ProviderResponse {
                id: stored.id,
                provider_type: stored.provider_type,
                enabled: stored.enabled,
                priority: stored.priority,
                display_name: info.display_name.to_string(),
                description: info.description.to_string(),
                website: info.website.to_string(),
                capabilities: info.capabilities.to_vec(),
                config_fields,
            })
        })
        .collect();

    Json(responses).into_response()
}

/// Update a metadata provider's configuration
pub async fn update_provider(
    State(state): State<AppState>,
    _auth: AuthUser,
    Path(provider_id): Path<Uuid>,
    Json(update): Json<UpdateProviderRequest>,
) -> impl IntoResponse {
    let registry = ProviderRegistry::new(state.pool.clone());
    match registry.update_provider(provider_id, &update).await {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(e) => {
            tracing::error!("Failed to update provider: {}", e);
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({ "error": "Failed to update provider" })),
            )
                .into_response()
        }
    }
}

/// Author with optional provider ID for enrichment
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct AuthorInput {
    pub name: String,
    /// Provider ID (e.g., Hardcover author ID)
    #[serde(default)]
    pub hardcover_id: Option<String>,
}

/// Apply metadata from a search result to an ingest queue item
#[derive(Debug, Deserialize, Serialize)]
pub struct ApplyMetadataRequest {
    pub title: Option<String>,
    /// Authors with optional Hardcover IDs
    pub authors: Option<Vec<AuthorInput>>,
    pub isbn: Option<String>,
    pub description: Option<String>,
    pub cover_url: Option<String>,
    pub provider: Option<String>,
    pub external_id: Option<String>,
    /// Series name (for creating/matching series)
    pub series: Option<String>,
    /// Series provider ID (Hardcover series ID)
    pub series_hardcover_id: Option<String>,
    /// Series index
    pub series_index: Option<f64>,
    /// Page count
    pub page_count: Option<u32>,
    /// Subjects/genres
    pub subjects: Option<Vec<String>>,
    /// Average rating (0.0 - 5.0 scale)
    pub rating: Option<f64>,
    /// Number of ratings
    pub rating_count: Option<i32>,
}

pub async fn apply_metadata(
    State(state): State<AppState>,
    _auth: AuthUser,
    Path(queue_item_id): Path<Uuid>,
    Json(metadata): Json<ApplyMetadataRequest>,
) -> impl IntoResponse {
    let registry = ProviderRegistry::new(state.pool.clone());

    // Fetch cover if URL provided
    let cover_data = if let (Some(provider), Some(url)) = (&metadata.provider, &metadata.cover_url)
    {
        match registry.fetch_cover(provider, url).await {
            Ok(data) => Some(data),
            Err(e) => {
                tracing::warn!("Failed to fetch cover: {}", e);
                None
            }
        }
    } else {
        None
    };

    // Update book metadata fields directly
    {
        use crate::repositories::books;

        // Build identifiers from ISBN and provider external_id
        let identifiers = {
            let mut ids = serde_json::Map::new();
            if let Some(ref isbn) = metadata.isbn {
                ids.insert("isbn".to_string(), serde_json::json!(isbn));
            }
            if let Some(ref external_id) = metadata.external_id
                && let Some(ref provider) = metadata.provider
            {
                ids.insert(provider.clone(), serde_json::json!(external_id));
            }
            if ids.is_empty() {
                None
            } else {
                Some(serde_json::Value::Object(ids))
            }
        };

        let update = books::BookUpdate {
            title: metadata.title.clone().map(|t| t.trim().to_string()),
            description: metadata.description.clone(),
            identifiers,
            page_count: metadata.page_count.map(|p| p as i32),
            rating: metadata.rating,
            rating_count: metadata.rating_count,
            ..Default::default()
        };

        if let Err(e) = books::update(&state.pool, queue_item_id, update).await {
            tracing::error!("Failed to apply metadata: {}", e);
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({ "error": "Failed to apply metadata" })),
            )
                .into_response();
        }

        // Update source_metadata on ingest source
        if let Ok(source_metadata) = serde_json::to_value(&metadata) {
            let _ = crate::repositories::book_ingest_source::update(
                &state.pool,
                queue_item_id,
                crate::repositories::book_ingest_source::BookIngestSourceUpdate {
                    source_metadata: Some(source_metadata),
                    ..Default::default()
                },
            )
            .await;
        }
    }

    // Link authors to the book - try to find existing authors, store rest in pending_authors
    if let Some(ref author_inputs) = metadata.authors {
        use crate::repositories::{authors, books};
        let mut pending_authors = Vec::new();

        // Clear existing author links first
        let _ = books::clear_authors(&state.pool, queue_item_id).await;

        for author in author_inputs {
            // Try to find existing author by name
            if let Ok(Some((author_id, _))) = authors::find_by_name(&state.pool, &author.name).await
            {
                // Link existing author
                let _ = books::link_author(&state.pool, queue_item_id, author_id).await;
            } else {
                // Store in pending_authors for creation on approval (with hardcover_id if available)
                let mut author_json = serde_json::json!({"name": author.name});
                if let Some(ref hardcover_id) = author.hardcover_id {
                    author_json["hardcover_id"] = serde_json::json!(hardcover_id);
                }
                pending_authors.push(author_json);
            }
        }

        // Update pending_authors if there are unlinked authors
        if !pending_authors.is_empty() {
            let _ = crate::repositories::book_ingest_source::update(
                &state.pool,
                queue_item_id,
                crate::repositories::book_ingest_source::BookIngestSourceUpdate {
                    pending_authors: Some(serde_json::json!(pending_authors)),
                    ..Default::default()
                },
            )
            .await;
        }
    }

    // Store series in pending_series (ingest items don't create series until approval)
    if metadata.series.is_some() || metadata.series_index.is_some() {
        let pending_series = metadata.series.as_ref().map(|s| {
            let mut json = serde_json::json!({"name": s});
            if let Some(ref hc_id) = metadata.series_hardcover_id {
                json["hardcover_id"] = serde_json::json!(hc_id);
            }
            json
        });
        let _ = crate::repositories::book_ingest_source::update(
            &state.pool,
            queue_item_id,
            crate::repositories::book_ingest_source::BookIngestSourceUpdate {
                pending_series,
                pending_series_index: metadata.series_index,
                ..Default::default()
            },
        )
        .await;
    }

    // Store genres/subjects
    if let Some(ref subjects) = metadata.subjects {
        use crate::repositories::genres;
        if let Err(e) = genres::set_book_genres(&state.pool, queue_item_id, subjects).await {
            tracing::warn!(error = %e, "Failed to set book genres from metadata");
        }
    }

    // Store cover data in the queue item if we fetched it
    if let Some(data) = cover_data {
        tracing::info!("Storing fetched cover image: {} bytes", data.len());
        if let Err(e) = ingest_queue::update_cover(&state.pool, queue_item_id, &data).await {
            tracing::error!("Failed to store cover: {}", e);
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({ "error": "Failed to store cover" })),
            )
                .into_response();
        }
    }

    StatusCode::NO_CONTENT.into_response()
}

// ============================================================================
// Metadata Merge
// ============================================================================

/// Request to merge multiple metadata results by priority
#[derive(Debug, Deserialize)]
pub struct MergeMetadataRequest {
    /// Results ordered by priority (first = highest priority)
    pub results: Vec<MetadataResult>,
}

#[derive(Debug, Serialize)]
pub struct MergeMetadataResponse {
    pub merged: MetadataResult,
}

/// Merge multiple metadata results by priority and return the merged result.
/// This is useful for previewing the merge before applying.
///
/// POST /api/v1/admin/metadata/merge
pub async fn merge_metadata(
    _auth: AuthUser,
    Json(request): Json<MergeMetadataRequest>,
) -> impl IntoResponse {
    if request.results.is_empty() {
        return (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({ "error": "At least one result is required" })),
        )
            .into_response();
    }

    match merge_by_priority(&request.results) {
        Some(merged) => Json(MergeMetadataResponse { merged }).into_response(),
        None => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({ "error": "Failed to merge results" })),
        )
            .into_response(),
    }
}
