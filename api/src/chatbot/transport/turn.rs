//! The consumer for a front end that cannot stream.
//!
//! The Command Palette maps [`TurnEvent`]s onto SSE frames token by token. A chat
//! thread has no such thing: you post a message and then patch it. So this is the
//! second implementation of the same seam, and it differs from the web one in
//! exactly the ways the design says it must —
//!
//! * [`TurnEvent::Delta`] is ignored outright. Nothing is posted per token.
//! * The **first** tool event posts; everything after it edits that one message,
//!   at most once per [`COALESCE_WINDOW`], so the thread collapses into one
//!   evolving line instead of a scroll of progress.
//! * The terminal event always lands, throttle or no throttle. Silence reads as
//!   broken, so [`TurnEvent::Failed`] and a loop that dies without saying
//!   anything both produce a visible message.
//! * A turn that calls no tool at all — the commonest turn there is — never sees
//!   a first tool event, so its answer is the post.
//!
//! The window is [`crate::chatbot::watches`]'s, deliberately: progress during a
//! turn and progress after it are the same message being edited by two different
//! writers, and two different rate limits would be two different bugs.

use std::time::{Duration, Instant};

use serde_json::Value;
use tracing::warn;

use super::{ChatTransport, Choice, Choices, LinkStyle, OutMessage, Target};
use crate::chatbot::events::{Source, TurnEvent, TurnReceiver};
use crate::chatbot::watches::COALESCE_WINDOW;

/// Buttons on one answer. Discord allows five to a row and five rows — but
/// twenty-five taps is a catalogue, not a question. Five matches what the
/// resolver is willing to offer (`resolve::ranking::MAX_CANDIDATES`), so every
/// candidate it names has a button, and it is one row on a phone.
const MAX_BUTTONS: usize = 5;
/// Discord's ceiling for a `custom_id`, and the tightest of the transports, so
/// ids are built to fit everywhere rather than cut per transport. Cutting it
/// downstream is worse than it sounds: the tail is where the year and the
/// season live, which is exactly what tells one button from the next, and two
/// ids that collide after cutting take the whole message down with them.
const MAX_CHOICE_ID: usize = 100;
const MAX_CITATIONS: usize = 5;

const LOST: &str = "Something went wrong on my end and I lost that one — ask me again?";

/// A tap has to mean the same thing as the sentence she would have typed, or a
/// button is a second confirmation mechanism with its own bugs. These prefixes are
/// how a `Choice::id` carries that sentence, and [`choice_reply`] is the only
/// thing that reads them.
pub const CONFIRM_PREFIX: &str = "confirm:";
pub const CANCEL_PREFIX: &str = "cancel:";
pub const PICK_PREFIX: &str = "pick:";

/// What a tapped button means, phrased as the reply it stands in for.
///
/// The turn then runs verbatim as if she had typed it, which is what makes a tap
/// and a typed "yes" the same code path: both end at `chat_pending::consume`.
pub fn choice_reply(choice_id: &str) -> String {
    if choice_id.starts_with(CONFIRM_PREFIX) {
        return "Yes, go ahead.".to_string();
    }
    if choice_id.starts_with(CANCEL_PREFIX) {
        return "No, stop.".to_string();
    }
    if let Some(label) = choice_id.strip_prefix(PICK_PREFIX) {
        return label.to_string();
    }
    choice_id.to_string()
}

/// Buttons for a tool result that just asked the user something.
///
/// Two shapes qualify: a write waiting on its confirmation, and a resolution that
/// came back ambiguous. Everything else answers without a question, and a card on
/// it would be decoration.
fn choices_from(outputs: &Value) -> Option<Choices> {
    confirmation_choices(outputs).or_else(|| pick_choices(outputs))
}

fn confirmation_choices(outputs: &Value) -> Option<Choices> {
    if outputs.get("pending_confirmation").and_then(Value::as_bool) != Some(true) {
        return None;
    }
    let token = outputs.get("confirm_token").and_then(Value::as_str)?;
    Some(Choices {
        // The sentence in the answer already asked; this only labels the taps.
        prompt: String::new(),
        choices: vec![
            Choice {
                id: format!("{CONFIRM_PREFIX}{token}"),
                label: "Go ahead".to_string(),
                sublabel: None,
            },
            Choice {
                id: format!("{CANCEL_PREFIX}{token}"),
                label: "Stop".to_string(),
                sublabel: None,
            },
        ],
    })
}

