use serde::Serialize;
use serde_json::Value;
use sqlx::{Executor, PgPool, Postgres};
use time::OffsetDateTime;
use uuid::Uuid;

use super::Result;

#[derive(Debug, Clone, Serialize)]
pub struct VettingCharacter {
    pub id: Uuid,
    pub name: String,
    pub aliases: Vec<String>,
    pub bible: Value,
    pub line_count: i32,
    pub pooled: bool,
    pub generic: bool,
    pub narrator: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct CastVoice {
    pub character_id: Uuid,
    pub from_chapter: i32,
    pub preset_id: Option<Uuid>,
    pub design_preset_id: Option<Uuid>,
    #[serde(with = "time::serde::rfc3339::option")]
    pub approved_at: Option<OffsetDateTime>,
    pub preset_name: Option<String>,
    pub has_clip: bool,
    pub description: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct UnattachedSpeaker {
    pub speaker: String,
    pub lines: i64,
}

#[derive(Debug, Serialize)]
pub struct PresetChoice {
    pub id: Uuid,
    pub name: String,
    pub has_clip: bool,
}

pub struct LineRef {
    pub segment_index: i32,
    pub span_start: i32,
}

pub async fn characters<'e, E>(exec: E, book_id: Uuid) -> Result<Vec<VettingCharacter>>
where
    E: Executor<'e, Database = Postgres>,
{
    Ok(sqlx::query_as!(
        VettingCharacter,
        r#"SELECT id, name, aliases, bible, line_count, pooled, generic, narrator
           FROM book_cast_characters WHERE book_id = $1
           ORDER BY line_count DESC, name"#,
        book_id
    )
    .fetch_all(exec)
    .await?)
}

pub async fn character<'e, E>(
    exec: E,
    book_id: Uuid,
    character_id: Uuid,
) -> Result<Option<VettingCharacter>>
where
    E: Executor<'e, Database = Postgres>,
{
    Ok(sqlx::query_as!(
        VettingCharacter,
        r#"SELECT id, name, aliases, bible, line_count, pooled, generic, narrator
           FROM book_cast_characters WHERE book_id = $1 AND id = $2"#,
        book_id,
        character_id
    )
    .fetch_optional(exec)
    .await?)
}

pub async fn voices<'e, E>(exec: E, book_id: Uuid) -> Result<Vec<CastVoice>>
where
    E: Executor<'e, Database = Postgres>,
{
    Ok(sqlx::query_as!(
        CastVoice,
        r#"SELECT v.character_id, v.from_chapter, v.preset_id, v.design_preset_id, v.approved_at,
                  p.name AS "preset_name?", (p.clip_filename IS NOT NULL) AS "has_clip!",
                  d.instructions AS "description?"
           FROM book_cast_voices v
           JOIN book_cast_characters c ON c.id = v.character_id
           LEFT JOIN user_voice_presets p ON p.id = v.preset_id
           LEFT JOIN user_voice_presets d ON d.id = v.design_preset_id
           WHERE c.book_id = $1
           ORDER BY v.character_id, v.from_chapter"#,
        book_id
    )
    .fetch_all(exec)
    .await?)
}

pub async fn preset_choices<'e, E>(exec: E, owner_id: Uuid) -> Result<Vec<PresetChoice>>
where
    E: Executor<'e, Database = Postgres>,
{
    Ok(sqlx::query_as!(
        PresetChoice,
        r#"SELECT id, name, (clip_filename IS NOT NULL) AS "has_clip!"
           FROM user_voice_presets WHERE user_id = $1 AND cast_book_id IS NULL
           ORDER BY name"#,
        owner_id
    )
    .fetch_all(exec)
    .await?)
}

/// A first-person book may already cast someone called "Narrator"; that
/// character becomes the narrator rather than colliding with a second one.
pub async fn ensure_narrator<'e, E>(exec: E, book_id: Uuid) -> Result<()>
where
    E: Executor<'e, Database = Postgres>,
{
    sqlx::query!(
        r#"
        WITH existing AS (
            SELECT id FROM book_cast_characters WHERE book_id = $1 AND narrator
        ), named AS (
            UPDATE book_cast_characters SET narrator = TRUE, generic = FALSE, pooled = FALSE
            WHERE book_id = $1 AND lower(name) = lower($3) AND NOT EXISTS (SELECT 1 FROM existing)
            RETURNING id
        )
        INSERT INTO book_cast_characters (id, book_id, name, narrator)
        SELECT $2, $1, $3, TRUE
        WHERE NOT EXISTS (SELECT 1 FROM existing) AND NOT EXISTS (SELECT 1 FROM named)
        "#,
        book_id,
        Uuid::now_v7(),
        crate::book_cast_vetting::NARRATOR_NAME
    )
    .execute(exec)
    .await?;
    Ok(())
}

pub async fn cast_preset_named<'e, E>(exec: E, book_id: Uuid, name: &str) -> Result<Option<Uuid>>
where
    E: Executor<'e, Database = Postgres>,
{
    Ok(sqlx::query_scalar!(
        "SELECT id FROM user_voice_presets WHERE cast_book_id = $1 AND name = $2 LIMIT 1",
        book_id,
        name
    )
    .fetch_optional(exec)
    .await?)
}

pub async fn create_cast_preset<'e, E>(
    exec: E,
    owner_id: Uuid,
    book_id: Uuid,
    name: &str,
    description: &str,
) -> Result<Uuid>
where
    E: Executor<'e, Database = Postgres>,
{
    Ok(sqlx::query_scalar!(
        r#"INSERT INTO user_voice_presets (user_id, name, instructions, cast_book_id)
           VALUES ($1, $2, $3, $4) RETURNING id"#,
        owner_id,
        name,
        description,
        book_id
    )
    .fetch_one(exec)
    .await?)
}

