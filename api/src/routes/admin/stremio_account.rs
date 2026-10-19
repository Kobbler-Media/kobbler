//! The instance-wide Stremio account whose Continue Watching shows on every profile's home page.

use axum::{Json, extract::State, http::StatusCode, response::IntoResponse};
use serde::Deserialize;
use serde_json::json;

use crate::AppState;
use crate::repositories::stremio_accounts;
use crate::routes::AuthUser;
use crate::stremio_library::client::{StremioClient, StremioError};

#[derive(Deserialize)]
pub struct LinkBody {
    pub email: String,
    pub password: String,
}

/// GET /api/v1/admin/stremio/account
pub async fn get_account(State(state): State<AppState>, auth: AuthUser) -> impl IntoResponse {
    if let Err(e) = auth.require_admin() {
        return e.into_response();
    }
    match stremio_accounts::instance_status(&state.pool).await {
        Ok(account) => Json(json!({ "account": account })).into_response(),
        Err(e) => internal(e),
    }
}

/// PUT /api/v1/admin/stremio/account
///
/// Signs in to Stremio and keeps only the session key — never the password.
pub async fn link_account(
    State(state): State<AppState>,
    auth: AuthUser,
    Json(body): Json<LinkBody>,
) -> impl IntoResponse {
    if let Err(e) = auth.require_admin() {
        return e.into_response();
    }
    let email = body.email.trim();
    if email.is_empty() || body.password.is_empty() {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({ "error": "Email and password are required" })),
        )
            .into_response();
    }
    let auth_key = match StremioClient::new(&state.config.stremio_api_url)
        .login(email, &body.password)
        .await
    {
        Ok(key) => key,
        Err(StremioError::Rejected(msg)) => {
            return (StatusCode::UNAUTHORIZED, Json(json!({ "error": msg }))).into_response();
        }
        Err(e) => {
            return (
                StatusCode::BAD_GATEWAY,
                Json(json!({ "error": e.to_string() })),
            )
                .into_response();
        }
    };
    if let Err(e) = stremio_accounts::upsert_instance(&state.pool, email, &auth_key).await {
        return internal(e);
    }
    enqueue_sync(&state).await;
    get_account(State(state), auth).await.into_response()
}

/// DELETE /api/v1/admin/stremio/account
pub async fn unlink_account(State(state): State<AppState>, auth: AuthUser) -> impl IntoResponse {
    if let Err(e) = auth.require_admin() {
        return e.into_response();
    }
    match stremio_accounts::delete_instance(&state.pool).await {
        Ok(_) => StatusCode::NO_CONTENT.into_response(),
        Err(e) => internal(e),
    }
}

/// POST /api/v1/admin/stremio/account/sync
pub async fn sync_account(State(state): State<AppState>, auth: AuthUser) -> impl IntoResponse {
    if let Err(e) = auth.require_admin() {
        return e.into_response();
    }
    enqueue_sync(&state).await;
    StatusCode::ACCEPTED.into_response()
}

async fn enqueue_sync(state: &AppState) {
    if let Err(e) = state
        .nats
        .publisher()
        .enqueue_job("kobbler", "stremio_sync", json!({}))
        .await
    {
        tracing::warn!("Failed to enqueue Stremio sync: {e}");
    }
}

fn internal(e: impl std::fmt::Display) -> axum::response::Response {
    (
        StatusCode::INTERNAL_SERVER_ERROR,
        Json(json!({ "error": e.to_string() })),
    )
        .into_response()
}
