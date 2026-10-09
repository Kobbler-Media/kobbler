//! Device management route handlers.

use axum::{
    extract::{Json, Path, State},
    http::StatusCode,
    response::IntoResponse,
};
use rand::distr::{Alphanumeric, SampleString};
use serde::{Deserialize, Serialize};
use sqlx::PgPool;
use time::OffsetDateTime;
use uuid::Uuid;

use crate::AppState;
use crate::repositories::{self, devices, devices::DeviceInput, entitlements};
use crate::routes::AuthUser;
use crate::types::DeviceType;
use crate::types::EntitlementScope;

// ========== Types ==========

#[derive(Deserialize)]
pub struct CreateDeviceRequest {
    pub user_id: Uuid,
    pub name: Option<String>,
    pub device_type: Option<String>,
}

#[derive(Serialize)]
pub struct CreateDeviceResponse {
    pub device_id: Uuid,
    pub device_token: String,
    pub sync_url: String,
}

#[derive(Serialize)]
pub struct DeviceSummary {
    pub id: Uuid,
    pub name: Option<String>,
    pub user_id: Uuid,
    pub token: String,
    pub sync_url: String,
    pub kobo_api_url: Option<String>,
    #[serde(with = "time::serde::rfc3339::option")]
    pub last_sync_at: Option<OffsetDateTime>,
    pub device_type: DeviceType,
    pub auto_sync_new_books: bool,
}

#[derive(Serialize)]
pub struct RotateDeviceTokenResponse {
    pub device_id: Uuid,
    pub device_token: String,
    pub sync_url: String,
}

#[derive(Deserialize)]
pub struct UpdateDeviceRequest {
    pub name: Option<String>,
    pub kobo_api_url: Option<String>,
    pub auto_sync_new_books: Option<bool>,
}

/// Request to update a device's sync provider toggle.
#[derive(Deserialize)]
pub struct UpdateDeviceSyncProviderRequest {
    pub enabled: bool,
}

#[derive(Serialize)]
pub struct DeviceBookSummary {
    pub id: Uuid,
    pub title: String,
}

#[derive(Deserialize)]
pub struct AddDeviceBooksRequest {
    pub book_ids: Vec<Uuid>,
}

// ========== Helpers ==========

fn generate_token(length: usize) -> String {
    Alphanumeric.sample_string(&mut rand::rng(), length)
}

/// Verify the authenticated user owns the device. Admins bypass this check.
async fn verify_device_ownership(
    pool: &PgPool,
    device_id: Uuid,
    auth: &AuthUser,
) -> Result<(), (StatusCode, &'static str)> {
    if auth.role == "admin" {
        return Ok(());
    }
    match devices::get_user_id(pool, device_id).await {
        Ok(Some(owner_id)) if owner_id == auth.user_id => Ok(()),
        Ok(Some(_)) => Err((StatusCode::FORBIDDEN, "not your device")),
        Ok(None) => Err((StatusCode::NOT_FOUND, "device not found")),
        Err(_) => Err((StatusCode::INTERNAL_SERVER_ERROR, "database error")),
    }
}

// ========== Device Handlers ==========

pub async fn list_devices(State(state): State<AppState>, auth: AuthUser) -> impl IntoResponse {
    let rows = match devices::list_with_sync_state(&state.pool).await {
        Ok(rows) => rows,
        Err(e) => {
            tracing::error!(error = %e, "failed to list devices");
            return (StatusCode::INTERNAL_SERVER_ERROR, "database error").into_response();
        }
    };

    let rows = if auth.role != "admin" {
        rows.into_iter()
            .filter(|r| r.user_id == auth.user_id)
            .collect()
    } else {
        rows
    };

    let runtime_config = state.runtime_config.get().await;
    let proxy_base = state
        .config
        .public_proxy_url(&runtime_config.public_proxy_url);
    let device_list = rows
        .into_iter()
        .map(|row| DeviceSummary {
            id: row.id,
            name: row.name,
            user_id: row.user_id,
            sync_url: format!("{}/{}", proxy_base, row.token),
            kobo_api_url: row.kobo_api_url,
            token: row.token,
            last_sync_at: row.last_sync_at,
            device_type: row.device_type,
            auto_sync_new_books: row.auto_sync_new_books,
        })
        .collect::<Vec<_>>();

    Json(device_list).into_response()
}

