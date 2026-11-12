//! Hand corrections to a cast: who says a line, which names are one person,
//! and what the bible says about them.

use axum::Json;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use serde::Deserialize;
use serde_json::{Value, json};
use uuid::Uuid;

use super::book_cast::{fail, internal, vetting_cast};
use super::book_cast_voices::reseed;
use crate::AppState;
use crate::book_cast::POOL_BELOW;
use crate::book_cast::chapters::same_name;
use crate::repositories::RepositoryError;
use crate::repositories::book_cast_vetting::{
    self as vetting, CharacterEdit, LineRef, NameTo, Said, Split, VettingCharacter,
};
use crate::repositories::book_casts::BookCast;
use crate::routes::AuthUser;

fn conflict_on_name(e: RepositoryError) -> Response {
    match &e {
        RepositoryError::Database(sqlx::Error::Database(db)) if db.is_unique_violation() => fail(
            StatusCode::CONFLICT,
            "the cast already has a character of that name",
        ),
        _ => internal(e),
    }
}

async fn character(
    state: &AppState,
    book_id: Uuid,
    character_id: Uuid,
) -> Result<VettingCharacter, Response> {
    vetting::character(&state.pool, book_id, character_id)
        .await
        .map_err(internal)?
        .ok_or_else(|| fail(StatusCode::NOT_FOUND, "no such character"))
}

fn not_narrator(c: &VettingCharacter) -> Result<(), Response> {
    if c.narrator {
        Err(fail(
            StatusCode::BAD_REQUEST,
            "the narrator reads narration; give it lines by making them narration",
        ))
    } else {
        Ok(())
    }
}

async fn done(state: &AppState, cast: &BookCast) -> Response {
    match reseed(state, cast).await {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(r) => r,
    }
}

pub(super) async fn save_bible(
    state: &AppState,
    cast: &BookCast,
    character_id: Uuid,
    bible: Value,
) -> Response {
    let edit = CharacterEdit {
        name: None,
        aliases: None,
        bible: Some(bible),
        generic: None,
        pooled: None,
    };
    if let Err(e) =
        vetting::update_character(&state.pool, cast.book_id, character_id, &edit, POOL_BELOW).await
    {
        return internal(e);
    }
    done(state, cast).await
}

#[derive(Debug, Deserialize)]
pub struct ReassignRequest {
    pub character_id: Option<Uuid>,
    #[serde(default)]
    pub not_speech: bool,
}

/// PUT /api/v1/admin/books/{book_id}/cast/lines/{segment_index}/{span_start}
///
/// `character_id` null, or the narrator's, makes the line narration;
/// `not_speech` marks a quote nobody says aloud.
pub async fn reassign_line(
    State(state): State<AppState>,
    auth: AuthUser,
    Path((book_id, segment_index, span_start)): Path<(Uuid, i32, i32)>,
    Json(req): Json<ReassignRequest>,
) -> Response {
    let cast = match vetting_cast(&state, &auth, book_id).await {
        Ok(c) => c,
        Err(r) => return r,
    };
    let to = match (req.character_id, req.not_speech) {
        (Some(_), true) => {
            return fail(
                StatusCode::BAD_REQUEST,
                "a line nobody says has no character",
            );
        }
        (Some(id), false) => match character(&state, book_id, id).await {
            Ok(c) if c.narrator => None,
            Ok(c) => Some(c),
            Err(r) => return r,
        },
        (None, _) => None,
    };
    let said = match &to {
        Some(c) => Said::By(c),
        None if req.not_speech => Said::NotSpeech,
        None => Said::Narration,
    };
    let line = LineRef {
        segment_index,
        span_start,
    };
    match vetting::reassign_line(&state.pool, book_id, &line, said, POOL_BELOW).await {
        Ok(true) => done(&state, &cast).await,
        Ok(false) => fail(StatusCode::NOT_FOUND, "no such line"),
        Err(e) => internal(e),
    }
}

#[derive(Debug, Deserialize)]
pub struct MergeRequest {
    pub absorb: Uuid,
}

