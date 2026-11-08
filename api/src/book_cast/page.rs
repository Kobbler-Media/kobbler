//! Where a page the reader shows sits among the cast's lines. The reader sends
//! the text on screen, not where it is, so each of its paragraphs is found in
//! the book's extracted text by letters and digits alone: the two disagree on
//! whitespace, quote marks and the odd soft hyphen, never on the words.

use std::collections::HashMap;
use std::ops::Range;

use crate::repositories::book_casts::CastLine;

/// How far past the last paragraph found the next one may start, in key bytes:
/// room for a heading or a footnote the reader shows and the extract dropped.
const SLACK: usize = 2_000;
/// The shortest paragraph trusted to fix the page in the book on its own.
const ANCHOR_MIN: usize = 24;

struct Key {
    text: String,
    at: Vec<usize>,
}

fn key(s: &str) -> Key {
    let mut text = String::new();
    let mut at = Vec::new();
    for (b, c) in s.char_indices().filter(|(_, c)| c.is_alphanumeric()) {
        for l in c.to_lowercase() {
            at.extend(std::iter::repeat_n(b, l.len_utf8()));
            text.push(l);
        }
    }
    Key { text, at }
}

pub struct Book<'a> {
    segments: &'a [(i32, &'a str)],
    text: String,
    starts: Vec<usize>,
}

impl<'a> Book<'a> {
    pub fn new(segments: &'a [(i32, &'a str)]) -> Self {
        let mut text = String::new();
        let mut starts = Vec::with_capacity(segments.len());
        for (_, s) in segments {
            starts.push(text.len());
            text.push_str(&key(s).text);
        }
        Self {
            segments,
            text,
            starts,
        }
    }

    fn segment_at(&self, k: usize) -> usize {
        self.starts.partition_point(|&s| s <= k) - 1
    }
}

struct Placed {
    page: Range<usize>,
    key: Key,
    book: usize,
}

pub struct Placement {
    paragraphs: Vec<Placed>,
    pub segments: Range<i32>,
}

/// The page's paragraphs found in the book, or `None` when none of them can
/// be told apart from the rest of it.
pub fn locate(page: &str, book: &Book) -> Option<Placement> {
    let mut paragraphs: Vec<(Range<usize>, Key)> = Vec::new();
    let mut from = 0;
    for p in page.split("\n\n") {
        let range = from..from + p.len();
        from = range.end + 2;
        let k = key(p);
        if !k.text.is_empty() {
            paragraphs.push((range, k));
        }
    }
    let anchor = paragraphs
        .iter()
        .enumerate()
        .filter(|(_, (_, k))| k.text.len() >= ANCHOR_MIN)
        .filter_map(|(i, (_, k))| {
            let at = book.text.find(&k.text)?;
            (book.text.rfind(&k.text) == Some(at)).then_some((i, at))
        })
        .max_by_key(|&(i, _)| paragraphs[i].1.text.len())?;

    let mut found: Vec<Option<usize>> = vec![None; paragraphs.len()];
    found[anchor.0] = Some(anchor.1);
    let mut cursor = anchor.1 + paragraphs[anchor.0].1.text.len();
    for (i, (_, k)) in paragraphs.iter().enumerate().skip(anchor.0 + 1) {
        let window = &book.text[cursor..(cursor + SLACK + k.text.len()).min(book.text.len())];
        if let Some(at) = window.find(&k.text) {
            found[i] = Some(cursor + at);
            cursor += at + k.text.len();
        }
    }
    let mut cursor = anchor.1;
    for (i, (_, k)) in paragraphs.iter().enumerate().take(anchor.0).rev() {
        let window = &book.text[cursor.saturating_sub(SLACK + k.text.len())..cursor];
        if let Some(at) = window.rfind(&k.text) {
            let at = cursor - window.len() + at;
            found[i] = Some(at);
            cursor = at;
        }
    }

    let paragraphs: Vec<Placed> = paragraphs
        .into_iter()
        .zip(found)
        .filter_map(|((page, key), at)| {
            Some(Placed {
                page,
                key,
                book: at?,
            })
        })
        .collect();
    let first = paragraphs.first()?;
    let last = paragraphs.last()?;
    let segments = book.segments[book.segment_at(first.book)].0
        ..book.segments[book.segment_at(last.book + last.key.text.len() - 1)].0 + 1;
    Some(Placement {
        paragraphs,
        segments,
    })
}

#[derive(Debug, Clone, PartialEq)]
pub struct Part {
    pub range: Range<usize>,
    pub line: Option<usize>,
}

