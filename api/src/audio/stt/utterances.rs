use super::TranscribedSegment;

#[derive(Debug, Clone)]
pub struct TimedWord {
    pub start: f64,
    pub end: f64,
    pub text: String,
}

const PAUSE_SECS: f64 = 0.7;
const SOFT_MAX_SECS: f64 = 12.0;
const SOFT_PAUSE_SECS: f64 = 0.3;
const HARD_MAX_SECS: f64 = 20.0;
const ABBREVIATIONS: &[&str] = &[
    "mr", "mrs", "ms", "dr", "st", "jr", "sr", "prof", "mt", "vs", "etc",
];

pub fn from_words(words: &[TimedWord]) -> Vec<TranscribedSegment> {
    let mut lines: Vec<Vec<&TimedWord>> = Vec::new();
    for word in words.iter().filter(|w| !w.text.trim().is_empty()) {
        let breaks = lines.last().is_none_or(|line| breaks_before(line, word));
        if breaks {
            lines.push(Vec::new());
        }
        lines.last_mut().unwrap().push(word);
    }
    lines
        .into_iter()
        .enumerate()
        .map(|(i, line)| TranscribedSegment {
            index: i as i32,
            start_seconds: line[0].start,
            end_seconds: line[line.len() - 1].end,
            text: line
                .iter()
                .map(|w| w.text.trim())
                .collect::<Vec<_>>()
                .join(" "),
            avg_logprob: None,
            no_speech_prob: None,
            compression_ratio: None,
        })
        .collect()
}

fn breaks_before(line: &[&TimedWord], next: &TimedWord) -> bool {
    let (first, last) = (line[0], line[line.len() - 1]);
    let gap = next.start - last.end;
    let length = next.end - first.start;
    ends_sentence(&last.text)
        || gap >= PAUSE_SECS
        || length > HARD_MAX_SECS
        || (length > SOFT_MAX_SECS && (gap >= SOFT_PAUSE_SECS || ends_clause(&last.text)))
}

fn closing_trimmed(text: &str) -> &str {
    text.trim()
        .trim_end_matches(['"', '\'', ')', ']', '\u{201d}', '\u{2019}'])
}

fn ends_sentence(text: &str) -> bool {
    let word = closing_trimmed(text);
    if word.ends_with(['?', '!', '\u{2026}']) {
        return true;
    }
    let Some(stem) = word.strip_suffix('.') else {
        return false;
    };
    let stem = stem.to_lowercase();
    !ABBREVIATIONS.contains(&stem.as_str())
        && !(stem.len() == 1 && stem.chars().all(char::is_alphabetic))
}

fn ends_clause(text: &str) -> bool {
    closing_trimmed(text).ends_with([',', ';', ':', '\u{2014}'])
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spoken(script: &[(&str, f64)]) -> Vec<TimedWord> {
        let mut at = 0.0;
        script
            .iter()
            .map(|(text, gap)| {
                at += gap;
                let word = TimedWord {
                    start: at,
                    end: at + 0.25,
                    text: text.to_string(),
                };
                at += 0.25;
                word
            })
            .collect()
    }

    fn lines(words: &[TimedWord]) -> Vec<String> {
        from_words(words).into_iter().map(|s| s.text).collect()
    }

    type Case<'a> = (&'a str, Vec<(&'a str, f64)>, Vec<String>);

    #[test]
    fn line_breaks() {
        let steady = |n: usize, word: &'static str| vec![(word, 0.05); n];
        let repeat = |n: usize| vec!["on"; n].join(" ");
        let cases: Vec<Case> = vec![
            (
                "abbreviations and initials do not end a sentence",
                vec![
                    ("Mr.", 0.0),
                    ("Smith", 0.05),
                    ("met", 0.05),
                    ("J.", 0.05),
                    ("Bloggs.", 0.05),
                    ("Then", 0.05),
                    ("left.", 0.05),
                ],
                vec!["Mr. Smith met J. Bloggs.".into(), "Then left.".into()],
            ),
            (
                "closing quotes still end the sentence",
                vec![("\"Stop!\"", 0.0), ("he", 0.05), ("said.", 0.05)],
                vec!["\"Stop!\"".into(), "he said.".into()],
            ),
            (
                "a long run breaks at its first clause end past the soft limit",
                [steady(40, "on"), vec![("and,", 0.05), ("then", 0.05)]].concat(),
                vec![format!("{} and,", repeat(40)), "then".into()],
            ),
            (
                "an unbroken run is cut at the hard limit",
                steady(70, "on"),
                vec![repeat(66), repeat(4)],
            ),
        ];
        for (name, script, expected) in cases {
            assert_eq!(lines(&spoken(&script)), expected, "{name}");
        }
    }

    #[test]
    fn lines_are_timed_from_their_first_word_to_their_last() {
        let words = spoken(&[("One", 1.0), ("two.", 0.25), ("Three", 2.0)]);
        let segments = from_words(&words);
        let spans: Vec<(i32, f64, f64)> = segments
            .iter()
            .map(|s| (s.index, s.start_seconds, s.end_seconds))
            .collect();
        assert_eq!(spans, vec![(0, 1.0, 1.75), (1, 3.75, 4.0)]);
    }
}
