//! A read-aloud cast: rendered once on the server, heard by a receiver that holds
//! only a playback token, and followed by the phone that started it.

use axum::Json;
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use serde::Deserialize;
use serde_json::json;
use tracing::error;
use uuid::Uuid;

use super::tts::{SynthesizeRequest, ndjson, plan_synthesis};
use crate::AppState;
use crate::routes::{AuthUser, PlaybackAuth};

/// POST /api/v1/admin/tts/cast — same body as synthesize, answered with `{id}`.
pub async fn start(
    State(state): State<AppState>,
    auth: AuthUser,
    Json(req): Json<SynthesizeRequest>,
) -> Response {
    let plan = match plan_synthesis(&state, auth.user_id, req).await {
        Ok(plan) => plan,
        Err(response) => return response,
    };
    match state.tts_casts.start(auth.user_id, plan) {
        Ok(id) => Json(json!({"id": id})).into_response(),
        Err(e) => {
            error!(error = %e, "no AAC encoder for the cast read");
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({"error": "Cannot encode audio for casting"})),
            )
                .into_response()
        }
    }
}

#[derive(Debug, Deserialize)]
pub struct AudioQuery {
    #[serde(default)]
    pub from_ms: u64,
}

/// GET /api/v1/admin/tts/cast/{id}/audio — one progressive ADTS AAC stream of the
/// whole read, with no length. `from_ms` reopens it part way, for a receiver whose
/// connection dropped.
pub async fn audio(
    State(state): State<AppState>,
    auth: PlaybackAuth,
    Path(id): Path<Uuid>,
    Query(query): Query<AudioQuery>,
) -> Response {
    let Some(render) = state.tts_casts.get(id, auth.user_id) else {
        return not_found();
    };
    Response::builder()
        .status(StatusCode::OK)
        .header("content-type", "audio/aac")
        .header("cache-control", "no-store")
        .header("x-accel-buffering", "no")
        .body(axum::body::Body::from_stream(render.audio(query.from_ms)))
        .unwrap()
}

/// GET /api/v1/admin/tts/cast/{id}/events — the synthesize timeline without its audio.
pub async fn events(
    State(state): State<AppState>,
    auth: AuthUser,
    Path(id): Path<Uuid>,
) -> Response {
    match state.tts_casts.get(id, auth.user_id) {
        Some(render) => ndjson(render.events()),
        None => not_found(),
    }
}

fn not_found() -> Response {
    (
        StatusCode::NOT_FOUND,
        Json(json!({"error": "No such cast"})),
    )
        .into_response()
}