/// A pooled preset already approved for the speakers sharing it is approved
/// for a newcomer too: it is one voice, however many speak with it.
pub async fn insert_voice<'e, E>(
    exec: E,
    character_id: Uuid,
    from_chapter: i32,
    preset_id: Uuid,
) -> Result<()>
where
    E: Executor<'e, Database = Postgres>,
{
    sqlx::query!(
        r#"INSERT INTO book_cast_voices (character_id, from_chapter, preset_id, design_preset_id, approved_at)
           VALUES ($1, $2, $3, $3,
                   (SELECT max(approved_at) FROM book_cast_voices WHERE preset_id = $3))
           ON CONFLICT DO NOTHING"#,
        character_id,
        from_chapter,
        preset_id
    )
    .execute(exec)
    .await?;
    Ok(())
}

pub async fn delete_voice<'e, E>(exec: E, character_id: Uuid, from_chapter: i32) -> Result<bool>
where
    E: Executor<'e, Database = Postgres>,
{
    let done = sqlx::query!(
        "DELETE FROM book_cast_voices WHERE character_id = $1 AND from_chapter = $2",
        character_id,
        from_chapter
    )
    .execute(exec)
    .await?;
    Ok(done.rows_affected() > 0)
}

pub async fn delete_unused_cast_presets<'e, E>(exec: E, book_id: Uuid) -> Result<Vec<Uuid>>
where
    E: Executor<'e, Database = Postgres>,
{
    Ok(sqlx::query_scalar!(
        r#"DELETE FROM user_voice_presets p
           WHERE p.cast_book_id = $1
             AND NOT EXISTS (
                 SELECT 1 FROM book_cast_voices v
                 WHERE v.preset_id = p.id OR v.design_preset_id = p.id
             )
           RETURNING p.id"#,
        book_id
    )
    .fetch_all(exec)
    .await?)
}

pub async fn set_description<'e, E>(exec: E, preset_id: Uuid, description: &str) -> Result<()>
where
    E: Executor<'e, Database = Postgres>,
{
    sqlx::query!(
        "UPDATE user_voice_presets SET instructions = $2, updated_at = NOW() WHERE id = $1",
        preset_id,
        description
    )
    .execute(exec)
    .await?;
    Ok(())
}

