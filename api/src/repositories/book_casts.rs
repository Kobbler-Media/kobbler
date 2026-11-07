use serde::Serialize;
use serde_json::Value;
use sqlx::{Executor, PgPool, Postgres};
use time::OffsetDateTime;
use uuid::Uuid;

use super::{Result, book_cast_vetting};
use crate::book_cast::carry;
use crate::book_cast_vetting::seed;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum CastState {
    Chapters,
    Bible,
    Voices,
    Casting,
    Rendering,
    Done,
    Failed,
}

impl CastState {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Chapters => "chapters",
            Self::Bible => "bible",
            Self::Voices => "voices",
            Self::Casting => "casting",
            Self::Rendering => "rendering",
            Self::Done => "done",
            Self::Failed => "failed",
        }
    }

    fn parse(s: &str) -> Self {
        match s {
            "chapters" => Self::Chapters,
            "bible" => Self::Bible,
            "voices" => Self::Voices,
            "casting" => Self::Casting,
            "rendering" => Self::Rendering,
            "done" => Self::Done,
            _ => Self::Failed,
        }
    }
}

#[derive(Debug, Serialize)]
pub struct BookCast {
    pub book_id: Uuid,
    pub book_file_id: Uuid,
    pub owner_id: Uuid,
    pub state: CastState,
    pub error: Option<String>,
    #[serde(with = "time::serde::rfc3339")]
    pub analysis_started_at: OffsetDateTime,
    #[serde(with = "time::serde::rfc3339")]
    pub updated_at: OffsetDateTime,
}

#[derive(Debug, Serialize)]
pub struct CastChapter {
    pub chapter_index: i32,
    pub title: Option<String>,
    pub first_segment: i32,
    pub last_segment: i32,
    pub notes: Value,
}

#[derive(Debug, Clone, Serialize)]
pub struct CastLine {
    pub segment_index: i32,
    pub span_start: i32,
    pub span_end: i32,
    pub chapter_index: i32,
    pub speaker: Option<String>,
    pub character_id: Option<Uuid>,
    pub cue: Option<String>,
    pub source: String,
    pub not_speech: bool,
}

impl CastLine {
    pub fn spoken(&self) -> bool {
        !self.not_speech
    }

    /// A line made narration by hand has been decided, so it waits for nobody.
    pub fn needs_speaker(&self) -> bool {
        self.character_id.is_none() && !self.not_speech && self.source != "user"
    }
}

#[derive(Clone)]
pub struct NewCharacter {
    pub name: String,
    pub aliases: Vec<String>,
    pub generic: bool,
    pub bible: Value,
}

pub struct Reattribution {
    pub segment_index: i32,
    pub span_start: i32,
    pub speaker: String,
}

/// Starts the cast over: everything a previous cast of the book worked out
/// goes, its presets with it. The audiobook it made stays in the library, and
/// stays the one the next render replaces.
pub async fn start(pool: &PgPool, book_id: Uuid, book_file_id: Uuid, owner_id: Uuid) -> Result<()> {
    let mut tx = pool.begin().await?;
    let audiobook = sqlx::query_scalar!(
        "DELETE FROM book_casts WHERE book_id = $1 RETURNING audiobook_file_id",
        book_id
    )
    .fetch_optional(&mut *tx)
    .await?
    .flatten();
    sqlx::query!(
        r#"INSERT INTO book_casts (book_id, book_file_id, owner_id, state, audiobook_file_id)
           VALUES ($1, $2, $3, 'chapters', $4)"#,
        book_id,
        book_file_id,
        owner_id,
        audiobook
    )
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(())
}

/// Runs the analysis again from `from` (the chapter or the cast pass), keeping
/// everything vetted. False unless the cast is past analysis; the caller sees
/// that no render is running.
pub async fn rerun(pool: &PgPool, book_id: Uuid, from: CastState) -> Result<bool> {
    let mut tx = pool.begin().await?;
    let moved = sqlx::query!(
        r#"UPDATE book_casts SET state = $2, error = NULL, updated_at = NOW(),
               analysis_started_at = NOW()
           WHERE book_id = $1 AND state IN ('casting', 'rendering', 'done', 'failed')"#,
        book_id,
        from.as_str()
    )
    .execute(&mut *tx)
    .await?
    .rows_affected()
        > 0;
    if moved && from == CastState::Chapters {
        sqlx::query!("DELETE FROM book_cast_chapters WHERE book_id = $1", book_id)
            .execute(&mut *tx)
            .await?;
    }
    tx.commit().await?;
    Ok(moved)
}

