//! Chat turn log repository.
//!
//! Content columns (what was said) and metadata columns (how the turn behaved) are
//! read by separate functions on purpose: a caller who does not own a row must never
//! have its content selected, so the split is enforced here rather than in the
//! response types or the UI.

use super::Result;
use serde_json::Value;
use sqlx::{Executor, PgPool, Postgres, QueryBuilder, Row};
use time::OffsetDateTime;
use uuid::Uuid;

pub const METADATA_COLUMNS: &str = r#"id, session_id, user_id, transport, succeeded, turn_error,
        extract_ms, extract_error, extract_shape,
        resolve_ms, resolve_candidate_count, resolve_error,
        action_ms, action_name, action_error,
        llm_provider, llm_model, llm_ms, prompt_tokens, completion_tokens, rounds,
        tool_call_count, total_ms, created_at,
        (SELECT COALESCE(jsonb_agg(t->>'name'), '[]'::jsonb)
           FROM jsonb_array_elements(tool_calls) t) AS tool_names"#;

#[derive(Debug, Default)]
pub struct ChatTurnLogInsert {
    pub id: Uuid,
    pub session_id: Option<Uuid>,
    pub user_id: Option<Uuid>,
    pub transport: String,
    pub user_message: Option<String>,
    pub succeeded: bool,
    pub turn_error: Option<String>,
    pub extract_ms: Option<i32>,
    pub extract_output: Option<Value>,
    pub extract_shape: Option<Value>,
    pub extract_error: Option<String>,
    pub resolve_ms: Option<i32>,
    pub resolve_query: Option<String>,
    pub resolved_ref: Option<String>,
    pub resolve_candidate_count: Option<i32>,
    pub resolve_candidates: Option<Value>,
    pub resolve_error: Option<String>,
    pub action_ms: Option<i32>,
    pub action_name: Option<String>,
    pub action_args: Option<Value>,
    pub action_result: Option<Value>,
    pub action_error: Option<String>,
    pub llm_provider: Option<String>,
    pub llm_model: Option<String>,
    pub llm_ms: Option<i32>,
    pub prompt_tokens: Option<i32>,
    pub completion_tokens: Option<i32>,
    pub rounds: i32,
    pub tool_calls: Value,
    pub tool_call_count: i32,
    pub assistant_text: Option<String>,
    pub total_ms: Option<i32>,
}

#[derive(Debug)]
pub struct ChatTurnLogMetadata {
    pub id: Uuid,
    pub session_id: Option<Uuid>,
    pub user_id: Option<Uuid>,
    pub transport: String,
    pub succeeded: bool,
    pub turn_error: Option<String>,
    pub extract_ms: Option<i32>,
    pub extract_error: Option<String>,
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
    pub created_at: OffsetDateTime,
}

#[derive(Debug)]
pub struct ChatTurnLogContent {
    pub id: Uuid,
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

/// Which stage failed. Maps onto the three separate error columns.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TurnStage {
    Extract,
    Resolve,
    Action,
}

impl TurnStage {
    pub fn as_str(self) -> &'static str {
        match self {
            TurnStage::Extract => "extract",
            TurnStage::Resolve => "resolve",
            TurnStage::Action => "action",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "extract" => Some(TurnStage::Extract),
            "resolve" => Some(TurnStage::Resolve),
            "action" => Some(TurnStage::Action),
            _ => None,
        }
    }

    fn error_column(self) -> &'static str {
        match self {
            TurnStage::Extract => "extract_error",
            TurnStage::Resolve => "resolve_error",
            TurnStage::Action => "action_error",
        }
    }
}

#[derive(Debug, Default)]
pub struct ListFilters {
    pub transport: Option<String>,
    pub succeeded: Option<bool>,
    pub stage: Option<TurnStage>,
    pub user_id: Option<Uuid>,
    pub session_id: Option<Uuid>,
    pub since: Option<OffsetDateTime>,
    pub until: Option<OffsetDateTime>,
    /// Free-text over the user message. Only ever matched against rows owned by
    /// `viewer_id`, because matching it against anyone else's row would leak the
    /// content the split above exists to withhold.
    pub query: Option<String>,
    pub limit: i64,
    pub offset: i64,
}

