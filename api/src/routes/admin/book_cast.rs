//! Full-cast audiobook analysis of a book: start, resume, rerun or redo it,
//! and read back the cast and each chapter's attributed lines.

use std::collections::HashMap;

use axum::Json;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use serde::Deserialize;
use serde_json::{Value, json};
use tracing::error;
use uuid::Uuid;

use crate::AppState;
use crate::book_cast::voices::needs_design;
use crate::book_cast::{POOL_BELOW, chapters, eta};
use crate::book_cast_render::work_dir;
use crate::book_cast_vetting::{self, clips, line_context, line_text};
use crate::jobs::handlers::APPROVE_JOB;
use crate::repositories::book_cast_vetting::{self as vetting, VettingCharacter};
use crate::repositories::book_casts::{self, BookCast, CastChapter, CastLine, CastState};
use crate::repositories::{ebook_segments, job_status};
use crate::routes::AuthUser;

const SAMPLES: usize = 3;

#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Rerun {
    Chapters,
    Bible,
}

/// `fresh` redoes everything from nothing; `rerun` runs the analysis again
/// from a pass, keeping everything vetted.
#[derive(Debug, Default, Deserialize)]
pub struct StartRequest {
    #[serde(default)]
    pub fresh: bool,
    pub rerun: Option<Rerun>,
}

pub(super) fn fail(status: StatusCode, message: &str) -> Response {
    (status, Json(json!({"error": message}))).into_response()
}

pub(super) fn internal(e: impl std::fmt::Display) -> Response {
    error!(error = %e, "book cast route failed");
    fail(StatusCode::INTERNAL_SERVER_ERROR, "book cast failed")
}

/// The cast as vetting needs it: there, and finished with analysis. Rendering
/// does not close it: a chapter read before an edit is just stale after it.
pub(super) async fn vetting_cast(
    state: &AppState,
    auth: &AuthUser,
    book_id: Uuid,
) -> Result<BookCast, Response> {
    auth.require_admin().map_err(IntoResponse::into_response)?;
    match book_casts::get(&state.pool, book_id).await {
        Ok(Some(c))
            if matches!(
                c.state,
                CastState::Casting | CastState::Rendering | CastState::Done | CastState::Failed
            ) =>
        {
            Ok(c)
        }
        Ok(Some(_)) => Err(fail(
            StatusCode::CONFLICT,
            "the cast is still being analysed",
        )),
        Ok(None) => Err(fail(StatusCode::NOT_FOUND, "the book has no cast")),
        Err(e) => Err(internal(e)),
    }
}

pub const RENDER_JOB: &str = "book_cast_render";
const ANALYSIS_JOBS: [&str; 3] = ["book_cast_chapters", "book_cast_bible", "book_cast_voices"];

pub(super) async fn running(state: &AppState, job: &str, book_id: Uuid) -> Result<bool, Response> {
    job_status::latest_active_for_entity(&state.pool, job, book_id)
        .await
        .map(|j| j.is_some())
        .map_err(internal)
}

