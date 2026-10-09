//! The plugin seam between the agent loop and whatever is carrying the conversation.
//!
//! Shaped after `notifications::NotificationAgentFactory`: a registry of factories, a
//! `ConfigFieldType` schema the settings page renders, and per-instance JSON config.
//! Nothing provider-specific — Google Chat's card JSON must not leak above this module.

pub mod config;
pub mod discord;
pub mod discord_gateway;
pub mod google_chat;
pub mod turn;
pub mod web;

use std::collections::HashMap;

use async_trait::async_trait;
use axum::http::HeaderMap;
use serde::{Deserialize, Serialize};

pub use crate::notifications::ConfigFieldType;

#[derive(Debug, Clone, Serialize)]
pub struct ChatConfigField {
    pub key: &'static str,
    pub label: &'static str,
    pub field_type: ConfigFieldType,
    pub required: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<&'static str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub help_url: Option<&'static str>,
}

/// What a transport can render, so the loop can degrade instead of guessing.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
pub struct Capabilities {
    pub edit_in_place: bool,
    pub cards: bool,
    pub buttons: bool,
    pub files: bool,
}

/// How a transport writes a hyperlink.
///
/// This is not decoration: Google Chat renders `[text](url)` literally, and Discord
/// renders `<url|text>` literally. Getting it wrong puts raw markup in front of the user
/// on every answer that cites anything, which is most of them.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
pub enum LinkStyle {
    /// `[text](url)`. Discord renders these in bot-sent messages, which is all we send.
    #[default]
    Markdown,
    /// `<url|text>`. Google Chat and Slack.
    AnglePipe,
}