pub async fn create_device(
    State(state): State<AppState>,
    auth: AuthUser,
    Json(payload): Json<CreateDeviceRequest>,
) -> impl IntoResponse {
    let device_token = generate_token(state.config.token_length);

    let device_type: DeviceType = payload
        .device_type
        .as_deref()
        .map(|s| s.parse().unwrap_or_default())
        .unwrap_or_default();

    let user_id = if auth.role == "admin" {
        payload.user_id
    } else {
        auth.user_id
    };

    let device_id = match repositories::devices::create(
        &state.pool,
        DeviceInput {
            user_id,
            token: device_token.clone(),
            name: payload.name.unwrap_or_default(),
            device_type,
        },
    )
    .await
    {
        Ok(id) => id,
        Err(_) => return (StatusCode::BAD_REQUEST, "failed to create device").into_response(),
    };

    let runtime_config = state.runtime_config.get().await;
    let sync_url = format!(
        "{}/{}",
        state
            .config
            .public_proxy_url(&runtime_config.public_proxy_url),
        device_token
    );
    Json(CreateDeviceResponse {
        device_id,
        device_token,
        sync_url,
    })
    .into_response()
}

pub async fn rotate_device_token(
    State(state): State<AppState>,
    auth: AuthUser,
    Path(device_id): Path<String>,
) -> impl IntoResponse {
    let device_id = match Uuid::parse_str(&device_id) {
        Ok(id) => id,
        Err(_) => return (StatusCode::BAD_REQUEST, "invalid device id").into_response(),
    };

    if let Err(e) = verify_device_ownership(&state.pool, device_id, &auth).await {
        return e.into_response();
    }

    let device_token = generate_token(state.config.token_length);
    if repositories::devices::update_token(&state.pool, device_id, &device_token)
        .await
        .is_err()
    {
        return (StatusCode::BAD_REQUEST, "failed to rotate token").into_response();
    }

    let runtime_config = state.runtime_config.get().await;
    let sync_url = format!(
        "{}/{}",
        state
            .config
            .public_proxy_url(&runtime_config.public_proxy_url),
        device_token
    );
    Json(RotateDeviceTokenResponse {
        device_id,
        device_token,
        sync_url,
    })
    .into_response()
}

pub async fn resync_device(
    State(state): State<AppState>,
    auth: AuthUser,
    Path(device_id): Path<String>,
) -> impl IntoResponse {
    let device_id = match Uuid::parse_str(&device_id) {
        Ok(id) => id,
        Err(_) => return (StatusCode::BAD_REQUEST, "invalid device id").into_response(),
    };

    if let Err(e) = verify_device_ownership(&state.pool, device_id, &auth).await {
        return e.into_response();
    }

    if let Err(e) = repositories::device_sync_state::delete_by_device(&state.pool, device_id).await
    {
        tracing::error!(error = %e, device_id = %device_id, "failed to delete device sync state for resync");
    }

    StatusCode::NO_CONTENT.into_response()
}

pub async fn update_device(
    State(state): State<AppState>,
    auth: AuthUser,
    Path(device_id): Path<String>,
    Json(payload): Json<UpdateDeviceRequest>,
) -> impl IntoResponse {
    let device_id = match Uuid::parse_str(&device_id) {
        Ok(id) => id,
        Err(_) => return (StatusCode::BAD_REQUEST, "invalid device id").into_response(),
    };

    if let Err(e) = verify_device_ownership(&state.pool, device_id, &auth).await {
        return e.into_response();
    }

    if repositories::devices::update(
        &state.pool,
        device_id,
        payload.name.as_deref(),
        payload.kobo_api_url.as_deref(),
        payload.auto_sync_new_books,
    )
    .await
    .is_err()
    {
        return (StatusCode::BAD_REQUEST, "failed to update device").into_response();
    }

    StatusCode::NO_CONTENT.into_response()
}

/// Update a device's sync provider enabled toggle.
pub async fn update_device_sync_provider(
    State(state): State<AppState>,
    auth: AuthUser,
    Path((device_id, provider_type)): Path<(String, String)>,
    Json(payload): Json<UpdateDeviceSyncProviderRequest>,
) -> impl IntoResponse {
    let device_id = match Uuid::parse_str(&device_id) {
        Ok(id) => id,
        Err(_) => return (StatusCode::BAD_REQUEST, "invalid device id").into_response(),
    };

    if let Err(e) = verify_device_ownership(&state.pool, device_id, &auth).await {
        return e.into_response();
    }

    if repositories::user_sync_providers::upsert_device_enabled(
        &state.pool,
        device_id,
        &provider_type,
        payload.enabled,
    )
    .await
    .is_err()
    {
        return (
            StatusCode::BAD_REQUEST,
            "failed to update sync provider setting",
        )
            .into_response();
    }

    StatusCode::NO_CONTENT.into_response()
}