/// POST /api/v1/admin/books/{book_id}/cast — resumes the pass a cast stopped
/// in, reruns the analysis from a pass with `{"rerun": "chapters" | "bible"}`
/// keeping everything vetted, or redoes everything with `{"fresh": true}`.
pub async fn start(
    State(state): State<AppState>,
    auth: AuthUser,
    Path(book_id): Path<Uuid>,
    body: Option<Json<StartRequest>>,
) -> Response {
    if let Err(e) = auth.require_admin() {
        return e.into_response();
    }
    let req = body.map(|b| b.0).unwrap_or_default();
    let existing = match book_casts::get(&state.pool, book_id).await {
        Ok(c) => c,
        Err(e) => return internal(e),
    };
    if existing
        .as_ref()
        .is_some_and(|c| c.state == CastState::Rendering)
        && (req.fresh || req.rerun.is_some())
    {
        match running(&state, RENDER_JOB, book_id).await {
            Ok(false) => {}
            Ok(true) => {
                return fail(
                    StatusCode::CONFLICT,
                    "the cast is rendering; let it finish first",
                );
            }
            Err(r) => return r,
        }
    }
    let job = match (existing, req.rerun) {
        (Some(cast), _) if req.fresh => match redo(&state, &auth, &cast).await {
            Ok(job) => job,
            Err(r) => return r,
        },
        (None, Some(_)) => return fail(StatusCode::NOT_FOUND, "the book has no cast"),
        (Some(_), Some(from)) => {
            let (state_from, job) = match from {
                Rerun::Chapters => (CastState::Chapters, "book_cast_chapters"),
                Rerun::Bible => (CastState::Bible, "book_cast_bible"),
            };
            match book_casts::rerun(&state.pool, book_id, state_from).await {
                Ok(true) => job,
                Ok(false) => {
                    return fail(
                        StatusCode::CONFLICT,
                        "the analysis is still running; resume it instead",
                    );
                }
                Err(e) => return internal(e),
            }
        }
        (Some(cast), None) => {
            let job = match cast.state {
                CastState::Chapters => "book_cast_chapters",
                CastState::Bible => "book_cast_bible",
                CastState::Voices => "book_cast_voices",
                _ => {
                    return fail(
                        StatusCode::CONFLICT,
                        "the cast is past analysis; rerun or redo it",
                    );
                }
            };
            match running(&state, job, book_id).await {
                Ok(false) => job,
                Ok(true) => return fail(StatusCode::CONFLICT, "the analysis is already running"),
                Err(r) => return r,
            }
        }
        (None, None) => match begin(&state, &auth, book_id).await {
            Ok(job) => job,
            Err(r) => return r,
        },
    };
    match crate::jobs::enqueue_book_cast(&state.pool, state.nats.publisher(), job, book_id).await {
        Ok(job_id) => (
            StatusCode::ACCEPTED,
            Json(json!({"job_id": job_id, "job_type": job})),
        )
            .into_response(),
        Err(e) => internal(e),
    }
}

async fn extracted_file(state: &AppState, book_id: Uuid) -> Result<Uuid, Response> {
    ebook_segments::file_with_segments(&state.pool, book_id)
        .await
        .map_err(internal)?
        .ok_or_else(|| {
            fail(
                StatusCode::CONFLICT,
                "the book has no extracted text; run ebook extraction first",
            )
        })
}

async fn cancel_active(state: &AppState, job: &str, book_id: Uuid) -> Result<(), Response> {
    while let Some(active) = job_status::latest_active_for_entity(&state.pool, job, book_id)
        .await
        .map_err(internal)?
    {
        if crate::routes::jobs::cancel(state, active.id)
            .await
            .map_err(internal)?
            .is_none()
        {
            break;
        }
    }
    Ok(())
}

/// Everything a cast made goes: the jobs still working on it, then the cast
/// itself with its rows, then its clones from the disk and the engine and its
/// rendered chapters. The files go last, once the cast no longer points to them.
async fn redo(
    state: &AppState,
    auth: &AuthUser,
    cast: &BookCast,
) -> Result<&'static str, Response> {
    let file_id = extracted_file(state, cast.book_id).await?;
    let clips = vetting::cast_clips(&state.pool, cast.book_id)
        .await
        .map_err(internal)?;
    for job in ANALYSIS_JOBS.into_iter().chain([APPROVE_JOB]) {
        cancel_active(state, job, cast.book_id).await?;
    }
    book_casts::start(&state.pool, cast.book_id, file_id, auth.user_id)
        .await
        .map_err(internal)?;
    for preset_id in clips {
        clips::discard(
            &state.config.storage_root,
            &state.runtime_config,
            cast.owner_id,
            preset_id,
        )
        .await;
    }
    let work = work_dir(&state.config.storage_root, cast.book_id);
    if work.exists() {
        tokio::fs::remove_dir_all(&work).await.map_err(internal)?;
    }
    Ok("book_cast_chapters")
}

async fn begin(state: &AppState, auth: &AuthUser, book_id: Uuid) -> Result<&'static str, Response> {
    let file_id = extracted_file(state, book_id).await?;
    book_casts::start(&state.pool, book_id, file_id, auth.user_id)
        .await
        .map_err(internal)?;
    Ok("book_cast_chapters")
}

/// How many voices the voice pass will design: before the cast pass has run,
/// a guess from the speakers with enough lines for a voice of their own.
fn voices_to_design(
    state: CastState,
    characters: &[VettingCharacter],
    lines: &[CastLine],
) -> usize {
    if state == CastState::Voices {
        let narrator = usize::from(!characters.iter().any(|c| c.narrator));
        return narrator + characters.iter().filter(|c| needs_design(c)).count();
    }
    let mut counts: HashMap<String, i32> = HashMap::new();
    for speaker in lines.iter().filter_map(|l| l.speaker.as_deref()) {
        *counts.entry(speaker.to_lowercase()).or_default() += 1;
    }
    1 + counts.values().filter(|&&n| n >= POOL_BELOW).count()
}