impl LinkStyle {
    pub fn render(self, url: &str, text: &str) -> String {
        match self {
            Self::Markdown => format!("[{text}]({url})"),
            Self::AnglePipe => format!("<{url}|{text}>"),
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct ChatTransportInfo {
    pub id: &'static str,
    pub display_name: &'static str,
    pub description: &'static str,
    pub config_fields: &'static [ChatConfigField],
    pub capabilities: Capabilities,
}

#[derive(Debug, thiserror::Error)]
pub enum ChatError {
    #[error("unauthenticated webhook: {0}")]
    Unauthorized(String),
    #[error("malformed payload: {0}")]
    BadPayload(String),
    #[error("transport not configured: {0}")]
    NotConfigured(String),
    #[error("transport request failed: {0}")]
    Transport(String),
}

/// Where a message goes. `space` plus `thread` is the conversation key everywhere.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Target {
    pub transport: String,
    pub space: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thread: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Choice {
    pub id: String,
    pub label: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sublabel: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Choices {
    pub prompt: String,
    pub choices: Vec<Choice>,
}

/// Lowest-common-denominator body: markdown-ish text plus optional structured choices.
///
/// A transport with buttons renders `choices` as buttons; one without renders
/// [`OutMessage::plain_text`], a numbered list the loop can accept "2" as a reply to.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct OutMessage {
    pub text: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub choices: Option<Choices>,
}

impl OutMessage {
    pub fn text(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            choices: None,
        }
    }

    pub fn with_choices(text: impl Into<String>, choices: Choices) -> Self {
        Self {
            text: text.into(),
            choices: Some(choices),
        }
    }

    pub fn plain_text(&self) -> String {
        let Some(choices) = &self.choices else {
            return self.text.clone();
        };

        let mut out = self.text.clone();
        if !out.is_empty() {
            out.push_str("\n\n");
        }
        out.push_str(&choices.prompt);
        for (i, choice) in choices.choices.iter().enumerate() {
            out.push_str(&format!("\n{}. {}", i + 1, choice.label));
            if let Some(sub) = &choice.sublabel {
                out.push_str(&format!(" — {sub}"));
            }
        }
        out
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EventKind {
    Message,
    Choice,
}

/// A normalised inbound event. Both identity keys are optional on purpose — Google Chat
/// only fills `email` in for senders inside the host Workspace domain.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Inbound {
    pub external_id: Option<String>,
    pub email: Option<String>,
    pub display_name: Option<String>,
    pub space: String,
    pub thread: Option<String>,
    pub text: String,
    pub event_kind: EventKind,
    /// The `Choice::id` that was tapped, for [`EventKind::Choice`].
    pub choice_id: Option<String>,
}

impl Inbound {
    pub fn target(&self, transport: &str) -> Target {
        Target {
            transport: transport.to_string(),
            space: self.space.clone(),
            thread: self.thread.clone(),
        }
    }
}

#[async_trait]
pub trait ChatTransport: Send + Sync {
    fn info(&self) -> &'static ChatTransportInfo;

    /// Authenticate an inbound webhook and normalise it. `None` for events we ignore
    /// (ADDED_TO_SPACE, typing, …) — those are not errors.
    async fn verify(&self, headers: &HeaderMap, body: &[u8]) -> Result<Option<Inbound>, ChatError>;

    /// Post a new message into a thread. Returns the provider's message id.
    async fn post(&self, target: &Target, msg: &OutMessage) -> Result<String, ChatError>;

    /// Edit a message in place — how "Searching…" becomes "Grabbed 1080p WEB-DL"
    /// without spamming the thread.
    async fn edit(
        &self,
        target: &Target,
        message_id: &str,
        msg: &OutMessage,
    ) -> Result<(), ChatError>;

    fn capabilities(&self) -> Capabilities;

    /// How to write a hyperlink here. Markdown is the common case; override it only for
    /// a transport that renders markdown links as literal text.
    fn link_style(&self) -> LinkStyle {
        LinkStyle::Markdown
    }

    /// A reply the transport can deliver inside the webhook response instead of a
    /// round trip, when it supports one. Google Chat does; most do not.
    fn synchronous_reply(&self, _msg: &OutMessage) -> Option<serde_json::Value> {
        None
    }
}

pub trait ChatTransportFactory: Send + Sync {
    fn info(&self) -> &'static ChatTransportInfo;
    fn create(&self, client: reqwest::Client, config: &serde_json::Value)
    -> Box<dyn ChatTransport>;
}

pub struct ChatTransportRegistry {
    factories: HashMap<&'static str, Box<dyn ChatTransportFactory>>,
}

impl Default for ChatTransportRegistry {
    fn default() -> Self {
        Self::new()
    }
}

impl ChatTransportRegistry {
    pub fn new() -> Self {
        let mut factories: HashMap<&'static str, Box<dyn ChatTransportFactory>> = HashMap::new();
        factories.insert(
            google_chat::INFO.id,
            Box::new(google_chat::GoogleChatTransportFactory),
        );
        factories.insert(discord::INFO.id, Box::new(discord::DiscordTransportFactory));
        Self { factories }
    }

    pub fn available_types(&self) -> Vec<&'static ChatTransportInfo> {
        let mut infos: Vec<_> = self.factories.values().map(|f| f.info()).collect();
        infos.sort_by_key(|i| i.id);
        infos
    }

    pub fn get_type_info(&self, transport: &str) -> Option<&'static ChatTransportInfo> {
        self.factories.get(transport).map(|f| f.info())
    }

    pub fn create(
        &self,
        transport: &str,
        config: &serde_json::Value,
    ) -> Option<Box<dyn ChatTransport>> {
        let factory = self.factories.get(transport)?;
        Some(factory.create(crate::http::new_client(), config))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn choices_degrade_to_a_numbered_list() {
        let msg = OutMessage::with_choices(
            "Found three.",
            Choices {
                prompt: "Which one?".to_string(),
                choices: vec![
                    Choice {
                        id: "a".to_string(),
                        label: "someShow S24E07".to_string(),
                        sublabel: Some("1080p WEB-DL".to_string()),
                    },
                    Choice {
                        id: "b".to_string(),
                        label: "someShow S24E06".to_string(),
                        sublabel: None,
                    },
                ],
            },
        );
        assert_eq!(
            msg.plain_text(),
            "Found three.\n\nWhich one?\n1. someShow S24E07 — 1080p WEB-DL\n2. someShow S24E06"
        );
    }

    #[test]
    fn plain_text_is_the_body_when_there_are_no_choices() {
        assert_eq!(OutMessage::text("hello").plain_text(), "hello");
    }

    #[test]
    fn each_link_style_writes_the_markup_its_transport_actually_renders() {
        assert_eq!(
            LinkStyle::Markdown.render("https://k/x", "The Stand"),
            "[The Stand](https://k/x)"
        );
        assert_eq!(
            LinkStyle::AnglePipe.render("https://k/x", "The Stand"),
            "<https://k/x|The Stand>"
        );
    }

    /// The two transports disagree, and each renders the other's markup as literal text.
    #[test]
    fn the_registered_transports_do_not_share_a_link_style() {
        let registry = ChatTransportRegistry::new();
        let google = registry
            .create("google_chat", &serde_json::json!({}))
            .expect("registered");
        let discord = registry
            .create("discord", &serde_json::json!({}))
            .expect("registered");
        assert_eq!(google.link_style(), LinkStyle::AnglePipe);
        assert_eq!(discord.link_style(), LinkStyle::Markdown);
    }

    #[test]
    fn google_chat_is_registered_and_described() {
        let registry = ChatTransportRegistry::new();
        assert!(registry.get_type_info("carrier_pigeon").is_none());
        let info = registry.get_type_info("google_chat").expect("registered");
        assert!(info.capabilities.edit_in_place);
        assert!(!info.config_fields.is_empty());
    }
}