pub async fn get<'e, E>(exec: E, book_id: Uuid) -> Result<Option<BookCast>>
where
    E: Executor<'e, Database = Postgres>,
{
    let row = sqlx::query!(
        r#"SELECT book_id, book_file_id, owner_id, state, error, analysis_started_at, updated_at
           FROM book_casts WHERE book_id = $1"#,
        book_id
    )
    .fetch_optional(exec)
    .await?;
    Ok(row.map(|r| BookCast {
        book_id: r.book_id,
        book_file_id: r.book_file_id,
        owner_id: r.owner_id,
        state: CastState::parse(&r.state),
        error: r.error,
        analysis_started_at: r.analysis_started_at,
        updated_at: r.updated_at,
    }))
}

pub async fn set_state<'e, E>(
    exec: E,
    book_id: Uuid,
    state: CastState,
    error: Option<&str>,
) -> Result<()>
where
    E: Executor<'e, Database = Postgres>,
{
    sqlx::query!(
        "UPDATE book_casts SET state = $2, error = $3, updated_at = NOW() WHERE book_id = $1",
        book_id,
        state.as_str(),
        error
    )
    .execute(exec)
    .await?;
    Ok(())
}

/// One run of the analysis: a pass holds it only while the cast is still in
/// its state and was not started again since, so a run a redo or rerun replaced
/// can neither write to the cast nor move it on.
#[derive(Debug, Clone, Copy)]
pub struct Run {
    pub book_id: Uuid,
    pub state: CastState,
    pub started: OffsetDateTime,
}

impl Run {
    pub fn of(cast: &BookCast) -> Self {
        Self {
            book_id: cast.book_id,
            state: cast.state,
            started: cast.analysis_started_at,
        }
    }

    /// Locks the cast's row for the transaction, so a redo or rerun waits for
    /// the run's write rather than landing in the middle of it.
    pub async fn hold(&self, tx: &mut sqlx::PgConnection) -> Result<bool> {
        Ok(sqlx::query_scalar!(
            r#"SELECT 1 AS "held!" FROM book_casts
               WHERE book_id = $1 AND state = $2 AND analysis_started_at = $3
               FOR UPDATE"#,
            self.book_id,
            self.state.as_str(),
            self.started
        )
        .fetch_optional(&mut *tx)
        .await?
        .is_some())
    }
}

pub async fn settle<'e, E>(exec: E, run: &Run, to: CastState, error: Option<&str>) -> Result<bool>
where
    E: Executor<'e, Database = Postgres>,
{
    let done = sqlx::query!(
        r#"UPDATE book_casts SET state = $4, error = $5, updated_at = NOW()
           WHERE book_id = $1 AND state = $2 AND analysis_started_at = $3"#,
        run.book_id,
        run.state.as_str(),
        run.started,
        to.as_str(),
        error
    )
    .execute(exec)
    .await?;
    Ok(done.rows_affected() > 0)
}

pub struct ChapterTime {
    pub chapter_index: i32,
    pub finished_at: OffsetDateTime,
}

pub async fn chapter_times<'e, E>(exec: E, book_id: Uuid) -> Result<Vec<ChapterTime>>
where
    E: Executor<'e, Database = Postgres>,
{
    Ok(sqlx::query_as!(
        ChapterTime,
        r#"SELECT chapter_index, finished_at FROM book_cast_chapters
           WHERE book_id = $1 ORDER BY chapter_index"#,
        book_id
    )
    .fetch_all(exec)
    .await?)
}