/// GET /api/v1/admin/books/{book_id}/cast
pub async fn get(
    State(state): State<AppState>,
    auth: AuthUser,
    Path(book_id): Path<Uuid>,
) -> Response {
    if let Err(e) = auth.require_admin() {
        return e.into_response();
    }
    let cast = match book_casts::get(&state.pool, book_id).await {
        Ok(Some(c)) => c,
        Ok(None) => return fail(StatusCode::NOT_FOUND, "the book has no cast"),
        Err(e) => return internal(e),
    };
    let loaded = async {
        let segments = ebook_segments::get_for_file(&state.pool, cast.book_file_id).await?;
        let times = book_casts::chapter_times(&state.pool, book_id).await?;
        let cast_chapters = book_casts::chapters(&state.pool, book_id).await?;
        let characters = vetting::characters(&state.pool, book_id).await?;
        let voices = vetting::voices(&state.pool, book_id).await?;
        let unattached = vetting::unattached_speakers(&state.pool, book_id).await?;
        let presets = vetting::preset_choices(&state.pool, cast.owner_id).await?;
        let lines = book_casts::lines(&state.pool, book_id, None).await?;
        let approving =
            job_status::latest_active_for_entity(&state.pool, APPROVE_JOB, book_id).await?;
        Ok::<_, crate::repositories::RepositoryError>((
            segments,
            times,
            cast_chapters,
            characters,
            voices,
            unattached,
            presets,
            lines,
            approving,
        ))
    }
    .await;
    let (segments, times, cast_chapters, characters, voices, unattached, presets, lines, approving) =
        match loaded {
            Ok(v) => v,
            Err(e) => return internal(e),
        };
    let texts: HashMap<i32, String> = segments
        .iter()
        .map(|s| (s.segment_index, s.text.clone()))
        .collect();
    let chapter_chars: HashMap<i32, u64> = chapters::group(
        segments
            .into_iter()
            .map(|s| (s.segment_index, s.text, s.chapter_title)),
    )
    .into_iter()
    .map(|c| {
        let chars = c.paragraphs.iter().map(|p| p.text.chars().count() as u64);
        (c.index, chars.sum())
    })
    .collect();
    let total = chapter_chars.len();
    let analysis_eta = eta::analysis(
        cast.state,
        eta::pass1(&chapter_chars, &times, cast.analysis_started_at),
        voices_to_design(cast.state, &characters, &lines),
    );
    let render = match super::book_cast_render::progress(
        &state,
        &cast,
        super::book_cast_render::Loaded {
            chapters: &cast_chapters,
            chapter_chars: &chapter_chars,
            lines: &lines,
            characters: &characters,
            voices: &voices,
        },
    )
    .await
    {
        Ok(r) => r,
        Err(e) => return internal(e),
    };

    let mut by_character: HashMap<Uuid, Vec<&CastLine>> = HashMap::new();
    for line in &lines {
        if let Some(id) = line.character_id {
            by_character.entry(id).or_default().push(line);
        }
    }
    let characters: Vec<Value> = characters
        .into_iter()
        .map(|c| {
            let own = by_character.remove(&c.id).unwrap_or_default();
            let mut samples: Vec<&str> = own.iter().filter_map(|l| line_text(&texts, l)).collect();
            samples.sort_by_key(|t| std::cmp::Reverse(t.len()));
            samples.truncate(SAMPLES);
            let names: Vec<Value> = std::iter::once(&c.name)
                .chain(&c.aliases)
                .map(|name| {
                    let lower = name.to_lowercase();
                    let said = own
                        .iter()
                        .filter(|l| {
                            l.speaker
                                .as_ref()
                                .is_some_and(|s| s.to_lowercase() == lower)
                        })
                        .count();
                    json!({"name": name, "lines": said})
                })
                .collect();
            let mut v = serde_json::to_value(&c).unwrap_or_default();
            v["names"] = json!(names);
            v["samples"] = json!(samples);
            v["voices"] = json!(
                voices
                    .iter()
                    .filter(|voice| voice.character_id == c.id)
                    .collect::<Vec<_>>()
            );
            v
        })
        .collect();

    Json(json!({
        "state": cast.state,
        "error": cast.error,
        "updated_at": cast.updated_at.format(&time::format_description::well_known::Rfc3339).ok(),
        "chapters": {"total": total, "done": times.len()},
        "lines": lines.iter().filter(|l| l.spoken()).count(),
        "unresolved": lines.iter().filter(|l| l.needs_speaker()).count(),
        "characters": characters,
        "unattached": unattached,
        "presets": presets,
        "render": render,
        "eta": {"analysis_seconds": analysis_eta},
        "approving": approving.map(|j| json!({
            "id": j.id, "status": j.status, "progress": j.progress,
        })),
    }))
    .into_response()
}

