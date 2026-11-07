use std::ops::Range;

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use super::quotes::{self, QuoteStyle};
use crate::repositories::book_casts::CastChapter;

pub struct Paragraph {
    pub segment_index: i32,
    pub text: String,
}

pub struct Chapter {
    pub index: i32,
    pub title: Option<String>,
    pub paragraphs: Vec<Paragraph>,
}

/// Chapters as runs of consecutive segments sharing a title; a book with no
/// titles at all is one chapter.
pub fn group(segments: impl IntoIterator<Item = (i32, String, Option<String>)>) -> Vec<Chapter> {
    let mut out: Vec<Chapter> = Vec::new();
    for (segment_index, text, title) in segments {
        let paragraph = Paragraph {
            segment_index,
            text,
        };
        match out.last_mut() {
            Some(chapter) if chapter.title == title => chapter.paragraphs.push(paragraph),
            _ => out.push(Chapter {
                index: out.len() as i32,
                title,
                paragraphs: vec![paragraph],
            }),
        }
    }
    out
}

pub struct Quote {
    pub segment_index: i32,
    pub span: Range<usize>,
}

/// A slice of a chapter small enough for one gemma request, its quotes
/// numbered from 0 and marked `[Qn]` in the text.
pub struct Window {
    pub text: String,
    pub quotes: Vec<Quote>,
}

pub fn windows(chapter: &Chapter, style: QuoteStyle, max_chars: usize) -> Vec<Window> {
    let mut out = Vec::new();
    let mut current = Window {
        text: String::new(),
        quotes: Vec::new(),
    };
    for p in &chapter.paragraphs {
        if !current.text.is_empty() && current.text.len() + p.text.len() > max_chars {
            out.push(std::mem::replace(
                &mut current,
                Window {
                    text: String::new(),
                    quotes: Vec::new(),
                },
            ));
        }
        let mut marked = String::with_capacity(p.text.len() + 16);
        let mut at = 0;
        for span in quotes::spans(&p.text, style) {
            marked.push_str(&p.text[at..span.start]);
            marked.push_str(&format!("[Q{}]", current.quotes.len()));
            marked.push_str(&p.text[span.clone()]);
            at = span.end;
            current.quotes.push(Quote {
                segment_index: p.segment_index,
                span,
            });
        }
        marked.push_str(&p.text[at..]);
        current.text.push_str(&marked);
        current.text.push_str("\n\n");
    }
    if !current.text.is_empty() {
        out.push(current);
    }
    out
}

#[derive(Debug, Deserialize)]
pub struct Reply {
    pub lines: Vec<ReplyLine>,
    pub characters: Vec<CharacterNote>,
}

#[derive(Debug, Deserialize)]
pub struct ReplyLine {
    pub q: usize,
    pub speaker: String,
    pub cue: String,
}

/// One character as a single chapter shows them; the later passes read every
/// chapter's notes on a character to find their shape across the book.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CharacterNote {
    pub name: String,
    pub aliases: Vec<String>,
    pub gender: String,
    pub age: String,
    pub role: String,
    pub attitude: String,
    pub relationships: String,
    pub speech: String,
    pub generic: bool,
}

impl CharacterNote {
    pub fn profile(&self) -> String {
        [
            ("gender", &self.gender),
            ("age", &self.age),
            ("role", &self.role),
            ("attitude", &self.attitude),
            ("relationships", &self.relationships),
            ("speech", &self.speech),
        ]
        .into_iter()
        .filter(|(_, v)| !v.trim().is_empty())
        .map(|(label, v)| format!("{label}: {}", v.trim()))
        .collect::<Vec<_>>()
        .join("; ")
    }
}

pub fn notes(chapter: &CastChapter) -> Vec<CharacterNote> {
    serde_json::from_value(chapter.notes["characters"].clone()).unwrap_or_default()
}

/// Words the model writes as a cue that say nothing about how a line sounds; a
/// TTS reads them as a direction, so they are no cue at all.
const NOT_DELIVERY: &[&str] = &[
    "plain",
    "normal",
    "neutral",
    "none",
    "said",
    "says",
    "saying",
    "asked",
    "asks",
    "asking",
    "replied",
    "replies",
    "replying",
    "answered",
    "answering",
    "responded",
    "responding",
    "continued",
    "continuing",
    "explained",
    "explaining",
    "stated",
    "stating",
    "statement",
    "spoke",
    "speaking",
    "told",
    "telling",
];

pub fn delivery(cue: &str) -> Option<String> {
    let cue = cue.trim();
    (!cue.is_empty() && !NOT_DELIVERY.iter().any(|w| cue.eq_ignore_ascii_case(w)))
        .then(|| cue.to_string())
}

/// A whole window's quotes, a short line each, plus a profile per speaker.
pub const MAX_TOKENS: u32 = 16_384;

pub const NOT_SPEECH: &str = "not_speech";
pub const UNKNOWN: &str = "unknown";

/// Names compare as the model writes them back, which is not always as it read
/// them: case, curly or straight apostrophes and stray spaces all vary.
pub fn same_name(a: &str, b: &str) -> bool {
    let key = |s: &str| -> String {
        s.chars()
            .filter(|c| !c.is_whitespace())
            .map(|c| match c {
                '\u{2018}' | '\u{2019}' | '`' | '\u{b4}' => '\'',
                c => c,
            })
            .flat_map(char::to_lowercase)
            .collect()
    };
    key(a) == key(b)
}

