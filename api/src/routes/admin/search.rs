//! Search-related endpoints for the admin API.

use axum::{
    Json,
    extract::{Query, State},
    http::StatusCode,
    response::IntoResponse,
};
use meilisearch_sdk::client::Client;
use serde::{Deserialize, Serialize};

use crate::AppState;
use crate::jobs::handlers::{SearchReindexPayload, get_static_commands, get_static_pages};
use crate::routes::AuthUser;

#[derive(Debug, Deserialize)]
pub struct SearchQuery {
    /// Search query string
    pub q: String,
    /// Max results per type (default: 5)
    #[serde(default = "default_limit")]
    pub limit: usize,
}

fn default_limit() -> usize {
    5
}

/// Unified search across all entity types with relevance ranking.
/// GET /api/v1/admin/search?q=...&limit=5
///
/// Returns results ranked by relevance across all indexes.
/// `top_hits` contains the best hit of each type (ordered by relevance).
/// `remaining` contains all other results in relevance order.
pub async fn search(
    State(state): State<AppState>,
    _auth: AuthUser,
    Query(query): Query<SearchQuery>,
) -> impl IntoResponse {
    if query.q.trim().is_empty() {
        return (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({ "error": "Search query cannot be empty" })),
        )
            .into_response();
    }

    match state.search.unified_search(&query.q, query.limit).await {
        Ok(result) => Json(result).into_response(),
        Err(e) => {
            tracing::error!(error = %e, "Search query failed");
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({ "error": "Search failed" })),
            )
                .into_response()
        }
    }
}

/// Get search backend status.
/// GET /api/v1/admin/search/status
pub async fn search_status(State(state): State<AppState>, _auth: AuthUser) -> impl IntoResponse {
    let status = state.search.status().await;
    Json(status).into_response()
}

/// Request to test a MeiliSearch connection.
#[derive(Debug, Deserialize)]
pub struct TestConnectionRequest {
    pub url: String,
    pub master_key: String,
}

/// Response from testing a MeiliSearch connection.
#[derive(Debug, Serialize)]
pub struct TestConnectionResponse {
    pub success: bool,
    pub message: String,
    pub version: Option<String>,
}

/// Test a MeiliSearch connection without affecting the running backend.
/// POST /api/v1/admin/search/test
pub async fn test_connection(
    auth: AuthUser,
    Json(payload): Json<TestConnectionRequest>,
) -> impl IntoResponse {
    if let Err(e) = auth.require_admin() {
        return e.into_response();
    }
    // Try to create a client and get version info
    let client = match Client::new(&payload.url, Some(&payload.master_key)) {
        Ok(c) => c,
        Err(e) => {
            return Json(TestConnectionResponse {
                success: false,
                message: format!("Failed to create client: {}", e),
                version: None,
            })
            .into_response();
        }
    };

    // Try to get version to verify connection
    match client.get_version().await {
        Ok(version) => Json(TestConnectionResponse {
            success: true,
            message: "Connection successful".to_string(),
            version: Some(version.pkg_version),
        })
        .into_response(),
        Err(e) => Json(TestConnectionResponse {
            success: false,
            message: format!("Connection failed: {}", e),
            version: None,
        })
        .into_response(),
    }
}

/// Trigger a full reindex of all entities (including static pages/commands).
/// POST /api/v1/admin/search/reindex
pub async fn trigger_reindex(
    State(state): State<AppState>,
    auth: AuthUser,
    body: Option<axum::Json<SearchReindexPayload>>,
) -> impl IntoResponse {
    if let Err(e) = auth.require_admin() {
        return e.into_response();
    }
    use crate::jobs::JobContext;

    let payload = body.map(|b| b.0).unwrap_or_default();
    let description = if let Some(types) = &payload.entity_types {
        format!("Rebuilding search indexes: {}", types.join(", "))
    } else {
        "Rebuilding search indexes".to_string()
    };
    let context = JobContext::new(description);
    match crate::jobs::enqueue(
        &state.pool,
        state.nats.publisher(),
        "search_reindex",
        serde_json::to_value(&payload).unwrap(),
        context,
    )
    .await
    {
        Ok(job_id) => Json(serde_json::json!({
            "message": "Reindex job enqueued",
            "job_id": job_id
        }))
        .into_response(),
        Err(e) => {
            tracing::error!(error = %e, "Failed to enqueue reindex job");
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({ "error": "Failed to start reindex" })),
            )
                .into_response()
        }
    }
}

/// Sync static content (pages, commands) to the search index.
/// POST /api/v1/admin/search/sync-static
///
/// This indexes pages and commands for fuzzy search in the command palette.
/// The reindex endpoint also syncs static content, but this endpoint is
/// faster for just updating pages/commands.
pub async fn sync_static(State(state): State<AppState>, auth: AuthUser) -> impl IntoResponse {
    if let Err(e) = auth.require_admin() {
        return e.into_response();
    }
    let pages = get_static_pages();
    let commands = get_static_commands();

    let pages_count = pages.len();
    let commands_count = commands.len();

    // Reindex pages
    if let Err(e) = state.search.reindex_all_pages(pages).await {
        tracing::error!(error = %e, "Failed to reindex pages");
        return (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({ "error": "Failed to reindex pages" })),
        )
            .into_response();
    }

    // Reindex commands
    if let Err(e) = state.search.reindex_all_commands(commands).await {
        tracing::error!(error = %e, "Failed to reindex commands");
        return (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({ "error": "Failed to reindex commands" })),
        )
            .into_response();
    }

    Json(serde_json::json!({
        "message": "Static content synced to search index",
        "pages_count": pages_count,
        "commands_count": commands_count
    }))
    .into_response()
}
