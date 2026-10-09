//! Authentication handlers for admin routes.

use axum::{
    Json as AxumJson,
    extract::{Json, State},
    http::{HeaderMap, HeaderValue, StatusCode},
    response::IntoResponse,
};
use rand::distr::{Alphanumeric, SampleString};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::{AppState, auth, repositories};

/// Check if the request arrived over HTTPS (via reverse proxy).
pub(super) fn is_https(headers: &HeaderMap) -> bool {
    headers
        .get("x-forwarded-proto")
        .and_then(|v| v.to_str().ok())
        .is_some_and(|s| s.eq_ignore_ascii_case("https"))
}

#[derive(Deserialize)]
pub struct LoginRequest {
    pub email: String,
    pub password: String,
}

#[derive(Serialize)]
pub struct LoginResponse {
    pub token: String,
    pub role: String,
    pub email: String,
}

#[derive(Serialize)]
pub struct MeResponse {
    pub user_id: Uuid,
    pub email: String,
    pub role: String,
    /// The name the user chose. Absent means the client should fall back to something
    /// derived from the email address.
    pub display_name: Option<String>,
    /// IANA zone name. Absent means the user has not said where they are, and
    /// wall-clock preferences fall back to the server's zone.
    pub timezone: Option<String>,
    pub avatar_url: Option<String>,
    pub has_pin: bool,
    /// Cast needs this at boot for every role, and the full system config is
    /// admin-only, so it rides along here.
    pub cast_receiver_app_id: Option<String>,
}

pub async fn login(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(payload): Json<LoginRequest>,
) -> impl IntoResponse {
    let ip = headers
        .get("x-forwarded-for")
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.split(',').next())
        .and_then(|s| s.trim().parse::<std::net::IpAddr>().ok())
        .unwrap_or(std::net::IpAddr::V4(std::net::Ipv4Addr::UNSPECIFIED));

    if !state.login_limiter.check(ip) {
        return StatusCode::TOO_MANY_REQUESTS.into_response();
    }

    // Any user can log in - no admin flag check
    let (user_id, password_hash, role) =
        match repositories::users::get_by_email_for_login(&state.pool, &payload.email).await {
            Ok(Some(data)) => data,
            _ => return (StatusCode::UNAUTHORIZED, "invalid credentials").into_response(),
        };

    if !auth::verify_password(&password_hash, &payload.password) {
        return (StatusCode::UNAUTHORIZED, "invalid credentials").into_response();
    }

    let token = generate_token(state.config.token_length);
    if let Err(e) = repositories::auth_tokens::create(&state.pool, user_id, &token, "admin").await {
        tracing::error!(error = %e, user_id = %user_id, "failed to insert auth token during login");
        return (StatusCode::INTERNAL_SERVER_ERROR, "login failed").into_response();
    }

    let secure_suffix = if is_https(&headers) { "; Secure" } else { "" };
    let session_cookie = format!(
        "kobbler_session={token}; Path=/; HttpOnly; SameSite=Lax; Max-Age=2592000{secure_suffix}",
    );
    let mut headers = HeaderMap::new();
    let cookie_value = match HeaderValue::from_str(&session_cookie) {
        Ok(v) => v,
        Err(e) => {
            tracing::error!(error = %e, "failed to create session cookie header");
            return (StatusCode::INTERNAL_SERVER_ERROR, "login failed").into_response();
        }
    };
    headers.append("Set-Cookie", cookie_value);
    (
        headers,
        AxumJson(LoginResponse {
            token,
            role,
            email: payload.email,
        }),
    )
        .into_response()
}