/// `None` goes back to the voice designed for the step. Either way the step is
/// approved again.
pub async fn use_preset<'e, E>(
    exec: E,
    character_id: Uuid,
    from_chapter: i32,
    preset_id: Option<Uuid>,
) -> Result<()>
where
    E: Executor<'e, Database = Postgres>,
{
    sqlx::query!(
        r#"UPDATE book_cast_voices
           SET preset_id = COALESCE($3, design_preset_id), approved_at = NULL
           WHERE character_id = $1 AND from_chapter = $2"#,
        character_id,
        from_chapter,
        preset_id
    )
    .execute(exec)
    .await?;
    Ok(())
}

/// A pooled preset is one voice however many speakers share it, so approving
/// it or taking the approval back does so for all of them.
pub async fn set_approved<'e, E>(
    exec: E,
    book_id: Uuid,
    preset_id: Uuid,
    approved: bool,
) -> Result<()>
where
    E: Executor<'e, Database = Postgres>,
{
    sqlx::query!(
        r#"UPDATE book_cast_voices v
           SET approved_at = CASE WHEN $3 THEN NOW() END
           FROM book_cast_characters c
           WHERE c.id = v.character_id AND c.book_id = $1 AND v.preset_id = $2"#,
        book_id,
        preset_id,
        approved
    )
    .execute(exec)
    .await?;
    Ok(())
}

pub async fn preset_in_use<'e, E>(exec: E, preset_id: Uuid) -> Result<bool>
where
    E: Executor<'e, Database = Postgres>,
{
    Ok(sqlx::query_scalar!(
        r#"SELECT EXISTS(SELECT 1 FROM book_cast_voices WHERE preset_id = $1) AS "used!""#,
        preset_id
    )
    .fetch_one(exec)
    .await?)
}

pub async fn cast_clips<'e, E>(exec: E, book_id: Uuid) -> Result<Vec<Uuid>>
where
    E: Executor<'e, Database = Postgres>,
{
    Ok(sqlx::query_scalar!(
        "SELECT id FROM user_voice_presets WHERE cast_book_id = $1 AND clip_filename IS NOT NULL",
        book_id
    )
    .fetch_all(exec)
    .await?)
}

pub async fn set_take<'e, E>(exec: E, preset_id: Uuid, ref_text: &str, seed: i64) -> Result<()>
where
    E: Executor<'e, Database = Postgres>,
{
    sqlx::query!(
        "UPDATE user_voice_presets SET ref_text = $2, seed = $3, updated_at = NOW() WHERE id = $1",
        preset_id,
        ref_text,
        seed
    )
    .execute(exec)
    .await?;
    Ok(())
}

pub async fn clear_take<'e, E>(exec: E, preset_id: Uuid) -> Result<()>
where
    E: Executor<'e, Database = Postgres>,
{
    sqlx::query!(
        r#"UPDATE user_voice_presets
           SET clip_filename = NULL, ref_text = NULL, seed = NULL, updated_at = NOW()
           WHERE id = $1"#,
        preset_id
    )
    .execute(exec)
    .await?;
    Ok(())
}

pub async fn unattached_speakers<'e, E>(exec: E, book_id: Uuid) -> Result<Vec<UnattachedSpeaker>>
where
    E: Executor<'e, Database = Postgres>,
{
    Ok(sqlx::query_as!(
        UnattachedSpeaker,
        r#"SELECT speaker AS "speaker!", COUNT(*) AS "lines!"
           FROM book_cast_lines
           WHERE book_id = $1 AND character_id IS NULL AND speaker IS NOT NULL
           GROUP BY speaker ORDER BY COUNT(*) DESC, speaker"#,
        book_id
    )
    .fetch_all(exec)
    .await?)
}

