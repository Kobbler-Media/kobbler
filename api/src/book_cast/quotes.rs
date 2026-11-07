use std::ops::Range;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QuoteStyle {
    Double,
    Single,
}

/// UK-house books quote with ‘ ’ and keep “ ” for quotes inside quotes; the
/// apostrophe shares ’ with the closing mark, so the style is chosen per book
/// from which opener is commoner rather than guessed per paragraph.
pub fn detect<'a>(texts: impl IntoIterator<Item = &'a str>) -> QuoteStyle {
    let (mut double, mut single) = (0usize, 0usize);
    for text in texts {
        double += text.matches(['“', '"']).count();
        single += text.matches('‘').count();
    }
    if single > double {
        QuoteStyle::Single
    } else {
        QuoteStyle::Double
    }
}

/// Byte ranges of the quoted speech in one paragraph, marks included. A quote
/// still open at the paragraph's end runs to it: that is how a speech carried
/// over several paragraphs is printed, each one reopening without a close.
pub fn spans(text: &str, style: QuoteStyle) -> Vec<Range<usize>> {
    let mut out = Vec::new();
    let mut open: Option<usize> = None;
    let mut prev: Option<char> = None;
    let chars: Vec<(usize, char)> = text.char_indices().collect();
    for (i, &(at, c)) in chars.iter().enumerate() {
        let next = chars.get(i + 1).map(|&(_, n)| n);
        let end = at + c.len_utf8();
        match (style, c, open) {
            (QuoteStyle::Double, '“', None) => open = Some(at),
            (QuoteStyle::Double, '”', Some(start)) => {
                out.push(start..end);
                open = None;
            }
            (QuoteStyle::Double, '"', None) if opens(prev) => open = Some(at),
            (QuoteStyle::Double, '"', Some(start)) => {
                out.push(start..end);
                open = None;
            }
            (QuoteStyle::Single, '‘', None) if opens(prev) => open = Some(at),
            (QuoteStyle::Single, '’', Some(start)) if closes(next) => {
                out.push(start..end);
                open = None;
            }
            _ => {}
        }
        prev = Some(c);
    }
    if let Some(start) = open {
        out.push(start..text.len());
    }
    out
}

fn opens(prev: Option<char>) -> bool {
    prev.is_none_or(|p| p.is_whitespace() || matches!(p, '(' | '[' | '—' | '–' | '-'))
}

fn closes(next: Option<char>) -> bool {
    next.is_none_or(|n| !n.is_alphanumeric())
}

/// The words of a quoted span without its marks, as a voice should speak them.
pub fn spoken(span: &str) -> &str {
    span.trim_matches(['“', '”', '"', '‘', '’', ' '])
}

#[cfg(test)]
mod tests {
    use super::*;

    fn quoted(text: &str, style: QuoteStyle) -> Vec<&str> {
        spans(text, style).into_iter().map(|r| &text[r]).collect()
    }

    #[test]
    fn splits_quotes_by_style() {
        let cases: &[(&str, QuoteStyle, &[&str])] = &[
            (
                "“Come in,” she said. “Sit.”",
                QuoteStyle::Double,
                &["“Come in,”", "“Sit.”"],
            ),
            (
                "He said \"no\" twice, \"no.\"",
                QuoteStyle::Double,
                &["\"no\"", "\"no.\""],
            ),
            (
                "‘It isn’t mine,’ said Tom’s sister.",
                QuoteStyle::Single,
                &["‘It isn’t mine,’"],
            ),
            (
                "“And then the long road north",
                QuoteStyle::Double,
                &["“And then the long road north"],
            ),
            ("No speech here at all.", QuoteStyle::Double, &[]),
            (
                "‘He said “run”, so I ran.’",
                QuoteStyle::Single,
                &["‘He said “run”, so I ran.’"],
            ),
        ];
        for (text, style, want) in cases {
            assert_eq!(&quoted(text, *style), want, "{text}");
        }
    }

    #[test]
    fn style_follows_the_commoner_opener() {
        assert_eq!(detect(["‘Hi,’ he said.", "‘Bye.’"]), QuoteStyle::Single);
        assert_eq!(detect(["“Hi,” he said.", "Tom’s"]), QuoteStyle::Double);
    }

    #[test]
    fn spoken_drops_the_marks() {
        assert_eq!(spoken("“Come in,”"), "Come in,");
        assert_eq!(spoken("‘It isn’t mine,’"), "It isn’t mine,");
    }
}
