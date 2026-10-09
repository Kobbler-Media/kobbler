//! Turning "s24 e7 of someShow" into structured hints.
//!
//! Everything here is deterministic string work. The model is not asked to
//! extract season numbers, because it gets them wrong and a wrong season is a
//! wrong download.

use std::sync::LazyLock;

use regex::Regex;

use super::text;
use super::types::{MediaTypeHint, WantFormat};

/// Relative time, resolved against air dates rather than left to the model.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Relative {
    /// "the latest", "the new one", "the newest".
    Latest,
    /// "the next one", "when's the next".
    Next,
    /// "last night's", "yesterday's".
    LastNight,
    /// "the first one", "the pilot".
    First,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Parsed {
    /// The query with season/episode codes and a trailing year removed.
    pub title: String,
    /// Title tokens, intent words stripped. Empty means nothing was named.
    pub title_tokens: Vec<String>,
    /// A person named in the request ("Ludwig with David Mitchell"). The title
    /// search cannot use it, but it settles a tie between same-named shows.
    pub person: Option<String>,
    pub season: Option<i32>,
    pub episode: Option<i32>,
    /// "all seasons", "every season" — every season of a show, not one of them.
    pub all_seasons: bool,
    pub year: Option<i32>,
    pub relative: Option<Relative>,
    pub format: Option<WantFormat>,
    pub media_type: Option<MediaTypeHint>,
    pub exclude_watched: bool,
}

static SEASON_EPISODE_CODE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)\bs\s*(\d{1,3})\s*[ex]\s*(\d{1,3})\b").unwrap());
static SEASON_EPISODE_WORDS: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)\bseason\s*(\d{1,3})\b\s*(?:,|and|&)?\s*\bepisode\s*(\d{1,3})\b").unwrap()
});
static SEASON_X_EPISODE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)\b(\d{1,3})\s*x\s*(\d{1,3})\b").unwrap());
static SEASON_ONLY: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)\b(?:season|series)\s*(\d{1,3})\b").unwrap());
static SEASON_CODE_ONLY: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)\bs\s*(\d{1,3})\b").unwrap());
static EPISODE_ONLY: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)\bepisode\s*(\d{1,3})\b").unwrap());
static EPISODE_CODE_ONLY: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)\be\s*(\d{1,3})\b").unwrap());
static YEAR: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\b(1[89]\d{2}|20\d{2})\b").unwrap());
/// "with David Mitchell", "starring Adam Scott" — a person, not title words.
///
/// The lead word is case-insensitive but the name must be capitalised: "with
/// the tents" is a description, "with David Mitchell" is a person. Two to four
/// words, so a one-word name never identifies anyone by accident.
static PERSON_PHRASE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"\b(?i:with|starring|featuring)\s+((?:[A-Z][A-Za-z0-9-]+(?:\s+[A-Z][A-Za-z0-9-]+){1,3}))",
    )
    .unwrap()
});

fn take_pair(re: &Regex, text: &mut String) -> Option<(i32, i32)> {
    let (a, b, range) = {
        let caps = re.captures(text)?;
        (
            caps.get(1)?.as_str().parse().ok()?,
            caps.get(2)?.as_str().parse().ok()?,
            caps.get(0)?.range(),
        )
    };
    text.replace_range(range, " ");
    Some((a, b))
}

fn take_one(re: &Regex, text: &mut String) -> Option<i32> {
    let (v, range) = {
        let caps = re.captures(text)?;
        (caps.get(1)?.as_str().parse().ok()?, caps.get(0)?.range())
    };
    text.replace_range(range, " ");
    Some(v)
}

fn contains_any(haystack: &str, needles: &[&str]) -> bool {
    needles.iter().any(|n| haystack.contains(n))
}

fn detect_relative(q: &str) -> Option<Relative> {
    if contains_any(q, &["last night", "lastnight", "yesterday"]) {
        return Some(Relative::LastNight);
    }
    if contains_any(q, &["next", "upcoming", "coming up"]) {
        return Some(Relative::Next);
    }
    if contains_any(q, &["first", "pilot", "beginning", "start of"]) {
        return Some(Relative::First);
    }
    if contains_any(
        q,
        &[
            "latest",
            "newest",
            "most recent",
            "the new ",
            "new one",
            "new episode",
            "new season",
        ],
    ) {
        return Some(Relative::Latest);
    }
    None
}

fn detect_format(q: &str) -> Option<WantFormat> {
    if contains_any(q, &["audiobook", "audio book", "audible", "on audio"]) {
        return Some(WantFormat::Audiobook);
    }
    if contains_any(q, &["ebook", "e book", "kindle", "epub", "kobo"]) {
        return Some(WantFormat::Ebook);
    }
    if contains_any(q, &["both formats", "either format"]) {
        return Some(WantFormat::Both);
    }
    None
}

