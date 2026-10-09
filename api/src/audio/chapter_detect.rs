//! Transcript-based chapter detection for audiobooks.
//!
//! Analyses transcribed text segments to detect chapter boundaries using
//! text cues (e.g. "Chapter 1", "Prologue", "Part Two").
//! Returns empty if fewer than 2 text cues are found.

use regex::Regex;
use std::sync::LazyLock;

/// Input segment for chapter detection.
pub struct DetectionSegment {
    pub start_seconds: f64,
    pub end_seconds: f64,
    pub text: String,
    pub no_speech_prob: Option<f64>,
}

/// A detected chapter boundary.
pub struct DetectedChapter {
    pub title: String,
    pub start_seconds: f64,
}

// Text patterns that signal chapter boundaries.
//
// Number-word alternations are ordered LONGER-FIRST so the regex engine
// (greedy-leftmost) matches "fourteen" before falling back to "four". A
// trailing `\b` would also work but explicit ordering makes the intent
// obvious and avoids subtle word-boundary surprises with punctuation.
static CHAPTER_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?i)^\s*(chapter|part|section|book)\s+(\d+|[IVXLC]+|thirteen|fourteen|fifteen|sixteen|seventeen|eighteen|nineteen|twenty|eleven|twelve|ten|one|two|three|four|five|six|seven|eight|nine)\b",
    )
    .unwrap()
});

static NAMED_SECTION_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?i)^\s*(prologue|epilogue|introduction|preface|afterword|foreword|appendix|interlude|postscript)\b",
    )
    .unwrap()
});

static ACT_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)^\s*act\s+(\d+|[IVXLC]+)\b").unwrap());

/// Chapters closer together than this get merged.
const MIN_CHAPTER_DURATION: f64 = 60.0;

/// Detect chapter boundaries from transcript segments.
///
/// Returns detected chapters with titles and start timestamps.
/// Returns empty if fewer than 2 chapters detected (not useful enough).
pub fn detect_chapters(segments: &[DetectionSegment]) -> Vec<DetectedChapter> {
    if segments.is_empty() {
        return Vec::new();
    }

    let mut chapters = Vec::new();

    // Pass 1: text cue detection. Each segment is checked alone, then with
    // its predecessor as a tail, to catch announcements split across a
    // segment boundary.
    for (i, seg) in segments.iter().enumerate() {
        if let Some(title) = extract_chapter_title(&seg.text) {
            chapters.push(DetectedChapter {
                title,
                start_seconds: seg.start_seconds,
            });
        } else if i > 0
            && let Some(title) = extract_split_chapter_title(&segments[i - 1].text, &seg.text)
        {
            chapters.push(DetectedChapter {
                title,
                start_seconds: seg.start_seconds,
            });
        }
    }

    if chapters.len() >= 2 {
        return finalize(chapters, segments);
    }

    // Not enough text cues found — return empty
    Vec::new()
}

/// Extract a chapter title from segment text, if it matches a known pattern.
///
/// Returns None for "Chapter X continued" markers — multi-CD audiobook rips
/// frequently spoken-mark the start of each file as a continuation of the
/// previous file's chapter, and those should not create a new chapter.
pub fn extract_chapter_title(text: &str) -> Option<String> {
    let raw_match = CHAPTER_RE
        .find(text)
        .or_else(|| NAMED_SECTION_RE.find(text))
        .or_else(|| ACT_RE.find(text))?;
    let title = raw_match.as_str().trim().to_string();

    // Skip continuation markers ("Chapter four continued", etc.)
    let after = text[raw_match.end()..]
        .trim_start_matches(|c: char| c.is_whitespace() || c == '.' || c == ',');
    if after.to_ascii_lowercase().starts_with("continued") {
        return None;
    }

    Some(title)
}