pub async fn delete_device(
    State(state): State<AppState>,
    auth: AuthUser,
    Path(device_id): Path<String>,
) -> impl IntoResponse {
    let device_id = match Uuid::parse_str(&device_id) {
        Ok(id) => id,
        Err(_) => return (StatusCode::BAD_REQUEST, "invalid device id").into_response(),
    };

    if let Err(e) = verify_device_ownership(&state.pool, device_id, &auth).await {
        return e.into_response();
    }

    // Delete related records first (sync state, reading states, device entitlements)
    if let Err(e) = repositories::device_sync_state::delete_by_device(&state.pool, device_id).await
    {
        tracing::error!(error = %e, device_id = %device_id, "failed to delete device sync state");
    }

    if let Err(e) = repositories::reading_states::delete_by_device(&state.pool, device_id).await {
        tracing::error!(error = %e, device_id = %device_id, "failed to delete device reading states");
    }

    if let Err(e) = repositories::entitlements::delete_by_device_scope(&state.pool, device_id).await
    {
        tracing::error!(error = %e, device_id = %device_id, "failed to delete device entitlements");
    }

    match repositories::devices::delete(&state.pool, device_id).await {
        Ok(true) => StatusCode::NO_CONTENT.into_response(),
        Ok(false) => (StatusCode::NOT_FOUND, "device not found").into_response(),
        Err(_) => (StatusCode::INTERNAL_SERVER_ERROR, "failed to delete device").into_response(),
    }
}

// ========== Device Books (Per-Device Entitlements) ==========

/// Get books entitled to this device (DEVICE scope entitlements).
pub async fn list_device_books(
    State(state): State<AppState>,
    auth: AuthUser,
    Path(device_id): Path<String>,
) -> impl IntoResponse {
    let device_id = match Uuid::parse_str(&device_id) {
        Ok(id) => id,
        Err(_) => return (StatusCode::BAD_REQUEST, "invalid device id").into_response(),
    };

    if let Err(e) = verify_device_ownership(&state.pool, device_id, &auth).await {
        return e.into_response();
    }

    let rows = match entitlements::list_device_books(&state.pool, device_id).await {
        Ok(rows) => rows,
        Err(e) => {
            tracing::error!("Failed to list device books: {}", e);
            return (StatusCode::INTERNAL_SERVER_ERROR, "database error").into_response();
        }
    };

    let books: Vec<DeviceBookSummary> = rows
        .into_iter()
        .map(|row| DeviceBookSummary {
            id: row.id,
            title: row.title,
        })
        .collect();

    Json(books).into_response()
}

/// Add book entitlements for a device (creates DEVICE scope entitlements).
pub async fn add_device_books(
    State(state): State<AppState>,
    auth: AuthUser,
    Path(device_id): Path<String>,
    Json(payload): Json<AddDeviceBooksRequest>,
) -> impl IntoResponse {
    let device_id = match Uuid::parse_str(&device_id) {
        Ok(id) => id,
        Err(_) => return (StatusCode::BAD_REQUEST, "invalid device id").into_response(),
    };

    if let Err(e) = verify_device_ownership(&state.pool, device_id, &auth).await {
        return e.into_response();
    }

    // Verify device exists
    let device_exists = repositories::devices::exists(&state.pool, device_id)
        .await
        .unwrap_or(false);

    if !device_exists {
        return (StatusCode::NOT_FOUND, "device not found").into_response();
    }

    let mut added = 0;
    for book_id in payload.book_ids {
        // Check if entitlement already exists
        let exists = repositories::entitlements::exists_for_device(&state.pool, book_id, device_id)
            .await
            .unwrap_or(false);

        let result = if exists {
            // Update to remove is_removed flag if it was set
            repositories::entitlements::unmark_removed_for_device(&state.pool, book_id, device_id)
                .await
        } else {
            // Create new entitlement
            repositories::entitlements::create(
                &state.pool,
                book_id,
                EntitlementScope::Device,
                Some(device_id),
            )
            .await
            .map(|_| true)
        };

        match result {
            Ok(_) => added += 1,
            Err(e) => tracing::error!(
                "Failed to add book {} to device {}: {}",
                book_id,
                device_id,
                e
            ),
        }
    }

    Json(serde_json::json!({ "added": added })).into_response()
}

