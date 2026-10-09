//! What happens to an inbound chat event once a transport has normalised it.
//!
//! Both inbound paths end here: Google Chat's signed webhook and Discord's gateway
//! socket. Identity resolution, the refusal replies and the spawned turn are identical
//! for both — the transports differ in how an answer is *delivered*, not in who is
//! allowed to ask for one. Anything that forks on transport below this line is a bug.

use serde_json::Value;

use crate::AppState;
use crate::chatbot::identity::{self, ChatPrincipal, ChatSender, PairingChallenge, Resolution};
use crate::chatbot::llm::ChatMessage;
use crate::chatbot::run::{self, Turn};
use crate::chatbot::tools::read;
use crate::chatbot::transport::turn::{ThreadSink, choice_reply};
use crate::chatbot::transport::{
    ChatError, ChatTransport, ChatTransportRegistry, EventKind, Inbound, OutMessage, Target, config,
};
use crate::chatbot::{events, store};

const UNMAPPED_REPLY: &str =
    "You're not set up to use this yet — ask whoever runs the library to add your chat address.";
const DISABLED_REPLY: &str = "Your access to this has been switched off.";
const OPEN_FAILED: &str = "I couldn't get to the library just then — try me again in a minute.";

/// What the caller should do about the event it just handed over.
///
/// The split exists because the two transports disagree about acknowledgements, and
/// only one of them is free. Google Chat can carry the ack in the webhook response at
/// no cost; Discord would have to *post* it, and then [`ThreadSink`] posts again for
/// the first tool event — two messages of throat-clearing before the answer. So the
/// ack is offered rather than sent, and Discord shows a typing indicator instead.
///
/// [`Accepted::Refused`] is not optional. Those are the cases where nothing else will
/// ever arrive, and silence is indistinguishable from the bot being broken.
pub enum Accepted {
    Started { ack: OutMessage },
    Refused(OutMessage),
    Ignore,
}

/// Resolve the sender, then run the turn on a spawned task.
///
/// Returns as soon as the turn is *accepted*, never when it finishes — Google gives the
/// webhook about 30 seconds and a real turn routinely outlives that.
pub async fn accept(
    state: &AppState,
    transport_id: &str,
    inbound: Inbound,
) -> Result<Accepted, ChatError> {
    let sender = ChatSender {
        transport: transport_id,
        external_id: inbound.external_id.as_deref(),
        email: inbound.email.as_deref(),
        display_name: inbound.display_name.as_deref(),
        space: Some(inbound.space.as_str()),
        thread: inbound.thread.as_deref(),
    };

    let resolution = identity::resolve(&state.pool, &sender).await.map_err(|e| {
        tracing::error!(error = %e, transport = %transport_id, "chat identity resolve failed");
        ChatError::Transport(format!("resolve failed: {e}"))
    })?;

    let principal = match resolution {
        Resolution::Principal(principal) => principal,
        Resolution::Disabled => return Ok(Accepted::Refused(OutMessage::text(DISABLED_REPLY))),
        Resolution::Unknown => return Ok(Accepted::Refused(OutMessage::text(UNMAPPED_REPLY))),
        Resolution::Pairing(challenge) => {
            return Ok(Accepted::Refused(OutMessage::text(pairing_text(
                &challenge,
            ))));
        }
        // Already told, and told recently. Anything further would be the spam we are
        // rate-limiting against.
        Resolution::Silent => return Ok(Accepted::Ignore),
    };

    let ack = ack_message(&inbound);
    spawn_turn(state, transport_id.to_string(), inbound, principal);
    Ok(Accepted::Started { ack })
}

/// The seam the agent loop plugs into.
///
/// Everything above this line has already answered the caller, so a turn may take as
/// long as it needs.
fn spawn_turn(state: &AppState, transport_id: String, inbound: Inbound, principal: ChatPrincipal) {
    let state = state.clone();
    tokio::spawn(async move {
        tracing::info!(
            transport = %transport_id,
            identity_id = %principal.identity_id,
            user_id = %principal.auth.user_id,
            role = %principal.auth.role,
            space = %inbound.space,
            thread = ?inbound.thread,
            "chat turn accepted"
        );
        run_turn(&state, &transport_id, &inbound, &principal).await;
    });
}

