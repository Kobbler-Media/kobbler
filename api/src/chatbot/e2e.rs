//! Direct-LLM e2e engine: the real system prompt and the real tool schemas,
//! sent to the box's configured chat LLM, with tool execution replaced by
//! fixtures.
//!
//! This answers "given this data, what does the LLM do" without the API
//! process, without a database, and without any global mode: the loop is the
//! same request the chatbot makes, and every tool call the model emits is
//! asserted and answered from a fixture.
//!
//! Two thin callers share this module:
//!
//! * `api/src/bin/chatbot_e2e.rs` — shipped in the api image, so a deployed
//!   box can be asked: `docker exec kobbler-api-1 /app/chatbot_e2e`;
//! * `api/tests/integration/chatbot_llm_e2e.rs` — build-box `cargo test`, skipped unless
//!   `CHATBOT_E2E=1` so `make` and CI never hit a live model.

use crate::chatbot::llm::{self, ChatMessage, Completion, LlmConfig};
use crate::chatbot::prompt;
use crate::chatbot::tools;
use serde_json::{Value, json};

/// A turn that needs more rounds than this is looping, not thinking.
pub const MAX_ROUNDS: usize = 6;

/// The fixture world: one provider show (Unexpected, 2017, four seasons) and
/// a `want_it` that offers, then honours the token it handed out.
pub struct Fixture {
    pub offer_token: String,
    /// Every (tool, args) pair the model emitted, in order.
    pub calls: Vec<(String, Value)>,
}

impl Default for Fixture {
    fn default() -> Self {
        Self::new()
    }
}

impl Fixture {
    pub fn new() -> Self {
        Self {
            offer_token: uuid::Uuid::now_v7().to_string(),
            calls: Vec::new(),
        }
    }

    /// Answer a tool call the way the real tool would have.
    pub fn answer(&mut self, tool: &str, args: &Value) -> Value {
        self.calls.push((tool.to_string(), args.clone()));
        match tool {
            "resolve_media" => json!({
                "ok": true,
                "resolution": "single",
                "candidates": [{
                    "ref": "provider:tmdb:95396:show",
                    "title": "Unexpected",
                    "subtitle": "2017",
                    "year": 2017,
                    "description": "British anthology comedy, 2017–2023",
                    "in_library": false,
                    "show_id": null,
                    "is_sports": false,
                    "score": 0.97,
                    "source": {"provider": "tmdb"},
                }],
                "seasons": [
                    {"season": 1, "episodes": 12, "air_date": "2017-09-11"},
                    {"season": 2, "episodes": 12, "air_date": "2018-01-08"},
                    {"season": 3, "episodes": 12, "air_date": "2019-04-09"},
                    {"season": 4, "episodes": 12, "air_date": "2023-03-14"},
                ],
            }),
            "want_it" => match args.get("confirm").and_then(Value::as_str) {
                Some(c) if c.eq_ignore_ascii_case("stop") || c.eq_ignore_ascii_case("no") => {
                    json!({
                        "ok": true,
                        "cancelled": true,
                        "note": "the user said stop; nothing was done.",
                    })
                }
                Some(_) => json!({
                    "ok": true,
                    "done": true,
                    "target": "all seasons of Unexpected (2017)",
                    "entity_id": "019cd7ec-70b9-7950-ad61-b8ff93cd3e4c",
                    "watching": false,
                    "say": "Right, all seasons of Unexpected (2017) is on the way — I'll tell you when it lands.",
                }),
                None => json!({
                    "ok": true,
                    "pending_confirmation": true,
                    "confirm_token": self.offer_token,
                    "confirmed": false,
                    "action": "get",
                    "target": "all seasons of Unexpected (2017)",
                    "say": "I'm about to get all seasons of Unexpected (2017) — say go ahead, or stop if that's wrong.",
                    "expires_in_seconds": 300,
                    "asked_with": args,
                    "note": "nothing has been done yet. Say this sentence to the user and wait for their reply. \
                             Call this tool again with the same ref, confirm='yes' and this confirm_token when they agree, \
                             or confirm='stop' when they do not.",
                }),
            },
            other => json!({"ok": false, "note": format!("no fixture for {other}")}),
        }
    }

    pub fn want_it_calls(&self) -> impl Iterator<Item = &Value> {
        self.calls
            .iter()
            .filter(|(t, _)| t == "want_it")
            .map(|(_, a)| a)
    }
}

impl std::fmt::Debug for Fixture {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Fixture")
            .field("calls", &self.calls)
            .finish()
    }
}

/// One user turn through the real request loop: LLM call, fixture answers,
/// repeat until the model stops calling tools. Returns the final text.
pub async fn run_turn(
    cfg: &LlmConfig,
    messages: &mut Vec<ChatMessage>,
    fixture: &mut Fixture,
) -> Result<String, String> {
    let schemas = tools::schemas();
    for _ in 0..MAX_ROUNDS {
        let completion: Completion = llm::complete(cfg, messages, &schemas, |_| async {}).await?;
        if completion.tool_calls.is_empty() {
            return Ok(completion.content);
        }
        eprintln!(
            "    tool calls: {}",
            completion
                .tool_calls
                .iter()
                .map(|c| format!("{}({})", c.name, c.arguments))
                .collect::<Vec<_>>()
                .join(" ; ")
        );
        let calls = completion.tool_calls.clone();
        messages.push(ChatMessage::assistant_calls(calls));
        for call in &completion.tool_calls {
            let obs = fixture.answer(&call.name, &call.args());
            messages.push(ChatMessage::tool(&call.id, &obs));
        }
    }
    Err(format!("no final text after {MAX_ROUNDS} rounds"))
}

