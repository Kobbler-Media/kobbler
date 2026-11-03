//! Streaming chat completions with tool calling, against whichever
//! OpenAI-compatible provider is configured.
//!
//! Multi-provider support is not a nicety here: tool calling on Gemma 4 12B is
//! the main quality risk in the chat bot, and `chat_llm_provider` is what turns
//! that risk into a setting. koblem wins when it is configured — that is the
//! order the Meili-proxy route this replaces used — and everything else falls
//! through to the configured cloud provider.
//!
//! No request timeout, deliberately. A koblem completion goes through
//! `acquire_for_llm` and a running flux/songgen render can evict the LLM
//! entirely, so a legitimate turn can sit for minutes; the codebase's standing
//! rule for GPU-gated calls is a connect timeout and nothing else.

use std::collections::HashMap;
use std::sync::LazyLock;
use std::time::Duration;

use futures::StreamExt as _;
use serde_json::{Value, json};

use crate::runtime_config::RuntimeConfig;

/// Context length assumed when the provider will not tell us. Small enough to
/// be safe against any local model, large enough for a real conversation.
const FALLBACK_CONTEXT: usize = 16_384;
/// Headroom inside the context for the system prompt, the tool schemas and the
/// model's own reply — the Tier 1 window gets what is left.
const CONTEXT_RESERVE: usize = 4_096;

static CLIENT: LazyLock<reqwest::Client> = LazyLock::new(|| {
    reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(10))
        .build()
        .expect("chatbot llm client")
});

#[derive(Debug, Clone, PartialEq)]
pub struct LlmConfig {
    /// Base URL up to and including the version segment, no trailing slash.
    pub base_url: String,
    pub api_key: Option<String>,
    pub model: String,
    /// koblem takes `chat_template_kwargs.enable_thinking` and ignores the
    /// model name; cloud providers do neither.
    pub is_koblem: bool,
    /// Effective thinking level for koblem routes (see [`thinking_level`]).
    /// Inert for cloud providers.
    pub thinking: ThinkingLevel,
}

/// Thinking level for a koblem chat route.
///
/// `Off` keeps the historical `chat_template_kwargs.enable_thinking: false`
/// baseline; the on-levels send a top-level `thinking_budget` in tokens, which
/// the beefy TabbyAPI/exl3 route honours (and streams back as
/// `reasoning_content`, which [`StreamAccumulator`] drops from user text).
/// llama-server's Gemma route reads only known sampling keys, so the extra key
/// is inert there.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ThinkingLevel {
    Off,
    Low,
    Medium,
    High,
}

impl ThinkingLevel {
    /// `thinking_budget` in tokens for the on-levels; `None` for [`Off`].
    pub fn budget(self) -> Option<u32> {
        match self {
            Self::Off => None,
            Self::Low => Some(256),
            Self::Medium => Some(1024),
            Self::High => Some(4096),
        }
    }
}

/// Resolve the effective thinking level: an explicit `chat_llm_thinking`
/// setting wins, and an unrecognised value falls back to the model default —
/// Qwen-family models serve with thinking on (medium), everything else off.
fn thinking_level(model: &str, setting: Option<&str>) -> ThinkingLevel {
    if let Some(s) = setting {
        match s.trim().to_ascii_lowercase().as_str() {
            "off" => return ThinkingLevel::Off,
            "low" => return ThinkingLevel::Low,
            "medium" => return ThinkingLevel::Medium,
            "high" => return ThinkingLevel::High,
            _ => {}
        }
    }
    if model.to_ascii_lowercase().contains("qwen") {
        ThinkingLevel::Medium
    } else {
        ThinkingLevel::Off
    }
}

impl LlmConfig {
    pub fn completions_url(&self) -> String {
        format!("{}/chat/completions", self.base_url)
    }
}

