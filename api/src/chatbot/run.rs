//! The turn loop.
//!
//! Rounds of: build the prompt, ask the model, run whatever tools it asked for,
//! fold the results back in, ask again. It emits [`TurnEvent`]s and knows
//! nothing about SSE, cards or HTTP — that separation is what lets the Command
//! Palette and Google Chat be the same assistant.
//!
//! There is no request timeout anywhere below. A koblem completion goes through
//! `acquire_for_llm`, and a flux or songgen render can evict the LLM outright,
//! so a legitimate turn can sit for minutes before a token arrives. The answer
//! to that is [`ToolStarted`]-style honesty — a "still thinking" note after ten
//! seconds so a transport has something to show — not a clock that kills the
//! work.

use std::time::{Duration, Instant};

use serde_json::Value;
use uuid::Uuid;

use crate::AppState;
use crate::routes::AuthUser;

use super::events::{Source, TurnEvent, TurnSender};
use super::llm::{self, ChatMessage, LlmConfig, ToolCall};
use super::logs::TurnLog;
use super::prompt;
use super::store::{self, Step};
use super::tools::dispatch::{self, Ctx};
use super::tools::read::{self, ToolOutcome};
use super::tools::{self, Tool};

/// Tool rounds per turn. Enough for "find the show, then check its schedule",
/// short enough that a confused model stops rather than grinding.
const MAX_ROUNDS: usize = 4;
/// Calls honoured from a single reply. Bounds the width of one round the way
/// `MAX_ROUNDS` bounds its depth.
const MAX_CALLS_PER_ROUND: usize = 3;
/// A tool that has failed this often this turn is not tried again — a model
/// that retries a broken search just spends the round budget arriving at the
/// same error.
const MAX_FAILURES_PER_TOOL: usize = 2;
/// How long a silent turn goes before the transport is told it is still alive.
const STILL_THINKING_AFTER: Duration = Duration::from_secs(10);

pub struct Turn<'a> {
    pub state: &'a AppState,
    pub auth: &'a AuthUser,
    /// Which front end asked. Recorded on the turn log so the viewer can tell a
    /// Command Palette turn from a Google Chat one.
    pub transport: &'a str,
    pub session_id: Uuid,
    pub message_id: Uuid,
    /// The conversation so far, oldest first, WITHOUT the new user message.
    pub history: Vec<ChatMessage>,
    pub user_message: String,
    /// What the previous turn's `resolve_media` settled on, if anything.
    pub context_ref: Option<String>,
}

pub struct TurnResult {
    pub text: String,
    pub steps: Vec<Step>,
    pub status: &'static str,
}

/// Run one turn to completion, emitting events as it goes.
///
/// Always emits exactly one terminal event — [`TurnEvent::Done`] or
/// [`TurnEvent::Failed`]. Silence reads as broken, so every path out of here
/// says something.
pub async fn run(turn: Turn<'_>, events: TurnSender) -> TurnResult {
    // One row per turn, written whether or not the turn answers — a failed turn
    // is the row worth having, and it is how the resolver gets tuned.
    let mut log = TurnLog::open(
        turn.transport,
        Some(turn.session_id),
        Some(turn.auth.user_id),
        &turn.user_message,
    );

    let config = match llm::resolve(&turn.state.runtime_config.get().await) {
        Ok(c) => c,
        Err(e) => {
            let _ = events.send(TurnEvent::Failed(e.clone())).await;
            log.fail(&turn.state.pool, &e).await;
            return TurnResult {
                text: e,
                steps: Vec::new(),
                status: "error",
            };
        }
    };
    log.llm(provider_name(&config), &config.model);

    match execute(&turn, &config, &events, &mut log).await {
        Ok((text, steps)) => {
            let _ = events.send(TurnEvent::Done { text: text.clone() }).await;
            log.finish(&turn.state.pool, &text).await;
            TurnResult {
                text,
                steps,
                status: "complete",
            }
        }
        Err((reason, steps)) => {
            let _ = events.send(TurnEvent::Failed(reason.clone())).await;
            log.fail(&turn.state.pool, &reason).await;
            TurnResult {
                text: reason,
                steps,
                status: "error",
            }
        }
    }
}

