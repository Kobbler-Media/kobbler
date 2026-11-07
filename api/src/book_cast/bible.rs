use std::collections::HashMap;

use serde::Deserialize;
use serde_json::{Value, json};

use super::chapters::{self, CharacterNote, same_name};
use crate::repositories::book_casts::{CastChapter, CastLine, NewCharacter};

const RECHECK_MAX: usize = 400;
/// Per speaker, so a whole book's cast fits the resident model's context: two
/// hundred speakers at full chapter profiles came to ~80k tokens.
const SAMPLES: usize = 2;
const SAMPLE_CHARS: usize = 120;
const ALIASES_MAX: usize = 8;

#[derive(Debug, Deserialize)]
pub struct Reply {
    pub characters: Vec<Character>,
    pub lines: Vec<Recheck>,
}

#[derive(Debug, Deserialize)]
pub struct Character {
    pub name: String,
    pub aliases: Vec<String>,
    pub generic: bool,
}

#[derive(Debug, Deserialize)]
pub struct Recheck {
    pub id: usize,
    pub speaker: String,
}

pub const SYSTEM: &str = "You are casting a full-cast audiobook of a novel. A first pass read \
each chapter on its own and named the speaker of every line, so one person may appear under \
several names (\"Elizabeth\", \"Lizzy\", \"Miss Bennet\"). Each first-pass name is summed up \
below from the chapters it speaks in.\n\n\
1) Give one entry per real person. `name` is their fullest name using only words the book uses \
for them: never add a surname, family or title the book does not give them, and know that many \
people go mostly by one name, often a surname (\"Sadeas\"). `aliases` lists every other \
first-pass name that is the same person, spelled exactly as given. Leave out \"unknown\".\n\
2) A generic speaker is a role label that different unnamed people share (\"soldier\", \
\"guard\", \"a voice in the crowd\"): give one entry per kind with `generic` true; the narrator \
reads their lines. One unnamed person who keeps coming back (the urchin, the stablemaster) is a \
person, with `generic` false, as is everyone else.\n\
3) For each numbered line under \"Lines to recheck\", give the speaker using a name from your \
list, or \"unknown\" or \"not_speech\".";

pub struct Recheckable {
    pub segment_index: i32,
    pub span_start: i32,
}

struct Speaker<'a> {
    name: &'a str,
    notes: Vec<(i32, &'a CharacterNote)>,
}

/// The request, plus what each recheck id refers to.
pub fn prompt(
    chapters: &[CastChapter],
    lines: &[CastLine],
    paragraphs: &HashMap<i32, String>,
) -> (String, Vec<Recheckable>) {
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

    let mut out = String::from("First-pass speakers, each with their line count and chapters:\n");
    for s in &speakers {
        let count = lines
            .iter()
            .filter(|l| l.speaker.as_deref().is_some_and(|n| same_name(n, s.name)))
            .count();
        out.push_str(&summary(s, count));
    }

    let mut ids = Vec::new();
    let unknown = lines
        .iter()
        .filter(|l| l.source != "user" && l.speaker.is_none())
        .take(RECHECK_MAX);
    out.push_str("\nLines to recheck:\n");
    for line in unknown {
        let Some(text) = paragraphs.get(&line.segment_index) else {
            continue;
        };
        let Some(quote) = text.get(line.span_start as usize..line.span_end as usize) else {
            continue;
        };
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
}

fn summary(s: &Speaker, count: usize) -> String {
    let first = s.notes.iter().map(|(c, _)| c + 1).min().unwrap_or(1);
    let last = s.notes.iter().map(|(c, _)| c + 1).max().unwrap_or(1);
    let chapters = if first == last {
        format!("chapter {first}")
    } else {
        format!("chapters {first}–{last}")
    };
    let plural = if count == 1 { "" } else { "s" };
    let mut out = format!("\n### {} ({count} line{plural}; {chapters})\n", s.name);
    if s.notes.iter().all(|(_, n)| n.generic) {
        out.push_str("- generic\n");
    }

    let mut aliases: Vec<&str> = Vec::new();
    for a in s.notes.iter().flat_map(|(_, n)| &n.aliases) {
        let a = a.trim();
        if !a.is_empty() && !same_name(a, s.name) && !aliases.iter().any(|k| same_name(k, a)) {
            aliases.push(a);
        }
    }
    let fields: [(&str, Vec<&str>); 4] = [
        (
            "also called",
            aliases.into_iter().take(ALIASES_MAX).collect(),
        ),
        ("gender", distinct(s, |n| &n.gender, 1)),
        ("role", distinct(s, |n| &n.role, SAMPLES)),
        ("relationships", distinct(s, |n| &n.relationships, SAMPLES)),
    ];
    for (label, values) in fields {
        if !values.is_empty() {
            let values: Vec<String> = values.iter().map(|v| clip(v)).collect();
            let sep = if label == "also called" { ", " } else { " / " };
            out.push_str(&format!("- {label}: {}\n", values.join(sep)));
        }
    }
    out
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

pub fn schema() -> Value {
    let s = json!({"type": "string"});
    json!({
        "type": "object",
        "additionalProperties": false,
        "required": ["characters", "lines"],
        "properties": {
            "characters": {"type": "array", "items": {
                "type": "object",
                "additionalProperties": false,
                "required": ["name", "aliases", "generic"],
                "properties": {
                    "name": s,
                    "aliases": {"type": "array", "items": s},
                    "generic": {"type": "boolean"},
                },
            }},
            "lines": {"type": "array", "items": {
                "type": "object",
                "additionalProperties": false,
                "required": ["id", "speaker"],
                "properties": {"id": {"type": "integer"}, "speaker": s},
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
