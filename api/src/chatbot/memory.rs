//! Tiers 2 and 3 of the conversation store.
//!
//! Tier 2 is a session closed on idle and reduced to ~150 words. Tier 3 is the
//! handful of durable facts extracted from the *same* call — one GPU trip, not
//! two — which then ride in every prompt instead of being retrieved.
//!
//! Two rules the design is emphatic about, and both are enforced here:
//!
//! * **not every session is worth a GPU call.** "what's on tonight" → answer →
//!   idle is worth nothing, and a context line saying *she asked what was on* is
//!   worse than no line at all. [`worth_summarising`] is that gate;
//! * a koblem completion goes through `acquire_for_llm` and a render can evict
//!   the model outright, so this runs in a job and takes as long as it takes. It
//!   is never on a live turn's path.

use serde_json::Value;
use sqlx::PgPool;
use uuid::Uuid;

use crate::repositories::chat_memory;
use crate::repositories::chat_profile_facts::{self, MAX_FACTS_PER_USER, valid_kind};
use crate::repositories::chat_sessions::{self, MessageRow};

use super::llm::{self, ChatMessage, LlmConfig};
use super::store::Step;

/// Idle before a session is considered over. Matches `store::IDLE`, so a session
/// Tier 1 would still resume is never one Tier 2 has already closed.
pub const IDLE_MINUTES: i32 = 45;

/// A session with no tool call needs at least this many user turns before it is
/// worth compressing. Below it there is nothing a summary could say.
const MIN_TURNS_WITHOUT_A_TOOL: usize = 4;

/// Facts taken from one conversation. A session that suddenly yields ten
/// "preferences" has invented most of them.
const MAX_FACTS_PER_SESSION: usize = 3;

const MAX_SUMMARY_WORDS: usize = 150;
/// Transcript lines fed to the summariser. A session longer than this is
/// summarised from its most recent turns, which is where the unfinished work is.
const MAX_TRANSCRIPT_ROWS: i64 = 60;
/// Sessions closed per sweep. The sweep runs every few minutes; a cap keeps one
/// pass from queueing an unbounded number of completions behind the LLM gate.
const MAX_SESSIONS_PER_SWEEP: i64 = 20;

const DIGEST_SYSTEM: &str = r#"You compress one finished conversation between a household member and the assistant for their media library.

Reply with JSON and nothing else:
{"summary": "...", "facts": [{"kind": "prefers", "value": "..."}]}

summary: at most 150 words, past tense. What was asked, what was actually done, and what was left unfinished. If something was started and never confirmed as finished, say so plainly — that is the most useful sentence in it.

facts: durable things about this person that would still be true next month — how they like to watch, what they enjoy, what they will not sit through. kind is one of prefers, likes, avoids. At most three, and only ones the conversation states outright. A one-off request is not a preference. Return an empty list rather than guessing."#;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewFact {
    pub kind: String,
    pub value: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SessionDigest {
    pub summary: String,
    pub facts: Vec<NewFact>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct SweepReport {
    pub closed: usize,
    pub summarised: usize,
    pub facts_added: usize,
    pub failed: usize,
}

/// Is this session worth a GPU call?
///
/// A tool call means something was *done*, which is exactly what "did that ever
/// finish?" the next morning is asking about. Failing that, a conversation has
/// to have actually been a conversation.
pub fn worth_summarising(rows: &[MessageRow]) -> bool {
    if rows.is_empty() {
        return false;
    }
    let used_a_tool = rows.iter().any(|r| {
        serde_json::from_value::<Vec<Step>>(r.steps.clone())
            .map(|s| !s.is_empty())
            .unwrap_or(false)
    });
    if used_a_tool {
        return true;
    }
    rows.iter().filter(|r| r.role == "user").count() >= MIN_TURNS_WITHOUT_A_TOOL
}

/// The transcript as the summariser sees it. Tool observations are named, never
/// pasted: what matters is that a search happened and what came of it, and the
/// raw JSON would be most of the prompt.
pub fn transcript(rows: &[MessageRow]) -> String {
    let mut out = String::new();
    for row in rows {
        let steps: Vec<Step> = serde_json::from_value(row.steps.clone()).unwrap_or_default();
        if !steps.is_empty() {
            let names: Vec<String> = steps
                .iter()
                .map(|s| format!("{} ({})", s.tool, s.state))
                .collect();
            out.push_str(&format!("[used {}]\n", names.join(", ")));
        }
        if row.content.trim().is_empty() {
            continue;
        }
        let who = match row.role.as_str() {
            "user" => "Them",
            "assistant" => "Assistant",
            other => other,
        };
        out.push_str(&format!("{who}: {}\n", row.content.trim()));
    }
    out
}

pub fn digest_messages(rows: &[MessageRow]) -> Vec<ChatMessage> {
    vec![
        ChatMessage::system(DIGEST_SYSTEM),
        ChatMessage::user(transcript(rows)),
    ]
}

/// Pull a digest out of whatever the model actually returned.
///
/// Gemma wraps JSON in fences about half the time and adds a sentence in front
/// of it often enough to matter, so this looks for the object rather than
/// trusting the whole body to parse.
pub fn parse_digest(raw: &str) -> Option<SessionDigest> {
    let start = raw.find('{')?;
    let end = raw.rfind('}')?;
    if end <= start {
        return None;
    }
    let value: Value = serde_json::from_str(&raw[start..=end]).ok()?;

    let summary = clamp_words(
        value
            .get("summary")
            .and_then(Value::as_str)
            .unwrap_or("")
            .trim(),
        MAX_SUMMARY_WORDS,
    );
    if summary.is_empty() {
        return None;
    }

    let facts = value
        .get("facts")
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(parse_fact)
                .take(MAX_FACTS_PER_SESSION)
                .collect()
        })
        .unwrap_or_default();

    Some(SessionDigest { summary, facts })
}