fn provider_name(config: &LlmConfig) -> &'static str {
    match config.is_koblem {
        true => "koblem",
        false => "external",
    }
}

type Failure = (String, Vec<Step>);

async fn execute(
    turn: &Turn<'_>,
    config: &LlmConfig,
    events: &TurnSender,
    log: &mut TurnLog,
) -> Result<(String, Vec<Step>), Failure> {
    let budget = llm::window_budget(llm::context_tokens(config).await);
    let mut system = prompt::system_prompt();
    // Tier 3 facts and the carried-forward session summary ride inside the
    // system message itself. The trimmer protects everything before the first
    // non-system message, so joining them loses nothing — and Qwen-family chat
    // templates hard-400 on a system message anywhere after index 0 ("System
    // message must be at the beginning."), which is what broke the bot the
    // first time a Qwen model was pointed at it. Gemma's template has no such
    // assertion, which is why the second message worked until then.
    if let Some(memory) =
        prompt::memory_prompt(&turn.state.pool, turn.auth.user_id, turn.session_id).await
    {
        system.push_str("\n\n");
        system.push_str(&memory);
    }
    let today = crate::notifications::prefs::zone_for_user(&turn.state.pool, turn.auth.user_id)
        .await
        .local_date(time::OffsetDateTime::now_utc());
    system.push_str("\n\n");
    system.push_str(&prompt::date_line(today));
    let mut messages = vec![ChatMessage::system(system)];
    messages.extend(turn.history.iter().cloned());
    messages.push(ChatMessage::user(&turn.user_message));
    let mut messages = store::trim_to_budget(messages, budget);

    let schemas = tools::schemas();
    let mut steps: Vec<Step> = Vec::new();
    let mut failures: Vec<(Tool, usize)> = Vec::new();
    let mut answer = String::new();
    let mut context_ref = turn.context_ref.clone();

    for _round in 0..MAX_ROUNDS {
        let round_started = Instant::now();
        let heartbeat = still_thinking(events.clone());
        let completion = llm::complete(config, &messages, &schemas, |delta| {
            let events = events.clone();
            async move {
                let _ = events.send(TurnEvent::Delta(delta)).await;
            }
        })
        .await;
        heartbeat.abort();
        let usage = completion.as_ref().ok().and_then(|c| c.usage);
        log.llm_round(
            round_started.elapsed(),
            usage.map(|u| u.prompt_tokens),
            usage.map(|u| u.completion_tokens),
        );

        let completion = match completion {
            Ok(c) => c,
            Err(e) => return Err((e, steps)),
        };

        let calls = bounded(completion.tool_calls);
        if calls.is_empty() {
            answer = completion.content;
            if answer.is_empty() {
                answer = "I could not come up with an answer for that.".to_string();
            }
            // The shape of the hallucinated "Done — I'm keeping an eye out":
            // the turn ends without a single tool call while a confirmation is
            // still live in the conversation. Nothing failed, so nothing else
            // logs it; the warn is the only trace of the turn doing nothing it
            // said it did.
            if steps.is_empty() && unspent_confirmation_in(&turn.history) {
                tracing::warn!(
                    session_id = %turn.session_id,
                    transport = turn.transport,
                    "turn ended with no tool calls while a confirmation was still live"
                );
            }
            return Ok((answer, steps));
        }

        // Whatever prose came alongside the calls is part of the answer the
        // user is watching stream in, so it is kept rather than discarded.
        if !completion.content.is_empty() {
            answer.push_str(&completion.content);
        }

        let mut results = Vec::with_capacity(calls.len());
        for call in &calls {
            let (step, observation) = run_one(
                turn,
                call,
                steps.len(),
                context_ref.as_deref(),
                &mut failures,
                events,
                log,
            )
            .await;
            // A resolution made this turn is the referent for the next call in
            // it, so "then get the next one" works inside one message too.
            if let Some(found) = read::context_ref_from(&observation) {
                context_ref = Some(found);
            }
            steps.push(step);
            results.push(ChatMessage::tool(&call.id, &observation));
        }
        let _ = store::put_steps(&turn.state.pool, turn.message_id, &steps).await;

        messages.push(ChatMessage::assistant_calls(calls));
        messages.extend(results);
        messages = store::trim_to_budget(messages, budget);

        if failures.iter().any(|(_, n)| *n >= MAX_FAILURES_PER_TOOL) {
            let reason = steps
                .iter()
                .rev()
                .find(|s| s.state == "failed")
                .map(|s| s.detail.clone())
                .unwrap_or_else(|| "a tool kept failing".to_string());
            return Ok((format!("I could not finish that — {reason}."), steps));
        }
    }

    Ok((
        match answer.is_empty() {
            true => {
                "I ran out of steps on that one — say which part to pick up and I will.".to_string()
            }
            false => answer,
        },
        steps,
    ))
}

