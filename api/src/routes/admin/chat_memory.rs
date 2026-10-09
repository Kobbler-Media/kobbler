//! What the bot remembers about you, and the controls for correcting it.
//!
//! The privacy decision from the design, enforced the way Agent D's
//! `chat_logs.rs` enforces it: conversation content and profile facts are
//! visible to **that person only**, and admins see metadata — counts and
//! timestamps. The split lives in the queries, not in the response types and
//! certainly not in Svelte: `chat_profile_facts::list_for_user` and
//! `chat_memory::summaries_for_user` both take the owner's id, and
//! `chat_memory::user_metadata` selects no prose at all, so a non-owner's
//! request never reads the content in the first place.
//!
//! "Forget everything" is deliberately the owner's own action. These are a
//! person's conversations; wiping them should not need an admin.

use axum::{
    Json,
    extract::{Path, State},
    http::StatusCode,
    response::IntoResponse,
};
use serde::{Deserialize, Serialize};
use serde_json::json;
use time::OffsetDateTime;
use uuid::Uuid;

use crate::AppState;
use crate::repositories::chat_memory::{self, SessionSummary, UserChatMetadata};
use crate::repositories::chat_profile_facts::{self, MAX_FACTS_PER_USER, ProfileFact, valid_kind};
use crate::routes::AuthUser;

const MAX_VALUE_LEN: usize = 200;
const SUMMARY_PAGE: i64 = 50;

pub enum ChatMemoryError {
    BadRequest(String),
    Forbidden,
    Conflict(String),
    NotFound,
    Internal(String),
}

impl IntoResponse for ChatMemoryError {
    fn into_response(self) -> axum::response::Response {
        let (status, message) = match self {
            ChatMemoryError::BadRequest(msg) => (StatusCode::BAD_REQUEST, msg),
            ChatMemoryError::Forbidden => (StatusCode::FORBIDDEN, "forbidden".to_string()),
            ChatMemoryError::Conflict(msg) => (StatusCode::CONFLICT, msg),
            ChatMemoryError::NotFound => (StatusCode::NOT_FOUND, "not found".to_string()),
            ChatMemoryError::Internal(msg) => (StatusCode::INTERNAL_SERVER_ERROR, msg),
        };
        (status, Json(json!({ "error": message }))).into_response()
    }
}

fn internal(what: &str) -> impl Fn(crate::repositories::RepositoryError) -> ChatMemoryError + '_ {
    move |e| ChatMemoryError::Internal(format!("{what}: {e}"))
}

// =============================================================================
// Response types
// =============================================================================

#[derive(Debug, Serialize)]
pub struct ProfileFactBody {
    pub id: Uuid,
    pub kind: String,
    pub value: String,
    pub source_id: Option<Uuid>,
    #[serde(with = "time::serde::rfc3339")]
    pub created_at: OffsetDateTime,
}

impl From<ProfileFact> for ProfileFactBody {
    fn from(f: ProfileFact) -> Self {
        Self {
            id: f.id,
            kind: f.kind,
            value: f.value,
            source_id: f.source_id,
            created_at: f.created_at,
        }
    }
}

#[derive(Debug, Serialize)]
pub struct SessionSummaryBody {
    pub id: Uuid,
    pub transport: String,
    pub summary: String,
    #[serde(with = "time::serde::rfc3339")]
    pub created_at: OffsetDateTime,
    #[serde(with = "time::serde::rfc3339::option")]
    pub closed_at: Option<OffsetDateTime>,
}

impl From<SessionSummary> for SessionSummaryBody {
    fn from(s: SessionSummary) -> Self {
        Self {
            id: s.id,
            transport: s.transport,
            summary: s.summary,
            created_at: s.created_at,
            closed_at: s.closed_at,
        }
    }
}

#[derive(Debug, Serialize)]
pub struct ChatMemoryResponse {
    pub facts: Vec<ProfileFactBody>,
    pub summaries: Vec<SessionSummaryBody>,
    pub max_facts: i64,
    pub kinds: Vec<String>,
}

/// The admin view. Counts and timestamps, never prose — and `content` is only
/// ever attached for the admin's own row.
#[derive(Debug, Serialize)]
pub struct UserMemoryMetadata {
    pub user_id: Uuid,
    pub email: String,
    pub display_name: Option<String>,
    pub session_count: i64,
    pub message_count: i64,
    pub summary_count: i64,
    pub fact_count: i64,
    #[serde(with = "time::serde::rfc3339::option")]
    pub last_seen: Option<OffsetDateTime>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub content: Option<ChatMemoryResponse>,
}

#[derive(Debug, Deserialize)]
pub struct FactInput {
    pub kind: String,
    pub value: String,
}

// =============================================================================
// Handlers
// =============================================================================

/// The one predicate. Widening it widens it everywhere.
fn content_visible(auth: &AuthUser, row_user_id: Uuid) -> bool {
    row_user_id == auth.user_id
}

async fn own_memory(
    state: &AppState,
    owner_id: Uuid,
) -> Result<ChatMemoryResponse, ChatMemoryError> {
    let facts = chat_profile_facts::list_for_user(&state.pool, owner_id)
        .await
        .map_err(internal("Profile fact query failed"))?;
    let summaries = chat_memory::summaries_for_user(&state.pool, owner_id, SUMMARY_PAGE)
        .await
        .map_err(internal("Session summary query failed"))?;

    Ok(ChatMemoryResponse {
        facts: facts.into_iter().map(ProfileFactBody::from).collect(),
        summaries: summaries
            .into_iter()
            .map(SessionSummaryBody::from)
            .collect(),
        max_facts: MAX_FACTS_PER_USER,
        kinds: chat_profile_facts::KINDS
            .iter()
            .map(|k| (*k).to_string())
            .collect(),
    })
}

