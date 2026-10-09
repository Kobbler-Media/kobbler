//! The Command Palette's chat endpoints.
//!
//! Route, request shape and SSE frames are unchanged from the MeiliSearch proxy
//! this replaces; what changed is who runs the loop. Kobbler now calls the
//! configured LLM itself with its own tool registry, and Meili is back to being
//! a search engine — a chat workspace cannot be taught a `want_it` tool, which
//! is the whole reason for the move.

use axum::{
    Json,
    body::Body,
    extract::State,
    http::{HeaderMap, StatusCode, header},
    response::IntoResponse,
};
use serde::Serialize;
use serde_json::Value;
use tracing::warn;

use crate::AppState;
use crate::chatbot::{
    events,
    llm::ChatMessage,
    run::{self, Turn},
    store::{self, TRANSPORT_WEB},
    tools::read,
    transport::web,
};
use crate::routes::AuthUser;

#[derive(Serialize)]
pub struct ChatStatus {
    pub enabled: bool,
    pub configured: bool,
    pub provider: Option<String>,
    pub model: Option<String>,
}

pub async fn chat_status(State(state): State<AppState>, _auth: AuthUser) -> impl IntoResponse {
    let config = state.runtime_config.get().await;
    let has_koblem = config
        .koblem_service_url
        .as_deref()
        .is_some_and(|s| !s.is_empty());
    let has_external_provider = !config
        .chat_llm_api_key
        .as_deref()
        .unwrap_or_default()
        .is_empty()
        && config.chat_llm_provider.is_some();
    let configured = config.chat_enabled && (has_koblem || has_external_provider);

    Json(ChatStatus {
        enabled: config.chat_enabled,
        configured,
        provider: config.chat_llm_provider.clone(),
        model: config.chat_llm_model.clone(),
    })
    .into_response()
}

pub async fn chat_completions(
    State(state): State<AppState>,
    auth: AuthUser,
    Json(body): Json<Value>,
) -> impl IntoResponse {
    let config = state.runtime_config.get().await;
    if !config.chat_enabled {
        return (StatusCode::BAD_REQUEST, "Chat is not enabled").into_response();
    }
    let has_koblem = config
        .koblem_service_url
        .as_deref()
        .is_some_and(|s| !s.is_empty());
    if !has_koblem
        && config
            .chat_llm_api_key
            .as_deref()
            .unwrap_or_default()
            .is_empty()
    {
        return (
            StatusCode::BAD_REQUEST,
            "Chat LLM API key not configured (or configure koblem_service_url)",
        )
            .into_response();
    }

    let Some((user_message, user_turns)) = web::read_request(&body) else {
        return (StatusCode::BAD_REQUEST, "No user message to answer").into_response();
    };

    let session = match store::open_user_session(
        &state.pool,
        TRANSPORT_WEB,
        auth.user_id,
        user_turns,
    )
    .await
    {
        Ok(s) => s,
        Err(e) => {
            warn!("could not open a chat session: {e}");
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                "Could not start the chat",
            )
                .into_response();
        }
    };

    // History comes from the store rather than the request because the request
    // carries only role/content — the tool observations that make "no, the
    // other one" resolvable live on the stored steps.
    let history: Vec<ChatMessage> = match store::window(&state.pool, session.id, None).await {
        Ok(h) => h,
        Err(e) => {
            warn!("could not read the chat history: {e}");
            Vec::new()
        }
    };

    if let Err(e) = store::record_user_turn(&state.pool, session.id, &user_message).await {
        warn!("could not record the user turn: {e}");
    }
    let message_id = match store::start_assistant_turn(&state.pool, session.id).await {
        Ok(id) => id,
        Err(e) => {
            warn!("could not start the assistant turn: {e}");
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                "Could not start the chat",
            )
                .into_response();
        }
    };

    // The previous turn's resolution is what "no, the other one" points at.
    let context_ref = history
        .iter()
        .rev()
        .filter(|m| m.role == "tool")
        .find_map(|m| {
            serde_json::from_str::<Value>(&m.content)
                .ok()
                .as_ref()
                .and_then(read::context_ref_from)
        });

    let (tx, rx) = events::channel();
    let task_state = state.clone();
    // The turn owns its own task so a browser that navigates away does not
    // abort work already paid for in GPU time; the row keeps ticking and the
    // next request reads it back.
    tokio::spawn(async move {
        let turn = Turn {
            state: &task_state,
            auth: &auth,
            transport: TRANSPORT_WEB,
            session_id: session.id,
            message_id,
            history,
            user_message,
            context_ref,
        };
        let result = run::run(turn, tx).await;
        if let Err(e) = store::finish_assistant_turn(
            &task_state.pool,
            message_id,
            &result.text,
            &result.steps,
            result.status,
        )
        .await
        {
            warn!("could not finalise the assistant turn: {e}");
        }
    });

    let mut headers = HeaderMap::new();
    headers.insert(header::CONTENT_TYPE, "text/event-stream".parse().unwrap());
    headers.insert(header::CACHE_CONTROL, "no-cache".parse().unwrap());
    (headers, Body::from_stream(web::sse_stream(rx))).into_response()
}
