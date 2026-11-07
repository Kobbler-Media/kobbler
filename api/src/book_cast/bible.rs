use std::collections::HashMap;

use serde::Deserialize;
use serde_json::{Value, json};

use super::chapters::{self, CharacterNote, same_name};
use crate::repositories::book_casts::{CastChapter, CastLine, NewCharacter};

const RECHECK_MAX: usize = 400;
const RECHECK_BATCH: usize = 40;
/// One line per first-pass name, so a whole book's names fit one small
/// request; the per-character detail is the voice pass's.
const SAMPLE_CHARS: usize = 100;
const ALIASES_MAX: usize = 6;
pub const GROUP_MAX_TOKENS: u32 = 6_144;
pub const RECHECK_MAX_TOKENS: u32 = 2_048;

#[derive(Debug, Deserialize)]
pub struct Groups {
    pub people: Vec<Person>,
}

#[derive(Debug, Deserialize)]
pub struct Person {
    pub name: String,
    pub names: Vec<String>,
    pub generic: bool,
}

#[derive(Debug, Deserialize)]
pub struct Rechecks {
    pub lines: Vec<Recheck>,
}

#[derive(Debug, Deserialize)]
pub struct Recheck {
    pub id: usize,
    pub speaker: String,
}

pub const GROUP_SYSTEM: &str = "You are casting a full-cast audiobook of a novel. A first pass \
read each chapter on its own and named the speaker of every line, so one person may appear \
under several names (\"Elizabeth\", \"Lizzy\", \"Miss Bennet\"). Below is every first-pass name \
with what its chapters showed.\n\n\
List only the groups:\n\
1) Each real person known by more than one of these names. `name` is their fullest name using \
only words the book uses for them: never add a surname, family or title the book does not give \
them, and know that many people go mostly by one name, often a surname (\"Sadeas\"). `names` \
lists their other first-pass names, spelled exactly as given.\n\
2) Each generic kind: a role label that different unnamed people share (\"soldier\", \"guard\", \
\"a voice in the crowd\"), with `generic` true and `names` the first-pass labels of that kind; \
the narrator reads their lines. One unnamed person who keeps coming back (the urchin, the \
stablemaster) is a person, not generic.\n\
Leave out everyone known by a single name, and \"unknown\": they are cast as they are.";

pub const RECHECK_SYSTEM: &str = "You are casting a full-cast audiobook of a novel. For each \
numbered line below, which a first pass could not attribute, give its speaker using a name \
from Names, or \"unknown\" when the passage does not tell, or \"not_speech\" for quoted text \
nobody says aloud.";

pub struct Recheckable {
    pub segment_index: i32,
    pub span_start: i32,
}

struct Speaker<'a> {
    name: &'a str,
    notes: Vec<(i32, &'a CharacterNote)>,
}

pub fn group_prompt(chapters: &[CastChapter], lines: &[CastLine]) -> String {
    let parsed: Vec<(i32, Vec<CharacterNote>)> = chapters
        .iter()
        .map(|c| (c.chapter_index, chapters::notes(c)))
        .collect();
    let mut speakers: Vec<Speaker> = Vec::new();
    for (chapter_index, notes) in &parsed {
        for n in notes {
            let note = (*chapter_index, n);
            match speakers.iter_mut().find(|s| same_name(s.name, &n.name)) {
                Some(s) => s.notes.push(note),
                None => speakers.push(Speaker {
                    name: &n.name,
                    notes: vec![note],
                }),
            }
        }
    }
    let mut out =
        String::from("First-pass names, with their lines, chapters and what they showed:\n");
    for s in &speakers {
        let count = lines
            .iter()
            .filter(|l| l.speaker.as_deref().is_some_and(|n| same_name(n, s.name)))
            .count();
        out.push_str(&summary(s, count));
    }
    out
}

/// Each request with what its recheck ids refer to.
pub fn recheck_batches(
    lines: &[CastLine],
    paragraphs: &HashMap<i32, String>,
    names: &[String],
) -> Vec<(String, Vec<Recheckable>)> {
    let unknown: Vec<(&CastLine, &str, &str)> = lines
        .iter()
        .filter(|l| l.needs_speaker() && l.speaker.is_none())
        .filter_map(|l| {
            let text = paragraphs.get(&l.segment_index)?;
            let quote = text.get(l.span_start as usize..l.span_end as usize)?;
            Some((l, quote, text.as_str()))
        })
        .take(RECHECK_MAX)
        .collect();
    unknown
        .chunks(RECHECK_BATCH)
        .map(|batch| {
            let mut out = format!("Names: {}\n\nLines to recheck:\n", names.join(", "));
            let mut ids = Vec::new();
            for (line, quote, text) in batch {
                out.push_str(&format!(
                    "\n[{}] chapter {}: {quote}\n  in: {text}\n",
                    ids.len(),
                    line.chapter_index + 1,
                ));
                ids.push(Recheckable {
                    segment_index: line.segment_index,
                    span_start: line.span_start,
                });
            }
            (out, ids)
        })
        .collect()
}