fn pick_choices(outputs: &Value) -> Option<Choices> {
    let candidates = outputs.get("candidates")?.as_array()?;
    if candidates.len() < 2 {
        return None;
    }
    let qualifier = outputs
        .get("pick_qualifier")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty());
    let named: Vec<Candidate> = candidates
        .iter()
        .filter_map(candidate)
        .take(MAX_BUTTONS)
        .collect();
    let choices = buttons(&named, qualifier);
    match choices.len() < 2 {
        true => None,
        false => Some(Choices {
            prompt: String::new(),
            choices,
        }),
    }
}

/// One candidate, in the two parts a button is made of: what it is called, and
/// whatever tells it apart from the one next to it.
struct Candidate {
    title: String,
    sublabel: Option<String>,
}

fn candidate(candidate: &Value) -> Option<Candidate> {
    let title = ["title", "display_title", "name"]
        .into_iter()
        .find_map(|k| candidate.get(k).and_then(Value::as_str))
        .map(str::trim)
        .filter(|s| !s.is_empty())?;
    let sublabel = candidate
        .get("subtitle")
        .and_then(Value::as_str)
        .map(str::to_string)
        .or_else(|| {
            candidate
                .get("year")
                .and_then(Value::as_i64)
                .map(|y| y.to_string())
        });
    Some(Candidate {
        title: title.to_string(),
        sublabel,
    })
}

/// The sentence a tap stands in for, inside the id budget.
///
/// The year and the season are the parts that make one button mean something
/// different from the one beside it, so when something has to give it is the
/// title, cut at a word boundary. A trimmed title is still a resolvable phrase —
/// the resolver retries on the longest leading run that finds something.
fn reply_of(title: &str, sublabel: Option<&str>, qualifier: Option<&str>) -> String {
    let mut suffix = String::new();
    if let Some(sublabel) = sublabel {
        suffix.push_str(&format!(" ({sublabel})"));
    }
    if let Some(qualifier) = qualifier {
        suffix.push(' ');
        suffix.push_str(qualifier);
    }
    let room = MAX_CHOICE_ID.saturating_sub(PICK_PREFIX.chars().count() + suffix.chars().count());
    format!("{}{suffix}", trimmed(title, room))
}

fn trimmed(title: &str, room: usize) -> String {
    if title.chars().count() <= room {
        return title.to_string();
    }
    let mut out = String::new();
    for word in title.split_whitespace() {
        let would = match out.is_empty() {
            true => word.chars().count(),
            false => out.chars().count() + 1 + word.chars().count(),
        };
        if would > room {
            break;
        }
        if !out.is_empty() {
            out.push(' ');
        }
        out.push_str(word);
    }
    // One word longer than the whole budget: there is no boundary to cut at.
    match out.is_empty() {
        true => title.chars().take(room).collect(),
        false => out,
    }
}

