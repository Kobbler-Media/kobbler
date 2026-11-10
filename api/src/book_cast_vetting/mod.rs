//! Vetting a full cast before it is voiced: every speaking part gets a voice
//! to audition and approve, and the attribution can be corrected by hand.

pub mod auditions;
pub mod clips;
pub mod error;
pub mod read_along;
pub mod seed;
pub mod steps;
pub mod takes;

use std::collections::HashMap;

use serde::Serialize;
use serde_json::Value;
use uuid::Uuid;

use crate::book_cast::quotes;
use crate::repositories::book_cast_vetting::{CastVoice, VettingCharacter};
use crate::repositories::book_casts::{CastChapter, CastLine};
use crate::repositories::user_voice_presets::UserVoicePreset;
use crate::tts::TtsParams;
use crate::tts::timeline::Voice;

pub const NARRATOR_NAME: &str = "Narrator";
pub const DEFAULT_NARRATOR_VOICE: &str = "A clear, warm audiobook narrator with a neutral accent, \
     reading at a steady, unhurried pace with gentle expression.";

/// The voice description a delivery cue rides on when a take is rendered
/// from the description rather than an approved clone.
pub fn with_cue(description: Option<&str>, cue: Option<&str>) -> Option<String> {
    let cue = cue.map(str::trim).filter(|c| !c.is_empty());
    match (description.map(str::trim).filter(|d| !d.is_empty()), cue) {
        (Some(d), Some(c)) => Some(format!("{d} Delivery: {c}.")),
        (Some(d), None) => Some(d.to_string()),
        (None, Some(c)) => Some(format!("Delivery: {c}.")),
        (None, None) => None,
    }
}

/// How a preset reads a line. An approved clone keeps its own timbre and can
/// carry only a delivery cue as its instruction; a design is its description,
/// the cue riding on it. A clone must be registered with the engine first.
pub fn speaking(preset: &UserVoicePreset, cue: Option<&str>) -> Voice {
    if preset.clip_filename.is_some() {
        voice(preset, format!("preset-{}", preset.id), with_cue(None, cue))
    } else {
        designed(preset, cue)
    }
}

/// A preset read from its description even when it has a clone, as a new take
/// of it is minted.
pub fn designed(preset: &UserVoicePreset, cue: Option<&str>) -> Voice {
    voice(
        preset,
        preset.voice.clone().unwrap_or_default(),
        with_cue(preset.instructions.as_deref(), cue),
    )
}

/// Breeze ignores an instruction at guidance 1 and follows it at 4, the
/// model's own recommendation, for ~30% more render time.
const INSTRUCTION_CFG: f64 = 4.0;

fn voice(preset: &UserVoicePreset, id: String, instructions: Option<String>) -> Voice {
    let cfg_scale = instructions.is_some().then_some(INSTRUCTION_CFG);
    Voice {
        id,
        params: TtsParams {
            speed: Some(preset.speed),
            temperature: preset.temperature,
            top_p: preset.top_p,
            top_k: preset.top_k.map(|k| k as u32),
            repetition_penalty: preset.repetition_penalty,
            seed: preset.seed,
            instructions,
            speaker: preset.speaker.clone(),
            max_new_frames: preset.max_new_frames.map(|f| f as u32),
            cfg_scale,
            response_format: None,
        },
    }
}

/// Who reads a line: its own character, or the narrator for narration and for
/// generic speakers. Each character's voice is the step its chapter has reached.
pub fn voice_for<'a>(
    characters: &[VettingCharacter],
    voices: &'a [CastVoice],
    character_id: Option<Uuid>,
    chapter_index: i32,
) -> Option<&'a CastVoice> {
    let speaker = character_id
        .and_then(|id| characters.iter().find(|c| c.id == id))
        .filter(|c| !c.generic)
        .or_else(|| characters.iter().find(|c| c.narrator))?;
    voices
        .iter()
        .filter(|v| v.character_id == speaker.id && v.from_chapter <= chapter_index)
        .max_by_key(|v| v.from_chapter)
}

pub fn arc_steps(bible: &Value) -> Vec<(i32, String)> {
    bible["arc"]
        .as_array()
        .map(|steps| {
            steps
                .iter()
                .filter_map(|s| {
                    let from = i32::try_from(s["from_chapter"].as_i64()?).ok()?;
                    (from > 0).then(|| (from, s["voice"].as_str().unwrap_or("").to_string()))
                })
                .collect()
        })
        .unwrap_or_default()
}

pub fn line_text<'a>(texts: &'a HashMap<i32, String>, line: &CastLine) -> Option<&'a str> {
    texts
        .get(&line.segment_index)?
        .get(line.span_start as usize..line.span_end as usize)
        .map(quotes::spoken)
}

/// A line as it reads in the book: its paragraph split around the quote, and
/// the paragraphs either side of it within its chapter.
#[derive(Debug, Serialize)]
pub struct LineContext<'a> {
    pub before: Option<&'a str>,
    pub pre: &'a str,
    pub quote: &'a str,
    pub post: &'a str,
    pub after: Option<&'a str>,
}

pub fn line_context<'a>(
    texts: &'a HashMap<i32, String>,
    chapter: Option<&CastChapter>,
    line: &CastLine,
) -> Option<LineContext<'a>> {
    let paragraph = texts.get(&line.segment_index)?;
    let (start, end) = (line.span_start as usize, line.span_end as usize);
    let neighbour = |index: i32| {
        chapter
            .filter(|c| (c.first_segment..=c.last_segment).contains(&index))
            .and_then(|_| texts.get(&index))
            .map(String::as_str)
    };
    Some(LineContext {
        before: neighbour(line.segment_index - 1),
        pre: paragraph.get(..start)?,
        quote: paragraph.get(start..end)?,
        post: paragraph.get(end..)?,
        after: neighbour(line.segment_index + 1),
    })
}