fn parse_fact(item: &Value) -> Option<NewFact> {
    let kind = item
        .get("kind")
        .and_then(Value::as_str)?
        .trim()
        .to_lowercase();
    if !valid_kind(&kind) {
        return None;
    }
    let value = item.get("value").and_then(Value::as_str)?.trim();
    if value.is_empty() || value.len() > 200 {
        return None;
    }
    Some(NewFact {
        kind,
        value: value.to_string(),
    })
}

fn clamp_words(text: &str, max: usize) -> String {
    let words: Vec<&str> = text.split_whitespace().collect();
    match words.len() > max {
        true => words[..max].join(" "),
        false => words.join(" "),
    }
}

/// The Tier 3 line that goes into every prompt for this person.
///
/// One line, grouped by kind, because it is paid for on every single message.
pub async fn facts_line(pool: &PgPool, user_id: Uuid) -> Option<String> {
    let facts = chat_profile_facts::for_prompt(pool, user_id).await.ok()?;
    if facts.is_empty() {
        return None;
    }
    let mut groups: Vec<(&str, Vec<String>)> = Vec::new();
    for kind in chat_profile_facts::KINDS {
        let values: Vec<String> = facts
            .iter()
            .filter(|f| f.kind == kind)
            .map(|f| f.value.clone())
            .collect();
        if !values.is_empty() {
            groups.push((kind, values));
        }
    }
    let rendered: Vec<String> = groups
        .into_iter()
        .map(|(kind, values)| format!("{kind}: {}", values.join(", ")))
        .collect();
    Some(rendered.join(" · "))
}

/// Close every session that has gone quiet, summarising the ones that earned it.
///
/// Sequential on purpose. Every completion queues behind `acquire_for_llm`, and
/// firing twenty at once at a gated GPU makes the wait longer, not shorter.
pub async fn sweep_idle_sessions(
    pool: &PgPool,
    config: Option<&LlmConfig>,
    idle_minutes: i32,
) -> Result<SweepReport, sqlx::Error> {
    let idle = chat_memory::idle_sessions(pool, idle_minutes, MAX_SESSIONS_PER_SWEEP)
        .await
        .map_err(to_sqlx)?;

    let mut report = SweepReport::default();
    for session in idle {
        let rows = chat_sessions::messages(pool, session.id, MAX_TRANSCRIPT_ROWS).await?;

        let digest = match (worth_summarising(&rows), config) {
            (true, Some(cfg)) => digest_of(cfg, &rows).await,
            _ => None,
        };

        let Some(digest) = digest else {
            chat_memory::close_with_summary(pool, session.id, None)
                .await
                .map_err(to_sqlx)?;
            report.closed += 1;
            continue;
        };

        chat_memory::close_with_summary(pool, session.id, Some(&digest.summary))
            .await
            .map_err(to_sqlx)?;
        report.closed += 1;
        report.summarised += 1;

        if let Some(user_id) = session.user_id {
            report.facts_added += store_facts(pool, user_id, session.id, &digest.facts)
                .await
                .map_err(to_sqlx)?;
        }
    }
    Ok(report)
}

/// One completion, folded into a digest. A model that answers with prose closes
/// the session silently rather than writing a line nobody can use.
async fn digest_of(config: &LlmConfig, rows: &[MessageRow]) -> Option<SessionDigest> {
    let messages = digest_messages(rows);
    match llm::complete(config, &messages, &[], |_| async {}).await {
        Ok(completion) => parse_digest(&completion.content),
        Err(e) => {
            tracing::warn!("chat session summary failed: {e}");
            None
        }
    }
}

/// Write the facts and hold the profile at its cap, oldest evicted.
pub async fn store_facts(
    pool: &PgPool,
    user_id: Uuid,
    source_id: Uuid,
    facts: &[NewFact],
) -> Result<usize, crate::repositories::RepositoryError> {
    let mut added = 0;
    for fact in facts {
        if chat_profile_facts::insert(pool, user_id, &fact.kind, &fact.value, Some(source_id))
            .await?
            .is_some()
        {
            added += 1;
        }
    }
    if added > 0 {
        chat_profile_facts::trim_to_cap(pool, user_id, MAX_FACTS_PER_USER).await?;
    }
    Ok(added)
}