pub async fn me(State(state): State<AppState>, auth: crate::routes::AuthUser) -> impl IntoResponse {
    match repositories::users::get_by_id(&state.pool, auth.user_id).await {
        Ok(Some((id, email, role))) => {
            let has_avatar = repositories::users::has_avatar(&state.pool, id)
                .await
                .unwrap_or(false);
            let display_name = repositories::users::get_display_name(&state.pool, id)
                .await
                .unwrap_or(None);
            let timezone = repositories::users::get_timezone(&state.pool, id)
                .await
                .unwrap_or(None);
            let has_pin = repositories::users::has_profile_pin(&state.pool, id)
                .await
                .unwrap_or(false);
            let avatar_url = if has_avatar {
                Some(format!("/api/v1/admin/users/{}/avatar", id))
            } else {
                None
            };
            let cast_receiver_app_id = state
                .runtime_config
                .get()
                .await
                .cast_receiver_app_id
                .clone();
            AxumJson(MeResponse {
                user_id: id,
                email,
                role,
                display_name,
                timezone,
                avatar_url,
                has_pin,
                cast_receiver_app_id,
            })
            .into_response()
        }
        _ => (StatusCode::INTERNAL_SERVER_ERROR, "failed to fetch user").into_response(),
    }
}

/// Log out by revoking the current session token.
pub async fn logout(State(state): State<AppState>, headers: HeaderMap) -> impl IntoResponse {
    let token = crate::routes::extract_bearer(&headers)
        .or_else(|| crate::routes::extract_session_cookie(&headers));

    if let Some(token) = token {
        let _ = repositories::auth_tokens::revoke_by_token(&state.pool, &token).await;
    }

    let secure_suffix = if is_https(&headers) { "; Secure" } else { "" };
    let clear_cookie =
        format!("kobbler_session=; Path=/; HttpOnly; SameSite=Lax; Max-Age=0{secure_suffix}",);
    let mut resp_headers = HeaderMap::new();
    if let Ok(v) = HeaderValue::from_str(&clear_cookie) {
        resp_headers.insert("Set-Cookie", v);
    }
    (resp_headers, StatusCode::NO_CONTENT).into_response()
}

#[derive(Deserialize)]
pub struct ChangePasswordRequest {
    pub current_password: String,
    pub new_password: String,
}

/// Change the current user's password.
pub async fn change_password(
    State(state): State<AppState>,
    auth: crate::routes::AuthUser,
    Json(payload): Json<ChangePasswordRequest>,
) -> impl IntoResponse {
    if payload.new_password.len() < 8 {
        return (
            StatusCode::BAD_REQUEST,
            "password must be at least 8 characters",
        )
            .into_response();
    }

    let password_hash = match repositories::users::get_password_hash(&state.pool, auth.user_id)
        .await
    {
        Ok(Some(h)) => h,
        _ => return (StatusCode::INTERNAL_SERVER_ERROR, "failed to fetch user").into_response(),
    };

    if !auth::verify_password(&password_hash, &payload.current_password) {
        return (StatusCode::UNAUTHORIZED, "current password is incorrect").into_response();
    }

    let new_hash = match auth::hash_password(&payload.new_password) {
        Ok(h) => h,
        Err(e) => {
            tracing::error!(error = %e, "failed to hash password");
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                "failed to update password",
            )
                .into_response();
        }
    };
    match repositories::users::update_password(&state.pool, auth.user_id, &new_hash).await {
        Ok(true) => StatusCode::NO_CONTENT.into_response(),
        _ => (
            StatusCode::INTERNAL_SERVER_ERROR,
            "failed to update password",
        )
            .into_response(),
    }
}

#[derive(Deserialize)]
pub struct UpdateDisplayNameRequest {
    pub display_name: Option<String>,
}