/// Remove a book entitlement from a device.
pub async fn remove_device_book(
    State(state): State<AppState>,
    auth: AuthUser,
    Path((device_id, book_id)): Path<(String, String)>,
) -> impl IntoResponse {
    let device_id = match Uuid::parse_str(&device_id) {
        Ok(id) => id,
        Err(_) => return (StatusCode::BAD_REQUEST, "invalid device id").into_response(),
    };

    if let Err(e) = verify_device_ownership(&state.pool, device_id, &auth).await {
        return e.into_response();
    }

    let book_id = match Uuid::parse_str(&book_id) {
        Ok(id) => id,
        Err(_) => return (StatusCode::BAD_REQUEST, "invalid book id").into_response(),
    };

    // Mark entitlement as removed (so device syncs the removal)
    match repositories::entitlements::mark_removed_for_device(&state.pool, book_id, device_id).await
    {
        Ok(true) => StatusCode::NO_CONTENT.into_response(),
        Ok(false) => (StatusCode::NOT_FOUND, "entitlement not found").into_response(),
        Err(_) => (StatusCode::INTERNAL_SERVER_ERROR, "failed to remove").into_response(),
    }
}

// ========== Book Entitlements (Global) ==========

/// Remove all entitlements for a book (marks them as removed so devices sync the removal).
pub async fn remove_book_entitlements(
    State(state): State<AppState>,
    auth: AuthUser,
    Path(book_id): Path<String>,
) -> impl IntoResponse {
    if let Err(e) = auth.require_admin() {
        return e.into_response();
    }
    let book_id = match Uuid::parse_str(&book_id) {
        Ok(id) => id,
        Err(_) => return (StatusCode::BAD_REQUEST, "invalid book id").into_response(),
    };

    if repositories::entitlements::mark_removed_by_book(&state.pool, book_id)
        .await
        .is_err()
    {
        return (
            StatusCode::INTERNAL_SERVER_ERROR,
            "failed to remove entitlements",
        )
            .into_response();
    }

    StatusCode::NO_CONTENT.into_response()
}

/// Restore all entitlements for a book (clears the is_removed flag).
pub async fn restore_book_entitlements(
    State(state): State<AppState>,
    auth: AuthUser,
    Path(book_id): Path<String>,
) -> impl IntoResponse {
    if let Err(e) = auth.require_admin() {
        return e.into_response();
    }
    let book_id = match Uuid::parse_str(&book_id) {
        Ok(id) => id,
        Err(_) => return (StatusCode::BAD_REQUEST, "invalid book id").into_response(),
    };

    if repositories::entitlements::unmark_removed_by_book(&state.pool, book_id)
        .await
        .is_err()
    {
        return (
            StatusCode::INTERNAL_SERVER_ERROR,
            "failed to restore entitlements",
        )
            .into_response();
    }

    StatusCode::NO_CONTENT.into_response()
}

// ========== Book Device Sync (Per-Book Device Status) ==========

/// Device status for a book (used in UI).
#[derive(Serialize)]
pub struct BookDeviceStatus {
    pub device_id: Uuid,
    pub device_name: Option<String>,
    pub device_type: String,
    pub entitled: bool,
    pub is_removed: bool,
}

/// Response for book device listing.
#[derive(Serialize)]
pub struct BookDevicesResponse {
    pub devices: Vec<BookDeviceStatus>,
    pub all_users_entitled: bool,
}