fn to_sqlx(error: crate::repositories::RepositoryError) -> sqlx::Error {
    match error {
        crate::repositories::RepositoryError::Database(e) => e,
        other => sqlx::Error::Protocol(other.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use time::OffsetDateTime;

    fn row(role: &str, content: &str, tools: &[&str]) -> MessageRow {
        let steps: Vec<Step> = tools
            .iter()
            .enumerate()
            .map(|(i, name)| Step {
                index: i as i32,
                call_id: format!("c{i}"),
                tool: (*name).to_string(),
                arguments: "{}".to_string(),
                summary: String::new(),
                state: "done".to_string(),
                detail: String::new(),
                observation: json!({"ok": true}),
            })
            .collect();
        MessageRow {
            id: Uuid::now_v7(),
            role: role.to_string(),
            content: content.to_string(),
            steps: serde_json::to_value(steps).unwrap(),
            status: "complete".to_string(),
            created_at: OffsetDateTime::now_utc(),
        }
    }

    /// The design's own example of a session that must close silently.
    #[test]
    fn a_one_shot_question_is_not_worth_a_gpu_call() {
        let rows = vec![
            row("user", "what's on tonight", &[]),
            row("assistant", "Nothing new tonight.", &[]),
        ];
        assert!(!worth_summarising(&rows));
    }

    #[test]
    fn a_session_that_did_something_is_always_summarised() {
        let rows = vec![
            row("user", "grab last night's episode", &[]),
            row("assistant", "Searching.", &["want_it"]),
        ];
        assert!(worth_summarising(&rows));
    }

    #[test]
    fn a_conversation_that_ran_on_is_summarised_without_a_tool_call() {
        let mut rows = Vec::new();
        for _ in 0..MIN_TURNS_WITHOUT_A_TOOL {
            rows.push(row("user", "and then?", &[]));
            rows.push(row("assistant", "well", &[]));
        }
        assert!(worth_summarising(&rows));
        assert!(!worth_summarising(&rows[..5]));
    }

    #[test]
    fn an_empty_session_closes_silently() {
        assert!(!worth_summarising(&[]));
    }

    #[test]
    fn a_digest_survives_fences_and_a_preamble() {
        let raw = "Sure, here you go:\n```json\n{\"summary\": \"She asked for season 24 and it was queued; the download had not finished.\", \"facts\": [{\"kind\": \"likes\", \"value\": \"70s horror\"}]}\n```";
        let digest = parse_digest(raw).expect("a digest");
        assert!(digest.summary.starts_with("She asked for season 24"));
        assert_eq!(
            digest.facts,
            vec![NewFact {
                kind: "likes".to_string(),
                value: "70s horror".to_string()
            }]
        );
    }

    #[test]
    fn a_summary_is_clamped_to_its_word_budget() {
        let long = "word ".repeat(400);
        let raw = json!({"summary": long, "facts": []}).to_string();
        let digest = parse_digest(&raw).expect("a digest");
        assert_eq!(digest.summary.split_whitespace().count(), MAX_SUMMARY_WORDS);
    }

    #[test]
    fn invented_kinds_and_empty_values_are_dropped() {
        let raw = json!({
            "summary": "Something happened.",
            "facts": [
                {"kind": "hates", "value": "musicals"},
                {"kind": "avoids", "value": ""},
                {"kind": "Prefers", "value": "subtitles over dubs"},
            ],
        })
        .to_string();
        let digest = parse_digest(&raw).expect("a digest");
        assert_eq!(
            digest.facts,
            vec![NewFact {
                kind: "prefers".to_string(),
                value: "subtitles over dubs".to_string()
            }]
        );
    }

    #[test]
    fn a_model_that_answered_in_prose_yields_no_digest() {
        assert_eq!(parse_digest("They talked about films."), None);
        assert_eq!(parse_digest("{\"facts\": []}"), None);
    }

    #[test]
    fn only_three_facts_come_out_of_one_conversation() {
        let facts: Vec<Value> = (0..9)
            .map(|i| json!({"kind": "likes", "value": format!("thing {i}")}))
            .collect();
        let raw = json!({"summary": "A long chat.", "facts": facts}).to_string();
        assert_eq!(
            parse_digest(&raw).expect("a digest").facts.len(),
            MAX_FACTS_PER_SESSION
        );
    }

    #[test]
    fn the_transcript_names_tools_rather_than_pasting_observations() {
        let rows = vec![
            row("user", "find carrie", &[]),
            row("assistant", "Two matches.", &["search_library"]),
        ];
        let text = transcript(&rows);
        assert!(text.contains("[used search_library (done)]"), "{text}");
        assert!(text.contains("Them: find carrie"), "{text}");
        assert!(!text.contains("\"ok\""), "an observation leaked in: {text}");
    }
}
