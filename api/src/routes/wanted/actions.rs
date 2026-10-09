//! Action routes for wanted items — status, search-now, grab-held, reject.
//!
//! These routes proxy to Kobload via NATS or HTTP based on the
//! `kobload_communication_mode` runtime config setting.

use axum::{
    Json,
    extract::{Path, State},
    http::StatusCode,
    response::IntoResponse,
};
use serde::Serialize;
use uuid::Uuid;

use nats_jobs::{GrabHeldMatch, SearchPatternNow};

use crate::AppState;
use crate::kobload::KobloadError;
use crate::routes::AuthUser;
use crate::routes::helpers::build_kobload_client;

/// GET /api/v1/admin/wanted/{id}/status
/// Always uses HTTP to get real-time status from Kobload.
pub async fn get_wanted_status(
    State(state): State<AppState>,
    auth: AuthUser,
    Path(id): Path<Uuid>,
) -> Result<impl IntoResponse, (StatusCode, String)> {
    auth.require_contributor()
        .map_err(|_| (StatusCode::FORBIDDEN, "Forbidden".to_string()))?;
    let runtime = state.runtime_config.get().await;
    let client = build_kobload_client(&runtime)?;
    let source_id = id.to_string();

    match client.get_pattern_detail("kobbler", &source_id).await {
        Ok(detail) => Ok(Json(WantedStatusResponse {
            pattern: WantedStatusPattern {
                id: detail.pattern.id.to_string(),
                status: detail.pattern.status,
                source_id: detail.pattern.source_id,
                enabled: detail.pattern.enabled,
            },
            matches: detail
                .matches
                .into_iter()
                .map(|m| WantedMatchResponse {
                    id: m.id.to_string(),
                    release_title: m.release_title,
                    match_score: m.match_score,
                    quality_score: m.quality_score,
                    action: m.action,
                    held: m.held,
                    hold_until: m.hold_until.map(|t| {
                        t.format(&time::format_description::well_known::Rfc3339)
                            .unwrap_or_default()
                    }),
                    download_id: m.download_id.map(|d| d.to_string()),
                    rejected: m.rejected,
                    created_at: m
                        .created_at
                        .format(&time::format_description::well_known::Rfc3339)
                        .unwrap_or_default(),
                })
                .collect(),
        })
        .into_response()),
        Err(KobloadError::Status(StatusCode::NOT_FOUND, _)) => Ok(Json(serde_json::json!({
            "pattern": null,
            "matches": [],
            "not_tracked": true
        }))
        .into_response()),
        Err(e) => Err((StatusCode::BAD_GATEWAY, format!("Kobload error: {e}"))),
    }
}

/// POST /api/v1/admin/wanted/{id}/search-now
/// Config-driven: NATS or HTTP.
pub async fn search_wanted_now(
    State(state): State<AppState>,
    auth: AuthUser,
    Path(id): Path<Uuid>,
) -> Result<impl IntoResponse, (StatusCode, String)> {
    auth.require_contributor()
        .map_err(|_| (StatusCode::FORBIDDEN, "Forbidden".to_string()))?;
    let runtime = state.runtime_config.get().await;
    let source_id = id.to_string();
    let mode = runtime
        .kobload_communication_mode
        .as_deref()
        .unwrap_or("nats");

    match mode {
        "nats" => {
            let payload = serde_json::to_value(SearchPatternNow {
                source_system: "kobbler".to_string(),
                source_id,
            })
            .map_err(|e| {
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    format!("Failed to serialize payload: {e}"),
                )
            })?;
            state
                .nats
                .publisher()
                .enqueue_job("kobload", "search_pattern_now", payload)
                .await
                .map_err(|e| {
                    (
                        StatusCode::INTERNAL_SERVER_ERROR,
                        format!("Failed to enqueue job: {e}"),
                    )
                })?;
            Ok(StatusCode::ACCEPTED.into_response())
        }
        _ => {
            let client = build_kobload_client(&runtime)?;
            match client.search_pattern_now("kobbler", &source_id).await {
                Ok(resp) => Ok(Json(serde_json::json!({ "found": resp.found })).into_response()),
                Err(e) => Err((StatusCode::BAD_GATEWAY, format!("Kobload error: {e}"))),
            }
        }
    }
}