pub async fn insert<'e, E>(exec: E, input: &ChatTurnLogInsert) -> Result<()>
where
    E: Executor<'e, Database = Postgres>,
{
    sqlx::query!(
        r#"
        INSERT INTO chat_turn_logs (
            id, session_id, user_id, transport, user_message,
            succeeded, turn_error,
            extract_ms, extract_output, extract_shape, extract_error,
            resolve_ms, resolve_query, resolved_ref, resolve_candidate_count,
            resolve_candidates, resolve_error,
            action_ms, action_name, action_args, action_result, action_error,
            llm_provider, llm_model, llm_ms, prompt_tokens, completion_tokens, rounds,
            tool_calls, tool_call_count, assistant_text, total_ms
        ) VALUES (
            $1, $2, $3, $4, $5,
            $6, $7,
            $8, $9, $10, $11,
            $12, $13, $14, $15,
            $16, $17,
            $18, $19, $20, $21, $22,
            $23, $24, $25, $26, $27, $28,
            $29, $30, $31, $32
        )
        "#,
        input.id,
        input.session_id,
        input.user_id,
        input.transport,
        input.user_message,
        input.succeeded,
        input.turn_error,
        input.extract_ms,
        input.extract_output,
        input.extract_shape,
        input.extract_error,
        input.resolve_ms,
        input.resolve_query,
        input.resolved_ref,
        input.resolve_candidate_count,
        input.resolve_candidates,
        input.resolve_error,
        input.action_ms,
        input.action_name,
        input.action_args,
        input.action_result,
        input.action_error,
        input.llm_provider,
        input.llm_model,
        input.llm_ms,
        input.prompt_tokens,
        input.completion_tokens,
        input.rounds,
        input.tool_calls,
        input.tool_call_count,
        input.assistant_text,
        input.total_ms,
    )
    .execute(exec)
    .await?;
    Ok(())
}

fn push_filters(qb: &mut QueryBuilder<Postgres>, f: &ListFilters, viewer_id: Uuid) {
    if let Some(transport) = &f.transport {
        qb.push(" AND transport = ").push_bind(transport.clone());
    }
    if let Some(succeeded) = f.succeeded {
        qb.push(" AND succeeded = ").push_bind(succeeded);
    }
    if let Some(stage) = f.stage {
        qb.push(" AND ")
            .push(stage.error_column())
            .push(" IS NOT NULL");
    }
    if let Some(user_id) = f.user_id {
        qb.push(" AND user_id = ").push_bind(user_id);
    }
    if let Some(session_id) = f.session_id {
        qb.push(" AND session_id = ").push_bind(session_id);
    }
    if let Some(since) = f.since {
        qb.push(" AND created_at >= ").push_bind(since);
    }
    if let Some(until) = f.until {
        qb.push(" AND created_at <= ").push_bind(until);
    }
    if let Some(query) = &f.query {
        qb.push(" AND user_id = ")
            .push_bind(viewer_id)
            .push(" AND user_message ILIKE ")
            .push_bind(format!("%{}%", query));
    }
}

fn row_to_metadata(row: &sqlx::postgres::PgRow) -> ChatTurnLogMetadata {
    ChatTurnLogMetadata {
        id: row.get("id"),
        session_id: row.get("session_id"),
        user_id: row.get("user_id"),
        transport: row.get("transport"),
        succeeded: row.get("succeeded"),
        turn_error: row.get("turn_error"),
        extract_ms: row.get("extract_ms"),
        extract_error: row.get("extract_error"),
        extract_shape: row.get("extract_shape"),
        resolve_ms: row.get("resolve_ms"),
        resolve_candidate_count: row.get("resolve_candidate_count"),
        resolve_error: row.get("resolve_error"),
        action_ms: row.get("action_ms"),
        action_name: row.get("action_name"),
        action_error: row.get("action_error"),
        llm_provider: row.get("llm_provider"),
        llm_model: row.get("llm_model"),
        llm_ms: row.get("llm_ms"),
        prompt_tokens: row.get("prompt_tokens"),
        completion_tokens: row.get("completion_tokens"),
        rounds: row.get("rounds"),
        tool_call_count: row.get("tool_call_count"),
        tool_names: row.get("tool_names"),
        total_ms: row.get("total_ms"),
        created_at: row.get("created_at"),
    }
}

