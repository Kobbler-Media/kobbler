//! A cast's voices: seeded from the voice designs, auditioned on each
//! character's own lines, then approved as engine clones.

use axum::Json;
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use serde::Deserialize;
use serde_json::json;
use uuid::Uuid;

use super::book_cast::{fail, internal, vetting_cast};
use super::tts::render_sample;
use crate::AppState;
use crate::book_cast_vetting::error::{PRESET_GONE, VettingError};
use crate::book_cast_vetting::steps::{self, Step};
use crate::book_cast_vetting::takes::{self, ApproveRequest, TakeRequest};
use crate::book_cast_vetting::{self, auditions, clips, seed};
use crate::jobs::handlers::APPROVE_JOB;
use crate::repositories::book_cast_vetting::{self as vetting, VettingCharacter};
use crate::repositories::book_casts::BookCast;
use crate::repositories::job_status;
use crate::routes::AuthUser;

pub(super) async fn reseed(state: &AppState, cast: &BookCast) -> Result<(), Response> {
    seed::seed(
        &state.pool,
        &state.runtime_config,
        &state.config.storage_root,
        cast.book_id,
        cast.owner_id,
    )
    .await
    .map_err(internal)
}

/// A design's approval and the clone that came with it, gone together.
async fn unapprove_design(
    state: &AppState,
    cast: &BookCast,
    preset_id: Uuid,
) -> Result<(), Response> {
    vetting::set_approved(&state.pool, cast.book_id, preset_id, false)
        .await
        .map_err(internal)?;
    vetting::clear_take(&state.pool, preset_id)
        .await
        .map_err(internal)?;
    clips::discard(
        &state.config.storage_root,
        &state.runtime_config,
        cast.owner_id,
        preset_id,
    )
    .await;
    Ok(())
}

impl IntoResponse for VettingError {
    fn into_response(self) -> Response {
        let status = match &self {
            VettingError::NotFound(_) => StatusCode::NOT_FOUND,
            VettingError::Conflict(_) => StatusCode::CONFLICT,
            VettingError::BadRequest(_) => StatusCode::BAD_REQUEST,
            VettingError::NoTts => StatusCode::SERVICE_UNAVAILABLE,
            VettingError::NotWav => StatusCode::BAD_GATEWAY,
            VettingError::Sample(_) => return self.into_sample_response(),
            VettingError::Clip(_) => return self.into_clip_response(),
            VettingError::Register(_) | VettingError::Repository(_) | VettingError::Io(_) => {
                return internal(self);
            }
        };
        fail(status, &self.to_string())
    }
}

impl VettingError {
    fn into_sample_response(self) -> Response {
        match self {
            VettingError::Sample(e) => e.into_response(),
            other => internal(other),
        }
    }

    fn into_clip_response(self) -> Response {
        match self {
            VettingError::Clip(e) => e.into_response(),
            other => internal(other),
        }
    }
}

async fn step_of(
    state: &AppState,
    book_id: Uuid,
    character_id: Uuid,
    from_chapter: i32,
) -> Result<Step, Response> {
    steps::step(&state.pool, book_id, character_id, from_chapter)
        .await
        .map_err(IntoResponse::into_response)
}

#[derive(Debug, Default, Deserialize)]
pub struct StepQuery {
    #[serde(default)]
    pub from_chapter: i32,
}

/// GET /api/v1/admin/books/{book_id}/cast/characters/{character_id}/auditions?from_chapter=
pub async fn audition_lines(
    State(state): State<AppState>,
    auth: AuthUser,
    Path((book_id, character_id)): Path<(Uuid, Uuid)>,
    Query(q): Query<StepQuery>,
) -> Response {
    let cast = match vetting_cast(&state, &auth, book_id).await {
        Ok(c) => c,
        Err(r) => return r,
    };
    let step = match step_of(&state, book_id, character_id, q.from_chapter).await {
        Ok(s) => s,
        Err(r) => return r,
    };
    match steps::candidates(&state.pool, &cast, &step).await {
        Ok(takes) => Json(json!({
            "voice": step.voice,
            "lines": auditions::pick(takes, auditions::LINES),
        }))
        .into_response(),
        Err(e) => e.into_response(),
    }
}

/// POST /api/v1/admin/books/{book_id}/cast/characters/{character_id}/auditions
pub async fn audition(
    State(state): State<AppState>,
    auth: AuthUser,
    Path((book_id, character_id)): Path<(Uuid, Uuid)>,
    Json(take): Json<TakeRequest>,
) -> Response {
    let cast = match vetting_cast(&state, &auth, book_id).await {
        Ok(c) => c,
        Err(r) => return r,
    };
    let rendered = async {
        let step = steps::step(&state.pool, book_id, character_id, take.from_chapter).await?;
        let r = takes::rendering(&state, &cast, &step, &take, false).await?;
        Ok::<_, VettingError>(render_sample(&state, &r.text, Some(r.voice), r.params).await?)
    }
    .await;
    match rendered {
        Ok(sample) => sample.into_response(),
        Err(e) => e.into_response(),
    }
}