/// A confirmation is live while its offer sits in the conversation unspent:
/// the offer step shows `pending_confirmation` without `confirmed`. Spent and
/// stale offers flip to `confirmed: true` or drop `pending_confirmation`, and
/// stepped turns older than the newest are flattened out of the history, so
/// this sees exactly the offer a tap could still be answering.
fn unspent_confirmation_in(history: &[ChatMessage]) -> bool {
    history.iter().rev().any(|m| {
        m.role == "tool"
            && m.content.contains("\"pending_confirmation\":true")
            && !m.content.contains("\"confirmed\":true")
    })
}

async fn run_one(
    turn: &Turn<'_>,
    call: &ToolCall,
    index: usize,
    context_ref: Option<&str>,
    failures: &mut Vec<(Tool, usize)>,
    events: &TurnSender,
    log: &mut TurnLog,
) -> (Step, Value) {
    let started = Instant::now();
    let args = call.args();
    let summary = Tool::parse(&call.name)
        .map(|t| dispatch::args_summary(t, &args))
        .unwrap_or_else(|| call.name.replace('_', " "));
    let _ = events
        .send(TurnEvent::ToolStarted {
            name: call.name.clone(),
            summary: summary.clone(),
        })
        .await;

    let ctx = Ctx {
        state: turn.state,
        auth: turn.auth,
        session_id: turn.session_id,
        context_ref,
        user_message: &turn.user_message,
    };
    let outcome = dispatch::dispatch(&ctx, &call.name, &args).await;
    let logged: Result<&Value, &String> = match outcome.ok {
        true => Ok(&outcome.observation),
        false => Err(&outcome.detail),
    };
    log.tool(started, &call.name, &args, logged);
    if Tool::parse(&call.name) == Some(Tool::ResolveMedia) {
        log_resolution(log, started, &args, &outcome);
    }

    if !outcome.ok
        && let Some(tool) = Tool::parse(&call.name)
    {
        match failures.iter_mut().find(|(t, _)| *t == tool) {
            Some((_, n)) => *n += 1,
            None => failures.push((tool, 1)),
        }
    }

    if !outcome.sources.is_empty() {
        let _ = events
            .send(TurnEvent::Sources(dedupe(outcome.sources.clone())))
            .await;
    }
    let _ = events
        .send(TurnEvent::ToolFinished {
            name: call.name.clone(),
            detail: outcome.detail.clone(),
            outputs: outcome.observation.clone(),
        })
        .await;

    let step = Step {
        index: index as i32,
        call_id: call.id.clone(),
        tool: call.name.clone(),
        arguments: call.arguments.clone(),
        summary,
        state: match outcome.ok {
            true => "done".to_string(),
            false => "failed".to_string(),
        },
        detail: outcome.detail,
        observation: outcome.observation.clone(),
    };
    (step, outcome.observation)
}

