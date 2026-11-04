//! Audiobook transcript segment CRUD operations.

use serde::Serialize;
use sqlx::{Executor, Postgres};
use time::OffsetDateTime;
use uuid::Uuid;

use super::Result;

/// Input for inserting a transcript segment.
#[derive(Debug)]
pub struct TranscriptSegmentInput {
    pub segment_index: i32,
    pub start_seconds: f64,
    pub end_seconds: f64,
    pub text: String,
    pub avg_logprob: Option<f64>,
    pub no_speech_prob: Option<f64>,
    pub compression_ratio: Option<f64>,
}

/// A transcript segment row from the database.
#[derive(Debug)]
pub struct TranscriptSegment {
    pub id: Uuid,
    pub audiobook_file_id: Uuid,
    pub segment_index: i32,
    pub start_seconds: f64,
    pub end_seconds: f64,
    pub text: String,
    pub avg_logprob: Option<f64>,
    pub no_speech_prob: Option<f64>,
    pub compression_ratio: Option<f64>,
    pub created_at: OffsetDateTime,
}

/// Insert multiple transcript segments for an audiobook file.
pub async fn insert_segments<'e, E>(
    exec: E,
    audiobook_file_id: Uuid,
    segments: &[TranscriptSegmentInput],
    config_tag: Option<&str>,
) -> Result<()>
where
    E: Executor<'e, Database = Postgres>,
{
    let ids: Vec<Uuid> = segments.iter().map(|_| Uuid::now_v7()).collect();
    let file_ids: Vec<Uuid> = segments.iter().map(|_| audiobook_file_id).collect();
    let indices: Vec<i32> = segments.iter().map(|s| s.segment_index).collect();
    let starts: Vec<f64> = segments.iter().map(|s| s.start_seconds).collect();
    let ends: Vec<f64> = segments.iter().map(|s| s.end_seconds).collect();
    let texts: Vec<String> = segments.iter().map(|s| s.text.clone()).collect();
    let avg_logprobs: Vec<Option<f64>> = segments.iter().map(|s| s.avg_logprob).collect();
    let no_speech_probs: Vec<Option<f64>> = segments.iter().map(|s| s.no_speech_prob).collect();
    let compression_ratios: Vec<Option<f64>> =
        segments.iter().map(|s| s.compression_ratio).collect();
    let config_tags: Vec<Option<String>> = segments
        .iter()
        .map(|_| config_tag.map(String::from))
        .collect();

    sqlx::query!(
        r#"
        INSERT INTO audiobook_transcripts (id, audiobook_file_id, segment_index, start_seconds, end_seconds, text, avg_logprob, no_speech_prob, compression_ratio, config_tag)
        SELECT * FROM UNNEST($1::uuid[], $2::uuid[], $3::int[], $4::float8[], $5::float8[], $6::text[], $7::float8[], $8::float8[], $9::float8[], $10::text[])
        ON CONFLICT (audiobook_file_id, segment_index) DO UPDATE SET
            start_seconds = EXCLUDED.start_seconds,
            end_seconds = EXCLUDED.end_seconds,
            text = EXCLUDED.text,
            avg_logprob = EXCLUDED.avg_logprob,
            no_speech_prob = EXCLUDED.no_speech_prob,
            compression_ratio = EXCLUDED.compression_ratio,
            config_tag = EXCLUDED.config_tag
        "#,
        &ids,
        &file_ids,
        &indices,
        &starts,
        &ends,
        &texts,
        &avg_logprobs as &[Option<f64>],
        &no_speech_probs as &[Option<f64>],
        &compression_ratios as &[Option<f64>],
        &config_tags as &[Option<String>],
    )
    .execute(exec)
    .await?;

    Ok(())
}

/// Delete all transcript segments for an audiobook file.
pub async fn delete_for_file<'e, E>(exec: E, audiobook_file_id: Uuid) -> Result<()>
where
    E: Executor<'e, Database = Postgres>,
{
    sqlx::query!(
        "DELETE FROM audiobook_transcripts WHERE audiobook_file_id = $1",
        audiobook_file_id,
    )
    .execute(exec)
    .await?;
    Ok(())
}