/// One chapter's attribution, written whole so a resumed pass never finds half
/// a chapter, and replacing what an earlier run found in it. A line someone
/// corrected by hand keeps their speaker.
pub async fn save_chapter(
    pool: &PgPool,
    run: &Run,
    chapter: &CastChapter,
    model: &str,
    lines: &[CastLine],
) -> Result<bool> {
    let book_id = run.book_id;
    let mut tx = pool.begin().await?;
    if !run.hold(&mut tx).await? {
        return Ok(false);
    }
    sqlx::query!(
        r#"
        INSERT INTO book_cast_chapters (book_id, chapter_index, title, first_segment, last_segment, notes, model)
        VALUES ($1, $2, $3, $4, $5, $6, $7)
        ON CONFLICT (book_id, chapter_index) DO UPDATE SET
            title = EXCLUDED.title, first_segment = EXCLUDED.first_segment,
            last_segment = EXCLUDED.last_segment, notes = EXCLUDED.notes,
            model = EXCLUDED.model, finished_at = NOW()
        "#,
        book_id,
        chapter.chapter_index,
        chapter.title,
        chapter.first_segment,
        chapter.last_segment,
        chapter.notes,
        model
    )
    .execute(&mut *tx)
    .await?;
    let segments: Vec<i32> = lines.iter().map(|l| l.segment_index).collect();
    let starts: Vec<i32> = lines.iter().map(|l| l.span_start).collect();
    sqlx::query!(
        r#"DELETE FROM book_cast_lines
           WHERE book_id = $1 AND chapter_index = $2 AND source <> 'user'
             AND (segment_index, span_start) NOT IN
                 (SELECT * FROM unnest($3::int[], $4::int[]))"#,
        book_id,
        chapter.chapter_index,
        &segments,
        &starts
    )
    .execute(&mut *tx)
    .await?;
    for line in lines {
        sqlx::query!(
            r#"
            INSERT INTO book_cast_lines
                (book_id, segment_index, span_start, span_end, chapter_index, speaker, cue, source,
                 not_speech)
            VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9)
            ON CONFLICT (book_id, segment_index, span_start) DO UPDATE SET
                span_end = EXCLUDED.span_end, speaker = EXCLUDED.speaker, cue = EXCLUDED.cue,
                source = EXCLUDED.source, not_speech = EXCLUDED.not_speech,
                character_id = CASE WHEN EXCLUDED.not_speech THEN NULL
                                    ELSE book_cast_lines.character_id END
            WHERE book_cast_lines.source <> 'user'
            "#,
            book_id,
            line.segment_index,
            line.span_start,
            line.span_end,
            line.chapter_index,
            line.speaker,
            line.cue,
            line.source,
            line.not_speech
        )
        .execute(&mut *tx)
        .await?;
    }
    sqlx::query!(
        "UPDATE book_casts SET updated_at = NOW() WHERE book_id = $1",
        book_id
    )
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(true)
}

pub async fn chapters<'e, E>(exec: E, book_id: Uuid) -> Result<Vec<CastChapter>>
where
    E: Executor<'e, Database = Postgres>,
{
    Ok(sqlx::query_as!(
        CastChapter,
        r#"SELECT chapter_index, title, first_segment, last_segment, notes
           FROM book_cast_chapters WHERE book_id = $1 ORDER BY chapter_index"#,
        book_id
    )
    .fetch_all(exec)
    .await?)
}

pub async fn lines<'e, E>(exec: E, book_id: Uuid, chapter: Option<i32>) -> Result<Vec<CastLine>>
where
    E: Executor<'e, Database = Postgres>,
{
    Ok(sqlx::query_as!(
        CastLine,
        r#"SELECT segment_index, span_start, span_end, chapter_index, speaker, character_id,
                  cue, source, not_speech
           FROM book_cast_lines
           WHERE book_id = $1 AND ($2::int IS NULL OR chapter_index = $2)
           ORDER BY segment_index, span_start"#,
        book_id,
        chapter
    )
    .fetch_all(exec)
    .await?)
}

