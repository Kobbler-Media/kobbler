//! Monitoring-related route handlers (author/series monitoring and metadata application).

use axum::{
    Json as AxumJson,
    extract::{Json, Path, State},
    http::StatusCode,
    response::IntoResponse,
};
use serde::Deserialize;
use uuid::Uuid;

use crate::AppState;
use crate::repositories::{
    authors as authors_repo, books as books_repo, provider_reconcile, series as series_repo,
};
use crate::routes::AuthUser;

// =============================================================================
// Request Types
// =============================================================================

#[derive(Debug, Deserialize)]
pub struct UpdateMonitoringRequest {
    pub is_monitored: Option<bool>,
    pub auto_want_mode: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct ApplyAuthorMetadataRequest {
    pub provider: String,
    pub provider_id: String,
    pub provider_slug: Option<String>,
    pub image_url: Option<String>,
    pub bio: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct ApplySeriesMetadataRequest {
    pub provider: String,
    pub provider_id: String,
    pub provider_slug: Option<String>,
    pub image_url: Option<String>,
    pub description: Option<String>,
    pub total_book_count: Option<i32>,
}

// =============================================================================
// Author Monitoring Handlers
// =============================================================================

/// PUT /api/v1/library/authors/{author_id}/monitoring
/// Update monitoring settings for an author
pub async fn update_author_monitoring(
    State(state): State<AppState>,
    _auth: AuthUser,
    Path(author_id): Path<Uuid>,
    Json(payload): Json<UpdateMonitoringRequest>,
) -> impl IntoResponse {
    // Validate auto_want_mode if provided
    if let Some(ref mode) = payload.auto_want_mode {
        let valid_modes = ["none", "new_only", "all"];
        if !valid_modes.contains(&mode.as_str()) {
            return (
                StatusCode::BAD_REQUEST,
                format!("invalid auto_want_mode: must be one of {:?}", valid_modes),
            )
                .into_response();
        }
    }

    if payload.is_monitored.is_none() && payload.auto_want_mode.is_none() {
        return StatusCode::NO_CONTENT.into_response();
    }

    match authors_repo::update_monitoring(
        &state.pool,
        author_id,
        payload.is_monitored,
        payload.auto_want_mode.as_deref(),
    )
    .await
    {
        Ok(true) => {
            // When auto_want_mode changes to "none", clear want_status on discovered books
            if payload.auto_want_mode.as_deref() == Some("none")
                && let Err(e) =
                    books_repo::clear_auto_wanted_for_author(&state.pool, author_id).await
            {
                tracing::warn!(error = %e, author_id = %author_id, "failed to clear auto-wanted books");
            }

            // When monitoring is turned on, trigger an immediate sync
            if payload.is_monitored == Some(true) {
                let _ = state
                    .nats
                    .publisher()
                    .enqueue_job(
                        "kobbler",
                        "author_enrich",
                        serde_json::json!({ "author_id": author_id }),
                    )
                    .await;
                tracing::info!(author_id = %author_id, "Author monitoring enabled, enrich job queued");
            }

            StatusCode::NO_CONTENT.into_response()
        }
        Ok(false) => (StatusCode::NOT_FOUND, "author not found").into_response(),
        Err(e) => {
            tracing::error!(error = %e, author_id = %author_id, "failed to update author monitoring");
            (StatusCode::INTERNAL_SERVER_ERROR, "database error").into_response()
        }
    }
}

/// POST /api/v1/library/authors/{author_id}/refresh
/// Trigger metadata refresh for an author
pub async fn refresh_author_metadata(
    State(state): State<AppState>,
    _auth: AuthUser,
    Path(author_id): Path<Uuid>,
) -> impl IntoResponse {
    // Verify author exists
    let author_name = match authors_repo::get_name(&state.pool, author_id).await {
        Ok(Some(name)) => name,
        Ok(None) => return (StatusCode::NOT_FOUND, "author not found").into_response(),
        Err(e) => {
            tracing::error!(error = %e, "failed to fetch author");
            return (StatusCode::INTERNAL_SERVER_ERROR, "database error").into_response();
        }
    };

    // Enqueue author enrichment job
    let _ = state
        .nats
        .publisher()
        .enqueue_job(
            "kobbler",
            "author_enrich",
            serde_json::json!({ "author_id": author_id }),
        )
        .await;

    AxumJson(serde_json::json!({
        "message": format!("Metadata refresh queued for author: {}", author_name)
    }))
    .into_response()
}

/// POST /api/v1/library/authors/{author_id}/apply-metadata
/// Apply metadata from a search result to an author (sets provider ID, etc.)
pub async fn apply_author_metadata(
    State(state): State<AppState>,
    _auth: AuthUser,
    Path(author_id): Path<Uuid>,
    Json(payload): Json<ApplyAuthorMetadataRequest>,
) -> impl IntoResponse {
    if payload.provider.is_empty() || payload.provider_id.is_empty() {
        return (
            StatusCode::BAD_REQUEST,
            "provider and provider_id are required",
        )
            .into_response();
    }

    let mut identifiers = serde_json::json!({ &payload.provider: payload.provider_id });
    let slug_key = format!("{}_slug", payload.provider);
    if let Some(ref slug) = payload.provider_slug {
        identifiers[&slug_key] = serde_json::json!(slug);
    }

    // Convert image_url to ImageMap
    let images = payload.image_url.as_ref().map(|url| {
        let mut map = crate::images::ImageMap::new();
        map.insert(
            "profile".to_string(),
            crate::images::ImageEntry {
                source_url: Some(url.clone()),
                ..Default::default()
            },
        );
        map
    });

    match authors_repo::apply_metadata(
        &state.pool,
        author_id,
        identifiers,
        images,
        payload.bio.as_deref(),
    )
    .await
    {
        Ok(true) => {
            tracing::info!(
                author_id = %author_id,
                provider = %payload.provider,
                provider_id = %payload.provider_id,
                "Applied metadata to author"
            );

            // Auto-trigger author enrichment to discover books/series
            let author_name: Option<String> = authors_repo::get_name(&state.pool, author_id)
                .await
                .ok()
                .flatten();
            let display = crate::jobs::fetch_job_display(&state.pool, "author", author_id).await;
            let context = nats_jobs::JobContext::for_entity(
                "author",
                author_id,
                format!(
                    "Enriching author: {}",
                    author_name.as_deref().unwrap_or("Unknown")
                ),
            )
            .with_display_opt(display);
            if let Err(e) = crate::jobs::enqueue(
                &state.pool,
                state.nats.publisher(),
                "author_enrich",
                serde_json::json!({ "author_id": author_id }),
                context,
            )
            .await
            {
                tracing::warn!(error = %e, author_id = %author_id, "Failed to enqueue author_enrich job");
            }

            StatusCode::NO_CONTENT.into_response()
        }
        Ok(false) => (StatusCode::NOT_FOUND, "author not found").into_response(),
        Err(e) => {
            tracing::error!(error = %e, author_id = %author_id, "Failed to apply author metadata");
            (StatusCode::INTERNAL_SERVER_ERROR, "database error").into_response()
        }
    }
}

// =============================================================================
// Series Monitoring Handlers
// =============================================================================

/// PUT /api/v1/library/series/{series_id}/monitoring
/// Update monitoring settings for a series (is_monitored and/or auto_want_mode)
pub async fn update_series_monitoring(
    State(state): State<AppState>,
    _auth: AuthUser,
    Path(series_id): Path<Uuid>,
    Json(payload): Json<UpdateMonitoringRequest>,
) -> impl IntoResponse {
    // Validate auto_want_mode if provided
    // 'inherit' = use author's mode (default), explicit values override
    if let Some(ref mode) = payload.auto_want_mode {
        let valid_modes = ["inherit", "none", "new_only", "all", "fill_gaps"];
        if !valid_modes.contains(&mode.as_str()) {
            return (
                StatusCode::BAD_REQUEST,
                format!("invalid auto_want_mode: must be one of {:?}", valid_modes),
            )
                .into_response();
        }
    }

    if payload.is_monitored.is_none() && payload.auto_want_mode.is_none() {
        return StatusCode::NO_CONTENT.into_response();
    }

    match series_repo::update_monitoring(
        &state.pool,
        series_id,
        payload.is_monitored,
        payload.auto_want_mode.as_deref(),
    )
    .await
    {
        Ok(true) => {
            // When auto_want_mode changes to "none", clear want_status on discovered books in this series
            if payload.auto_want_mode.as_deref() == Some("none")
                && let Err(e) =
                    books_repo::clear_auto_wanted_for_series(&state.pool, series_id).await
            {
                tracing::warn!(error = %e, series_id = %series_id, "failed to clear auto-wanted books");
            }

            // When monitoring is turned on, trigger an immediate enrichment
            if payload.is_monitored == Some(true) {
                let _ = state
                    .nats
                    .publisher()
                    .enqueue_job(
                        "kobbler",
                        "series_enrich",
                        serde_json::json!({ "series_id": series_id }),
                    )
                    .await;
                tracing::info!(series_id = %series_id, "Series monitoring enabled, enrich job queued");
            }

            StatusCode::NO_CONTENT.into_response()
        }
        Ok(false) => (StatusCode::NOT_FOUND, "series not found").into_response(),
        Err(e) => {
            tracing::error!(error = %e, series_id = %series_id, "failed to update series monitoring");
            (StatusCode::INTERNAL_SERVER_ERROR, "database error").into_response()
        }
    }
}

/// POST /api/v1/library/series/{series_id}/refresh
/// Trigger metadata refresh for a series
pub async fn refresh_series_metadata(
    State(state): State<AppState>,
    _auth: AuthUser,
    Path(series_id): Path<Uuid>,
) -> impl IntoResponse {
    // Verify series exists
    let series_name = match series_repo::get_name(&state.pool, series_id).await {
        Ok(Some(name)) => name,
        Ok(None) => return (StatusCode::NOT_FOUND, "series not found").into_response(),
        Err(e) => {
            tracing::error!(error = %e, "failed to fetch series");
            return (StatusCode::INTERNAL_SERVER_ERROR, "database error").into_response();
        }
    };

    // Enqueue series enrichment job
    let _ = state
        .nats
        .publisher()
        .enqueue_job(
            "kobbler",
            "series_enrich",
            serde_json::json!({ "series_id": series_id }),
        )
        .await;

    AxumJson(serde_json::json!({
        "message": format!("Metadata refresh queued for series: {}", series_name)
    }))
    .into_response()
}

/// POST /api/v1/library/series/{series_id}/apply-metadata
/// Apply metadata from a search result to a series (sets provider ID, etc.)
pub async fn apply_series_metadata(
    State(state): State<AppState>,
    _auth: AuthUser,
    Path(series_id): Path<Uuid>,
    Json(payload): Json<ApplySeriesMetadataRequest>,
) -> impl IntoResponse {
    if payload.provider.is_empty() || payload.provider_id.is_empty() {
        return (
            StatusCode::BAD_REQUEST,
            "provider and provider_id are required",
        )
            .into_response();
    }

    // Get series name for job context
    let series_name: Option<String> = series_repo::get_name(&state.pool, series_id)
        .await
        .ok()
        .flatten();

    let mut identifiers = serde_json::json!({ &payload.provider: payload.provider_id });
    let slug_key = format!("{}_slug", payload.provider);
    if let Some(ref slug) = payload.provider_slug {
        identifiers[&slug_key] = serde_json::json!(slug);
    }

    // Convert image_url to ImageMap
    let images = payload.image_url.as_ref().map(|url| {
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

    match series_repo::apply_metadata(
        &state.pool,
        series_id,
        identifiers,
        images,
        payload.description.as_deref(),
        payload.total_book_count,
    )
    .await
    {
        Ok(true) => {
            tracing::info!(
                series_id = %series_id,
                provider = %payload.provider,
                provider_id = %payload.provider_id,
                "Applied metadata to series"
            );

            // Auto-trigger series enrichment to fetch all books
            let display = crate::jobs::fetch_job_display(&state.pool, "series", series_id).await;
            let context = nats_jobs::JobContext::for_entity(
                "series",
                series_id,
                format!(
                    "Enriching series: {}",
                    series_name.as_deref().unwrap_or("Unknown")
                ),
            )
            .with_display_opt(display);
            if let Err(e) = crate::jobs::enqueue(
                &state.pool,
                state.nats.publisher(),
                "series_enrich",
                serde_json::json!({ "series_id": series_id }),
                context,
            )
            .await
            {
                tracing::warn!(error = %e, series_id = %series_id, "Failed to enqueue series_enrich job");
            }

            StatusCode::NO_CONTENT.into_response()
        }
        Ok(false) => (StatusCode::NOT_FOUND, "series not found").into_response(),
        Err(e) => {
            tracing::error!(error = %e, series_id = %series_id, "Failed to apply series metadata");
            (StatusCode::INTERNAL_SERVER_ERROR, "database error").into_response()
        }
    }
}

/// GET /api/v1/library/provider-link-issues
/// Links reconcile could not settle on its own: Hardcover deleted something we hold
/// user data for (now unlinked), or merged two things we both hold user data for.
pub async fn list_provider_link_issues(
    State(state): State<AppState>,
    _auth: AuthUser,
) -> impl IntoResponse {
    match provider_reconcile::list_issues(&state.pool).await {
        Ok(rows) => AxumJson(rows).into_response(),
        Err(e) => {
            tracing::error!(error = %e, "failed to list provider link issues");
            (StatusCode::INTERNAL_SERVER_ERROR, "database error").into_response()
        }
    }
}
