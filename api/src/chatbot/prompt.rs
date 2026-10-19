//! The system prompt.
//!
//! Started from the Meili chat workspace's `SYSTEM_PROMPT` in `search/chat.rs`
//! and adapted for a loop that owns its own tools: the paragraph telling the
//! model what is searchable is now a list of the tools it has, and the
//! per-index descriptions live where they always did.
//!
//! Runs on whichever model is loaded (Gemma or Qwen). Small models punish
//! contradictory clauses harder than missing ones, so this stays short and every rule earns its line. When the model misbehaves, the
//! first thing to try is deleting a rule, not adding one.

use sqlx::PgPool;
use time::Date;
use uuid::Uuid;

use crate::repositories::chat_memory;

use super::memory;

const BASE: &str = r#"You are the assistant for Kobbler, a household's personal media library: books, audiobooks, movies, TV shows and sport.

Rules:
1. Look things up with your tools before answering; never answer from memory. The library is this household's, not a general one. If a tool has not told you which seasons a show has, you do not know.
2. When they point at one particular thing ("the new one", "last night's episode", "the new Sanderson"), call resolve_media first. Give it only the words that name the thing and leave the rest of the sentence behind: "the latest episode of X" out of "get me a new copy of the latest episode of X, the sound is out of sync".
3. If a request could mean more than one thing, ask which one, once. Do not pick. When they answer, go and do it.
4. A request to get, download, record or grab something is an instruction: call the tool straight away, with no confirmation step and no asking permission they already gave. A question ("when does it come out", "is it any good") is answered, not acted on. If they say yes to something you offered, do it now.
5. Which copy to download is yours to choose. Show a list of copies only if they ask to pick.
6. Never say something is done until a tool has told you it is. A season with a future air date is not out yet, but it can be queued: say it is queued and will be grabbed when it airs.
7. Cite titles and names from what the tools returned. If a search comes back empty, say so. Pass on any reliability caveat in a tool result. If a result has a "say" line, say that in your own words; its numbers are right.
8. You cannot delete anything, change anyone's files, or rename anything. If asked, say so."#;

/// The per-index descriptions, reused verbatim from the Meili chat workspace.
/// They are good prompt material and describe the same documents `search_library`
/// now queries directly.
const WHAT_IS_SEARCHABLE: &str = r#"What the library holds:
- books, movies, TV shows, episodes (code like S24E07, with air date): title, year, genres, library/want status and a short synopsis. Shows also carry network, status and how many episodes are actually available.
- A show is in the library when episodes of it are available; shows have no in-library flag. Monitored only means new episodes are being watched for, not that any exist.
- people, authors, series, genres: names with counts only. For the titles someone is in, or a title's cast, use the credits tools.
- passages: the full text of transcribed audiobooks and ebooks, with a timestamp (audio) or location (ebook)."#;

pub fn system_prompt() -> String {
    format!("{BASE}\n\n{WHAT_IS_SEARCHABLE}")
}

/// The model's own sense of "now" is its training cutoff, so without this it
/// calls things unreleased that came out months ago. Date only, no clock time:
/// the line then changes once a day instead of busting the prefix cache on
/// every turn.
pub fn date_line(today: Date) -> String {
    format!(
        "Today is {} {} {} {}. Anything dated before today has already happened, whatever your own knowledge says.",
        today.weekday(),
        today.day(),
        today.month(),
        today.year()
    )
}

/// Tier 3 facts, plus the one line carried forward from the last conversation in
/// this space.
///
/// Both are small enough to pay for on *every* message, which is the whole
/// argument for a capped profile over retrieval across a pile of summaries: no
/// lookup step, no ranking, nothing to get wrong. Returns `None` when there is
/// nothing to say — an empty "here is what I know about you" line is worse than
/// no line.
pub async fn memory_prompt(pool: &PgPool, user_id: Uuid, session_id: Uuid) -> Option<String> {
    let facts = memory::facts_line(pool, user_id).await;
    let previously = chat_memory::carry_forward_summary(pool, session_id)
        .await
        .ok()
        .flatten()
        .filter(|s| !s.trim().is_empty());

    let mut lines = Vec::new();
    if let Some(facts) = facts {
        lines.push(format!(
            "What you already know about the person you are talking to — {facts}. Use it when it helps; do not read it back to them unless they ask."
        ));
    }
    if let Some(previously) = previously {
        lines.push(format!(
            "Where the last conversation here got to: {previously}"
        ));
    }
    match lines.is_empty() {
        true => None,
        false => Some(lines.join("\n\n")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Lessons from live transcripts that must survive any trim.
    #[test]
    fn the_prompt_keeps_the_rules_learned_from_transcripts() {
        let prompt = system_prompt();
        // The Ludwig transcript: with no tool saying which seasons exist, the model
        // asserted "season 2 is not out yet" from memory.
        assert!(prompt.contains("you do not know"));
        // A show has no in-library flag; without this the model looks for one, does
        // not find it, and denies having a show it just cited.
        assert!(prompt.contains("A show is in the library when episodes of it are available"));
        assert!(prompt.contains("resolve_media"));
    }

    /// Explicit requests act immediately; only unclear ones or questions get a
    /// follow-up. There is no confirmation handshake to leak into replies.
    #[test]
    fn the_prompt_acts_on_explicit_requests_without_a_confirmation_step() {
        let prompt = system_prompt();
        assert!(prompt.contains("no confirmation step"));
        assert!(prompt.contains("permission they already gave"));
        assert!(!prompt.contains("token"));
        assert!(prompt.contains("Never say something is done until a tool has told you it is"));
        // A future season queues fine — Kobload holds the search until the air
        // date — but the model must say "queued, not out yet", not "on its way".
        assert!(prompt.contains("will be grabbed when it airs"));
    }

    /// "None of those are right" came out of a resolve call carrying the whole
    /// sentence, and "I can't automatically pick a copy for you" came out of a
    /// prompt that never said choosing one was the assistant's job.
    #[test]
    fn the_prompt_scopes_the_query_and_owns_the_choice_of_copy() {
        let prompt = system_prompt();
        assert!(prompt.contains("leave the rest of the sentence behind"));
        assert!(prompt.contains("Which copy to download is yours to choose"));
    }

    #[test]
    fn the_date_line_names_the_day_in_words() {
        let line = date_line(time::macros::date!(2026 - 10 - 19));
        assert!(line.starts_with("Today is Monday 19 October 2026."), "{line}");
    }

    /// Promising what the registry cannot do is the contradiction this model
    /// handles worst, and there is still no tool that deletes anything.
    #[test]
    fn the_prompt_does_not_promise_actions_that_do_not_exist() {
        let prompt = system_prompt().to_lowercase();
        assert!(prompt.contains("cannot delete anything"));
        assert!(!prompt.contains("cannot download"));
    }
}