fn summary(s: &Speaker, count: usize) -> String {
    let first = s.notes.iter().map(|(c, _)| c + 1).min().unwrap_or(1);
    let last = s.notes.iter().map(|(c, _)| c + 1).max().unwrap_or(1);
    let plural = if count == 1 { "" } else { "s" };
    let mut parts = vec![
        format!("{count} line{plural}"),
        if first == last {
            format!("chapter {first}")
        } else {
            format!("chapters {first}–{last}")
        },
    ];
    if s.notes.iter().all(|(_, n)| n.generic) {
        parts.push("generic".into());
    }
    parts.extend(
        distinct(s, |n| &n.gender, 1)
            .into_iter()
            .map(str::to_string),
    );
    let mut aliases: Vec<&str> = Vec::new();
    for a in s.notes.iter().flat_map(|(_, n)| &n.aliases) {
        let a = a.trim();
        if !a.is_empty() && !same_name(a, s.name) && !aliases.iter().any(|k| same_name(k, a)) {
            aliases.push(a);
        }
    }
    if !aliases.is_empty() {
        let shown: Vec<&str> = aliases.into_iter().take(ALIASES_MAX).collect();
        parts.push(format!("also: {}", shown.join(", ")));
    }
    for (label, field) in [
        (
            "role",
            (|n: &CharacterNote| &n.role) as fn(&CharacterNote) -> &String,
        ),
        ("with", |n: &CharacterNote| &n.relationships),
    ] {
        if let Some(v) = distinct(s, field, 1).first() {
            parts.push(format!("{label}: {}", clip(v)));
        }
    }
    format!("- {} ({})\n", s.name, parts.join("; "))
}

/// Up to `n` different values, spread across the book rather than its opening.
fn distinct<'a>(s: &Speaker<'a>, field: fn(&CharacterNote) -> &String, n: usize) -> Vec<&'a str> {
    let mut seen: Vec<&'a str> = Vec::new();
    for (_, note) in &s.notes {
        let v = field(note).trim();
        if !v.is_empty() && !seen.iter().any(|k| k.eq_ignore_ascii_case(v)) {
            seen.push(v);
        }
    }
    spread(seen, n)
}

pub fn spread<T>(items: Vec<T>, n: usize) -> Vec<T> {
    let len = items.len();
    if len <= n {
        return items;
    }
    let picks: Vec<usize> = (0..n).map(|i| i * (len - 1) / (n - 1).max(1)).collect();
    items
        .into_iter()
        .enumerate()
        .filter(|(i, _)| picks.contains(i))
        .map(|(_, v)| v)
        .collect()
}

fn clip(s: &str) -> String {
    match s.char_indices().nth(SAMPLE_CHARS) {
        Some((i, _)) => format!("{}…", &s[..i]),
        None => s.to_string(),
    }
}

/// The gender and age a character's chapters gave them most often: all a
/// pooled voice is chosen by, since the voice pass designs only the others.
pub fn traits(c: &NewCharacter, notes: &[CharacterNote]) -> Value {
    let own: Vec<&CharacterNote> = notes
        .iter()
        .filter(|n| same_name(&n.name, &c.name) || c.aliases.iter().any(|a| same_name(a, &n.name)))
        .collect();
    let mut out = serde_json::Map::new();
    for (field, value) in [
        ("gender", usual(own.iter().map(|n| n.gender.as_str()))),
        ("age", usual(own.iter().map(|n| n.age.as_str()))),
    ] {
        if let Some(v) = value {
            out.insert(field.to_string(), json!(v));
        }
    }
    Value::Object(out)
}

fn usual<'a>(values: impl Iterator<Item = &'a str>) -> Option<&'a str> {
    let mut counts: Vec<(&str, usize)> = Vec::new();
    for v in values.map(str::trim).filter(|v| !v.is_empty()) {
        match counts.iter_mut().find(|(k, _)| k.eq_ignore_ascii_case(v)) {
            Some((_, n)) => *n += 1,
            None => counts.push((v, 1)),
        }
    }
    counts
        .into_iter()
        .rev()
        .max_by_key(|(_, n)| *n)
        .map(|(v, _)| v)
}

pub fn group_schema() -> Value {
    let s = json!({"type": "string"});
    json!({
        "type": "object",
        "additionalProperties": false,
        "required": ["people"],
        "properties": {
            "people": {"type": "array", "items": {
                "type": "object",
                "additionalProperties": false,
                "required": ["name", "names", "generic"],
                "properties": {
                    "name": s,
                    "names": {"type": "array", "items": s},
                    "generic": {"type": "boolean"},
                },
            }},
        },
    })
}

pub fn recheck_schema() -> Value {
    json!({
        "type": "object",
        "additionalProperties": false,
        "required": ["lines"],
        "properties": {
            "lines": {"type": "array", "items": {
                "type": "object",
                "additionalProperties": false,
                "required": ["id", "speaker"],
                "properties": {"id": {"type": "integer"}, "speaker": {"type": "string"}},
            }},
        },
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spread_keeps_the_ends_and_spaces_the_rest() {
        let cases: &[(usize, usize, &[usize])] = &[
            (3, 5, &[0, 1, 2]),
            (5, 2, &[0, 4]),
            (9, 3, &[0, 4, 8]),
            (4, 1, &[0]),
        ];
        for (len, n, want) in cases {
            assert_eq!(spread((0..*len).collect(), *n), *want, "{len} into {n}");
        }
    }
}
