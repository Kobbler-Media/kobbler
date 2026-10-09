//! Sync provider API endpoints.
//!
//! Handles book linking for external providers like Hardcover.

use axum::{
    Json, Router,
    extract::{Path, State},
    http::StatusCode,
    routing::{delete, get, post},
};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::{
    AppState,
    repositories::{books, user_sync_providers},
    routes::AuthUser,
    sync_providers::SyncProviderRegistry,
};

// ============================================================================
// Route Setup
// ============================================================================

pub fn routes() -> Router<AppState> {
    Router::new()
        // Book linking
        .route("/books/{book_id}/links", get(get_book_links))
        .route(
            "/books/{book_id}/links/{provider}/search",
            post(search_provider_for_book),
        )
        .route("/books/{book_id}/links/{provider}", post(link_book))
        .route("/books/{book_id}/links/{provider}", delete(unlink_book))
        // Rating
        .route("/books/{book_id}/rate/{provider}", post(rate_book))
}

// ============================================================================
// Book Linking Types
// ============================================================================

/// Response showing all provider links for a book
#[derive(Debug, Serialize)]
struct BookLinksResponse {
    book_id: Uuid,
    links: Vec<BookProviderLink>,
}

/// A link between a book and a provider
#[derive(Debug, Serialize)]
struct BookProviderLink {
    provider: String,
    external_id: String,
}

/// Request to search for a book on a provider
#[derive(Debug, Deserialize)]
struct SearchRequest {
    query: String,
}

/// Request to link a book to a provider
#[derive(Debug, Deserialize)]
struct LinkBookRequest {
    external_id: String,
}

/// Request to rate a book
#[derive(Debug, Deserialize)]
struct RateBookRequest {
    rating: i32,
}

/// Search result response
#[derive(Debug, Serialize)]
struct SearchResultsResponse {
    results: Vec<crate::sync_providers::SyncBookResult>,
}

// ============================================================================
// Book Linking Endpoints
// ============================================================================

/// Get all provider links for a book.
async fn get_book_links(
    State(state): State<AppState>,
    _auth: AuthUser,
    Path(book_id): Path<Uuid>,
) -> Result<Json<BookLinksResponse>, StatusCode> {
    // Get book identifiers
    let identifiers = books::get_identifiers(&state.pool, book_id)
        .await
        .ok()
        .flatten()
        .unwrap_or(serde_json::json!({}));

    // Get available providers
    let registry = SyncProviderRegistry::new();
    let factories = registry.list_factories();

    // Build links list
    let mut links = Vec::new();
    for factory in factories {
        let info = factory.info();
        // The identifier_key is used to check if the book is linked
        let key = info.id; // For hardcover, this is "hardcover"
        if let Some(external_id) = identifiers.get(key).and_then(|v| v.as_str()) {
            links.push(BookProviderLink {
                provider: info.id.to_string(),
                external_id: external_id.to_string(),
            });
        }
    }

    Ok(Json(BookLinksResponse { book_id, links }))
}

/// Search a provider for matching books.
async fn search_provider_for_book(
    State(state): State<AppState>,
    auth: AuthUser,
    Path((book_id, provider)): Path<(Uuid, String)>,
    Json(request): Json<SearchRequest>,
) -> Result<Json<SearchResultsResponse>, StatusCode> {
    // Verify book exists
    if !books::exists(&state.pool, book_id)
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
    {
        return Err(StatusCode::NOT_FOUND);
    }

    // Get user's API key for this provider
    let api_key = user_sync_providers::get_api_key(&state.pool, auth.user_id, &provider)
        .await
        .ok()
        .flatten();

    let Some(api_key) = api_key else {
        return Err(StatusCode::BAD_REQUEST); // No API key configured
    };

    let registry = SyncProviderRegistry::new();
    let Some(factory) = registry.get_factory(&provider) else {
        return Err(StatusCode::NOT_FOUND);
    };

    let client = crate::http::new_client();
    let provider_impl = factory.create(client, &api_key);

    let results = provider_impl
        .search_book(&request.query)
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    Ok(Json(SearchResultsResponse { results }))
}

/// Link a book to a provider.
async fn link_book(
    State(state): State<AppState>,
    auth: AuthUser,
    Path((book_id, provider)): Path<(Uuid, String)>,
    Json(request): Json<LinkBookRequest>,
) -> Result<StatusCode, StatusCode> {
    auth.require_contributor()
        .map_err(|_| StatusCode::FORBIDDEN)?;

    // Verify provider exists
    let registry = SyncProviderRegistry::new();
    if registry.get_factory(&provider).is_none() {
        return Err(StatusCode::NOT_FOUND);
    }

    // Verify book exists
    if !books::exists(&state.pool, book_id)
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
    {
        return Err(StatusCode::NOT_FOUND);
    }

    // Get existing identifiers
    let mut identifiers = books::get_identifiers(&state.pool, book_id)
        .await
        .ok()
        .flatten()
        .unwrap_or(serde_json::json!({}));

    // Add the link
    if let Some(obj) = identifiers.as_object_mut() {
        obj.insert(
            provider.clone(),
            serde_json::Value::String(request.external_id),
        );
    }

    // Update book
    books::set_identifiers(&state.pool, book_id, identifiers)
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    Ok(StatusCode::NO_CONTENT)
}

/// Unlink a book from a provider.
async fn unlink_book(
    State(state): State<AppState>,
    auth: AuthUser,
    Path((book_id, provider)): Path<(Uuid, String)>,
) -> Result<StatusCode, StatusCode> {
    auth.require_contributor()
        .map_err(|_| StatusCode::FORBIDDEN)?;

    // Get existing identifiers
    let mut identifiers = books::get_identifiers(&state.pool, book_id)
        .await
        .ok()
        .flatten()
        .unwrap_or(serde_json::json!({}));

    // Remove the link
    if let Some(obj) = identifiers.as_object_mut() {
        obj.remove(&provider);
    }

    // Update book
    books::set_identifiers(&state.pool, book_id, identifiers)
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    Ok(StatusCode::NO_CONTENT)
}

/// Rate a book on a provider.
async fn rate_book(
    State(state): State<AppState>,
    auth: AuthUser,
    Path((book_id, provider)): Path<(Uuid, String)>,
    Json(request): Json<RateBookRequest>,
) -> Result<StatusCode, StatusCode> {
    // Get book identifiers and provider metadata
    let (identifiers, provider_metadata) =
        books::get_identifiers_and_metadata(&state.pool, book_id)
            .await
            .ok()
            .flatten()
            .ok_or(StatusCode::NOT_FOUND)?;

    // Check if book is linked to this provider
    let external_id = identifiers
        .get(&provider)
        .and_then(|v| v.as_str())
        .ok_or(StatusCode::BAD_REQUEST)?;

    // Get user's API key for this provider
    let api_key = user_sync_providers::get_api_key(&state.pool, auth.user_id, &provider)
        .await
        .ok()
        .flatten();

    let api_key = api_key.ok_or(StatusCode::BAD_REQUEST)?;

    let registry = SyncProviderRegistry::new();
    let factory = registry
        .get_factory(&provider)
        .ok_or(StatusCode::NOT_FOUND)?;

    // Rate the book
    let client = crate::http::new_client();
    let provider_impl = factory.create(client, &api_key);

    provider_impl
        .set_rating(external_id, request.rating, &provider_metadata)
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    Ok(StatusCode::NO_CONTENT)
}