/// Resolve the configured provider, replicating the Meili-proxy route's rules:
/// koblem first when set, then the configured cloud provider, and the same
/// per-provider default model when none is pinned.
pub fn resolve(config: &RuntimeConfig) -> Result<LlmConfig, String> {
    let koblem = config
        .koblem_service_url
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty());
    let api_key = config
        .chat_llm_api_key
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty());

    let model = config
        .chat_llm_model
        .clone()
        .filter(|m| !m.is_empty())
        .unwrap_or_else(|| default_model(koblem.is_some(), config.chat_llm_provider.as_deref()));
    let thinking = thinking_level(&model, config.chat_llm_thinking.as_deref());

    if let Some(url) = koblem {
        return Ok(LlmConfig {
            base_url: format!("{}/v1", url.trim_end_matches('/')),
            api_key: None,
            model,
            is_koblem: true,
            thinking,
        });
    }

    let Some(key) = api_key else {
        return Err(
            "Chat LLM API key not configured (or configure koblem_service_url)".to_string(),
        );
    };
    let base_url = base_url_for(
        config.chat_llm_provider.as_deref(),
        config.chat_llm_base_url.as_deref(),
    )
    .ok_or_else(|| "Chat LLM provider has no base URL — set chat_llm_base_url".to_string())?;

    Ok(LlmConfig {
        base_url,
        api_key: Some(key.to_string()),
        model,
        is_koblem: false,
        thinking,
    })
}

fn default_model(has_koblem: bool, provider: Option<&str>) -> String {
    if has_koblem {
        return "koblem".to_string();
    }
    match provider {
        Some("mistral") => "mistral-small-latest".to_string(),
        Some("openai") => "gpt-4o-mini".to_string(),
        _ => "gemini-2.5-flash".to_string(),
    }
}

fn base_url_for(provider: Option<&str>, explicit: Option<&str>) -> Option<String> {
    if let Some(url) = explicit
        && !url.trim().is_empty()
    {
        return Some(url.trim().trim_end_matches('/').to_string());
    }
    let url = match provider {
        Some("gemini") => "https://generativelanguage.googleapis.com/v1beta/openai",
        Some("openai") => "https://api.openai.com/v1",
        Some("mistral") => "https://api.mistral.ai/v1",
        _ => return None,
    };
    Some(url.to_string())
}

/// One message as the loop holds it, before it becomes wire JSON.
#[derive(Debug, Clone, PartialEq)]
pub struct ChatMessage {
    pub role: String,
    pub content: String,
    pub tool_calls: Vec<ToolCall>,
    pub tool_call_id: Option<String>,
}

impl ChatMessage {
    pub fn system(content: impl Into<String>) -> Self {
        Self::plain("system", content)
    }

    pub fn user(content: impl Into<String>) -> Self {
        Self::plain("user", content)
    }

    pub fn assistant(content: impl Into<String>) -> Self {
        Self::plain("assistant", content)
    }

    fn plain(role: &str, content: impl Into<String>) -> Self {
        Self {
            role: role.to_string(),
            content: content.into(),
            tool_calls: Vec::new(),
            tool_call_id: None,
        }
    }

    pub fn assistant_calls(calls: Vec<ToolCall>) -> Self {
        Self {
            role: "assistant".to_string(),
            content: String::new(),
            tool_calls: calls,
            tool_call_id: None,
        }
    }

    /// A tool observation. The content is compact JSON, never media or prose:
    /// this is what the model reads back, so every byte is prompt budget.
    pub fn tool(call_id: impl Into<String>, observation: &Value) -> Self {
        Self {
            role: "tool".to_string(),
            content: serde_json::to_string(observation).unwrap_or_else(|_| "{}".to_string()),
            tool_calls: Vec::new(),
            tool_call_id: Some(call_id.into()),
        }
    }

    /// Rough token cost. There is no tokenizer here and the number only has to
    /// order a trim, so four bytes per token — an over-estimate for ASCII,
    /// which is the safe direction.
    pub fn estimated_tokens(&self) -> usize {
        let calls: usize = self
            .tool_calls
            .iter()
            .map(|c| c.name.len() + c.arguments.len())
            .sum();
        (self.content.len() + calls + 16).div_ceil(4)
    }

