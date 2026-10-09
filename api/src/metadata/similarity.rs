//! Fuzzy string matching utilities for metadata comparison.

/// Strip periods and commas for punctuation-insensitive comparison.
pub(crate) fn strip_punctuation(s: &str) -> String {
    s.replace(['.', ','], "")
}

/// Fuzzy string match using tokenized Jaro-Winkler similarity.
///
/// Handles reordered words ("C., Clarke, Arthur" vs "Arthur C. Clarke"),
/// abbreviations ("A. Clarke" vs "Arthur Clarke"), and minor typos.
/// Both inputs should already be lowercased and punctuation-stripped.
pub(crate) fn fuzzy_match(a: &str, b: &str) -> bool {
    let a = a.trim();
    let b = b.trim();
    if a.is_empty() || b.is_empty() {
        return false;
    }
    if a.contains(b) || b.contains(a) {
        return true;
    }
    let mut a_tokens: Vec<&str> = a.split_whitespace().collect();
    let mut b_tokens: Vec<&str> = b.split_whitespace().collect();

    // Check if all tokens from the shorter side match (substring or abbreviation)
    // e.g. "a clarke" matches "arthur clarke" because "a" is a prefix of "arthur"
    let (short, long) = if a_tokens.len() <= b_tokens.len() {
        (&a_tokens, &b_tokens)
    } else {
        (&b_tokens, &a_tokens)
    };
    let abbreviation_match = short.iter().all(|st| {
        long.iter().any(|lt| {
            lt.eq_ignore_ascii_case(st) || lt.to_lowercase().starts_with(&st.to_lowercase())
        })
    });
    if abbreviation_match {
        return true;
    }

    a_tokens.sort_unstable();
    b_tokens.sort_unstable();
    let a_sorted = a_tokens.join(" ");
    let b_sorted = b_tokens.join(" ");
    strsim::jaro_winkler(&a_sorted, &b_sorted) >= 0.85
}

/// Fuzzy title match — stricter than `fuzzy_match` (designed for author names).
///
/// No raw substring containment (prevents "Star" matching "Starship Troopers").
/// No abbreviation matching (titles don't have initials like "A. Clarke").
/// Uses proportional containment, token overlap, and Jaro-Winkler.
/// Both inputs should already be lowercased and punctuation-stripped.
pub(crate) fn fuzzy_match_title(a: &str, b: &str) -> bool {
    let a = a.trim();
    let b = b.trim();
    if a.is_empty() || b.is_empty() {
        return false;
    }
    if a == b {
        return true;
    }

    let (short, long) = if a.len() <= b.len() { (a, b) } else { (b, a) };

    // Containment: shorter must be ≥2 words AND ≥30% the length of longer.
    // This catches "Ender's Game" inside "Ender's saga #01 - Ender's Game" (36%)
    // while rejecting "Star" inside "Starship Troopers" (1 word, 24%).
    let short_word_count = short.split_whitespace().count();
    if long.contains(short)
        && short_word_count >= 2
        && short.len() as f64 / long.len() as f64 >= 0.3
    {
        return true;
    }

    // Token overlap: ≥80% of shorter's tokens appear exactly in longer's tokens,
    // AND shorter must have at least 3 tokens (prevents "red" matching "the red queen").
    // Handles "Diary of a Wimpy Kid" vs "Diary of a Wimpy Kid Dog Days".
    let short_tokens: Vec<&str> = short.split_whitespace().collect();
    let long_tokens: Vec<&str> = long.split_whitespace().collect();
    if short_tokens.len() >= 3 {
        let matches = short_tokens
            .iter()
            .filter(|st| long_tokens.iter().any(|lt| lt.eq_ignore_ascii_case(st)))
            .count();
        if matches as f64 / short_tokens.len() as f64 >= 0.8 {
            return true;
        }
    }

    // Jaro-Winkler for typo tolerance
    let mut a_tokens: Vec<&str> = a.split_whitespace().collect();
    let mut b_tokens: Vec<&str> = b.split_whitespace().collect();
    a_tokens.sort_unstable();
    b_tokens.sort_unstable();
    let a_sorted = a_tokens.join(" ");
    let b_sorted = b_tokens.join(" ");
    strsim::jaro_winkler(&a_sorted, &b_sorted) >= 0.85
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_strip_punctuation() {
        assert_eq!(strip_punctuation("A.C. Clarke"), "AC Clarke");
        assert_eq!(strip_punctuation("no, change"), "no change");
        assert_eq!(strip_punctuation("clean"), "clean");
    }

    #[test]
    fn test_fuzzy_match_exact() {
        assert!(fuzzy_match("arthur clarke", "arthur clarke"));
    }

    #[test]
    fn test_fuzzy_match_substring() {
        assert!(fuzzy_match("arthur c clarke", "clarke"));
    }

    #[test]
    fn test_fuzzy_match_abbreviation() {
        assert!(fuzzy_match("a clarke", "arthur clarke"));
    }

    #[test]
    fn test_fuzzy_match_reordered() {
        assert!(fuzzy_match("clarke arthur", "arthur clarke"));
    }

    #[test]
    fn test_fuzzy_match_empty() {
        assert!(!fuzzy_match("", "something"));
        assert!(!fuzzy_match("something", ""));
        assert!(!fuzzy_match("", ""));
    }

    #[test]
    fn test_fuzzy_match_no_match() {
        assert!(!fuzzy_match("brandon sanderson", "terry pratchett"));
    }

    // fuzzy_match_title tests

    #[test]
    fn test_title_exact() {
        assert!(fuzzy_match_title("a game of thrones", "a game of thrones"));
    }

    #[test]
    fn test_title_short_substring_rejected() {
        // "star" is only 4/18 = 22% of "starship troopers" — rejected
        assert!(!fuzzy_match_title("star", "starship troopers"));
    }

    #[test]
    fn test_title_short_word_rejected() {
        // "red" is too short relative to "the red queen"
        assert!(!fuzzy_match_title("red", "the red queen"));
    }

    #[test]
    fn test_title_token_overlap() {
        // 5/5 tokens match = 100% overlap
        assert!(fuzzy_match_title(
            "diary of a wimpy kid",
            "diary of a wimpy kid dog days"
        ));
    }

    #[test]
    fn test_title_typo_jaro_winkler() {
        assert!(fuzzy_match_title("the hobbit", "the hobbt"));
    }

    #[test]
    fn test_title_proportional_containment() {
        // "mistborn" (8) / "mistborn the final empire" (25) = 32% — too short
        assert!(!fuzzy_match_title("mistborn", "mistborn the final empire"));
    }

    #[test]
    fn test_title_high_overlap_match() {
        // "the final empire" has 3/3 tokens in "mistborn the final empire" = 100%
        assert!(fuzzy_match_title(
            "the final empire",
            "mistborn the final empire"
        ));
    }

    #[test]
    fn test_title_no_abbreviation_match() {
        // Unlike fuzzy_match, "a" should NOT prefix-match "arthur"
        assert!(!fuzzy_match_title("a tale", "arthur tale"));
    }

    #[test]
    fn test_title_empty() {
        assert!(!fuzzy_match_title("", "something"));
        assert!(!fuzzy_match_title("something", ""));
    }

    #[test]
    fn test_title_embedded_in_series_prefix() {
        // "enders game" is inside "enders saga #01 - enders game" (2 words, 36%)
        assert!(fuzzy_match_title(
            "enders game",
            "enders saga #01 - enders game"
        ));
    }

    #[test]
    fn test_title_completely_different() {
        assert!(!fuzzy_match_title("the great gatsby", "war and peace"));
    }
}
