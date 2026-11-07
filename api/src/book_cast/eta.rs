use std::collections::HashMap;

use time::OffsetDateTime;

use crate::repositories::book_casts::{CastState, ChapterTime};

use super::voices::BATCH;

/// Rough by design: pass 2 is one long request to beefy over every speaker's
/// summary, pass 3 a request per batch of voices, and neither has a history to
/// measure until it has run.
const BIBLE_SECS: i64 = 600;
const VOICE_BATCH_SECS: i64 = 90;

/// The time the work so far took, scaled to the text still to go.
pub fn remaining(done_chars: u64, left_chars: u64, spent_secs: f64) -> Option<i64> {
    if done_chars == 0 || spent_secs <= 0.0 {
        return None;
    }
    Some((spent_secs / done_chars as f64 * left_chars as f64).ceil() as i64)
}

/// What is left of the chapter pass, from the chapters this run has finished.
pub fn pass1(
    chapter_chars: &HashMap<i32, u64>,
    finished: &[ChapterTime],
    started: OffsetDateTime,
) -> Option<i64> {
    let this_run: Vec<&ChapterTime> = finished
        .iter()
        .filter(|t| t.finished_at >= started)
        .collect();
    let last = this_run.iter().map(|t| t.finished_at).max()?;
    let chars = |i: &i32| chapter_chars.get(i).copied().unwrap_or_default();
    let done: u64 = this_run.iter().map(|t| chars(&t.chapter_index)).sum();
    let left: u64 = chapter_chars
        .keys()
        .filter(|i| !finished.iter().any(|t| t.chapter_index == **i))
        .map(chars)
        .sum();
    remaining(done, left, (last - started).as_seconds_f64())
}

/// `pass1` is what is left of the chapter pass, `voices` how many voices the
/// voice pass will design.
pub fn analysis(state: CastState, pass1: Option<i64>, voices: usize) -> Option<i64> {
    let pass3 = voices.div_ceil(BATCH) as i64 * VOICE_BATCH_SECS;
    match state {
        CastState::Chapters => pass1.map(|s| s + BIBLE_SECS + pass3),
        CastState::Bible => Some(BIBLE_SECS + pass3),
        CastState::Voices => Some(pass3),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scales_the_work_done_to_the_work_left() {
        assert_eq!(remaining(1_000, 3_000, 60.0), Some(180));
        assert_eq!(remaining(0, 3_000, 60.0), None, "nothing to measure yet");
        assert_eq!(remaining(1_000, 0, 60.0), Some(0));
        assert_eq!(
            analysis(CastState::Chapters, Some(100), 7),
            Some(100 + BIBLE_SECS + 2 * VOICE_BATCH_SECS)
        );
        assert_eq!(analysis(CastState::Chapters, None, 7), None);
        assert_eq!(analysis(CastState::Voices, None, 6), Some(VOICE_BATCH_SECS));
        assert_eq!(analysis(CastState::Casting, Some(5), 6), None);
    }
}
