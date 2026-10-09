//! User management route handlers.

use axum::{
    Json as AxumJson,
    extract::{Json, Path, State},
    http::StatusCode,
    response::IntoResponse,
};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::{AppState, auth, repositories, routes::AuthUser};

// ============================================================================
// Types
// ============================================================================

#[derive(Deserialize)]
pub struct CreateUserRequest {
    pub email: String,
    pub password: String,
    pub role: Option<String>,
    pub display_name: Option<String>,
}

#[derive(Serialize)]
pub struct CreateUserResponse {
    pub user_id: Uuid,
}

#[derive(Deserialize)]
pub struct MintDeviceTokenRequest {
    pub device_name: Option<String>,
}

#[derive(Serialize)]
pub struct MintDeviceTokenResponse {
    pub user_id: Uuid,
    pub token: String,
    #[serde(with = "time::serde::rfc3339")]
    pub expires_at: time::OffsetDateTime,
}

#[derive(Serialize)]
pub struct UserSummary {
    pub id: Uuid,
    pub email: String,
    pub role: String,
    pub display_name: Option<String>,
    pub timezone: Option<String>,
}

#[derive(Deserialize)]
pub struct UpdateUserRequest {
    pub email: Option<String>,
    pub password: Option<String>,
    pub role: Option<String>,
    pub display_name: Option<String>,
    pub timezone: Option<String>,
}

#[derive(Serialize)]
pub struct UpdateUserResponse {
    pub user_id: Uuid,
    pub email: String,
    pub role: String,
}

/// Response for a single sync provider's user config.
#[derive(Serialize)]
pub struct SyncProviderSettingsResponse {
    pub provider_type: String,
    pub enabled: bool,
    pub has_api_key: bool,
}

/// Request to update a sync provider's settings.
#[derive(Deserialize)]
pub struct UpdateSyncProviderSettingsRequest {
    pub enabled: Option<bool>,
    /// If provided, sets the API key. If empty string, clears the API key.
    pub api_key: Option<String>,
}

// ============================================================================
// Handlers
// ============================================================================

pub async fn list_users(State(state): State<AppState>, auth: AuthUser) -> impl IntoResponse {
    // The Devices page resolves owner names from this list. Viewers only ever see
    // their own devices, so they only need their own record.
    let self_only = auth.require_contributor().is_err();
    match repositories::users::list_all(&state.pool).await {
        Ok(users) => {
            let users = users
                .into_iter()
                .filter(|u| !self_only || u.id == auth.user_id)
                .map(|u| UserSummary {
                    id: u.id,
                    email: u.email,
                    role: u.role,
                    display_name: u.display_name,
                    timezone: u.timezone,
                })
                .collect::<Vec<_>>();
            AxumJson(users).into_response()
        }
        Err(e) => {
            tracing::error!(error = %e, "failed to list users");
            (StatusCode::INTERNAL_SERVER_ERROR, "database error").into_response()
        }
    }
}

pub async fn create_user(
    State(state): State<AppState>,
    auth: AuthUser,
    Json(payload): Json<CreateUserRequest>,
) -> impl IntoResponse {
    if let Err(e) = auth.require_admin() {
        return e.into_response();
    }
    let role = payload.role.as_deref().unwrap_or("viewer");
    if !["admin", "contributor", "viewer"].contains(&role) {
        return (StatusCode::BAD_REQUEST, "invalid role").into_response();
    }

    if payload.password.trim().len() < 8 {
        return (
            StatusCode::BAD_REQUEST,
            "password must be at least 8 characters",
        )
            .into_response();
    }

    let password_hash = match auth::hash_password(&payload.password) {
        Ok(hash) => hash,
        Err(_) => return (StatusCode::INTERNAL_SERVER_ERROR, "hash failed").into_response(),
    };

    let user_id = match repositories::users::create(
        &state.pool,
        &payload.email,
        &password_hash,
        role,
    )
    .await
    {
        Ok(id) => id,
        Err(_) => return (StatusCode::BAD_REQUEST, "failed to create user").into_response(),
    };

    if let Some(name) = payload
        .display_name
        .as_deref()
        .map(str::trim)
        .filter(|n| !n.is_empty())
        && repositories::users::update_display_name(&state.pool, user_id, Some(name))
            .await
            .is_err()
    {
        tracing::error!(user_id = %user_id, "created user but failed to set display name");
    }

    AxumJson(CreateUserResponse { user_id }).into_response()
}

