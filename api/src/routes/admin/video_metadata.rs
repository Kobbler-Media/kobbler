use std::collections::HashMap;

use axum::{
    Json,
    extract::{Path, Query, State},
    http::StatusCode,
    response::IntoResponse,
};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::AppState;
use crate::routes::AuthUser;
use crate::routes::admin::library_status::{LibraryStatus, batch_movie_status, batch_tv_status};
use crate::video_metadata::merge;
use crate::video_metadata::types::{
    UpdateVideoProviderRequest, VideoCapability, VideoConfigFieldType, VideoIdentifiers,
};
use crate::video_metadata::{
    MovieQuery, MovieResult, TvQuery, TvShowResult, VideoProviderRegistry,
};

// ============================================================================
// Provider Response Types
// ============================================================================

#[derive(Debug, Serialize)]
pub struct VideoConfigFieldResponse {
    pub key: String,
    pub label: String,
    pub field_type: VideoConfigFieldType,
    pub required: bool,
    pub description: Option<String>,
    pub help_url: Option<String>,
    pub default: Option<String>,
    pub current_value: Option<serde_json::Value>,
}

#[derive(Debug, Serialize)]
pub struct VideoProviderResponse {
    pub id: Uuid,
    pub provider_type: String,
    pub enabled: bool,
    pub priority: i32,
    pub display_name: String,
    pub description: String,
    pub website: String,
    pub capabilities: Vec<VideoCapability>,
    pub config_fields: Vec<VideoConfigFieldResponse>,
}

// ============================================================================
// List Providers
// ============================================================================

