//! The Discord gateway listener — the inbound half of the Discord transport.
//!
//! Discord only delivers plain conversational messages over a WebSocket. A webhook
//! ("interactions endpoint") carries slash commands and button taps and nothing else, so
//! taking the webhook would have meant `/ask <question>` instead of talking. It would
//! also have meant exposing kobbler to the internet; a gateway connection is outbound,
//! so nothing here needs a public URL, a domain, or an open port.
//!
//! Setting an interactions endpoint URL in the developer portal would *divert* button
//! taps to HTTP and stop them arriving here. Leave it blank.
//!
//! [`twilight_gateway::Shard`] owns the parts worth not writing by hand: heartbeats,
//! resume-versus-reidentify, close-code semantics and zombie-connection detection. What
//! is left is this file — which events matter, and who is allowed to send them.
//!
//! ## Intents
//!
//! Deliberately only the two non-privileged ones. Discord always delivers message
//! content in DMs with the app and in messages that mention it, and those are exactly
//! the messages the assistant answers, so `MESSAGE_CONTENT` buys nothing and asking for
//! it unapproved closes the socket with 4014.

use std::sync::Arc;
use std::time::Duration;

use twilight_gateway::{
    Event, EventTypeFlags, Intents, Shard, ShardId, ShardState, StreamExt as _,
};
use twilight_model::application::interaction::{Interaction, InteractionData};
use twilight_model::channel::Message;
use twilight_model::id::Id;
use twilight_model::id::marker::{ApplicationMarker, UserMarker};

use super::discord::{DiscordTransport, TRANSPORT_ID};
use super::{ChatTransport, EventKind, Inbound, Target};
use crate::AppState;
use crate::chatbot::inbound::{self, Accepted};
use crate::chatbot::transport::config;

/// How long to wait before looking again for a token that is not configured yet.
/// Configuring Discord in the settings page should not need an API restart.
const RECHECK: Duration = Duration::from_secs(60);
/// Breather after a socket drops. Twilight reconnects on its own, so reaching this at
/// all means the shard gave up and we are rebuilding it from freshly loaded config.
const RECONNECT_PAUSE: Duration = Duration::from_secs(5);

/// Supervise the gateway for the lifetime of the process.
///
/// Never returns. Reloads config on every reconnect, so changing the token in the
/// settings page takes effect without a restart.
pub async fn run(state: AppState) {
    loop {
        let Some(transport) = load(&state).await else {
            tokio::time::sleep(RECHECK).await;
            continue;
        };
        let transport = Arc::new(transport);

        // A fatal close is a wrong token or an intent Discord refused. Reconnecting on
        // the 5 s timer would just spin against the same rejection, so wait for someone
        // to change the setting — which the next pass reloads.
        let pause = match connect(&state, &transport).await {
            Outcome::FatallyClosed => RECHECK,
            Outcome::Dropped => RECONNECT_PAUSE,
        };
        tokio::time::sleep(pause).await;
    }
}

/// Build a transport from the stored config, or `None` when Discord is not set up.
async fn load(state: &AppState) -> Option<DiscordTransport> {
    let config = config::load(&state.pool, TRANSPORT_ID).await.ok()?;
    let transport = DiscordTransport::from_config(crate::http::new_client(), &config);
    match transport.token().is_empty() {
        true => None,
        false => Some(transport),
    }
}

/// Why a connection ended, which is the only thing that changes what we do next.
enum Outcome {
    /// Discord refused the credential or the intents. Retrying identically cannot help.
    FatallyClosed,
    /// The socket ended for a reason a fresh connection may survive.
    Dropped,
}