/// Mint a client token for another user so one admin pairing can provision a whole
/// household's profiles onto a device without each of them typing a password.
pub async fn mint_device_token(
    State(state): State<AppState>,
    auth: AuthUser,
    Path(user_id): Path<Uuid>,
    body: Option<Json<MintDeviceTokenRequest>>,
) -> impl IntoResponse {
    if let Err(e) = auth.require_admin() {
        return e.into_response();
    }
    match repositories::users::exists(&state.pool, user_id).await {
        Ok(true) => {}
        Ok(false) => return (StatusCode::NOT_FOUND, "user not found").into_response(),
        Err(e) => {
            tracing::error!(error = %e, user_id = %user_id, "failed to check user exists");
            return (StatusCode::INTERNAL_SERVER_ERROR, "database error").into_response();
        }
    }

    let device_name = body
        .and_then(|Json(b)| b.device_name)
        .map(|v| v.trim().chars().take(64).collect::<String>())
        .filter(|v| !v.is_empty());

    let token = super::auth::generate_token(state.config.token_length);
    if let Err(e) = repositories::auth_tokens::create_with_expiry_days(
        &state.pool,
        user_id,
        &token,
        "admin",
        crate::routes::admin::sessions::SESSION_TTL_DAYS,
        device_name.as_deref(),
        Some(auth.user_id),
    )
    .await
    {
        tracing::error!(error = %e, user_id = %user_id, "failed to mint device token");
        return (StatusCode::INTERNAL_SERVER_ERROR, "failed to create token").into_response();
    }

    tracing::info!(admin = %auth.user_id, user_id = %user_id, "minted device token for user");
    AxumJson(MintDeviceTokenResponse {
        user_id,
        token,
        expires_at: time::OffsetDateTime::now_utc()
            + time::Duration::days(crate::routes::admin::sessions::SESSION_TTL_DAYS as i64),
    })
    .into_response()
}