/// GET /api/v1/admin/books/{book_id}/cast/chapters/{chapter}
pub async fn chapter_lines(
    State(state): State<AppState>,
    auth: AuthUser,
    Path((book_id, chapter)): Path<(Uuid, i32)>,
) -> Response {
    if let Err(e) = auth.require_admin() {
        return e.into_response();
    }
    let cast = match book_casts::get(&state.pool, book_id).await {
        Ok(Some(c)) => c,
        Ok(None) => return fail(StatusCode::NOT_FOUND, "the book has no cast"),
        Err(e) => return internal(e),
    };
    let (segments, lines, characters, voices, cast_chapters) = match tokio::try_join!(
        ebook_segments::get_for_file(&state.pool, cast.book_file_id),
        book_casts::lines(&state.pool, book_id, Some(chapter)),
        vetting::characters(&state.pool, book_id),
        vetting::voices(&state.pool, book_id),
        book_casts::chapters(&state.pool, book_id),
    ) {
        Ok(v) => v,
        Err(e) => return internal(e),
    };
    let texts: HashMap<i32, String> = segments
        .into_iter()
        .map(|s| (s.segment_index, s.text))
        .collect();
    let this_chapter = cast_chapters.iter().find(|c| c.chapter_index == chapter);
    let lines: Vec<Value> = lines
        .iter()
        .map(|l| {
            let mut v = serde_json::to_value(l).unwrap_or_default();
            v["text"] = json!(line_text(&texts, l));
            v["context"] = json!(line_context(&texts, this_chapter, l));
            v["voice"] = json!(book_cast_vetting::voice_for(
                &characters,
                &voices,
                l.character_id,
                l.chapter_index
            ));
            v
        })
        .collect();
    Json(json!({"lines": lines})).into_response()
}

/// GET /api/v1/admin/books/{book_id}/cast/lines/unattributed — every line in
/// the book still waiting for a speaker, with the text around it to judge it by.
pub async fn unattributed(
    State(state): State<AppState>,
    auth: AuthUser,
    Path(book_id): Path<Uuid>,
) -> Response {
    if let Err(e) = auth.require_admin() {
        return e.into_response();
    }
    let cast = match book_casts::get(&state.pool, book_id).await {
        Ok(Some(c)) => c,
        Ok(None) => return fail(StatusCode::NOT_FOUND, "the book has no cast"),
        Err(e) => return internal(e),
    };
    let (segments, lines, cast_chapters) = match tokio::try_join!(
        ebook_segments::get_for_file(&state.pool, cast.book_file_id),
        book_casts::lines(&state.pool, book_id, None),
        book_casts::chapters(&state.pool, book_id),
    ) {
        Ok(v) => v,
        Err(e) => return internal(e),
    };
    let texts: HashMap<i32, String> = segments
        .into_iter()
        .map(|s| (s.segment_index, s.text))
        .collect();
    let by_index: HashMap<i32, &CastChapter> =
        cast_chapters.iter().map(|c| (c.chapter_index, c)).collect();
    let lines: Vec<Value> = lines
        .iter()
        .filter(|l| l.needs_speaker())
        .map(|l| {
            json!({
                "segment_index": l.segment_index,
                "span_start": l.span_start,
                "span_end": l.span_end,
                "chapter_index": l.chapter_index,
                "chapter_title": by_index.get(&l.chapter_index).and_then(|c| c.title.as_deref()),
                "speaker": l.speaker,
                "cue": l.cue,
                "text": line_text(&texts, l),
                "context": line_context(&texts, by_index.get(&l.chapter_index).copied(), l),
            })
        })
        .collect();
    Json(json!({"lines": lines})).into_response()
}
