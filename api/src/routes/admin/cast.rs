//! Google Cast playback token route handlers.
//!
//! Short-lived tokens for cookie-free authentication, used by Cast receivers
//! and external players that cannot send session cookies.

use axum::{
    Json as AxumJson,
    extract::{Path, State},
    http::StatusCode,
    response::IntoResponse,
};
use rand::distr::{Alphanumeric, SampleString};
use serde::{Deserialize, Serialize};
use time::OffsetDateTime;

use crate::AppState;
use crate::repositories;
use crate::routes::PlaybackAuth;

#[derive(Deserialize)]
pub struct CreatePlaybackTokenRequest {
    /// Token TTL in hours. Defaults to 24.
    pub ttl_hours: Option<i64>,
}

#[derive(Serialize)]
pub struct CreatePlaybackTokenResponse {
    pub token: String,
    #[serde(with = "time::serde::rfc3339")]
    pub expires_at: OffsetDateTime,
}

fn generate_token(length: usize) -> String {
    Alphanumeric.sample_string(&mut rand::rng(), length)
}

pub async fn create_playback_token(
    State(state): State<AppState>,
    auth: PlaybackAuth,
    body: Option<axum::extract::Json<CreatePlaybackTokenRequest>>,
) -> impl IntoResponse {
    let ttl_hours = body.and_then(|b| b.0.ttl_hours).unwrap_or(24);
    if !(1..=168).contains(&ttl_hours) {
        return (
            StatusCode::BAD_REQUEST,
            "ttl_hours must be between 1 and 168",
        )
            .into_response();
    }

    let token = generate_token(state.config.token_length);
    let expires_at = OffsetDateTime::now_utc() + time::Duration::hours(ttl_hours);

    match repositories::playback_tokens::create(&state.pool, auth.user_id, &token, expires_at).await
    {
        Ok(_) => AxumJson(CreatePlaybackTokenResponse { token, expires_at }).into_response(),
        Err(e) => {
            tracing::error!(error = %e, "failed to create playback token");
            (StatusCode::INTERNAL_SERVER_ERROR, "failed to create token").into_response()
        }
    }
}

pub async fn revoke_playback_token(
    State(state): State<AppState>,
    _auth: PlaybackAuth,
    Path(token): Path<String>,
) -> impl IntoResponse {
    match repositories::playback_tokens::revoke(&state.pool, &token).await {
        Ok(true) => StatusCode::NO_CONTENT.into_response(),
        Ok(false) => (StatusCode::NOT_FOUND, "token not found").into_response(),
        Err(e) => {
            tracing::error!(error = %e, "failed to revoke playback token");
            (StatusCode::INTERNAL_SERVER_ERROR, "failed to revoke token").into_response()
        }
    }
}