/// POST /api/v1/admin/books/{book_id}/cast/characters/{character_id}/voices/{from_chapter}/approval
pub async fn approve(
    State(state): State<AppState>,
    auth: AuthUser,
    Path((book_id, character_id, from_chapter)): Path<(Uuid, Uuid, i32)>,
    Json(req): Json<ApproveRequest>,
) -> Response {
    let cast = match vetting_cast(&state, &auth, book_id).await {
        Ok(c) => c,
        Err(r) => return r,
    };
    match takes::approve_step(&state, &cast, character_id, from_chapter, Some(&req)).await {
        Ok(()) => voices_of(&state, book_id, character_id).await,
        Err(e) => e.into_response(),
    }
}

/// DELETE /api/v1/admin/books/{book_id}/cast/characters/{character_id}/voices/{from_chapter}/approval
pub async fn unapprove(
    State(state): State<AppState>,
    auth: AuthUser,
    Path((book_id, character_id, from_chapter)): Path<(Uuid, Uuid, i32)>,
) -> Response {
    let cast = match vetting_cast(&state, &auth, book_id).await {
        Ok(c) => c,
        Err(r) => return r,
    };
    let step = match step_of(&state, book_id, character_id, from_chapter).await {
        Ok(s) => s,
        Err(r) => return r,
    };
    let Some(preset_id) = step.voice.preset_id else {
        return PRESET_GONE.into_response();
    };
    let undone = if step.voice.design_preset_id == Some(preset_id) {
        unapprove_design(&state, &cast, preset_id).await
    } else {
        vetting::set_approved(&state.pool, book_id, preset_id, false)
            .await
            .map_err(internal)
    };
    match undone {
        Ok(()) => voices_of(&state, book_id, character_id).await,
        Err(r) => r,
    }
}

/// POST /api/v1/admin/books/{book_id}/cast/voices/approve-all
///
/// Approves every voice still unapproved with its default take, as a job: a
/// big cast is a hundred and more takes to mint.
pub async fn approve_all(
    State(state): State<AppState>,
    auth: AuthUser,
    Path(book_id): Path<Uuid>,
) -> Response {
    if let Err(r) = vetting_cast(&state, &auth, book_id).await {
        return r;
    }
    match job_status::latest_active_for_entity(&state.pool, APPROVE_JOB, book_id).await {
        Ok(Some(_)) => return fail(StatusCode::CONFLICT, "the cast's voices are being approved"),
        Ok(None) => {}
        Err(e) => return internal(e),
    }
    match crate::jobs::enqueue_book_cast(&state.pool, state.nats.publisher(), APPROVE_JOB, book_id)
        .await
    {
        Ok(job_id) => (
            StatusCode::ACCEPTED,
            Json(json!({"job_id": job_id, "job_type": APPROVE_JOB})),
        )
            .into_response(),
        Err(e) => internal(e),
    }
}

async fn voices_of(state: &AppState, book_id: Uuid, character_id: Uuid) -> Response {
    match vetting::voices(&state.pool, book_id).await {
        Ok(voices) => Json(json!({
            "voices": voices.into_iter().filter(|v| v.character_id == character_id).collect::<Vec<_>>()
        }))
        .into_response(),
        Err(e) => internal(e),
    }
}

#[derive(Debug, Default, Deserialize)]
pub struct VoiceChange {
    pub description: Option<String>,
    #[serde(default, deserialize_with = "crate::serde_patch::double_option")]
    pub preset_id: Option<Option<Uuid>>,
}