/// The candidates as buttons, each one saying something the others do not.
///
/// The tap replies with the title, because that is what she would have typed. A
/// ref in a message is the syntax the design forbids — but a title alone is not
/// the sentence either when the question was about a season, so the qualifier
/// goes back with it.
///
/// Two candidates can carry the same title: a remake, a reissue, two unrelated
/// shows called "Seen". Left alone that is fatal rather than merely confusing —
/// Discord rejects an entire message whose buttons share a `custom_id`
/// (`COMPONENT_CUSTOM_ID_DUPLICATED`), so the question it was attached to never
/// arrives at all and the thread just stops. What tells them apart is the year,
/// which is also what she would have said out loud, so it goes into the caption
/// and into the sentence together.
fn buttons(candidates: &[Candidate], qualifier: Option<&str>) -> Vec<Choice> {
    let clashing: std::collections::HashSet<&str> = candidates
        .iter()
        .filter(|c| {
            candidates
                .iter()
                .filter(|other| other.title == c.title)
                .count()
                > 1
        })
        .map(|c| c.title.as_str())
        .collect();

    let mut taken = std::collections::HashSet::new();
    candidates
        .iter()
        .filter_map(|c| {
            let folded = clashing.contains(c.title.as_str());
            let name = match (folded, &c.sublabel) {
                (true, Some(sub)) => format!("{} ({sub})", c.title),
                _ => c.title.clone(),
            };
            // Two candidates that still read as the same words are one choice.
            // A second button for it could not mean anything different, and on
            // Discord it would take the whole message down with it.
            let telling = match folded {
                true => c.sublabel.as_deref(),
                false => None,
            };
            let id = format!("{PICK_PREFIX}{}", reply_of(&c.title, telling, qualifier));
            if !taken.insert(id.clone()) {
                return None;
            }
            Some(Choice {
                id,
                label: name,
                // Folded in above; saying it twice is noise on a transport that
                // renders the sublabel, and invisible on one that does not.
                sublabel: match folded {
                    true => None,
                    false => c.sublabel.clone(),
                },
            })
        })
        .collect()
}

/// Whether a pick card still belongs under this answer.
///
/// The buttons come from the tool result and the words come from the model, and
/// the model is allowed to move on. Asked which of three shows she meant, it
/// answered "I've found **Last Seen** (2026) — the whole show, or a season?",
/// which is a better question than the one the tool asked — but the card under
/// it still offered the other two shows. A card that asks something the text
/// does not is a second conversation.
///
/// The test is how many candidates the answer still names. All of them, or none
/// of them, and the question is still "which one". Exactly one, and it has been
/// settled: the buttons go and the text stands on its own.
///
/// A confirmation card is never contradicted this way — naming the one thing it
/// is about is exactly what its sentence does — so only picks are weighed.
fn still_asking(choices: &Choices, answer: &str) -> bool {
    if !choices
        .choices
        .iter()
        .all(|c| c.id.starts_with(PICK_PREFIX))
    {
        return true;
    }
    named(&choices.choices, answer) != 1
}

fn named(choices: &[Choice], answer: &str) -> usize {
    let mut haystack: Vec<Option<String>> = words(answer).into_iter().map(Some).collect();
    let mut labels: Vec<Vec<String>> = choices.iter().map(|c| words(&c.label)).collect();
    // Longest first, and a match claims the words it matched, so "Seen" cannot
    // count itself against the "Seen" inside "Last Seen".
    labels.sort_by_key(|l| std::cmp::Reverse(l.len()));
    labels
        .iter()
        .filter(|label| claim_run(&mut haystack, label.as_slice()))
        .count()
}

/// Find `needle` as a run of words nothing has claimed, and claim it.
fn claim_run(haystack: &mut [Option<String>], needle: &[String]) -> bool {
    if needle.is_empty() || needle.len() > haystack.len() {
        return false;
    }
    for start in 0..=haystack.len() - needle.len() {
        let hit = needle
            .iter()
            .enumerate()
            .all(|(i, w)| haystack[start + i].as_deref() == Some(w.as_str()));
        if hit {
            for slot in &mut haystack[start..start + needle.len()] {
                *slot = None;
            }
            return true;
        }
    }
    false
}

/// Words, lowercased, punctuation dropped — enough to tell a whole name from a
/// name that happens to sit inside a longer one.
fn words(text: &str) -> Vec<String> {
    text.split(|c: char| !c.is_alphanumeric())
        .filter(|w| !w.is_empty())
        .map(str::to_lowercase)
        .collect()
}

pub struct ThreadSink<'a> {
    transport: &'a dyn ChatTransport,
    target: Target,
    coalesce: Duration,
    message_id: Option<String>,
    last_sent: Option<Instant>,
    sources: Vec<Source>,
    choices: Option<Choices>,
    public_url: Option<String>,
}

impl<'a> ThreadSink<'a> {
    pub fn new(transport: &'a dyn ChatTransport, target: Target) -> Self {
        Self::with_coalesce(transport, target, COALESCE_WINDOW)
    }

