//! Asking the model who really says one character's lines. The chapter pass
//! files several people under one label ("woman", "spren"), and only learns a
//! speaker's name when the text gives it, often lines after they first speak.
//! The lines go in book order, each in the passage around it, so the model can
//! carry a name revealed late in a scene back to that speaker's earlier lines.
//! Its answers are suggestions for the admin to accept, never applied.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use uuid::Uuid;

use super::chapters::same_name;
use crate::repositories::book_cast_vetting::VettingCharacter;
use crate::repositories::book_casts::{CastChapter, CastLine};

/// Paragraphs either side of a line: enough for a scene's turn-taking and for
/// a name given a few exchanges later.
pub const AROUND: i32 = 4;
/// ~10k tokens of passages a request.
pub const BATCH_CHARS: usize = 40_000;
/// A main character's hundreds of lines are not what this is for.
pub const MOST_LINES: usize = 120;
pub const UNKNOWN: &str = "unknown";

pub const SYSTEM: &str = "You check who speaks some lines of dialogue in a novel, for a \
full-cast audiobook. An earlier pass filed all of these lines under one speaker, and it is \
often wrong: it files different people under one label (an unnamed \"woman\" in several \
scenes, \"spren\" for several spren), and it cannot know a speaker's name before the text \
gives it (\"the woman said\" three times, then \"her name was Navani\").\n\n\
The passages are in book order. [Ln] before a quote marks a line to answer for; {Name} before \
a quote says who the cast already has for that quote, to help you follow the turn-taking. \
For EVERY [Ln], give `speaker`: a name from the cast list when it is that person; a name the \
text itself gives this speaker when they are not in the cast (never invent one, never add a \
surname or title the text has not used); the line's current speaker when that already fits; \
or \"unknown\" when the text does not let you tell. One person speaking several lines in a \
scene gets the same answer for each: when the scene names them anywhere, before or after, use \
that name for all their lines. `reason` is one short sentence pointing at the words that \
decide it.";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct LineKey {
    pub segment_index: i32,
    pub span_start: i32,
}

impl LineKey {
    pub fn of(l: &CastLine) -> Self {
        Self {
            segment_index: l.segment_index,
            span_start: l.span_start,
        }
    }
}

/// Passages to send in one request; `lines` index the asked lines, each
/// marked `[L{i + 1}]` in `text`.
#[derive(Debug, Default)]
pub struct Batch {
    pub text: String,
    pub lines: Vec<usize>,
}

struct Window {
    chapter: Option<i32>,
    start: i32,
    end: i32,
    lines: Vec<usize>,
}

/// Each asked line in the paragraphs around it, kept inside its chapter;
/// windows that touch become one passage so a scene is read once, whole.
fn windows(asked: &[&CastLine], chapters: &[CastChapter]) -> Vec<Window> {
    let mut out: Vec<Window> = Vec::new();
    for (i, l) in asked.iter().enumerate() {
        let chapter = chapters
            .iter()
            .find(|c| (c.first_segment..=c.last_segment).contains(&l.segment_index));
        let (lo, hi) = chapter.map_or((l.segment_index, l.segment_index), |c| {
            (c.first_segment, c.last_segment)
        });
        let start = (l.segment_index - AROUND).max(lo);
        let end = (l.segment_index + AROUND).min(hi);
        let chapter = chapter.map(|c| c.chapter_index);
        match out.last_mut() {
            Some(w) if w.chapter == chapter && start <= w.end + 1 => {
                w.end = w.end.max(end);
                w.lines.push(i);
            }
            _ => out.push(Window {
                chapter,
                start,
                end,
                lines: vec![i],
            }),
        }
    }
    out
}

/// A paragraph with a marker before each quote: `[Ln]` for an asked line,
/// `{Name}` for one the cast already gives someone else.
fn marked(
    text: &str,
    quotes: &[&CastLine],
    asked: &HashMap<LineKey, usize>,
    names: &HashMap<Uuid, &str>,
) -> String {
    let mut out = String::new();
    let mut at = 0;
    for q in quotes {
        let start = q.span_start as usize;
        let marker = match asked.get(&LineKey::of(q)) {
            Some(i) => format!("[L{}]", i + 1),
            None => match q.character_id.and_then(|id| names.get(&id)) {
                Some(name) => format!("{{{name}}}"),
                None => continue,
            },
        };
        let Some(before) = text.get(at..start) else {
            continue;
        };
        out.push_str(before);
        out.push_str(&marker);
        at = start;
    }
    out.push_str(text.get(at..).unwrap_or_default());
    out
}