/// List all video metadata providers with their capabilities and config schema.
/// GET /api/v1/admin/video-metadata/providers
pub async fn list_providers(State(state): State<AppState>, auth: AuthUser) -> impl IntoResponse {
    // Non-admins reach this for the Home provider selector and metadata pickers;
    // config_fields carries provider API keys, so it is admin-only.
    let expose_config = auth.require_admin().is_ok();
    let registry = VideoProviderRegistry::new(state.pool.clone());
    let stored_providers = registry.get_all_providers().await;

    let responses: Vec<VideoProviderResponse> = stored_providers
        .into_iter()
        .filter_map(|stored| {
            let info = registry.get_provider_info(&stored.provider_type)?;

            let config_fields: Vec<VideoConfigFieldResponse> = if expose_config {
                info.config_fields
                    .iter()
                    .map(|field| VideoConfigFieldResponse {
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

            Some(VideoProviderResponse {
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

// ============================================================================
// Update Provider
// ============================================================================

/// Update a video metadata provider's configuration.
/// PATCH /api/v1/admin/video-metadata/providers/:id
pub async fn update_provider(
    State(state): State<AppState>,
    auth: AuthUser,
    Path(provider_id): Path<Uuid>,
    Json(update): Json<UpdateVideoProviderRequest>,
) -> impl IntoResponse {
    if let Err(e) = auth.require_admin() {
        return e.into_response();
    }
    let registry = VideoProviderRegistry::new(state.pool.clone());
    match registry.update_provider(provider_id, &update).await {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(e) => {
            tracing::error!("Failed to update video provider: {}", e);
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({ "error": "Failed to update provider" })),
            )
                .into_response()
        }
    }
}

// ============================================================================
// Library status merge
// ============================================================================

/// A provider search result with library presence merged in, so the clients can
/// send an owned title straight to its library page.
#[derive(Debug, Serialize)]
pub struct SearchResultWithStatus<T: Serialize> {
    #[serde(flatten)]
    pub result: T,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub library_id: Option<Uuid>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub want_status: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub is_in_library: Option<bool>,
}

impl<T: Serialize> SearchResultWithStatus<T> {
    fn new(tmdb_id: Option<String>, result: T, status: &HashMap<String, LibraryStatus>) -> Self {
        let found = tmdb_id.as_deref().and_then(|id| status.get(id));
        Self {
            result,
            library_id: found.map(|s| s.id),
            want_status: found.and_then(|s| s.want_status.clone()),
            is_in_library: found.and_then(|s| s.is_in_library),
        }
    }
}

fn tmdb_ids_of<'a>(identifiers: impl Iterator<Item = &'a VideoIdentifiers>) -> Vec<String> {
    identifiers.filter_map(|i| i.tmdb_id.clone()).collect()
}

// ============================================================================
// Movie Search
// ============================================================================

#[derive(Debug, Deserialize)]
pub struct MovieSearchQuery {
    pub q: String,
    pub year: Option<u16>,
    pub provider: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct MovieSearchResponse {
    pub results: Vec<SearchResultWithStatus<MovieResult>>,
}

/// Search for movies across enabled video metadata providers.
/// GET /api/v1/admin/video-metadata/search/movies?q=&year=&provider=
pub async fn search_movies(
    State(state): State<AppState>,
    _auth: AuthUser,
    Query(params): Query<MovieSearchQuery>,
) -> impl IntoResponse {
    if params.q.is_empty() {
        return (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({ "error": "Query parameter 'q' is required" })),
        )
            .into_response();
    }

    let query = MovieQuery {
        title: params.q,
        year: params.year,
    };

    let registry = VideoProviderRegistry::new(state.pool.clone());
    let outcome = registry
        .search_movies(&query, params.provider.as_deref())
        .await;

    let tmdb_ids = tmdb_ids_of(outcome.results.iter().map(|r| &r.identifiers));
    let status = batch_movie_status(&state.pool, &tmdb_ids).await;

    Json(MovieSearchResponse {
        results: outcome
            .results
            .into_iter()
            .map(|r| SearchResultWithStatus::new(r.identifiers.tmdb_id.clone(), r, &status))
            .collect(),
    })
    .into_response()
}

// ============================================================================
// TV Search
// ============================================================================

#[derive(Debug, Deserialize)]
pub struct TvSearchQuery {
    pub q: String,
    pub provider: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct TvSearchResponse {
    pub results: Vec<SearchResultWithStatus<TvShowResult>>,
}

/// Search for TV shows across enabled video metadata providers.
/// GET /api/v1/admin/video-metadata/search/tv?q=&provider=
pub async fn search_tv(
    State(state): State<AppState>,
    _auth: AuthUser,
    Query(params): Query<TvSearchQuery>,
) -> impl IntoResponse {
    if params.q.is_empty() {
        return (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({ "error": "Query parameter 'q' is required" })),
        )
            .into_response();
    }

    let query = TvQuery {
        title: params.q,
        ..Default::default()
    };

    let registry = VideoProviderRegistry::new(state.pool.clone());
    let outcome = registry.search_tv(&query, params.provider.as_deref()).await;

    let tmdb_ids = tmdb_ids_of(outcome.results.iter().map(|r| &r.identifiers));
    let status = batch_tv_status(&state.pool, &tmdb_ids).await;

    Json(TvSearchResponse {
        results: outcome
            .results
            .into_iter()
            .map(|r| SearchResultWithStatus::new(r.identifiers.tmdb_id.clone(), r, &status))
            .collect(),
    })
    .into_response()
}

// ============================================================================
// Movie Details
// ============================================================================

/// Get full movie details from a specific provider.
/// GET /api/v1/admin/video-metadata/movies/:provider_id?provider=tmdb
pub async fn get_movie_details(
    State(state): State<AppState>,
    _auth: AuthUser,
    Path(provider_id): Path<String>,
    Query(params): Query<ProviderQuery>,
) -> impl IntoResponse {
    let registry = VideoProviderRegistry::new(state.pool.clone());
    let default_provider = registry
        .preferred_provider(VideoCapability::MovieDetails)
        .await
        .unwrap_or_else(|| "tmdb".to_string());
    let provider_type = params.provider.as_deref().unwrap_or(&default_provider);

    match registry.get_movie(provider_type, &provider_id).await {
        Ok(details) => Json(details).into_response(),
        Err(e) => {
            tracing::warn!("Failed to get movie details: {}", e);
            (
                StatusCode::from_u16(error_status(&e)).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR),
                Json(serde_json::json!({ "error": e.to_string() })),
            )
                .into_response()
        }
    }
}

// ============================================================================
// TV Show Details
// ============================================================================

/// Get full TV show details from a specific provider.
/// GET /api/v1/admin/video-metadata/tv/:provider_id?provider=tmdb
pub async fn get_tv_details(
    State(state): State<AppState>,
    _auth: AuthUser,
    Path(provider_id): Path<String>,
    Query(params): Query<ProviderQuery>,
) -> impl IntoResponse {
    let registry = VideoProviderRegistry::new(state.pool.clone());
    let default_provider = registry
        .preferred_provider(VideoCapability::TvDetails)
        .await
        .unwrap_or_else(|| "tmdb".to_string());
    let provider_type = params.provider.as_deref().unwrap_or(&default_provider);

    match registry.get_tv_show(provider_type, &provider_id).await {
        Ok(details) => Json(details).into_response(),
        Err(e) => {
            tracing::warn!("Failed to get TV show details: {}", e);
            (
                StatusCode::from_u16(error_status(&e)).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR),
                Json(serde_json::json!({ "error": e.to_string() })),
            )
                .into_response()
        }
    }
}

// ============================================================================
// Season Details
// ============================================================================

#[derive(Debug, Deserialize)]
pub struct SeasonPath {
    pub provider_id: String,
    pub season_number: u32,
}

/// Get season details (episodes) from a specific provider.
/// GET /api/v1/admin/video-metadata/tv/:provider_id/season/:season_number?provider=tmdb
pub async fn get_season_details(
    State(state): State<AppState>,
    _auth: AuthUser,
    Path(path): Path<SeasonPath>,
    Query(params): Query<ProviderQuery>,
) -> impl IntoResponse {
    let registry = VideoProviderRegistry::new(state.pool.clone());
    let default_provider = registry
        .preferred_provider(VideoCapability::SeasonDetails)
        .await
        .unwrap_or_else(|| "tmdb".to_string());
    let provider_type = params.provider.as_deref().unwrap_or(&default_provider);

    match registry
        .get_season(provider_type, &path.provider_id, path.season_number)
        .await
    {
        Ok(details) => Json(details).into_response(),
        Err(e) => {
            tracing::warn!("Failed to get season details: {}", e);
            (
                StatusCode::from_u16(error_status(&e)).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR),
                Json(serde_json::json!({ "error": e.to_string() })),
            )
                .into_response()
        }
    }
}

// ============================================================================
// Shared Types
// ============================================================================

#[derive(Debug, Deserialize)]
pub struct ProviderQuery {
    pub provider: Option<String>,
}

fn error_status(e: &crate::video_metadata::VideoProviderError) -> u16 {
    use crate::video_metadata::VideoProviderError;
    match e {
        VideoProviderError::NotFound => 404,
        VideoProviderError::NotSupported => 400,
        VideoProviderError::RateLimited => 429,
        _ => 500,
    }
}

// ============================================================================
// Merge Results
// ============================================================================

#[derive(Debug, Deserialize)]
#[serde(tag = "type")]
pub enum MergeRequest {
    #[serde(rename = "movie")]
    Movie { results: Vec<MovieResult> },
    #[serde(rename = "tv")]
    Tv { results: Vec<TvShowResult> },
}

#[derive(Debug, Serialize)]
#[serde(untagged)]
pub enum MergeResponse {
    Movie(MovieResult),
    Tv(TvShowResult),
}

/// Merge multiple search results into one using priority-based field selection.
/// POST /api/v1/admin/video-metadata/merge
pub async fn merge_results(auth: AuthUser, Json(request): Json<MergeRequest>) -> impl IntoResponse {
    // Pure merge over results the caller already has; reached from the add
    // movie/show flow on /movies and /tv.
    if let Err(e) = auth.require_contributor() {
        return e.into_response();
    }
    match request {
        MergeRequest::Movie { results } => match merge::merge_movie_results(&results) {
            Some(merged) => Json(MergeResponse::Movie(merged)).into_response(),
            None => (
                StatusCode::BAD_REQUEST,
                Json(serde_json::json!({ "error": "No results to merge" })),
            )
                .into_response(),
        },
        MergeRequest::Tv { results } => match merge::merge_tv_results(&results) {
            Some(merged) => Json(MergeResponse::Tv(merged)).into_response(),
            None => (
                StatusCode::BAD_REQUEST,
                Json(serde_json::json!({ "error": "No results to merge" })),
            )
                .into_response(),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::video_metadata::types::VideoIdentifiers;

    fn movie() -> MovieResult {
        MovieResult {
            provider: "tmdb".into(),
            provider_id: "603".into(),
            title: "The Matrix".into(),
            year: Some(1999),
            description: None,
            poster_url: None,
            rating: None,
            rating_count: None,
            identifiers: VideoIdentifiers {
                tmdb_id: Some("603".into()),
                ..Default::default()
            },
            genres: vec![],
            network: None,
        }
    }

    // kobapp decodes these results with kotlinx.serialization, which throws on a
    // missing non-optional field. Pin the key names so a rename here can't
    // silently empty out the app's search results.
    #[test]
    fn search_result_wire_shape() {
        let status = HashMap::new();
        let wrapped = SearchResultWithStatus::new(Some("603".into()), movie(), &status);
        let v = serde_json::to_value(&wrapped).unwrap();
        let obj = v.as_object().unwrap();

        assert!(obj.contains_key("external_id"));
        assert!(!obj.contains_key("provider_id"));
        assert!(!obj.contains_key("media_type"));
        assert!(
            !obj.contains_key("library_id"),
            "absent when not in library"
        );
    }

    #[test]
    fn search_result_carries_library_status() {
        let id = Uuid::new_v4();
        let mut status = HashMap::new();
        status.insert(
            "603".to_string(),
            LibraryStatus {
                id,
                want_status: Some("wanted".into()),
                is_in_library: Some(false),
            },
        );

        let wrapped = SearchResultWithStatus::new(Some("603".into()), movie(), &status);
        let v = serde_json::to_value(&wrapped).unwrap();

        assert_eq!(v["library_id"], serde_json::json!(id));
        assert_eq!(v["want_status"], "wanted");
        assert_eq!(v["is_in_library"], false);
    }
}