    /// Injectable window, so a test proves coalescing without sitting through
    /// fifteen real seconds.
    pub fn with_coalesce(
        transport: &'a dyn ChatTransport,
        target: Target,
        coalesce: Duration,
    ) -> Self {
        Self {
            transport,
            target,
            coalesce,
            message_id: None,
            last_sent: None,
            sources: Vec::new(),
            choices: None,
            public_url: None,
        }
    }

    /// The externally reachable base URL, when the admin has configured one.
    /// Without it a citation stays the text it is today, because a relative path
    /// is dead in a phone notification.
    pub fn with_public_url(mut self, public_url: Option<String>) -> Self {
        self.public_url = public_url.filter(|u| !u.is_empty());
        self
    }

    /// Drive a turn into the thread, returning the message the answer landed on.
    pub async fn consume(mut self, mut events: TurnReceiver) -> Option<String> {
        let mut answered = false;
        while let Some(event) = events.recv().await {
            match event {
                // A chat client has no way to render a token, and posting one
                // per delta would be a message per word.
                TurnEvent::Delta(_) => {}
                TurnEvent::ToolStarted { summary, .. } => {
                    self.progress(&sentence(&summary, "…")).await;
                }
                TurnEvent::ToolFinished {
                    detail, outputs, ..
                } => {
                    if let Some(choices) = choices_from(&outputs) {
                        self.choices = Some(choices);
                    }
                    self.progress(&sentence(&detail, "")).await;
                }
                TurnEvent::Sources(mut found) => self.sources.append(&mut found),
                TurnEvent::Done { text } => {
                    self.answer(&text).await;
                    answered = true;
                    break;
                }
                TurnEvent::Failed(reason) => {
                    self.answer(&sentence(&reason, "")).await;
                    answered = true;
                    break;
                }
            }
        }
        // The loop dropped its sender without a terminal event. Saying nothing
        // would leave her looking at "Looking…" forever.
        if !answered {
            self.answer(LOST).await;
        }
        self.message_id
    }

    async fn progress(&mut self, text: &str) {
        if text.is_empty() {
            return;
        }
        // The first tool event is the post the design asks for; it is never
        // throttled, because there is nothing yet to edit.
        if self.message_id.is_none() {
            self.send(OutMessage::text(text)).await;
            return;
        }
        // A transport that can only post would turn progress into a new message
        // every time, which is the spam this whole scheme avoids.
        if !self.transport.capabilities().edit_in_place || !self.due() {
            return;
        }
        self.send(OutMessage::text(text)).await;
    }

    /// Deliver the answer, and mean it.
    ///
    /// This is the one message of the turn that cannot be dropped, so a failure
    /// here is not the end of it. Two things bounce a terminal message and
    /// neither is about the words: the buttons (Discord rejects an entire
    /// message whose `custom_id`s collide, taking the answer with it) and the
    /// message being edited (deleted, or never posted). So the retry drops both
    /// — plain text, posted fresh — and what she was actually asking for still
    /// arrives, buttonless.
    async fn answer(&mut self, text: &str) {
        let body = with_citations(
            text,
            &self.sources,
            self.public_url.as_deref(),
            self.transport.link_style(),
        );
        let choices = self
            .choices
            .take()
            .filter(|choices| still_asking(choices, text));
        let msg = match (choices, self.transport.capabilities().buttons) {
            (Some(choices), true) => OutMessage::with_choices(body.clone(), choices),
            // Without buttons the question is still in the answer text — the
            // confirmation is a sentence first and a card second — so there is
            // nothing to degrade into.
            _ => OutMessage::text(body.clone()),
        };
        if self.send(msg).await {
            return;
        }
        warn!(
            transport = %self.target.transport,
            space = %self.target.space,
            "A chat answer would not land; posting it again as plain text",
        );
        self.message_id = None;
        self.send(OutMessage::text(body)).await;
    }

    fn due(&self) -> bool {
        self.last_sent
            .is_none_or(|at| at.elapsed() >= self.coalesce)
    }