/// POST /api/v1/admin/wanted/{id}/grab-held
/// Config-driven: NATS or HTTP.
pub async fn grab_wanted_held(
    State(state): State<AppState>,
    auth: AuthUser,
    Path(id): Path<Uuid>,
) -> Result<impl IntoResponse, (StatusCode, String)> {
    auth.require_contributor()
        .map_err(|_| (StatusCode::FORBIDDEN, "Forbidden".to_string()))?;
    let runtime = state.runtime_config.get().await;
    let source_id = id.to_string();
    let mode = runtime
        .kobload_communication_mode
        .as_deref()
        .unwrap_or("nats");

    match mode {
        "nats" => {
            let payload = serde_json::to_value(GrabHeldMatch {
                source_system: "kobbler".to_string(),
                source_id,
            })
            .map_err(|e| {
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    format!("Failed to serialize payload: {e}"),
                )
            })?;
            state
                .nats
                .publisher()
                .enqueue_job("kobload", "grab_held_match", payload)
                .await
                .map_err(|e| {
                    (
                        StatusCode::INTERNAL_SERVER_ERROR,
                        format!("Failed to enqueue job: {e}"),
                    )
                })?;
            Ok(StatusCode::ACCEPTED.into_response())
        }
        _ => {
            let client = build_kobload_client(&runtime)?;
            match client.grab_held_match("kobbler", &source_id).await {
                Ok(resp) => Ok(
                    Json(serde_json::json!({ "download_id": resp.download_id })).into_response()
                ),
                Err(e) => Err((StatusCode::BAD_GATEWAY, format!("Kobload error: {e}"))),
            }
        }
    }
}

/// POST /api/v1/admin/wanted/{id}/reject-download
/// Always HTTP — calls KobloadClient.reject_active_download().
pub async fn reject_wanted_download(
    State(state): State<AppState>,
    auth: AuthUser,
    Path(id): Path<Uuid>,
    body: Option<Json<RejectRequest>>,
) -> Result<impl IntoResponse, (StatusCode, String)> {
    auth.require_contributor()
        .map_err(|_| (StatusCode::FORBIDDEN, "Forbidden".to_string()))?;
    let runtime = state.runtime_config.get().await;
    let client = build_kobload_client(&runtime)?;
    let source_id = id.to_string();
    let reason = body.and_then(|b| b.reason.clone());

    match client
        .reject_active_download(&source_id, reason.as_deref())
        .await
    {
        Ok(Some(match_id)) => {
            Ok(Json(serde_json::json!({ "rejected_match_id": match_id })).into_response())
        }
        Ok(None) => Err((
            StatusCode::NOT_FOUND,
            "No active download to reject".to_string(),
        )),
        Err(e) => Err((StatusCode::BAD_GATEWAY, format!("Kobload error: {e}"))),
    }
}

// ============================================================================
// Response types
// ============================================================================

#[derive(Debug, Serialize)]
pub struct WantedStatusResponse {
    pub pattern: WantedStatusPattern,
    pub matches: Vec<WantedMatchResponse>,
}

#[derive(Debug, Serialize)]
pub struct WantedStatusPattern {
    pub id: String,
    pub status: String,
    pub source_id: String,
    pub enabled: bool,
}

#[derive(Debug, Serialize)]
pub struct WantedMatchResponse {
    pub id: String,
    pub release_title: String,
    pub match_score: f64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub quality_score: Option<f64>,
    pub action: String,
    pub held: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hold_until: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub download_id: Option<String>,
    pub rejected: bool,
    pub created_at: String,
}

#[derive(Debug, serde::Deserialize)]
pub struct RejectRequest {
    pub reason: Option<String>,
}
