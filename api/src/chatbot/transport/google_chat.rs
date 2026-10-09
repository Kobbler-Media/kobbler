//! Google Chat as a [`ChatTransport`].
//!
//! Inbound authentication is the bearer JWT Google signs with
//! `chat@system.gserviceaccount.com` — there is no shared secret. Outbound reuses the
//! Chat REST client the notification agent already owns, so there is one credential and
//! one client for the whole app.

use async_trait::async_trait;
use axum::http::HeaderMap;
use serde::Deserialize;
use serde_json::{Value, json};

use super::{
    Capabilities, ChatConfigField, ChatError, ChatTransport, ChatTransportFactory,
    ChatTransportInfo, ConfigFieldType, EventKind, Inbound, LinkStyle, OutMessage, Target,
};
use crate::google::id_token::{CHAT_ISSUER, IdTokenVerifier};
use crate::google::service_account::ServiceAccountKey;
use crate::notifications::google_chat::{patch_message, post_message};

pub const TRANSPORT_ID: &str = "google_chat";
const CHOICE_FUNCTION: &str = "chat_choice";
const CHOICE_PARAM: &str = "choice_id";

static CONFIG_FIELDS: &[ChatConfigField] = &[
    ChatConfigField {
        key: "project_number",
        label: "Project Number",
        field_type: ConfigFieldType::String,
        required: true,
        description: Some(
            "The numeric Cloud project number of the Chat app. Inbound webhooks are signed with this as the audience, so a wrong value rejects every message",
        ),
        help_url: Some(
            "https://developers.google.com/workspace/chat/authenticate-authorize-chat-app",
        ),
    },
    ChatConfigField {
        key: "service_account_json",
        label: "Service Account Key (JSON)",
        field_type: ConfigFieldType::Secret,
        required: true,
        description: Some(
            "Needed for anything posted after the webhook response — progress edits and completion messages",
        ),
        help_url: None,
    },
];

pub static INFO: ChatTransportInfo = ChatTransportInfo {
    id: TRANSPORT_ID,
    display_name: "Google Chat",
    description: "Talk to the assistant from a Google Chat space or DM",
    config_fields: CONFIG_FIELDS,
    capabilities: Capabilities {
        edit_in_place: true,
        cards: true,
        buttons: true,
        files: false,
    },
};

pub struct GoogleChatTransportFactory;

impl ChatTransportFactory for GoogleChatTransportFactory {
    fn info(&self) -> &'static ChatTransportInfo {
        &INFO
    }

    fn create(&self, _client: reqwest::Client, config: &Value) -> Box<dyn ChatTransport> {
        let string_field = |key: &str| {
            config
                .get(key)
                .and_then(|v| v.as_str())
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(String::from)
        };
        Box::new(GoogleChatTransport {
            audience: string_field("project_number").unwrap_or_default(),
            service_account_json: string_field("service_account_json").unwrap_or_default(),
            // Undeclared on purpose: an escape hatch for a JWK mirror, not a setting.
            jwk_base_url: string_field("jwk_base_url"),
        })
    }
}

pub struct GoogleChatTransport {
    audience: String,
    service_account_json: String,
    jwk_base_url: Option<String>,
}

impl GoogleChatTransport {
    fn key(&self) -> Result<ServiceAccountKey, ChatError> {
        if self.service_account_json.is_empty() {
            return Err(ChatError::NotConfigured(
                "google_chat.service_account_json".into(),
            ));
        }
        ServiceAccountKey::from_json(&self.service_account_json)
            .map_err(|e| ChatError::NotConfigured(format!("google_chat.service_account_json: {e}")))
    }
}