    /// `false` means the transport refused it and nothing is in the thread.
    async fn send(&mut self, msg: OutMessage) -> bool {
        let existing = self.message_id.clone();
        let posted = match existing.as_deref() {
            Some(id) if self.transport.capabilities().edit_in_place => self
                .transport
                .edit(&self.target, id, &msg)
                .await
                .map(|()| None),
            _ => self.transport.post(&self.target, &msg).await.map(Some),
        };
        match posted {
            Ok(new_id) => {
                if let Some(id) = new_id {
                    self.message_id = Some(id);
                }
                self.last_sent = Some(Instant::now());
                true
            }
            Err(e) => {
                warn!(
                    transport = %self.target.transport,
                    space = %self.target.space,
                    error = %e,
                    "Could not deliver a chat turn update",
                );
                false
            }
        }
    }
}

fn sentence(text: &str, suffix: &str) -> String {
    let text = text.trim();
    if text.is_empty() {
        return String::new();
    }
    let mut chars = text.chars();
    let head = chars.next().expect("non-empty").to_uppercase().to_string();
    format!("{head}{}{suffix}", chars.as_str())
}

/// Citations, as text. Chat has no `sources` event, and [`Source::url`] is a path
/// into the web UI — dead on its own in a phone notification. With a configured
/// public base URL the title becomes a real link; without one, what survives is the
/// part that is useful either way: which things the answer was actually about.
fn with_citations(
    text: &str,
    sources: &[Source],
    public_url: Option<&str>,
    style: LinkStyle,
) -> String {
    let cited = cited(sources, text);
    if cited.is_empty() {
        return text.to_string();
    }
    let list: Vec<String> = cited
        .iter()
        .take(MAX_CITATIONS)
        .map(|s| {
            let title = linked_title(s, public_url, style);
            match (&s.description, s.year) {
                (Some(d), _) if !d.is_empty() => format!("• {title} — {d}"),
                (_, Some(y)) => format!("• {title} ({y})"),
                _ => format!("• {title}"),
            }
        })
        .collect();
    match text.is_empty() {
        true => list.join("\n"),
        false => format!("{text}\n\n{}", list.join("\n")),
    }
}

/// Google Chat's own hyperlink syntax for message text: `<url|label>`. Nothing
/// else in the line changes, and a client that failed to render the markup would
/// still show the title between the delimiters.
fn linked_title(source: &Source, public_url: Option<&str>, style: LinkStyle) -> String {
    match public_url {
        Some(base) if !source.url.is_empty() => {
            style.render(&absolute(base, &source.url), &source.title)
        }
        _ => source.title.clone(),
    }
}

fn absolute(base: &str, path: &str) -> String {
    format!(
        "{}/{}",
        base.trim_end_matches('/'),
        path.trim_start_matches('/')
    )
}