pub fn batches(
    asked: &[&CastLine],
    all: &[CastLine],
    texts: &HashMap<i32, String>,
    chapters: &[CastChapter],
    names: &HashMap<Uuid, &str>,
) -> Vec<Batch> {
    let index: HashMap<LineKey, usize> = asked
        .iter()
        .enumerate()
        .map(|(i, l)| (LineKey::of(l), i))
        .collect();
    let mut by_segment: HashMap<i32, Vec<&CastLine>> = HashMap::new();
    for l in all {
        by_segment.entry(l.segment_index).or_default().push(l);
    }
    for quotes in by_segment.values_mut() {
        quotes.sort_by_key(|l| l.span_start);
    }
    let mut out: Vec<Batch> = Vec::new();
    for w in windows(asked, chapters) {
        let title = w
            .chapter
            .and_then(|n| chapters.iter().find(|c| c.chapter_index == n))
            .map(|c| match &c.title {
                Some(t) => format!("Chapter {}: {t}", c.chapter_index + 1),
                None => format!("Chapter {}", c.chapter_index + 1),
            })
            .unwrap_or_else(|| "Passage".to_string());
        let mut passage = format!("## {title}\n\n");
        for s in w.start..=w.end {
            let Some(text) = texts.get(&s) else {
                continue;
            };
            let quotes = by_segment.get(&s).map(Vec::as_slice).unwrap_or_default();
            passage.push_str(&marked(text, quotes, &index, names));
            passage.push_str("\n\n");
        }
        match out.last_mut() {
            Some(b) if b.text.len() + passage.len() <= BATCH_CHARS => {
                b.text.push_str(&passage);
                b.lines.extend(w.lines);
            }
            _ => out.push(Batch {
                text: passage,
                lines: w.lines,
            }),
        }
    }
    out
}

pub fn prompt(who: &VettingCharacter, cast: &[VettingCharacter], batch: &Batch) -> String {
    let also = if who.aliases.is_empty() {
        String::new()
    } else {
        format!(" (also called {})", who.aliases.join(", "))
    };
    let list: Vec<String> = cast
        .iter()
        .filter(|c| !c.narrator)
        .map(|c| {
            if c.aliases.is_empty() {
                c.name.clone()
            } else {
                format!("{} ({})", c.name, c.aliases.join(", "))
            }
        })
        .collect();
    format!(
        "Lines filed under: {}{also}\nThe cast: {}\n\n{}",
        who.name,
        list.join("; "),
        batch.text.trim_end()
    )
}

pub fn schema() -> Value {
    let s = json!({"type": "string"});
    json!({
        "type": "object",
        "additionalProperties": false,
        "required": ["lines"],
        "properties": {
            "lines": {"type": "array", "items": {
                "type": "object",
                "additionalProperties": false,
                "required": ["l", "speaker", "reason"],
                "properties": {"l": {"type": "integer"}, "speaker": s, "reason": s},
            }},
        },
    })
}

pub fn max_tokens(lines: usize) -> u32 {
    256 + 96 * lines as u32
}

#[derive(Debug, Deserialize)]
pub struct Reply {
    pub lines: Vec<Answer>,
}

#[derive(Debug, Deserialize)]
pub struct Answer {
    pub l: usize,
    pub speaker: String,
    pub reason: String,
}

/// What the model says of one line. `character_id` is the cast member the
/// name is, or none for a name the cast does not have yet (or "unknown").
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Suggestion {
    pub segment_index: i32,
    pub span_start: i32,
    pub speaker: String,
    pub character_id: Option<Uuid>,
    pub reason: String,
}

