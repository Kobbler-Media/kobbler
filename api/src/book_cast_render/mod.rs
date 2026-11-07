//! Voicing a vetted cast: each chapter read in turn by the voices approved for
//! it, then muxed into one chaptered m4b and imported as the book's audiobook.
//! A chapter is read again only when what it was read with has changed.

pub mod casting;
pub mod chapter;
pub mod fingerprint;
pub mod m4b;

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use serde::Serialize;
use uuid::Uuid;

use crate::book_cast::quotes;
use crate::repositories::book_cast_render::RenderedChapter;
use crate::repositories::book_cast_vetting::{CastVoice, VettingCharacter};
use crate::repositories::book_casts::{CastChapter, CastLine};
use fingerprint::{Fingerprint, Stale};

pub fn work_dir(storage_root: &str, book_id: Uuid) -> PathBuf {
    PathBuf::from(storage_root)
        .join("book_casts")
        .join(book_id.to_string())
}

pub fn chapter_file(work: &Path, chapter_index: i32) -> PathBuf {
    work.join(format!("chapter-{chapter_index:04}.aac"))
}

#[derive(Debug, Clone, PartialEq)]
pub enum Rendered {
    Fresh,
    Missing,
    Stale(Stale),
}

pub struct ChapterStatus {
    pub chapter_index: i32,
    pub rendered: Rendered,
    pub fingerprint: Fingerprint,
}