/// Same rule the Command Palette uses: prefer the items the answer named, and
/// fall back to all of them when it paraphrased everything.
fn cited<'s>(sources: &'s [Source], answer: &str) -> Vec<&'s Source> {
    let mut seen = std::collections::HashSet::new();
    let unique: Vec<&Source> = sources
        .iter()
        .filter(|s| seen.insert((s.item_type.as_str(), s.id.as_str())))
        .collect();
    if unique.is_empty() {
        return unique;
    }
    let lowered = answer.to_lowercase();
    let mentioned: Vec<&Source> = unique
        .iter()
        .copied()
        .filter(|s| lowered.contains(&s.title.to_lowercase()))
        .collect();
    match mentioned.is_empty() {
        true => unique,
        false => mentioned,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn a_tap_says_what_she_would_have_typed() {
        assert_eq!(choice_reply("confirm:abc"), "Yes, go ahead.");
        assert_eq!(choice_reply("cancel:abc"), "No, stop.");
        assert_eq!(choice_reply("pick:The Stand"), "The Stand");
        assert_eq!(choice_reply("something else"), "something else");
    }

    #[test]
    fn a_pending_write_offers_go_ahead_and_stop() {
        let choices = choices_from(&json!({
            "pending_confirmation": true,
            "confirm_token": "tok-1",
            "say": "I'm about to get S24E01 — say go ahead, or stop if that's wrong.",
        }))
        .expect("a pending write is a question");
        assert_eq!(choices.choices.len(), 2);
        assert_eq!(choices.choices[0].id, "confirm:tok-1");
        assert_eq!(choices.choices[1].id, "cancel:tok-1");
        assert_eq!(choice_reply(&choices.choices[0].id), "Yes, go ahead.");
    }

    #[test]
    fn an_ambiguous_resolution_offers_the_candidates_by_name() {
        let choices = choices_from(&json!({
            "resolution": "ambiguous",
            "candidates": [
                {"ref": "movie:1", "title": "Carrie", "year": 1976},
                {"ref": "movie:2", "title": "Carrie", "year": 2013, "subtitle": "remake"},
            ],
        }))
        .expect("two candidates is a question");
        // Two films called Carrie: the year is what tells them apart, so it is
        // in the caption and in the sentence the tap stands in for.
        assert_eq!(choices.choices[0].id, "pick:Carrie (1976)");
        assert_eq!(choices.choices[0].label, "Carrie (1976)");
        assert_eq!(choices.choices[1].id, "pick:Carrie (remake)");
        assert_eq!(choice_reply(&choices.choices[0].id), "Carrie (1976)");
    }

    /// One title, one button. Two candidates that read the same way are not a
    /// choice — and on Discord a repeated `custom_id` is rejected outright, so
    /// the answer they were attached to never arrives at all.
    #[test]
    fn candidates_that_share_a_title_still_get_buttons_that_differ() {
        let choices = choices_from(&json!({
            "resolution": "ambiguous",
            "candidates": [
                {"title": "Last Seen", "year": 2026},
                {"title": "Seen", "year": 2021},
                {"title": "Seen", "year": 2018},
            ],
        }))
        .expect("three candidates is a question");
        let ids: Vec<&str> = choices.choices.iter().map(|c| c.id.as_str()).collect();
        assert_eq!(
            ids,
            ["pick:Last Seen", "pick:Seen (2021)", "pick:Seen (2018)"]
        );
        let unique: std::collections::HashSet<&&str> = ids.iter().collect();
        assert_eq!(unique.len(), ids.len(), "a duplicate id loses the message");
        assert_eq!(
            choices.choices[0].label, "Last Seen",
            "a title nothing clashes with is left alone",
        );
    }

    /// Same words, nothing to tell them apart. The second button could only
    /// repeat the first, and it would take the message down with it.
    #[test]
    fn an_indistinguishable_candidate_is_dropped_rather_than_repeated() {
        assert!(
            choices_from(&json!({
                "resolution": "ambiguous",
                "candidates": [{"title": "Seen"}, {"title": "Seen"}],
            }))
            .is_none(),
            "one usable button is not a choice",
        );
    }

    /// Two shows, one named season. Without the number the tap resolves back to
    /// the same two shows and asks the same question again.
    #[test]
    fn a_tap_carries_the_season_the_question_was_about() {
        let choices = choices_from(&json!({
            "resolution": "ambiguous",
            "pick_qualifier": "season 3",
            "candidates": [
                {"ref": "show:1", "title": "Battlestar Galactica", "year": 1978},
                {"ref": "show:2", "title": "Battlestar Galactica", "year": 2004},
            ],
        }))
        .expect("two candidates is a question");
        assert_eq!(
            choice_reply(&choices.choices[0].id),
            "Battlestar Galactica (1978) season 3",
        );
        assert_eq!(
            choice_reply(&choices.choices[1].id),
            "Battlestar Galactica (2004) season 3",
        );
        assert_eq!(
            choices.choices[0].label, "Battlestar Galactica (1978)",
            "the button still reads as the thing, not as the sentence",
        );
    }

    fn three_shows() -> Choices {
        choices_from(&json!({
            "resolution": "ambiguous",
            "candidates": [
                {"title": "Last Seen", "year": 2026},
                {"title": "Seen", "year": 2021},
                {"title": "Seen", "year": 2018},
            ],
        }))
        .expect("three candidates is a question")
    }

    /// The real turn: the model looked past the ambiguity and asked a better
    /// question, and the card underneath was still offering two other shows.
    #[test]
    fn an_answer_that_settled_on_one_candidate_drops_the_card() {
        let choices = three_shows();
        assert!(
            !still_asking(
                &choices,
                "I've found the TV show **Last Seen** (2026). Would you like the whole show or a specific season?",
            ),
            "naming one of them is picking it",
        );
    }

    #[test]
    fn an_answer_still_asking_keeps_the_card() {
        let choices = three_shows();
        assert!(
            still_asking(
                &choices,
                "Which one did you mean — Last Seen (2026), or Seen (2021), or Seen (2018)?",
            ),
            "an answer that lists them all is the question the card is for",
        );
        assert!(
            still_asking(&choices, "I found a few. Which one?"),
            "naming none of them leaves the buttons the only way to choose",
        );
    }

    /// "Seen" must not count itself against the "Seen" inside "Last Seen", or
    /// every answer looks like it named everything.
    #[test]
    fn a_shorter_title_cannot_claim_a_longer_ones_words() {
        let choices = three_shows();
        assert_eq!(named(&choices.choices, "Last Seen is the one."), 1);
        assert_eq!(named(&choices.choices, "Seen (2021) then."), 1);
    }

    #[test]
    fn a_confirmation_card_is_never_dropped_for_naming_its_subject() {
        let choices = choices_from(&json!({
            "pending_confirmation": true,
            "confirm_token": "tok-1",
        }))
        .expect("a pending write is a question");
        assert!(still_asking(
            &choices,
            "I'm about to get season 2 of Law & Order — say go ahead.",
        ));
    }

    /// Discord rejects a `custom_id` over 100 characters, and cutting the tail
    /// off would take the year with it — the one part that tells this button
    /// from the next.
    #[test]
    fn a_long_title_gives_way_to_the_year_and_the_season() {
        let title = "The Extraordinarily Long Chronicles of Something That Went On For Quite Some Time Indeed Volume Two";
        let reply = reply_of(title, Some("1978"), Some("season 3"));
        assert!(
            PICK_PREFIX.chars().count() + reply.chars().count() <= MAX_CHOICE_ID,
            "{} chars: {reply}",
            reply.chars().count(),
        );
        assert!(reply.ends_with("(1978) season 3"), "{reply}");
        assert!(
            reply.starts_with("The Extraordinarily Long Chronicles"),
            "{reply}"
        );
        assert!(
            !reply.contains("  ") && !reply.contains("Volum"),
            "the cut lands on a word boundary: {reply}",
        );
    }

    #[test]
    fn a_title_that_fits_is_left_exactly_as_it_is() {
        assert_eq!(reply_of("Carrie", Some("1976"), None), "Carrie (1976)");
        assert_eq!(
            reply_of("Carrie", None, Some("season 3")),
            "Carrie season 3"
        );
        assert_eq!(reply_of("Carrie", None, None), "Carrie");
    }

    #[test]
    fn a_confident_answer_offers_nothing_to_tap() {
        assert!(choices_from(&json!({"ok": true, "results": []})).is_none());
        assert!(
            choices_from(&json!({"resolution": "single", "candidates": [{"title": "Carrie"}]}))
                .is_none(),
            "one candidate is an answer, not a question",
        );
    }

    fn two_sources() -> Vec<Source> {
        vec![
            Source {
                item_type: "book".into(),
                id: "1".into(),
                title: "The Stand".into(),
                url: "/library/books/1".into(),
                cover_url: None,
                year: None,
                description: Some("Stephen King".into()),
            },
            Source {
                item_type: "book".into(),
                id: "2".into(),
                title: "Carrie".into(),
                url: "/library/books/2".into(),
                cover_url: None,
                year: Some(1974),
                description: None,
            },
        ]
    }

    #[test]
    fn citations_name_what_the_answer_was_about() {
        let sources = two_sources();
        let body = with_citations("The Stand is here.", &sources, None, LinkStyle::AnglePipe);
        assert!(body.contains("The Stand is here."));
        assert!(body.contains("• The Stand — Stephen King"), "{body}");
        assert!(
            !body.contains("Carrie"),
            "only what the answer named is cited: {body}"
        );
        assert!(
            !body.contains("/library/books/"),
            "a web path is noise in a chat thread: {body}"
        );

        let paraphrased = with_citations(
            "A couple of King novels.",
            &sources,
            None,
            LinkStyle::AnglePipe,
        );
        assert!(paraphrased.contains("The Stand"));
        assert!(paraphrased.contains("Carrie (1974)"), "{paraphrased}");
    }

    /// The no-regression case: every install that has never set a public URL
    /// must get byte-for-byte what it got before the setting existed.
    #[test]
    fn without_a_public_url_the_citation_block_is_unchanged() {
        let sources = two_sources();
        assert_eq!(
            with_citations("The Stand is here.", &sources, None, LinkStyle::AnglePipe),
            "The Stand is here.\n\n• The Stand — Stephen King"
        );
        assert_eq!(
            with_citations(
                "A couple of King novels.",
                &sources,
                None,
                LinkStyle::AnglePipe
            ),
            "A couple of King novels.\n\n• The Stand — Stephen King\n• Carrie (1974)"
        );
        assert_eq!(
            with_citations("Nothing cited.", &[], None, LinkStyle::AnglePipe),
            "Nothing cited.",
            "no sources means no block at all"
        );
        assert_eq!(
            with_citations("", &sources, None, LinkStyle::AnglePipe),
            "• The Stand — Stephen King\n• Carrie (1974)"
        );
    }

    #[test]
    fn a_public_url_turns_each_citation_into_a_chat_hyperlink() {
        let sources = two_sources();
        assert_eq!(
            with_citations(
                "A couple of King novels.",
                &sources,
                Some("https://kobbler.example.com"),
                LinkStyle::AnglePipe
            ),
            "A couple of King novels.\n\n• <https://kobbler.example.com/library/books/1|The Stand> — Stephen King\n• <https://kobbler.example.com/library/books/2|Carrie> (1974)"
        );
    }

    /// Discord renders `<url|text>` as literal text, so a citation block written in
    /// Google Chat's syntax would put raw markup in front of the user on almost every
    /// answer. Each transport gets the markup it actually renders.
    #[test]
    fn a_markdown_transport_gets_markdown_citations() {
        let sources = two_sources();
        assert_eq!(
            with_citations(
                "A couple of King novels.",
                &sources,
                Some("https://kobbler.example.com"),
                LinkStyle::Markdown
            ),
            "A couple of King novels.\n\n• [The Stand](https://kobbler.example.com/library/books/1) — Stephen King\n• [Carrie](https://kobbler.example.com/library/books/2) (1974)"
        );
    }

    #[test]
    fn the_base_and_the_path_are_joined_by_exactly_one_slash() {
        assert_eq!(
            absolute("https://kobbler.example.com/", "/movies/1"),
            "https://kobbler.example.com/movies/1"
        );
        assert_eq!(
            absolute("https://kobbler.example.com", "/movies/1"),
            "https://kobbler.example.com/movies/1"
        );
        assert_eq!(
            absolute("https://example.com/kobbler", "/movies/1"),
            "https://example.com/kobbler/movies/1"
        );
    }

    #[test]
    fn a_source_with_no_path_stays_plain_text() {
        let source = Source {
            item_type: "book".into(),
            id: "1".into(),
            title: "The Stand".into(),
            url: String::new(),
            cover_url: None,
            year: None,
            description: None,
        };
        for style in [LinkStyle::Markdown, LinkStyle::AnglePipe] {
            assert_eq!(
                linked_title(&source, Some("https://kobbler.example.com"), style),
                "The Stand"
            );
        }
    }

    #[test]
    fn a_progress_line_reads_like_a_sentence() {
        assert_eq!(
            sentence("searching for carrie", "…"),
            "Searching for carrie…"
        );
        assert_eq!(sentence("  3 results  ", ""), "3 results");
        assert_eq!(sentence("   ", "…"), "");
    }
}