/// One scenario's verdict, with everything a human needs to read the failure.
pub struct ScenarioResult {
    pub name: &'static str,
    pub passed: bool,
    pub detail: String,
}

fn ok(name: &'static str, detail: String) -> ScenarioResult {
    ScenarioResult {
        name,
        passed: true,
        detail,
    }
}

fn fail(name: &'static str, detail: String) -> ScenarioResult {
    ScenarioResult {
        name,
        passed: false,
        detail,
    }
}

fn calls_summary(fixture: &Fixture) -> String {
    fixture
        .calls
        .iter()
        .map(|(t, a)| format!("{t}({a})"))
        .collect::<Vec<_>>()
        .join(" ; ")
}

/// An explicit command resolves, offers, and self-confirms in the same turn —
/// with no invented token, the right season, and the year in the say-sentence.
pub async fn explicit_command(cfg: &LlmConfig) -> ScenarioResult {
    let name = "explicit_command";
    let mut fixture = Fixture::new();
    let mut messages = vec![
        ChatMessage::system(prompt::system_prompt()),
        ChatMessage::user("download all seasons of unexpected 2017"),
    ];
    let text = match run_turn(cfg, &mut messages, &mut fixture).await {
        Ok(t) => t,
        Err(e) => return fail(name, format!("{e}; calls: {}", calls_summary(&fixture))),
    };
    let wants: Vec<&Value> = fixture.want_it_calls().collect();
    let base = format!("calls: {}\nsaid: {text}", calls_summary(&fixture));
    if wants.is_empty() {
        return fail(name, format!("no want_it call at all; {base}"));
    }
    if wants[0].get("confirm").is_some() {
        return fail(
            name,
            format!("first want_it invented a confirmation; {base}"),
        );
    }
    let Some(confirmed) = wants.iter().find(|a| a.get("confirm").is_some()) else {
        return fail(name, format!("the command did not self-confirm; {base}"));
    };
    if confirmed["confirm"]
        .as_str()
        .unwrap_or_default()
        .to_lowercase()
        != "yes"
    {
        return fail(
            name,
            format!("confirm is not an acceptance: {confirmed:?}; {base}"),
        );
    }
    if confirmed.get("confirm_token").and_then(Value::as_str) != Some(fixture.offer_token.as_str())
    {
        return fail(
            name,
            format!("confirm token is not the one the offer handed out; {base}"),
        );
    }
    if wants[0].get("season").and_then(Value::as_str) != Some("all seasons") {
        return fail(name, format!("season arg: {wants:?}; {base}"));
    }
    if !text.to_lowercase().contains("unexpected") || !text.contains("2017") {
        return fail(
            name,
            format!("the year must be named in the say-sentence; {base}"),
        );
    }
    ok(name, base)
}

/// A question gets an answer, not a download.
pub async fn question(cfg: &LlmConfig) -> ScenarioResult {
    let name = "question";
    let mut fixture = Fixture::new();
    let mut messages = vec![
        ChatMessage::system(prompt::system_prompt()),
        ChatMessage::user("what seasons of unexpected 2017 are available?"),
    ];
    let text = match run_turn(cfg, &mut messages, &mut fixture).await {
        Ok(t) => t,
        Err(e) => return fail(name, format!("{e}; calls: {}", calls_summary(&fixture))),
    };
    let base = format!("calls: {}\nsaid: {text}", calls_summary(&fixture));
    if fixture.want_it_calls().next().is_some() {
        return fail(
            name,
            format!("a question must not queue a download; {base}"),
        );
    }
    if !text.to_lowercase().contains("unexpected") && !text.contains("2017") {
        return fail(name, format!("the answer should name the show; {base}"));
    }
    ok(name, base)
}

/// "Stop" after an offer means nothing gets confirmed.
pub async fn stop_after_offer(cfg: &LlmConfig) -> ScenarioResult {
    let name = "stop_after_offer";
    let mut fixture = Fixture::new();
    let mut messages = vec![ChatMessage::system(prompt::system_prompt())];

    // Turn 1: a look, not a command — the model offers and waits.
    messages.push(ChatMessage::user(
        "can you get all seasons of unexpected 2017?",
    ));
    let first = match run_turn(cfg, &mut messages, &mut fixture).await {
        Ok(t) => t,
        Err(e) => return fail(name, format!("{e}; calls: {}", calls_summary(&fixture))),
    };
    if fixture.want_it_calls().next().is_none() {
        return fail(
            name,
            format!(
                "the offer never came; calls: {}\nturn 1 said: {first}",
                calls_summary(&fixture)
            ),
        );
    }

    // Turn 2: "stop" — nothing may be confirmed.
    messages.push(ChatMessage::user("stop"));
    let second = match run_turn(cfg, &mut messages, &mut fixture).await {
        Ok(t) => t,
        Err(e) => return fail(name, format!("{e}; calls: {}", calls_summary(&fixture))),
    };
    let base = format!(
        "calls: {}\nturn 1 said: {first}\nturn 2 said: {second}",
        calls_summary(&fixture)
    );
    if fixture.want_it_calls().any(|a| {
        a.get("confirm")
            .and_then(Value::as_str)
            .is_some_and(|c| !c.eq_ignore_ascii_case("stop") && !c.eq_ignore_ascii_case("no"))
    }) {
        return fail(name, format!("'stop' was confirmed; {base}"));
    }
    ok(name, base)
}

/// Every scenario, in order.
pub async fn run_all(cfg: &LlmConfig) -> Vec<ScenarioResult> {
    vec![
        explicit_command(cfg).await,
        question(cfg).await,
        stop_after_offer(cfg).await,
    ]
}