pub async fn update_user(
    State(state): State<AppState>,
    auth: AuthUser,
    Path(user_id): Path<Uuid>,
    Json(payload): Json<UpdateUserRequest>,
) -> impl IntoResponse {
    if let Err(e) = auth.require_admin() {
        return e.into_response();
    }
    // Check user exists
    match repositories::users::exists(&state.pool, user_id).await {
        Ok(true) => {}
        Ok(false) => return (StatusCode::NOT_FOUND, "user not found").into_response(),
        Err(e) => {
            tracing::error!(error = %e, user_id = %user_id, "failed to check user exists");
            return (StatusCode::INTERNAL_SERVER_ERROR, "database error").into_response();
        }
    }

    // Update email if provided
    if let Some(ref email) = payload.email {
        // Check for duplicate email
        match repositories::users::email_exists_for_other(&state.pool, email, user_id).await {
            Ok(false) => {}
            Ok(true) => return (StatusCode::CONFLICT, "email already in use").into_response(),
            Err(e) => {
                tracing::error!(error = %e, email = email, "failed to check duplicate email");
                return (StatusCode::INTERNAL_SERVER_ERROR, "database error").into_response();
            }
        }

        if repositories::users::update_email(&state.pool, user_id, email)
            .await
            .is_err()
        {
            return (StatusCode::INTERNAL_SERVER_ERROR, "failed to update email").into_response();
        }
    }

    // Update display name if provided; an empty string clears it.
    if let Some(ref name) = payload.display_name {
        let trimmed = name.trim();
        let value = if trimmed.is_empty() {
            None
        } else {
            Some(trimmed)
        };
        if repositories::users::update_display_name(&state.pool, user_id, value)
            .await
            .is_err()
        {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                "failed to update display name",
            )
                .into_response();
        }
    }

    // Update timezone if provided; an empty string clears it. An unparseable
    // zone is refused rather than stored — see `Zone::parse`.
    if let Some(ref raw) = payload.timezone {
        let trimmed = raw.trim();
        let value = if trimmed.is_empty() {
            None
        } else {
            Some(trimmed)
        };
        if let Some(name) = value
            && let Err(e) = crate::notifications::prefs::Zone::parse(name)
        {
            return (StatusCode::BAD_REQUEST, e.to_string()).into_response();
        }
        if repositories::users::update_timezone(&state.pool, user_id, value)
            .await
            .is_err()
        {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                "failed to update timezone",
            )
                .into_response();
        }
    }

    // Update password if provided
    if let Some(ref password) = payload.password {
        if password.trim().len() < 8 {
            return (
                StatusCode::BAD_REQUEST,
                "password must be at least 8 characters",
            )
                .into_response();
        }

        let password_hash = match auth::hash_password(password) {
            Ok(hash) => hash,
            Err(_) => {
                return (StatusCode::INTERNAL_SERVER_ERROR, "failed to hash password")
                    .into_response();
            }
        };

        if repositories::users::update_password(&state.pool, user_id, &password_hash)
            .await
            .is_err()
        {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                "failed to update password",
            )
                .into_response();
        }
    }

    // Update role if provided
    if let Some(ref role) = payload.role {
        if !["admin", "contributor", "viewer"].contains(&role.as_str()) {
            return (StatusCode::BAD_REQUEST, "invalid role").into_response();
        }

        // Prevent removing the last admin
        if role != "admin" {
            let current = match repositories::users::get_by_id(&state.pool, user_id).await {
                Ok(Some((_, _, current_role))) => current_role,
                _ => {
                    return (StatusCode::INTERNAL_SERVER_ERROR, "failed to fetch user")
                        .into_response();
                }
            };
            if current == "admin" {
                let admin_count = match repositories::users::count_admins(&state.pool).await {
                    Ok(c) => c,
                    Err(_) => {
                        return (StatusCode::INTERNAL_SERVER_ERROR, "database error")
                            .into_response();
                    }
                };
                if admin_count <= 1 {
                    return (StatusCode::CONFLICT, "cannot remove the last admin").into_response();
                }
            }
        }

        if repositories::users::update_role(&state.pool, user_id, role)
            .await
            .is_err()
        {
            return (StatusCode::INTERNAL_SERVER_ERROR, "failed to update role").into_response();
        }
    }

    // Return updated user
    match repositories::users::get_by_id(&state.pool, user_id).await {
        Ok(Some((id, email, role))) => AxumJson(UpdateUserResponse {
            user_id: id,
            email,
            role,
        })
        .into_response(),
        _ => (StatusCode::INTERNAL_SERVER_ERROR, "failed to fetch user").into_response(),
    }
}

pub async fn delete_user(
    State(state): State<AppState>,
    auth: AuthUser,
    Path(user_id): Path<Uuid>,
) -> impl IntoResponse {
    if let Err(e) = auth.require_admin() {
        return e.into_response();
    }

    // Check user exists and get their role
    let user_role = match repositories::users::get_by_id(&state.pool, user_id).await {
        Ok(Some((_, _, role))) => role,
        Ok(None) => return (StatusCode::NOT_FOUND, "user not found").into_response(),
        Err(e) => {
            tracing::error!(error = %e, user_id = %user_id, "failed to fetch user for delete");
            return (StatusCode::INTERNAL_SERVER_ERROR, "database error").into_response();
        }
    };

    // Prevent deleting the last admin
    if user_role == "admin" {
        let admin_count = match repositories::users::count_admins(&state.pool).await {
            Ok(c) => c,
            Err(e) => {
                tracing::error!(error = %e, "failed to count admins");
                return (StatusCode::INTERNAL_SERVER_ERROR, "database error").into_response();
            }
        };
        if admin_count <= 1 {
            return (StatusCode::CONFLICT, "cannot delete the last admin").into_response();
        }
    }

    match repositories::users::delete(&state.pool, user_id).await {
        Ok(true) => StatusCode::NO_CONTENT.into_response(),
        Ok(false) => (StatusCode::NOT_FOUND, "user not found").into_response(),
        Err(e) => {
            tracing::error!(error = %e, user_id = %user_id, "failed to delete user");
            (StatusCode::INTERNAL_SERVER_ERROR, "failed to delete user").into_response()
        }
    }
}

