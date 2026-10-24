pub mod admin;
pub mod chat_webhook;
pub mod events;
pub mod health;
pub(crate) mod helpers;
pub mod ingest;
pub mod jobs;
pub mod jobs_concurrency;
pub mod library;
pub mod metadata;
pub mod providers;
pub mod stremio;
pub mod sync;
pub mod wanted;
pub mod watcher;

use axum::extract::{FromRef, FromRequestParts, OptionalFromRequestParts};
use axum::http::request::Parts;
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use uuid::Uuid;

use std::convert::Infallible;

use crate::AppState;
use crate::repositories::{auth_tokens, ingest_tokens, playback_tokens};

/// Extractor for authenticated users.
/// Returns the user's ID and role, or rejects with 401.
#[derive(Debug, Clone)]
pub struct AuthUser {
    pub user_id: Uuid,
    pub role: String,
    pub is_kids: bool,
}

impl AuthUser {
    /// Require admin role, returning 403 if not admin.
    pub fn require_admin(&self) -> Result<(), RoleRejection> {
        if self.role == "admin" {
            Ok(())
        } else {
            Err(RoleRejection)
        }
    }

    /// Require at least contributor role (admin or contributor), returning 403 otherwise.
    pub fn require_contributor(&self) -> Result<(), RoleRejection> {
        if self.role == "admin" || self.role == "contributor" {
            Ok(())
        } else {
            Err(RoleRejection)
        }
    }
}

/// Rejection for missing/invalid auth token (401).
#[derive(Debug)]
pub struct AuthRejection;

impl IntoResponse for AuthRejection {
    fn into_response(self) -> Response {
        (StatusCode::UNAUTHORIZED, "unauthorized").into_response()
    }
}

/// Rejection for insufficient role (403).
#[derive(Debug)]
pub struct RoleRejection;

impl IntoResponse for RoleRejection {
    fn into_response(self) -> Response {
        (StatusCode::FORBIDDEN, "forbidden").into_response()
    }
}

impl<S> FromRequestParts<S> for AuthUser
where
    S: Send + Sync,
    AppState: FromRef<S>,
{
    type Rejection = AuthRejection;

    async fn from_request_parts(parts: &mut Parts, state: &S) -> Result<Self, Self::Rejection> {
        let app_state = AppState::from_ref(state);

        let token = extract_bearer(&parts.headers)
            .or_else(|| extract_session_cookie(&parts.headers))
            .unwrap_or_default();

        match require_auth_token(&app_state, &token).await {
            Some((user_id, role, is_kids)) => Ok(AuthUser {
                user_id,
                role,
                is_kids,
            }),
            None => Err(AuthRejection),
        }
    }
}

impl<S> OptionalFromRequestParts<S> for AuthUser
where
    S: Send + Sync,
    AppState: FromRef<S>,
{
    type Rejection = Infallible;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &S,
    ) -> Result<Option<Self>, Self::Rejection> {
        Ok(
            <AuthUser as FromRequestParts<S>>::from_request_parts(parts, state)
                .await
                .ok(),
        )
    }
}

/// Extractor for the specific routes a playback token is allowed to authenticate:
/// the ones kobapp's player (`KobblerApi.playerUrl`) or the bundled Cast receiver
/// (`web/static/cast-receiver/receiver.js`) actually call with no Authorization
/// header or session cookie available to them — session lifecycle, position/
/// watch-state saves, debrid search/resolve/play, the direct-tuner fallback and
/// IPTV direct play. Bearer and cookie auth work here too, so kobapp's own
/// authenticated calls to the same routes are unaffected. Every other route takes
/// `AuthUser`, which no longer accepts `?token=` at all — a playback token only
/// ever reaches the handlers that explicitly ask for `PlaybackAuth`.
#[derive(Debug, Clone)]
pub struct PlaybackAuth {
    pub user_id: Uuid,
    pub role: String,
    pub is_kids: bool,
}

impl PlaybackAuth {
    /// Require admin role, returning 403 if not admin.
    pub fn require_admin(&self) -> Result<(), RoleRejection> {
        if self.role == "admin" {
            Ok(())
        } else {
            Err(RoleRejection)
        }
    }

    /// Require at least contributor role (admin or contributor), returning 403 otherwise.
    pub fn require_contributor(&self) -> Result<(), RoleRejection> {
        if self.role == "admin" || self.role == "contributor" {
            Ok(())
        } else {
            Err(RoleRejection)
        }
    }
}

