use std::collections::{HashMap, HashSet};

use sqlx::PgPool;
use uuid::Uuid;

use super::auditions::Take;
use super::error::VettingError;
use super::line_text;
use crate::repositories::book_cast_vetting::{self as vetting, CastVoice, VettingCharacter};
use crate::repositories::book_casts::{self, BookCast};
use crate::repositories::ebook_segments;

pub struct Step {
    pub character: VettingCharacter,
    pub voice: CastVoice,
    pub until: Option<i32>,
}

pub async fn step(
    pool: &PgPool,
    book_id: Uuid,
    character_id: Uuid,
    from_chapter: i32,
) -> Result<Step, VettingError> {
    let (character, voices) = tokio::try_join!(
        vetting::character(pool, book_id, character_id),
        vetting::voices(pool, book_id),
    )?;
    let character = character.ok_or(VettingError::NotFound("no such character"))?;
    let own: Vec<&CastVoice> = voices
        .iter()
        .filter(|v| v.character_id == character_id)
        .collect();
    let voice = own
        .iter()
        .find(|v| v.from_chapter == from_chapter)
        .map(|v| (*v).clone())
        .ok_or(VettingError::NotFound("the character has no such voice"))?;
    let until = own
        .iter()
        .map(|v| v.from_chapter)
        .filter(|&f| f > from_chapter)
        .min();
    Ok(Step {
        character,
        voice,
        until,
    })
}

pub async fn candidates(
    pool: &PgPool,
    cast: &BookCast,
    step: &Step,
) -> Result<Vec<Take>, VettingError> {
    let in_step =
        |chapter: i32| chapter >= step.voice.from_chapter && step.until.is_none_or(|u| chapter < u);
    let (segments, lines, chapters) = tokio::try_join!(
        ebook_segments::get_for_file(pool, cast.book_file_id),
        book_casts::lines(pool, cast.book_id, None),
        book_casts::chapters(pool, cast.book_id),
    )?;
    let takes: Vec<Take> = if step.character.narrator {
        let spoken: HashSet<i32> = lines.iter().map(|l| l.segment_index).collect();
        segments
            .iter()
            .filter(|s| !spoken.contains(&s.segment_index))
            .filter_map(|s| {
                let chapter = chapters
                    .iter()
                    .find(|c| (c.first_segment..=c.last_segment).contains(&s.segment_index))?;
                Some(Take {
                    segment_index: s.segment_index,
                    span_start: None,
                    chapter_index: chapter.chapter_index,
                    text: s.text.trim().to_string(),
                    cue: None,
                })
            })
            .collect()
    } else {
        let texts: HashMap<i32, String> = segments
            .into_iter()
            .map(|s| (s.segment_index, s.text))
            .collect();
        lines
            .iter()
            .filter(|l| l.character_id == Some(step.character.id))
            .filter_map(|l| {
                Some(Take {
                    segment_index: l.segment_index,
                    span_start: Some(l.span_start),
                    chapter_index: l.chapter_index,
                    text: line_text(&texts, l)?.to_string(),
                    cue: l.cue.clone(),
                })
            })
            .collect()
    };
    let (mine, rest): (Vec<_>, Vec<_>) = takes.into_iter().partition(|t| in_step(t.chapter_index));
    Ok(if mine.is_empty() { rest } else { mine })
}
