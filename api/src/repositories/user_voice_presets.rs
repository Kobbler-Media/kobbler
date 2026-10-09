//! User voice preset repository operations.
//!
//! Per-user TTS voice presets with configurable voice parameters.
//! Each user can have multiple presets and one active preset.

use super::Result;
use serde::Serialize;
use sqlx::{Executor, FromRow, Postgres};
use time::OffsetDateTime;
use uuid::Uuid;

// =============================================================================
// Types
// =============================================================================

/// Stored user voice preset.
///
/// `temperature` is shared across backends; the rest of the engine-native
/// fields (`ref_text` … `max_new_frames`) are NULL for chatterbox-era presets
/// and populated by the voice-page flow. The synth handler reads them only when
/// the active backend is breeze.
///
/// `speed` is a render parameter, not a player setting: it is sent to the engine
/// and the audio comes back at that rate. It acts as the FALLBACK when a synth
/// request omits the field, so a request that means 1x has to say so explicitly
/// rather than leaving it out.
#[derive(Debug, Serialize, FromRow)]
pub struct UserVoicePreset {
    pub id: Uuid,
    pub user_id: Uuid,
    pub name: String,
    pub voice: Option<String>,
    pub clip_filename: Option<String>,
    pub speed: f64,
    pub temperature: Option<f64>,
    pub ref_text: Option<String>,
    pub instructions: Option<String>,
    /// Built-in narrator token "S0".."S9". NULL = the engine's default.
    pub speaker: Option<String>,
    pub seed: Option<i64>,
    pub top_p: Option<f64>,
    pub top_k: Option<i32>,
    pub repetition_penalty: Option<f64>,
    pub max_new_frames: Option<i32>,
    pub created_at: OffsetDateTime,
    pub updated_at: OffsetDateTime,
}

/// Input for creating a new voice preset.
pub struct CreatePreset {
    pub name: String,
    pub voice: Option<String>,
    pub clip_filename: Option<String>,
    pub speed: Option<f64>,
    pub temperature: Option<f64>,
    pub ref_text: Option<String>,
    pub instructions: Option<String>,
    /// Built-in narrator token "S0".."S9". NULL = the engine's default.
    pub speaker: Option<String>,
    pub seed: Option<i64>,
    pub top_p: Option<f64>,
    pub top_k: Option<i32>,
    pub repetition_penalty: Option<f64>,
    pub max_new_frames: Option<i32>,
}

/// Input for updating an existing voice preset.
///
/// `name`, `voice`, `clip_filename`, `speed` use COALESCE (None = keep
/// existing). All nullable fields are passed through `ResolvedNullable` —
/// the route layer resolves `SetOrClear` against the existing row before
/// calling, so the SQL is one-shot.
pub struct UpdatePreset {
    pub name: Option<String>,
    pub voice: Option<String>,
    pub clip_filename: Option<String>,
    pub speed: Option<f64>,
}

/// Resolved snapshot of every nullable preset field. Built by the route
/// layer (which has the existing row in hand) and passed to `update_preset`
/// — keeps the SQL one-shot and avoids round-trips inside the repository.
#[derive(Default)]
pub struct ResolvedNullable {
    pub temperature: Option<f64>,
    pub ref_text: Option<String>,
    pub instructions: Option<String>,
    /// Built-in narrator token "S0".."S9". NULL = the engine's default.
    pub speaker: Option<String>,
    pub seed: Option<i64>,
    pub top_p: Option<f64>,
    pub top_k: Option<i32>,
    pub repetition_penalty: Option<f64>,
    pub max_new_frames: Option<i32>,
}

impl ResolvedNullable {
    /// Snapshot from an existing preset row — used by partial updates that
    /// only touch one column (e.g. clip-only updates) so the rest stay put.
    pub fn from_preset(p: &UserVoicePreset) -> Self {
        Self {
            temperature: p.temperature,
            ref_text: p.ref_text.clone(),
            instructions: p.instructions.clone(),
            speaker: p.speaker.clone(),
            seed: p.seed,
            top_p: p.top_p,
            top_k: p.top_k,
            repetition_penalty: p.repetition_penalty,
            max_new_frames: p.max_new_frames,
        }
    }
}

