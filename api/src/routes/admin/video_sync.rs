//! Video sync provider routes (Trakt, Simkl, etc.)
//!
//! Handles listing providers, updating config, and OAuth flows.
//! Provider config is stored per-user in user_sync_providers.

use axum::{
    Json,
    extract::{Path, State},
    http::StatusCode,
    response::IntoResponse,
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::AppState;
use crate::repositories::user_sync_providers as repo;
use crate::routes::AuthUser;
use crate::video_sync_providers::{VideoSyncProviderRegistry, trakt};

// ============================================================================
// Response Types
// ============================================================================

#[derive(Serialize)]
struct ProviderResponse {
    provider_type: String,
    enabled: bool,
    config: Value,
    display_name: String,
    description: String,
    website: String,
    uses_oauth: bool,
}

#[derive(Deserialize)]
pub struct UpdateProviderRequest {
    pub enabled: Option<bool>,
    pub config: Option<std::collections::HashMap<String, Value>>,
}

#[derive(Deserialize)]
pub struct AuthorizeRequest {
    pub client_id: String,
}

#[derive(Deserialize)]
pub struct PollRequest {
    pub client_id: String,
    pub client_secret: String,
}

#[derive(Deserialize)]
pub struct ImportHistoryRequest {
    pub media_type: Option<String>, // "movies", "shows", or both if omitted
}

// ============================================================================
// List Providers
// ============================================================================

/// GET /api/v1/admin/video-sync/providers
pub async fn list_providers(
    State(state): State<AppState>,
    auth: AuthUser,
) -> Result<impl IntoResponse, StatusCode> {
    let stored = repo::list_for_user(&state.pool, auth.user_id)
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    let registry = VideoSyncProviderRegistry::new();
    let available = registry.available_providers();

    // Build response: show all registered providers, merging with stored config
    let response: Vec<ProviderResponse> = available
        .iter()
        .map(|info| {
            let stored_provider = stored.iter().find(|s| s.provider_type == info.id);

            ProviderResponse {
                provider_type: info.id.to_string(),
                enabled: stored_provider.map(|s| s.enabled).unwrap_or(false),
                config: stored_provider
                    .map(|s| mask_secrets(&s.config))
                    .unwrap_or_else(|| json!({})),
                display_name: info.display_name.to_string(),
                description: info.description.to_string(),
                website: info.website.to_string(),
                uses_oauth: info.uses_oauth,
            }
        })
        .collect();

    Ok(Json(response))
}

// ============================================================================
// Update Provider
// ============================================================================

/// PATCH /api/v1/admin/video-sync/providers/:provider_type
pub async fn update_provider(
    State(state): State<AppState>,
    auth: AuthUser,
    Path(provider_type): Path<String>,
    Json(body): Json<UpdateProviderRequest>,
) -> Result<impl IntoResponse, StatusCode> {
    // Upsert: if the user doesn't have a row yet, create one
    let existing = repo::get_by_type(&state.pool, auth.user_id, &provider_type)
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    if let Some(existing) = existing {
        // Update enabled flag if provided
        if let Some(enabled) = body.enabled {
            repo::update_enabled(&state.pool, auth.user_id, &provider_type, enabled)
                .await
                .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
        }

        // Merge config if provided
        if let Some(ref config_updates) = body.config {
            repo::update_config_merge(&state.pool, auth.user_id, &provider_type, config_updates)
                .await
                .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
        }

        let _ = existing; // used above
    } else {
        // Create new row
        let config = body
            .config
            .as_ref()
            .map(|updates| {
                let mut obj = serde_json::Map::new();
                for (k, v) in updates {
                    if !v.is_null() {
                        obj.insert(k.clone(), v.clone());
                    }
                }
                Value::Object(obj)
            })
            .unwrap_or_else(|| json!({}));

        repo::upsert(
            &state.pool,
            auth.user_id,
            &provider_type,
            body.enabled.unwrap_or(false),
            &config,
        )
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    }

    Ok(StatusCode::NO_CONTENT)
}

// ============================================================================
// Trakt OAuth: Authorize (Start Device Code Flow)
// ============================================================================

/// POST /api/v1/admin/video-sync/trakt/authorize
pub async fn trakt_authorize(
    _auth: AuthUser,
    Json(body): Json<AuthorizeRequest>,
) -> Result<impl IntoResponse, StatusCode> {
    let client = crate::http::new_client();

    let result = trakt::start_device_auth(&client, None, &body.client_id)
        .await
        .map_err(|e| {
            tracing::error!("Trakt device auth failed: {}", e);
            StatusCode::BAD_GATEWAY
        })?;

    Ok(Json(json!({
        "device_code": result.device_code,
        "user_code": result.user_code,
        "verification_url": result.verification_url,
        "expires_in": result.expires_in,
        "interval": result.interval,
    })))
}

// ============================================================================
// Trakt OAuth: Poll for Token
// ============================================================================

/// POST /api/v1/admin/video-sync/trakt/poll/:device_code
pub async fn trakt_poll(
    State(state): State<AppState>,
    auth: AuthUser,
    Path(device_code): Path<String>,
    Json(body): Json<PollRequest>,
) -> Result<impl IntoResponse, StatusCode> {
    let client = crate::http::new_client();

    let result = trakt::poll_device_token(
        &client,
        None,
        &body.client_id,
        &body.client_secret,
        &device_code,
    )
    .await
    .map_err(|e| {
        tracing::error!("Trakt poll failed: {}", e);
        StatusCode::BAD_GATEWAY
    })?;

    match result {
        crate::video_sync_providers::PollResult::Authorized {
            access_token,
            refresh_token,
        } => {
            // Store tokens in the user's Trakt provider config
            let mut config = serde_json::Map::new();
            config.insert("access_token".to_string(), json!(access_token));
            if let Some(rt) = refresh_token {
                config.insert("refresh_token".to_string(), json!(rt));
            }
            config.insert("client_id".to_string(), json!(body.client_id));
            config.insert("client_secret".to_string(), json!(body.client_secret));

            repo::upsert(
                &state.pool,
                auth.user_id,
                "trakt",
                true,
                &Value::Object(config),
            )
            .await
            .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

            Ok(Json(json!({ "status": "authorized" })))
        }
        crate::video_sync_providers::PollResult::Pending => {
            Ok(Json(json!({ "status": "pending" })))
        }
        crate::video_sync_providers::PollResult::Expired => {
            Ok(Json(json!({ "status": "expired" })))
        }
        crate::video_sync_providers::PollResult::Denied => Ok(Json(json!({ "status": "denied" }))),
    }
}

// ============================================================================
// Trakt: Disconnect
// ============================================================================

/// POST /api/v1/admin/video-sync/trakt/disconnect
pub async fn trakt_disconnect(
    State(state): State<AppState>,
    auth: AuthUser,
) -> Result<impl IntoResponse, StatusCode> {
    repo::delete(&state.pool, auth.user_id, "trakt")
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    Ok(StatusCode::NO_CONTENT)
}

// ============================================================================
// Trakt: Import History
// ============================================================================

/// POST /api/v1/admin/video-sync/trakt/import-history
pub async fn trakt_import_history(
    State(state): State<AppState>,
    auth: AuthUser,
    Json(body): Json<ImportHistoryRequest>,
) -> Result<impl IntoResponse, StatusCode> {
    // Enqueue an import job with user_id
    let payload = json!({
        "provider": "trakt",
        "media_type": body.media_type.unwrap_or_else(|| "all".to_string()),
        "user_id": auth.user_id,
    });

    crate::jobs::enqueue(
        &state.pool,
        state.nats.publisher(),
        "trakt_import_history",
        payload,
        crate::jobs::JobContext::new("Importing watch history from Trakt"),
    )
    .await
    .map_err(|e| {
        tracing::error!("Failed to enqueue Trakt import job: {}", e);
        StatusCode::INTERNAL_SERVER_ERROR
    })?;

    Ok(Json(json!({ "status": "queued" })))
}

// ============================================================================
// Helpers
// ============================================================================

/// Mask secret fields in config for API responses.
fn mask_secrets(config: &Value) -> Value {
    let Some(obj) = config.as_object() else {
        return config.clone();
    };

    let mut masked = obj.clone();
    for key in ["access_token", "refresh_token", "client_secret"] {
        if let Some(val) = masked.get(key)
            && val.as_str().map(|s| !s.is_empty()).unwrap_or(false)
        {
            masked.insert(key.to_string(), json!("••••••••"));
        }
    }

    Value::Object(masked)
}