/// Get all transcript segments for an audiobook file, ordered by segment index.
pub async fn get_for_file<'e, E>(exec: E, audiobook_file_id: Uuid) -> Result<Vec<TranscriptSegment>>
where
    E: Executor<'e, Database = Postgres>,
{
    let rows = sqlx::query_as!(
        TranscriptSegment,
        r#"
        SELECT id, audiobook_file_id, segment_index, start_seconds, end_seconds, text,
               avg_logprob, no_speech_prob, compression_ratio, created_at
        FROM audiobook_transcripts
        WHERE audiobook_file_id = $1
        ORDER BY segment_index
        "#,
        audiobook_file_id,
    )
    .fetch_all(exec)
    .await?;
    Ok(rows)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TranscriptState {
    Current,
    Outdated,
    Missing,
}

pub async fn transcript_state<'e, E>(
    exec: E,
    audiobook_file_id: Uuid,
    config_tag: &str,
) -> Result<TranscriptState>
where
    E: Executor<'e, Database = Postgres>,
{
    let row = sqlx::query!(
        r#"
        SELECT COALESCE(BOOL_OR(config_tag = $2), FALSE) AS "current!", COUNT(*) AS "segments!"
        FROM audiobook_transcripts
        WHERE audiobook_file_id = $1
        "#,
        audiobook_file_id,
        config_tag,
    )
    .fetch_one(exec)
    .await?;
    Ok(match (row.current, row.segments) {
        (true, _) => TranscriptState::Current,
        (false, 0) => TranscriptState::Missing,
        (false, _) => TranscriptState::Outdated,
    })
}

/// A book with at least one available audio file lacking a `config_tag`
/// transcript, with the total duration of its available audio.
pub struct StaleBook {
    pub id: Uuid,
    pub title: String,
    pub seconds: f64,
}

/// Books with an available file that has no transcript at all, plus, with
/// `include_outdated`, those whose transcript predates `config_tag`.
pub async fn stale_books<'e, E>(
    exec: E,
    config_tag: &str,
    include_outdated: bool,
) -> Result<Vec<StaleBook>>
where
    E: Executor<'e, Database = Postgres>,
{
    let rows = sqlx::query_as!(
        StaleBook,
        r#"
            SELECT b.id, b.title, COALESCE(SUM(af.duration_seconds), 0) AS "seconds!"
            FROM books b
            JOIN audiobook_files af ON af.book_id = b.id
            WHERE af.available = TRUE
              AND EXISTS (
                  SELECT 1 FROM media_root_files mrf
                  WHERE (mrf.file_path = af.file_path OR mrf.file_path = af.source_path)
                    AND mrf.available = TRUE
              )
              AND NOT EXISTS (
                  SELECT 1 FROM audiobook_transcripts at
                  WHERE at.audiobook_file_id = af.id AND at.config_tag = $1
              )
              AND ($2 OR NOT EXISTS (
                  SELECT 1 FROM audiobook_transcripts at
                  WHERE at.audiobook_file_id = af.id
              ))
            GROUP BY b.id, b.title
            ORDER BY b.id
            "#,
        config_tag,
        include_outdated,
    )
    .fetch_all(exec)
    .await?;
    Ok(rows)
}

#[derive(Debug, Default, Serialize)]
pub struct BacklogTotals {
    pub books: i64,
    pub files: i64,
    pub hours: f64,
}

#[derive(Debug, Serialize)]
pub struct TranscriptBacklog {
    pub missing: BacklogTotals,
    pub outdated: BacklogTotals,
}