/// Each chapter against what it was last rendered with. A row whose audio
/// file is gone (a restored backup) is missing, as is one never rendered.
pub fn status(
    chapters: &[CastChapter],
    lines: &[CastLine],
    cast: &fingerprint::Cast,
    rendered: &[RenderedChapter],
    work: &Path,
) -> Vec<ChapterStatus> {
    let mut by_chapter: HashMap<i32, Vec<&CastLine>> = HashMap::new();
    for l in lines {
        by_chapter.entry(l.chapter_index).or_default().push(l);
    }
    chapters
        .iter()
        .map(|ch| {
            let own = by_chapter.remove(&ch.chapter_index).unwrap_or_default();
            let now = fingerprint::chapter(cast, ch.chapter_index, &own);
            let was = rendered
                .iter()
                .find(|r| r.chapter_index == ch.chapter_index);
            let state = match was {
                Some(r) if r.duration_ms == 0 || chapter_file(work, r.chapter_index).exists() => {
                    match fingerprint::stale(
                        ch.chapter_index,
                        &r.fingerprint,
                        &now,
                        cast.characters,
                    ) {
                        Some(stale) => Rendered::Stale(stale),
                        None => Rendered::Fresh,
                    }
                }
                _ => Rendered::Missing,
            };
            ChapterStatus {
                chapter_index: ch.chapter_index,
                rendered: state,
                fingerprint: now,
            }
        })
        .collect()
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Unapproved {
    pub name: String,
    pub character_id: Option<Uuid>,
    pub from_chapter: Option<i32>,
    pub preset_id: Option<Uuid>,
    pub speakers: Vec<String>,
    /// A step that approves the voice, for a pool the first speaker's.
    #[serde(skip)]
    pub step: Option<(Uuid, i32)>,
}

/// Every voice the render would read with that is not approved yet: the
/// narrator's, each own-voiced character's arc steps, and each pooled preset
/// once, however many speakers share it. A part with no voice at all counts.
pub fn unapproved(characters: &[VettingCharacter], voices: &[CastVoice]) -> Vec<Unapproved> {
    let mut out: Vec<Unapproved> = Vec::new();
    let mut ordered: Vec<&VettingCharacter> = characters
        .iter()
        .filter(|c| c.narrator || !c.generic)
        .collect();
    ordered.sort_by_key(|c| !c.narrator);
    for c in ordered {
        let own: Vec<&CastVoice> = voices.iter().filter(|v| v.character_id == c.id).collect();
        if own.is_empty() {
            out.push(Unapproved {
                name: c.name.clone(),
                character_id: Some(c.id),
                from_chapter: None,
                preset_id: None,
                speakers: vec![c.name.clone()],
                step: None,
            });
        }
        for v in own
            .into_iter()
            .filter(|v| v.approved_at.is_none() || v.preset_id.is_none())
        {
            let pooled = c.pooled && !c.narrator;
            if pooled
                && let Some(entry) = out
                    .iter_mut()
                    .find(|u| u.character_id.is_none() && u.preset_id == v.preset_id)
            {
                entry.speakers.push(c.name.clone());
                continue;
            }
            out.push(Unapproved {
                name: if pooled {
                    v.preset_name.clone().unwrap_or_else(|| c.name.clone())
                } else {
                    c.name.clone()
                },
                character_id: (!pooled).then_some(c.id),
                from_chapter: (!pooled).then_some(v.from_chapter),
                preset_id: v.preset_id,
                speakers: vec![c.name.clone()],
                step: Some((c.id, v.from_chapter)),
            });
        }
    }
    out
}

#[derive(Debug, Clone, PartialEq)]
pub struct Piece {
    pub text: String,
    pub character_id: Option<Uuid>,
    pub cue: Option<String>,
}

/// A chapter in reading order, cut into narration and the quoted lines a
/// character speaks. A quote nobody has been given stays in the narration as
/// written, quote marks and all.
pub fn script<'a>(
    segments: impl IntoIterator<Item = (i32, &'a str)>,
    lines: &[CastLine],
) -> Vec<Piece> {
    let mut by_segment: HashMap<i32, Vec<&CastLine>> = HashMap::new();
    for l in lines.iter().filter(|l| l.character_id.is_some()) {
        by_segment.entry(l.segment_index).or_default().push(l);
    }
    let mut out = Vec::new();
    let mut narration = String::new();
    let flush = |narration: &mut String, out: &mut Vec<Piece>| {
        let text = narration.trim();
        if text.chars().any(char::is_alphanumeric) {
            out.push(Piece {
                text: text.to_string(),
                character_id: None,
                cue: None,
            });
        }
        narration.clear();
    };
    for (segment_index, text) in segments {
        let mut spans = by_segment.remove(&segment_index).unwrap_or_default();
        spans.sort_by_key(|l| l.span_start);
        let mut cursor = 0;
        for l in spans {
            let (start, end) = (l.span_start as usize, l.span_end as usize);
            let (Some(before), Some(span)) = (text.get(cursor..start), text.get(start..end)) else {
                continue;
            };
            let spoken = quotes::spoken(span).trim();
            if spoken.is_empty() {
                continue;
            }
            narration.push_str(before);
            flush(&mut narration, &mut out);
            out.push(Piece {
                text: spoken.to_string(),
                character_id: l.character_id,
                cue: l.cue.clone(),
            });
            cursor = end;
        }
        narration.push_str(text.get(cursor..).unwrap_or_default());
        narration.push_str("\n\n");
    }
    flush(&mut narration, &mut out);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn line(segment_index: i32, text: &str, quote: &str, who: Option<Uuid>) -> CastLine {
        let start = text.find(quote).unwrap() as i32;
        CastLine {
            segment_index,
            span_start: start,
            span_end: start + quote.len() as i32,
            chapter_index: 0,
            speaker: None,
            character_id: who,
            cue: who.map(|_| "softly".to_string()),
            source: "gemma".into(),
            not_speech: false,
        }
    }

    #[test]
    fn a_chapter_is_cut_into_narration_and_spoken_lines_in_reading_order() {
        let ann = Uuid::now_v7();
        let a = "“Come in,” said Ann. “Sit.”";
        let b = "The fire was low. “Who?” said a voice.";
        let c = "Night fell. The sign read “CLOSED”.";
        let lines = [
            line(0, a, "“Come in,”", Some(ann)),
            line(0, a, "“Sit.”", Some(ann)),
            line(1, b, "“Who?”", None),
            CastLine {
                not_speech: true,
                ..line(2, c, "“CLOSED”", None)
            },
        ];
        let pieces = script([(0, a), (1, b), (2, c)], &lines);
        let shape: Vec<(&str, Option<Uuid>)> = pieces
            .iter()
            .map(|p| (p.text.as_str(), p.character_id))
            .collect();
        assert_eq!(
            shape,
            [
                ("Come in,", Some(ann)),
                ("said Ann.", None),
                ("Sit.", Some(ann)),
                (
                    "The fire was low. “Who?” said a voice.\n\nNight fell. The sign read “CLOSED”.",
                    None
                ),
            ]
        );
        assert_eq!(pieces[0].cue.as_deref(), Some("softly"));
    }
}