/// POST /api/v1/admin/books/{book_id}/cast/characters/{character_id}/merge
pub async fn merge(
    State(state): State<AppState>,
    auth: AuthUser,
    Path((book_id, character_id)): Path<(Uuid, Uuid)>,
    Json(req): Json<MergeRequest>,
) -> Response {
    let cast = match vetting_cast(&state, &auth, book_id).await {
        Ok(c) => c,
        Err(r) => return r,
    };
    if req.absorb == character_id {
        return fail(StatusCode::BAD_REQUEST, "a character cannot absorb itself");
    }
    let pair = async {
        let survivor = character(&state, book_id, character_id).await?;
        let absorbed = character(&state, book_id, req.absorb).await?;
        not_narrator(&survivor)?;
        not_narrator(&absorbed)?;
        Ok::<_, Response>((survivor, absorbed))
    }
    .await;
    let (survivor, absorbed) = match pair {
        Ok(p) => p,
        Err(r) => return r,
    };
    match vetting::merge(&state.pool, book_id, survivor.id, absorbed.id, POOL_BELOW).await {
        Ok(()) => done(&state, &cast).await,
        Err(e) => internal(e),
    }
}

#[derive(Debug, Deserialize)]
pub struct LineKey {
    pub segment_index: i32,
    pub span_start: i32,
}

#[derive(Debug, Deserialize)]
pub struct SplitRequest {
    pub name: String,
    #[serde(default)]
    pub aliases: Vec<String>,
    #[serde(default)]
    pub lines: Vec<LineKey>,
}

/// POST /api/v1/admin/books/{book_id}/cast/characters/{character_id}/split
pub async fn split(
    State(state): State<AppState>,
    auth: AuthUser,
    Path((book_id, character_id)): Path<(Uuid, Uuid)>,
    Json(req): Json<SplitRequest>,
) -> Response {
    let cast = match vetting_cast(&state, &auth, book_id).await {
        Ok(c) => c,
        Err(r) => return r,
    };
    let name = req.name.trim();
    if name.is_empty() {
        return fail(StatusCode::BAD_REQUEST, "the new character needs a name");
    }
    if req.lines.is_empty() && req.aliases.is_empty() {
        return fail(StatusCode::BAD_REQUEST, "move at least one line or name");
    }
    let from = match character(&state, book_id, character_id).await {
        Ok(c) => c,
        Err(r) => return r,
    };
    if let Err(r) = not_narrator(&from) {
        return r;
    }
    let lines: Vec<LineRef> = req
        .lines
        .iter()
        .map(|l| LineRef {
            segment_index: l.segment_index,
            span_start: l.span_start,
        })
        .collect();
    let split = Split {
        name,
        aliases: &req.aliases,
        lines: &lines,
    };
    match vetting::split(&state.pool, book_id, &from, &split, POOL_BELOW).await {
        Ok(Some(id)) => match reseed(&state, &cast).await {
            Ok(()) => (StatusCode::CREATED, Json(json!({"id": id}))).into_response(),
            Err(r) => r,
        },
        Ok(None) => fail(
            StatusCode::BAD_REQUEST,
            "every line moved has to be the character's own",
        ),
        Err(e) => conflict_on_name(e),
    }
}

#[derive(Debug, Deserialize)]
pub struct MoveNameRequest {
    pub name: String,
    pub character_id: Option<Uuid>,
    pub new_name: Option<String>,
}

/// POST /api/v1/admin/books/{book_id}/cast/characters/{character_id}/names/move
///
/// One of the character's aliases, and the lines pass 1 gave it, go to
/// `character_id`, or to a new character called `new_name` (the alias itself
/// when left out).
pub async fn move_name(
    State(state): State<AppState>,
    auth: AuthUser,
    Path((book_id, character_id)): Path<(Uuid, Uuid)>,
    Json(req): Json<MoveNameRequest>,
) -> Response {
    let cast = match vetting_cast(&state, &auth, book_id).await {
        Ok(c) => c,
        Err(r) => return r,
    };
    let from = match character(&state, book_id, character_id).await {
        Ok(c) => c,
        Err(r) => return r,
    };
    if let Err(r) = not_narrator(&from) {
        return r;
    }
    let asked = req.name.trim();
    if same_name(&from.name, asked) {
        return fail(
            StatusCode::BAD_REQUEST,
            "rename the character to another of its names before moving this one",
        );
    }
    let Some(name) = from.aliases.iter().find(|a| same_name(a, asked)) else {
        return fail(StatusCode::BAD_REQUEST, "the character has no such name");
    };
    let existing = match req.character_id {
        Some(id) if id == from.id => {
            return fail(StatusCode::BAD_REQUEST, "the name is already theirs");
        }
        Some(id) => match character(&state, book_id, id).await {
            Ok(c) => match not_narrator(&c) {
                Ok(()) => Some(c),
                Err(r) => return r,
            },
            Err(r) => return r,
        },
        None => None,
    };
    let to = match &existing {
        Some(c) => NameTo::Existing(c),
        None => NameTo::New(
            req.new_name
                .as_deref()
                .map(str::trim)
                .filter(|n| !n.is_empty())
                .unwrap_or(name),
        ),
    };
    match vetting::move_name(&state.pool, book_id, &from, name, to, POOL_BELOW).await {
        Ok(id) => match reseed(&state, &cast).await {
            Ok(()) => {
                let status = if existing.is_some() {
                    StatusCode::OK
                } else {
                    StatusCode::CREATED
                };
                (status, Json(json!({"id": id}))).into_response()
            }
            Err(r) => r,
        },
        Err(e) => conflict_on_name(e),
    }
}