pub async fn backlog<'e, E>(exec: E, config_tag: &str) -> Result<TranscriptBacklog>
where
    E: Executor<'e, Database = Postgres>,
{
    let row = sqlx::query!(
        r#"
        WITH pending AS (
            SELECT af.book_id, COALESCE(af.duration_seconds, 0) AS seconds,
                   EXISTS (
                       SELECT 1 FROM audiobook_transcripts at
                       WHERE at.audiobook_file_id = af.id
                   ) AS outdated
            FROM audiobook_files af
            WHERE af.available = TRUE
              AND EXISTS (
                  SELECT 1 FROM media_root_files mrf
                  WHERE (mrf.file_path = af.file_path OR mrf.file_path = af.source_path)
                    AND mrf.available = TRUE
              )
              AND NOT EXISTS (
                  SELECT 1 FROM audiobook_transcripts at
                  WHERE at.audiobook_file_id = af.id AND at.config_tag = $1
              )
        )
        SELECT
            COUNT(DISTINCT book_id) FILTER (WHERE NOT outdated) AS "missing_books!",
            COUNT(*) FILTER (WHERE NOT outdated) AS "missing_files!",
            COALESCE(SUM(seconds) FILTER (WHERE NOT outdated), 0) AS "missing_seconds!",
            COUNT(DISTINCT book_id) FILTER (WHERE outdated) AS "outdated_books!",
            COUNT(*) FILTER (WHERE outdated) AS "outdated_files!",
            COALESCE(SUM(seconds) FILTER (WHERE outdated), 0) AS "outdated_seconds!"
        FROM pending
        "#,
        config_tag,
    )
    .fetch_one(exec)
    .await?;
    Ok(TranscriptBacklog {
        missing: BacklogTotals {
            books: row.missing_books,
            files: row.missing_files,
            hours: row.missing_seconds / 3600.0,
        },
        outdated: BacklogTotals {
            books: row.outdated_books,
            files: row.outdated_files,
            hours: row.outdated_seconds / 3600.0,
        },
    })
}

/// Get all transcript segments for a book (across all audiobook files), ordered by file index then segment index.
pub async fn get_for_book<'e, E>(exec: E, book_id: Uuid) -> Result<Vec<TranscriptSegment>>
where
    E: Executor<'e, Database = Postgres>,
{
    let rows = sqlx::query_as!(
        TranscriptSegment,
        r#"
        SELECT t.id, t.audiobook_file_id, t.segment_index, t.start_seconds, t.end_seconds, t.text,
               t.avg_logprob, t.no_speech_prob, t.compression_ratio, t.created_at
        FROM audiobook_transcripts t
        JOIN audiobook_files af ON af.id = t.audiobook_file_id
        WHERE af.book_id = $1
        ORDER BY af.file_index, t.segment_index
        "#,
        book_id,
    )
    .fetch_all(exec)
    .await?;
    Ok(rows)
}

/// Check if any transcript segments exist for a book.
pub async fn has_transcript<'e, E>(exec: E, book_id: Uuid) -> Result<bool>
where
    E: Executor<'e, Database = Postgres>,
{
    let row = sqlx::query_scalar!(
        r#"
        SELECT EXISTS(
            SELECT 1 FROM audiobook_transcripts t
            JOIN audiobook_files af ON af.id = t.audiobook_file_id
            WHERE af.book_id = $1
        ) AS "exists!"
        "#,
        book_id,
    )
    .fetch_one(exec)
    .await?;
    Ok(row)
}

/// Get transcript segments within a time range for an audiobook file.
pub async fn get_in_range<'e, E>(
    exec: E,
    audiobook_file_id: Uuid,
    start: f64,
    end: f64,
) -> Result<Vec<TranscriptSegment>>
where
    E: Executor<'e, Database = Postgres>,
{
    let rows = sqlx::query_as!(
        TranscriptSegment,
        r#"
        SELECT id, audiobook_file_id, segment_index, start_seconds, end_seconds, text,
               avg_logprob, no_speech_prob, compression_ratio, created_at
        FROM audiobook_transcripts
        WHERE audiobook_file_id = $1 AND end_seconds >= $2 AND start_seconds <= $3
        ORDER BY segment_index
        "#,
        audiobook_file_id,
        start,
        end,
    )
    .fetch_all(exec)
    .await?;
    Ok(rows)
}