/// Catch chapter announcements split across segment boundaries.
///
/// Whisper/Parakeet sometimes break a "Chapter Fourteen" announcement so
/// that "Chapter" lands at the end of one segment and "Fourteen" at the
/// start of the next. `extract_chapter_title` is anchored at the segment
/// start and would miss both halves. This helper checks for that specific
/// shape: the previous segment ends with a chapter-marker word AND the
/// current segment starts with a number word.
pub fn extract_split_chapter_title(prev_text: &str, curr_text: &str) -> Option<String> {
    static CHAPTER_NUMBER_START_RE: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(
            r"(?i)^\s*(\d+|[IVXLC]+|thirteen|fourteen|fifteen|sixteen|seventeen|eighteen|nineteen|twenty|eleven|twelve|ten|one|two|three|four|five|six|seven|eight|nine)\b",
        )
        .unwrap()
    });

    let prev_trimmed =
        prev_text.trim_end_matches(|c: char| c.is_whitespace() || c.is_ascii_punctuation());
    let last_word = prev_trimmed
        .rsplit_once(char::is_whitespace)
        .map(|(_, w)| w)
        .unwrap_or(prev_trimmed);
    let marker = matches!(
        last_word.to_ascii_lowercase().as_str(),
        "chapter" | "part" | "section" | "book" | "act"
    );
    if !marker {
        return None;
    }

    let m = CHAPTER_NUMBER_START_RE.find(curr_text)?;
    let after = curr_text[m.end()..]
        .trim_start_matches(|c: char| c.is_whitespace() || c == '.' || c == ',');
    if after.to_ascii_lowercase().starts_with("continued") {
        return None;
    }
    Some(format!("{} {}", last_word, m.as_str().trim()))
}

/// Merge chapters that are too close together and ensure first chapter starts at 0.
fn finalize(
    mut chapters: Vec<DetectedChapter>,
    segments: &[DetectionSegment],
) -> Vec<DetectedChapter> {
    if chapters.is_empty() {
        return chapters;
    }

    // Ensure first chapter starts at segment 0
    chapters[0].start_seconds = segments[0].start_seconds;

    // Merge chapters that are < MIN_CHAPTER_DURATION apart
    let mut merged = vec![chapters.remove(0)];
    for ch in chapters {
        let last = merged.last().unwrap();
        if ch.start_seconds - last.start_seconds < MIN_CHAPTER_DURATION {
            // Too close — keep the earlier one (it likely has the real title)
            continue;
        }
        merged.push(ch);
    }

    // Need at least 2 chapters to be useful
    if merged.len() < 2 {
        return Vec::new();
    }

    merged
}

#[cfg(test)]
mod tests {
    use super::*;

    fn seg(start: f64, end: f64, text: &str) -> DetectionSegment {
        DetectionSegment {
            start_seconds: start,
            end_seconds: end,
            text: text.to_string(),
            no_speech_prob: Some(0.1),
        }
    }

    #[test]
    fn test_text_cue_detection() {
        let segments = vec![
            seg(0.0, 5.0, "Chapter 1. It was a dark and stormy night."),
            seg(5.0, 10.0, "The wind howled through the trees."),
            seg(300.0, 305.0, "Chapter 2. The next morning dawned bright."),
            seg(305.0, 310.0, "She walked to the market."),
            seg(600.0, 605.0, "Chapter 3. The confrontation."),
            seg(605.0, 610.0, "They met at the crossroads."),
        ];
        let result = detect_chapters(&segments);
        assert_eq!(result.len(), 3);
        assert_eq!(result[0].title, "Chapter 1");
        assert_eq!(result[1].title, "Chapter 2");
        assert_eq!(result[2].title, "Chapter 3");
    }

    #[test]
    fn test_named_sections() {
        let segments = vec![
            seg(0.0, 5.0, "Prologue. In the beginning there was nothing."),
            seg(5.0, 10.0, "Nothing but silence."),
            seg(300.0, 305.0, "Chapter 1. Then something happened."),
            seg(600.0, 605.0, "Epilogue. And so it ended."),
        ];
        let result = detect_chapters(&segments);
        assert_eq!(result.len(), 3);
        assert_eq!(result[0].title, "Prologue");
        assert_eq!(result[1].title, "Chapter 1");
        assert_eq!(result[2].title, "Epilogue");
    }

    #[test]
    fn test_no_text_cues_returns_empty() {
        // No chapter text cues → should return empty (no silence fallback)
        let segments = vec![
            seg(0.0, 100.0, "Some narration."),
            seg(100.0, 200.0, "More narration."),
            seg(208.0, 300.0, "After a gap."),
            seg(300.0, 400.0, "Continuing."),
        ];
        let result = detect_chapters(&segments);
        assert!(result.is_empty());
    }

    #[test]
    fn test_single_chapter_returns_empty() {
        let segments = vec![
            seg(0.0, 100.0, "Chapter 1. The only chapter."),
            seg(100.0, 200.0, "Still going."),
        ];
        let result = detect_chapters(&segments);
        assert!(result.is_empty());
    }