/// Three-state update field: Skip (don't change), Clear (set NULL), or Set(value).
pub enum SetOrClear<T> {
    /// Don't change this field (use existing DB value).
    Skip,
    /// Set the field to NULL.
    Clear,
    /// Set the field to this value.
    Set(T),
}

impl<T: Copy> SetOrClear<T> {
    /// Convert to Option for SQL binding. Skip returns the existing value.
    pub fn resolve(&self, existing: Option<T>) -> Option<T> {
        match self {
            SetOrClear::Skip => existing,
            SetOrClear::Clear => None,
            SetOrClear::Set(v) => Some(*v),
        }
    }
}

impl<T: Clone> SetOrClear<T> {
    /// Like `resolve` but for non-Copy types (e.g. String).
    pub fn resolve_clone(&self, existing: Option<&T>) -> Option<T> {
        match self {
            SetOrClear::Skip => existing.cloned(),
            SetOrClear::Clear => None,
            SetOrClear::Set(v) => Some(v.clone()),
        }
    }
}

impl<T> From<Option<Option<T>>> for SetOrClear<T> {
    /// Adapter for the JSON tristate (`Option<Option<T>>`):
    /// `None` = absent → Skip; `Some(None)` = explicit null → Clear;
    /// `Some(Some(v))` = set to `v`.
    fn from(v: Option<Option<T>>) -> Self {
        match v {
            None => SetOrClear::Skip,
            Some(None) => SetOrClear::Clear,
            Some(Some(v)) => SetOrClear::Set(v),
        }
    }
}

// =============================================================================
// CRUD Operations
// =============================================================================

const SELECT_COLS: &str = "id, user_id, name, voice, clip_filename, speed, \
                           temperature, ref_text, instructions, speaker, seed, \
                           top_p, top_k, repetition_penalty, max_new_frames, \
                           created_at, updated_at";

/// List all voice presets for a user.
pub async fn list_presets<'e, E>(exec: E, user_id: Uuid) -> Result<Vec<UserVoicePreset>>
where
    E: Executor<'e, Database = Postgres>,
{
    let sql = format!(
        "SELECT {SELECT_COLS} FROM user_voice_presets WHERE user_id = $1 ORDER BY name ASC"
    );
    let rows = sqlx::query_as::<_, UserVoicePreset>(sqlx::AssertSqlSafe(sql.as_str()))
        .bind(user_id)
        .fetch_all(exec)
        .await?;
    Ok(rows)
}

/// Get a single voice preset by ID, scoped to user.
pub async fn get_preset<'e, E>(
    exec: E,
    preset_id: Uuid,
    user_id: Uuid,
) -> Result<Option<UserVoicePreset>>
where
    E: Executor<'e, Database = Postgres>,
{
    let sql =
        format!("SELECT {SELECT_COLS} FROM user_voice_presets WHERE id = $1 AND user_id = $2");
    let row = sqlx::query_as::<_, UserVoicePreset>(sqlx::AssertSqlSafe(sql.as_str()))
        .bind(preset_id)
        .bind(user_id)
        .fetch_optional(exec)
        .await?;
    Ok(row)
}

/// Create a new voice preset.
pub async fn create_preset<'e, E>(
    exec: E,
    user_id: Uuid,
    input: &CreatePreset,
) -> Result<UserVoicePreset>
where
    E: Executor<'e, Database = Postgres>,
{
    let sql = format!(
        r#"
        INSERT INTO user_voice_presets
            (user_id, name, voice, clip_filename, speed, temperature,
             ref_text, instructions, speaker, seed,
             top_p, top_k, repetition_penalty, max_new_frames)
        VALUES
            ($1, $2, $3, $4, COALESCE($5, 1.0), $6,
             $7, $8, $9, $10,
             $11, $12, $13, $14)
        RETURNING {SELECT_COLS}
        "#
    );
    let row = sqlx::query_as::<_, UserVoicePreset>(sqlx::AssertSqlSafe(sql.as_str()))
        .bind(user_id)
        .bind(&input.name)
        .bind(&input.voice)
        .bind(&input.clip_filename)
        .bind(input.speed)
        .bind(input.temperature)
        .bind(&input.ref_text)
        .bind(&input.instructions)
        .bind(&input.speaker)
        .bind(input.seed)
        .bind(input.top_p)
        .bind(input.top_k)
        .bind(input.repetition_penalty)
        .bind(input.max_new_frames)
        .fetch_one(exec)
        .await?;
    Ok(row)
}

