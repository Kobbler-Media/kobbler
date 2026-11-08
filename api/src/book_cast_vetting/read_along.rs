//! A read-along page read by the book's cast: each line its speaker's approved
//! voice with its cue, the narration the cast's narrator. A voice not approved
//! yet reads as the narrator, and a narrator not approved as the reader's own.

use std::collections::HashMap;
use std::sync::Arc;

use sqlx::PgPool;
use tracing::warn;
use uuid::Uuid;

use super::{speaking, voice_for};
use crate::book_cast::page::{self, Book};
use crate::book_cast::quotes;
use crate::repositories::book_cast_vetting::{self as vetting, CastVoice};
use crate::repositories::book_casts;
use crate::repositories::ebook_segments;
use crate::repositories::user_voice_presets::{self, UserVoicePreset};
use crate::routes::admin::tts::preset_clip_path;
use crate::tts::timeline::{Passage, Route, Voice};
use crate::tts::{self, TtsBackend};

pub struct Engine<'a> {
    pub storage_root: &'a str,
    pub service_url: &'a str,
    pub backend: TtsBackend,
}

/// The page's passages with the cast's voices, or `None` to read it with the
/// reader's own voice: no cast, nothing approved, or a page not found in the
/// book. Best-effort: a cast that fails to load costs the voices, not the page.
pub async fn passages(
    pool: &PgPool,
    engine: &Engine<'_>,
    book_id: Uuid,
    text: &str,
    reader: Voice,
    route: &Route,
) -> Option<Vec<Passage>> {
    match cast(pool, engine, book_id, text, reader, route).await {
        Ok(p) => p,
        Err(e) => {
            warn!(error = %e, book_id = %book_id, "read-along could not use the book's cast");
            None
        }
    }
}

async fn cast(
    pool: &PgPool,
    engine: &Engine<'_>,
    book_id: Uuid,
    text: &str,
    reader: Voice,
    route: &Route,
) -> anyhow::Result<Option<Vec<Passage>>> {
    let Some(cast) = book_casts::get(pool, book_id).await? else {
        return Ok(None);
    };
    let voices: Vec<CastVoice> = vetting::voices(pool, book_id)
        .await?
        .into_iter()
        .filter(|v| v.approved_at.is_some() && v.preset_id.is_some())
        .collect();
    if voices.is_empty() {
        return Ok(None);
    }
    let segments = ebook_segments::get_for_file(pool, cast.book_file_id).await?;
    let segments: Vec<(i32, &str)> = segments
        .iter()
        .map(|s| (s.segment_index, s.text.as_str()))
        .collect();
    let book = Book::new(&segments);
    let Some(placement) = page::locate(text, &book) else {
        return Ok(None);
    };
    let chapters = book_casts::chapters(pool, book_id).await?;
    let in_page = |c: &book_casts::CastChapter| {
        c.first_segment < placement.segments.end && c.last_segment >= placement.segments.start
    };
    let page_chapter = chapters
        .iter()
        .find(|c| in_page(c))
        .map_or(0, |c| c.chapter_index);
    let mut lines = Vec::new();
    for c in chapters.iter().filter(|c| in_page(c)) {
        lines.extend(book_casts::lines(pool, book_id, Some(c.chapter_index)).await?);
    }
    let parts = page::parts(text, &book, &placement, &lines);

    let characters = vetting::characters(pool, book_id).await?;
    let ids: Vec<Uuid> = voices.iter().filter_map(|v| v.preset_id).collect();
    let presets: HashMap<Uuid, UserVoicePreset> =
        user_voice_presets::presets_by_ids(pool, cast.owner_id, &ids)
            .await?
            .into_iter()
            .map(|p| (p.id, p))
            .collect();
    let preset_for = |character_id: Option<Uuid>, chapter: i32| {
        voice_for(&characters, &voices, character_id, chapter)
            .and_then(|v| v.preset_id)
            .and_then(|id| presets.get(&id))
    };

    let speed = reader.params.speed;
    let reader = Arc::new(reader);
    let mut cache: HashMap<(Uuid, Option<String>), Arc<Voice>> = HashMap::new();
    let mut voiced = |preset: &UserVoicePreset, cue: Option<&str>| {
        cache
            .entry((preset.id, cue.map(str::to_string)))
            .or_insert_with(|| {
                let mut v = speaking(preset, cue);
                v.params.speed = speed.or(v.params.speed);
                Arc::new(v)
            })
            .clone()
    };
    let mut used: Vec<&UserVoicePreset> = Vec::new();
    let mut out = Vec::new();
    for part in parts {
        let slice = &text[part.range];
        let (said, preset, cue) = match part.line.map(|i| &lines[i]) {
            Some(l) => {
                let bare = quotes::spoken(slice);
                let said = if tts::word_count(bare) == tts::word_count(slice) {
                    bare
                } else {
                    slice
                };
                let preset = preset_for(l.character_id, l.chapter_index)
                    .or_else(|| preset_for(None, l.chapter_index));
                (said, preset, l.cue.as_deref())
            }
            None => (slice, preset_for(None, page_chapter), None),
        };
        let voice = match preset {
            Some(p) => {
                if !used.iter().any(|u| u.id == p.id) {
                    used.push(p);
                }
                voiced(p, cue)
            }
            None => reader.clone(),
        };
        for text in route.split(said) {
            out.push(Passage {
                text,
                voice: voice.clone(),
            });
        }
    }

    let words: usize = out.iter().map(|p| tts::word_count(&p.text)).sum();
    if words != tts::word_count(text) {
        warn!(book_id = %book_id, words, page = tts::word_count(text), "a cast page lost words; reading it with one voice");
        return Ok(None);
    }
    register(engine, &used).await?;
    Ok(Some(out))
}

async fn register(engine: &Engine<'_>, presets: &[&UserVoicePreset]) -> anyhow::Result<()> {
    let clones: Vec<&&UserVoicePreset> = presets
        .iter()
        .filter(|p| p.clip_filename.is_some())
        .collect();
    if clones.is_empty() {
        return Ok(());
    }
    let known = tts::get_voices(engine.service_url).await;
    for p in clones {
        let name = format!("preset-{}", p.id);
        if known.iter().any(|v| v.id == name) {
            continue;
        }
        let wav = tokio::fs::read(preset_clip_path(engine.storage_root, p.user_id, p.id)).await?;
        tts::upload_voice(
            engine.service_url,
            engine.backend,
            &name,
            &wav,
            p.ref_text.as_deref(),
        )
        .await?;
    }
    Ok(())
}