/// Get all devices with their entitlement status for a specific book.
/// GET /api/v1/admin/books/{book_id}/devices
pub async fn list_book_devices(
    State(state): State<AppState>,
    auth: AuthUser,
    Path(book_id): Path<String>,
) -> impl IntoResponse {
    let book_id = match Uuid::parse_str(&book_id) {
        Ok(id) => id,
        Err(_) => return (StatusCode::BAD_REQUEST, "invalid book id").into_response(),
    };

    match entitlements::list_book_devices(&state.pool, book_id).await {
        Ok((statuses, all_users)) => {
            // Non-admins only see their own devices
            let filtered: Vec<_> = if auth.role == "admin" {
                statuses
            } else {
                let user_devices: std::collections::HashSet<Uuid> =
                    match devices::list_ids_for_user(&state.pool, auth.user_id).await {
                        Ok(ids) => ids.into_iter().collect(),
                        Err(_) => std::collections::HashSet::new(),
                    };
                statuses
                    .into_iter()
                    .filter(|s| user_devices.contains(&s.device_id))
                    .collect()
            };
            let response = BookDevicesResponse {
                devices: filtered
                    .into_iter()
                    .map(|s| BookDeviceStatus {
                        device_id: s.device_id,
                        device_name: s.device_name,
                        device_type: s.device_type,
                        entitled: s.entitled,
                        is_removed: s.is_removed,
                    })
                    .collect(),
                all_users_entitled: all_users,
            };
            Json(response).into_response()
        }
        Err(e) => {
            tracing::error!(error = %e, book_id = %book_id, "failed to list book devices");
            (StatusCode::INTERNAL_SERVER_ERROR, "database error").into_response()
        }
    }
}

/// Add a book entitlement for a specific device.
/// POST /api/v1/admin/books/{book_id}/devices/{device_id}
pub async fn add_book_device(
    State(state): State<AppState>,
    auth: AuthUser,
    Path((book_id, device_id)): Path<(String, String)>,
) -> impl IntoResponse {
    let book_id = match Uuid::parse_str(&book_id) {
        Ok(id) => id,
        Err(_) => return (StatusCode::BAD_REQUEST, "invalid book id").into_response(),
    };

    let device_id = match Uuid::parse_str(&device_id) {
        Ok(id) => id,
        Err(_) => return (StatusCode::BAD_REQUEST, "invalid device id").into_response(),
    };

    if let Err(e) = verify_device_ownership(&state.pool, device_id, &auth).await {
        return e.into_response();
    }

    // Check if entitlement already exists
    let exists = entitlements::exists_for_device(&state.pool, book_id, device_id)
        .await
        .unwrap_or(false);

    let result = if exists {
        // Update to remove is_removed flag if it was set
        entitlements::unmark_removed_for_device(&state.pool, book_id, device_id).await
    } else {
        // Create new entitlement
        entitlements::create(
            &state.pool,
            book_id,
            EntitlementScope::Device,
            Some(device_id),
        )
        .await
        .map(|_| true)
    };

    match result {
        Ok(_) => {
            // Trigger kindle delivery if this is a kindle device with targets
            super::trigger_kindle_delivery_if_needed(
                &state.pool,
                state.nats.publisher(),
                book_id,
                device_id,
            )
            .await;
            StatusCode::NO_CONTENT.into_response()
        }
        Err(e) => {
            tracing::error!(
                error = %e,
                book_id = %book_id,
                device_id = %device_id,
                "failed to add book to device"
            );
            (StatusCode::INTERNAL_SERVER_ERROR, "failed to add").into_response()
        }
    }
}

/// Remove a book entitlement for a specific device.
/// If the book has an ALL_USERS entitlement, converts it to individual DEVICE
/// entitlements for all other devices first, then removes the target device.
/// DELETE /api/v1/admin/books/{book_id}/devices/{device_id}
pub async fn remove_book_device(
    State(state): State<AppState>,
    auth: AuthUser,
    Path((book_id, device_id)): Path<(String, String)>,
) -> impl IntoResponse {
    let book_id = match Uuid::parse_str(&book_id) {
        Ok(id) => id,
        Err(_) => return (StatusCode::BAD_REQUEST, "invalid book id").into_response(),
    };

    let device_id = match Uuid::parse_str(&device_id) {
        Ok(id) => id,
        Err(_) => return (StatusCode::BAD_REQUEST, "invalid device id").into_response(),
    };

    if let Err(e) = verify_device_ownership(&state.pool, device_id, &auth).await {
        return e.into_response();
    }

    // Check if the book has an ALL_USERS entitlement — if so, convert to per-device
    let has_all_users = entitlements::has_all_users(&state.pool, book_id)
        .await
        .unwrap_or(false);

    if has_all_users {
        // Create DEVICE entitlements for all devices except the one being removed
        if let Err(e) =
            entitlements::create_device_entitlements_for_all_except(&state.pool, book_id, device_id)
                .await
        {
            tracing::error!(error = %e, "failed to create device entitlements during ALL_USERS conversion");
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                "failed to convert entitlements",
            )
                .into_response();
        }

        // Remove the ALL_USERS entitlement
        if let Err(e) = entitlements::mark_removed_all_users(&state.pool, book_id).await {
            tracing::error!(error = %e, "failed to remove ALL_USERS entitlement");
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                "failed to remove ALL_USERS entitlement",
            )
                .into_response();
        }

        return StatusCode::NO_CONTENT.into_response();
    }

    // Normal case: mark the DEVICE entitlement as removed
    match entitlements::mark_removed_for_device(&state.pool, book_id, device_id).await {
        Ok(true) => StatusCode::NO_CONTENT.into_response(),
        Ok(false) => (StatusCode::NOT_FOUND, "entitlement not found").into_response(),
        Err(_) => (StatusCode::INTERNAL_SERVER_ERROR, "failed to remove").into_response(),
    }
}