/// PUT /api/v1/admin/books/{book_id}/cast/characters/{character_id}/voices/{from_chapter}
///
/// Edits the designed voice's description, or points the step at one of the
/// owner's own presets (`preset_id`; null goes back to the design).
pub async fn update_voice(
    State(state): State<AppState>,
    auth: AuthUser,
    Path((book_id, character_id, from_chapter)): Path<(Uuid, Uuid, i32)>,
    Json(req): Json<VoiceChange>,
) -> Response {
    let cast = match vetting_cast(&state, &auth, book_id).await {
        Ok(c) => c,
        Err(r) => return r,
    };
    let step = match step_of(&state, book_id, character_id, from_chapter).await {
        Ok(s) => s,
        Err(r) => return r,
    };
    if let Some(description) = &req.description {
        let Some(design) = step.voice.design_preset_id else {
            return fail(StatusCode::CONFLICT, "the voice has no design to describe");
        };
        if description.trim().is_empty() {
            return fail(StatusCode::BAD_REQUEST, "the description is empty");
        }
        if step.voice.preset_id == Some(design)
            && (step.voice.approved_at.is_some() || step.voice.has_clip)
            && let Err(r) = unapprove_design(&state, &cast, design).await
        {
            return r;
        }
        if let Err(e) = vetting::set_description(&state.pool, design, description.trim()).await {
            return internal(e);
        }
    }
    if let Some(choice) = req.preset_id {
        if let Some(id) = choice {
            match vetting::preset_choices(&state.pool, cast.owner_id).await {
                Ok(presets) if presets.iter().any(|p| p.id == id) => {}
                Ok(_) => {
                    return fail(
                        StatusCode::BAD_REQUEST,
                        "not one of the cast owner's presets",
                    );
                }
                Err(e) => return internal(e),
            }
        }
        if let Err(e) = vetting::use_preset(&state.pool, character_id, from_chapter, choice).await {
            return internal(e);
        }
        if let Some(left) = step.voice.preset_id.filter(|&p| {
            step.voice.has_clip && step.voice.design_preset_id == Some(p) && choice != Some(p)
        }) {
            match vetting::preset_in_use(&state.pool, left).await {
                Ok(true) => {}
                Ok(false) => {
                    if let Err(r) = unapprove_design(&state, &cast, left).await {
                        return r;
                    }
                }
                Err(e) => return internal(e),
            }
        }
    }
    voices_of(&state, book_id, character_id).await
}

#[derive(Debug, Deserialize)]
pub struct ArcStepRequest {
    pub from_chapter: i32,
    #[serde(default)]
    pub change: String,
    #[serde(default)]
    pub description: String,
}

fn own_voice(c: &VettingCharacter) -> Result<(), Response> {
    if c.narrator || (!c.pooled && !c.generic) {
        Ok(())
    } else {
        Err(fail(
            StatusCode::CONFLICT,
            "pooled and generic speakers have no voice of their own to change",
        ))
    }
}

/// POST /api/v1/admin/books/{book_id}/cast/characters/{character_id}/voices
///
/// A lasting change to the voice from a chapter on, as the voice pass writes
/// into the bible's `arc`.
pub async fn add_voice_change(
    State(state): State<AppState>,
    auth: AuthUser,
    Path((book_id, character_id)): Path<(Uuid, Uuid)>,
    Json(req): Json<ArcStepRequest>,
) -> Response {
    let cast = match vetting_cast(&state, &auth, book_id).await {
        Ok(c) => c,
        Err(r) => return r,
    };
    if req.from_chapter < 1 {
        return fail(
            StatusCode::BAD_REQUEST,
            "a voice change starts after the first chapter",
        );
    }
    let character = match vetting::character(&state.pool, book_id, character_id).await {
        Ok(Some(c)) => c,
        Ok(None) => return fail(StatusCode::NOT_FOUND, "no such character"),
        Err(e) => return internal(e),
    };
    if let Err(r) = own_voice(&character) {
        return r;
    }
    let mut arc: Vec<serde_json::Value> = character.bible["arc"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    if book_cast_vetting::arc_steps(&character.bible)
        .iter()
        .any(|(from, _)| *from == req.from_chapter)
    {
        return fail(
            StatusCode::CONFLICT,
            "the voice already changes at that chapter",
        );
    }
    arc.push(
        json!({"from_chapter": req.from_chapter, "change": req.change, "voice": req.description}),
    );
    arc.sort_by_key(|s| s["from_chapter"].as_i64());
    super::book_cast_edits::save_bible(&state, &cast, character_id, json!({"arc": arc})).await
}

/// DELETE /api/v1/admin/books/{book_id}/cast/characters/{character_id}/voices/{from_chapter}
pub async fn remove_voice_change(
    State(state): State<AppState>,
    auth: AuthUser,
    Path((book_id, character_id, from_chapter)): Path<(Uuid, Uuid, i32)>,
) -> Response {
    let cast = match vetting_cast(&state, &auth, book_id).await {
        Ok(c) => c,
        Err(r) => return r,
    };
    if from_chapter < 1 {
        return fail(
            StatusCode::BAD_REQUEST,
            "the first voice is not a change to remove",
        );
    }
    let character = match vetting::character(&state.pool, book_id, character_id).await {
        Ok(Some(c)) => c,
        Ok(None) => return fail(StatusCode::NOT_FOUND, "no such character"),
        Err(e) => return internal(e),
    };
    let arc: Vec<serde_json::Value> = character.bible["arc"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    let kept: Vec<serde_json::Value> = arc
        .iter()
        .filter(|s| s["from_chapter"].as_i64() != Some(i64::from(from_chapter)))
        .cloned()
        .collect();
    if kept.len() == arc.len() {
        return fail(
            StatusCode::NOT_FOUND,
            "the voice does not change at that chapter",
        );
    }
    super::book_cast_edits::save_bible(&state, &cast, character_id, json!({"arc": kept})).await
}