fn detect_media_type(q: &str, season: Option<i32>, episode: Option<i32>) -> Option<MediaTypeHint> {
    if contains_any(q, &["audiobook", "audio book", "audible"]) {
        return Some(MediaTypeHint::Audiobook);
    }
    if contains_any(q, &["ebook", "e book", "kindle", "epub"]) {
        return Some(MediaTypeHint::Book);
    }
    if episode.is_some() {
        return Some(MediaTypeHint::Episode);
    }
    if season.is_some() {
        return Some(MediaTypeHint::Season);
    }
    if contains_any(q, &["episode"]) {
        return Some(MediaTypeHint::Episode);
    }
    if contains_any(q, &["season"]) {
        return Some(MediaTypeHint::Season);
    }
    if contains_any(q, &["movie", "film"]) {
        return Some(MediaTypeHint::Movie);
    }
    if contains_any(q, &[" show", "tv series", " series", "tv "]) {
        return Some(MediaTypeHint::Show);
    }
    if contains_any(q, &["book", "novel"]) {
        return Some(MediaTypeHint::Book);
    }
    None
}

/// Every season of a show, not one named one. The phrases are ones a title
/// cannot own: "all seasons" and friends are never part of a show's name, so
/// stripping them never guts a real title the way "new" or "last" can.
fn detect_all_seasons(q: &str) -> bool {
    contains_any(q, &["all seasons", "every season", "all of it"])
}

fn detect_exclude_watched(q: &str) -> bool {
    contains_any(
        q,
        &[
            "haven't seen",
            "havent seen",
            "have not seen",
            "not seen",
            "unwatched",
            "not watched",
            "new to me",
            "haven't watched",
            "havent watched",
        ],
    )
}

pub fn parse(query: &str) -> Parsed {
    let lower = query.to_lowercase();
    let relative = detect_relative(&lower);
    let format = detect_format(&lower);
    let exclude_watched = detect_exclude_watched(&lower);
    let all_seasons = detect_all_seasons(&lower);
    let person = detect_person(query);
    let mut work = lower.clone();
    // The all-seasons intent is not part of the title: the providers are asked
    // for the show, not for all seasons of it.
    if all_seasons {
        for phrase in ["all seasons", "every season", "all of it"] {
            work = work.replace(phrase, " ");
        }
    }
    let pair = take_pair(&SEASON_EPISODE_WORDS, &mut work)
        .or_else(|| take_pair(&SEASON_EPISODE_CODE, &mut work))
        .or_else(|| take_pair(&SEASON_X_EPISODE, &mut work));
    let (season, episode) = match pair {
        Some((s, e)) => (Some(s), Some(e)),
        None => {
            let season = take_one(&SEASON_ONLY, &mut work)
                .or_else(|| take_one(&SEASON_CODE_ONLY, &mut work));
            let mut episode = take_one(&EPISODE_ONLY, &mut work);
            if season.is_some() && episode.is_none() {
                episode = take_one(&EPISODE_CODE_ONLY, &mut work);
            }
            (season, episode)
        }
    };

    let year = take_one(&YEAR, &mut work);
    let media_type = detect_media_type(&lower, season, episode);

    let title = text::normalize(&work);
    let title_tokens = text::strict_content_tokens(&title);

    Parsed {
        title,
        title_tokens,
        season,
        episode,
        year,
        relative,
        format,
        all_seasons,
        media_type,
        exclude_watched,
        person,
    }
}