impl<S> FromRequestParts<S> for PlaybackAuth
where
    S: Send + Sync,
    AppState: FromRef<S>,
{
    type Rejection = AuthRejection;

    async fn from_request_parts(parts: &mut Parts, state: &S) -> Result<Self, Self::Rejection> {
        let app_state = AppState::from_ref(state);

        let token = extract_bearer(&parts.headers)
            .or_else(|| extract_session_cookie(&parts.headers))
            .unwrap_or_default();

        // Bearer/cookie auth first — kobapp's own authenticated calls to these
        // same routes (not just the Cast receiver's) take this path.
        if let Some((user_id, role, is_kids)) = require_auth_token(&app_state, &token).await {
            return Ok(PlaybackAuth {
                user_id,
                role,
                is_kids,
            });
        }

        // Fall back to ?token= (Cast receiver, libVLC/ExoPlayer/MobileVLCKit —
        // none of which can attach a header to the URL they're handed).
        if let Some(query) = parts.uri.query() {
            for pair in query.split('&') {
                if let Some(val) = pair.strip_prefix("token=") {
                    let val = val.trim();
                    if !val.is_empty()
                        && let Ok(Some((user_id, role, is_kids))) =
                            playback_tokens::validate(&app_state.pool, val).await
                    {
                        return Ok(PlaybackAuth {
                            user_id,
                            role,
                            is_kids,
                        });
                    }
                }
            }
        }

        Err(AuthRejection)
    }
}

impl<S> OptionalFromRequestParts<S> for PlaybackAuth
where
    S: Send + Sync,
    AppState: FromRef<S>,
{
    type Rejection = Infallible;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &S,
    ) -> Result<Option<Self>, Self::Rejection> {
        Ok(
            <PlaybackAuth as FromRequestParts<S>>::from_request_parts(parts, state)
                .await
                .ok(),
        )
    }
}

/// Extract bearer token from Authorization header
pub fn extract_bearer(headers: &HeaderMap) -> Option<String> {
    let auth = headers
        .get("authorization")
        .and_then(|val| val.to_str().ok())
        .unwrap_or("");

    let token = auth.strip_prefix("Bearer ").unwrap_or("").trim();
    if token.is_empty() {
        None
    } else {
        Some(token.to_string())
    }
}

/// Extract session token from kobbler_session cookie
pub fn extract_session_cookie(headers: &HeaderMap) -> Option<String> {
    let cookie_header = headers.get("cookie")?.to_str().ok()?;
    for part in cookie_header.split(';') {
        let trimmed = part.trim();
        if let Some(value) = trimmed.strip_prefix("kobbler_session=") {
            let token = value.trim();
            if !token.is_empty() {
                return Some(token.to_string());
            }
        }
    }
    None
}

/// The (user_id, role) behind an auth token, or None if it does not name one.
///
/// There is one way to fail and nothing to say about it — an unusable token is an
/// unusable token — so the absence is the whole answer.
pub async fn require_auth_token(state: &AppState, token: &str) -> Option<(Uuid, String, bool)> {
    if token.is_empty() {
        return None;
    }

    auth_tokens::validate_admin_token(&state.pool, token)
        .await
        .ok()?
}

/// Validate an auth token and return only the user_id.
/// Used by routes that do manual token validation (covers, images).
pub async fn require_admin_token(state: &AppState, token: &str) -> Option<Uuid> {
    require_auth_token(state, token)
        .await
        .map(|(uid, _, _)| uid)
}

/// Extractor for ingest token authentication.
#[derive(Debug, Clone)]
pub struct IngestAuth;

/// Rejection type for IngestAuth extractor.
#[derive(Debug)]
pub struct IngestAuthRejection;

impl IntoResponse for IngestAuthRejection {
    fn into_response(self) -> Response {
        (StatusCode::UNAUTHORIZED, "unauthorized").into_response()
    }
}

impl<S> FromRequestParts<S> for IngestAuth
where
    S: Send + Sync,
    AppState: FromRef<S>,
{
    type Rejection = IngestAuthRejection;

    async fn from_request_parts(parts: &mut Parts, state: &S) -> Result<Self, Self::Rejection> {
        let app_state = AppState::from_ref(state);
        let token = extract_bearer(&parts.headers).unwrap_or_default();

        if !require_ingest_token(&app_state, &token).await {
            return Err(IngestAuthRejection);
        }

        Ok(IngestAuth)
    }
}

/// Whether an ingest token is one the server issued.
pub async fn require_ingest_token(state: &AppState, token: &str) -> bool {
    if token.is_empty() {
        return false;
    }

    ingest_tokens::is_valid(&state.pool, token)
        .await
        .unwrap_or(false)
}

/// Extractor for routes that serve both the kobbler frontend (session cookie
/// or admin bearer) and external service callers using an ingest token
/// (koblem's `HttpLibraryProvider`). Returns a marker — no user identity is
/// available when an ingest token authenticates.
#[derive(Debug, Clone)]
pub struct ProviderOrUserAuth;

impl<S> FromRequestParts<S> for ProviderOrUserAuth
where
    S: Send + Sync,
    AppState: FromRef<S>,
{
    type Rejection = AuthRejection;

    async fn from_request_parts(parts: &mut Parts, state: &S) -> Result<Self, Self::Rejection> {
        let app_state = AppState::from_ref(state);

        let token = extract_bearer(&parts.headers)
            .or_else(|| extract_session_cookie(&parts.headers))
            .unwrap_or_default();

        if require_auth_token(&app_state, &token).await.is_some() {
            return Ok(ProviderOrUserAuth);
        }
        if require_ingest_token(&app_state, &token).await {
            return Ok(ProviderOrUserAuth);
        }
        Err(AuthRejection)
    }
}