    #[test]
    fn test_empty_segments() {
        let result = detect_chapters(&[]);
        assert!(result.is_empty());
    }

    #[test]
    fn test_split_chapter_announcement_across_segments() {
        // Regression: "Chapter Fourteen" sometimes splits with "Chapter" at
        // the end of one segment and "Fourteen" at the start of the next.
        // detect_chapters should still pick it up.
        let segments = vec![
            seg(0.0, 5.0, "Chapter 1. The very first words."),
            // "Chapter" trails the previous narration, then a new segment
            // starts with the number — typical Whisper boundary split.
            seg(300.0, 305.0, "...outside Harry's window. Chapter"),
            seg(305.0, 310.0, "Fourteen Cornelius Fudge entered the room."),
            seg(600.0, 605.0, "Chapter 3. The end."),
        ];
        let result = detect_chapters(&segments);
        assert_eq!(result.len(), 3);
        assert_eq!(result[0].title, "Chapter 1");
        assert_eq!(result[1].title, "Chapter Fourteen");
        // start_seconds for the split chapter should be the *current*
        // segment's start (where the number-word is), not the previous one.
        assert_eq!(result[1].start_seconds, 305.0);
        assert_eq!(result[2].title, "Chapter 3");
    }

    #[test]
    fn test_split_chapter_skips_continuation() {
        // "Chapter Four continued." split across segments — should still skip.
        let segments = vec![
            seg(0.0, 5.0, "Chapter 1. Start."),
            seg(300.0, 305.0, "...the previous chapter ended. Chapter"),
            seg(305.0, 310.0, "Four continued, with no new content."),
            seg(600.0, 605.0, "Chapter 3. The end."),
        ];
        let result = detect_chapters(&segments);
        // Only Chapter 1 and Chapter 3 — the split "Chapter Four continued"
        // should NOT count as a new chapter.
        assert_eq!(result.len(), 2);
        assert_eq!(result[0].title, "Chapter 1");
        assert_eq!(result[1].title, "Chapter 3");
    }

    #[test]
    fn test_continuation_markers_skipped() {
        // Multi-CD audiobook rips typically start each file with a spoken
        // "Chapter X continued" marker — those should NOT register as new
        // chapter starts, the chapter began on the previous file.
        for input in [
            "CHAPTER four continued.",
            "Chapter eight continued.",
            "Chapter twelve continued The labyrinthine passages were deserted.",
            "CHAPTER ten continued",
        ] {
            assert_eq!(extract_chapter_title(input), None, "input was {input:?}");
        }
        // Sanity: the same titles without "continued" still match.
        assert_eq!(
            extract_chapter_title("CHAPTER four At Flourish and Blots"),
            Some("CHAPTER four".to_string())
        );
    }

    #[test]
    fn test_named_numbers_with_shared_prefixes() {
        // Regression: greedy alternation used to match "fourteen" as "four",
        // "sixteen" as "six", etc. The longer numbers must take precedence.
        for (input, expected) in [
            ("Chapter fourteen Cornelius Fudge", "Chapter fourteen"),
            ("Chapter Sixteen The Chamber", "Chapter Sixteen"),
            ("CHAPTER seventeen The Heir", "CHAPTER seventeen"),
            ("Chapter eighteen Dobby's Reward", "Chapter eighteen"),
            ("CHAPTER nineteen End of term", "CHAPTER nineteen"),
            ("Chapter four At Flourish", "Chapter four"),
            ("Chapter six Gilderoy Lockhart", "Chapter six"),
            ("Chapter eight The Deathday", "Chapter eight"),
            ("Chapter nine The Writing", "Chapter nine"),
        ] {
            let got = extract_chapter_title(input).unwrap_or_default();
            assert_eq!(got, expected, "input was {input:?}");
        }
    }

    #[test]
    fn test_merge_close_chapters() {
        let segments = vec![
            seg(0.0, 5.0, "Chapter 1. Start."),
            seg(5.0, 10.0, "Chapter 2. Too close to chapter 1."),
            seg(300.0, 305.0, "Chapter 3. Far enough away."),
        ];
        let result = detect_chapters(&segments);
        // Chapter 2 should be merged into Chapter 1
        assert_eq!(result.len(), 2);
        assert_eq!(result[0].title, "Chapter 1");
        assert_eq!(result[1].title, "Chapter 3");
    }
}
