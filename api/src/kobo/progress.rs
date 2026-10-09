//! Shared progress-percent comparison helpers.
//!
//! Kobo ereaders emit slightly different percentages from Hardcover (which computes
//! progress from page counts) and from our own web reader (which computes from EPUB
//! spine position). Treating every float delta as a real change causes spurious
//! "continue reading from another device?" prompts on Nickel. This epsilon is the
//! smallest delta we trust as a genuine move — roughly one page on a 350-page book.

pub const MEANINGFUL_PROGRESS_EPSILON: f64 = 0.3;

/// Returns true when `new` represents a meaningful progress change relative to `old`.
///
/// - Both None → no change.
/// - Exactly one None → meaningful (adding or clearing progress).
/// - Both Some → meaningful iff `|new - old| >= MEANINGFUL_PROGRESS_EPSILON`.
pub fn is_meaningful_delta(old: Option<f64>, new: Option<f64>) -> bool {
    match (old, new) {
        (None, None) => false,
        (Some(_), None) | (None, Some(_)) => true,
        (Some(a), Some(b)) => (b - a).abs() >= MEANINGFUL_PROGRESS_EPSILON,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn none_none_not_meaningful() {
        assert!(!is_meaningful_delta(None, None));
    }

    #[test]
    fn some_none_and_none_some_meaningful() {
        assert!(is_meaningful_delta(Some(17.0), None));
        assert!(is_meaningful_delta(None, Some(17.0)));
    }

    #[test]
    fn below_epsilon_not_meaningful() {
        assert!(!is_meaningful_delta(Some(17.0), Some(17.1)));
        assert!(!is_meaningful_delta(Some(15.0), Some(15.0003)));
        assert!(!is_meaningful_delta(Some(17.0), Some(17.0)));
    }

    #[test]
    fn at_or_above_epsilon_meaningful() {
        assert!(is_meaningful_delta(Some(17.0), Some(17.3)));
        assert!(is_meaningful_delta(Some(17.0), Some(18.0)));
        assert!(is_meaningful_delta(Some(17.0), Some(16.7)));
    }
}