/// An answer for a line outside the batch, or a second one for a line, is
/// dropped; a line the model skipped gets no suggestion.
pub fn suggestions(
    asked: &[&CastLine],
    batch: &Batch,
    reply: &Reply,
    cast: &[VettingCharacter],
) -> Vec<Suggestion> {
    let mut out: Vec<(usize, Suggestion)> = Vec::new();
    for a in &reply.lines {
        let Some(i) = a.l.checked_sub(1).filter(|i| batch.lines.contains(i)) else {
            continue;
        };
        if out.iter().any(|(j, _)| *j == i) {
            continue;
        }
        let name = a.speaker.trim();
        let unknown = name.is_empty() || name.eq_ignore_ascii_case(UNKNOWN);
        let character = cast.iter().filter(|c| !c.narrator).find(|c| {
            !unknown && (same_name(&c.name, name) || c.aliases.iter().any(|x| same_name(x, name)))
        });
        out.push((
            i,
            Suggestion {
                segment_index: asked[i].segment_index,
                span_start: asked[i].span_start,
                speaker: match character {
                    Some(c) => c.name.clone(),
                    None if unknown => UNKNOWN.to_string(),
                    None => name.to_string(),
                },
                character_id: character.map(|c| c.id),
                reason: a.reason.trim().to_string(),
            },
        ));
    }
    out.sort_by_key(|(i, _)| *i);
    out.into_iter().map(|(_, s)| s).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn line(segment_index: i32, span_start: i32, character_id: Option<Uuid>) -> CastLine {
        CastLine {
            segment_index,
            span_start,
            span_end: span_start + 4,
            chapter_index: 0,
            speaker: None,
            character_id,
            cue: None,
            source: "gemma".into(),
            not_speech: false,
        }
    }

    fn chapter(chapter_index: i32, first_segment: i32, last_segment: i32) -> CastChapter {
        CastChapter {
            chapter_index,
            title: None,
            first_segment,
            last_segment,
            notes: json!({}),
        }
    }

    fn character(name: &str, aliases: &[&str], narrator: bool) -> VettingCharacter {
        VettingCharacter {
            id: Uuid::now_v7(),
            name: name.into(),
            aliases: aliases.iter().map(|a| a.to_string()).collect(),
            bible: json!({}),
            line_count: 1,
            pooled: false,
            generic: false,
            narrator,
        }
    }

    #[test]
    fn lines_of_one_scene_share_a_passage_and_other_quotes_say_who_has_them() {
        let woman = Uuid::now_v7();
        let syl = Uuid::now_v7();
        let texts: HashMap<i32, String> = (0..40)
            .map(|i| (i, format!("“p{i}” said someone.")))
            .collect();
        let all = vec![
            line(2, 0, Some(woman)),
            line(3, 0, Some(syl)),
            line(5, 0, Some(woman)),
            line(30, 0, Some(woman)),
        ];
        let asked: Vec<&CastLine> = all
            .iter()
            .filter(|l| l.character_id == Some(woman))
            .collect();
        let names = HashMap::from([(syl, "Syl")]);
        let chapters = [chapter(0, 0, 19), chapter(1, 20, 39)];
        let b = batches(&asked, &all, &texts, &chapters, &names);
        assert_eq!(b.len(), 1, "small passages share a request");
        assert_eq!(b[0].lines, vec![0, 1, 2]);
        let text = &b[0].text;
        assert!(text.contains("[L1]“p2”"), "{text}");
        assert!(text.contains("{Syl}“p3”"), "{text}");
        assert!(text.contains("[L2]“p5”"), "{text}");
        assert!(text.contains("[L3]“p30”"), "{text}");
        assert_eq!(
            text.matches("## Chapter 1\n").count(),
            1,
            "overlapping windows are one passage: {text}"
        );
        assert!(
            text.contains("“p0”"),
            "the window reaches back to the chapter start"
        );
        assert!(text.contains("“p9”") && !text.contains("“p10”"));
        assert!(
            !text.contains("“p19”") && text.contains("“p26”"),
            "a window stays inside its chapter"
        );
    }

    #[test]
    fn answers_match_the_cast_by_any_of_its_names_and_a_new_name_stays_a_name() {
        let woman = Uuid::now_v7();
        let all = vec![
            line(1, 0, Some(woman)),
            line(2, 0, Some(woman)),
            line(3, 0, Some(woman)),
            line(4, 0, Some(woman)),
        ];
        let asked: Vec<&CastLine> = all.iter().collect();
        let navani = character("Navani Kholin", &["Navani"], false);
        let cast = [character("Narrator", &[], true), navani.clone()];
        let batch = Batch {
            text: String::new(),
            lines: vec![0, 1, 2],
        };
        let reply = Reply {
            lines: [
                (1, "navani"),
                (2, "Jasnah"),
                (3, "Unknown"),
                (3, "Navani"),
                (4, "Navani"),
                (9, "Navani"),
            ]
            .into_iter()
            .map(|(l, s)| Answer {
                l,
                speaker: s.into(),
                reason: " because ".into(),
            })
            .collect(),
        };
        let got = suggestions(&asked, &batch, &reply, &cast);
        let pairs: Vec<(i32, &str, Option<Uuid>)> = got
            .iter()
            .map(|s| (s.segment_index, s.speaker.as_str(), s.character_id))
            .collect();
        assert_eq!(
            pairs,
            vec![
                (1, "Navani Kholin", Some(navani.id)),
                (2, "Jasnah", None),
                (3, UNKNOWN, None),
            ],
            "an alias finds the character; a line outside the batch or answered twice is dropped"
        );
        assert_eq!(got[0].reason, "because");
    }
}