/// `resolve_media` gets its own log columns, not just a tool-call entry: a
/// resolution that returned the wrong candidate and one that failed outright
/// have completely different fixes, and the viewer is how the resolver is tuned.
fn log_resolution(log: &mut TurnLog, started: Instant, args: &Value, outcome: &ToolOutcome) {
    let query = args
        .get("query")
        .and_then(Value::as_str)
        .unwrap_or_default();
    if !outcome.ok {
        let failed: Result<&[Value], &String> = Err(&outcome.detail);
        log.resolve(started, query, None, failed);
        return;
    }
    let candidates: Vec<Value> = outcome
        .observation
        .get("candidates")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let resolved = read::context_ref_from(&outcome.observation);
    let found: Result<&[Value], &String> = Ok(&candidates);
    log.resolve(started, query, resolved.as_deref(), found);
}

/// One "still thinking" nudge, then silence. Repeating it every ten seconds
/// tells the transport nothing new and turns a slow turn into a spam of edits.
fn still_thinking(events: TurnSender) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        tokio::time::sleep(STILL_THINKING_AFTER).await;
        let _ = events
            .send(TurnEvent::ToolStarted {
                name: "thinking".to_string(),
                summary: "still working on it".to_string(),
            })
            .await;
    })
}

fn bounded(mut calls: Vec<ToolCall>) -> Vec<ToolCall> {
    calls.truncate(MAX_CALLS_PER_ROUND);
    calls
}

/// The citation list is keyed by id in the client, so a duplicate is a rendering
/// bug rather than a duplicate row.
fn dedupe(sources: Vec<Source>) -> Vec<Source> {
    let mut seen = std::collections::HashSet::new();
    sources
        .into_iter()
        .filter(|s| seen.insert((s.item_type.clone(), s.id.clone())))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn source(kind: &str, id: &str) -> Source {
        Source {
            item_type: kind.to_string(),
            id: id.to_string(),
            title: id.to_string(),
            url: format!("/{kind}/{id}"),
            cover_url: None,
            year: None,
            description: None,
        }
    }

    #[test]
    fn one_round_cannot_queue_more_calls_than_the_turn_accounts_for() {
        let calls: Vec<ToolCall> = (0..9)
            .map(|i| ToolCall {
                id: format!("c{i}"),
                name: "search_library".to_string(),
                arguments: "{}".to_string(),
            })
            .collect();
        assert_eq!(bounded(calls).len(), MAX_CALLS_PER_ROUND);
    }

    /// A live offer is one the tap could still be answering: it shows
    /// `pending_confirmation` without `confirmed`. Spent and stale offers flip
    /// to `confirmed: true`, and plain observations never match.
    #[test]
    fn an_unspent_confirmation_is_seen_but_not_a_spent_one() {
        let offer = ChatMessage::tool(
            "c1",
            &json!({
                "ok": true,
                "pending_confirmation": true,
                "confirmed": false,
                "confirm_token": "t-token-0123456789abcdef0123456789abcdef",
            }),
        );
        let spent = ChatMessage::tool(
            "c2",
            &json!({
                "ok": true,
                "pending_confirmation": true,
                "confirmed": true,
                "confirm_token": "t-token-0123456789abcdef0123456789abcdef",
            }),
        );
        let plain = ChatMessage::tool("c3", &json!({ "ok": true, "done": true }));

        assert!(unspent_confirmation_in(&[plain.clone(), offer.clone()]));
        assert!(!unspent_confirmation_in(&[plain.clone(), spent.clone()]));
        assert!(!unspent_confirmation_in(&[plain]));
    }

    #[test]
    fn citations_are_deduped_because_the_client_keys_on_id() {
        let deduped = dedupe(vec![
            source("movie", "1"),
            source("movie", "1"),
            source("book", "1"),
        ]);
        assert_eq!(deduped.len(), 2);
    }
}
