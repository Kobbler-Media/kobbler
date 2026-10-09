use axum::Json as AxumJson;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::IntoResponse;
use serde::{Deserialize, Deserializer, Serialize};
use time::OffsetDateTime;
use uuid::Uuid;

use crate::AppState;
use crate::chatbot::identity::{self, ROLE_ADMIN, ROLE_CONTRIBUTOR, ROLE_VIEWER};
use crate::chatbot::transport::{ChatTransportInfo, ChatTransportRegistry};
use crate::repositories::chat_identities::{
    self, ChatIdentityPatch, ChatIdentityWithUser, NewChatIdentity, PairingCode,
};
use crate::repositories::users;
use crate::routes::AuthUser;

/// What an admin needs to tell a working binding from a typo'd one.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum IdentityStatus {
    /// Switched off by an admin.
    Disabled,
    /// Added, but nothing has ever arrived from it.
    AwaitingFirstMessage,
    Active,
}

#[derive(Debug, Serialize)]
pub struct ChatIdentityResponse {
    pub id: Uuid,
    pub transport: String,
    pub address: Option<String>,
    pub external_id: Option<String>,
    pub user_id: Uuid,
    pub user_email: String,
    pub user_display_name: Option<String>,
    pub user_role: String,
    pub max_role: String,
    pub effective_role: String,
    pub enabled: bool,
    pub status: IdentityStatus,
    #[serde(with = "time::serde::rfc3339")]
    pub created_at: OffsetDateTime,
    #[serde(with = "time::serde::rfc3339::option")]
    pub last_seen_at: Option<OffsetDateTime>,
}

#[derive(Debug, Serialize)]
pub struct PendingPairingResponse {
    pub id: Uuid,
    pub transport: String,
    pub external_id: String,
    pub code: String,
    pub display_name: Option<String>,
    pub space: Option<String>,
    #[serde(with = "time::serde::rfc3339")]
    pub created_at: OffsetDateTime,
    #[serde(with = "time::serde::rfc3339")]
    pub expires_at: OffsetDateTime,
}

#[derive(Debug, Serialize)]
pub struct ChatIdentitiesResponse {
    pub identities: Vec<ChatIdentityResponse>,
    pub pending_pairings: Vec<PendingPairingResponse>,
    pub transports: Vec<&'static ChatTransportInfo>,
}

#[derive(Debug, Deserialize)]
pub struct CreateChatIdentityRequest {
    pub transport: String,
    pub user_id: Uuid,
    #[serde(default)]
    pub address: Option<String>,
    #[serde(default)]
    pub external_id: Option<String>,
    #[serde(default)]
    pub max_role: Option<String>,
    #[serde(default)]
    pub enabled: Option<bool>,
}

/// `Some(None)` clears the field; a missing key leaves it alone.
fn explicit_null<'de, T, D>(de: D) -> Result<Option<Option<T>>, D::Error>
where
    T: Deserialize<'de>,
    D: Deserializer<'de>,
{
    Deserialize::deserialize(de).map(Some)
}

#[derive(Debug, Default, Deserialize)]
pub struct UpdateChatIdentityRequest {
    #[serde(default, deserialize_with = "explicit_null")]
    pub address: Option<Option<String>>,
    #[serde(default, deserialize_with = "explicit_null")]
    pub external_id: Option<Option<String>>,
    #[serde(default)]
    pub user_id: Option<Uuid>,
    #[serde(default)]
    pub max_role: Option<String>,
    #[serde(default)]
    pub enabled: Option<bool>,
}

#[derive(Debug, Deserialize)]
pub struct ApprovePairingRequest {
    /// Bind the code to an identity row that already exists.
    #[serde(default)]
    pub identity_id: Option<Uuid>,
    /// Or mint one for this kobbler user.
    #[serde(default)]
    pub user_id: Option<Uuid>,
    #[serde(default)]
    pub max_role: Option<String>,
}

fn to_response(row: ChatIdentityWithUser) -> ChatIdentityResponse {
    let status = if !row.identity.enabled {
        IdentityStatus::Disabled
    } else if row.identity.last_seen_at.is_none() {
        IdentityStatus::AwaitingFirstMessage
    } else {
        IdentityStatus::Active
    };
    ChatIdentityResponse {
        effective_role: identity::effective_role(&row.user_role, &row.identity.max_role),
        id: row.identity.id,
        transport: row.identity.transport,
        address: row.identity.address,
        external_id: row.identity.external_id,
        user_id: row.identity.user_id,
        user_email: row.user_email,
        user_display_name: row.user_display_name,
        user_role: row.user_role,
        max_role: row.identity.max_role,
        enabled: row.identity.enabled,
        status,
        created_at: row.identity.created_at,
        last_seen_at: row.identity.last_seen_at,
    }
}