/// One connection's lifetime. Returns when the socket ends.
///
/// Twilight handles heartbeats, resumes and ordinary reconnects internally, so the
/// stream ending at all means the shard is finished — see [`ShardState::FatallyClosed`],
/// which makes `poll_next` return `None`.
async fn connect(state: &AppState, transport: &Arc<DiscordTransport>) -> Outcome {
    let intents = Intents::DIRECT_MESSAGES | Intents::GUILD_MESSAGES;
    let wanted =
        EventTypeFlags::READY | EventTypeFlags::MESSAGE_CREATE | EventTypeFlags::INTERACTION_CREATE;

    let mut shard = Shard::new(ShardId::ONE, transport.token().to_string(), intents);
    tracing::info!("discord gateway connecting");

    // Learned from READY. Until it arrives we cannot tell a mention of us from a mention
    // of anyone else, so guild messages are ignored rather than guessed at. READY also
    // carries the application id in `application`; a message can address the bot as the
    // user account (@-mention) or as the app (mention picker), so both ids are kept.
    let mut me: Option<Id<UserMarker>> = None;
    let mut app_id: Option<Id<ApplicationMarker>> = None;

    while let Some(item) = shard.next_event(wanted).await {
        let event = match item {
            Ok(event) => event,
            // Every error twilight surfaces here is one it intends to recover from; a
            // failure it cannot recover from ends the stream instead.
            Err(e) => {
                tracing::warn!(error = %e, "discord gateway receive failed");
                continue;
            }
        };

        match event {
            Event::Ready(ready) => {
                me = Some(ready.user.id);
                app_id = Some(ready.application.id);
                tracing::info!(user = %ready.user.name, "discord gateway ready");
            }
            Event::MessageCreate(msg) => {
                if let Some(inbound) = from_message(&msg, me, app_id) {
                    spawn_handle(state, transport, Pending::Message(inbound));
                }
            }
            Event::InteractionCreate(interaction) => {
                if let Some(pending) = from_interaction(&interaction.0) {
                    spawn_handle(state, transport, pending);
                }
            }
            _ => {}
        }
    }
    match shard.state() {
        ShardState::FatallyClosed => {
            tracing::error!(
                "discord refused the connection — check the bot token in Settings, and \
                 that the application has a bot user"
            );
            Outcome::FatallyClosed
        }
        state => {
            tracing::warn!(?state, "discord gateway stream ended; reconnecting");
            Outcome::Dropped
        }
    }
}

/// Normalise a Discord message into the shape every transport hands to the loop.
///
/// Returns `None` for anything the assistant should not answer, which is not an error:
/// its own posts, other bots, empty messages, and guild chatter that was not addressed
/// to it.
fn from_message(
    msg: &Message,
    me: Option<Id<UserMarker>>,
    app_id: Option<Id<ApplicationMarker>>,
) -> Option<Inbound> {
    // Without this the bot answers itself and the pair never stop talking.
    if msg.author.bot {
        return None;
    }

    let dm = msg.guild_id.is_none();
    let text = match dm {
        true => msg.content.trim().to_string(),
        // In a channel it only replies when spoken to. Discord delivers content for a
        // mention without any privileged intent, which is the whole reason this works.
        false => {
            let me = me?;
            // A mention can carry the user id (@-mention) or the application id
            // (mention picker); either addresses the bot.
            let addressed = msg
                .mentions
                .iter()
                .any(|m| m.id == me || app_id.is_some_and(|a| m.id.get() == a.get()));
            if !addressed {
                return None;
            }
            strip_mention(&msg.content, me, app_id)
        }
    };

    if text.is_empty() {
        return None;
    }

    Some(Inbound {
        external_id: Some(msg.author.id.to_string()),
        // Discord never exposes an address to a bot. Identity binds on the snowflake,
        // which is stable and unique in a way a display name is not.
        email: None,
        display_name: Some(
            msg.author
                .global_name
                .clone()
                .unwrap_or_else(|| msg.author.name.clone()),
        ),
        space: msg.channel_id.to_string(),
        // A Discord thread is itself a channel, so its id is already the space.
        thread: None,
        text,
        event_kind: EventKind::Message,
        choice_id: None,
    })
}

