//! What a chapter was read with, so a chapter can be told apart from one read
//! with what the cast says now: each voice reading in it, as the engine is
//! asked for it, and who says which line how.

use std::collections::{BTreeMap, HashMap};
use std::iter::once;

use serde::{Deserialize, Serialize};
use serde_json::json;
use sha2::{Digest, Sha256};
use uuid::Uuid;

use crate::book_cast_vetting::{speaking, voice_for};
use crate::repositories::book_cast_vetting::{CastVoice, VettingCharacter};
use crate::repositories::book_casts::CastLine;
use crate::repositories::user_voice_presets::UserVoicePreset;

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Fingerprint {
    pub lines: String,
    /// By the character whose voice reads.
    pub voices: BTreeMap<Uuid, String>,
}

pub struct Cast<'a> {
    pub characters: &'a [VettingCharacter],
    pub voices: &'a [CastVoice],
    pub presets: &'a HashMap<Uuid, UserVoicePreset>,
}

fn digest(value: &serde_json::Value) -> String {
    hex::encode(&Sha256::digest(value.to_string().as_bytes())[..8])
}

fn voice(step: &CastVoice, preset: Option<&UserVoicePreset>) -> String {
    let spoken = preset.map(|p| {
        let v = speaking(p, None);
        json!({
            "id": v.id,
            "speed": v.params.speed,
            "temperature": v.params.temperature,
            "top_p": v.params.top_p,
            "top_k": v.params.top_k,
            "repetition_penalty": v.params.repetition_penalty,
            "seed": v.params.seed,
            "instructions": v.params.instructions,
            "speaker": v.params.speaker,
            "max_new_frames": v.params.max_new_frames,
            "clip": p.clip_filename,
            "ref_text": p.ref_text,
        })
    });
    digest(&json!({
        "preset_id": step.preset_id,
        "from_chapter": step.from_chapter,
        "approved_at": step.approved_at.map(|t| t.unix_timestamp_nanos().to_string()),
        "preset": spoken,
    }))
}

pub fn chapter(cast: &Cast, chapter_index: i32, lines: &[&CastLine]) -> Fingerprint {
    let mut voices = BTreeMap::new();
    for who in once(None).chain(lines.iter().map(|l| l.character_id)) {
        if let Some(step) = voice_for(cast.characters, cast.voices, who, chapter_index) {
            voices.entry(step.character_id).or_insert_with(|| {
                voice(step, step.preset_id.and_then(|id| cast.presets.get(&id)))
            });
        }
    }
    let mut said: Vec<&&CastLine> = lines.iter().collect();
    said.sort_by_key(|l| (l.segment_index, l.span_start));
    let said: Vec<_> = said
        .iter()
        .map(|l| {
            json!([
                l.segment_index,
                l.span_start,
                l.span_end,
                l.character_id,
                l.cue
            ])
        })
        .collect();
    Fingerprint {
        lines: digest(&json!(said)),
        voices,
    }
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct StaleVoice {
    pub character_id: Uuid,
    pub name: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Stale {
    pub chapter_index: i32,
    pub attribution: bool,
    pub voices: Vec<StaleVoice>,
}

/// Why a chapter no longer sounds as the cast would read it now: a line given
/// to someone else, or a voice reading in it that changed. A voice that only
/// stopped reading is an attribution change unless the lines stayed the same.
pub fn stale(
    chapter_index: i32,
    was: &Fingerprint,
    now: &Fingerprint,
    characters: &[VettingCharacter],
) -> Option<Stale> {
    if was == now {
        return None;
    }
    let attribution = was.lines != now.lines;
    let mut changed: Vec<Uuid> = now
        .voices
        .iter()
        .filter(|(id, hash)| was.voices.get(*id) != Some(*hash))
        .map(|(id, _)| *id)
        .collect();
    if !attribution {
        changed.extend(was.voices.keys().filter(|id| !now.voices.contains_key(*id)));
    }
    Some(Stale {
        chapter_index,
        attribution,
        voices: changed
            .into_iter()
            .map(|character_id| StaleVoice {
                character_id,
                name: characters
                    .iter()
                    .find(|c| c.id == character_id)
                    .map(|c| c.name.clone()),
            })
            .collect(),
    })
}
