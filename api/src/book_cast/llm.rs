use std::time::Duration;

use serde::de::DeserializeOwned;
use serde_json::{Value, json};
use tokio_util::sync::CancellationToken;
use tracing::warn;

use crate::chatbot::llm::{self, ChatMessage, LlmConfig, ThinkingLevel};
use crate::runtime_config::RuntimeConfig;

#[derive(Debug, thiserror::Error)]
pub enum LlmError {
    #[error("koblem is not configured")]
    NoKoblem,
    #[error("{0}")]
    Failed(String),
    #[error("cancelled")]
    Cancelled,
}

/// koblem's one LLM URL: beefy while it holds the box, the resident model otherwise.
pub fn config(config: &RuntimeConfig) -> Result<LlmConfig, LlmError> {
    let url = config
        .koblem_service_url
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .ok_or(LlmError::NoKoblem)?
        .trim_end_matches('/');
    Ok(LlmConfig {
        base_url: format!("{url}/v1"),
        api_key: None,
        model: "koblem".to_string(),
        is_koblem: true,
        thinking: ThinkingLevel::Off,
    })
}

/// One completion constrained to `schema`, parsed into `T`. A failure waits
/// out each of `retry` in turn and asks again; a cancel ends the wait.
pub async fn json<T: DeserializeOwned>(
    cfg: &LlmConfig,
    retry: &[Duration],
    cancel: &CancellationToken,
    system: &str,
    user: &str,
    name: &str,
    schema: Value,
) -> Result<T, LlmError> {
    let mut waits = retry.iter();
    loop {
        let result = tokio::select! {
            () = cancel.cancelled() => return Err(LlmError::Cancelled),
            r = attempt(cfg, system, user, name, schema.clone()) => r,
        };
        match result {
            Err(LlmError::Failed(e)) => match waits.next() {
                Some(wait) => {
                    warn!(error = %e, ?wait, "book cast LLM call failed; retrying");
                    tokio::select! {
                        () = cancel.cancelled() => return Err(LlmError::Cancelled),
                        () = tokio::time::sleep(*wait) => {}
                    }
                }
                None => return Err(LlmError::Failed(e)),
            },
            other => return other,
        }
    }
}

async fn attempt<T: DeserializeOwned>(
    cfg: &LlmConfig,
    system: &str,
    user: &str,
    name: &str,
    schema: Value,
) -> Result<T, LlmError> {
    let messages = [ChatMessage::system(system), ChatMessage::user(user)];
    let mut body = llm::request_body(cfg, &messages, &[]);
    body["temperature"] = json!(0.2);
    body["response_format"] = json!({
        "type": "json_schema",
        "json_schema": {"name": name, "strict": true, "schema": schema},
    });
    let completion = llm::complete_body(cfg, body, |_| async {})
        .await
        .map_err(LlmError::Failed)?;
    parse(&completion.content)
}

fn parse<T: DeserializeOwned>(content: &str) -> Result<T, LlmError> {
    let start = content.find('{');
    let end = content.rfind('}');
    let object = match (start, end) {
        (Some(s), Some(e)) if s < e => &content[s..=e],
        _ => content,
    };
    serde_json::from_str(object).map_err(|e| {
        LlmError::Failed(format!(
            "the model's JSON did not parse: {e}: {}",
            content.chars().take(200).collect::<String>()
        ))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(serde::Deserialize, Debug, PartialEq)]
    struct Reply {
        a: i32,
    }

    #[test]
    fn parses_through_fences_and_chatter() {
        for content in [
            r#"{"a": 1}"#,
            "```json\n{\"a\": 1}\n```",
            "Here you go: {\"a\": 1} hope that helps",
        ] {
            assert_eq!(parse::<Reply>(content).unwrap(), Reply { a: 1 });
        }
        assert!(parse::<Reply>("no json").is_err());
    }
}