#[async_trait]
impl ChatTransport for GoogleChatTransport {
    fn info(&self) -> &'static ChatTransportInfo {
        &INFO
    }

    async fn verify(&self, headers: &HeaderMap, body: &[u8]) -> Result<Option<Inbound>, ChatError> {
        if self.audience.is_empty() {
            return Err(ChatError::NotConfigured(
                "google_chat.project_number".into(),
            ));
        }

        let token = crate::routes::extract_bearer(headers)
            .ok_or_else(|| ChatError::Unauthorized("missing bearer token".into()))?;

        let verifier = match self.jwk_base_url.as_deref() {
            Some(base) => IdTokenVerifier::shared(base),
            None => IdTokenVerifier::global(),
        };
        verifier
            .verify(&token, &self.audience, CHAT_ISSUER)
            .await
            .map_err(|e| ChatError::Unauthorized(e.to_string()))?;

        let event: ChatEvent = serde_json::from_slice(body)
            .map_err(|e| ChatError::BadPayload(format!("not a Chat event: {e}")))?;

        Ok(normalise(event))
    }

    async fn post(&self, target: &Target, msg: &OutMessage) -> Result<String, ChatError> {
        let key = self.key()?;
        post_message(&key, &target.space, target.thread.as_deref(), &render(msg))
            .await
            .map_err(|e| ChatError::Transport(e.to_string()))
    }

    async fn edit(
        &self,
        _target: &Target,
        message_id: &str,
        msg: &OutMessage,
    ) -> Result<(), ChatError> {
        let key = self.key()?;
        patch_message(&key, message_id, &render(msg))
            .await
            .map_err(|e| ChatError::Transport(e.to_string()))
    }

    fn capabilities(&self) -> Capabilities {
        INFO.capabilities
    }

    /// Google Chat renders `[text](url)` as literal text; `<url|text>` is its syntax.
    fn link_style(&self) -> LinkStyle {
        LinkStyle::AnglePipe
    }

    fn synchronous_reply(&self, msg: &OutMessage) -> Option<Value> {
        Some(render(msg))
    }
}

/// Build the Chat message body. This is the only place card JSON exists.
fn render(msg: &OutMessage) -> Value {
    let Some(choices) = &msg.choices else {
        return json!({ "text": msg.text });
    };

    let buttons: Vec<Value> = choices
        .choices
        .iter()
        .map(|choice| {
            json!({
                "text": choice.label,
                "onClick": {
                    "action": {
                        "function": CHOICE_FUNCTION,
                        "parameters": [{ "key": CHOICE_PARAM, "value": choice.id }],
                    }
                }
            })
        })
        .collect();

    let mut section = json!({ "widgets": [{ "buttonList": { "buttons": buttons } }] });
    // An empty header still renders as a blank row. Buttons that sit under a
    // question already asked in `text` have nothing to put there.
    if !choices.prompt.is_empty()
        && let Some(obj) = section.as_object_mut()
    {
        obj.insert("header".to_string(), json!(choices.prompt));
    }

    json!({
        "text": msg.text,
        "cardsV2": [{
            "cardId": "choices",
            "card": { "sections": [section] }
        }]
    })
}

#[derive(Debug, Deserialize)]
struct ChatEvent {
    #[serde(rename = "type")]
    #[serde(default)]
    kind: String,
    #[serde(default)]
    space: Option<ChatSpace>,
    #[serde(default)]
    message: Option<ChatMessage>,
    #[serde(default)]
    user: Option<ChatUser>,
    #[serde(default)]
    action: Option<ChatAction>,
    #[serde(default)]
    common: Option<ChatCommon>,
}

#[derive(Debug, Deserialize)]
struct ChatSpace {
    #[serde(default)]
    name: Option<String>,
}

#[derive(Debug, Deserialize)]
struct ChatMessage {
    #[serde(default)]
    text: Option<String>,
    #[serde(rename = "argumentText", default)]
    argument_text: Option<String>,
    #[serde(default)]
    thread: Option<ChatThread>,
    #[serde(default)]
    sender: Option<ChatUser>,
    #[serde(default)]
    space: Option<ChatSpace>,
}

