//! Kindle account management routes.
//!
//! OAuth flow, target device management, and delivery triggers.

use crate::AppState;
use crate::jobs::{self, JobContext};
use crate::repositories::kindle;
use crate::routes::AuthUser;
use axum::Json;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::IntoResponse;
use serde::Deserialize;
use serde_json::json;
use stk_client::{Client as StkClient, OAuth2};
use tracing::{error, info, warn};
use uuid::Uuid;

use crate::DbPool;
use nats_jobs::Publisher;

// =============================================================================
// OAuth Flow
// =============================================================================

/// Get the Amazon OAuth sign-in URL.
pub async fn get_oauth_url(State(state): State<AppState>, auth: AuthUser) -> impl IntoResponse {
    let oauth = OAuth2::new();
    let signin_url = oauth.get_signin_url();

    // Store the verifier for later use
    if let Err(e) = kindle::set_pending_verifier(&state.pool, auth.user_id, oauth.verifier()).await
    {
        error!(error = %e, "Failed to store OAuth verifier");
        return (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({ "error": "Failed to initiate OAuth flow" })),
        )
            .into_response();
    }

    (StatusCode::OK, Json(json!({ "url": signin_url }))).into_response()
}

#[derive(Deserialize)]
pub struct OAuthCompleteRequest {
    pub redirect_url: String,
    /// The Kobbler device to link kindle targets to.
    pub device_id: Option<Uuid>,
}

/// Complete the OAuth flow with the redirect URL from Amazon.
pub async fn complete_oauth(
    State(state): State<AppState>,
    auth: AuthUser,
    Json(req): Json<OAuthCompleteRequest>,
) -> impl IntoResponse {
    let user_id = auth.user_id;

    // Retrieve the stored verifier
    let verifier = match kindle::get_pending_verifier(&state.pool, user_id).await {
        Ok(Some(v)) => v,
        Ok(None) => {
            return (
                StatusCode::BAD_REQUEST,
                Json(json!({ "error": "No pending OAuth flow found. Please start again." })),
            )
                .into_response();
        }
        Err(e) => {
            error!(error = %e, "Failed to get OAuth verifier");
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({ "error": "Failed to complete OAuth flow" })),
            )
                .into_response();
        }
    };

    // Complete the OAuth flow using the native client
    let oauth = OAuth2::with_verifier(verifier);
    let client = match oauth.create_client(&req.redirect_url).await {
        Ok(c) => c,
        Err(e) => {
            error!(error = %e, "Failed to complete OAuth flow");
            return (
                StatusCode::BAD_REQUEST,
                Json(json!({ "error": format!("OAuth failed: {}", e) })),
            )
                .into_response();
        }
    };

    // Serialize and store credentials
    let credentials = client.dumps();

    // Get owned devices
    let devices = match client.get_owned_devices().await {
        Ok(d) => d,
        Err(e) => {
            error!(error = %e, "Failed to get owned devices");
            Vec::new()
        }
    };

    // Create or update kindle account
    let existing = kindle::get_account_by_user(&state.pool, user_id).await;

    let account_id = match existing {
        Ok(Some((id, _))) => {
            // Update credentials
            let _ = kindle::update_credentials(&state.pool, id, &credentials).await;
            id
        }
        _ => match kindle::create_account(&state.pool, user_id, &credentials).await {
            Ok(id) => id,
            Err(e) => {
                error!(error = %e, "Failed to create kindle account");
                return (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(json!({ "error": "Failed to save account" })),
                )
                    .into_response();
            }
        },
    };

    // Clear the pending verifier
    let _ = kindle::clear_pending_verifier(&state.pool, user_id).await;

    // Upsert discovered devices as targets, linking to the Kobbler device if provided
    for d in &devices {
        let serial = &d.device_serial_number;
        let name = Some(d.device_name.as_str());
        if !serial.is_empty() {
            let _ =
                kindle::upsert_target(&state.pool, account_id, serial, name, req.device_id).await;
        }
    }

    info!(user_id = %user_id, account_id = %account_id, device_id = ?req.device_id, "Kindle account connected");

    (StatusCode::OK, Json(json!({ "account_id": account_id }))).into_response()
}

// =============================================================================
// Account Management
// =============================================================================

