use std::collections::HashMap;

use serde::Deserialize;
use uuid::Uuid;

use super::error::{PRESET_GONE, VettingError};
use super::steps::{Step, candidates, step};
use super::{auditions, clips, designed, line_text, speaking};
use crate::AppState;
use crate::repositories::book_cast_vetting as vetting;
use crate::repositories::book_casts::{self, BookCast};
use crate::repositories::{ebook_segments, user_voice_presets};
use crate::routes::admin::tts::{
    ensure_preset_registered, preset_clip_path, register_preset_clip, render_sample,
};
use crate::tts::{TtsBackend, TtsParams};

#[derive(Debug, Deserialize)]
pub struct TakeRequest {
    #[serde(default)]
    pub from_chapter: i32,
    pub segment_index: i32,
    pub span_start: Option<i32>,
    pub seed: Option<i64>,
    #[serde(default)]
    pub with_cue: bool,
}

#[derive(Debug, Default, Deserialize)]
pub struct ApproveRequest {
    pub segment_index: Option<i32>,
    pub span_start: Option<i32>,
    pub seed: Option<i64>,
    #[serde(default)]
    pub with_cue: bool,
}

pub struct Rendering {
    pub text: String,
    pub voice: String,
    pub params: TtsParams,
}

/// How a take sounds: an approved clone keeps its own timbre and can carry only
/// a delivery cue as its instruction; a design is its description, with the cue
/// appended when asked for. Whether the engine honours an instruction on top of
/// a clone is exactly what auditioning both ways is for. A take being minted is
/// always read from the description, never from the clone it will replace. A
/// take without its cue reads on through the character's next lines, so the
/// take that is heard is the reference it is cloned from.
pub async fn rendering(
    state: &AppState,
    cast: &BookCast,
    step: &Step,
    take: &TakeRequest,
    minting: bool,
) -> Result<Rendering, VettingError> {
    let preset_id = step.voice.preset_id.ok_or(PRESET_GONE)?;
    let preset = user_voice_presets::get_preset(&state.pool, preset_id, cast.owner_id)
        .await?
        .ok_or(PRESET_GONE)?;
    let segment = ebook_segments::get_for_file(&state.pool, cast.book_file_id)
        .await?
        .into_iter()
        .find(|s| s.segment_index == take.segment_index)
        .ok_or(VettingError::NotFound("no such line"))?;
    let (text, cue) = match take.span_start {
        None => (segment.text.trim().to_string(), None),
        Some(span_start) => {
            let line = book_casts::lines(&state.pool, cast.book_id, None)
                .await?
                .into_iter()
                .find(|l| l.segment_index == take.segment_index && l.span_start == span_start)
                .ok_or(VettingError::NotFound("no such line"))?;
            let texts = HashMap::from([(segment.segment_index, segment.text)]);
            let text = line_text(&texts, &line)
                .ok_or(VettingError::NotFound("no such line"))?
                .to_string();
            (text, line.cue)
        }
    };
    let cue = cue.filter(|_| take.with_cue);
    let text = if take.with_cue {
        text
    } else {
        auditions::reference(
            &candidates(&state.pool, cast, step).await?,
            take.segment_index,
            take.span_start,
        )
        .unwrap_or(text)
    };
    if minting {
        let voice = designed(&preset, cue.as_deref());
        return Ok(Rendering {
            text,
            voice: voice.id,
            params: TtsParams {
                seed: take.seed,
                response_format: Some("wav".to_string()),
                speed: None,
                ..voice.params
            },
        });
    }
    if preset.clip_filename.is_some() {
        let config = state.runtime_config.get().await;
        let url = config
            .effective_tts_url()
            .filter(|u| !u.is_empty())
            .ok_or(VettingError::NoTts)?;
        let backend = TtsBackend::from_config(&config.tts_service_type);
        ensure_preset_registered(&state.config.storage_root, &url, backend, &preset).await?;
    }
    let voice = speaking(&preset, cue.as_deref());
    Ok(Rendering {
        text,
        voice: voice.id,
        params: TtsParams {
            seed: take.seed.or(preset.seed),
            response_format: Some("mp3".to_string()),
            ..voice.params
        },
    })
}

/// The take an approval without one takes: seed 0 on the first line an
/// audition would offer.
async fn default_take(
    state: &AppState,
    cast: &BookCast,
    step: &Step,
) -> Result<TakeRequest, VettingError> {
    let line = auditions::pick(candidates(&state.pool, cast, step).await?, auditions::LINES)
        .into_iter()
        .next()
        .ok_or(VettingError::Conflict("the voice has no line to take"))?;
    Ok(TakeRequest {
        from_chapter: step.voice.from_chapter,
        segment_index: line.segment_index,
        span_start: line.span_start,
        seed: Some(0),
        with_cue: false,
    })
}

/// Approves one voice. A designed voice is approved by rendering the chosen
/// take again (same seed, same text: the engine renders it identically) and
/// registering it as the preset's clone, as the read-along's narrator is
/// minted; approving another take replaces the clone. A preset of the owner's
/// own is approved as it stands. `None` is the default take.
pub async fn approve_step(
    state: &AppState,
    cast: &BookCast,
    character_id: Uuid,
    from_chapter: i32,
    req: Option<&ApproveRequest>,
) -> Result<(), VettingError> {
    let step = step(&state.pool, cast.book_id, character_id, from_chapter).await?;
    let preset_id = step.voice.preset_id.ok_or(PRESET_GONE)?;
    if step.voice.design_preset_id == Some(preset_id) {
        let take = match req {
            None => default_take(state, cast, &step).await?,
            Some(req) => {
                let (Some(segment_index), Some(seed)) = (req.segment_index, req.seed) else {
                    return Err(VettingError::BadRequest(
                        "approving a designed voice needs the take: segment_index and seed",
                    ));
                };
                TakeRequest {
                    from_chapter,
                    segment_index,
                    span_start: req.span_start,
                    seed: Some(seed),
                    with_cue: req.with_cue,
                }
            }
        };
        let r = rendering(state, cast, &step, &take, true).await?;
        let seed = take.seed.unwrap_or_default();
        let sample = render_sample(state, &r.text, Some(r.voice), r.params).await?;
        if sample.format != "wav" {
            return Err(VettingError::NotWav);
        }
        if step.voice.has_clip {
            clips::forget(&state.runtime_config, preset_id).await;
        }
        let path = preset_clip_path(&state.config.storage_root, cast.owner_id, preset_id);
        if let Some(dir) = path.parent() {
            tokio::fs::create_dir_all(dir).await?;
        }
        tokio::fs::write(&path, &sample.audio.bytes).await?;
        vetting::set_take(&state.pool, preset_id, &r.text, seed).await?;
        register_preset_clip(
            state,
            cast.owner_id,
            preset_id,
            &path,
            format!("{}.wav", step.character.name),
            Some(&r.text),
        )
        .await?;
    }
    vetting::set_approved(&state.pool, cast.book_id, preset_id, true).await?;
    Ok(())
}