fn to_pairing_response(row: PairingCode) -> PendingPairingResponse {
    PendingPairingResponse {
        id: row.id,
        transport: row.transport,
        external_id: row.external_id,
        code: row.code,
        display_name: row.display_name,
        space: row.space,
        created_at: row.created_at,
        expires_at: row.expires_at,
    }
}

fn clean(value: Option<String>) -> Option<String> {
    value
        .map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty())
}

fn valid_role(role: &str) -> bool {
    matches!(role, ROLE_VIEWER | ROLE_CONTRIBUTOR | ROLE_ADMIN)
}

async fn user_exists(state: &AppState, user_id: Uuid) -> bool {
    matches!(users::get_by_id(&state.pool, user_id).await, Ok(Some(_)))
}

fn db_error(e: impl std::fmt::Display, what: &str) -> axum::response::Response {
    tracing::error!(error = %e, "chat identity {what} failed");
    (StatusCode::INTERNAL_SERVER_ERROR, "database error").into_response()
}

pub async fn list_chat_identities(
    State(state): State<AppState>,
    auth: AuthUser,
) -> impl IntoResponse {
    if let Err(e) = auth.require_admin() {
        return e.into_response();
    }

    let identities = match chat_identities::list(&state.pool).await {
        Ok(rows) => rows.into_iter().map(to_response).collect(),
        Err(e) => return db_error(e, "list"),
    };
    let pending_pairings = match chat_identities::list_pending_pairing_codes(&state.pool).await {
        Ok(rows) => rows.into_iter().map(to_pairing_response).collect(),
        Err(e) => return db_error(e, "pending pairings"),
    };

    AxumJson(ChatIdentitiesResponse {
        identities,
        pending_pairings,
        transports: ChatTransportRegistry::new().available_types(),
    })
    .into_response()
}

pub async fn create_chat_identity(
    State(state): State<AppState>,
    auth: AuthUser,
    AxumJson(payload): AxumJson<CreateChatIdentityRequest>,
) -> impl IntoResponse {
    if let Err(e) = auth.require_admin() {
        return e.into_response();
    }

    let transport = payload.transport.trim().to_string();
    if ChatTransportRegistry::new()
        .get_type_info(&transport)
        .is_none()
    {
        return (StatusCode::BAD_REQUEST, "unknown transport").into_response();
    }

    let address = clean(payload.address);
    let external_id = clean(payload.external_id);
    if address.is_none() && external_id.is_none() {
        return (StatusCode::BAD_REQUEST, "address or external_id required").into_response();
    }

    let max_role = payload
        .max_role
        .map(|r| r.trim().to_string())
        .unwrap_or_else(|| ROLE_CONTRIBUTOR.to_string());
    if !valid_role(&max_role) {
        return (StatusCode::BAD_REQUEST, "invalid max_role").into_response();
    }

    if !user_exists(&state, payload.user_id).await {
        return (StatusCode::BAD_REQUEST, "unknown user").into_response();
    }

    let input = NewChatIdentity {
        transport: &transport,
        address: address.as_deref(),
        external_id: external_id.as_deref(),
        user_id: payload.user_id,
        max_role: &max_role,
        enabled: payload.enabled.unwrap_or(true),
    };

    match chat_identities::create(&state.pool, &input).await {
        Ok(row) => (StatusCode::CREATED, AxumJson(row.id)).into_response(),
        Err(crate::repositories::RepositoryError::Database(sqlx::Error::Database(e)))
            if e.is_unique_violation() =>
        {
            (StatusCode::CONFLICT, "that address is already bound").into_response()
        }
        Err(e) => db_error(e, "create"),
    }
}

pub async fn update_chat_identity(
    State(state): State<AppState>,
    auth: AuthUser,
    Path(id): Path<Uuid>,
    AxumJson(payload): AxumJson<UpdateChatIdentityRequest>,
) -> impl IntoResponse {
    if let Err(e) = auth.require_admin() {
        return e.into_response();
    }

    if let Some(role) = payload.max_role.as_deref()
        && !valid_role(role.trim())
    {
        return (StatusCode::BAD_REQUEST, "invalid max_role").into_response();
    }
    if let Some(user_id) = payload.user_id
        && !user_exists(&state, user_id).await
    {
        return (StatusCode::BAD_REQUEST, "unknown user").into_response();
    }

    let existing = match chat_identities::get(&state.pool, id).await {
        Ok(Some(row)) => row,
        Ok(None) => return (StatusCode::NOT_FOUND, "not found").into_response(),
        Err(e) => return db_error(e, "load"),
    };

    let address = payload.address.map(clean);
    let external_id = payload.external_id.map(clean);

    let next_address = address.clone().unwrap_or(existing.address);
    let next_external = external_id.clone().unwrap_or(existing.external_id);
    if next_address.is_none() && next_external.is_none() {
        return (StatusCode::BAD_REQUEST, "address or external_id required").into_response();
    }

    let max_role = payload.max_role.map(|r| r.trim().to_string());
    let patch = ChatIdentityPatch {
        address: address.as_ref().map(|v| v.as_deref()),
        external_id: external_id.as_ref().map(|v| v.as_deref()),
        user_id: payload.user_id,
        max_role: max_role.as_deref(),
        enabled: payload.enabled,
    };

    match chat_identities::update(&state.pool, id, &patch).await {
        Ok(Some(_)) => StatusCode::NO_CONTENT.into_response(),
        Ok(None) => (StatusCode::NOT_FOUND, "not found").into_response(),
        Err(crate::repositories::RepositoryError::Database(sqlx::Error::Database(e)))
            if e.is_unique_violation() =>
        {
            (StatusCode::CONFLICT, "that address is already bound").into_response()
        }
        Err(e) => db_error(e, "update"),
    }
}