/// Metadata for one page of turns, plus the total matching the same filters.
pub async fn list_metadata(
    pool: &PgPool,
    filters: &ListFilters,
    viewer_id: Uuid,
) -> Result<(Vec<ChatTurnLogMetadata>, i64)> {
    let mut data: QueryBuilder<Postgres> = QueryBuilder::new(format!(
        "SELECT {METADATA_COLUMNS} FROM chat_turn_logs WHERE 1=1"
    ));
    push_filters(&mut data, filters, viewer_id);
    data.push(" ORDER BY created_at DESC, id DESC LIMIT ")
        .push_bind(filters.limit)
        .push(" OFFSET ")
        .push_bind(filters.offset);

    let mut count: QueryBuilder<Postgres> =
        QueryBuilder::new("SELECT COUNT(*) AS count FROM chat_turn_logs WHERE 1=1");
    push_filters(&mut count, filters, viewer_id);

    let total: i64 = count.build().fetch_one(pool).await?.get("count");
    let rows = data.build().fetch_all(pool).await?;

    Ok((rows.iter().map(row_to_metadata).collect(), total))
}

/// Content for the subset of `ids` owned by `owner_id`. Rows owned by anyone else
/// are excluded by the query, so their content is never read.
pub async fn list_content(
    pool: &PgPool,
    ids: &[Uuid],
    owner_id: Uuid,
) -> Result<Vec<ChatTurnLogContent>> {
    if ids.is_empty() {
        return Ok(Vec::new());
    }
    let rows = sqlx::query_as!(
        ChatTurnLogContent,
        r#"
        SELECT id, user_message, assistant_text, extract_output, resolve_query,
               resolved_ref, resolve_candidates, action_args, action_result,
               tool_calls as "tool_calls!"
        FROM chat_turn_logs
        WHERE id = ANY($1) AND user_id = $2
        "#,
        ids,
        owner_id,
    )
    .fetch_all(pool)
    .await?;
    Ok(rows)
}

pub async fn get_metadata(pool: &PgPool, id: Uuid) -> Result<Option<ChatTurnLogMetadata>> {
    let mut qb: QueryBuilder<Postgres> = QueryBuilder::new(format!(
        "SELECT {METADATA_COLUMNS} FROM chat_turn_logs WHERE id = "
    ));
    qb.push_bind(id);
    let row = qb.build().fetch_optional(pool).await?;
    Ok(row.as_ref().map(row_to_metadata))
}

/// Content for one turn, only if `owner_id` owns it.
pub async fn get_content(
    pool: &PgPool,
    id: Uuid,
    owner_id: Uuid,
) -> Result<Option<ChatTurnLogContent>> {
    let row = sqlx::query_as!(
        ChatTurnLogContent,
        r#"
        SELECT id, user_message, assistant_text, extract_output, resolve_query,
               resolved_ref, resolve_candidates, action_args, action_result,
               tool_calls as "tool_calls!"
        FROM chat_turn_logs
        WHERE id = $1 AND user_id = $2
        "#,
        id,
        owner_id,
    )
    .fetch_optional(pool)
    .await?;
    Ok(row)
}

pub async fn delete_older_than_days(pool: &PgPool, days: i32) -> Result<u64> {
    let deleted = sqlx::query!(
        "DELETE FROM chat_turn_logs WHERE created_at < NOW() - make_interval(days => $1)",
        days,
    )
    .execute(pool)
    .await?
    .rows_affected();
    Ok(deleted)
}