/// Set or clear the current user's display name.
///
/// Self-service on purpose: a viewer has no admin routes, and the name is what every
/// profile selector shows for them.
pub async fn update_display_name(
    State(state): State<AppState>,
    auth: crate::routes::AuthUser,
    AxumJson(payload): AxumJson<UpdateDisplayNameRequest>,
) -> impl IntoResponse {
    let name = payload
        .display_name
        .as_deref()
        .map(str::trim)
        .filter(|n| !n.is_empty());
    if name.is_some_and(|n| n.chars().count() > 64) {
        return (StatusCode::BAD_REQUEST, "display name too long").into_response();
    }
    match repositories::users::update_display_name(&state.pool, auth.user_id, name).await {
        Ok(true) => StatusCode::NO_CONTENT.into_response(),
        Ok(false) => (StatusCode::NOT_FOUND, "user not found").into_response(),
        Err(e) => {
            tracing::error!(error = %e, "failed to update display name");
            (StatusCode::INTERNAL_SERVER_ERROR, "database error").into_response()
        }
    }
}

#[derive(Deserialize)]
pub struct UpdateTimezoneRequest {
    pub timezone: Option<String>,
}

/// Set or clear the current user's timezone.
///
/// Self-service like the display name, and validated here: an unparseable zone
/// must be refused at the boundary, because the failure it causes downstream is a
/// notification at 2 am that nothing else reveals.
pub async fn update_timezone(
    State(state): State<AppState>,
    auth: crate::routes::AuthUser,
    AxumJson(payload): AxumJson<UpdateTimezoneRequest>,
) -> impl IntoResponse {
    let raw = payload
        .timezone
        .as_deref()
        .map(str::trim)
        .filter(|z| !z.is_empty());
    if let Some(name) = raw
        && let Err(e) = crate::notifications::prefs::Zone::parse(name)
    {
        return (StatusCode::BAD_REQUEST, e.to_string()).into_response();
    }
    match repositories::users::update_timezone(&state.pool, auth.user_id, raw).await {
        Ok(true) => StatusCode::NO_CONTENT.into_response(),
        Ok(false) => (StatusCode::NOT_FOUND, "user not found").into_response(),
        Err(e) => {
            tracing::error!(error = %e, "failed to update timezone");
            (StatusCode::INTERNAL_SERVER_ERROR, "database error").into_response()
        }
    }
}

/// Upload/replace the current user's avatar.
pub async fn upload_avatar(
    State(state): State<AppState>,
    auth: crate::routes::AuthUser,
    headers: HeaderMap,
    body: axum::body::Bytes,
) -> impl IntoResponse {
    let content_type = headers
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("application/octet-stream");

    let allowed = ["image/jpeg", "image/png", "image/webp"];
    if !allowed.contains(&content_type) {
        return (
            StatusCode::BAD_REQUEST,
            "content type must be image/jpeg, image/png, or image/webp",
        )
            .into_response();
    }

    // 256KB max
    if body.len() > 256 * 1024 {
        return (StatusCode::BAD_REQUEST, "avatar must be 256KB or smaller").into_response();
    }

    match repositories::users::update_avatar(&state.pool, auth.user_id, &body, content_type).await {
        Ok(_) => StatusCode::NO_CONTENT.into_response(),
        Err(e) => {
            tracing::error!(error = %e, "failed to save avatar");
            (StatusCode::INTERNAL_SERVER_ERROR, "failed to save avatar").into_response()
        }
    }
}

/// Serve a user's avatar image.
pub async fn get_avatar(
    State(state): State<AppState>,
    _auth: crate::routes::AuthUser,
    axum::extract::Path(user_id): axum::extract::Path<Uuid>,
) -> impl IntoResponse {
    match repositories::users::get_avatar(&state.pool, user_id).await {
        Ok(Some((data, content_type))) => {
            let mut headers = HeaderMap::new();
            if let Ok(ct) = HeaderValue::from_str(&content_type) {
                headers.insert("Content-Type", ct);
            }
            headers.insert(
                "Cache-Control",
                HeaderValue::from_static("public, max-age=3600"),
            );
            (headers, data).into_response()
        }
        Ok(None) => StatusCode::NOT_FOUND.into_response(),
        Err(e) => {
            tracing::error!(error = %e, "failed to fetch avatar");
            (StatusCode::INTERNAL_SERVER_ERROR, "failed to fetch avatar").into_response()
        }
    }
}