/// Remove the `@bot` so the model sees the sentence, not the addressing.
///
/// Discord sends mentions as `<@id>`, and older clients as `<@!id>`. A mention in the
/// middle of a sentence leaves a gap where it was, and this text is stored verbatim as
/// the user's turn, so the gap is closed — but only on a line a mention was actually
/// removed from, which leaves the layout of a multi-line message alone.
fn strip_mention(content: &str, me: Id<UserMarker>, app: Option<Id<ApplicationMarker>>) -> String {
    let mut forms = vec![format!("<@{me}>"), format!("<@!{me}>")];
    if let Some(app) = app {
        forms.push(format!("<@{app}>"));
        forms.push(format!("<@!{app}>"));
    }

    content
        .lines()
        .map(|line| {
            let mut out = line.to_string();
            let mut touched = false;
            for form in &forms {
                if out.contains(form.as_str()) {
                    out = out.replace(form.as_str(), " ");
                    touched = true;
                }
            }
            match touched {
                true => out.split_whitespace().collect::<Vec<_>>().join(" "),
                false => out,
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
        .trim()
        .to_string()
}

/// An inbound event, and what has to happen before it can be treated as an ordinary turn.
enum Pending {
    Message(Inbound),
    /// A button tap. Discord shows the user an error if nothing answers the interaction
    /// within three seconds, and a turn takes far longer than that, so the tap is
    /// acknowledged before anything else happens.
    Tap {
        inbound: Inbound,
        interaction_id: String,
        token: String,
    },
}

/// A button tap, normalised.
///
/// Everything past the acknowledgement runs on the ordinary path, so a tap and a typed
/// "yes" are the same code — [`crate::chatbot::transport::turn::choice_reply`] turns the
/// `custom_id` back into the sentence it stood for.
fn from_interaction(interaction: &Interaction) -> Option<Pending> {
    let InteractionData::MessageComponent(data) = interaction.data.as_ref()? else {
        return None;
    };
    let channel = interaction.channel.as_ref().map(|c| c.id)?;
    let user = interaction.author_id()?;

    Some(Pending::Tap {
        inbound: Inbound {
            external_id: Some(user.to_string()),
            email: None,
            display_name: interaction
                .author()
                .map(|u| u.global_name.clone().unwrap_or_else(|| u.name.clone())),
            space: channel.to_string(),
            thread: None,
            text: String::new(),
            event_kind: EventKind::Choice,
            choice_id: Some(data.custom_id.clone()),
        },
        interaction_id: interaction.id.to_string(),
        token: interaction.token.clone(),
    })
}

/// Handle an event off the socket task.
///
/// `next_event` is what drives the shard, heartbeats included, so anything awaited
/// between two calls to it is time the connection is not being kept alive. Identity
/// resolution hits the database and the acknowledgement hits Discord — both are normally
/// quick and neither is guaranteed to be, so neither happens here.
fn spawn_handle(state: &AppState, transport: &Arc<DiscordTransport>, pending: Pending) {
    let state = state.clone();
    let transport = Arc::clone(transport);

    tokio::spawn(async move {
        let inbound = match pending {
            Pending::Message(inbound) => inbound,
            Pending::Tap {
                inbound,
                interaction_id,
                token,
            } => {
                if let Err(e) = transport.ack_interaction(&interaction_id, &token).await {
                    tracing::warn!(error = %e, "could not acknowledge a discord interaction");
                }
                inbound
            }
        };
        handle(&state, &transport, inbound).await;
    });
}

/// Hand the event to the shared inbound path and deliver whatever comes back.
///
/// The ack is deliberately dropped for an accepted turn. Google Chat can carry one home
/// in the webhook response for free; here it would be a posted message, and then
/// `ThreadSink` posts again for the first tool event — two lines of throat-clearing
/// before the answer. A typing indicator says the same thing and costs no message.
async fn handle(state: &AppState, transport: &DiscordTransport, inbound: Inbound) {
    let channel = inbound.space.clone();
    let is_message = inbound.event_kind == EventKind::Message;

    match inbound::accept(state, TRANSPORT_ID, inbound).await {
        Ok(Accepted::Started { .. }) => {
            if is_message {
                transport.typing(&channel).await;
            }
        }
        Ok(Accepted::Refused(msg)) => {
            let target = Target {
                transport: TRANSPORT_ID.to_string(),
                space: channel,
                thread: None,
            };
            if let Err(e) = transport.post(&target, &msg).await {
                tracing::warn!(error = %e, "could not deliver a discord refusal");
            }
        }
        Ok(Accepted::Ignore) => {}
        Err(e) => tracing::error!(error = %e, "discord inbound failed"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn me() -> Id<UserMarker> {
        Id::new(42)
    }

    fn app() -> Id<ApplicationMarker> {
        Id::new(99)
    }

    #[test]
    fn a_mention_is_stripped_in_both_forms() {
        assert_eq!(
            strip_mention("<@42> what is new?", me(), Some(app())),
            "what is new?"
        );
        assert_eq!(
            strip_mention("<@!42> what is new?", me(), Some(app())),
            "what is new?"
        );
        assert_eq!(
            strip_mention("hey <@42> what is new?", me(), Some(app())),
            "hey what is new?"
        );
    }

    /// The app id is a second way to address the bot, and it must be stripped too.
    #[test]
    fn an_app_mention_is_stripped_like_a_user_mention() {
        assert_eq!(
            strip_mention("<@99> what is new?", me(), Some(app())),
            "what is new?"
        );
        assert_eq!(
            strip_mention("<@!99> what is new?", me(), Some(app())),
            "what is new?"
        );
    }

    #[test]
    fn stripping_leaves_an_unmentioned_message_alone() {
        assert_eq!(
            strip_mention("what is new?", me(), Some(app())),
            "what is new?"
        );
    }

    /// A message that is only the mention carries no question, and answering it would
    /// mean answering an empty string.
    #[test]
    fn a_bare_mention_strips_to_nothing() {
        assert!(strip_mention("<@42>", me(), Some(app())).is_empty());
    }

    /// The stripped text is stored verbatim as the user's turn, so a multi-line message
    /// has to keep its shape — only the line the mention was on gets closed up.
    #[test]
    fn stripping_leaves_other_lines_untouched() {
        let msg = "<@42> get these:\n  Dune\n  Dune  Two";
        assert_eq!(
            strip_mention(msg, me(), Some(app())),
            "get these:\n  Dune\n  Dune  Two"
        );
    }
}