/// Update an existing voice preset.
///
/// COALESCE is used for name/voice/clip_filename/speed (None = keep existing).
/// Nullable fields are written directly from `resolved` so the caller's
/// `SetOrClear` semantics survive: pass `Some(v)` to set, `None` to clear.
pub async fn update_preset<'e, E>(
    exec: E,
    preset_id: Uuid,
    user_id: Uuid,
    input: &UpdatePreset,
    resolved: &ResolvedNullable,
) -> Result<Option<UserVoicePreset>>
where
    E: Executor<'e, Database = Postgres>,
{
    let sql = format!(
        r#"
        UPDATE user_voice_presets SET
            name = COALESCE($3, name),
            voice = COALESCE($4, voice),
            clip_filename = COALESCE($5, clip_filename),
            speed = COALESCE($6, speed),
            temperature = $7,
            ref_text = $8,
            instructions = $9,
            speaker = $10,
            seed = $11,
            top_p = $12,
            top_k = $13,
            repetition_penalty = $14,
            max_new_frames = $15,
            updated_at = NOW()
        WHERE id = $1 AND user_id = $2
        RETURNING {SELECT_COLS}
        "#
    );
    let row = sqlx::query_as::<_, UserVoicePreset>(sqlx::AssertSqlSafe(sql.as_str()))
        .bind(preset_id)
        .bind(user_id)
        .bind(&input.name)
        .bind(&input.voice)
        .bind(&input.clip_filename)
        .bind(input.speed)
        .bind(resolved.temperature)
        .bind(&resolved.ref_text)
        .bind(&resolved.instructions)
        .bind(&resolved.speaker)
        .bind(resolved.seed)
        .bind(resolved.top_p)
        .bind(resolved.top_k)
        .bind(resolved.repetition_penalty)
        .bind(resolved.max_new_frames)
        .fetch_optional(exec)
        .await?;
    Ok(row)
}

/// Delete a voice preset. Returns true if a row was deleted.
pub async fn delete_preset<'e, E>(exec: E, preset_id: Uuid, user_id: Uuid) -> Result<bool>
where
    E: Executor<'e, Database = Postgres>,
{
    let result = sqlx::query("DELETE FROM user_voice_presets WHERE id = $1 AND user_id = $2")
        .bind(preset_id)
        .bind(user_id)
        .execute(exec)
        .await?;

    Ok(result.rows_affected() > 0)
}

// =============================================================================
// Active Preset
// =============================================================================

/// Get the user's active voice preset (via users.active_voice_preset_id).
pub async fn get_active_preset<'e, E>(exec: E, user_id: Uuid) -> Result<Option<UserVoicePreset>>
where
    E: Executor<'e, Database = Postgres>,
{
    let row = sqlx::query_as::<_, UserVoicePreset>(
        r#"
        SELECT p.id, p.user_id, p.name, p.voice, p.clip_filename, p.speed,
               p.temperature, p.ref_text, p.instructions, p.speaker, p.seed,
               p.top_p, p.top_k, p.repetition_penalty, p.max_new_frames,
               p.created_at, p.updated_at
        FROM users u
        JOIN user_voice_presets p ON p.id = u.active_voice_preset_id
        WHERE u.id = $1
        "#,
    )
    .bind(user_id)
    .fetch_optional(exec)
    .await?;
    Ok(row)
}

/// Set or clear the user's active voice preset.
/// Pass `None` to clear the active preset.
pub async fn set_active_preset<'e, E>(
    exec: E,
    user_id: Uuid,
    preset_id: Option<Uuid>,
) -> Result<bool>
where
    E: Executor<'e, Database = Postgres>,
{
    let result = sqlx::query("UPDATE users SET active_voice_preset_id = $2 WHERE id = $1")
        .bind(user_id)
        .bind(preset_id)
        .execute(exec)
        .await?;

    Ok(result.rows_affected() > 0)
}
