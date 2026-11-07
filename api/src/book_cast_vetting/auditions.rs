use serde::Serialize;

pub const LINES: usize = 5;
/// Long enough to hear a voice settle, short enough to render in seconds and
/// to sit inside the preview path's own text limit.
pub const MAX_CHARS: usize = 400;
const MIN_CHARS: usize = 12;
/// What a voice is cloned from: koblem asks for 10–15 s of clean speech with
/// its transcript, and one line is often a few words.
pub const REFERENCE_CHARS: usize = 180;

#[derive(Debug, Clone, Serialize)]
pub struct Take {
    pub segment_index: i32,
    pub span_start: Option<i32>,
    pub chapter_index: i32,
    pub text: String,
    pub cue: Option<String>,
}

/// The longest lines show the voice at length; the median ones show how it
/// sounds most of the time. Half of each, longest first.
pub fn pick(mut takes: Vec<Take>, n: usize) -> Vec<Take> {
    takes.retain(|t| t.text.len() <= MAX_CHARS && !t.text.trim().is_empty());
    let enough: Vec<Take> = takes
        .iter()
        .filter(|t| t.text.len() >= MIN_CHARS)
        .cloned()
        .collect();
    let mut takes = if enough.is_empty() { takes } else { enough };
    takes.sort_by_key(|t| std::cmp::Reverse(t.text.len()));
    if takes.len() <= n {
        return takes;
    }
    let longest = n.div_ceil(2);
    let mut out: Vec<Take> = takes.drain(..longest).collect();
    let mid = takes.len() / 2;
    let start = mid.saturating_sub((n - longest) / 2);
    out.extend(takes.drain(start..start + (n - longest)));
    out
}

/// The chosen line and the character's next lines, in book order, until there
/// is enough to clone a voice from.
pub fn reference(takes: &[Take], segment_index: i32, span_start: Option<i32>) -> Option<String> {
    let start = takes
        .iter()
        .position(|t| t.segment_index == segment_index && t.span_start == span_start)?;
    let mut out = takes[start].text.trim().to_string();
    for t in &takes[start + 1..] {
        let next = t.text.trim();
        if out.len() >= REFERENCE_CHARS || out.len() + 1 + next.len() > MAX_CHARS {
            break;
        }
        out.push(' ');
        out.push_str(next);
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn take(len: usize) -> Take {
        Take {
            segment_index: len as i32,
            span_start: Some(0),
            chapter_index: 0,
            text: "x".repeat(len),
            cue: None,
        }
    }

    #[test]
    fn a_reference_reads_on_until_long_enough_to_clone_from() {
        let takes: Vec<Take> = [100, 60, 50, 360, 10]
            .iter()
            .enumerate()
            .map(|(i, &len)| Take {
                segment_index: i as i32,
                ..take(len)
            })
            .collect();
        let cases: &[(i32, usize, &str)] = &[
            (0, 100 + 1 + 60 + 1 + 50, "stops once past the target"),
            (2, 50, "never runs past the preview limit"),
            (3, 360, "a long line is enough on its own"),
            (4, 10, "the last line is all there is"),
        ];
        for (from, len, why) in cases {
            assert_eq!(
                reference(&takes, *from, Some(0)).unwrap().len(),
                *len,
                "{why}"
            );
        }
        assert!(reference(&takes, 9, Some(0)).is_none());
    }

    #[test]
    fn picks_longest_then_typical_within_the_limit() {
        let lengths = [20, 30, 40, 50, 60, 70, 80, 90, 100, 500, 3];
        let picked: Vec<usize> = pick(lengths.iter().map(|&l| take(l)).collect(), 4)
            .iter()
            .map(|t| t.text.len())
            .collect();
        assert_eq!(picked, [100, 90, 60, 50]);
        let few: Vec<usize> = pick(vec![take(3), take(5)], 4)
            .iter()
            .map(|t| t.text.len())
            .collect();
        assert_eq!(
            few,
            [5, 3],
            "a speaker of nothing but short lines still auditions"
        );
    }
}
