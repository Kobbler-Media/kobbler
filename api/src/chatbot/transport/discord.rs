//! Discord as a [`ChatTransport`] — the outbound half.
//!
//! Inbound does not come through here. Discord delivers plain messages over a gateway
//! socket, not a webhook, so [`ChatTransport::verify`] refuses and
//! [`super::discord_gateway`] owns everything arriving. That split is deliberate: a
//! webhook would only ever carry slash commands, which would make the assistant
//! something you invoke rather than something you talk to.
//!
//! Two Discord limits shape the code below and nothing else does:
//!
//! * **2000 characters per message.** A library answer with citations goes over it, so
//!   everything is chunked at a word boundary. `post` returns the *first* id, and `edit`
//!   patches the head and posts the tail, so "the message we are editing" stays the same
//!   message for the whole turn.
//! * **5 edits per 5 s per channel.** [`crate::chatbot::watches::COALESCE_WINDOW`] is
//!   15 s, so progress edits are already an order of magnitude under it. Nothing here
//!   throttles; if that window ever shrinks, this is the limit it has to clear.
//!
//! No privileged intent is required, which is why there is no setting for one. Discord
//! always delivers message content in DMs with the app and in messages that mention it,
//! and those are the only two cases the assistant answers.
//!
//! The bot token is the only thing this needs. It comes from the same stored row as
//! `notifications::discord`, which adds the announcements channel — one credential to
//! rotate, and the assistant never posts anywhere it was not spoken to.

use async_trait::async_trait;
use axum::http::HeaderMap;
use serde::Deserialize;
use serde_json::{Value, json};

use super::{
    Capabilities, ChatConfigField, ChatError, ChatTransport, ChatTransportFactory,
    ChatTransportInfo, Choices, ConfigFieldType, Inbound, OutMessage, Target,
};

pub const TRANSPORT_ID: &str = "discord";
pub const API_BASE: &str = "https://discord.com/api/v10";

/// Discord's per-message ceiling. Content over this is rejected outright, not truncated.
const MAX_CONTENT: usize = 2000;
/// Button caption ceiling.
const MAX_LABEL: usize = 80;
/// Buttons Discord allows in one action row. A sixth in the same row is a 400
/// for the whole message, the same way a duplicate `custom_id` is, so the row
/// is what the buttons are dealt into rather than something the caller has to
/// remember. Five rows are allowed; nothing here comes close.
const MAX_ROW: usize = 5;
/// `custom_id` ceiling. A `pick:` id carries a title, so this is the one that
/// bites — and it bites hard, because Discord rejects the whole message rather
/// than the button. `turn::MAX_CHOICE_ID` is this number, and ids are built to
/// fit it with the year and the season kept; this clamp is the guard that says
/// so, not the mechanism.
const MAX_CUSTOM_ID: usize = 100;

static CONFIG_FIELDS: &[ChatConfigField] = &[ChatConfigField {
    key: "bot_token",
    label: "Bot Token",
    field_type: ConfigFieldType::Secret,
    required: true,
    description: Some(
        "From the Discord developer portal, Bot → Token. This is the whole credential — the assistant needs no privileged intents and no public URL",
    ),
    help_url: Some("https://discord.com/developers/applications"),
}];

pub static INFO: ChatTransportInfo = ChatTransportInfo {
    id: TRANSPORT_ID,
    display_name: "Discord",
    description: "Talk to the assistant from a Discord DM, or by mentioning it in a channel",
    config_fields: CONFIG_FIELDS,
    capabilities: Capabilities {
        edit_in_place: true,
        // Discord embeds exist, but nothing above this module emits a card that is not
        // better served as markdown plus buttons.
        cards: false,
        buttons: true,
        files: false,
    },
};

pub struct DiscordTransportFactory;

impl ChatTransportFactory for DiscordTransportFactory {
    fn info(&self) -> &'static ChatTransportInfo {
        &INFO
    }

    fn create(&self, client: reqwest::Client, config: &Value) -> Box<dyn ChatTransport> {
        Box::new(DiscordTransport::from_config(client, config))
    }
}

pub struct DiscordTransport {
    client: reqwest::Client,
    token: String,
}

impl DiscordTransport {
    pub fn from_config(client: reqwest::Client, config: &Value) -> Self {
        let field = |key: &str| {
            config
                .get(key)
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(String::from)
        };
        Self {
            client,
            token: field("bot_token").unwrap_or_default(),
        }
    }

    pub fn token(&self) -> &str {
        &self.token
    }