/// The page cut into narration and the lines a character speaks, in reading
/// order. The cuts fall on whitespace, so the parts hold every word of the page
/// once: the reader's highlight counts words across them.
pub fn parts(page: &str, book: &Book, placement: &Placement, lines: &[CastLine]) -> Vec<Part> {
    let mut by_segment: HashMap<i32, Vec<usize>> = HashMap::new();
    for (i, l) in lines.iter().enumerate() {
        if l.character_id.is_some() && !l.not_speech {
            by_segment.entry(l.segment_index).or_default().push(i);
        }
    }
    let mut spoken: Vec<(Range<usize>, usize)> = Vec::new();
    for p in &placement.paragraphs {
        let end = p.book + p.key.text.len();
        for s in book.segment_at(p.book)..=book.segment_at(end - 1) {
            let (segment_index, text) = book.segments[s];
            let Some(own) = by_segment.get(&segment_index) else {
                continue;
            };
            let seg = key(text);
            for &i in own {
                let l = &lines[i];
                let from = book.starts[s] + seg.at.partition_point(|&b| b < l.span_start as usize);
                let to = book.starts[s] + seg.at.partition_point(|&b| b < l.span_end as usize);
                let (from, to) = (from.max(p.book), to.min(end));
                if from >= to {
                    continue;
                }
                let last = p.key.at[to - 1 - p.book];
                let range = p.page.start + p.key.at[from - p.book]
                    ..p.page.start + last + char_len(page, p.page.start + last);
                spoken.push((widen(page, range), i));
            }
        }
    }
    spoken.sort_by_key(|(r, _)| r.start);

    let mut out: Vec<Part> = Vec::new();
    let mut cursor = 0;
    for (range, line) in spoken {
        if range.start < cursor {
            continue;
        }
        narrate(page, cursor..range.start, &mut out);
        out.push(Part {
            range: range.clone(),
            line: Some(line),
        });
        cursor = range.end;
    }
    narrate(page, cursor..page.len(), &mut out);
    out
}

fn char_len(s: &str, at: usize) -> usize {
    s[at..].chars().next().map_or(0, char::len_utf8)
}

fn widen(page: &str, range: Range<usize>) -> Range<usize> {
    let start = page[..range.start]
        .rfind(|c: char| c.is_ascii_whitespace())
        .map_or(0, |i| i + 1);
    let end = page[range.end..]
        .find(|c: char| c.is_ascii_whitespace())
        .map_or(page.len(), |i| range.end + i);
    start..end
}

/// Words with nothing to say aloud, a dash between two lines, ride on the part
/// before them rather than going to the engine alone.
fn narrate(page: &str, range: Range<usize>, out: &mut Vec<Part>) {
    let text = &page[range.clone()];
    if text.trim().is_empty() {
        return;
    }
    if !text.chars().any(char::is_alphanumeric)
        && let Some(prev) = out.last_mut()
    {
        prev.range.end = range.end;
        return;
    }
    out.push(Part { range, line: None });
}

#[cfg(test)]
mod tests {
    use super::*;
    use uuid::Uuid;

    fn line(segments: &[(i32, &str)], segment_index: i32, quote: &str, who: bool) -> CastLine {
        let text = segments.iter().find(|s| s.0 == segment_index).unwrap().1;
        let start = text.find(quote).unwrap() as i32;
        CastLine {
            segment_index,
            span_start: start,
            span_end: start + quote.len() as i32,
            chapter_index: 1,
            speaker: None,
            character_id: who.then(Uuid::now_v7),
            cue: None,
            source: "gemma".into(),
            not_speech: false,
        }
    }

    fn cut<'a>(
        page: &'a str,
        segments: &[(i32, &str)],
        lines: &[CastLine],
    ) -> Vec<(&'a str, Option<usize>)> {
        let book = Book::new(segments);
        let placement = locate(page, &book).expect("the page is in the book");
        parts(page, &book, &placement, lines)
            .into_iter()
            .map(|p| (&page[p.range], p.line))
            .collect()
    }

    const BOOK: &[(i32, &str)] = &[
        (0, "Chapter One"),
        (
            1,
            "The fire had burned low by the time the travellers came in from the rain.",
        ),
        (
            2,
            "“Come in,” said Ann, \"and sit.\" She pointed at the bench — “There.”",
        ),
        (3, "“Yes.”"),
        (
            4,
            "Nobody moved. The sign over the bar read “CLOSED”, and the rain kept on.",
        ),
        (5, "“Yes.”"),
    ];

    #[test]
    fn a_page_is_cut_on_whitespace_into_narration_and_the_lines_spoken() {
        let lines = [
            line(BOOK, 2, "“Come in,”", true),
            line(BOOK, 2, "\"and sit.\"", true),
            line(BOOK, 2, "“There.”", true),
            line(BOOK, 3, "“Yes.”", true),
            line(BOOK, 4, "“CLOSED”", false),
            line(BOOK, 5, "“Yes.”", true),
        ];
        let page = "the travellers came in from the rain.\n\n\u{201c}Come in,\u{201d} said Ann, \u{201c}and sit.\u{201d} She pointed at the bench \u{2014} \u{201c}There.\u{201d}\n\n\u{201c}Yes.\u{201d}\n\nNobody moved. The sign over the bar read \u{201c}CLOSED\u{201d}, and";
        assert_eq!(
            cut(page, BOOK, &lines),
            [
                ("the travellers came in from the rain.\n\n", None),
                ("“Come in,”", Some(0)),
                (" said Ann, ", None),
                ("“and sit.”", Some(1)),
                (" She pointed at the bench — ", None),
                ("“There.”", Some(2)),
                ("\n\n", None),
                ("“Yes.”", Some(3)),
                (
                    "\n\nNobody moved. The sign over the bar read “CLOSED”, and",
                    None
                ),
            ]
            .into_iter()
            .filter(|(t, _)| !t.trim().is_empty())
            .collect::<Vec<_>>()
        );
    }
}