/// Get the current user's Kindle account status and targets.
pub async fn get_account(State(state): State<AppState>, auth: AuthUser) -> impl IntoResponse {
    let account = kindle::get_account_by_user(&state.pool, auth.user_id).await;

    match account {
        Ok(Some((account_id, _))) => {
            let targets = kindle::list_targets(&state.pool, account_id)
                .await
                .unwrap_or_default();

            let targets_json: Vec<serde_json::Value> = targets
                .into_iter()
                .map(|t| {
                    json!({
                        "id": t.id,
                        "device_serial": t.device_serial,
                        "device_name": t.device_name,
                        "enabled": t.enabled,
                    })
                })
                .collect();

            (
                StatusCode::OK,
                Json(json!({
                    "connected": true,
                    "account_id": account_id,
                    "targets": targets_json,
                })),
            )
                .into_response()
        }
        _ => (
            StatusCode::OK,
            Json(json!({
                "connected": false,
                "account_id": null,
                "targets": [],
            })),
        )
            .into_response(),
    }
}

/// Disconnect the Kindle account (deletes account and all targets/deliveries).
pub async fn disconnect_account(
    State(state): State<AppState>,
    auth: AuthUser,
) -> impl IntoResponse {
    let account = kindle::get_account_by_user(&state.pool, auth.user_id).await;

    match account {
        Ok(Some((account_id, _))) => {
            let _ = kindle::delete_account(&state.pool, account_id).await;
            info!(user_id = %auth.user_id, "Kindle account disconnected");
            StatusCode::NO_CONTENT.into_response()
        }
        _ => (StatusCode::NOT_FOUND, "No Kindle account found").into_response(),
    }
}

#[derive(Deserialize)]
pub struct RefreshDevicesRequest {
    /// The Kobbler device to link kindle targets to.
    pub device_id: Option<Uuid>,
}

/// Refresh the list of Kindle devices from Amazon.
pub async fn refresh_devices(
    State(state): State<AppState>,
    auth: AuthUser,
    body: Option<Json<RefreshDevicesRequest>>,
) -> impl IntoResponse {
    let device_id = body.and_then(|b| b.device_id);
    let account = kindle::get_account_by_user(&state.pool, auth.user_id).await;

    let (account_id, credentials) = match account {
        Ok(Some(a)) => a,
        _ => return (StatusCode::NOT_FOUND, "No Kindle account found").into_response(),
    };

    // Load the client from credentials
    let client = match StkClient::loads(&credentials) {
        Ok(c) => c,
        Err(e) => {
            error!(error = %e, "Failed to load Kindle credentials");
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                "Invalid stored credentials",
            )
                .into_response();
        }
    };

    // Get owned devices
    let devices = match client.get_owned_devices().await {
        Ok(d) => d,
        Err(e) => {
            error!(error = %e, "Failed to get owned devices");
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("Failed to get devices: {}", e),
            )
                .into_response();
        }
    };

    for d in &devices {
        let serial = &d.device_serial_number;
        let name = Some(d.device_name.as_str());
        if !serial.is_empty() {
            let _ = kindle::upsert_target(&state.pool, account_id, serial, name, device_id).await;
        }
    }

    // Return updated target list
    let targets = kindle::list_targets(&state.pool, account_id)
        .await
        .unwrap_or_default();

    let targets_json: Vec<serde_json::Value> = targets
        .into_iter()
        .map(|t| {
            json!({
                "id": t.id,
                "device_serial": t.device_serial,
                "device_name": t.device_name,
                "enabled": t.enabled,
            })
        })
        .collect();

    (StatusCode::OK, Json(json!({ "targets": targets_json }))).into_response()
}

// =============================================================================
// Target Management
// =============================================================================

#[derive(Deserialize)]
pub struct UpdateTargetRequest {
    pub enabled: bool,
}

/// Update a target's enabled status.
pub async fn update_target(
    State(state): State<AppState>,
    _auth: AuthUser,
    Path(target_id): Path<Uuid>,
    Json(req): Json<UpdateTargetRequest>,
) -> impl IntoResponse {
    match kindle::update_target_enabled(&state.pool, target_id, req.enabled).await {
        Ok(true) => StatusCode::NO_CONTENT.into_response(),
        Ok(false) => (StatusCode::NOT_FOUND, "Target not found").into_response(),
        Err(e) => {
            error!(error = %e, "Failed to update target");
            (StatusCode::INTERNAL_SERVER_ERROR, "Update failed").into_response()
        }
    }
}

// =============================================================================
// Delivery
// =============================================================================

