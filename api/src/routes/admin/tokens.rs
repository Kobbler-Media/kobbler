//! Ingest token management route handlers.

use axum::{
    Json as AxumJson,
    extract::{Json, Path, State},
    http::StatusCode,
    response::IntoResponse,
};
use rand::distr::{Alphanumeric, SampleString};
use serde::{Deserialize, Serialize};
use time::OffsetDateTime;
use uuid::Uuid;

use crate::AppState;
use crate::repositories;
use crate::routes::AuthUser;

// ============================================================================
// Types
// ============================================================================

#[derive(Deserialize)]
pub struct CreateIngestTokenRequest {
    pub name: Option<String>,
}

#[derive(Deserialize)]
pub struct UpdateIngestTokenRequest {
    pub name: String,
}

#[derive(Serialize)]
pub struct CreateIngestTokenResponse {
    pub token: String,
}

#[derive(Serialize)]
pub struct IngestTokenSummary {
    pub id: Uuid,
    pub token: String,
    pub name: Option<String>,
    #[serde(with = "time::serde::rfc3339")]
    pub created_at: OffsetDateTime,
    pub revoked: bool,
}

// ============================================================================
// Helpers
// ============================================================================

fn generate_token(length: usize) -> String {
    Alphanumeric.sample_string(&mut rand::rng(), length)
}

// ============================================================================
// Handlers
// ============================================================================

pub async fn list_ingest_tokens(
    State(state): State<AppState>,
    auth: AuthUser,
) -> impl IntoResponse {
    if let Err(e) = auth.require_admin() {
        return e.into_response();
    }
    match repositories::ingest_tokens::list_all(&state.pool).await {
        Ok(tokens) => {
            let tokens = tokens
                .into_iter()
                .map(|t| IngestTokenSummary {
                    id: t.id,
                    token: t.token,
                    name: t.name,
                    created_at: t.created_at,
                    revoked: t.revoked,
                })
                .collect::<Vec<_>>();
            AxumJson(tokens).into_response()
        }
        Err(e) => {
            tracing::error!(error = %e, "failed to list ingest tokens");
            (StatusCode::INTERNAL_SERVER_ERROR, "database error").into_response()
        }
    }
}

pub async fn create_ingest_token(
    State(state): State<AppState>,
    auth: AuthUser,
    body: Option<Json<CreateIngestTokenRequest>>,
) -> impl IntoResponse {
    if let Err(e) = auth.require_admin() {
        return e.into_response();
    }
    let name = body.and_then(|b| b.0.name).unwrap_or_default();
    let token = generate_token(state.config.token_length);

    if repositories::ingest_tokens::create(&state.pool, &token, &name)
        .await
        .is_err()
    {
        return (StatusCode::INTERNAL_SERVER_ERROR, "failed to create token").into_response();
    }

    AxumJson(CreateIngestTokenResponse { token }).into_response()
}

pub async fn update_ingest_token(
    State(state): State<AppState>,
    auth: AuthUser,
    Path(token_id): Path<String>,
    Json(body): Json<UpdateIngestTokenRequest>,
) -> impl IntoResponse {
    if let Err(e) = auth.require_admin() {
        return e.into_response();
    }
    let token_id = match Uuid::parse_str(&token_id) {
        Ok(id) => id,
        Err(_) => return (StatusCode::BAD_REQUEST, "invalid token id").into_response(),
    };

    match repositories::ingest_tokens::update_name(&state.pool, token_id, &body.name).await {
        Ok(true) => StatusCode::NO_CONTENT.into_response(),
        Ok(false) => (StatusCode::NOT_FOUND, "token not found").into_response(),
        Err(e) => {
            tracing::error!(error = %e, "failed to update ingest token name");
            (StatusCode::INTERNAL_SERVER_ERROR, "failed to update").into_response()
        }
    }
}

pub async fn revoke_ingest_token(
    State(state): State<AppState>,
    auth: AuthUser,
    Path(token_id): Path<String>,
) -> impl IntoResponse {
    if let Err(e) = auth.require_admin() {
        return e.into_response();
    }
    let token_id = match Uuid::parse_str(&token_id) {
        Ok(id) => id,
        Err(_) => return (StatusCode::BAD_REQUEST, "invalid token id").into_response(),
    };

    if repositories::ingest_tokens::revoke(&state.pool, token_id)
        .await
        .is_err()
    {
        return (StatusCode::INTERNAL_SERVER_ERROR, "failed to revoke").into_response();
    }

    StatusCode::NO_CONTENT.into_response()
}