/// Line counts follow every hand edit, and a speaker who crosses the pooling
/// line in either direction loses the voice that no longer fits; the next seed
/// gives them the right kind. A hand-set `edits.pooled` overrides the count.
pub(super) async fn recount(
    tx: &mut sqlx::PgConnection,
    book_id: Uuid,
    pool_below: i32,
) -> Result<()> {
    let flipped = sqlx::query!(
        r#"
        UPDATE book_cast_characters c SET
            line_count = n.count::int,
            pooled = NOT c.generic AND NOT c.narrator
                AND COALESCE((c.edits->>'pooled')::boolean, n.count < $2)
        FROM (
            SELECT ch.id, ch.pooled AS was, COUNT(l.character_id) AS count
            FROM book_cast_characters ch
            LEFT JOIN book_cast_lines l ON l.character_id = ch.id
            WHERE ch.book_id = $1
            GROUP BY ch.id
        ) n
        WHERE c.id = n.id
        RETURNING c.id, (c.pooled <> n.was) AS "flipped!"
        "#,
        book_id,
        i64::from(pool_below)
    )
    .fetch_all(&mut *tx)
    .await?;
    let flipped: Vec<Uuid> = flipped
        .into_iter()
        .filter(|r| r.flipped)
        .map(|r| r.id)
        .collect();
    sqlx::query!(
        "DELETE FROM book_cast_voices WHERE character_id = ANY($1)",
        &flipped
    )
    .execute(&mut *tx)
    .await?;
    Ok(())
}

pub enum Said<'a> {
    By(&'a VettingCharacter),
    Narration,
    NotSpeech,
}

/// A line corrected by hand keeps its speaker through any later analysis rerun.
pub async fn reassign_line(
    pool: &PgPool,
    book_id: Uuid,
    line: &LineRef,
    said: Said<'_>,
    pool_below: i32,
) -> Result<bool> {
    let by = match said {
        Said::By(c) => Some(c),
        Said::Narration | Said::NotSpeech => None,
    };
    let mut tx = pool.begin().await?;
    let done = sqlx::query!(
        r#"UPDATE book_cast_lines
           SET character_id = $4, speaker = $5, not_speech = $6, source = 'user'
           WHERE book_id = $1 AND segment_index = $2 AND span_start = $3"#,
        book_id,
        line.segment_index,
        line.span_start,
        by.map(|c| c.id),
        by.map(|c| c.name.as_str()),
        matches!(said, Said::NotSpeech)
    )
    .execute(&mut *tx)
    .await?;
    if done.rows_affected() == 0 {
        return Ok(false);
    }
    recount(&mut tx, book_id, pool_below).await?;
    tx.commit().await?;
    Ok(true)
}

/// The survivor takes the other's lines, names and any voice step it has no
/// step of its own for; its own bible wins where both say something. Both are
/// read as they stand now, not as the caller last saw them.
pub async fn merge(
    pool: &PgPool,
    book_id: Uuid,
    survivor: Uuid,
    absorbed: Uuid,
    pool_below: i32,
) -> Result<()> {
    let mut tx = pool.begin().await?;
    sqlx::query!(
        "UPDATE book_cast_lines SET character_id = $2 WHERE character_id = $1",
        absorbed,
        survivor
    )
    .execute(&mut *tx)
    .await?;
    sqlx::query!(
        r#"UPDATE book_cast_voices SET character_id = $2
           WHERE character_id = $1 AND from_chapter NOT IN
               (SELECT from_chapter FROM book_cast_voices WHERE character_id = $2)"#,
        absorbed,
        survivor
    )
    .execute(&mut *tx)
    .await?;
    sqlx::query!(
        r#"UPDATE book_cast_characters s SET
               aliases = s.aliases || ARRAY(
                   SELECT n FROM (
                       SELECT DISTINCT ON (lower(n)) n, i
                       FROM unnest(array_prepend(a.name, a.aliases)) WITH ORDINALITY AS t(n, i)
                       WHERE lower(n) <> lower(s.name)
                         AND lower(n) <> ALL(SELECT lower(x) FROM unnest(s.aliases) x)
                       ORDER BY lower(n), i
                   ) names ORDER BY i
               ),
               bible = a.bible || s.bible,
               edits = a.edits || s.edits || jsonb_build_object('bible',
                   COALESCE(a.edits->'bible', '{}'::jsonb) || COALESCE(s.edits->'bible', '{}'::jsonb))
           FROM book_cast_characters a
           WHERE s.id = $1 AND a.id = $2"#,
        survivor,
        absorbed
    )
    .execute(&mut *tx)
    .await?;
    sqlx::query!("DELETE FROM book_cast_characters WHERE id = $1", absorbed)
        .execute(&mut *tx)
        .await?;
    recount(&mut tx, book_id, pool_below).await?;
    tx.commit().await?;
    Ok(())
}