    /// The wire shape. The single place a tool exchange's JSON is decided —
    /// llama.cpp rejects an object `content` on a tool message, and Gemma's
    /// baked template renders object `arguments` to exact DSL where a string is
    /// brace-stripped, so both rules live here.
    pub fn to_wire(&self) -> Value {
        if self.role == "tool" {
            return json!({
                "role": "tool",
                "tool_call_id": self.tool_call_id.clone().unwrap_or_default(),
                "content": self.content,
            });
        }
        if !self.tool_calls.is_empty() {
            return json!({
                "role": "assistant",
                "content": Value::Null,
                "tool_calls": self.tool_calls.iter().map(ToolCall::to_wire).collect::<Vec<_>>(),
            });
        }
        json!({"role": self.role, "content": self.content})
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolCall {
    pub id: String,
    pub name: String,
    /// Raw `arguments` string exactly as the model emitted it.
    pub arguments: String,
}

impl ToolCall {
    pub fn args(&self) -> Value {
        serde_json::from_str(&self.arguments).unwrap_or_else(|_| json!({}))
    }

    fn to_wire(&self) -> Value {
        let arguments = serde_json::from_str::<Value>(&self.arguments)
            .ok()
            .filter(Value::is_object)
            .unwrap_or_else(|| Value::String(self.arguments.clone()));
        json!({
            "id": self.id,
            "type": "function",
            "function": {"name": self.name, "arguments": arguments},
        })
    }
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Completion {
    pub content: String,
    pub tool_calls: Vec<ToolCall>,
    /// `None` when the provider ignored `stream_options.include_usage`.
    pub usage: Option<Usage>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Usage {
    pub prompt_tokens: i32,
    pub completion_tokens: i32,
}

/// Accumulates one streamed completion.
///
/// Two shapes have to be tolerated and both were observed on the live box: the
/// first chunk of a tool call carries `id` and `function.name`, every later
/// chunk carries only `index` and an argument fragment; and `reasoning_content`
/// arrives as its own delta key, which must never reach the user.
#[derive(Debug, Default)]
pub struct StreamAccumulator {
    content: String,
    slots: Vec<ToolCall>,
    by_index: HashMap<u64, usize>,
    usage: Option<Usage>,
}

impl StreamAccumulator {
    pub fn new() -> Self {
        Self::default()
    }

    /// Fold one `data:` payload in. Returns the user-visible text this chunk
    /// added, if any — `reasoning_content` deliberately returns `None`.
    pub fn push_chunk(&mut self, chunk: &Value) -> Option<String> {
        // The usage chunk comes last with `choices: []` (OpenAI, TabbyAPI) or rides
        // on the final delta (llama-server), so it is read before the delta is.
        if let Some(usage) = chunk.get("usage").filter(|u| !u.is_null()) {
            let count = |key: &str| usage.get(key).and_then(Value::as_i64).unwrap_or(0) as i32;
            self.usage = Some(Usage {
                prompt_tokens: count("prompt_tokens"),
                completion_tokens: count("completion_tokens"),
            });
        }
        let delta = chunk.pointer("/choices/0/delta")?;

        if let Some(calls) = delta.get("tool_calls").and_then(Value::as_array) {
            for call in calls {
                self.push_call_fragment(call);
            }
        }

        // Non-streaming responses come back under `message` rather than `delta`;
        // callers that ask for a single JSON body still go through here.
        let text = delta.get("content").and_then(Value::as_str)?;
        if text.is_empty() {
            return None;
        }
        self.content.push_str(text);
        Some(text.to_string())
    }

    fn push_call_fragment(&mut self, call: &Value) {
        let index = call.get("index").and_then(Value::as_u64).unwrap_or(0);
        let slot = match self.by_index.get(&index) {
            Some(slot) => *slot,
            None => {
                self.slots.push(ToolCall {
                    id: String::new(),
                    name: String::new(),
                    arguments: String::new(),
                });
                self.by_index.insert(index, self.slots.len() - 1);
                self.slots.len() - 1
            }
        };
        if let Some(id) = call.get("id").and_then(Value::as_str)
            && !id.is_empty()
        {
            self.slots[slot].id = id.to_string();
        }
        let Some(function) = call.get("function") else {
            return;
        };
        if let Some(name) = function.get("name").and_then(Value::as_str)
            && !name.is_empty()
        {
            self.slots[slot].name = name.to_string();
        }
        match function.get("arguments") {
            Some(Value::String(s)) => self.slots[slot].arguments.push_str(s),
            Some(other) if !other.is_null() => {
                self.slots[slot].arguments.push_str(&other.to_string())
            }
            _ => {}
        }
    }

    pub fn finish(self) -> Completion {
        Completion {
            content: self.content.trim().to_string(),
            // A fragment that never got a name is a provider hiccup, not a call.
            tool_calls: self
                .slots
                .into_iter()
                .filter(|c| !c.name.is_empty())
                .map(|mut c| {
                    if c.id.is_empty() {
                        c.id = format!("call_{}", c.name);
                    }
                    c
                })
                .collect(),
            usage: self.usage,
        }
    }
}

/// Split an SSE buffer into complete `data:` payloads, leaving any partial line
/// in `buf`. `[DONE]` is consumed here — it is a framing marker, not a payload.
// Bytes, not text: a network read can end inside a multi-byte character, and
// only a whole line is guaranteed to be whole UTF-8.
pub fn drain_sse_payloads(buf: &mut Vec<u8>) -> Vec<String> {
    let mut out = Vec::new();
    while let Some(newline) = buf.iter().position(|&b| b == b'\n') {
        let raw: Vec<u8> = buf.drain(..=newline).collect();
        let line = String::from_utf8_lossy(&raw).trim().to_string();
        let Some(data) = line.strip_prefix("data:") else {
            continue;
        };
        let data = data.trim();
        if data.is_empty() || data == "[DONE]" {
            continue;
        }
        out.push(data.to_string());
    }
    out
}

/// The wire boundary's version of "at most one system message, and it is
/// first".
///
/// Qwen-family chat templates (the model's own jinja, applied by whatever
/// runtime serves it) raise `System message must be at the beginning.` on a
/// system message anywhere after index 0, and the error arrives as a 400 that
/// kills the whole turn — so the invariant is enforced here rather than
/// trusted of every caller. System content is joined in order; everything
/// else keeps its position.
fn single_system(messages: &[ChatMessage]) -> Vec<ChatMessage> {
    let mut system = String::new();
    for m in messages {
        if m.role == "system" {
            if !system.is_empty() && !m.content.is_empty() {
                system.push_str("\n\n");
            }
            system.push_str(&m.content);
        }
    }
    if system.is_empty() {
        return messages.to_vec();
    }
    let mut out: Vec<ChatMessage> = messages
        .iter()
        .filter(|m| m.role != "system")
        .cloned()
        .collect();
    out.insert(0, ChatMessage::system(system));
    out
}

/// Run one streaming completion, calling `on_delta` for each piece of
/// user-visible text as it arrives.
pub async fn complete<F, Fut>(
    cfg: &LlmConfig,
    messages: &[ChatMessage],
    tools: &[Value],
    mut on_delta: F,
) -> Result<Completion, String>
where
    F: FnMut(String) -> Fut,
    Fut: std::future::Future<Output = ()>,
{
    let messages = single_system(messages);
    let mut body = json!({
        "model": cfg.model,
        "stream": true,
        "stream_options": {"include_usage": true},
        "messages": messages.iter().map(ChatMessage::to_wire).collect::<Vec<_>>(),
    });
    if !tools.is_empty() {
        body["tools"] = Value::Array(tools.to_vec());
        body["tool_choice"] = json!("auto");
    }
    if cfg.is_koblem {
        match cfg.thinking.budget() {
            None => {
                // Every koblem route baselines thinking off, and a reasoning
                // trace is pure prompt cost on a turn whose output is tool JSON.
                body["chat_template_kwargs"] = json!({"enable_thinking": false});
            }
            Some(budget) => {
                // The beefy route turns this into a reasoning trace; Gemma
                // ignores the unknown key.
                body["thinking_budget"] = json!(budget);
            }
        }
    }

    let mut request = CLIENT.post(cfg.completions_url()).json(&body);
    if let Some(key) = &cfg.api_key {
        request = request.bearer_auth(key);
    }

    let response = request
        .send()
        .await
        .map_err(|e| format!("the language model is unreachable: {e}"))?;
    if !response.status().is_success() {
        let status = response.status();
        let text = response.text().await.unwrap_or_default();
        return Err(format!(
            "the language model returned {status}: {}",
            text.chars().take(400).collect::<String>()
        ));
    }

    let mut acc = StreamAccumulator::new();
    let mut buf = Vec::new();
    let mut stream = response.bytes_stream();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|e| format!("the language model stream failed: {e}"))?;
        buf.extend_from_slice(&chunk);
        for payload in drain_sse_payloads(&mut buf) {
            let Ok(value) = serde_json::from_str::<Value>(&payload) else {
                continue;
            };
            if let Some(error) = value.get("error") {
                let message = error
                    .get("message")
                    .and_then(Value::as_str)
                    .map_or_else(|| error.to_string(), str::to_string);
                return Err(format!("the language model reported an error: {message}"));
            }
            if let Some(text) = acc.push_chunk(&value) {
                on_delta(text).await;
            }
        }
    }
    Ok(acc.finish())
}

/// The context length the configured provider is actually running.
///
/// koblem advertises it on `/v1/models`; a bare llama-server reports it under
/// `/props`. A cloud provider tells us nothing, so the fallback stands.
///
/// Cached per base URL with a TTL rather than probed every turn — the number
/// only changes when an operator swaps models, and two extra HTTP round trips
/// in front of every message is latency for nothing. The TTL is what makes a
/// swap take effect without a restart.
pub async fn context_tokens(cfg: &LlmConfig) -> usize {
    if let Some(hit) = cached_context(&cfg.base_url) {
        return hit;
    }
    let resolved = probe_context(cfg).await.unwrap_or(FALLBACK_CONTEXT);
    remember_context(&cfg.base_url, resolved);
    resolved
}

const CONTEXT_TTL: Duration = Duration::from_secs(300);

static CONTEXT_CACHE: LazyLock<std::sync::Mutex<HashMap<String, (usize, std::time::Instant)>>> =
    LazyLock::new(|| std::sync::Mutex::new(HashMap::new()));

fn cached_context(base_url: &str) -> Option<usize> {
    let cache = CONTEXT_CACHE.lock().ok()?;
    cache
        .get(base_url)
        .filter(|(_, at)| at.elapsed() < CONTEXT_TTL)
        .map(|(n, _)| *n)
}

fn remember_context(base_url: &str, value: usize) {
    if let Ok(mut cache) = CONTEXT_CACHE.lock() {
        cache.insert(base_url.to_string(), (value, std::time::Instant::now()));
    }
}

/// What is left for the Tier 1 window once the prompt's fixed costs are paid.
pub fn window_budget(context: usize) -> usize {
    context.saturating_sub(CONTEXT_RESERVE).max(1_024)
}

async fn probe_context(cfg: &LlmConfig) -> Option<usize> {
    let models = get_json(cfg, &format!("{}/models", cfg.base_url)).await;
    if let Some(v) = models
        && let Some(n) = v
            .pointer("/data/0/context_length")
            .and_then(Value::as_u64)
            .filter(|n| *n > 0)
    {
        return Some(n as usize);
    }
    let root = cfg.base_url.trim_end_matches("/v1");
    let props = get_json(cfg, &format!("{root}/props")).await?;
    props
        .pointer("/default_generation_settings/n_ctx")
        .or_else(|| props.get("n_ctx"))
        .and_then(Value::as_u64)
        .filter(|n| *n > 0)
        .map(|n| n as usize)
}

async fn get_json(cfg: &LlmConfig, url: &str) -> Option<Value> {
    let mut request = CLIENT.get(url).timeout(Duration::from_secs(5));
    if let Some(key) = &cfg.api_key {
        request = request.bearer_auth(key);
    }
    let response = request.send().await.ok()?;
    if !response.status().is_success() {
        return None;
    }
    response.json().await.ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn chunk(delta: Value) -> Value {
        json!({"choices": [{"index": 0, "delta": delta}]})
    }

    fn cfg() -> RuntimeConfig {
        RuntimeConfig::default()
    }

    #[test]
    fn koblem_wins_over_a_configured_cloud_key() {
        let mut c = cfg();
        c.koblem_service_url = Some("http://koblem:8090/".to_string());
        c.chat_llm_provider = Some("openai".to_string());
        c.chat_llm_api_key = Some("sk-test".to_string());
        let resolved = resolve(&c).expect("koblem is enough to resolve");
        assert!(resolved.is_koblem);
        assert_eq!(resolved.base_url, "http://koblem:8090/v1");
        assert_eq!(
            resolved.completions_url(),
            "http://koblem:8090/v1/chat/completions"
        );
        assert_eq!(resolved.model, "koblem");
        assert!(resolved.api_key.is_none());
    }

    #[test]
    fn each_provider_keeps_the_model_default_the_meili_route_used() {
        for (provider, model, base) in [
            (
                "gemini",
                "gemini-2.5-flash",
                "https://generativelanguage.googleapis.com/v1beta/openai",
            ),
            ("openai", "gpt-4o-mini", "https://api.openai.com/v1"),
            (
                "mistral",
                "mistral-small-latest",
                "https://api.mistral.ai/v1",
            ),
        ] {
            let mut c = cfg();
            c.chat_llm_provider = Some(provider.to_string());
            c.chat_llm_api_key = Some("key".to_string());
            let resolved = resolve(&c).expect("a keyed provider resolves");
            assert_eq!(resolved.model, model, "{provider}");
            assert_eq!(resolved.base_url, base, "{provider}");
            assert!(!resolved.is_koblem);
        }
    }

    #[test]
    fn an_explicit_base_url_overrides_the_provider_default() {
        let mut c = cfg();
        c.chat_llm_provider = Some("openai".to_string());
        c.chat_llm_api_key = Some("key".to_string());
        c.chat_llm_base_url = Some("http://localhost:8092/v1/".to_string());
        let resolved = resolve(&c).unwrap();
        assert_eq!(resolved.base_url, "http://localhost:8092/v1");
    }

    #[test]
    fn no_koblem_and_no_key_is_an_error_not_a_guess() {
        let mut c = cfg();
        c.chat_llm_provider = Some("openai".to_string());
        assert!(resolve(&c).is_err());
    }

    /// The live box sends the id and name once, then argument fragments keyed
    /// only by `index`. Losing that mapping is how a tool call arrives with an
    /// empty name and silently does nothing.
    #[test]
    fn a_tool_call_reassembles_from_fragments_that_only_carry_an_index() {
        let mut acc = StreamAccumulator::new();
        acc.push_chunk(&chunk(json!({"role": "assistant", "content": null})));
        acc.push_chunk(&chunk(json!({"tool_calls": [{
            "index": 0, "id": "p6iv5", "type": "function",
            "function": {"name": "search_library", "arguments": "{"},
        }]})));
        for fragment in ["\"query", "\":", "\"horror", "\"", "}"] {
            acc.push_chunk(&chunk(
                json!({"tool_calls": [{"index": 0, "function": {"arguments": fragment}}]}),
            ));
        }
        let done = acc.finish();
        assert_eq!(done.tool_calls.len(), 1);
        assert_eq!(done.tool_calls[0].name, "search_library");
        assert_eq!(done.tool_calls[0].id, "p6iv5");
        assert_eq!(done.tool_calls[0].args()["query"], "horror");
        assert!(done.content.is_empty());
    }

    #[test]
    fn parallel_calls_stay_separate_by_index() {
        let mut acc = StreamAccumulator::new();
        acc.push_chunk(&chunk(json!({"tool_calls": [
            {"index": 0, "id": "a", "function": {"name": "search_library", "arguments": "{\"query\":\"x\"}"}},
            {"index": 1, "id": "b", "function": {"name": "library_stats", "arguments": "{}"}},
        ]})));
        let done = acc.finish();
        assert_eq!(done.tool_calls.len(), 2);
        assert_eq!(done.tool_calls[0].id, "a");
        assert_eq!(done.tool_calls[1].name, "library_stats");
    }

    #[test]
    fn usage_is_read_from_the_trailing_chunk_with_no_choices() {
        let mut acc = StreamAccumulator::new();
        acc.push_chunk(&chunk(json!({"content": "Hi"})));
        let trailer =
            json!({"choices": [], "usage": {"prompt_tokens": 6956, "completion_tokens": 45}});
        assert_eq!(acc.push_chunk(&trailer), None);
        assert_eq!(
            acc.finish().usage,
            Some(Usage {
                prompt_tokens: 6956,
                completion_tokens: 45
            })
        );
    }

    #[test]
    fn reasoning_content_never_becomes_user_visible_text() {
        let mut acc = StreamAccumulator::new();
        assert_eq!(
            acc.push_chunk(&chunk(json!({"reasoning_content": "The user wants"}))),
            None
        );
        assert_eq!(
            acc.push_chunk(&chunk(json!({"content": "Here you go"}))),
            Some("Here you go".to_string())
        );
        let done = acc.finish();
        assert_eq!(done.content, "Here you go");
        assert!(!done.content.contains("The user wants"));
    }

    #[test]
    fn sse_payloads_survive_a_chunk_boundary_mid_line() {
        let mut buf = Vec::new();
        buf.extend_from_slice(b"data: {\"a\":");
        assert!(drain_sse_payloads(&mut buf).is_empty());
        buf.extend_from_slice(b"1}\n\ndata: [DONE]\n\n");
        assert_eq!(drain_sse_payloads(&mut buf), vec!["{\"a\":1}".to_string()]);
        assert!(buf.is_empty());
    }

    #[test]
    fn sse_payloads_survive_a_chunk_boundary_mid_character() {
        let line = "data: {\"t\":\"café\"}\n\n".as_bytes();
        let split = line.iter().position(|&b| b == 0xC3).unwrap() + 1;
        let mut buf = Vec::new();
        buf.extend_from_slice(&line[..split]);
        assert!(drain_sse_payloads(&mut buf).is_empty());
        buf.extend_from_slice(&line[split..]);
        assert_eq!(
            drain_sse_payloads(&mut buf),
            vec!["{\"t\":\"café\"}".to_string()]
        );
    }

    #[test]
    fn a_tool_exchange_serialises_to_the_shape_llama_cpp_accepts() {
        let calls = vec![ToolCall {
            id: "c1".into(),
            name: "search_library".into(),
            arguments: r#"{"query":"carrie"}"#.into(),
        }];
        let assistant = ChatMessage::assistant_calls(calls).to_wire();
        assert_eq!(assistant["content"], Value::Null);
        assert!(
            assistant["tool_calls"][0]["function"]["arguments"].is_object(),
            "object arguments render to exact Gemma DSL; a string is brace-stripped"
        );

        let observation = ChatMessage::tool("c1", &json!({"ok": true})).to_wire();
        assert!(
            observation["content"].is_string(),
            "llama.cpp 400s on an object tool content"
        );
        assert_eq!(observation["tool_call_id"], "c1");
    }

    #[test]
    fn unparseable_arguments_go_back_verbatim_rather_than_being_dropped() {
        let msg = ChatMessage::assistant_calls(vec![ToolCall {
            id: "c1".into(),
            name: "x".into(),
            arguments: "{not json".into(),
        }]);
        assert_eq!(
            msg.to_wire()["tool_calls"][0]["function"]["arguments"],
            "{not json"
        );
    }

    #[test]
    fn the_window_budget_leaves_headroom_and_never_goes_to_zero() {
        assert_eq!(window_budget(73_728), 73_728 - CONTEXT_RESERVE);
        assert_eq!(window_budget(2_048), 1_024);
    }

    /// The turn loop used to send the memory prompt as a second leading system
    /// message, which the Qwen template 400s on and the Gemma template
    /// silently accepts — the split is exactly why the bug only appeared when
    /// a Qwen model was pointed at the bot.
    #[test]
    fn extra_system_messages_fold_into_one_at_the_front() {
        let messages = vec![
            ChatMessage::system("base"),
            ChatMessage::system("memory"),
            ChatMessage::user("hi"),
        ];
        let merged = single_system(&messages);
        assert_eq!(merged.len(), 2);
        assert_eq!(merged[0].role, "system");
        assert_eq!(merged[0].content, "base\n\nmemory");
        assert_eq!(merged[1].role, "user");
    }

    #[test]
    fn a_system_message_late_in_the_list_moves_to_the_front() {
        let messages = vec![ChatMessage::user("hi"), ChatMessage::system("late")];
        let merged = single_system(&messages);
        assert_eq!(merged.len(), 2);
        assert_eq!(merged[0].role, "system");
        assert_eq!(merged[0].content, "late");
        assert_eq!(merged[1].role, "user");
    }

    #[test]
    fn a_system_free_list_passes_through_untouched() {
        let messages = vec![ChatMessage::user("hi"), ChatMessage::assistant("there")];
        assert_eq!(single_system(&messages), messages);
    }

    #[test]
    fn the_model_default_is_medium_for_qwen_and_off_for_everything_else() {
        assert_eq!(
            thinking_level("Qwen3-32B-Instruct", None),
            ThinkingLevel::Medium
        );
        assert_eq!(thinking_level("qwen2.5-7b", None), ThinkingLevel::Medium);
        assert_eq!(thinking_level("gemma-4-12b", None), ThinkingLevel::Off);
        assert_eq!(thinking_level("koblem", None), ThinkingLevel::Off);
    }

    #[test]
    fn an_explicit_setting_wins_over_the_model_default() {
        assert_eq!(
            thinking_level("Qwen3-32B-Instruct", Some("off")),
            ThinkingLevel::Off
        );
        assert_eq!(
            thinking_level("gemma-4-12b", Some("high")),
            ThinkingLevel::High
        );
        assert_eq!(
            thinking_level("gemma-4-12b", Some("low")),
            ThinkingLevel::Low
        );
        assert_eq!(
            thinking_level("Qwen3-32B-Instruct", Some("medium")),
            ThinkingLevel::Medium
        );
    }

    #[test]
    fn the_setting_is_case_insensitive_and_an_unknown_value_falls_back_to_the_model() {
        assert_eq!(
            thinking_level("Qwen3-32B-Instruct", Some("OFF")),
            ThinkingLevel::Off
        );
        assert_eq!(
            thinking_level("Qwen3-32B-Instruct", Some("  high ")),
            ThinkingLevel::High
        );
        assert_eq!(
            thinking_level("Qwen3-32B-Instruct", Some("turbo")),
            ThinkingLevel::Medium
        );
        assert_eq!(
            thinking_level("gemma-4-12b", Some("turbo")),
            ThinkingLevel::Off
        );
    }

    #[test]
    fn the_on_levels_map_to_their_budgets_and_off_has_none() {
        assert_eq!(ThinkingLevel::Off.budget(), None);
        assert_eq!(ThinkingLevel::Low.budget(), Some(256));
        assert_eq!(ThinkingLevel::Medium.budget(), Some(1024));
        assert_eq!(ThinkingLevel::High.budget(), Some(4096));
    }

    #[test]
    fn resolve_carries_the_effective_thinking_level_into_the_config() {
        let mut c = cfg();
        c.koblem_service_url = Some("http://koblem:8090".to_string());
        c.chat_llm_model = Some("Qwen3-32B-Instruct".to_string());
        assert_eq!(
            resolve(&c).unwrap().thinking,
            ThinkingLevel::Medium,
            "qwen model, no setting"
        );
        c.chat_llm_thinking = Some("off".to_string());
        assert_eq!(
            resolve(&c).unwrap().thinking,
            ThinkingLevel::Off,
            "explicit off wins"
        );
        c.chat_llm_model = Some("gemma-4-12b".to_string());
        c.chat_llm_thinking = None;
        assert_eq!(
            resolve(&c).unwrap().thinking,
            ThinkingLevel::Off,
            "non-qwen model, no setting"
        );
    }
}