#[derive(Deserialize)]
pub struct SendToKindleRequest {
    pub target_id: Uuid,
    #[serde(default)]
    pub force: bool,
}

/// Send a book to a specific Kindle target.
pub async fn send_to_kindle(
    State(state): State<AppState>,
    _auth: AuthUser,
    Path(book_id): Path<Uuid>,
    Json(req): Json<SendToKindleRequest>,
) -> impl IntoResponse {
    let display = crate::jobs::fetch_job_display(&state.pool, "book", book_id).await;
    let context = JobContext::for_entity(
        "book",
        book_id,
        format!("Sending book to Kindle target {}", req.target_id),
    )
    .with_display_opt(display);

    match jobs::enqueue(
        &state.pool,
        state.nats.publisher(),
        "kindle_delivery",
        json!({
            "book_id": book_id,
            "target_id": req.target_id,
            "force": req.force,
        }),
        context,
    )
    .await
    {
        Ok(job_id) => (StatusCode::OK, Json(json!({ "job_id": job_id }))).into_response(),
        Err(e) => {
            error!(error = %e, "Failed to enqueue kindle delivery");
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                "Failed to queue delivery",
            )
                .into_response()
        }
    }
}

/// Get delivery history for a book.
pub async fn get_book_deliveries(
    State(state): State<AppState>,
    _auth: AuthUser,
    Path(book_id): Path<Uuid>,
) -> impl IntoResponse {
    match kindle::list_deliveries_for_book(&state.pool, book_id).await {
        Ok(deliveries) => {
            let json: Vec<serde_json::Value> = deliveries
                .into_iter()
                .map(|d| {
                    json!({
                        "id": d.id,
                        "target_id": d.target_id,
                        "device_name": d.device_name,
                        "delivered_at": d.delivered_at.format(&time::format_description::well_known::Rfc3339).unwrap_or_default(),
                    })
                })
                .collect();

            (StatusCode::OK, Json(json!({ "deliveries": json }))).into_response()
        }
        Err(e) => {
            error!(error = %e, "Failed to list deliveries");
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                "Failed to list deliveries",
            )
                .into_response()
        }
    }
}

// =============================================================================
// Shared Helpers
// =============================================================================

/// Trigger kindle delivery for a specific device if it's a kindle device with targets.
/// Called after creating DEVICE-scoped entitlements.
pub async fn trigger_kindle_delivery_if_needed(
    pool: &DbPool,
    nats: &Publisher,
    book_id: Uuid,
    device_id: Uuid,
) {
    let targets = match kindle::list_enabled_targets_for_device(pool, device_id).await {
        Ok(t) if !t.is_empty() => t,
        _ => return,
    };

    let display = crate::jobs::fetch_job_display(pool, "book", book_id).await;
    for target in targets {
        let target_name = target
            .device_name
            .as_deref()
            .unwrap_or(&target.device_serial);
        let context =
            JobContext::for_entity("book", book_id, format!("Sending to Kindle: {target_name}"))
                .with_display_opt(display.clone());
        if let Err(e) = jobs::enqueue(
            pool,
            nats,
            "kindle_delivery",
            json!({
                "book_id": book_id,
                "target_id": target.id,
            }),
            context,
        )
        .await
        {
            warn!(error = %e, target_id = %target.id, "Failed to enqueue kindle delivery");
        }
    }
}

/// Trigger kindle delivery for all kindle devices (for ALL_USERS entitlements).
pub async fn trigger_kindle_delivery_all_devices(pool: &DbPool, nats: &Publisher, book_id: Uuid) {
    let targets = match kindle::list_all_enabled_targets(pool).await {
        Ok(t) => t,
        Err(e) => {
            warn!(error = %e, "Failed to list kindle targets for ALL_USERS delivery");
            return;
        }
    };

    let display = crate::jobs::fetch_job_display(pool, "book", book_id).await;
    for target in targets {
        let target_name = target
            .device_name
            .as_deref()
            .unwrap_or(&target.device_serial);
        let context =
            JobContext::for_entity("book", book_id, format!("Sending to Kindle: {target_name}"))
                .with_display_opt(display.clone());
        if let Err(e) = jobs::enqueue(
            pool,
            nats,
            "kindle_delivery",
            json!({
                "book_id": book_id,
                "target_id": target.id,
            }),
            context,
        )
        .await
        {
            warn!(error = %e, target_id = %target.id, "Failed to enqueue kindle delivery");
        }
    }
}