#[derive(Debug, Deserialize)]
struct ChatThread {
    #[serde(default)]
    name: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
struct ChatUser {
    #[serde(default)]
    name: Option<String>,
    #[serde(rename = "displayName", default)]
    display_name: Option<String>,
    #[serde(default)]
    email: Option<String>,
    #[serde(rename = "type", default)]
    kind: Option<String>,
}

#[derive(Debug, Deserialize)]
struct ChatAction {
    #[serde(default)]
    parameters: Vec<ChatActionParameter>,
}

#[derive(Debug, Deserialize)]
struct ChatActionParameter {
    #[serde(default)]
    key: String,
    #[serde(default)]
    value: String,
}

#[derive(Debug, Deserialize)]
struct ChatCommon {
    #[serde(default)]
    parameters: Option<std::collections::HashMap<String, String>>,
}

fn normalise(event: ChatEvent) -> Option<Inbound> {
    let event_kind = match event.kind.as_str() {
        "MESSAGE" => EventKind::Message,
        "CARD_CLICKED" => EventKind::Choice,
        _ => return None,
    };

    // `event.user` is the acting human; `message.sender` is the same person on MESSAGE.
    // Either way a bot echoing itself would loop forever, so drop those.
    let sender = event
        .user
        .or_else(|| event.message.as_ref().and_then(|m| m.sender.clone()))?;
    if sender.kind.as_deref() == Some("BOT") {
        return None;
    }

    let space = event.space.and_then(|s| s.name).or_else(|| {
        event
            .message
            .as_ref()
            .and_then(|m| m.space.as_ref())
            .and_then(|s| s.name.clone())
    })?;

    let thread = event
        .message
        .as_ref()
        .and_then(|m| m.thread.as_ref())
        .and_then(|t| t.name.clone());

    let text = event
        .message
        .as_ref()
        .and_then(|m| {
            m.argument_text
                .as_deref()
                .or(m.text.as_deref())
                .map(str::trim)
        })
        .unwrap_or_default()
        .to_string();

    let choice_id = event
        .common
        .and_then(|c| c.parameters)
        .and_then(|p| p.get(CHOICE_PARAM).cloned())
        .or_else(|| {
            event.action.and_then(|a| {
                a.parameters
                    .into_iter()
                    .find(|p| p.key == CHOICE_PARAM)
                    .map(|p| p.value)
            })
        });

    Some(Inbound {
        external_id: sender.name.filter(|n| !n.is_empty()),
        email: sender.email.filter(|e| !e.is_empty()),
        display_name: sender.display_name.filter(|d| !d.is_empty()),
        space,
        thread,
        text,
        event_kind,
        choice_id,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::chatbot::transport::{Choice, Choices};

    fn event(raw: Value) -> Option<Inbound> {
        normalise(serde_json::from_value(raw).expect("valid event"))
    }

    #[test]
    fn message_carries_both_identity_keys_when_google_supplies_them() {
        let inbound = event(json!({
            "type": "MESSAGE",
            "space": { "name": "spaces/AAA" },
            "user": { "name": "users/1234", "displayName": "Her", "email": "her@example.com", "type": "HUMAN" },
            "message": {
                "text": "@kobbler get season 24",
                "argumentText": " get season 24",
                "thread": { "name": "spaces/AAA/threads/T1" }
            }
        }))
        .expect("message is handled");

        assert_eq!(inbound.external_id.as_deref(), Some("users/1234"));
        assert_eq!(inbound.email.as_deref(), Some("her@example.com"));
        assert_eq!(inbound.space, "spaces/AAA");
        assert_eq!(inbound.thread.as_deref(), Some("spaces/AAA/threads/T1"));
        assert_eq!(inbound.text, "get season 24");
        assert_eq!(inbound.event_kind, EventKind::Message);
    }

    #[test]
    fn a_gmail_sender_arrives_with_no_email() {
        let inbound = event(json!({
            "type": "MESSAGE",
            "space": { "name": "spaces/AAA" },
            "user": { "name": "users/9999", "displayName": "Her", "type": "HUMAN" },
            "message": { "text": "hello" }
        }))
        .expect("message is handled");

        assert_eq!(inbound.external_id.as_deref(), Some("users/9999"));
        assert!(inbound.email.is_none());
        assert!(inbound.thread.is_none());
    }

    #[test]
    fn ignored_event_kinds_and_bots_produce_nothing() {
        for kind in ["ADDED_TO_SPACE", "REMOVED_FROM_SPACE", "WIDGET_UPDATED"] {
            assert!(
                event(json!({
                    "type": kind,
                    "space": { "name": "spaces/AAA" },
                    "user": { "name": "users/1", "type": "HUMAN" }
                }))
                .is_none(),
                "{kind} must be ignored"
            );
        }
        assert!(
            event(json!({
                "type": "MESSAGE",
                "space": { "name": "spaces/AAA" },
                "user": { "name": "users/bot", "type": "BOT" },
                "message": { "text": "echo" }
            }))
            .is_none(),
            "the bot must not answer itself"
        );
    }

    #[test]
    fn card_clicks_surface_the_choice_id_from_either_api_shape() {
        let modern = event(json!({
            "type": "CARD_CLICKED",
            "space": { "name": "spaces/AAA" },
            "user": { "name": "users/1234", "type": "HUMAN" },
            "common": { "parameters": { "choice_id": "episode:abc" } }
        }))
        .expect("card click is handled");
        assert_eq!(modern.event_kind, EventKind::Choice);
        assert_eq!(modern.choice_id.as_deref(), Some("episode:abc"));

        let legacy = event(json!({
            "type": "CARD_CLICKED",
            "space": { "name": "spaces/AAA" },
            "user": { "name": "users/1234", "type": "HUMAN" },
            "action": { "parameters": [{ "key": "choice_id", "value": "episode:xyz" }] }
        }))
        .expect("card click is handled");
        assert_eq!(legacy.choice_id.as_deref(), Some("episode:xyz"));
    }

    #[test]
    fn plain_text_renders_without_a_card() {
        let body = render(&OutMessage::text("Looking…"));
        assert_eq!(body["text"], "Looking…");
        assert!(body.get("cardsV2").is_none());
    }

    #[test]
    fn choices_render_as_buttons_carrying_their_ids() {
        let body = render(&OutMessage::with_choices(
            "Found two.",
            Choices {
                prompt: "Which one?".into(),
                choices: vec![Choice {
                    id: "episode:abc".into(),
                    label: "S24E07".into(),
                    sublabel: None,
                }],
            },
        ));
        let section = &body["cardsV2"][0]["card"]["sections"][0];
        assert_eq!(section["header"], "Which one?");
        let button = &section["widgets"][0]["buttonList"]["buttons"][0];
        assert_eq!(button["text"], "S24E07");
        assert_eq!(
            button["onClick"]["action"]["parameters"][0]["value"],
            "episode:abc"
        );
    }

    /// The confirmation sentence is already in `text`; a blank card header above
    /// the buttons is a rendered empty row.
    #[test]
    fn buttons_under_a_question_already_asked_carry_no_header() {
        let body = render(&OutMessage::with_choices(
            "I'm about to get S24E01 — say go ahead, or stop if that's wrong.",
            Choices {
                prompt: String::new(),
                choices: vec![Choice {
                    id: "confirm:tok".into(),
                    label: "Go ahead".into(),
                    sublabel: None,
                }],
            },
        ));
        let section = &body["cardsV2"][0]["card"]["sections"][0];
        assert!(section.get("header").is_none(), "{section}");
        assert_eq!(
            section["widgets"][0]["buttonList"]["buttons"][0]["text"],
            "Go ahead"
        );
    }
}