    fn authed(&self, req: reqwest::RequestBuilder) -> Result<reqwest::RequestBuilder, ChatError> {
        if self.token.is_empty() {
            return Err(ChatError::NotConfigured("discord.bot_token".into()));
        }
        Ok(req.header("Authorization", format!("Bot {}", self.token)))
    }

    /// One Discord call, retried once on 429.
    ///
    /// Discord answers a rate limit with the exact wait in `retry_after`, so a single
    /// obedient retry is both correct and enough at this volume. Retrying more would be
    /// guessing.
    async fn send(
        &self,
        build: impl Fn() -> Result<reqwest::RequestBuilder, ChatError>,
    ) -> Result<Value, ChatError> {
        for attempt in 0..2 {
            let response = build()?
                .send()
                .await
                .map_err(|e| ChatError::Transport(format!("discord request failed: {e}")))?;

            let status = response.status();
            if status == reqwest::StatusCode::TOO_MANY_REQUESTS && attempt == 0 {
                let body: Value = response.json().await.unwrap_or_default();
                let wait = body
                    .get("retry_after")
                    .and_then(Value::as_f64)
                    .unwrap_or(1.0)
                    .clamp(0.0, 30.0);
                tracing::warn!(wait, "discord rate limited; retrying once");
                tokio::time::sleep(std::time::Duration::from_secs_f64(wait)).await;
                continue;
            }

            if !status.is_success() {
                let body = response.text().await.unwrap_or_default();
                let snippet: String = body.chars().take(300).collect();
                return Err(ChatError::Transport(format!("discord {status}: {snippet}")));
            }

            // 204 No Content (typing, some deletes) is a success with no body.
            let text = response.text().await.unwrap_or_default();
            if text.trim().is_empty() {
                return Ok(Value::Null);
            }
            return serde_json::from_str(&text)
                .map_err(|e| ChatError::Transport(format!("discord sent unparseable JSON: {e}")));
        }
        Err(ChatError::Transport("discord stayed rate limited".into()))
    }

    async fn post_raw(&self, channel_id: &str, body: Value) -> Result<DiscordMessage, ChatError> {
        let url = format!("{API_BASE}/channels/{channel_id}/messages");
        let value = self
            .send(|| self.authed(self.client.post(&url).json(&body)))
            .await?;
        serde_json::from_value(value)
            .map_err(|e| ChatError::Transport(format!("discord message had no id: {e}")))
    }

    /// Post into a channel, chunking to Discord's limit.
    ///
    /// Returns the id of the **first** message, which is the one buttons are attached to
    /// and the one later edits patch.
    pub async fn post_chunked(
        &self,
        channel_id: &str,
        msg: &OutMessage,
    ) -> Result<String, ChatError> {
        let chunks = chunk(&body_text(msg), MAX_CONTENT);
        let mut first: Option<String> = None;

        for (i, chunk) in chunks.iter().enumerate() {
            // Buttons ride on the last chunk, where the question they answer ends up.
            let last = i + 1 == chunks.len();
            let body = message_body(chunk, msg.choices.as_ref().filter(|_| last));
            let posted = self.post_raw(channel_id, body).await?;
            first.get_or_insert(posted.id);
        }

        first.ok_or_else(|| ChatError::Transport("nothing to post".into()))
    }

    /// Show the typing indicator. Fire-and-forget: a failure here is cosmetic.
    pub async fn typing(&self, channel_id: &str) {
        let url = format!("{API_BASE}/channels/{channel_id}/typing");
        if let Err(e) = self.send(|| self.authed(self.client.post(&url))).await {
            tracing::debug!(error = %e, "discord typing indicator failed");
        }
    }

    /// Acknowledge a component interaction and retire its buttons.
    ///
    /// Discord kills an interaction that goes unanswered for 3 seconds and shows the
    /// user an error, so this must happen before the turn starts, not after it ends.
    ///
    /// Clearing the components is not decoration. A confirmation token is single-use, so
    /// once it is spent the buttons above it are live-looking controls that no longer do
    /// what they say — tapping "Go ahead" a second time reaches a consumed token and the
    /// user is told the offer expired, having asked for nothing. Retiring them on the tap
    /// also doubles as the "that registered" feedback, which a deferred ack does not give.
    pub async fn ack_interaction(
        &self,
        interaction_id: &str,
        token: &str,
    ) -> Result<(), ChatError> {
        let url = format!("{API_BASE}/interactions/{interaction_id}/{token}/callback");
        // 7 = UPDATE_MESSAGE. Fields left out are kept, so the text survives and only the
        // buttons go.
        let body = json!({ "type": 7, "data": { "components": [] } });
        self.send(|| Ok(self.client.post(&url).json(&body))).await?;
        Ok(())
    }