/// The bible pass's result, replacing any earlier one. Every chapter-pass
/// speaker name a character claims (its own or an alias, case-insensitively)
/// is resolved to it; a name no character claims stays unresolved.
///
/// A rerun keeps what was vetted: a character found again (see
/// [`carry::place`]) keeps its id, and so its voices, its hand-corrected lines
/// and the edits laid over its bible, while a voice nobody touched goes for the
/// voice pass to design afresh. A character no longer found goes with all of it.
pub async fn replace_characters(
    pool: &PgPool,
    run: &Run,
    characters: &[NewCharacter],
    reattributed: &[Reattribution],
    pool_below: i32,
) -> Result<bool> {
    let book_id = run.book_id;
    let mut tx = pool.begin().await?;
    if !run.hold(&mut tx).await? {
        return Ok(false);
    }
    let old = book_cast_vetting::characters(&mut *tx, book_id).await?;
    let old_voices = book_cast_vetting::voices(&mut *tx, book_id).await?;
    let known: Vec<carry::Known> = old
        .iter()
        .map(|c| carry::Known {
            id: c.id,
            name: c.name.clone(),
            aliases: c.aliases.clone(),
            generic: c.generic,
            narrator: c.narrator,
        })
        .collect();
    let placed = carry::place(&known, characters.to_vec());
    let kept: Vec<Uuid> = placed.iter().filter_map(|p| p.id).collect();
    sqlx::query!(
        "DELETE FROM book_cast_characters WHERE book_id = $1 AND id <> ALL($2)",
        book_id,
        &kept
    )
    .execute(&mut *tx)
    .await?;
    for (character_id, from_chapter) in seed::untouched(&old, &old_voices) {
        book_cast_vetting::delete_voice(&mut *tx, character_id, from_chapter).await?;
    }
    for r in reattributed {
        sqlx::query!(
            r#"UPDATE book_cast_lines SET speaker = $4, source = 'beefy'
               WHERE book_id = $1 AND segment_index = $2 AND span_start = $3 AND source <> 'user'"#,
            book_id,
            r.segment_index,
            r.span_start,
            r.speaker
        )
        .execute(&mut *tx)
        .await?;
    }
    sqlx::query!(
        "UPDATE book_cast_lines SET character_id = NULL WHERE book_id = $1 AND source <> 'user'",
        book_id
    )
    .execute(&mut *tx)
    .await?;
    for p in &placed {
        let c = &p.character;
        let id = match p.id {
            Some(id) => {
                sqlx::query!(
                    r#"UPDATE book_cast_characters SET
                           aliases = $2,
                           bible = jsonb_strip_nulls($3 || COALESCE(edits->'bible', '{}'::jsonb)),
                           generic = NOT narrator AND COALESCE((edits->>'generic')::boolean, $4)
                       WHERE id = $1"#,
                    id,
                    &c.aliases,
                    c.bible,
                    c.generic
                )
                .execute(&mut *tx)
                .await?;
                id
            }
            None => {
                let id = Uuid::now_v7();
                sqlx::query!(
                    "INSERT INTO book_cast_characters (id, book_id, name, aliases, bible, generic) VALUES ($1, $2, $3, $4, $5, $6)",
                    id,
                    book_id,
                    c.name,
                    &c.aliases,
                    c.bible,
                    c.generic
                )
                .execute(&mut *tx)
                .await?;
                id
            }
        };
        let mut names: Vec<String> = c.aliases.iter().map(|a| a.to_lowercase()).collect();
        names.push(c.name.to_lowercase());
        sqlx::query!(
            r#"UPDATE book_cast_lines SET character_id = $2
               WHERE book_id = $1 AND character_id IS NULL AND lower(speaker) = ANY($3)"#,
            book_id,
            id,
            &names
        )
        .execute(&mut *tx)
        .await?;
    }
    book_cast_vetting::recount(&mut tx, book_id, pool_below).await?;
    tx.commit().await?;
    Ok(true)
}

pub async fn set_voice<'e, E>(exec: E, character_id: Uuid, voice: &Value) -> Result<()>
where
    E: Executor<'e, Database = Postgres>,
{
    sqlx::query!(
        r#"UPDATE book_cast_characters
           SET bible = jsonb_strip_nulls(bible || $2 || COALESCE(edits->'bible', '{}'::jsonb))
           WHERE id = $1"#,
        character_id,
        voice
    )
    .execute(exec)
    .await?;
    Ok(())
}
