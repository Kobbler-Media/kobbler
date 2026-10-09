//! Admin viewer for `chat_turn_logs`.
//!
//! Admin-only, and admin does not mean "reads everyone's conversations": the turn's
//! own user sees what was said, every other admin sees only how the turn behaved.
//! `content_visible` below is the single place that decides, and the content columns
//! are fetched by a separate query so a non-owner's request never selects them.

use axum::{
    Json,
    extract::{Path, Query, State},
    http::StatusCode,
    response::IntoResponse,
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use time::{OffsetDateTime, format_description::well_known::Rfc3339};
use uuid::Uuid;

use crate::AppState;
use crate::repositories::chat_turn_logs::{
    self, ChatTurnLogContent, ChatTurnLogMetadata, ListFilters, TurnStage,
};
use crate::routes::AuthUser;

const DEFAULT_LIMIT: i64 = 50;
const MAX_LIMIT: i64 = 500;

pub enum ChatLogsError {
    BadRequest(String),
    Forbidden,
    NotFound,
    Internal(String),
}

impl IntoResponse for ChatLogsError {
    fn into_response(self) -> axum::response::Response {
        let (status, message) = match self {
            ChatLogsError::BadRequest(msg) => (StatusCode::BAD_REQUEST, msg),
            ChatLogsError::Forbidden => (StatusCode::FORBIDDEN, "forbidden".to_string()),
            ChatLogsError::NotFound => (StatusCode::NOT_FOUND, "turn log not found".to_string()),
            ChatLogsError::Internal(msg) => (StatusCode::INTERNAL_SERVER_ERROR, msg),
        };
        (status, Json(json!({ "error": message }))).into_response()
    }
}

#[derive(Debug, Deserialize)]
pub struct ChatLogsQuery {
    pub limit: Option<i64>,
    pub offset: Option<i64>,
    pub transport: Option<String>,
    /// `ok` or `failed`.
    pub status: Option<String>,
    /// `extract`, `resolve` or `action` — turns where that stage recorded an error.
    pub stage: Option<String>,
    pub user_id: Option<Uuid>,
    pub session_id: Option<Uuid>,
    pub since: Option<String>,
    pub until: Option<String>,
    /// Free text over the user message. Only matches the caller's own turns.
    pub q: Option<String>,
}

/// Everything about how the turn behaved. Safe for any admin to read.
#[derive(Debug, Serialize)]
pub struct ChatTurnLogEntry {
    pub id: Uuid,
    pub session_id: Option<Uuid>,
    pub user_id: Option<Uuid>,
    pub transport: String,
    pub succeeded: bool,
    pub failed_stage: Option<String>,
    pub turn_error: Option<String>,
    pub extract_ms: Option<i32>,
    pub extract_error: Option<String>,
    /// Which slots the extraction filled — key names and JSON types, never values.
    pub extract_shape: Option<Value>,
    pub resolve_ms: Option<i32>,
    pub resolve_candidate_count: Option<i32>,
    pub resolve_error: Option<String>,
    pub action_ms: Option<i32>,
    pub action_name: Option<String>,
    pub action_error: Option<String>,
    pub llm_provider: Option<String>,
    pub llm_model: Option<String>,
    pub llm_ms: Option<i32>,
    pub prompt_tokens: Option<i32>,
    pub completion_tokens: Option<i32>,
    pub rounds: i32,
    pub tool_call_count: i32,
    pub tool_names: Value,
    pub total_ms: Option<i32>,
    #[serde(with = "time::serde::rfc3339")]
    pub created_at: OffsetDateTime,
    /// What was actually said. Present only for the turn's own user.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub content: Option<ChatTurnLogContentBody>,
}

#[derive(Debug, Serialize)]
pub struct ChatTurnLogContentBody {
    pub user_message: Option<String>,
    pub assistant_text: Option<String>,
    pub extract_output: Option<Value>,
    pub resolve_query: Option<String>,
    pub resolved_ref: Option<String>,
    pub resolve_candidates: Option<Value>,
    pub action_args: Option<Value>,
    pub action_result: Option<Value>,
    pub tool_calls: Value,
}

#[derive(Debug, Serialize)]
pub struct ChatLogsResponse {
    pub logs: Vec<ChatTurnLogEntry>,
    pub total: i64,
}

/// The privacy split, in one predicate: conversation content belongs to the person
/// who had the conversation. Widening this widens it everywhere.
fn content_visible(auth: &AuthUser, row_user_id: Option<Uuid>) -> bool {
    row_user_id == Some(auth.user_id)
}

fn failed_stage(m: &ChatTurnLogMetadata) -> Option<String> {
    if m.action_error.is_some() {
        Some(TurnStage::Action.as_str().to_string())
    } else if m.resolve_error.is_some() {
        Some(TurnStage::Resolve.as_str().to_string())
    } else if m.extract_error.is_some() {
        Some(TurnStage::Extract.as_str().to_string())
    } else {
        None
    }
}

fn to_entry(m: ChatTurnLogMetadata, content: Option<ChatTurnLogContent>) -> ChatTurnLogEntry {
    ChatTurnLogEntry {
        failed_stage: failed_stage(&m),
        id: m.id,
        session_id: m.session_id,
        user_id: m.user_id,
        transport: m.transport,
        succeeded: m.succeeded,
        turn_error: m.turn_error,
        extract_ms: m.extract_ms,
        extract_error: m.extract_error,
        extract_shape: m.extract_shape,
        resolve_ms: m.resolve_ms,
        resolve_candidate_count: m.resolve_candidate_count,
        resolve_error: m.resolve_error,
        action_ms: m.action_ms,
        action_name: m.action_name,
        action_error: m.action_error,
        llm_provider: m.llm_provider,
        llm_model: m.llm_model,
        llm_ms: m.llm_ms,
        prompt_tokens: m.prompt_tokens,
        completion_tokens: m.completion_tokens,
        rounds: m.rounds,
        tool_call_count: m.tool_call_count,
        tool_names: m.tool_names,
        total_ms: m.total_ms,
        created_at: m.created_at,
        content: content.map(|c| ChatTurnLogContentBody {
            user_message: c.user_message,
            assistant_text: c.assistant_text,
            extract_output: c.extract_output,
            resolve_query: c.resolve_query,
            resolved_ref: c.resolved_ref,
            resolve_candidates: c.resolve_candidates,
            action_args: c.action_args,
            action_result: c.action_result,
            tool_calls: c.tool_calls,
        }),
    }
}

fn parse_time(field: &str, raw: &Option<String>) -> Result<Option<OffsetDateTime>, ChatLogsError> {
    match raw {
        None => Ok(None),
        Some(s) if s.is_empty() => Ok(None),
        Some(s) => OffsetDateTime::parse(s, &Rfc3339).map(Some).map_err(|_| {
            ChatLogsError::BadRequest(format!("{field} must be an RFC 3339 timestamp"))
        }),
    }
}

/// GET /api/v1/admin/chat/logs
pub async fn list_chat_logs(
    State(state): State<AppState>,
    auth: AuthUser,
    Query(query): Query<ChatLogsQuery>,
) -> Result<Json<ChatLogsResponse>, ChatLogsError> {
    auth.require_admin().map_err(|_| ChatLogsError::Forbidden)?;

    let stage = match &query.stage {
        None => None,
        Some(s) if s.is_empty() => None,
        Some(s) => Some(TurnStage::parse(s).ok_or_else(|| {
            ChatLogsError::BadRequest("stage must be extract, resolve or action".to_string())
        })?),
    };
    let succeeded = match query.status.as_deref() {
        None | Some("") | Some("all") => None,
        Some("ok") => Some(true),
        Some("failed") => Some(false),
        Some(_) => {
            return Err(ChatLogsError::BadRequest(
                "status must be ok or failed".to_string(),
            ));
        }
    };

    let filters = ListFilters {
        transport: query.transport.filter(|t| !t.is_empty()),
        succeeded,
        stage,
        user_id: query.user_id,
        session_id: query.session_id,
        since: parse_time("since", &query.since)?,
        until: parse_time("until", &query.until)?,
        query: query.q.filter(|q| !q.is_empty()),
        limit: query.limit.unwrap_or(DEFAULT_LIMIT).clamp(1, MAX_LIMIT),
        offset: query.offset.unwrap_or(0).max(0),
    };

    let (rows, total) = chat_turn_logs::list_metadata(&state.pool, &filters, auth.user_id)
        .await
        .map_err(|e| ChatLogsError::Internal(format!("Chat turn log query failed: {e}")))?;

    let owned: Vec<Uuid> = rows
        .iter()
        .filter(|m| content_visible(&auth, m.user_id))
        .map(|m| m.id)
        .collect();
    let mut content = chat_turn_logs::list_content(&state.pool, &owned, auth.user_id)
        .await
        .map_err(|e| ChatLogsError::Internal(format!("Chat turn log query failed: {e}")))?;

    let logs = rows
        .into_iter()
        .map(|m| {
            let c = content
                .iter()
                .position(|c| c.id == m.id)
                .map(|i| content.swap_remove(i));
            to_entry(m, c)
        })
        .collect();

    Ok(Json(ChatLogsResponse { logs, total }))
}

/// GET /api/v1/admin/chat/logs/{id}
pub async fn get_chat_log(
    State(state): State<AppState>,
    auth: AuthUser,
    Path(id): Path<Uuid>,
) -> Result<Json<ChatTurnLogEntry>, ChatLogsError> {
    auth.require_admin().map_err(|_| ChatLogsError::Forbidden)?;

    let metadata = chat_turn_logs::get_metadata(&state.pool, id)
        .await
        .map_err(|e| ChatLogsError::Internal(format!("Chat turn log query failed: {e}")))?
        .ok_or(ChatLogsError::NotFound)?;

    let content = if content_visible(&auth, metadata.user_id) {
        chat_turn_logs::get_content(&state.pool, id, auth.user_id)
            .await
            .map_err(|e| ChatLogsError::Internal(format!("Chat turn log query failed: {e}")))?
    } else {
        None
    };

    Ok(Json(to_entry(metadata, content)))
}
