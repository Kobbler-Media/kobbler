//! Tier 1 of the conversation store: the live turn window.
//!
//! Two rules from the design, and both are load-bearing:
//!
//! * the window is a **token** budget read off the model's real context, not a
//!   message count — the same number is 6 turns on a cloud model and 60 on
//!   koblem's gemma;
//! * trimming keeps the system prompt and the most recent tool result, because
//!   "no, the other one" needs a referent.
//!
//! It trims whole tool exchanges rather than messages. Dropping an assistant
//! turn that carried `tool_calls` while keeping its `role: "tool"` replies
//! leaves a dangling tool response, which Gemma's baked template renders into
//! a corrupt prompt and llama.cpp rejects outright.

use serde::{Deserialize, Serialize};
use serde_json::Value;
use sqlx::PgPool;
use time::{Duration, OffsetDateTime};
use uuid::Uuid;

use crate::repositories::chat_sessions::{self, MessageRow, NewSession, SessionRow};

use super::llm::{ChatMessage, ToolCall};

pub const TRANSPORT_WEB: &str = "web";

/// A conversation goes stale after this long with nothing said. Matches the
/// design's Tier 2 close-on-idle window, so a resumed conversation is one that
/// Tier 2 would not yet have summarised.
const IDLE: Duration = Duration::minutes(45);

/// Rows read back per session. A hard stop on the query so a runaway thread
/// cannot make the window build cost unbounded before the trimmer sees it.
const MAX_HISTORY_ROWS: i64 = 200;

/// One tool call as durably recorded on the assistant message.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Step {
    pub index: i32,
    pub call_id: String,
    pub tool: String,
    /// Raw arguments JSON exactly as the model emitted them.
    pub arguments: String,
    /// Short human-facing rendering of the arguments, for transports and logs.
    pub summary: String,
    /// `running` | `done` | `failed`
    pub state: String,
    pub detail: String,
    pub observation: Value,
}

