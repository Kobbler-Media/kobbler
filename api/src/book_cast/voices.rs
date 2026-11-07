use serde::Deserialize;
use serde_json::{Value, json};

use super::bible::spread;
use super::chapters::{self, same_name};
use crate::repositories::book_cast_vetting::VettingCharacter;
use crate::repositories::book_casts::CastChapter;

/// Few enough that the model has room to describe each voice properly; at a
/// whole cast per reply it wrote what people say instead of how they sound.
pub const BATCH: usize = 6;
/// What a batch's profiles may come to, and one character's share of it, so a
/// request fits the resident model's context; a lead speaks in ~100 chapters.
const BATCH_CHARS: usize = 48_000;
const CHARACTER_CHARS: usize = 16_000;

#[derive(Debug, Deserialize)]
pub struct Reply {
    pub characters: Vec<Voice>,
}

#[derive(Debug, Deserialize)]
pub struct Voice {
    pub name: String,
    pub gender: String,
    pub age: String,
    pub personality: String,
    pub accent: String,
    pub voice: String,
}

pub const SYSTEM: &str = "You design voices for a full-cast audiobook. Each character comes \
with what every chapter they speak in showed of them; read those as one life. `gender` and \
`age` are as the book shows them overall, since a single chapter can guess wrong, and \
`personality` sums them up in one sentence. The narrator comes with the opening of the book \
instead: give them a voice that suits its tone and genre, one a listener could hear for hours.\n\n\
A text-to-speech model will build each voice from your description alone, so describe only \
how the voice SOUNDS: gender and age, pitch, timbre (warm, husky, nasal, breathy, gravelly, \
bright), pace and rhythm, accent, and their settled temperament. Never describe what they say: no \
vocabulary, slang, catchphrases, topics or speech habits. One to three plain sentences, and make \
each voice in the batch distinct from the others.\n\n\
`accent` is a real-world accent (\"Scottish\", \"Received Pronunciation\", \"West African\", \
\"neutral American\"), never the name of a people or place from the book. Map an invented people \
to the real accent that best matches how the book describes their speech, or say \"neutral\". \
Use the same accent in `voice`. Leave out passing mood and emotion: each line's delivery \
carries those.";

pub fn needs_design(c: &VettingCharacter) -> bool {
    !c.pooled && !c.generic && c.bible.get("voice").is_none()
}

/// The characters to design, a batch at a time, each batch with its request.
pub fn batches<'c>(
    todo: &[&'c VettingCharacter],
    chapters: &[CastChapter],
    opening: &str,
) -> Vec<(Vec<&'c VettingCharacter>, String)> {
    let parsed: Vec<_> = chapters
        .iter()
        .map(|c| (c.chapter_index, chapters::notes(c)))
        .collect();
    let mut out: Vec<(Vec<&VettingCharacter>, String)> = Vec::new();
    for c in todo {
        let mut seen: Vec<String> = Vec::new();
        for (index, notes) in &parsed {
            for n in notes.iter().filter(|n| is(c, &n.name)) {
                let entry = format!("- chapter {}: {}\n", index + 1, n.profile());
                if !n.profile().is_empty() && !seen.contains(&entry) {
                    seen.push(entry);
                }
            }
        }
        let total: usize = seen.iter().map(String::len).sum();
        if total > CHARACTER_CHARS {
            let keep = seen.len() * CHARACTER_CHARS / total;
            seen = spread(seen, keep.max(1));
        }
        let block = if c.narrator {
            format!(
                "## Voice design for {}\nThe opening of the book:\n{opening}\n",
                c.name
            )
        } else {
            format!("## Voice design for {}\n{}\n", c.name, seen.concat())
        };
        match out.last_mut() {
            Some((batch, prompt))
                if batch.len() < BATCH && prompt.len() + block.len() <= BATCH_CHARS =>
            {
                batch.push(c);
                prompt.push_str(&block);
            }
            _ => out.push((vec![c], block)),
        }
    }
    out
}

/// Each design to the character it is for: by name first, then the leftovers
/// in the order they were asked, since the model sometimes writes a name back
/// in a form that matches nothing ("Talenel’Elin" came back unmatched alone).
pub fn pair<'a, 'c>(
    batch: &[&'c VettingCharacter],
    reply: &'a [Voice],
) -> Vec<(&'c VettingCharacter, &'a Voice)> {
    let mut taken = vec![false; reply.len()];
    let mut paired: Vec<Option<&'a Voice>> = batch
        .iter()
        .map(|c| {
            let i = (0..reply.len()).find(|&i| !taken[i] && is(c, &reply[i].name))?;
            taken[i] = true;
            Some(&reply[i])
        })
        .collect();
    let mut leftovers = (0..reply.len()).filter(|&i| !taken[i]);
    for slot in paired.iter_mut().filter(|p| p.is_none()) {
        *slot = leftovers.next().map(|i| &reply[i]);
    }
    batch
        .iter()
        .zip(paired)
        .filter_map(|(c, v)| Some((*c, v?)))
        .collect()
}

fn is(c: &VettingCharacter, name: &str) -> bool {
    same_name(&c.name, name) || c.aliases.iter().any(|a| same_name(a, name))
}

pub fn design(v: &Voice) -> Value {
    json!({
        "gender": v.gender,
        "age": v.age,
        "personality": v.personality,
        "accent": v.accent,
        "voice": v.voice,
    })
}

pub fn schema() -> Value {
    let s = json!({"type": "string"});
    json!({
        "type": "object",
        "additionalProperties": false,
        "required": ["characters"],
        "properties": {
            "characters": {"type": "array", "items": {
                "type": "object",
                "additionalProperties": false,
                "required": ["name", "gender", "age", "personality", "accent", "voice"],
                "properties": {
                    "name": s,
                    "gender": s,
                    "age": s,
                    "personality": s,
                    "accent": s,
                    "voice": s,
                },
            }},
        },
    })
}