pub struct Split<'a> {
    pub name: &'a str,
    pub aliases: &'a [String],
    pub lines: &'a [LineRef],
}

/// A new character from some of another's lines and names. Every line has to
/// be the source character's: `Ok(None)` when one is not.
pub async fn split(
    pool: &PgPool,
    book_id: Uuid,
    from: &VettingCharacter,
    split: &Split<'_>,
    pool_below: i32,
) -> Result<Option<Uuid>> {
    let mut tx = pool.begin().await?;
    let id = insert_character(&mut tx, book_id, split.name, split.aliases).await?;
    drop_aliases(&mut tx, from.id, split.aliases).await?;
    for line in split.lines {
        let moved = sqlx::query!(
            r#"UPDATE book_cast_lines SET character_id = $5, speaker = $6, source = 'user'
               WHERE book_id = $1 AND segment_index = $2 AND span_start = $3
                 AND character_id = $4"#,
            book_id,
            line.segment_index,
            line.span_start,
            from.id,
            id,
            split.name
        )
        .execute(&mut *tx)
        .await?;
        if moved.rows_affected() == 0 {
            return Ok(None);
        }
    }
    recount(&mut tx, book_id, pool_below).await?;
    tx.commit().await?;
    Ok(Some(id))
}

async fn insert_character(
    tx: &mut sqlx::PgConnection,
    book_id: Uuid,
    name: &str,
    aliases: &[String],
) -> Result<Uuid> {
    let id = Uuid::now_v7();
    sqlx::query!(
        "INSERT INTO book_cast_characters (id, book_id, name, aliases) VALUES ($1, $2, $3, $4)",
        id,
        book_id,
        name,
        aliases
    )
    .execute(&mut *tx)
    .await?;
    Ok(id)
}

async fn drop_aliases(
    tx: &mut sqlx::PgConnection,
    character_id: Uuid,
    names: &[String],
) -> Result<()> {
    let lower: Vec<String> = names.iter().map(|a| a.to_lowercase()).collect();
    sqlx::query!(
        r#"UPDATE book_cast_characters
           SET aliases = ARRAY(SELECT a FROM unnest(aliases) a WHERE lower(a) <> ALL($2))
           WHERE id = $1"#,
        character_id,
        &lower
    )
    .execute(&mut *tx)
    .await?;
    Ok(())
}

async fn claim_name(tx: &mut sqlx::PgConnection, to: &VettingCharacter, name: &str) -> Result<()> {
    let known = name.to_lowercase();
    if to.name.to_lowercase() == known || to.aliases.iter().any(|a| a.to_lowercase() == known) {
        return Ok(());
    }
    sqlx::query!(
        "UPDATE book_cast_characters SET aliases = array_append(aliases, $2) WHERE id = $1",
        to.id,
        name
    )
    .execute(&mut *tx)
    .await?;
    Ok(())
}