/// One turn, from a chat address, through the same loop and the same registry the
/// Command Palette uses.
///
/// The only thing that differs is who consumes the events: [`ThreadSink`] posts and
/// patches where `transport::web` streams. `principal.auth` is a real `AuthUser` built
/// from `users.role` capped by `chat_identities.max_role`, so `require_contributor()`
/// gates dispatch verbatim — there is no second permission path.
async fn run_turn(
    state: &AppState,
    transport_id: &str,
    inbound: &Inbound,
    principal: &ChatPrincipal,
) {
    let Some(user_message) = user_message(inbound) else {
        return;
    };
    let target: Target = inbound.target(transport_id);

    // `space` + `thread` is the conversation key. A payload with no thread (a DM that
    // Google did not thread, or any Discord channel) still needs one, and the space is
    // the only stable thing left; the reply target keeps the real `thread`, `None`.
    let thread_key = inbound.thread.as_deref().unwrap_or(inbound.space.as_str());
    let session = match store::open_addressed_session(
        &state.pool,
        transport_id,
        &inbound.space,
        thread_key,
        Some(principal.auth.user_id),
    )
    .await
    {
        Ok(session) => session,
        Err(e) => {
            tracing::error!(error = %e, transport = %transport_id, "could not open a chat session");
            say(state, transport_id, &target, OPEN_FAILED).await;
            return;
        }
    };

    // History comes from the store, not from the event: the tool observations that make
    // "no, the other one" and a pending confirmation resolvable live on the steps.
    let history: Vec<ChatMessage> = match store::window(&state.pool, session.id, None).await {
        Ok(history) => history,
        Err(e) => {
            tracing::warn!(error = %e, "could not read the chat history");
            Vec::new()
        }
    };
    let context_ref = last_context_ref(&history);

    if let Err(e) = store::record_user_turn(&state.pool, session.id, &user_message).await {
        tracing::warn!(error = %e, "could not record the user turn");
    }
    let message_id = match store::start_assistant_turn(&state.pool, session.id).await {
        Ok(id) => id,
        Err(e) => {
            tracing::error!(error = %e, "could not start the assistant turn");
            say(state, transport_id, &target, OPEN_FAILED).await;
            return;
        }
    };

    let Some(transport) = open_transport(state, transport_id).await else {
        return;
    };

    let (tx, rx) = events::channel();
    let turn = Turn {
        state,
        auth: &principal.auth,
        transport: transport_id,
        session_id: session.id,
        message_id,
        history,
        user_message,
        context_ref,
    };
    // The loop's channel is bounded backpressure, not a queue, so the sink has to be
    // draining it while the turn runs or a talkative turn blocks on its own progress.
    let public_url = state.runtime_config.get().await.public_url;
    let sink = ThreadSink::new(transport.as_ref(), target).with_public_url(public_url);
    let (result, _) = tokio::join!(run::run(turn, tx), sink.consume(rx));

    if let Err(e) = store::finish_assistant_turn(
        &state.pool,
        message_id,
        &result.text,
        &result.steps,
        result.status,
    )
    .await
    {
        tracing::warn!(error = %e, "could not finalise the assistant turn");
    }
}

/// What the sender said, whether they typed it or tapped it.
///
/// A tap resolves to the sentence the button stood for, so it re-enters the loop as an
/// ordinary user turn and ends at the same `chat_pending::consume` that a typed "yes"
/// does. Building a separate confirmation path for taps is how the two drift.
fn user_message(inbound: &Inbound) -> Option<String> {
    let text = match inbound.event_kind {
        EventKind::Message => inbound.text.trim().to_string(),
        EventKind::Choice => inbound
            .choice_id
            .as_deref()
            .map(choice_reply)
            .unwrap_or_default(),
    };
    match text.is_empty() {
        true => None,
        false => Some(text),
    }
}

/// The previous turn's resolution — what "no, the other one" points at.
fn last_context_ref(history: &[ChatMessage]) -> Option<String> {
    history
        .iter()
        .rev()
        .filter(|m| m.role == "tool")
        .find_map(|m| {
            serde_json::from_str::<Value>(&m.content)
                .ok()
                .as_ref()
                .and_then(read::context_ref_from)
        })
}

async fn open_transport(state: &AppState, transport_id: &str) -> Option<Box<dyn ChatTransport>> {
    let config = match config::load(&state.pool, transport_id).await {
        Ok(config) => config,
        Err(e) => {
            tracing::error!(error = %e, transport = %transport_id, "no chat transport config for a turn");
            return None;
        }
    };
    ChatTransportRegistry::new().create(transport_id, &config)
}

/// Say one thing outside a turn. Used only where the turn could not start — silence
/// there reads exactly like the bot being broken.
async fn say(state: &AppState, transport_id: &str, target: &Target, text: &str) {
    let Some(transport) = open_transport(state, transport_id).await else {
        return;
    };
    if let Err(e) = transport.post(target, &OutMessage::text(text)).await {
        tracing::warn!(error = %e, transport = %transport_id, "could not deliver a chat failure notice");
    }
}

fn ack_message(inbound: &Inbound) -> OutMessage {
    if inbound.text.is_empty() {
        OutMessage::text("Got it.")
    } else {
        OutMessage::text("Looking…")
    }
}

fn pairing_text(challenge: &PairingChallenge) -> String {
    format!(
        "I don't know who you are yet. Give this code to whoever runs the library and they can \
         connect your account: {}",
        challenge.code
    )
}
