//! Ebook or audiobook, without asking every time.
//!
//! Asking every time is the friction that kills the whole thing, so the
//! question is reserved for a history that genuinely does not lean either way.

use super::types::{FormatSkew, WantFormat};

/// Below this there is no history to read, and a default beats a question.
pub const MIN_EVIDENCE: i64 = 4;
/// Outside this band the history has an answer in it.
pub const AUDIO_LEAN: f64 = 0.65;
pub const EBOOK_LEAN: f64 = 0.35;

pub const FORMAT_QUESTION: &str = "Ebook or audiobook?";

/// 1. What she said. 2. What she usually does. 3. Only then, ask.
pub fn resolve_format(
    explicit: Option<WantFormat>,
    skew: FormatSkew,
) -> (Option<WantFormat>, Option<String>) {
    if let Some(format) = explicit {
        return (Some(format), None);
    }
    let total = skew.ebook + skew.audiobook;
    if total < MIN_EVIDENCE {
        return (Some(WantFormat::Ebook), None);
    }
    let audio_ratio = skew.audiobook as f64 / total as f64;
    if audio_ratio >= AUDIO_LEAN {
        (Some(WantFormat::Audiobook), None)
    } else if audio_ratio <= EBOOK_LEAN {
        (Some(WantFormat::Ebook), None)
    } else {
        (None, Some(FORMAT_QUESTION.to_string()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn skew(ebook: i64, audiobook: i64) -> FormatSkew {
        FormatSkew { ebook, audiobook }
    }

    #[test]
    fn explicit_beats_history() {
        let (format, question) = resolve_format(Some(WantFormat::Audiobook), skew(50, 0));
        assert_eq!(format, Some(WantFormat::Audiobook));
        assert!(question.is_none());
    }

    #[test]
    fn history_decides_when_it_leans() {
        assert_eq!(
            resolve_format(None, skew(1, 9)).0,
            Some(WantFormat::Audiobook)
        );
        assert_eq!(resolve_format(None, skew(9, 1)).0, Some(WantFormat::Ebook));
    }

    #[test]
    fn only_a_genuinely_split_history_gets_the_question() {
        let (format, question) = resolve_format(None, skew(5, 5));
        assert!(format.is_none());
        assert_eq!(question.as_deref(), Some(FORMAT_QUESTION));
    }

    #[test]
    fn no_history_is_a_default_not_a_question() {
        let (format, question) = resolve_format(None, skew(0, 0));
        assert_eq!(format, Some(WantFormat::Ebook));
        assert!(question.is_none());
        assert!(resolve_format(None, skew(1, 1)).1.is_none());
    }
}