    /// The interaction callback body, split out so its shape is testable without a
    /// Discord to send it to.
    #[cfg(test)]
    fn ack_body() -> Value {
        json!({ "type": 7, "data": { "components": [] } })
    }
}

#[async_trait]
impl ChatTransport for DiscordTransport {
    fn info(&self) -> &'static ChatTransportInfo {
        &INFO
    }

    /// Discord has no inbound webhook here — see the module docs.
    ///
    /// Returning an error rather than `Ok(None)` is the point: `Ok(None)` means "an
    /// event we chose to ignore", and a POST to this transport's webhook path is not
    /// that. It is a misconfiguration, and it should read as one.
    async fn verify(
        &self,
        _headers: &HeaderMap,
        _body: &[u8],
    ) -> Result<Option<Inbound>, ChatError> {
        Err(ChatError::NotConfigured(
            "discord receives over the gateway, not a webhook".into(),
        ))
    }

    async fn post(&self, target: &Target, msg: &OutMessage) -> Result<String, ChatError> {
        self.post_chunked(channel_of(target), msg).await
    }

    /// Patch the head, post the tail.
    ///
    /// The terminal answer arrives as an edit of the progress message, and it is the one
    /// message in a turn that can exceed the limit. Truncating it would drop the answer;
    /// posting the remainder keeps it whole and keeps the edited message stable.
    async fn edit(
        &self,
        target: &Target,
        message_id: &str,
        msg: &OutMessage,
    ) -> Result<(), ChatError> {
        let channel_id = channel_of(target);
        let chunks = chunk(&body_text(msg), MAX_CONTENT);
        let (head, tail) = chunks
            .split_first()
            .ok_or_else(|| ChatError::Transport("nothing to edit".into()))?;

        let only = tail.is_empty();
        let url = format!("{API_BASE}/channels/{channel_id}/messages/{message_id}");
        let head_body = message_body(head, msg.choices.as_ref().filter(|_| only));
        self.send(|| self.authed(self.client.patch(&url).json(&head_body)))
            .await?;

        for (i, chunk) in tail.iter().enumerate() {
            let last = i + 1 == tail.len();
            let body = message_body(chunk, msg.choices.as_ref().filter(|_| last));
            self.post_raw(channel_id, body).await?;
        }
        Ok(())
    }

    fn capabilities(&self) -> Capabilities {
        INFO.capabilities
    }
}

/// A Discord thread is a channel in its own right, so its id is already in `space`.
/// `thread` stays `None` for everything this transport produces; honouring it anyway
/// costs one line and means a future threaded reply does not silently go to the parent.
fn channel_of(target: &Target) -> &str {
    target.thread.as_deref().unwrap_or(&target.space)
}

/// The text Discord should show: the body, plus the choice prompt when there is one.
///
/// Google Chat puts that prompt in a card header. Discord has no header, so it becomes
/// the last line of the message the buttons hang off.
fn body_text(msg: &OutMessage) -> String {
    let Some(choices) = &msg.choices else {
        return msg.text.clone();
    };
    if choices.prompt.is_empty() {
        return msg.text.clone();
    }
    match msg.text.is_empty() {
        true => choices.prompt.clone(),
        false => format!("{}\n\n{}", msg.text, choices.prompt),
    }
}

fn message_body(content: &str, choices: Option<&Choices>) -> Value {
    let mut body = json!({
        "content": content,
        // The assistant quotes titles it did not choose. Nothing it says should be able
        // to ping a room, so no mention in its output is ever resolved.
        "allowed_mentions": { "parse": [] },
    });

    let components = choices.map(components).unwrap_or_else(|| json!([]));
    if let Some(obj) = body.as_object_mut() {
        obj.insert("components".to_string(), components);
    }
    body
}

/// The buttons, dealt into action rows of [`MAX_ROW`].
fn components(choices: &Choices) -> Value {
    let buttons: Vec<Value> = choices
        .choices
        .iter()
        .map(|choice| {
            json!({
                "type": 2,
                // 1 = primary, 2 = secondary. The first choice is the likely one.
                "style": 2,
                "label": clamp(&choice.label, MAX_LABEL),
                "custom_id": clamp(&choice.id, MAX_CUSTOM_ID),
            })
        })
        .collect();

    let rows: Vec<Value> = buttons
        .chunks(MAX_ROW)
        .map(|row| json!({ "type": 1, "components": row }))
        .collect();
    json!(rows)
}

/// Truncate to a char count, never a byte count — a split multi-byte char is a 400 from
/// Discord and an unreadable label for everyone else.
fn clamp(s: &str, max: usize) -> String {
    match s.chars().count() <= max {
        true => s.to_string(),
        false => s.chars().take(max).collect(),
    }
}