#[derive(Debug, Deserialize)]
pub struct AttachRequest {
    pub speaker: String,
    pub character_id: Uuid,
}

/// POST /api/v1/admin/books/{book_id}/cast/speakers/attach
///
/// A speaker name the analysis left to nobody goes to a character, with all
/// its lines, and becomes one of their aliases.
pub async fn attach_speaker(
    State(state): State<AppState>,
    auth: AuthUser,
    Path(book_id): Path<Uuid>,
    Json(req): Json<AttachRequest>,
) -> Response {
    let cast = match vetting_cast(&state, &auth, book_id).await {
        Ok(c) => c,
        Err(r) => return r,
    };
    let to = match character(&state, book_id, req.character_id).await {
        Ok(c) => c,
        Err(r) => return r,
    };
    if let Err(r) = not_narrator(&to) {
        return r;
    }
    match vetting::attach_speaker(&state.pool, book_id, req.speaker.trim(), &to, POOL_BELOW).await {
        Ok(0) => fail(
            StatusCode::NOT_FOUND,
            "no unattached lines have that speaker",
        ),
        Ok(n) => match reseed(&state, &cast).await {
            Ok(()) => Json(json!({"attached": n})).into_response(),
            Err(r) => r,
        },
        Err(e) => internal(e),
    }
}

#[derive(Debug, Deserialize)]
pub struct CharacterPatch {
    pub name: Option<String>,
    pub aliases: Option<Vec<String>>,
    pub bible: Option<Value>,
    pub generic: Option<bool>,
    pub pooled: Option<bool>,
}

/// PATCH /api/v1/admin/books/{book_id}/cast/characters/{character_id}
///
/// `bible` is merged into the character's; a field set to null is removed.
/// `pooled` puts a character in the shared minor-part voices, or takes them
/// out, whatever their line count says.
pub async fn update_character(
    State(state): State<AppState>,
    auth: AuthUser,
    Path((book_id, character_id)): Path<(Uuid, Uuid)>,
    Json(req): Json<CharacterPatch>,
) -> Response {
    let cast = match vetting_cast(&state, &auth, book_id).await {
        Ok(c) => c,
        Err(r) => return r,
    };
    let c = match character(&state, book_id, character_id).await {
        Ok(c) => c,
        Err(r) => return r,
    };
    if c.narrator && req.generic == Some(true) {
        return fail(StatusCode::BAD_REQUEST, "the narrator cannot be generic");
    }
    if c.narrator && req.pooled == Some(true) {
        return fail(
            StatusCode::BAD_REQUEST,
            "the narrator cannot share a pooled voice",
        );
    }
    if req.bible.as_ref().is_some_and(|b| !b.is_object()) {
        return fail(StatusCode::BAD_REQUEST, "bible must be an object");
    }
    let name = req.name.as_deref().map(str::trim);
    if name == Some("") {
        return fail(StatusCode::BAD_REQUEST, "the name is empty");
    }
    let edit = CharacterEdit {
        name: name.map(str::to_string),
        aliases: req.aliases,
        bible: req.bible,
        generic: req.generic,
        pooled: req.pooled,
    };
    match vetting::update_character(&state.pool, book_id, character_id, &edit, POOL_BELOW).await {
        Ok(()) => done(&state, &cast).await,
        Err(e) => conflict_on_name(e),
    }
}