pub enum NameTo<'a> {
    Existing(&'a VettingCharacter),
    New(&'a str),
}

/// One of a character's aliases goes to another character, with every line
/// pass 1 gave that name, bar those placed by hand. The lines stay the
/// analysis's, so the name can be moved again.
pub async fn move_name(
    pool: &PgPool,
    book_id: Uuid,
    from: &VettingCharacter,
    name: &str,
    to: NameTo<'_>,
    pool_below: i32,
) -> Result<Uuid> {
    let mut tx = pool.begin().await?;
    let id = match to {
        NameTo::Existing(c) => {
            claim_name(&mut tx, c, name).await?;
            c.id
        }
        NameTo::New(new_name) => {
            let aliases: Vec<String> = (new_name.to_lowercase() != name.to_lowercase())
                .then(|| name.to_string())
                .into_iter()
                .collect();
            insert_character(&mut tx, book_id, new_name, &aliases).await?
        }
    };
    drop_aliases(&mut tx, from.id, &[name.to_string()]).await?;
    sqlx::query!(
        r#"UPDATE book_cast_lines SET character_id = $3
           WHERE book_id = $1 AND lower(speaker) = lower($2) AND source <> 'user'"#,
        book_id,
        name,
        id
    )
    .execute(&mut *tx)
    .await?;
    recount(&mut tx, book_id, pool_below).await?;
    tx.commit().await?;
    Ok(id)
}

/// Gives a speaker name no character claimed to one, lines and all.
pub async fn attach_speaker(
    pool: &PgPool,
    book_id: Uuid,
    speaker: &str,
    to: &VettingCharacter,
    pool_below: i32,
) -> Result<u64> {
    let mut tx = pool.begin().await?;
    let attached = sqlx::query!(
        r#"UPDATE book_cast_lines SET character_id = $3, speaker = $4, source = 'user'
           WHERE book_id = $1 AND character_id IS NULL AND lower(speaker) = lower($2)"#,
        book_id,
        speaker,
        to.id,
        to.name
    )
    .execute(&mut *tx)
    .await?
    .rows_affected();
    if attached == 0 {
        return Ok(0);
    }
    claim_name(&mut tx, to, speaker).await?;
    recount(&mut tx, book_id, pool_below).await?;
    tx.commit().await?;
    Ok(attached)
}

pub struct CharacterEdit {
    pub name: Option<String>,
    pub aliases: Option<Vec<String>>,
    pub bible: Option<Value>,
    pub generic: Option<bool>,
    /// Kept only in `edits`: `recount` decides the column from it.
    pub pooled: Option<bool>,
}

/// `bible` is merged into what is there; a key set to null is removed. The
/// edit is also kept apart, to lay over the analysis when it is run again, and
/// a renamed character keeps its old name as an alias so a rerun finds it.
pub async fn update_character(
    pool: &PgPool,
    book_id: Uuid,
    character_id: Uuid,
    edit: &CharacterEdit,
    pool_below: i32,
) -> Result<()> {
    let mut tx = pool.begin().await?;
    sqlx::query!(
        r#"UPDATE book_cast_characters SET
               name = COALESCE($2, name),
               aliases = CASE
                   WHEN $3::text[] IS NOT NULL THEN $3
                   WHEN $2::text IS NOT NULL AND lower($2) <> lower(name)
                       AND lower(name) <> ALL(SELECT lower(a) FROM unnest(aliases) a)
                       THEN array_append(array_remove(aliases, $2), name)
                   ELSE aliases
               END,
               bible = jsonb_strip_nulls(bible || COALESCE($4, '{}'::jsonb)),
               generic = COALESCE($5, generic),
               edits = edits
                   || CASE WHEN $5::boolean IS NULL THEN '{}'::jsonb
                           ELSE jsonb_build_object('generic', $5) END
                   || CASE WHEN $6::boolean IS NULL THEN '{}'::jsonb
                           ELSE jsonb_build_object('pooled', $6) END
                   || CASE WHEN $4::jsonb IS NULL THEN '{}'::jsonb
                           ELSE jsonb_build_object('bible',
                               COALESCE(edits->'bible', '{}'::jsonb) || $4) END
           WHERE id = $1"#,
        character_id,
        edit.name,
        edit.aliases.as_deref(),
        edit.bible,
        edit.generic,
        edit.pooled
    )
    .execute(&mut *tx)
    .await?;
    recount(&mut tx, book_id, pool_below).await?;
    tx.commit().await?;
    Ok(())
}