/// Get current user's sync provider settings for all providers.
pub async fn get_sync_providers(
    State(state): State<AppState>,
    auth: AuthUser,
) -> impl IntoResponse {
    let providers = repositories::user_sync_providers::list_for_user(&state.pool, auth.user_id)
        .await
        .unwrap_or_default();

    // Only show reading providers (not video providers like Trakt which share the table)
    let registry = crate::sync_providers::SyncProviderRegistry::new();
    let available = registry.available_providers();

    let reading_ids: std::collections::HashSet<&str> =
        available.iter().map(|info| info.id).collect();

    let configured_types: std::collections::HashSet<&str> =
        providers.iter().map(|p| p.provider_type.as_str()).collect();

    let mut results: Vec<SyncProviderSettingsResponse> = providers
        .iter()
        .filter(|p| reading_ids.contains(p.provider_type.as_str()))
        .map(|p| {
            let has_api_key = p
                .config
                .get("api_key")
                .and_then(|v| v.as_str())
                .map(|s| !s.is_empty())
                .unwrap_or(false);
            SyncProviderSettingsResponse {
                provider_type: p.provider_type.clone(),
                enabled: p.enabled,
                has_api_key,
            }
        })
        .collect();

    // Add unconfigured providers as disabled with no key
    for info in available {
        if !configured_types.contains(info.id) {
            results.push(SyncProviderSettingsResponse {
                provider_type: info.id.to_string(),
                enabled: false,
                has_api_key: false,
            });
        }
    }

    AxumJson(results).into_response()
}

/// Update a sync provider's settings for the current user.
pub async fn update_sync_provider(
    State(state): State<AppState>,
    auth: AuthUser,
    Path(provider_type): Path<String>,
    Json(payload): Json<UpdateSyncProviderSettingsRequest>,
) -> impl IntoResponse {
    // Get existing config or create default
    let existing =
        repositories::user_sync_providers::get_by_type(&state.pool, auth.user_id, &provider_type)
            .await
            .ok()
            .flatten();

    let current_enabled = existing.as_ref().map(|e| e.enabled).unwrap_or(true);
    let mut current_config = existing
        .as_ref()
        .map(|e| e.config.clone())
        .unwrap_or_else(|| serde_json::json!({}));

    let enabled = payload.enabled.unwrap_or(current_enabled);

    // Update API key if provided
    if let Some(ref api_key) = payload.api_key {
        let key = api_key.trim();
        let normalized_key = if key.to_lowercase().starts_with("bearer ") {
            key[7..].trim()
        } else {
            key
        };

        if normalized_key.is_empty() {
            // Clear the API key
            if let Some(obj) = current_config.as_object_mut() {
                obj.remove("api_key");
            }
        } else if let Some(obj) = current_config.as_object_mut() {
            obj.insert(
                "api_key".to_string(),
                serde_json::Value::String(normalized_key.to_string()),
            );
        }
    }

    if let Err(e) = repositories::user_sync_providers::upsert(
        &state.pool,
        auth.user_id,
        &provider_type,
        enabled,
        &current_config,
    )
    .await
    {
        tracing::error!(error = %e, "failed to update sync provider settings");
        return (
            StatusCode::INTERNAL_SERVER_ERROR,
            "failed to update settings",
        )
            .into_response();
    }

    StatusCode::NO_CONTENT.into_response()
}
