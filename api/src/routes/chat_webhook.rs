//! `POST /api/v1/chat/{transport}/webhook` — unauthenticated, verified per transport.
//!
//! Two clocks force the shape here: Google gives the webhook ~30 s, and a real turn can
//! take much longer than that. So this handler only verifies and answers; everything
//! past identity resolution belongs to [`crate::chatbot::inbound`], which the Discord
//! gateway enters at the same door.
//!
//! Not every transport has a webhook. Discord's plain messages arrive over a socket, so
//! `verify` rejects there and this route 503s — which is the truth, not a failure.

use axum::body::Bytes;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::IntoResponse;
use serde_json::json;

use crate::AppState;
use crate::chatbot::inbound::{self, Accepted};
use crate::chatbot::transport::{
    ChatError, ChatTransport, ChatTransportRegistry, OutMessage, config,
};

pub async fn chat_webhook(
    State(state): State<AppState>,
    Path(transport_id): Path<String>,
    headers: HeaderMap,
    body: Bytes,
) -> impl IntoResponse {
    let registry = ChatTransportRegistry::new();
    if registry.get_type_info(&transport_id).is_none() {
        return (StatusCode::NOT_FOUND, "unknown transport").into_response();
    }

    let config = match config::load(&state.pool, &transport_id).await {
        Ok(config) => config,
        Err(e) => return chat_error_response(e),
    };
    let Some(transport) = registry.create(&transport_id, &config) else {
        return (StatusCode::NOT_FOUND, "unknown transport").into_response();
    };

    let inbound = match transport.verify(&headers, &body).await {
        Ok(Some(inbound)) => inbound,
        // Events we do not act on are not failures: ADDED_TO_SPACE, typing, our own bot.
        Ok(None) => return StatusCode::OK.into_response(),
        Err(e) => return chat_error_response(e),
    };

    match inbound::accept(&state, &transport_id, inbound).await {
        Ok(Accepted::Started { ack }) => reply(&*transport, ack),
        Ok(Accepted::Refused(msg)) => reply(&*transport, msg),
        Ok(Accepted::Ignore) => StatusCode::OK.into_response(),
        Err(e) => chat_error_response(e),
    }
}

/// Deliver the reply inside the webhook response when the transport can carry one,
/// so the ack costs no round trip and cannot miss the 30 s budget.
fn reply(transport: &dyn ChatTransport, msg: OutMessage) -> axum::response::Response {
    match transport.synchronous_reply(&msg) {
        Some(body) => (StatusCode::OK, axum::Json(body)).into_response(),
        None => (
            StatusCode::OK,
            axum::Json(json!({ "text": msg.plain_text() })),
        )
            .into_response(),
    }
}

fn chat_error_response(e: ChatError) -> axum::response::Response {
    match e {
        ChatError::Unauthorized(reason) => {
            tracing::warn!(reason = %reason, "rejected chat webhook");
            (StatusCode::UNAUTHORIZED, "unauthorized").into_response()
        }
        ChatError::BadPayload(reason) => {
            tracing::warn!(reason = %reason, "malformed chat webhook");
            (StatusCode::BAD_REQUEST, "bad request").into_response()
        }
        ChatError::NotConfigured(what) => {
            tracing::warn!(what = %what, "chat transport is not configured");
            (StatusCode::SERVICE_UNAVAILABLE, "transport not configured").into_response()
        }
        ChatError::Transport(reason) => {
            tracing::error!(reason = %reason, "chat transport failed");
            (StatusCode::BAD_GATEWAY, "transport error").into_response()
        }
    }
}
