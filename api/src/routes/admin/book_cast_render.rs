//! Rendering a vetted cast into a full-cast audiobook, and how far the
//! audiobook is from what the cast says now.

use std::collections::HashMap;

use axum::Json;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use serde_json::{Value, json};
use tracing::error;
use uuid::Uuid;

use super::book_cast::{RENDER_JOB, fail, internal, running};
use crate::AppState;
use crate::book_cast::eta;
use crate::book_cast_render::fingerprint::{self, Stale};
use crate::book_cast_render::{ChapterStatus, Rendered, status, unapproved, work_dir};
use crate::repositories::book_cast_render as repo;
use crate::repositories::book_cast_vetting::{self as vetting, CastVoice, VettingCharacter};
use crate::repositories::book_casts::{self, BookCast, CastChapter, CastLine, CastState};
use crate::repositories::{RepositoryError, user_voice_presets};
use crate::routes::AuthUser;

pub(super) struct Loaded<'a> {
    pub chapters: &'a [CastChapter],
    pub chapter_chars: &'a HashMap<i32, u64>,
    pub lines: &'a [CastLine],
    pub characters: &'a [VettingCharacter],
    pub voices: &'a [CastVoice],
}

async fn chapter_status(
    state: &AppState,
    cast: &BookCast,
    chapters: &[CastChapter],
    lines: &[CastLine],
    characters: &[VettingCharacter],
    voices: &[CastVoice],
) -> Result<(Vec<ChapterStatus>, Vec<repo::RenderedChapter>), RepositoryError> {
    let preset_ids: Vec<Uuid> = voices.iter().filter_map(|v| v.preset_id).collect();
    let (rendered, presets) = tokio::try_join!(
        repo::rendered(&state.pool, cast.book_id),
        user_voice_presets::presets_by_ids(&state.pool, cast.owner_id, &preset_ids),
    )?;
    let presets = presets.into_iter().map(|p| (p.id, p)).collect();
    let snapshot = fingerprint::Cast {
        characters,
        voices,
        presets: &presets,
    };
    let work = work_dir(&state.config.storage_root, cast.book_id);
    Ok((
        status(chapters, lines, &snapshot, &rendered, &work),
        rendered,
    ))
}

/// The render half of `GET .../cast`: which chapters are up to date and why
/// the others are not, what still stands in the render's way, how long the
/// rest would take at the pace so far, and the audiobook it made.
pub(super) async fn progress(
    state: &AppState,
    cast: &BookCast,
    loaded: Loaded<'_>,
) -> Result<Value, RepositoryError> {
    let ((chapters, rendered), audiobook) = tokio::try_join!(
        chapter_status(
            state,
            cast,
            loaded.chapters,
            loaded.lines,
            loaded.characters,
            loaded.voices
        ),
        repo::audiobook(&state.pool, cast.book_id),
    )?;
    let chars = |i: i32| loaded.chapter_chars.get(&i).copied().unwrap_or_default();
    let count = |want: fn(&Rendered) -> bool| chapters.iter().filter(|c| want(&c.rendered)).count();
    let stale: Vec<&Stale> = chapters
        .iter()
        .filter_map(|c| match &c.rendered {
            Rendered::Stale(s) => Some(s),
            _ => None,
        })
        .collect();
    let timed: Vec<&repo::RenderedChapter> = rendered.iter().filter(|r| r.render_ms > 0).collect();
    let left: u64 = chapters
        .iter()
        .filter(|c| c.rendered != Rendered::Fresh)
        .map(|c| chars(c.chapter_index))
        .sum();
    let eta_seconds = eta::remaining(
        timed.iter().map(|r| chars(r.chapter_index)).sum(),
        left,
        timed.iter().map(|r| r.render_ms).sum::<i64>() as f64 / 1000.0,
    );
    Ok(json!({
        "chapters": {
            "total": chapters.len(),
            "done": count(|r| *r == Rendered::Fresh),
            "stale": stale.len(),
            "missing": count(|r| *r == Rendered::Missing),
        },
        "stale": stale,
        "unapproved": unapproved(loaded.characters, loaded.voices),
        "audiobook": audiobook,
        "eta_seconds": eta_seconds,
    }))
}

/// POST /api/v1/admin/books/{book_id}/cast/render
///
/// Renders the chapters missing or stale, then muxes and imports the book in
/// place of the audiobook a render made before.
pub async fn start(
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
    match cast.state {
        CastState::Casting | CastState::Done | CastState::Failed => {}
        CastState::Rendering => match running(&state, RENDER_JOB, book_id).await {
            Ok(false) => {}
            Ok(true) => return fail(StatusCode::CONFLICT, "the cast is already rendering"),
            Err(r) => return r,
        },
        _ => return fail(StatusCode::CONFLICT, "the cast is not ready to render"),
    }
    let loaded = async {
        let (chapters, lines, characters, voices, audiobook) = tokio::try_join!(
            book_casts::chapters(&state.pool, book_id),
            book_casts::lines(&state.pool, book_id, None),
            vetting::characters(&state.pool, book_id),
            vetting::voices(&state.pool, book_id),
            repo::audiobook(&state.pool, book_id),
        )?;
        let (status, _) =
            chapter_status(&state, &cast, &chapters, &lines, &characters, &voices).await?;
        Ok::<_, RepositoryError>((characters, voices, audiobook, status))
    }
    .await;
    let (characters, voices, audiobook, status) = match loaded {
        Ok(v) => v,
        Err(e) => return internal(e),
    };
    let missing = unapproved(&characters, &voices);
    if !missing.is_empty() {
        return (
            StatusCode::CONFLICT,
            Json(json!({
                "error": "every voice must be approved before the cast is rendered",
                "unapproved": missing,
            })),
        )
            .into_response();
    }
    if audiobook.is_some() && status.iter().all(|c| c.rendered == Rendered::Fresh) {
        return fail(StatusCode::CONFLICT, "the audiobook is up to date");
    }
    match repo::begin(&state.pool, book_id, cast.state).await {
        Ok(true) => {}
        Ok(false) => return fail(StatusCode::CONFLICT, "the cast is already rendering"),
        Err(e) => return internal(e),
    }
    match crate::jobs::enqueue_book_cast(&state.pool, state.nats.publisher(), RENDER_JOB, book_id)
        .await
    {
        Ok(job_id) => (
            StatusCode::ACCEPTED,
            Json(json!({"job_id": job_id, "job_type": RENDER_JOB})),
        )
            .into_response(),
        Err(e) => {
            if let Err(set) = book_casts::set_state(
                &state.pool,
                book_id,
                CastState::Failed,
                Some(&e.to_string()),
            )
            .await
            {
                error!(error = %set, book_id = %book_id, "a cast whose render was never queued is left rendering");
            }
            internal(e)
        }
    }
}