/// The speaker answers that name no one: a character by these names is the model echoing its options.
pub fn is_nobody(name: &str) -> bool {
    let name = name.trim();
    name.is_empty() || name.eq_ignore_ascii_case(UNKNOWN) || name.eq_ignore_ascii_case(NOT_SPEECH)
}

pub const SYSTEM: &str = "You attribute dialogue in a passage of a novel for a full-cast \
audiobook. Every quotation in the passage is preceded by a marker like [Q3]. For EVERY marker, \
name the character who speaks it, by a name the text actually uses for them: never add a \
surname or title the text has not given them, and know that many people go mostly by one name, \
often a surname. Reuse a name from the known characters list whenever it is the same person. \
An unnamed speaker who could be any of several people (a soldier, a guard, a voice in the \
crowd) gets a plain role label such as \"soldier\", reused for every such speaker in the \
passage; one unnamed person who keeps coming back (the urchin, the stablemaster) is a \
character, not a role label. Use \
\"unknown\" when the text does not let you tell, and \"not_speech\" for quoted text nobody \
says aloud (a sign, a title, a written note, scare quotes). `cue` is how the line sounds, as \
a direction to a voice actor in at most six words (\"whispered\", \"shouting, furious\", \
\"quietly, still shaken\"), or \"\" when plain; never a speech verb or what the line does \
(\"said\", \"asked\", \"explaining\").\n\n\
Then profile every character who speaks in this passage, as this passage shows them: the other \
names it uses for them, gender, apparent age, `role` (who they are and what they are doing \
here), `attitude` (temperament and mood here), `relationships` (who they are close to, \
opposed to or serve, and how), and `speech` (how they talk: accent, register, habits). Write \
\"\" for what the passage does not show. `generic` is true for the role-label speakers above.";

pub fn prompt(window: &Window, title: Option<&str>, known: &[String]) -> String {
    let known = if known.is_empty() {
        "none yet".to_string()
    } else {
        known.join(", ")
    };
    format!(
        "Chapter: {}\nKnown characters: {known}\n\nPassage:\n\n{}",
        title.unwrap_or("untitled"),
        window.text
    )
}

pub fn schema() -> Value {
    let s = json!({"type": "string"});
    json!({
        "type": "object",
        "additionalProperties": false,
        "required": ["lines", "characters"],
        "properties": {
            "lines": {"type": "array", "items": {
                "type": "object",
                "additionalProperties": false,
                "required": ["q", "speaker", "cue"],
                "properties": {"q": {"type": "integer"}, "speaker": s, "cue": s},
            }},
            "characters": {"type": "array", "items": {
                "type": "object",
                "additionalProperties": false,
                "required": ["name", "aliases", "gender", "age", "role", "attitude",
                             "relationships", "speech", "generic"],
                "properties": {
                    "name": s,
                    "aliases": {"type": "array", "items": s},
                    "gender": s,
                    "age": s,
                    "role": s,
                    "attitude": s,
                    "relationships": s,
                    "speech": s,
                    "generic": {"type": "boolean"},
                },
            }},
        },
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_cue_is_a_delivery_not_a_speech_verb() {
        let cases = [
            ("whispered", Some("whispered")),
            ("  shouting, furious ", Some("shouting, furious")),
            ("plain", None),
            ("Said", None),
            ("asked", None),
            ("continuing", None),
            ("explaining", None),
            ("", None),
            ("asked quietly", Some("asked quietly")),
        ];
        for (cue, want) in cases {
            assert_eq!(delivery(cue).as_deref(), want, "{cue:?}");
        }
    }

    fn segs(rows: &[(&str, Option<&str>)]) -> Vec<(i32, String, Option<String>)> {
        rows.iter()
            .enumerate()
            .map(|(i, (t, c))| (i as i32, t.to_string(), c.map(str::to_string)))
            .collect()
    }

    #[test]
    fn groups_runs_of_titles() {
        let chapters = group(segs(&[
            ("a", Some("One")),
            ("b", Some("One")),
            ("c", Some("Two")),
            ("d", None),
        ]));
        let shape: Vec<(i32, usize)> = chapters
            .iter()
            .map(|c| (c.index, c.paragraphs.len()))
            .collect();
        assert_eq!(shape, [(0, 2), (1, 1), (2, 1)]);
    }

    #[test]
    fn windows_number_quotes_and_split_long_chapters() {
        let chapter = &group(segs(&[
            ("“Hi,” said Ann. “Bye.”", Some("One")),
            ("Tom waved. “Later.”", Some("One")),
        ]))[0];
        let one = windows(chapter, QuoteStyle::Double, 10_000);
        assert_eq!(one.len(), 1);
        assert_eq!(
            one[0].text,
            "[Q0]“Hi,” said Ann. [Q1]“Bye.”\n\nTom waved. [Q2]“Later.”\n\n"
        );
        assert_eq!(one[0].quotes[2].segment_index, 1);

        let split = windows(chapter, QuoteStyle::Double, 30);
        assert_eq!(split.len(), 2);
        assert_eq!(split[1].text, "Tom waved. [Q0]“Later.”\n\n");
    }
}