pub async fn delete_chat_identity(
    State(state): State<AppState>,
    auth: AuthUser,
    Path(id): Path<Uuid>,
) -> impl IntoResponse {
    if let Err(e) = auth.require_admin() {
        return e.into_response();
    }
    match chat_identities::delete(&state.pool, id).await {
        Ok(true) => StatusCode::NO_CONTENT.into_response(),
        Ok(false) => (StatusCode::NOT_FOUND, "not found").into_response(),
        Err(e) => db_error(e, "delete"),
    }
}

pub async fn approve_chat_pairing(
    State(state): State<AppState>,
    auth: AuthUser,
    Path(id): Path<Uuid>,
    AxumJson(payload): AxumJson<ApprovePairingRequest>,
) -> impl IntoResponse {
    if let Err(e) = auth.require_admin() {
        return e.into_response();
    }

    let max_role = payload
        .max_role
        .map(|r| r.trim().to_string())
        .unwrap_or_else(|| ROLE_CONTRIBUTOR.to_string());
    if !valid_role(&max_role) {
        return (StatusCode::BAD_REQUEST, "invalid max_role").into_response();
    }

    let pending = match chat_identities::get_pairing_code(&state.pool, id).await {
        Ok(Some(row)) => row,
        Ok(None) => return (StatusCode::NOT_FOUND, "not found").into_response(),
        Err(e) => return db_error(e, "load pairing"),
    };
    if pending.consumed_at.is_some() {
        return (StatusCode::CONFLICT, "already approved").into_response();
    }
    if pending.expires_at <= OffsetDateTime::now_utc() {
        return (StatusCode::GONE, "code expired").into_response();
    }

    let identity_id = match (payload.identity_id, payload.user_id) {
        (Some(identity_id), None) => match chat_identities::get(&state.pool, identity_id).await {
            Ok(Some(row)) if row.transport == pending.transport => identity_id,
            Ok(Some(_)) => {
                return (StatusCode::BAD_REQUEST, "identity is on another transport")
                    .into_response();
            }
            Ok(None) => return (StatusCode::BAD_REQUEST, "unknown identity").into_response(),
            Err(e) => return db_error(e, "load identity"),
        },
        (None, Some(user_id)) => {
            if !user_exists(&state, user_id).await {
                return (StatusCode::BAD_REQUEST, "unknown user").into_response();
            }
            let input = NewChatIdentity {
                transport: &pending.transport,
                address: None,
                external_id: Some(&pending.external_id),
                user_id,
                max_role: &max_role,
                enabled: true,
            };
            match chat_identities::create(&state.pool, &input).await {
                Ok(row) => row.id,
                Err(crate::repositories::RepositoryError::Database(sqlx::Error::Database(e)))
                    if e.is_unique_violation() =>
                {
                    return (StatusCode::CONFLICT, "that sender is already bound").into_response();
                }
                Err(e) => return db_error(e, "create from pairing"),
            }
        }
        _ => {
            return (
                StatusCode::BAD_REQUEST,
                "exactly one of identity_id or user_id required",
            )
                .into_response();
        }
    };

    match identity::approve_pairing(&state.pool, &pending.transport, &pending.code, identity_id)
        .await
    {
        Ok(Some(_)) => StatusCode::NO_CONTENT.into_response(),
        Ok(None) => (StatusCode::CONFLICT, "code is no longer valid").into_response(),
        Err(e) => db_error(e, "approve pairing"),
    }
}

pub async fn reject_chat_pairing(
    State(state): State<AppState>,
    auth: AuthUser,
    Path(id): Path<Uuid>,
) -> impl IntoResponse {
    if let Err(e) = auth.require_admin() {
        return e.into_response();
    }
    match chat_identities::delete_pairing_code(&state.pool, id).await {
        Ok(true) => StatusCode::NO_CONTENT.into_response(),
        Ok(false) => (StatusCode::NOT_FOUND, "not found").into_response(),
        Err(e) => db_error(e, "delete pairing"),
    }
}