/// The person the request names, when it names one: "Ludwig with David
/// Mitchell". Run on the original casing, because a person is capitalised and
/// a description is not.
fn detect_person(query: &str) -> Option<String> {
    // Season, episode and year phrases come off first, so "season 2" cannot be
    // swallowed as the tail of a name.
    let mut work = query.to_string();
    for re in [
        &SEASON_EPISODE_WORDS,
        &SEASON_EPISODE_CODE,
        &SEASON_X_EPISODE,
        &SEASON_ONLY,
        &SEASON_CODE_ONLY,
        &EPISODE_ONLY,
        &EPISODE_CODE_ONLY,
        &YEAR,
    ] {
        work = re.replace_all(&work, " ").to_string();
    }
    Some(PERSON_PHRASE.captures(&work)?.get(1)?.as_str().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn padded_code() {
        let p = parse("someShow S24E07");
        assert_eq!((p.season, p.episode), (Some(24), Some(7)));
        assert_eq!(p.title_tokens, vec!["someshow"]);
        assert_eq!(p.media_type, Some(MediaTypeHint::Episode));
    }

    #[test]
    fn split_code() {
        let p = parse("s24 e7 of someShow");
        assert_eq!((p.season, p.episode), (Some(24), Some(7)));
        assert_eq!(p.title_tokens, vec!["someshow"]);
    }

    #[test]
    fn spelled_out() {
        let p = parse("someShow season 24 episode 7");
        assert_eq!((p.season, p.episode), (Some(24), Some(7)));
        assert_eq!(p.title_tokens, vec!["someshow"]);
    }

    #[test]
    fn season_only() {
        let p = parse("season 24 of someShow");
        assert_eq!((p.season, p.episode), (Some(24), None));
        assert_eq!(p.media_type, Some(MediaTypeHint::Season));
    }

    #[test]
    fn cross_notation() {
        let p = parse("someShow 24x07");
        assert_eq!((p.season, p.episode), (Some(24), Some(7)));
    }

    #[test]
    fn years_are_not_episode_numbers() {
        let p = parse("dune 1984");
        assert_eq!(p.year, Some(1984));
        assert_eq!(p.season, None);
        assert_eq!(p.title_tokens, vec!["dune"]);

        let p = parse("dune (2021)");
        assert_eq!(p.year, Some(2021));
        assert_eq!(p.title_tokens, vec!["dune"]);
    }

    /// The bug this split exists for. "last" is an intent word in "last night's
    /// episode", so the ranker is right to drop it — but a provider asked for
    /// `seen` comes back with strangers. `title` is what gets asked; the tokens
    /// are what gets scored.
    #[test]
    fn a_title_built_from_intent_words_survives_in_the_spoken_title() {
        let p = parse("Last Seen (2026)");
        assert_eq!(p.year, Some(2026), "TMDB wants this in its own field");
        assert_eq!(p.title, "last seen", "what the providers are asked for");
        assert_eq!(p.title_tokens, vec!["seen"], "what the ranker scores with");

        let p = parse("Night Court");
        assert_eq!(p.title, "night court");
        assert_eq!(p.title_tokens, vec!["court"]);
    }

    #[test]
    fn relative_words() {
        assert_eq!(parse("the new one").relative, Some(Relative::Latest));
        assert_eq!(parse("the latest episode").relative, Some(Relative::Latest));
        assert_eq!(
            parse("last night's episode").relative,
            Some(Relative::LastNight)
        );
        assert_eq!(parse("the next one").relative, Some(Relative::Next));
        assert_eq!(
            parse("when's the next someShow").relative,
            Some(Relative::Next)
        );
        assert_eq!(
            parse("the first episode of someShow").relative,
            Some(Relative::First)
        );
        assert_eq!(parse("the new Sanderson").relative, Some(Relative::Latest));
        assert_eq!(parse("severance").relative, None);
    }

    #[test]
    fn nothing_named_is_visible() {
        assert!(parse("the new one").title_tokens.is_empty());
        assert!(parse("the latest episode").title_tokens.is_empty());
        assert!(
            !parse("the latest episode of someShow")
                .title_tokens
                .is_empty()
        );
    }

    #[test]
    fn formats() {
        assert_eq!(
            parse("the audiobook of the way of kings").format,
            Some(WantFormat::Audiobook)
        );
        assert_eq!(
            parse("the ebook of the way of kings").format,
            Some(WantFormat::Ebook)
        );
        assert_eq!(parse("the way of kings").format, None);
    }

    #[test]
    fn media_type_hints() {
        assert_eq!(
            parse("that baking show").media_type,
            Some(MediaTypeHint::Show)
        );
        assert_eq!(parse("the great escape").media_type, None);
        assert_eq!(
            parse("the new Sanderson audiobook").media_type,
            Some(MediaTypeHint::Audiobook)
        );
    }

    #[test]
    fn all_seasons_intent_is_seen_and_kept_out_of_the_title() {
        let p = parse("Deadloch all seasons");
        assert!(p.all_seasons);
        assert_eq!(p.title, "deadloch");
        assert_eq!(p.title_tokens, vec!["deadloch"]);
        assert_eq!(p.season, None);
        assert_eq!(p.media_type, Some(MediaTypeHint::Season));
        let p = parse("every season of Deadloch");
        assert!(p.all_seasons);
        assert_eq!(p.title, "of deadloch");
        let p = parse("Deadloch season 2");
        assert!(!p.all_seasons);
        assert_eq!(p.season, Some(2));
    }

    #[test]
    fn unwatched_intent() {
        assert!(parse("the latest someShow I haven't seen").exclude_watched);
        assert!(!parse("the latest someShow").exclude_watched);
    }
}