/// Delete the current user's avatar.
pub async fn delete_avatar(
    State(state): State<AppState>,
    auth: crate::routes::AuthUser,
) -> impl IntoResponse {
    match repositories::users::delete_avatar(&state.pool, auth.user_id).await {
        Ok(_) => StatusCode::NO_CONTENT.into_response(),
        Err(e) => {
            tracing::error!(error = %e, "failed to delete avatar");
            (StatusCode::INTERNAL_SERVER_ERROR, "failed to delete avatar").into_response()
        }
    }
}

/// Generate a random alphanumeric token of the specified length.
/// This is used for authentication tokens, device tokens, and ingest tokens.
pub(super) fn generate_token(length: usize) -> String {
    Alphanumeric.sample_string(&mut rand::rng(), length)
}

#[derive(Deserialize)]
pub struct ProfilePinRequest {
    pub pin: String,
}

fn validate_pin(pin: &str) -> Result<(), &'static str> {
    let len = pin.chars().count();
    if !(3..=32).contains(&len) {
        return Err("pin must be 3-32 characters");
    }
    if pin.chars().any(|c| c.is_whitespace()) {
        return Err("pin must not contain whitespace");
    }
    Ok(())
}

/// Set or replace the current user's profile PIN.
pub async fn set_profile_pin(
    State(state): State<AppState>,
    auth: crate::routes::AuthUser,
    Json(payload): Json<ProfilePinRequest>,
) -> impl IntoResponse {
    if let Err(msg) = validate_pin(&payload.pin) {
        return (StatusCode::BAD_REQUEST, msg).into_response();
    }
    let hash = match auth::hash_password(&payload.pin) {
        Ok(h) => h,
        Err(e) => {
            tracing::error!(error = %e, "failed to hash profile pin");
            return (StatusCode::INTERNAL_SERVER_ERROR, "failed to set pin").into_response();
        }
    };
    match repositories::users::set_profile_pin(&state.pool, auth.user_id, &hash).await {
        Ok(_) => StatusCode::NO_CONTENT.into_response(),
        Err(e) => {
            tracing::error!(error = %e, "failed to persist profile pin");
            (StatusCode::INTERNAL_SERVER_ERROR, "failed to set pin").into_response()
        }
    }
}

/// Clear the current user's profile PIN.
pub async fn clear_profile_pin(
    State(state): State<AppState>,
    auth: crate::routes::AuthUser,
) -> impl IntoResponse {
    match repositories::users::clear_profile_pin(&state.pool, auth.user_id).await {
        Ok(_) => StatusCode::NO_CONTENT.into_response(),
        Err(e) => {
            tracing::error!(error = %e, "failed to clear profile pin");
            (StatusCode::INTERNAL_SERVER_ERROR, "failed to clear pin").into_response()
        }
    }
}

/// Verify a submitted PIN against the current user's stored PIN.
///
/// Returns 204 on match, 401 on mismatch, 409 if no PIN is configured.
pub async fn verify_profile_pin(
    State(state): State<AppState>,
    auth: crate::routes::AuthUser,
    Json(payload): Json<ProfilePinRequest>,
) -> impl IntoResponse {
    let hash = match repositories::users::get_profile_pin_hash(&state.pool, auth.user_id).await {
        Ok(Some(h)) => h,
        Ok(None) => return (StatusCode::CONFLICT, "no pin set").into_response(),
        Err(e) => {
            tracing::error!(error = %e, "failed to read profile pin hash");
            return (StatusCode::INTERNAL_SERVER_ERROR, "verify failed").into_response();
        }
    };
    if auth::verify_password(&hash, &payload.pin) {
        StatusCode::NO_CONTENT.into_response()
    } else {
        (StatusCode::UNAUTHORIZED, "invalid pin").into_response()
    }
}
