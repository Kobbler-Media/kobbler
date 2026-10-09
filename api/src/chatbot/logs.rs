//! One `chat_turn_logs` row per turn, success or failure.
//!
//! The three stages get their own recorder each so a failure can only ever land in
//! its own column — `extract_error`, `resolve_error` and `action_error` have
//! completely different fixes and the viewer distinguishes them.

use std::fmt::Display;
use std::time::{Duration, Instant};

use serde::Serialize;
use serde_json::{Value, json};
use sqlx::PgPool;
use uuid::Uuid;

use crate::repositories::chat_turn_logs::{self, ChatTurnLogInsert};

pub const TRANSPORT_WEB: &str = "web";
pub const TRANSPORT_GOOGLE_CHAT: &str = "google_chat";

const MAX_TEXT: usize = 8_000;

pub struct TurnLog {
    started: Instant,
    row: ChatTurnLogInsert,
    tool_calls: Vec<Value>,
}

impl TurnLog {
    pub fn open(
        transport: &str,
        session_id: Option<Uuid>,
        user_id: Option<Uuid>,
        user_message: &str,
    ) -> Self {
        Self {
            started: Instant::now(),
            row: ChatTurnLogInsert {
                id: Uuid::now_v7(),
                session_id,
                user_id,
                transport: transport.to_string(),
                user_message: Some(truncate(user_message)),
                tool_calls: json!([]),
                ..Default::default()
            },
            tool_calls: Vec::new(),
        }
    }

    pub fn id(&self) -> Uuid {
        self.row.id
    }

    pub fn llm(&mut self, provider: &str, model: &str) -> &mut Self {
        self.row.llm_provider = Some(provider.to_string());
        self.row.llm_model = Some(model.to_string());
        self
    }

    pub fn llm_round(
        &mut self,
        elapsed: Duration,
        prompt_tokens: Option<i32>,
        completion_tokens: Option<i32>,
    ) -> &mut Self {
        self.row.rounds += 1;
        self.row.llm_ms = Some(self.row.llm_ms.unwrap_or(0) + as_ms(elapsed));
        if let Some(p) = prompt_tokens {
            self.row.prompt_tokens = Some(self.row.prompt_tokens.unwrap_or(0) + p);
        }
        if let Some(c) = completion_tokens {
            self.row.completion_tokens = Some(self.row.completion_tokens.unwrap_or(0) + c);
        }
        self
    }

    /// Intent extraction: what the model pulled out of the message.
    pub fn extract<T: Serialize, E: Display>(
        &mut self,
        started: Instant,
        outcome: Result<&T, &E>,
    ) -> &mut Self {
        self.row.extract_ms = Some(elapsed_ms(started));
        match outcome {
            Ok(output) => {
                let value = to_value(output);
                self.row.extract_shape = value.as_ref().map(slot_shape);
                self.row.extract_output = value;
            }
            Err(e) => self.row.extract_error = Some(truncate(&e.to_string())),
        }
        self
    }

    /// `resolve_media`: the query it ran and the candidates it came back with.
    pub fn resolve<T: Serialize, E: Display>(
        &mut self,
        started: Instant,
        query: &str,
        resolved_ref: Option<&str>,
        outcome: Result<&[T], &E>,
    ) -> &mut Self {
        self.row.resolve_ms = Some(elapsed_ms(started));
        self.row.resolve_query = Some(truncate(query));
        self.row.resolved_ref = resolved_ref.map(|r| r.to_string());
        match outcome {
            Ok(candidates) => {
                self.row.resolve_candidate_count = Some(candidates.len() as i32);
                self.row.resolve_candidates = to_value(&candidates);
            }
            Err(e) => self.row.resolve_error = Some(truncate(&e.to_string())),
        }
        self
    }

    /// The write the turn performed, if any. Also recorded as a tool call.
    pub fn action<A: Serialize, T: Serialize, E: Display>(
        &mut self,
        started: Instant,
        name: &str,
        args: &A,
        outcome: Result<&T, &E>,
    ) -> &mut Self {
        self.row.action_ms = Some(elapsed_ms(started));
        self.row.action_name = Some(name.to_string());
        self.row.action_args = to_value(args);
        match outcome {
            Ok(result) => self.row.action_result = to_value(result),
            Err(e) => self.row.action_error = Some(truncate(&e.to_string())),
        }
        self.tool(started, name, args, outcome)
    }

    pub fn tool<A: Serialize, T: Serialize, E: Display>(
        &mut self,
        started: Instant,
        name: &str,
        args: &A,
        outcome: Result<&T, &E>,
    ) -> &mut Self {
        let (result, error) = match outcome {
            Ok(v) => (to_value(v), None),
            Err(e) => (None, Some(truncate(&e.to_string()))),
        };
        self.tool_calls.push(json!({
            "name": name,
            "arguments": to_value(args),
            "result": result,
            "error": error,
            "duration_ms": elapsed_ms(started),
        }));
        self
    }

    /// The turn answered. Writes the row.
    pub async fn finish(mut self, pool: &PgPool, assistant_text: &str) {
        self.row.succeeded = true;
        self.row.assistant_text = Some(truncate(assistant_text));
        self.write(pool).await;
    }

    /// The turn did not answer. Writes the row anyway — a failed turn is the row
    /// worth having. `error` lands in `turn_error`; a stage that failed has already
    /// recorded itself in its own column.
    pub async fn fail<E: Display>(mut self, pool: &PgPool, error: &E) {
        self.row.turn_error = Some(truncate(&error.to_string()));
        self.write(pool).await;
    }

    /// Logging never fails a turn, so a write error is logged and dropped.
    async fn write(&mut self, pool: &PgPool) {
        self.row.total_ms = Some(elapsed_ms(self.started));
        self.row.tool_call_count = self.tool_calls.len() as i32;
        self.row.tool_calls = Value::Array(std::mem::take(&mut self.tool_calls));

        if let Err(e) = chat_turn_logs::insert(pool, &self.row).await {
            tracing::warn!(error = %e, turn_id = %self.row.id, "Failed to write chat turn log");
        }
    }
}

fn to_value<T: Serialize>(v: &T) -> Option<Value> {
    serde_json::to_value(v).ok()
}

/// Which slots the extraction filled, as `{"season": null, "query": "string"}` —
/// key names and JSON types only, never values, because this is the one part of
/// the extraction a non-owner admin may read.
///
/// Safe only because the keys come from a serde struct whose field names are our
/// own tool schema. Passing a map keyed by user input would put message content
/// in a metadata column.
fn slot_shape(value: &Value) -> Value {
    match value {
        Value::Object(map) => {
            Value::Object(map.iter().map(|(k, v)| (k.clone(), json_type(v))).collect())
        }
        other => json!({ "_type": json_type(other) }),
    }
}

fn json_type(v: &Value) -> Value {
    match v {
        Value::Null => Value::Null,
        Value::Bool(_) => Value::String("boolean".into()),
        Value::Number(n) if n.is_f64() => Value::String("number".into()),
        Value::Number(_) => Value::String("integer".into()),
        Value::String(_) => Value::String("string".into()),
        Value::Array(_) => Value::String("array".into()),
        Value::Object(_) => Value::String("object".into()),
    }
}

fn elapsed_ms(started: Instant) -> i32 {
    as_ms(started.elapsed())
}

fn as_ms(d: Duration) -> i32 {
    d.as_millis().min(i32::MAX as u128) as i32
}

fn truncate(s: &str) -> String {
    if s.len() <= MAX_TEXT {
        return s.to_string();
    }
    let mut end = MAX_TEXT;
    while end > 0 && !s.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}…", &s[..end])
}
