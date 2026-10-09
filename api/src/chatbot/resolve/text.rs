//! Token utilities shared by the ranker and the fixture index.
//!
//! Deliberately the same code in both places: if the fixture index matched
//! differently from the ranker, the phrasing table would be testing the fixture
//! rather than the resolver.

/// Words that carry intent, not identity. "that baking show" is about baking;
/// "show" is a media-type hint, and the ranker gets that separately.
const NOISE: &[&str] = &[
    "the",
    "a",
    "an",
    "of",
    "for",
    "to",
    "in",
    "on",
    "at",
    "and",
    "that",
    "this",
    "one",
    "ones",
    "it",
    "its",
    "me",
    "my",
    "please",
    "can",
    "you",
    "get",
    "us",
    "give",
    "with",
    "new",
    "newest",
    "latest",
    "recent",
    "next",
    "last",
    "night",
    "nights",
    "yesterday",
    "first",
    "episode",
    "episodes",
    "ep",
    "season",
    "seasons",
    "series",
    "show",
    "shows",
    "movie",
    "movies",
    "film",
    "films",
    "book",
    "books",
    "audiobook",
    "audiobooks",
    "ebook",
    "ebooks",
    "download",
    "grab",
    "watch",
    "read",
    "listen",
    "want",
    "wants",
    "wanted",
    "another",
    "copy",
    "when",
    "is",
    "whens",
    "does",
    "do",
    "come",
    "out",
    "about",
    "some",
    "thing",
    "stuff",
    "s",
];

pub fn is_noise(token: &str) -> bool {
    NOISE.contains(&token)
}

/// Lowercase, drop punctuation, collapse whitespace.
pub fn normalize(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut last_space = true;
    for ch in s.chars() {
        if ch.is_alphanumeric() {
            for lower in ch.to_lowercase() {
                out.push(lower);
            }
            last_space = false;
        } else if !last_space {
            out.push(' ');
            last_space = true;
        }
    }
    out.trim_end().to_string()
}

pub fn tokens(s: &str) -> Vec<String> {
    normalize(s)
        .split_whitespace()
        .map(str::to_string)
        .collect()
}

/// Tokens with the intent words removed. May be empty — that is the signal that
/// the caller named no title at all ("the new one").
pub fn strict_content_tokens(s: &str) -> Vec<String> {
    tokens(s).into_iter().filter(|t| !is_noise(t)).collect()
}

/// `strict_content_tokens`, falling back to the full token list so matching
/// never runs against nothing.
pub fn content_tokens(s: &str) -> Vec<String> {
    let kept = strict_content_tokens(s);
    if kept.is_empty() { tokens(s) } else { kept }
}

/// Light suffix stripping, enough that "baking" and "bake off" are the same
/// show and "tents" and "tent" are the same set. Not a real stemmer and does
/// not need to be.
pub fn stem(word: &str) -> String {
    let mut w = word.to_string();
    if w.len() > 4 && w.ends_with("sses") {
        w.truncate(w.len() - 2);
    } else if w.len() > 4 && w.ends_with("ies") {
        w.truncate(w.len() - 3);
        w.push('i');
    } else if w.len() > 3 && w.ends_with('s') && !w.ends_with("ss") && !w.ends_with("us") {
        w.truncate(w.len() - 1);
    }
    if w.len() > 4 && w.ends_with("ing") {
        w.truncate(w.len() - 3);
    } else if w.len() > 4 && w.ends_with("ed") {
        w.truncate(w.len() - 2);
    }
    if w.len() > 3 && w.ends_with('e') {
        w.truncate(w.len() - 1);
    }
    w
}

/// Two tokens mean the same thing. Stemming, prefix and Jaro-Winkler stand in
/// for the stemming, prefix matching and typo tolerance Meili does server-side.
pub fn token_equiv(a: &str, b: &str) -> bool {
    if a == b {
        return true;
    }
    if stem(a) == stem(b) {
        return true;
    }
    let (short, long) = if a.len() <= b.len() { (a, b) } else { (b, a) };
    if short.len() >= 5 && long.starts_with(short) {
        return true;
    }
    if short.len() >= 5 && strsim::jaro_winkler(a, b) >= 0.89 {
        return true;
    }
    false
}

/// Dice coefficient over equivalent tokens: rewards covering the query without
/// letting a candidate with twenty extra words win on containment alone.
pub fn dice(query: &[String], candidate: &[String]) -> f64 {
    if query.is_empty() || candidate.is_empty() {
        return 0.0;
    }
    let mut used = vec![false; candidate.len()];
    let mut hits = 0usize;
    for q in query {
        for (i, c) in candidate.iter().enumerate() {
            if !used[i] && token_equiv(q, c) {
                used[i] = true;
                hits += 1;
                break;
            }
        }
    }
    2.0 * hits as f64 / (query.len() + candidate.len()) as f64
}

/// Fraction of the query's tokens present anywhere in the haystack. Used for
/// descriptions, where length is not a signal.
pub fn coverage(query: &[String], haystack: &[String]) -> f64 {
    if query.is_empty() {
        return 0.0;
    }
    let hits = query
        .iter()
        .filter(|q| haystack.iter().any(|h| token_equiv(q, h)))
        .count();
    hits as f64 / query.len() as f64
}

pub fn jaro(a: &str, b: &str) -> f64 {
    if a.is_empty() || b.is_empty() {
        return 0.0;
    }
    strsim::jaro_winkler(a, b)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_strips_punctuation_and_case() {
        assert_eq!(normalize("Dune (2021)"), "dune 2021");
        assert_eq!(normalize("last night's episode"), "last night s episode");
        assert_eq!(normalize("  Law & Order  "), "law order");
    }

    #[test]
    fn content_tokens_drop_intent_words() {
        assert_eq!(content_tokens("that baking show"), vec!["baking"]);
        assert_eq!(content_tokens("the new Sanderson"), vec!["sanderson"]);
        assert_eq!(
            content_tokens("the latest episode of someShow"),
            vec!["someshow"]
        );
        assert_eq!(content_tokens("the one with the tents"), vec!["tents"]);
    }

    #[test]
    fn strict_content_tokens_is_empty_when_nothing_was_named() {
        assert!(strict_content_tokens("the new one").is_empty());
        assert!(strict_content_tokens("last night's episode").is_empty());
        assert!(!strict_content_tokens("the new someShow").is_empty());
    }

    #[test]
    fn content_tokens_never_empty() {
        assert!(!content_tokens("the new one").is_empty());
    }

    #[test]
    fn token_equiv_covers_stems_and_typos() {
        assert!(token_equiv("baking", "bake"));
        assert!(token_equiv("tents", "tent"));
        assert!(token_equiv("arival", "arrival"));
        assert!(token_equiv("dune", "dunes"));
        assert!(!token_equiv("dune", "dude"));
        assert!(!token_equiv("cat", "dog"));
        assert!(!token_equiv("galactica", "gattaca"));
    }

    #[test]
    fn dice_prefers_the_tighter_title() {
        let q = tokens("the great escape");
        let tight = dice(&q, &tokens("The Great Escape"));
        let loose = dice(&q, &tokens("The Great British Bake Off"));
        assert!(tight > loose, "{tight} !> {loose}");
        assert!((tight - 1.0).abs() < 1e-9);
    }
}