impl Step {
    pub fn to_call(&self) -> ToolCall {
        ToolCall {
            id: self.call_id.clone(),
            name: self.tool.clone(),
            arguments: self.arguments.clone(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionKind {
    Reused,
    Fresh,
}

pub struct OpenSession {
    pub id: Uuid,
    pub kind: SessionKind,
}

/// Resolve the session at a transport-native address (Google Chat's
/// space + thread), creating one on first contact.
pub async fn open_addressed_session(
    db: &PgPool,
    transport: &str,
    space: &str,
    thread: &str,
    user_id: Option<Uuid>,
) -> Result<OpenSession, sqlx::Error> {
    if let Some(existing) =
        chat_sessions::find_open_by_address(db, transport, space, thread).await?
        && !is_stale(&existing)
    {
        chat_sessions::touch_session(db, existing.id).await?;
        return Ok(OpenSession {
            id: existing.id,
            kind: SessionKind::Reused,
        });
    }
    if let Some(stale) = chat_sessions::find_open_by_address(db, transport, space, thread).await? {
        chat_sessions::close_session(db, stale.id).await?;
    }
    let created = chat_sessions::create_session(
        db,
        NewSession {
            transport,
            space: Some(space),
            thread: Some(thread),
            user_id,
        },
    )
    .await?;
    Ok(OpenSession {
        id: created.id,
        kind: SessionKind::Fresh,
    })
}

/// Resolve the session for a front end that has no conversation address of its
/// own — the Command Palette, whose "session" is the browser's own chat state.
///
/// The client resends its whole visible history on every turn and clears it on
/// idle, so `user_turns` (how many user messages the client believes are in the
/// conversation) is the honest key: it lines up with the stored count exactly
/// when the client is continuing the conversation the store remembers, and
/// diverges the moment the user clears, expires or opens a second tab. A
/// divergence forks rather than corrupts.
pub async fn open_user_session(
    db: &PgPool,
    transport: &str,
    user_id: Uuid,
    user_turns: usize,
) -> Result<OpenSession, sqlx::Error> {
    let cutoff = OffsetDateTime::now_utc() - IDLE;
    if user_turns > 1
        && let Some(existing) =
            chat_sessions::find_open_for_user(db, transport, user_id, cutoff).await?
    {
        let stored = chat_sessions::count_messages_with_role(db, existing.id, "user").await?;
        if stored as usize + 1 == user_turns {
            chat_sessions::touch_session(db, existing.id).await?;
            return Ok(OpenSession {
                id: existing.id,
                kind: SessionKind::Reused,
            });
        }
        chat_sessions::close_session(db, existing.id).await?;
    } else if let Some(existing) =
        chat_sessions::find_open_for_user(db, transport, user_id, cutoff).await?
    {
        chat_sessions::close_session(db, existing.id).await?;
    }

    let created = chat_sessions::create_session(
        db,
        NewSession {
            transport,
            space: None,
            thread: None,
            user_id: Some(user_id),
        },
    )
    .await?;
    Ok(OpenSession {
        id: created.id,
        kind: SessionKind::Fresh,
    })
}

fn is_stale(session: &SessionRow) -> bool {
    OffsetDateTime::now_utc() - session.updated_at > IDLE
}

pub async fn record_user_turn(
    db: &PgPool,
    session_id: Uuid,
    content: &str,
) -> Result<Uuid, sqlx::Error> {
    chat_sessions::append_message(db, session_id, "user", content, "complete").await
}

pub async fn start_assistant_turn(db: &PgPool, session_id: Uuid) -> Result<Uuid, sqlx::Error> {
    chat_sessions::append_message(db, session_id, "assistant", "", "running").await
}

pub async fn put_steps(db: &PgPool, message_id: Uuid, steps: &[Step]) -> Result<(), sqlx::Error> {
    let value = serde_json::to_value(steps).unwrap_or_else(|_| Value::Array(Vec::new()));
    chat_sessions::finish_message(db, message_id, "", &value, "running").await
}

pub async fn finish_assistant_turn(
    db: &PgPool,
    message_id: Uuid,
    content: &str,
    steps: &[Step],
    status: &str,
) -> Result<(), sqlx::Error> {
    let value = serde_json::to_value(steps).unwrap_or_else(|_| Value::Array(Vec::new()));
    chat_sessions::finish_message(db, message_id, content, &value, status).await
}

/// Replay a session as prompt messages, newest tool exchange included.
///
/// Only the most recent assistant turn replays its tool calls. Older turns come
/// back as narration alone: their results are already reflected in what the
/// assistant said, and re-sending them both costs tokens and invites the model
/// to redo work it has already done.
pub async fn window(
    db: &PgPool,
    session_id: Uuid,
    exclude: Option<Uuid>,
) -> Result<Vec<ChatMessage>, sqlx::Error> {
    let rows = chat_sessions::messages(db, session_id, MAX_HISTORY_ROWS).await?;
    Ok(replay(&rows, exclude))
}

fn replay(rows: &[MessageRow], exclude: Option<Uuid>) -> Vec<ChatMessage> {
    let kept: Vec<&MessageRow> = rows
        .iter()
        .filter(|r| Some(r.id) != exclude && r.status != "error")
        .collect();
    let newest_with_steps = kept
        .iter()
        .rposition(|r| r.role == "assistant" && !steps_of(r).is_empty());

    let mut out = Vec::new();
    for (n, row) in kept.iter().enumerate() {
        if row.role != "assistant" {
            out.push(ChatMessage {
                role: row.role.clone(),
                content: row.content.clone(),
                tool_calls: Vec::new(),
                tool_call_id: None,
            });
            continue;
        }
        if Some(n) == newest_with_steps {
            let steps = steps_of(row);
            out.push(ChatMessage::assistant_calls(
                steps.iter().map(Step::to_call).collect(),
            ));
            for step in &steps {
                out.push(ChatMessage::tool(&step.call_id, &step.observation));
            }
        }
        if !row.content.is_empty() {
            out.push(ChatMessage::assistant(row.content.clone()));
        }
    }
    out
}

fn steps_of(row: &MessageRow) -> Vec<Step> {
    serde_json::from_value(row.steps.clone()).unwrap_or_default()
}

/// Trim a prompt to a token budget, oldest first.
///
/// Protected from trimming: every leading system message, the newest exchange,
/// and the newest exchange that carries a tool result.
pub fn trim_to_budget(messages: Vec<ChatMessage>, budget: usize) -> Vec<ChatMessage> {
    let lead = messages
        .iter()
        .position(|m| m.role != "system")
        .unwrap_or(messages.len());
    let (system, rest) = messages.split_at(lead);
    let mut groups = group_exchanges(rest.to_vec());

    let protected = {
        let last = groups.len().saturating_sub(1);
        let last_tool = groups
            .iter()
            .rposition(|g| g.iter().any(|m| m.role == "tool"));
        move |i: usize| i == last || Some(i) == last_tool
    };

    let system_cost: usize = system.iter().map(ChatMessage::estimated_tokens).sum();
    let mut cost: usize = system_cost
        + groups
            .iter()
            .flatten()
            .map(ChatMessage::estimated_tokens)
            .sum::<usize>();

    let mut drop_flags = vec![false; groups.len()];
    for i in 0..groups.len() {
        if cost <= budget {
            break;
        }
        if protected(i) {
            continue;
        }
        cost -= groups[i]
            .iter()
            .map(ChatMessage::estimated_tokens)
            .sum::<usize>();
        drop_flags[i] = true;
    }

    let mut out: Vec<ChatMessage> = system.to_vec();
    for (i, group) in groups.drain(..).enumerate() {
        if drop_flags[i] {
            continue;
        }
        out.extend(group);
    }
    out
}

/// An assistant turn carrying `tool_calls` and the `role: "tool"` replies that
/// answer it are one indivisible unit; everything else is a group of one.
fn group_exchanges(messages: Vec<ChatMessage>) -> Vec<Vec<ChatMessage>> {
    let mut groups: Vec<Vec<ChatMessage>> = Vec::new();
    for message in messages {
        let continues = message.role == "tool"
            || (message.role == "assistant"
                && groups
                    .last()
                    .is_some_and(|g| g.iter().any(|m| !m.tool_calls.is_empty())));
        match continues {
            true if !groups.is_empty() => groups.last_mut().expect("checked").push(message),
            _ => groups.push(vec![message]),
        }
    }
    groups
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn tool_step(name: &str, id: &str) -> Step {
        Step {
            index: 0,
            call_id: id.to_string(),
            tool: name.to_string(),
            arguments: r#"{"query":"carrie"}"#.to_string(),
            summary: "carrie".to_string(),
            state: "done".to_string(),
            detail: "3 results".to_string(),
            observation: json!({"ok": true, "results": ["Carrie", "Carrie (2013)"]}),
        }
    }

    fn row(role: &str, content: &str, steps: Vec<Step>) -> MessageRow {
        MessageRow {
            id: Uuid::now_v7(),
            role: role.to_string(),
            content: content.to_string(),
            steps: serde_json::to_value(steps).unwrap(),
            status: "complete".to_string(),
            created_at: OffsetDateTime::now_utc(),
        }
    }

    fn long(role: &str, n: usize) -> ChatMessage {
        ChatMessage {
            role: role.to_string(),
            content: "x".repeat(n),
            tool_calls: Vec::new(),
            tool_call_id: None,
        }
    }

    #[test]
    fn the_system_prompt_survives_any_budget() {
        let trimmed = trim_to_budget(
            vec![
                ChatMessage::system("stay"),
                long("user", 4_000),
                long("assistant", 4_000),
                long("user", 4_000),
            ],
            10,
        );
        assert_eq!(trimmed[0].role, "system");
        assert_eq!(trimmed[0].content, "stay");
    }

    #[test]
    fn trimming_drops_the_oldest_turns_first() {
        let trimmed = trim_to_budget(
            vec![
                ChatMessage::system("s"),
                ChatMessage::user("oldest"),
                ChatMessage::assistant("a1"),
                ChatMessage::user("middle"),
                ChatMessage::assistant("a2"),
                ChatMessage::user("newest"),
            ],
            // Enough for the system prompt and roughly the last turn only.
            14,
        );
        let text: Vec<&str> = trimmed.iter().map(|m| m.content.as_str()).collect();
        assert!(text.contains(&"newest"), "{text:?}");
        assert!(!text.contains(&"oldest"), "{text:?}");
    }

    /// "no, the other one" only works while the last tool result is still in
    /// context, so it is protected even when everything around it is dropped.
    #[test]
    fn the_most_recent_tool_result_is_never_trimmed() {
        let call = ToolCall {
            id: "c1".into(),
            name: "search_library".into(),
            arguments: "{}".into(),
        };
        let trimmed = trim_to_budget(
            vec![
                ChatMessage::system("s"),
                long("user", 2_000),
                long("assistant", 2_000),
                long("user", 2_000),
                ChatMessage::assistant_calls(vec![call]),
                ChatMessage::tool("c1", &json!({"results": ["Carrie", "Carrie (2013)"]})),
                ChatMessage::assistant("Two matches"),
                ChatMessage::user("no, the other one"),
            ],
            60,
        );
        assert!(
            trimmed.iter().any(|m| m.role == "tool"),
            "the referent for 'the other one' was trimmed away"
        );
        assert!(trimmed.iter().any(|m| m.content == "no, the other one"));
        assert!(trimmed.iter().all(|m| m.content.len() < 2_000));
    }

    /// Dropping the assistant turn but keeping its replies leaves a dangling
    /// tool response, which is a corrupt prompt rather than a smaller one.
    #[test]
    fn a_tool_exchange_is_dropped_whole_or_not_at_all() {
        let call = ToolCall {
            id: "c1".into(),
            name: "search_library".into(),
            arguments: "{}".into(),
        };
        let trimmed = trim_to_budget(
            vec![
                ChatMessage::system("s"),
                ChatMessage::assistant_calls(vec![call]),
                ChatMessage::tool("c1", &json!({"a": 1})),
                ChatMessage::assistant("older answer"),
                long("user", 8_000),
                long("assistant", 8_000),
                ChatMessage::user("newest"),
            ],
            40,
        );
        let orphan = trimmed.iter().any(|m| m.role == "tool")
            && !trimmed.iter().any(|m| !m.tool_calls.is_empty());
        assert!(!orphan, "a tool reply outlived the call it answers");
    }

    #[test]
    fn a_prompt_already_inside_budget_is_returned_untouched() {
        let input = vec![
            ChatMessage::system("s"),
            ChatMessage::user("hi"),
            ChatMessage::assistant("hello"),
        ];
        assert_eq!(trim_to_budget(input.clone(), 10_000), input);
    }

    #[test]
    fn only_the_newest_stored_turn_replays_its_tool_calls() {
        let rows = vec![
            row("user", "find carrie", vec![]),
            row(
                "assistant",
                "Found two",
                vec![tool_step("search_library", "c1")],
            ),
            row("user", "and the sequel?", vec![]),
            row(
                "assistant",
                "Found one",
                vec![tool_step("search_library", "c2")],
            ),
        ];
        let replayed = replay(&rows, None);
        let call_ids: Vec<String> = replayed
            .iter()
            .flat_map(|m| m.tool_calls.iter().map(|c| c.id.clone()))
            .collect();
        assert_eq!(call_ids, vec!["c2".to_string()]);
        assert_eq!(
            replayed.iter().filter(|m| m.role == "tool").count(),
            1,
            "older observations replay as narration, not as tool messages"
        );
        assert!(replayed.iter().any(|m| m.content == "Found two"));
    }

    #[test]
    fn the_turn_being_answered_is_excluded_from_its_own_history() {
        let rows = vec![row("user", "hi", vec![]), row("assistant", "", vec![])];
        let replayed = replay(&rows, Some(rows[1].id));
        assert_eq!(replayed.len(), 1);
        assert_eq!(replayed[0].content, "hi");
    }

    #[test]
    fn a_failed_turn_is_not_replayed_as_context() {
        let mut failed = row("assistant", "the model fell over", vec![]);
        failed.status = "error".to_string();
        let rows = vec![row("user", "hi", vec![]), failed];
        assert_eq!(replay(&rows, None).len(), 1);
    }
}
