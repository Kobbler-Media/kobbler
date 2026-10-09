//! Linked-session (auth token) listing, refresh and revocation.

use axum::{
    Json as AxumJson,
    extract::{Json, Path, Query, State},
    http::{HeaderMap, HeaderValue, StatusCode},
    response::IntoResponse,
};
use serde::{Deserialize, Serialize};
use time::OffsetDateTime;
use uuid::Uuid;

use crate::routes::AuthUser;
use crate::{AppState, repositories};

/// Matches the pin-login token lifetime, so a client that checks in keeps a full
/// window ahead of it rather than decaying toward whatever it had left.
pub const SESSION_TTL_DAYS: i32 = 90;

const MAX_DEVICE_NAME: usize = 64;

#[derive(Deserialize)]
pub struct ListSessionsQuery {
    #[serde(default)]
    pub all: bool,
}

#[derive(Serialize)]
pub struct SessionSummary {
    pub id: Uuid,
    pub user_id: Uuid,
    pub email: String,
    pub display_name: Option<String>,
    pub device_name: Option<String>,
    pub client_kind: Option<String>,
    #[serde(with = "time::serde::rfc3339")]
    pub created_at: OffsetDateTime,
    #[serde(with = "time::serde::rfc3339")]
    pub expires_at: OffsetDateTime,
    #[serde(with = "time::serde::rfc3339::option")]
    pub last_used_at: Option<OffsetDateTime>,
    pub created_by_email: Option<String>,
    pub is_current: bool,
}

#[derive(Deserialize)]
pub struct RefreshSessionRequest {
    pub device_name: Option<String>,
    pub client_kind: Option<String>,
}

#[derive(Serialize)]
pub struct RefreshSessionResponse {
    #[serde(with = "time::serde::rfc3339")]
    pub expires_at: OffsetDateTime,
}

fn current_token(headers: &HeaderMap) -> Option<String> {
    crate::routes::extract_bearer(headers)
        .or_else(|| crate::routes::extract_session_cookie(headers))
}

/// Re-issue the session cookie alongside a slid expiry. The browser's own Max-Age is what
/// actually logs a web user out, so extending only the database row would change nothing.
fn refreshed_cookie(headers: &HeaderMap, token: &str) -> Option<HeaderValue> {
    if crate::routes::extract_bearer(headers).is_some() {
        return None;
    }
    let secure_suffix = if super::auth::is_https(headers) {
        "; Secure"
    } else {
        ""
    };
    let max_age = SESSION_TTL_DAYS as i64 * 24 * 60 * 60;
    HeaderValue::from_str(&format!(
        "kobbler_session={token}; Path=/; HttpOnly; SameSite=Lax; Max-Age={max_age}{secure_suffix}",
    ))
    .ok()
}

fn clean_label(value: Option<String>) -> Option<String> {
    value
        .map(|v| v.trim().chars().take(MAX_DEVICE_NAME).collect::<String>())
        .filter(|v| !v.is_empty())
}

pub async fn list_sessions(
    State(state): State<AppState>,
    auth: AuthUser,
    headers: HeaderMap,
    Query(query): Query<ListSessionsQuery>,
) -> impl IntoResponse {
    let everyone = query.all && auth.require_admin().is_ok();
    let scope = if everyone { None } else { Some(auth.user_id) };

    let current_id = match current_token(&headers) {
        Some(token) => repositories::auth_tokens::id_by_token(&state.pool, &token)
            .await
            .unwrap_or(None),
        None => None,
    };

    match repositories::auth_tokens::list_sessions(&state.pool, scope).await {
        Ok(rows) => AxumJson(
            rows.into_iter()
                .map(|r| SessionSummary {
                    is_current: Some(r.id) == current_id,
                    id: r.id,
                    user_id: r.user_id,
                    email: r.email,
                    display_name: r.display_name,
                    device_name: r.device_name,
                    client_kind: r.client_kind,
                    created_at: r.created_at,
                    expires_at: r.expires_at,
                    last_used_at: r.last_used_at,
                    created_by_email: r.created_by_email,
                })
                .collect::<Vec<_>>(),
        )
        .into_response(),
        Err(e) => {
            tracing::error!(error = %e, "failed to list sessions");
            (StatusCode::INTERNAL_SERVER_ERROR, "database error").into_response()
        }
    }
}

pub async fn refresh_session(
    State(state): State<AppState>,
    _auth: AuthUser,
    headers: HeaderMap,
    body: Option<Json<RefreshSessionRequest>>,
) -> impl IntoResponse {
    // A cast/playback `?token=` query satisfies AuthUser but is not an auth_tokens row,
    // so there is nothing to slide - reject rather than silently no-op.
    let Some(token) = current_token(&headers) else {
        return (StatusCode::BAD_REQUEST, "no bearer or session token").into_response();
    };
    let (device_name, client_kind) = match body {
        Some(Json(b)) => (clean_label(b.device_name), clean_label(b.client_kind)),
        None => (None, None),
    };

    match repositories::auth_tokens::touch(
        &state.pool,
        &token,
        SESSION_TTL_DAYS,
        device_name.as_deref(),
        client_kind.as_deref(),
    )
    .await
    {
        Ok(Some(expires_at)) => {
            let mut resp_headers = HeaderMap::new();
            if let Some(cookie) = refreshed_cookie(&headers, &token) {
                resp_headers.insert("Set-Cookie", cookie);
            }
            (
                resp_headers,
                AxumJson(RefreshSessionResponse { expires_at }),
            )
                .into_response()
        }
        Ok(None) => (StatusCode::UNAUTHORIZED, "session is no longer valid").into_response(),
        Err(e) => {
            tracing::error!(error = %e, "failed to refresh session");
            (StatusCode::INTERNAL_SERVER_ERROR, "database error").into_response()
        }
    }
}

pub async fn revoke_session(
    State(state): State<AppState>,
    auth: AuthUser,
    Path(session_id): Path<Uuid>,
) -> impl IntoResponse {
    let owner = match repositories::auth_tokens::owner_of(&state.pool, session_id).await {
        Ok(Some(owner)) => owner,
        Ok(None) => return (StatusCode::NOT_FOUND, "session not found").into_response(),
        Err(e) => {
            tracing::error!(error = %e, "failed to look up session owner");
            return (StatusCode::INTERNAL_SERVER_ERROR, "database error").into_response();
        }
    };

    if owner != auth.user_id && auth.require_admin().is_err() {
        return (StatusCode::FORBIDDEN, "forbidden").into_response();
    }

    match repositories::auth_tokens::revoke_by_id(&state.pool, session_id).await {
        Ok(true) => StatusCode::NO_CONTENT.into_response(),
        Ok(false) => (StatusCode::NOT_FOUND, "session not found").into_response(),
        Err(e) => {
            tracing::error!(error = %e, "failed to revoke session");
            (StatusCode::INTERNAL_SERVER_ERROR, "database error").into_response()
        }
    }
}