/// GET /api/v1/admin/chat/memory
pub async fn get_chat_memory(
    State(state): State<AppState>,
    auth: AuthUser,
) -> Result<Json<ChatMemoryResponse>, ChatMemoryError> {
    Ok(Json(own_memory(&state, auth.user_id).await?))
}

/// GET /api/v1/admin/chat/memory/users
///
/// Admin-only, and admin does not mean "reads everyone's profile".
pub async fn list_chat_memory_users(
    State(state): State<AppState>,
    auth: AuthUser,
) -> Result<Json<Vec<UserMemoryMetadata>>, ChatMemoryError> {
    auth.require_admin()
        .map_err(|_| ChatMemoryError::Forbidden)?;

    let rows = chat_memory::user_metadata(&state.pool)
        .await
        .map_err(internal("Chat memory metadata query failed"))?;

    let mut out = Vec::with_capacity(rows.len());
    for row in rows {
        let content = match content_visible(&auth, row.user_id) {
            true => Some(own_memory(&state, row.user_id).await?),
            false => None,
        };
        out.push(to_metadata(row, content));
    }
    Ok(Json(out))
}

fn to_metadata(row: UserChatMetadata, content: Option<ChatMemoryResponse>) -> UserMemoryMetadata {
    UserMemoryMetadata {
        user_id: row.user_id,
        email: row.email,
        display_name: row.display_name,
        session_count: row.session_count,
        message_count: row.message_count,
        summary_count: row.summary_count,
        fact_count: row.fact_count,
        last_seen: row.last_seen,
        content,
    }
}

fn validate(input: &FactInput) -> Result<(String, String), ChatMemoryError> {
    let kind = input.kind.trim().to_lowercase();
    if !valid_kind(&kind) {
        return Err(ChatMemoryError::BadRequest(
            "kind must be prefers, likes or avoids".to_string(),
        ));
    }
    let value = input.value.trim();
    if value.is_empty() {
        return Err(ChatMemoryError::BadRequest(
            "value must not be empty".to_string(),
        ));
    }
    if value.chars().count() > MAX_VALUE_LEN {
        return Err(ChatMemoryError::BadRequest(format!(
            "value must be {MAX_VALUE_LEN} characters or fewer"
        )));
    }
    Ok((kind, value.to_string()))
}

/// POST /api/v1/admin/chat/memory/facts
///
/// At the cap this refuses rather than evicting: someone adding a fact by hand
/// should not silently lose another one.
pub async fn create_chat_fact(
    State(state): State<AppState>,
    auth: AuthUser,
    Json(input): Json<FactInput>,
) -> Result<(StatusCode, Json<ProfileFactBody>), ChatMemoryError> {
    let (kind, value) = validate(&input)?;

    let count = chat_profile_facts::count_for_user(&state.pool, auth.user_id)
        .await
        .map_err(internal("Profile fact query failed"))?;
    if count >= MAX_FACTS_PER_USER {
        return Err(ChatMemoryError::Conflict(format!(
            "you already have {MAX_FACTS_PER_USER} facts — remove one first"
        )));
    }

    let fact = chat_profile_facts::insert(&state.pool, auth.user_id, &kind, &value, None)
        .await
        .map_err(internal("Profile fact insert failed"))?
        .ok_or_else(|| ChatMemoryError::Conflict("you already know that one".to_string()))?;

    Ok((StatusCode::CREATED, Json(ProfileFactBody::from(fact))))
}

/// PATCH /api/v1/admin/chat/memory/facts/{fact_id}
pub async fn update_chat_fact(
    State(state): State<AppState>,
    auth: AuthUser,
    Path(fact_id): Path<Uuid>,
    Json(input): Json<FactInput>,
) -> Result<Json<ProfileFactBody>, ChatMemoryError> {
    let (kind, value) = validate(&input)?;

    let fact = chat_profile_facts::update(&state.pool, fact_id, auth.user_id, &kind, &value)
        .await
        .map_err(internal("Profile fact update failed"))?
        .ok_or(ChatMemoryError::NotFound)?;

    Ok(Json(ProfileFactBody::from(fact)))
}

/// DELETE /api/v1/admin/chat/memory/facts/{fact_id}
pub async fn delete_chat_fact(
    State(state): State<AppState>,
    auth: AuthUser,
    Path(fact_id): Path<Uuid>,
) -> Result<StatusCode, ChatMemoryError> {
    let deleted = chat_profile_facts::delete(&state.pool, fact_id, auth.user_id)
        .await
        .map_err(internal("Profile fact delete failed"))?;

    match deleted {
        true => Ok(StatusCode::NO_CONTENT),
        false => Err(ChatMemoryError::NotFound),
    }
}

/// DELETE /api/v1/admin/chat/memory — forget everything, all three tiers.
pub async fn forget_chat_memory(
    State(state): State<AppState>,
    auth: AuthUser,
) -> Result<Json<serde_json::Value>, ChatMemoryError> {
    let forgotten = chat_memory::forget_everything(&state.pool, auth.user_id)
        .await
        .map_err(internal("Forget everything failed"))?;

    Ok(Json(json!({
        "sessions": forgotten.sessions,
        "messages": forgotten.messages,
        "facts": forgotten.facts,
        "turn_logs": forgotten.turn_logs,
    })))
}