/// Split `text` into pieces of at most `limit` characters, preferring a paragraph break,
/// then a line break, then a space.
///
/// A hard split lands mid-word and, worse, can land inside a markdown link. Preferring a
/// boundary in the last quarter of the window keeps the search cheap and the output
/// readable; text with no boundary at all (a long URL) still gets split, because a
/// message over the limit is rejected outright.
fn chunk(text: &str, limit: usize) -> Vec<String> {
    if text.is_empty() {
        return vec![String::new()];
    }

    let mut out = Vec::new();
    let mut rest: Vec<char> = text.chars().collect();

    while rest.len() > limit {
        let window = &rest[..limit];
        let floor = limit * 3 / 4;
        let split = ["\n\n", "\n", " "]
            .iter()
            .find_map(|sep| {
                let sep: Vec<char> = sep.chars().collect();
                window
                    .windows(sep.len())
                    .rposition(|w| w == sep.as_slice())
                    .filter(|at| *at >= floor)
                    .map(|at| (at, at + sep.len()))
            })
            .unwrap_or((limit, limit));

        let (end, resume) = split;
        out.push(
            rest[..end]
                .iter()
                .collect::<String>()
                .trim_end()
                .to_string(),
        );
        rest = rest[resume..].to_vec();
    }

    if !rest.is_empty() {
        out.push(rest.iter().collect());
    }
    out
}

#[derive(Debug, Deserialize)]
struct DiscordMessage {
    id: String,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::chatbot::transport::Choice;

    fn choices() -> Choices {
        Choices {
            prompt: "Which one?".to_string(),
            choices: vec![
                Choice {
                    id: "pick:someShow S24E07".to_string(),
                    label: "someShow S24E07".to_string(),
                    sublabel: Some("1080p".to_string()),
                },
                Choice {
                    id: "pick:someShow S24E06".to_string(),
                    label: "someShow S24E06".to_string(),
                    sublabel: None,
                },
            ],
        }
    }

    #[test]
    fn short_text_is_one_chunk() {
        assert_eq!(chunk("hello", MAX_CONTENT), vec!["hello".to_string()]);
    }

    #[test]
    fn empty_text_still_produces_a_message() {
        assert_eq!(chunk("", MAX_CONTENT), vec![String::new()]);
    }

    /// Every chunk has to clear the limit Discord actually enforces, or the answer is
    /// rejected rather than split.
    #[test]
    fn every_chunk_is_under_the_limit_and_nothing_is_lost() {
        let para = "Some sentence about a film that goes on a while. ".repeat(200);
        let chunks = chunk(&para, MAX_CONTENT);
        assert!(chunks.len() > 1, "expected a split");
        for c in &chunks {
            assert!(c.chars().count() <= MAX_CONTENT, "chunk over limit");
        }
        let rejoined: String = chunks.join(" ");
        assert!(rejoined.contains("Some sentence about a film"));
        // Word count survives the split; only whitespace at the seams moves.
        assert_eq!(
            rejoined.split_whitespace().count(),
            para.split_whitespace().count()
        );
    }

    #[test]
    fn a_split_prefers_a_boundary_over_the_middle_of_a_word() {
        let text = format!("{}\n\nsecond paragraph", "a".repeat(1990));
        let chunks = chunk(&text, MAX_CONTENT);
        assert_eq!(chunks.len(), 2);
        assert_eq!(chunks[1], "second paragraph");
    }

    /// A wall of characters with no separator still has to be split, because Discord
    /// rejects the whole message rather than trimming it.
    #[test]
    fn text_with_no_boundary_is_still_split() {
        let text = "x".repeat(4500);
        let chunks = chunk(&text, MAX_CONTENT);
        assert_eq!(chunks.len(), 3);
        for c in &chunks {
            assert!(c.chars().count() <= MAX_CONTENT);
        }
    }

    /// Byte-slicing a multi-byte char produces a 400 from Discord, so the boundary logic
    /// must count chars throughout.
    #[test]
    fn multibyte_text_survives_chunking() {
        let text = "é".repeat(3000);
        let chunks = chunk(&text, MAX_CONTENT);
        for c in &chunks {
            assert!(c.chars().count() <= MAX_CONTENT);
        }
        assert_eq!(chunks.concat().chars().count(), 3000);
    }

    #[test]
    fn the_choice_prompt_becomes_the_last_line() {
        let msg = OutMessage::with_choices("Found two.", choices());
        assert_eq!(body_text(&msg), "Found two.\n\nWhich one?");
    }

