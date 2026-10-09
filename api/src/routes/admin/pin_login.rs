//! PIN-based client app login via a configgymajiggy relay (`KOBBLER_PIN_SERVICE_URL`,
//! default pin.biboop.com).

use axum::{Json, extract::State, http::StatusCode, response::IntoResponse};
use serde::Deserialize;
use std::collections::HashMap;
use tracing::error;

use super::auth::generate_token;
use crate::routes::AuthUser;
use crate::{AppState, http, repositories};

const PIN_NAMESPACE: &str = "kobbler";

/// The pin service owns the pin format, and its length is configurable there.
/// Only sanity-check the shape here - pinning an exact length means every
/// PIN_LENGTH change on the service rejects every login.
const PIN_MIN_LEN: usize = 4;
const PIN_MAX_LEN: usize = 32;

#[derive(Deserialize)]
pub struct PinLoginRequest {
    pub pin: String,
    pub api_url: String,
}

pub async fn pin_login(
    State(state): State<AppState>,
    auth: AuthUser,
    Json(payload): Json<PinLoginRequest>,
) -> impl IntoResponse {
    let pin = payload.pin.trim().to_uppercase();
    if !(PIN_MIN_LEN..=PIN_MAX_LEN).contains(&pin.len())
        || !pin.chars().all(|c| c.is_ascii_alphanumeric())
    {
        return (
            StatusCode::BAD_REQUEST,
            "PIN must be 4-32 alphanumeric characters",
        )
            .into_response();
    }

    let api_url = payload.api_url.trim().trim_end_matches('/');
    if api_url.is_empty() {
        return (StatusCode::BAD_REQUEST, "api_url is required").into_response();
    }

    let token = generate_token(state.config.token_length);
    if let Err(e) = repositories::auth_tokens::create_with_expiry_days(
        &state.pool,
        auth.user_id,
        &token,
        "admin",
        super::sessions::SESSION_TTL_DAYS,
        None,
        None,
    )
    .await
    {
        error!(error = %e, "failed to create PIN login token");
        return (StatusCode::INTERNAL_SERVER_ERROR, "failed to create token").into_response();
    }

    let mut config_payload = HashMap::new();
    config_payload.insert("api_url", api_url.to_string());
    config_payload.insert("token", token);

    let url = format!(
        "{}/pin/{}/{}",
        state.config.pin_service_url, PIN_NAMESPACE, pin
    );
    let client = http::new_client();
    match client.put(&url).json(&config_payload).send().await {
        Ok(resp) if resp.status() == reqwest::StatusCode::ACCEPTED => {
            StatusCode::NO_CONTENT.into_response()
        }
        Ok(resp) if resp.status() == reqwest::StatusCode::NOT_FOUND => {
            (StatusCode::NOT_FOUND, "PIN not found or expired").into_response()
        }
        Ok(resp) if resp.status() == reqwest::StatusCode::CONFLICT => (
            StatusCode::CONFLICT,
            "PIN already used; ask the app for a new one",
        )
            .into_response(),
        Ok(resp) => {
            let status = resp.status();
            error!(%status, "unexpected response from PIN service");
            (
                StatusCode::BAD_GATEWAY,
                "unexpected response from PIN service",
            )
                .into_response()
        }
        Err(e) => {
            error!(error = %e, "failed to contact PIN service");
            (StatusCode::BAD_GATEWAY, "failed to contact PIN service").into_response()
        }
    }
}