/// Create an ALL_USERS entitlement for a book.
/// POST /api/v1/admin/books/{book_id}/devices/all
pub async fn add_book_all_devices(
    State(state): State<AppState>,
    auth: AuthUser,
    Path(book_id): Path<String>,
) -> impl IntoResponse {
    if let Err(e) = auth.require_admin() {
        return e.into_response();
    }

    let book_id = match Uuid::parse_str(&book_id) {
        Ok(id) => id,
        Err(_) => return (StatusCode::BAD_REQUEST, "invalid book id").into_response(),
    };

    // Use create_or_restore to handle both new and previously-removed ALL_USERS entitlements
    match entitlements::create_or_restore(&state.pool, book_id, EntitlementScope::AllUsers, None)
        .await
    {
        Ok(_) => StatusCode::NO_CONTENT.into_response(),
        Err(e) => {
            tracing::error!(error = %e, book_id = %book_id, "failed to add ALL_USERS entitlement");
            (StatusCode::INTERNAL_SERVER_ERROR, "failed to add").into_response()
        }
    }
}

// ========== Device Type Handlers ==========

// ========== Device Activity Types ==========

#[derive(Serialize)]
pub struct MiniBook {
    pub id: Uuid,
    pub title: String,
    pub cover_url: Option<String>,
}

#[derive(Serialize)]
pub struct MiniBookWithProgress {
    pub id: Uuid,
    pub title: String,
    pub cover_url: Option<String>,
    pub progress_percent: i32,
}

#[derive(Serialize)]
pub struct MiniBookWithTimestamp {
    pub id: Uuid,
    pub title: String,
    pub cover_url: Option<String>,
    #[serde(with = "time::serde::rfc3339")]
    pub timestamp: OffsetDateTime,
}

#[derive(Serialize)]
pub struct DeviceActivity {
    #[serde(with = "time::serde::rfc3339::option")]
    pub last_sync_at: Option<OffsetDateTime>,
    pub pending_sync: Vec<MiniBook>,
    pub recently_read: Vec<MiniBookWithProgress>,
    pub recently_synced: Vec<MiniBookWithTimestamp>,
    // Kindle-specific
    pub pending_delivery: Vec<MiniBook>,
    pub recently_delivered: Vec<MiniBookWithTimestamp>,
    #[serde(with = "time::serde::rfc3339::option")]
    pub last_delivery_at: Option<OffsetDateTime>,
}

#[derive(Serialize)]
pub struct ActiveDeviceTypes {
    pub kobo: bool,
    pub kindle: bool,
    pub web: bool,
    pub generic: bool,
}

/// GET /api/v1/admin/device-types — which device types have at least one device.
pub async fn get_active_device_types(
    State(state): State<AppState>,
    _auth: AuthUser,
) -> impl IntoResponse {
    let active = match devices::active_device_types(&state.pool).await {
        Ok(types) => types,
        Err(e) => {
            tracing::error!(error = %e, "failed to get active device types");
            return (StatusCode::INTERNAL_SERVER_ERROR, "database error").into_response();
        }
    };

    Json(ActiveDeviceTypes {
        kobo: active.contains(&DeviceType::Kobo),
        kindle: active.contains(&DeviceType::Kindle),
        web: active.contains(&DeviceType::Web),
        generic: active.contains(&DeviceType::Generic),
    })
    .into_response()
}

// ========== Device Activity ==========