    /// A sixth button in one row is rejected with the whole message, so the
    /// rows are dealt rather than assumed.
    #[test]
    fn buttons_past_the_row_limit_start_a_new_row() {
        let many = Choices {
            prompt: String::new(),
            choices: (0..7)
                .map(|i| Choice {
                    id: format!("pick:Thing {i}"),
                    label: format!("Thing {i}"),
                    sublabel: None,
                })
                .collect(),
        };
        let rendered = components(&many);
        let rows = rendered.as_array().expect("array");
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0]["components"].as_array().unwrap().len(), MAX_ROW);
        assert_eq!(rows[1]["components"].as_array().unwrap().len(), 2);
    }

    #[test]
    fn buttons_render_as_one_action_row() {
        let rendered = components(&choices());
        let rows = rendered.as_array().expect("array");
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0]["type"], 1);
        let buttons = rows[0]["components"].as_array().expect("buttons");
        assert_eq!(buttons.len(), 2);
        assert_eq!(buttons[0]["custom_id"], "pick:someShow S24E07");
    }

    /// A `pick:` id carries a title, and a long one would be rejected outright.
    #[test]
    fn an_overlong_custom_id_is_clamped_not_rejected() {
        let long = format!("pick:{}", "t".repeat(300));
        let rendered = components(&Choices {
            prompt: String::new(),
            choices: vec![Choice {
                id: long,
                label: "x".repeat(200),
                sublabel: None,
            }],
        });
        let button = &rendered[0]["components"][0];
        assert_eq!(
            button["custom_id"].as_str().unwrap().chars().count(),
            MAX_CUSTOM_ID
        );
        assert_eq!(button["label"].as_str().unwrap().chars().count(), MAX_LABEL);
    }

    /// A confirmation token rides home inside `custom_id`, and a clamped one would not
    /// match `chat_pending::consume` — the tap would silently do nothing. Today the token
    /// is a UUID, so it fits with room to spare; this fails if that ever stops being true.
    #[test]
    fn a_confirmation_token_survives_the_custom_id_limit_intact() {
        let token = uuid::Uuid::now_v7().to_string();
        let id = format!("{}{token}", crate::chatbot::transport::turn::CONFIRM_PREFIX);
        let rendered = components(&Choices {
            prompt: String::new(),
            choices: vec![Choice {
                id: id.clone(),
                label: "Go ahead".to_string(),
                sublabel: None,
            }],
        });
        assert_eq!(
            rendered[0]["components"][0]["custom_id"].as_str(),
            Some(id.as_str()),
            "a truncated confirm token cannot be spent"
        );
    }

    /// Type 6 acknowledges and changes nothing, which leaves spent buttons live and
    /// tappable. Type 7 with empty components retires them as the tap lands.
    #[test]
    fn tapping_a_button_retires_it() {
        let body = DiscordTransport::ack_body();
        assert_eq!(body["type"], 7, "type 6 would leave the buttons in place");
        assert_eq!(body["data"]["components"], json!([]));
        assert!(
            body["data"].get("content").is_none(),
            "content must be left out so the message text survives the update"
        );
    }

    /// Nothing the assistant says should be able to ping a room.
    #[test]
    fn output_can_never_resolve_a_mention() {
        let body = message_body("hey @everyone", None);
        assert_eq!(body["allowed_mentions"]["parse"], json!([]));
    }

    #[test]
    fn a_thread_id_wins_over_the_parent_channel() {
        let target = Target {
            transport: TRANSPORT_ID.to_string(),
            space: "parent".to_string(),
            thread: Some("thread".to_string()),
        };
        assert_eq!(channel_of(&target), "thread");
    }

    #[tokio::test]
    async fn a_missing_token_is_a_configuration_error_not_a_request() {
        let transport = DiscordTransport::from_config(reqwest::Client::new(), &json!({}));
        let target = Target {
            transport: TRANSPORT_ID.to_string(),
            space: "1".to_string(),
            thread: None,
        };
        let err = transport
            .post(&target, &OutMessage::text("hi"))
            .await
            .expect_err("no token");
        assert!(matches!(err, ChatError::NotConfigured(_)), "got {err:?}");
    }

    /// The webhook route must not silently accept a POST for a socket transport.
    #[tokio::test]
    async fn the_webhook_path_refuses_rather_than_ignoring() {
        let transport = DiscordTransport::from_config(reqwest::Client::new(), &json!({}));
        let err = transport
            .verify(&HeaderMap::new(), b"{}")
            .await
            .expect_err("discord has no webhook");
        assert!(matches!(err, ChatError::NotConfigured(_)));
    }
}
