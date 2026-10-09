//! Kobload integration handlers (connection test, watch callback).

use axum::{
    Json,
    extract::{Path, State},
    http::StatusCode,
    response::IntoResponse,
};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::AppState;
use crate::kobload::KobloadClient;
use crate::repositories::{ingest_tokens, wanted_attempts};
use crate::routes::AuthUser;
use crate::routes::ingest::KobloadFileInfo;

// =============================================================================
// Kobload Connection Test
// =============================================================================

#[derive(Debug, Serialize)]
pub struct TestConnectionResult {
    pub success: bool,
    pub message: String,
}

#[derive(Debug, Deserialize, Default)]
pub struct TestKobloadRequest {
    pub kobload_base_url: Option<String>,
    pub kobload_api_key: Option<String>,
}

/// POST /api/v1/admin/test-kobload
pub async fn test_kobload_connection(
    State(state): State<AppState>,
    auth: AuthUser,
    body: Option<Json<TestKobloadRequest>>,
) -> impl IntoResponse {
    if let Err(e) = auth.require_admin() {
        return e.into_response();
    }
    let runtime = state.runtime_config.get().await;
    let req = body.map(|b| b.0).unwrap_or_default();

    let base_url = req
        .kobload_base_url
        .filter(|s| !s.trim().is_empty())
        .or_else(|| runtime.kobload_base_url.clone());
    let api_key = req
        .kobload_api_key
        .filter(|s| !s.trim().is_empty())
        .or_else(|| runtime.kobload_api_key.clone());

    let base_url = match base_url {
        Some(url) if !url.trim().is_empty() => url,
        _ => {
            return Json(TestConnectionResult {
                success: false,
                message: "Kobload Base URL is not configured".to_string(),
            })
            .into_response();
        }
    };

    let api_key = match api_key {
        Some(key) if !key.trim().is_empty() => key,
        _ => {
            return Json(TestConnectionResult {
                success: false,
                message: "Kobload API Key is not configured".to_string(),
            })
            .into_response();
        }
    };

    let client = KobloadClient::new(base_url, api_key);
    match client.health_check().await {
        Ok(_) => Json(TestConnectionResult {
            success: true,
            message: "Connection successful".to_string(),
        })
        .into_response(),
        Err(err) => Json(TestConnectionResult {
            success: false,
            message: format!("Connection failed: {err}"),
        })
        .into_response(),
    }
}

// =============================================================================
// Quality Profiles Proxy
// =============================================================================

/// GET /api/v1/admin/kobload/quality-profiles
pub async fn list_quality_profiles(
    State(state): State<AppState>,
    auth: AuthUser,
) -> impl IntoResponse {
    if let Err(e) = auth.require_contributor() {
        return e.into_response();
    }
    let runtime = state.runtime_config.get().await;

    let base_url = match &runtime.kobload_base_url {
        Some(url) if !url.trim().is_empty() => url.clone(),
        _ => return StatusCode::SERVICE_UNAVAILABLE.into_response(),
    };
    let api_key = match &runtime.kobload_api_key {
        Some(key) if !key.trim().is_empty() => key.clone(),
        _ => return StatusCode::SERVICE_UNAVAILABLE.into_response(),
    };

    let client = KobloadClient::new(base_url, api_key);
    match client.list_quality_profiles().await {
        Ok(profiles) => Json(profiles).into_response(),
        Err(err) => {
            tracing::warn!(error = %err, "Failed to fetch quality profiles from Kobload");
            (StatusCode::BAD_GATEWAY, format!("Kobload error: {err}")).into_response()
        }
    }
}

// =============================================================================
// Watch Pattern Callback
// =============================================================================

#[derive(Debug, Deserialize)]
pub struct WatchCallbackPayload {
    pub download_id: Uuid,
    pub status: String,
    pub files: Option<Vec<KobloadFileInfo>>,
    pub metadata: Option<serde_json::Value>,
    pub message: Option<String>,
}

/// POST /api/v1/wanted/watch-callback/{token}
pub async fn watch_callback(
    State(state): State<AppState>,
    Path(token): Path<String>,
    Json(payload): Json<WatchCallbackPayload>,
) -> impl IntoResponse {
    // Validate ingest token
    let token_valid = ingest_tokens::is_valid(&state.pool, &token)
        .await
        .unwrap_or(false);

    if !token_valid {
        return (StatusCode::UNAUTHORIZED, "invalid token").into_response();
    }

    let book_id = payload
        .metadata
        .as_ref()
        .and_then(|m| m.get("book_id"))
        .and_then(|v| v.as_str())
        .and_then(|s| Uuid::parse_str(s).ok());

    let book_id = match book_id {
        Some(id) => id,
        None => {
            tracing::warn!("Watch callback missing book_id in metadata");
            return StatusCode::OK.into_response();
        }
    };

    // Update or create attempt
    let files_json = payload
        .files
        .as_ref()
        .and_then(|f| serde_json::to_value(f).ok());

    let _ = wanted_attempts::upsert_by_external_id(
        &state.pool,
        wanted_attempts::BookAttemptUpsert {
            book_id,
            source: "watch",
            external_id: &payload.download_id.to_string(),
            status: &payload.status,
            message: payload.message.as_deref(),
            files_json,
            ..Default::default()
        },
    )
    .await;

    StatusCode::OK.into_response()
}