/// GET /api/v1/admin/devices/{device_id}/activity — device activity for sidebar display.
pub async fn get_device_activity(
    State(state): State<AppState>,
    auth: AuthUser,
    Path(device_id): Path<String>,
) -> impl IntoResponse {
    let device_id = match Uuid::parse_str(&device_id) {
        Ok(id) => id,
        Err(_) => return (StatusCode::BAD_REQUEST, "invalid device id").into_response(),
    };

    if let Err(e) = verify_device_ownership(&state.pool, device_id, &auth).await {
        return e.into_response();
    }

    // Get device info to determine type and user
    let device = match devices::get_with_sync_state(&state.pool, device_id).await {
        Ok(Some(d)) => d,
        Ok(None) => return (StatusCode::NOT_FOUND, "device not found").into_response(),
        Err(e) => {
            tracing::error!(error = %e, device_id = %device_id, "failed to get device");
            return (StatusCode::INTERNAL_SERVER_ERROR, "database error").into_response();
        }
    };

    // Get pending sync books (for Kobo - books updated since last sync)
    let pending_sync = if device.device_type == DeviceType::Kobo {
        let since = device
            .last_sync_at
            .unwrap_or(time::OffsetDateTime::UNIX_EPOCH);
        match repositories::devices::get_pending_sync_books(
            &state.pool,
            device.user_id,
            device_id,
            since,
            3,
        )
        .await
        {
            Ok(books) => books
                .into_iter()
                .map(|b| MiniBook {
                    id: b.id,
                    title: b.title,
                    cover_url: b.cover_url,
                })
                .collect(),
            Err(e) => {
                tracing::warn!(error = %e, "failed to get pending sync books");
                Vec::new()
            }
        }
    } else {
        Vec::new()
    };

    // Get recently read books (books with reading progress on this device's user)
    let recently_read = match repositories::devices::get_recently_read_books(
        &state.pool,
        device.user_id,
        3,
    )
    .await
    {
        Ok(books) => books
            .into_iter()
            .map(|b| MiniBookWithProgress {
                id: b.id,
                title: b.title,
                cover_url: b.cover_url,
                progress_percent: b.progress_percent,
            })
            .collect(),
        Err(e) => {
            tracing::warn!(error = %e, "failed to get recently read books");
            Vec::new()
        }
    };

    // Get recently synced books (books synced to this device based on entitlement updates)
    let recently_synced = if device.device_type == DeviceType::Kobo {
        match repositories::devices::get_recently_synced_books(
            &state.pool,
            device.user_id,
            device_id,
            3,
        )
        .await
        {
            Ok(books) => books
                .into_iter()
                .map(|b| MiniBookWithTimestamp {
                    id: b.id,
                    title: b.title,
                    cover_url: b.cover_url,
                    timestamp: b.synced_at,
                })
                .collect(),
            Err(e) => {
                tracing::warn!(error = %e, "failed to get recently synced books");
                Vec::new()
            }
        }
    } else {
        Vec::new()
    };

    // Kindle-specific: pending delivery and recently delivered
    let (pending_delivery, recently_delivered, last_delivery_at) =
        if device.device_type == DeviceType::Kindle {
            let pending = match repositories::kindle::get_pending_delivery_books(
                &state.pool,
                device.user_id,
                device_id,
                3,
            )
            .await
            {
                Ok(books) => books
                    .into_iter()
                    .map(|b| MiniBook {
                        id: b.id,
                        title: b.title,
                        cover_url: b.cover_url,
                    })
                    .collect(),
                Err(e) => {
                    tracing::warn!(error = %e, "failed to get pending delivery books");
                    Vec::new()
                }
            };

            let (delivered, last_at) =
                match repositories::kindle::get_recently_delivered_books(&state.pool, device_id, 3)
                    .await
                {
                    Ok((books, last)) => (
                        books
                            .into_iter()
                            .map(|b| MiniBookWithTimestamp {
                                id: b.id,
                                title: b.title,
                                cover_url: b.cover_url,
                                timestamp: b.delivered_at,
                            })
                            .collect(),
                        last,
                    ),
                    Err(e) => {
                        tracing::warn!(error = %e, "failed to get recently delivered books");
                        (Vec::new(), None)
                    }
                };

            (pending, delivered, last_at)
        } else {
            (Vec::new(), Vec::new(), None)
        };

    Json(DeviceActivity {
        last_sync_at: device.last_sync_at,
        pending_sync,
        recently_read,
        recently_synced,
        pending_delivery,
        recently_delivered,
        last_delivery_at,
    })
    .into_response()
}
