//! Device and sync route handlers.

use axum::{
    Json as AxumJson,
    extract::{Json, State},
    http::StatusCode,
    response::IntoResponse,
};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::AppState;
use crate::events::LibraryEvent;
use crate::repositories::{devices, entitlements};
use crate::routes::AuthUser;
use crate::types::EntitlementScope;

use super::helpers::record_event;

#[derive(Serialize)]
pub struct LibraryDevice {
    pub id: Uuid,
    pub name: Option<String>,
    pub user_id: Uuid,
}

#[derive(Deserialize)]
pub struct SyncRequest {
    pub book_ids: Vec<Uuid>,
    pub scope_type: Option<String>,
    pub scope_id: Option<Uuid>,
}

pub async fn list_devices(State(state): State<AppState>, auth: AuthUser) -> impl IntoResponse {
    // Non-admins only ever see their own devices; admins keep the household
    // view because they dispatch books to other users' readers.
    let scope = (auth.role != "admin").then_some(auth.user_id);
    match devices::list_summary(&state.pool, scope).await {
        Ok(device_list) => {
            let devices = device_list
                .into_iter()
                .map(|d| LibraryDevice {
                    id: d.id,
                    name: d.name,
                    user_id: d.user_id,
                })
                .collect::<Vec<_>>();
            AxumJson(devices).into_response()
        }
        Err(e) => {
            tracing::error!(error = %e, "failed to list devices");
            (StatusCode::INTERNAL_SERVER_ERROR, "database error").into_response()
        }
    }
}

pub async fn sync_books(
    State(state): State<AppState>,
    auth: AuthUser,
    Json(payload): Json<SyncRequest>,
) -> impl IntoResponse {
    if payload.book_ids.is_empty() {
        return (StatusCode::BAD_REQUEST, "book_ids required").into_response();
    }

    let scope_type: EntitlementScope = payload
        .scope_type
        .as_deref()
        .map(|s| s.parse().unwrap_or_default())
        .unwrap_or_default();
    let scope_id = payload.scope_id;

    if scope_type == EntitlementScope::Device && scope_id.is_none() {
        return (StatusCode::BAD_REQUEST, "scope_id required for DEVICE").into_response();
    }

    // Verify device ownership: non-admins can only sync to their own devices
    if scope_type == EntitlementScope::Device
        && auth.role != "admin"
        && let Some(device_id) = scope_id
    {
        match devices::get_user_id(&state.pool, device_id).await {
            Ok(Some(owner_id)) if owner_id == auth.user_id => {}
            _ => return (StatusCode::FORBIDDEN, "not your device").into_response(),
        }
    }

    for book_id in payload.book_ids {
        let _ = entitlements::create_or_restore(&state.pool, book_id, scope_type, scope_id).await;

        record_event(
            &state,
            "sync.queue",
            Some(book_id),
            "ok",
            Some(scope_type.to_string()),
            None,
        )
        .await;
        let _ = state
            .library_events
            .send(LibraryEvent::library_updated(Some(book_id)));
    }

    StatusCode::NO_CONTENT.into_response()
}
