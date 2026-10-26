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

const UNITS: [&str; 9] = [
    "one", "two", "three", "four", "five", "six", "seven", "eight", "nine",
];
const TEENS: [&str; 10] = [
    "ten",
    "eleven",
    "twelve",
    "thirteen",
    "fourteen",
    "fifteen",
    "sixteen",
    "seventeen",
    "eighteen",
    "nineteen",
];
const TENS: [&str; 8] = [
    "twenty", "thirty", "forty", "fifty", "sixty", "seventy", "eighty", "ninety",
];

// Whisper spells chapter numbers out ("Chapter twenty four"), so the whole
// spoken number has to be read, or every chapter from 20 to 29 comes out as
// "Chapter twenty". Alternations put longer words first because the regex
// engine takes the first alternative that matches: "fourteen" before "four".
// Roman numerals are matched case-sensitively, which keeps "Part I think"
// and "book I knew" from reading as numerals.
static NUMBER: LazyLock<String> = LazyLock::new(|| {
    let mut teens = TEENS.to_vec();
    teens.sort_by_key(|w| std::cmp::Reverse(w.len()));
    let units = UNITS.join("|");
    let below_hundred = format!(
        r"(?:(?:{tens})(?:[\s-]+(?:{units}))?|{teens}|{units})",
        tens = TENS.join("|"),
        teens = teens.join("|"),
    );
    format!(
        r"(?:\d+|(?-i:[IVXLC]+)|(?:(?:one|a)[\s-]+)?hundred(?:[\s-]+(?:and[\s-]+)?{below_hundred})?|{below_hundred})"
    )
});

static CHAPTER_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(&format!(
        r"(?i)^\s*(chapter|part|section|book|act)\s+({})\b",
        *NUMBER
    ))
    .unwrap()
});

static NUMBER_START_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(&format!(r"(?i)^\s*({})\b", *NUMBER)).unwrap());

static NAMED_SECTION_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?i)^\s*(prologue|epilogue|introduction|preface|afterword|foreword|appendix|interlude|postscript)\b",
    )
    .unwrap()
});

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
    if let Some(caps) = CHAPTER_RE.captures(text) {
        let number = caps.get(2)?;
        return numbered_title(&caps[1], number.as_str(), &text[number.end()..]);
    }
    let named = NAMED_SECTION_RE.captures(text)?;
    Some(capitalized(&named[1]))
}

/// Catch chapter announcements split across segment boundaries.
///
/// Whisper/Parakeet sometimes break a "Chapter Fourteen" announcement so
/// that "Chapter" lands at the end of one segment and "Fourteen" at the
/// start of the next. `extract_chapter_title` is anchored at the segment
/// start and would miss both halves. This helper checks for that specific
/// shape: the previous segment ends with a chapter-marker word AND the
/// current segment starts with a number word. A marker that closes a
/// sentence ("…in the book.") is narration, not an announcement.
pub fn extract_split_chapter_title(prev_text: &str, curr_text: &str) -> Option<String> {
    let prev_trimmed = prev_text.trim_end();
    if !prev_trimmed.ends_with(char::is_alphabetic) {
        return None;
    }
    let last_word = prev_trimmed
        .rsplit_once(char::is_whitespace)
        .map(|(_, w)| w)
        .unwrap_or(prev_trimmed);
    if !matches!(
        last_word.to_ascii_lowercase().as_str(),
        "chapter" | "part" | "section" | "book" | "act"
    ) {
        return None;
    }

    let number = NUMBER_START_RE.captures(curr_text)?.get(1)?;
    numbered_title(last_word, number.as_str(), &curr_text[number.end()..])
}

fn numbered_title(marker: &str, number: &str, after: &str) -> Option<String> {
    let after = after.trim_start_matches(|c: char| c.is_whitespace() || c == '.' || c == ',');
    if after.to_ascii_lowercase().starts_with("continued") {
        return None;
    }
    if number == "I" && after.starts_with(char::is_lowercase) {
        return None;
    }
    Some(format!("{} {}", capitalized(marker), number_value(number)?))
}

fn capitalized(word: &str) -> String {
    let lower = word.to_lowercase();
    let mut chars = lower.chars();
    chars
        .next()
        .map(|first| first.to_uppercase().chain(chars).collect())
        .unwrap_or_default()
}

fn number_value(number: &str) -> Option<u32> {
    if let Ok(n) = number.parse() {
        return Some(n);
    }
    if number.chars().all(|c| "IVXLC".contains(c)) {
        return roman_value(number);
    }
    let mut total = 0;
    for word in number.split(|c: char| c.is_whitespace() || c == '-') {
        let word = word.to_lowercase();
        let position = |list: &[&str]| list.iter().position(|w| *w == word).map(|i| i as u32);
        if let Some(i) = position(&UNITS) {
            total += i + 1;
        } else if let Some(i) = position(&TEENS) {
            total += i + 10;
        } else if let Some(i) = position(&TENS) {
            total += (i + 2) * 10;
        } else if word == "hundred" {
            total = total.max(1) * 100;
        }
    }
    (total > 0).then_some(total)
}

fn roman_value(numeral: &str) -> Option<u32> {
    let digit = |c| match c {
        'I' => 1,
        'V' => 5,
        'X' => 10,
        'L' => 50,
        _ => 100,
    };
    let digits: Vec<u32> = numeral.chars().map(digit).collect();
    let total = digits.iter().enumerate().fold(0i64, |acc, (i, d)| {
        if digits.get(i + 1).is_some_and(|next| next > d) {
            acc - *d as i64
        } else {
            acc + *d as i64
        }
    });
    u32::try_from(total).ok().filter(|n| *n > 0)
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
        assert_eq!(result[1].title, "Chapter 14");
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
            Some("Chapter 4".to_string())
        );
    }

    #[test]
    fn test_named_numbers_with_shared_prefixes() {
        // Regression: greedy alternation used to match "fourteen" as "four",
        // "sixteen" as "six", etc. The longer numbers must take precedence.
        for (input, expected) in [
            ("Chapter fourteen Cornelius Fudge", "Chapter 14"),
            ("Chapter Sixteen The Chamber", "Chapter 16"),
            ("CHAPTER seventeen The Heir", "Chapter 17"),
            ("Chapter eighteen Dobby's Reward", "Chapter 18"),
            ("CHAPTER nineteen End of term", "Chapter 19"),
            ("Chapter four At Flourish", "Chapter 4"),
            ("Chapter six Gilderoy Lockhart", "Chapter 6"),
            ("Chapter eight The Deathday", "Chapter 8"),
            ("Chapter nine The Writing", "Chapter 9"),
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
